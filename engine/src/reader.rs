use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use ethos_core::{LoadOutcome, MAX_PAGES, PageInfo, Rect, SearchHit, TextRun, find_hit};
use pdfium_render::prelude::*;

const MAX_CACHED_CHARACTERS: usize = 4_000_000;

/// A bitmap request for one page at a final, rotated pixel size.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub token: u64,
    pub source: u32,
    pub rotation: u16,
    pub width: u32,
    pub height: u32,
}

type Reply<T> = Sender<Result<T, String>>;
type LibraryWork = Box<dyn FnOnce(Result<&Pdfium, String>) + Send>;
type TemporaryWork = Box<dyn for<'d> FnOnce(Result<&PdfDocument<'d>, String>) + Send>;

enum Job {
    Load {
        token: u64,
        bytes: Vec<u8>,
        password: Option<String>,
        reply: Reply<LoadOutcome>,
    },
    Render {
        ticket: u64,
        frame: Frame,
        reply: Reply<Vec<u8>>,
    },
    Text {
        token: u64,
        source: u32,
        reply: Reply<Vec<TextRun>>,
    },
    Search {
        token: u64,
        query: String,
        sources: Vec<u32>,
        reply: Reply<Vec<SearchHit>>,
    },
    Close,
    /// Runs work with the library itself, for tools that create documents.
    Library {
        work: LibraryWork,
    },
    /// Runs work against a separate document, between other jobs.
    Temporary {
        bytes: Vec<u8>,
        password: Option<String>,
        work: TemporaryWork,
    },
}

/// Handle to the thread that owns PDFium, which is not safe to call concurrently.
pub struct Engine {
    jobs: Sender<Job>,
    cancelled: Arc<Mutex<HashSet<u64>>>,
}

impl Engine {
    pub fn start(library: PathBuf) -> Self {
        let (jobs, queue) = channel();
        let cancelled = Arc::new(Mutex::new(HashSet::new()));
        let worker_cancelled = Arc::clone(&cancelled);
        thread::spawn(move || {
            let bindings = Pdfium::bind_to_library(&library).map_err(|error| error.to_string());
            match bindings {
                Ok(bindings) => {
                    let pdfium = Pdfium::new(bindings);
                    let mut worker = Worker {
                        pdfium: &pdfium,
                        document: None,
                        text: TextCache::default(),
                    };
                    for job in queue {
                        worker.run(job, &worker_cancelled);
                    }
                }
                Err(error) => {
                    for job in queue {
                        job.refuse(&error);
                    }
                }
            }
        });
        Self { jobs, cancelled }
    }

    fn ask<T>(&self, job: impl FnOnce(Reply<T>) -> Job) -> Result<T, String> {
        let (reply, answer) = channel();
        self.jobs
            .send(job(reply))
            .map_err(|_| "The PDF engine has stopped.".to_owned())?;
        answer
            .recv()
            .map_err(|_| "The PDF engine has stopped.".to_owned())?
    }

    /// Parses `bytes` and, on success, replaces the current document.
    pub fn load(
        &self,
        token: u64,
        bytes: Vec<u8>,
        password: Option<String>,
    ) -> Result<LoadOutcome, String> {
        self.ask(|reply| Job::Load {
            token,
            bytes,
            password,
            reply,
        })
    }

    /// Renders a page to RGBA bytes. `ticket` lets `cancel` skip it while it is queued.
    pub fn render(&self, ticket: u64, frame: Frame) -> Result<Vec<u8>, String> {
        self.ask(|reply| Job::Render {
            ticket,
            frame,
            reply,
        })
    }

    pub fn cancel(&self, ticket: u64) {
        self.cancelled
            .lock()
            .expect("cancel list poisoned")
            .insert(ticket);
    }

    pub fn text_runs(&self, token: u64, source: u32) -> Result<Vec<TextRun>, String> {
        self.ask(|reply| Job::Text {
            token,
            source,
            reply,
        })
    }

    pub fn search(
        &self,
        token: u64,
        query: String,
        sources: Vec<u32>,
    ) -> Result<Vec<SearchHit>, String> {
        self.ask(|reply| Job::Search {
            token,
            query,
            sources,
            reply,
        })
    }

