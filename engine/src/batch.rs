//! Whole-document tools, run a few pages at a time so reading stays smooth and they can stop.

use ethos_core::{Change, TextRun};
use pdfium_render::prelude::*;

use crate::compare::compare_lines;
use crate::media::encode_image;
use crate::ocr::{OcrPage, add_text_layer, recognize};
use crate::structure::{Block, page_blocks};
use crate::{Engine, page_view};

/// Pages handled per trip to the PDFium thread, between which the reader gets a turn.
const PAGES_PER_TURN: u32 = 6;
/// OCR needs more detail than the screen does.
const OCR_SCALE: f32 = 2.5;

pub const CANCELLED: &str = "Cancelled.";

/// Reports `(done, total)`, and returns false to stop the work.
pub trait Progress: Fn(u32, u32) -> bool + Send + Sync {}
impl<T: Fn(u32, u32) -> bool + Send + Sync> Progress for T {}

pub fn page_count(engine: &Engine, bytes: &[u8]) -> Result<u32, String> {
    engine.with_temporary(bytes.to_vec(), None, |document| {
        Ok(document.pages().len() as u32)
    })
}

/// Runs `work` for every page, in turns, collecting one result per page.
fn each_page<T: Send + 'static>(
    engine: &Engine,
    bytes: &[u8],
    progress: &impl Progress,
    work: impl Fn(&PdfPage, u32) -> Result<T, String> + Send + Sync + Clone + 'static,
) -> Result<Vec<T>, String> {
    let total = page_count(engine, bytes)?;
    let mut results = Vec::with_capacity(total as usize);
    let mut next = 1;
    while next <= total {
        if !progress(next - 1, total) {
            return Err(CANCELLED.into());
        }
        let last = (next + PAGES_PER_TURN - 1).min(total);
        let work = work.clone();
        let turn = engine.with_temporary(bytes.to_vec(), None, move |document| {
            (next..=last)
                .map(|number| {
                    let page = document
                        .pages()
                        .get(number as i32 - 1)
                        .map_err(|error| error.to_string())?;
                    work(&page, number)
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        results.extend(turn);
        next = last + 1;
    }
    progress(total, total);
    Ok(results)
}

pub fn document_blocks(
    engine: &Engine,
    bytes: &[u8],
    progress: &impl Progress,
) -> Result<Vec<Vec<Block>>, String> {
    each_page(engine, bytes, progress, |page, _| page_blocks(page))
}

fn bitmap_of(page: &PdfPage, scale: f32) -> Result<(Vec<u8>, u32, u32), String> {
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .render_annotations(true)
        .render_form_data(true);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|error| error.to_string())?;
    Ok((
        bitmap.as_rgba_bytes(),
        bitmap.width() as u32,
        bitmap.height() as u32,
    ))
}

/// Every page as an encoded picture at `dpi`.
pub fn document_images(
    engine: &Engine,
    bytes: &[u8],
    dpi: u32,
    jpeg: bool,
    progress: &impl Progress,
) -> Result<Vec<Vec<u8>>, String> {
    let scale = dpi as f32 / 72.0;
    each_page(engine, bytes, progress, move |page, _| {
        let (rgba, width, height) = bitmap_of(page, scale)?;
        encode_image(rgba, width, height, jpeg)
    })
}

fn has_text(page: &PdfPage) -> bool {
    page.text().is_ok_and(|text| {
        text.segments()
            .iter()
            .any(|segment| !segment.text().trim().is_empty())
    })
}

/// A copy with recognized text added to pages that have none. Returns it and pages read.
pub fn searchable_copy(
    engine: &Engine,
    bytes: &[u8],
    progress: &impl Progress,
) -> Result<(Vec<u8>, u32), String> {
    let total = page_count(engine, bytes)?;
    let mut pages = Vec::new();
    for number in 1..=total {
        if !progress(number - 1, total) {
            return Err(CANCELLED.into());
        }
        // Recognition is slow, so it runs here, away from the thread drawing pages.
        let rendered = engine.with_temporary(bytes.to_vec(), None, move |document| {
            let page = document
                .pages()
                .get(number as i32 - 1)
                .map_err(|error| error.to_string())?;
            if has_text(&page) {
                return Ok(None);
            }
            let info = page_view(&page)?;
            Ok(Some((bitmap_of(&page, OCR_SCALE)?, info)))
        })?;
        let Some(((rgba, width, height), info)) = rendered else {
            continue;
        };
        pages.push(OcrPage {
            number,
            view: info.view,
            rotation: info.rotate,
            scale: f64::from(OCR_SCALE),
            words: recognize(&rgba, width, height)?,
        });
    }
    progress(total, total);
    let read = pages.iter().filter(|page| !page.words.is_empty()).count() as u32;
    Ok((add_text_layer(bytes, &pages)?, read))
}

fn all_lines(
    engine: &Engine,
    bytes: &[u8],
    progress: &impl Progress,
) -> Result<Vec<(u32, TextRun)>, String> {
    let pages = each_page(engine, bytes, progress, |page, number| {
        let runs = crate::reader::page_runs(page)?;
        Ok(runs
            .into_iter()
            .map(|run| (number, run))
            .collect::<Vec<_>>())
    })?;
    Ok(pages.into_iter().flatten().collect())
}

/// What differs between `other` and the open document's `current` bytes.
pub fn compare_documents(
    engine: &Engine,
    other: &[u8],
    current: &[u8],
    progress: &impl Progress,
) -> Result<Vec<Change>, String> {
    let before = all_lines(engine, other, progress)?;
    let after = all_lines(engine, current, progress)?;
    if !progress(0, 1) {
        return Err(CANCELLED.into());
    }
    let changes = compare_lines(&before, &after);
    if !progress(u32::from(changes.is_ok()), 1) {
        return Err(CANCELLED.into());
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::tests::fixture;
    use crate::reader::tests::engine;
    use crate::{create_sample, to_text};
    use ethos_core::ChangeKind;

    #[test]
    fn the_sample_keeps_its_headings_and_paragraphs() {
        let engine = engine();
        let blocks = document_blocks(&engine, &create_sample().unwrap(), &|_, _| true).unwrap();
        assert_eq!(blocks.len(), 4);
        assert!(
            blocks[0].iter().any(|block| matches!(
                block,
                Block::Heading { level: 1, text } if text.contains("A little more room to think")
            )),
            "{:#?}",
            blocks[0]
        );
        let text = to_text(&blocks);
        assert!(text.contains("The best tools give you space. Space to read closely, follow an idea, and make something your own."), "{text}");
    }

    #[test]
    fn progress_reaches_the_end_and_can_cancel() {
        let engine = engine();
        let source = fixture();
        let seen = std::sync::Mutex::new(Vec::new());
        document_blocks(&engine, &source, &|done, total| {
            seen.lock().unwrap().push((done, total));
            true
        })
        .unwrap();
        assert_eq!(seen.lock().unwrap().last(), Some(&(3, 3)));
        let stopped = document_blocks(&engine, &source, &|_, _| false);
        assert_eq!(stopped.unwrap_err(), CANCELLED);
    }

    #[test]
    fn every_page_becomes_a_picture() {
        let engine = engine();
        let pictures = document_images(&engine, &fixture(), 72, false, &|_, _| true).unwrap();
        assert_eq!(pictures.len(), 3);
        assert_eq!(
            image::guess_format(&pictures[0]).unwrap(),
            image::ImageFormat::Png
        );
        let picture = image::load_from_memory(&pictures[0]).unwrap();
        assert_eq!((picture.width(), picture.height()), (310, 400));
    }

    #[test]
    fn pages_that_already_have_text_are_not_read_again() {
        let engine = engine();
        let (_, read) = searchable_copy(&engine, &fixture(), &|_, _| true).unwrap();
        assert_eq!(read, 0);
    }

    #[test]
    fn comparing_two_files_finds_the_changed_page() {
        let engine = engine();
        let edited = crate::extract_pages(&fixture(), &[1, 3]).unwrap();
        let changes = compare_documents(&engine, &fixture(), &edited, &|_, _| true).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Removed);
        assert_eq!(changes[0].before, "Original page 2");
    }
}