    /// Opens `bytes` as a second document for the length of `work`, leaving the reader's
    /// document in place. Long tools call this once per page, so scrolling stays smooth.
    pub fn with_temporary<T: Send + 'static>(
        &self,
        bytes: Vec<u8>,
        password: Option<String>,
        work: impl for<'d> FnOnce(&PdfDocument<'d>) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        self.ask(|reply| Job::Temporary {
            bytes,
            password,
            work: Box::new(move |document| drop(reply.send(document.and_then(work)))),
        })
    }

    /// Runs `work` on the PDFium thread, for tools that build a document from nothing.
    pub fn with_pdfium<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Pdfium) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        self.ask(|reply| Job::Library {
            work: Box::new(move |pdfium| drop(reply.send(pdfium.and_then(work)))),
        })
    }

    pub fn close(&self) {
        // A stopped worker holds no document, so there is nothing left to release.
        let _ = self.jobs.send(Job::Close);
    }
}

impl Job {
    fn refuse(self, error: &str) {
        let error = error.to_owned();
        match self {
            Job::Load { reply, .. } => drop(reply.send(Err(error))),
            Job::Render { reply, .. } => drop(reply.send(Err(error))),
            Job::Text { reply, .. } => drop(reply.send(Err(error))),
            Job::Search { reply, .. } => drop(reply.send(Err(error))),
            Job::Close => {}
            Job::Temporary { work, .. } => work(Err(error)),
            Job::Library { work } => work(Err(error)),
        }
    }
}

struct Worker<'a> {
    pdfium: &'a Pdfium,
    document: Option<(u64, PdfDocument<'a>)>,
    text: TextCache,
}

impl Worker<'_> {
    fn run(&mut self, job: Job, cancelled: &Mutex<HashSet<u64>>) {
        match job {
            Job::Load {
                token,
                bytes,
                password,
                reply,
            } => drop(reply.send(self.load(token, bytes, password))),
            Job::Render {
                ticket,
                frame,
                reply,
            } => {
                let skipped = cancelled
                    .lock()
                    .expect("cancel list poisoned")
                    .remove(&ticket);
                let result = if skipped {
                    Err("This render was cancelled.".to_owned())
                } else {
                    self.render(frame)
                };
                drop(reply.send(result));
            }
            Job::Text {
                token,
                source,
                reply,
            } => drop(reply.send(self.text_runs(token, source))),
            Job::Search {
                token,
                query,
                sources,
                reply,
            } => drop(reply.send(self.search(token, &query, &sources))),
            Job::Close => {
                self.document = None;
                self.text = TextCache::default();
            }
            Job::Library { work } => work(Ok(self.pdfium)),
            Job::Temporary {
                bytes,
                password,
                work,
            } => {
                let document = self
                    .pdfium
                    .load_pdf_from_byte_vec(bytes, password.as_deref())
                    .map_err(|_| "This PDF could not be opened.".to_owned());
                match &document {
                    Ok(document) => work(Ok(document)),
                    Err(error) => work(Err(error.clone())),
                }
            }
        }
    }

    fn load(
        &mut self,
        token: u64,
        bytes: Vec<u8>,
        password: Option<String>,
    ) -> Result<LoadOutcome, String> {
        let document = match self
            .pdfium
            .load_pdf_from_byte_vec(bytes, password.as_deref())
        {
            Ok(document) => document,
            Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                return Ok(LoadOutcome::Password {
                    incorrect: password.is_some(),
                });
            }
            Err(_) => return Err("This PDF could not be opened. It may be damaged.".into()),
        };
        if document.pages().len() as usize > MAX_PAGES {
            return Err("This version supports documents up to 2,000 pages.".into());
        }
        let pages = document
            .pages()
            .iter()
            .map(|page| page_view(&page))
            .collect::<Result<Vec<_>, _>>()?;
        self.document = Some((token, document));
        self.text = TextCache::default();
        Ok(LoadOutcome::Ready {
            pages,
            marks: Vec::new(),
            restored: None,
            fields: Vec::new(),
            protected: false,
        })
    }

    /// Rejects stale document tokens and invalid one-based page numbers.
    fn page(&self, token: u64, source: u32) -> Result<PdfPage<'_>, String> {
        let (current, document) = self
            .document
            .as_ref()
            .filter(|(current, _)| *current == token)
            .ok_or("This document is no longer open.")?;
        debug_assert_eq!(*current, token);
        let index = source
            .checked_sub(1)
            .and_then(|index| i32::try_from(index).ok())
            .ok_or_else(|| "Invalid page.".to_owned())?;
        document
            .pages()
            .get(index)
            .map_err(|error| error.to_string())
    }

    fn render(&self, frame: Frame) -> Result<Vec<u8>, String> {
        let page = self.page(frame.token, frame.source)?;
        let stored = degrees(page.rotation().map_err(|error| error.to_string())?);
        // PDFium applies render rotation on top of the page's own /Rotate entry.
        let relative = (frame.rotation + 360 - stored) % 360;
        let width = Pixels::try_from(frame.width).map_err(|_| "Page too large.".to_owned())?;
        let height = Pixels::try_from(frame.height).map_err(|_| "Page too large.".to_owned())?;
        let config = PdfRenderConfig::new()
            .set_target_size(width, height)
            .rotate(render_rotation(relative), false)
            .render_annotations(true)
            .render_form_data(true);
        page.render_with_config(&config)
            .map(|bitmap| bitmap.as_rgba_bytes())
            .map_err(|error| error.to_string())
    }

    fn text_runs(&self, token: u64, source: u32) -> Result<Vec<TextRun>, String> {
        page_runs(&self.page(token, source)?)
    }

    /// Joins the page's lines; `PdfPageText::all` clips to the wrong box on rotated pages.
    fn page_text(&self, token: u64, source: u32) -> Result<String, String> {
        let lines: Vec<_> = self
            .text_runs(token, source)?
            .into_iter()
            .map(|run| run.text)
            .collect();
        Ok(lines
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "))
    }

    /// Searches only the current document, checking its token before any cache access.
    fn search(
        &mut self,
        token: u64,
        query: &str,
        sources: &[u32],
    ) -> Result<Vec<SearchHit>, String> {
        if self.document.as_ref().map(|(current, _)| *current) != Some(token) {
            return Err("This document is no longer open.".into());
        }
        let mut hits = Vec::new();
        for &source in sources {
            let text = match self.text.get(source) {
                Some(text) => text,
                None => {
                    let joined = self.page_text(token, source)?;
                    self.text.insert(source, joined)
                }
            };
            hits.extend(find_hit(source, &text, query));
        }
        Ok(hits)
    }
}

/// The lines of text on a page.
pub(crate) fn page_runs(page: &PdfPage) -> Result<Vec<TextRun>, String> {
    let text = page.text().map_err(|error| error.to_string())?;
    let segments = text.segments();
    let runs = segments
        .iter()
        .filter_map(|segment| {
            let content = segment.text();
            let bounds = segment.bounds();
            (!content.trim().is_empty()).then(|| TextRun {
                direction: reading_direction(&segment),
                text: content.trim_end_matches(['\r', '\n']).to_owned(),
                rect: Rect {
                    x: f64::from(bounds.left().value),
                    y: f64::from(bounds.bottom().value),
                    width: f64::from(bounds.width().value),
                    height: f64::from(bounds.height().value),
                },
            })
        })
        .collect();
    Ok(runs)
}

/// shortcut: skewed text snaps to the nearest quarter turn, so selection boxes only
/// approximate it. Measure each run's real angle if slanted documents matter.
fn reading_direction(segment: &PdfPageTextSegment) -> u16 {
    let angle = segment
        .chars()
        .ok()
        .and_then(|chars| {
            chars
                .iter()
                .next()
                .and_then(|first| first.angle_degrees().ok())
        })
        .unwrap_or(0.0);
    // PDFium measures clockwise; `TextRun::direction` is counterclockwise.
    (-(angle / 90.0).round() as i32 * 90).rem_euclid(360) as u16
}

fn degrees(rotation: PdfPageRenderRotation) -> u16 {
    match rotation {
        PdfPageRenderRotation::None => 0,
        PdfPageRenderRotation::Degrees90 => 90,
        PdfPageRenderRotation::Degrees180 => 180,
        PdfPageRenderRotation::Degrees270 => 270,
    }
}

fn render_rotation(degrees: u16) -> PdfPageRenderRotation {
    match degrees {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

/// A page's size, position and stored rotation.
pub fn page_view(page: &PdfPage) -> Result<PageInfo, String> {
    let bounds = page
        .boundaries()
        .bounding()
        .map_err(|error| error.to_string())?
        .bounds;
    Ok(PageInfo {
        view: [
            f64::from(bounds.left().value),
            f64::from(bounds.bottom().value),
            f64::from(bounds.right().value),
            f64::from(bounds.top().value),
        ],
        rotate: degrees(page.rotation().map_err(|error| error.to_string())?),
    })
}

/// Page text kept for repeat searches, oldest-used first, bounded by total characters.
#[derive(Default)]
struct TextCache {
    pages: VecDeque<(u32, Arc<str>)>,
    characters: usize,
}

impl TextCache {
    fn get(&mut self, source: u32) -> Option<Arc<str>> {
        let index = self.pages.iter().position(|(page, _)| *page == source)?;
        let entry = self.pages.remove(index)?;
        let text = Arc::clone(&entry.1);
        self.pages.push_back(entry);
        Some(text)
    }

    fn insert(&mut self, source: u32, text: String) -> Arc<str> {
        let size = text.len();
        let text: Arc<str> = text.into();
        if size > MAX_CACHED_CHARACTERS {
            return text;
        }
        while self.characters + size > MAX_CACHED_CHARACTERS {
            let Some((_, oldest)) = self.pages.pop_front() else {
                break;
            };
            self.characters -= oldest.len();
        }
        self.pages.push_back((source, Arc::clone(&text)));
        self.characters += size;
        text
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::create_sample;
    use crate::export::tests::{fixture, inherited_fixture, protected_fixture, sideways_fixture};
    use std::sync::{LazyLock, MutexGuard, PoisonError};

    /// PDFium binds once per process and holds one document, so tests take turns.
    pub(crate) fn engine() -> MutexGuard<'static, Engine> {
        static ENGINE: LazyLock<Mutex<Engine>> = LazyLock::new(|| {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            Mutex::new(Engine::start(root.join("../src-tauri/pdfium/pdfium.dll")))
        });
        ENGINE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn pages(outcome: LoadOutcome) -> Vec<PageInfo> {
        match outcome {
            LoadOutcome::Ready { pages, .. } => pages,
            LoadOutcome::Password { .. } => panic!("unexpected password prompt"),
        }
    }

    fn page_text(engine: &Engine, token: u64, source: u32) -> String {
        engine
            .text_runs(token, source)
            .unwrap()
            .into_iter()
            .map(|run| run.text)
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn loads_page_geometry_from_the_crop_box_and_inherited_rotation() {
        let engine = engine();
        let info = pages(engine.load(1, inherited_fixture(), None).unwrap());
        assert_eq!(info[0].view, [10.0, 20.0, 290.0, 380.0]);
        assert_eq!(info[0].rotate, 90);
        assert_eq!(info[1].rotate, 0);
    }

    #[test]
    fn text_runs_are_selectable_lines_in_pdf_coordinates() {
        let engine = engine();
        engine.load(1, create_sample().unwrap(), None).unwrap();
        let runs = engine.text_runs(1, 1).unwrap();
        let line = runs
            .iter()
            .find(|run| run.text.contains("The best tools give you space."))
            .expect("body line");
        assert!((line.rect.x - 56.0).abs() < 2.0, "{line:?}");
        assert!((line.rect.y - 405.0).abs() < 8.0, "{line:?}");
        assert!(
            line.rect.height > 5.0 && line.rect.height < 16.0,
            "{line:?}"
        );
    }

    #[test]
    fn sideways_text_reports_its_reading_direction() {
        let engine = engine();
        engine.load(1, sideways_fixture(), None).unwrap();
        let run = &engine.text_runs(1, 1).unwrap()[0];
        assert_eq!(
            (run.text.as_str(), run.direction),
            ("Reading upward", 90),
            "{run:?}"
        );
    }

    #[test]
    fn renders_white_paper_with_dark_ink_at_the_requested_size() {
        let engine = engine();
        engine.load(1, create_sample().unwrap(), None).unwrap();
        let frame = Frame {
            token: 1,
            source: 1,
            rotation: 0,
            width: 306,
            height: 396,
        };
        let rgba = engine.render(1, frame).unwrap();
        assert_eq!(rgba.len(), 306 * 396 * 4);
        assert!(rgba.chunks(4).any(|pixel| pixel[0] < 90));
    }

    /// Finds the rendered ink's center of mass as a fraction of the bitmap.
    fn ink_center(rgba: &[u8], width: usize) -> (f64, f64) {
        let (mut x, mut y, mut count) = (0.0, 0.0, 0.0);
        for (index, pixel) in rgba.chunks(4).enumerate() {
            if pixel[0] < 128 {
                x += (index % width) as f64;
                y += (index / width) as f64;
                count += 1.0;
            }
        }
        let height = rgba.len() / 4 / width;
        (x / count / width as f64, y / count / height as f64)
    }

    #[test]
    fn rotation_is_absolute_even_when_the_page_stores_its_own() {
        let engine = engine();
        // Page 2 stores /Rotate 90 and its text sits near the top of the unrotated page.
        engine.load(1, fixture(), None).unwrap();
        let render = |rotation, width: u32, height: u32| {
            let frame = Frame {
                token: 1,
                source: 2,
                rotation,
                width,
                height,
            };
            let rgba = engine.render(1, frame).unwrap();
            assert_eq!(rgba.len(), (width * height * 4) as usize);
            ink_center(&rgba, width as usize)
        };
        let upright = render(0, 320, 400);
        assert!(upright.1 < 0.3, "text near the top: {upright:?}");
        let clockwise = render(90, 400, 320);
        assert!(clockwise.0 > 0.7, "text near the right: {clockwise:?}");
        let inverted = render(180, 320, 400);
        assert!(inverted.1 > 0.7, "text near the bottom: {inverted:?}");
        let counter = render(270, 400, 320);
        assert!(counter.0 < 0.3, "text near the left: {counter:?}");
    }

    #[test]
    fn cancelled_renders_are_skipped() {
        let engine = engine();
        engine.load(1, create_sample().unwrap(), None).unwrap();
        engine.cancel(7);
        let frame = Frame {
            token: 1,
            source: 1,
            rotation: 0,
            width: 10,
            height: 10,
        };
        assert!(engine.render(7, frame).is_err());
        assert!(engine.render(8, frame).is_ok());
    }

    #[test]
    fn requests_for_a_replaced_document_fail() {
        let engine = engine();
        engine.load(1, create_sample().unwrap(), None).unwrap();
        engine.load(2, fixture(), None).unwrap();
        assert!(engine.text_runs(1, 1).is_err());
        assert_eq!(page_text(&engine, 2, 1), "Original page 1");
        engine.close();
        assert!(engine.text_runs(2, 1).is_err());
    }

    #[test]
    fn cached_search_rejects_stale_tokens_and_closed_documents() {
        let engine = engine();
        engine.load(1, create_sample().unwrap(), None).unwrap();
        engine.load(2, fixture(), None).unwrap();
        assert!(
            !engine
                .search(2, "Original".into(), vec![1])
                .unwrap()
                .is_empty()
        );
        assert!(engine.search(1, "Original".into(), vec![1]).is_err());
        assert!(engine.search(1, "Original".into(), vec![]).is_err());
        engine.close();
        assert!(engine.search(2, "Original".into(), vec![]).is_err());
    }

    #[test]
    fn page_zero_is_rejected_without_stopping_the_worker() {
        let engine = engine();
        engine.load(1, fixture(), None).unwrap();
        assert_eq!(engine.text_runs(1, 0).unwrap_err(), "Invalid page.");
        assert_eq!(page_text(&engine, 1, 1), "Original page 1");
    }

    #[test]
    fn search_follows_the_requested_page_order_and_deletion() {
        let engine = engine();
        engine.load(1, fixture(), None).unwrap();
        let order = |query: &str, sources: Vec<u32>| {
            let hits = engine.search(1, query.into(), sources).unwrap();
            hits.into_iter().map(|hit| hit.page).collect::<Vec<_>>()
        };
        assert_eq!(order(" ORIGINAL ", vec![3, 1]), [3, 1]);
        assert_eq!(order("page 2", vec![1, 2, 3]), [2]);
        assert_eq!(order("page 2", vec![1, 3]), [] as [u32; 0]);
    }

    #[test]
    fn protected_documents_ask_for_their_password() {
        let engine = engine();
        let bytes = protected_fixture();
        assert_eq!(
            engine.load(1, bytes.clone(), None).unwrap(),
            LoadOutcome::Password { incorrect: false }
        );
        assert_eq!(
            engine.load(1, bytes.clone(), Some("wrong".into())).unwrap(),
            LoadOutcome::Password { incorrect: true }
        );
        pages(engine.load(1, bytes, Some("secret".into())).unwrap());
        assert_eq!(page_text(&engine, 1, 1), "Original page 1");
    }

    #[test]
    fn damaged_files_report_an_error() {
        let error = engine()
            .load(1, b"%PDF-1.7 nope".to_vec(), None)
            .unwrap_err();
        assert!(error.contains("could not be opened"));
    }

    #[test]
    fn text_cache_evicts_the_least_recently_used_page() {
        let mut cache = TextCache::default();
        cache.insert(1, "a".repeat(MAX_CACHED_CHARACTERS / 2));
        cache.insert(2, "b".repeat(MAX_CACHED_CHARACTERS / 2));
        assert!(cache.get(1).is_some());
        cache.insert(3, "c".repeat(10));
        assert!(cache.get(2).is_none());
        assert!(cache.get(1).is_some());
        assert!(cache.get(3).is_some());
    }
}
