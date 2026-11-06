//! The tools: choose where the result goes, do the work with progress, write new files.
//! Every tool writes a new file. An open document and its source are never changed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use ethos_core::{Edits, MAX_FILE_SIZE, ToolOutcome, ToolProgress, ToolRequest, ToolResult};
use ethos_engine::{
    OutputFile, PageFit, apply_redactions, compare_documents, compress_pdf, document_blocks,
    document_images, every_n_pages, export_pdf, extract_pages, images_to_pdf, merge_pdfs,
    page_count, parse_ranges, protect_pdf, redaction_pages, remove_password, searchable_copy,
    to_docx, to_markdown, to_text, verify_redactions, word_to_pdf,
};
use tauri::async_runtime::spawn_blocking;
use tauri::{AppHandle, Emitter, Manager};

use crate::dialogs::{Filter, pick_destination, pick_files, pick_folder, pick_pdf};
use crate::files::{self, stem_of, unique_path};
use crate::{Answer, Shell};

pub const PICTURES: Filter = Filter {
    name: "Pictures",
    extensions: &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
};
pub const PDFS: Filter = Filter {
    name: "PDF documents",
    extensions: &["pdf"],
};
const WORD: Filter = Filter {
    name: "Word documents",
    extensions: &["docx"],
};

/// Marks the single tool slot busy until the tool finishes, however it finishes.
struct ToolSlot<'a>(&'a Shell);

impl<'a> ToolSlot<'a> {
    fn claim(shell: &'a Shell) -> Answer<Self> {
        if shell.tool_running.swap(true, Ordering::SeqCst) {
            return Err("Another tool is still working.".into());
        }
        shell.tool_cancelled.store(false, Ordering::SeqCst);
        Ok(Self(shell))
    }
}

impl Drop for ToolSlot<'_> {
    fn drop(&mut self) {
        self.0.tool_running.store(false, Ordering::SeqCst);
    }
}

/// Announces progress to the interface and reports whether the tool should continue.
fn reporter(
    app: &AppHandle,
    label: &'static str,
) -> impl Fn(u32, u32) -> bool + Send + Sync + use<> {
    let app = app.clone();
    move |done, total| {
        // Progress is decoration: losing an update must not stop the work.
        let _ = app.emit(
            "tool-progress",
            ToolProgress {
                label: label.to_owned(),
                done,
                total,
            },
        );
        !app.state::<Shell>().tool_cancelled.load(Ordering::SeqCst)
    }
}

fn read_limited(path: &Path) -> Answer<Vec<u8>> {
    let size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if size > MAX_FILE_SIZE {
        return Err("Each file must be smaller than 200 MB.".into());
    }
    fs::read(path).map_err(|error| error.to_string())
}

/// What the open document looks like now, with its marks, page changes and form values.
fn current_bytes(shell: &Shell, edits: Option<&Edits>) -> Answer<Vec<u8>> {
    let source = shell
        .state()
        .current
        .clone()
        .ok_or("No document is open.")?;
    match edits {
        Some(edits) => export_pdf(&source, edits),
        None => Ok(source.to_vec()),
    }
}

fn sources(shell: &Shell) -> Vec<PathBuf> {
    shell.state().sources.clone()
}

fn describe_size(bytes: usize) -> String {
    let size = bytes as f64;
    if size < 1024.0 * 1024.0 {
        format!("{} KB", (size / 1024.0).round().max(1.0))
    } else {
        format!("{:.1} MB", size / 1024.0 / 1024.0)
    }
}

/// Writes one result and remembers it, so the interface can ask for it to be opened.
fn finish_file(
    shell: &Shell,
    path: PathBuf,
    bytes: &[u8],
    label: String,
    detail: Option<String>,
) -> Answer<ToolOutcome> {
    files::write_copy(&path, bytes, &sources(shell))?;
    let shown = path.to_string_lossy().into_owned();
    shell.state().outputs.push(path);
    Ok(ToolOutcome::Done(ToolResult {
        path: shown,
        label,
        detail,
    }))
}

/// Writes results into `folder`, never over something already there.
fn finish_folder(
    shell: &Shell,
    folder: &Path,
    files: Vec<OutputFile>,
    label: String,
) -> Answer<ToolOutcome> {
    for file in &files {
        let target = folder.join(&file.name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(unique_path(&target), &file.bytes).map_err(|error| error.to_string())?;
    }
    let shown = folder.to_string_lossy().into_owned();
    shell.state().outputs.push(folder.to_path_buf());
    Ok(ToolOutcome::Done(ToolResult {
        path: shown,
        label,
        detail: None,
    }))
}

fn plural(count: usize, one: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {one}s")
    }
}

/// Runs a tool from start to finish. Picking comes first, so nobody waits to be asked.
pub async fn run(
    app: AppHandle,
    request: ToolRequest,
    edits: Option<Edits>,
    name: String,
) -> Answer<ToolOutcome> {
    let shell = app.state::<Shell>();
    let _slot = ToolSlot::claim(&shell)?;
    let stem = stem_of(&name);
    match request {
        ToolRequest::SaveText {
            suggested,
            text,
            kind,
        } => {
            let filter = if kind == "md" {
                Filter {
                    name: "Markdown",
                    extensions: &["md"],
                }
            } else {
                Filter {
                    name: "Text",
                    extensions: &["txt"],
                }
            };
            let Some(path) = pick_destination(&app, suggested, &filter, "Export notes").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            finish_file(
                &shell,
                path,
                text.as_bytes(),
                "Notes exported.".into(),
                None,
            )
        }
        ToolRequest::Markdown | ToolRequest::Text | ToolRequest::Word => {
            let (suggested, filter, title) = match request {
                ToolRequest::Markdown => (
                    format!("{stem}.md"),
                    Filter {
                        name: "Markdown",
                        extensions: &["md"],
                    },
                    "Export as Markdown",
                ),
                ToolRequest::Text => (
                    format!("{stem}.txt"),
                    Filter {
                        name: "Text",
                        extensions: &["txt"],
                    },
                    "Export as text",
                ),
                _ => (format!("{stem}.docx"), WORD, "Export as Word"),
            };
            let Some(path) = pick_destination(&app, suggested, &filter, title).await? else {
                return Ok(ToolOutcome::Cancelled);
            };
            let bytes = current_bytes(&shell, edits.as_ref())?;
            let worker = app.clone();
            let kind = request.clone();
            let output_stem = stem_of(&path.to_string_lossy());
            let folder = path.parent().map(Path::to_path_buf);
            let produced: Answer<Vec<OutputFile>> = spawn_blocking(move || {
                let shell = worker.state::<Shell>();
                let progress = reporter(&worker, "Reading the document");
                let blocks = document_blocks(&shell.engine, &bytes, &progress)?;
                match kind {
                    ToolRequest::Markdown => Ok(to_markdown(&blocks, &output_stem)),
                    ToolRequest::Text => Ok(vec![OutputFile {
                        name: format!("{output_stem}.txt"),
                        bytes: to_text(&blocks).into_bytes(),
                    }]),
                    _ => Ok(vec![OutputFile {
                        name: format!("{output_stem}.docx"),
                        bytes: to_docx(&blocks)?,
                    }]),
                }
            })
            .await
            .map_err(|error| error.to_string())?;
            let produced: Vec<OutputFile> = produced?;
            let mut outputs = produced.into_iter();
            let main = outputs.next().ok_or("Nothing was produced.")?;
            if let Some(folder) = folder {
                for extra in outputs {
                    let target = folder.join(&extra.name);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                    }
                    fs::write(target, extra.bytes).map_err(|error| error.to_string())?;
                }
            }
            let label = format!(
                "Saved {}.",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            finish_file(&shell, path, &main.bytes, label, None)
        }
        ToolRequest::Images { jpeg, dpi } => {
            let Some(folder) = pick_folder(&app, "Choose a folder for the pictures").await? else {
                return Ok(ToolOutcome::Cancelled);
            };
            let bytes = current_bytes(&shell, edits.as_ref())?;
            let worker = app.clone();
            let pictures = spawn_blocking(move || {
                let shell = worker.state::<Shell>();
                document_images(
                    &shell.engine,
                    &bytes,
                    dpi,
                    jpeg,
                    &reporter(&worker, "Drawing pages"),
                )
            })
            .await
            .map_err(|error| error.to_string())?;
            let pictures = pictures?;
            let extension = if jpeg { "jpg" } else { "png" };
            let count = pictures.len();
            let files = pictures
                .into_iter()
                .enumerate()
                .map(|(index, bytes)| OutputFile {
                    name: format!("{stem} - page {}.{extension}", index + 1),
                    bytes,
                })
                .collect();
            finish_folder(
                &shell,
                &folder,
                files,
                format!("Saved {}.", plural(count, "picture")),
            )
        }
        ToolRequest::Extract { pages } => {
            let suggested = format!("{stem} - {}.pdf", plural(pages.len(), "page"));
            let Some(path) =
                pick_destination(&app, suggested, &PDFS, "Save pages as a new PDF").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            let bytes = current_bytes(&shell, edits.as_ref())?;
            let output = extract_pages(&bytes, &pages)?;
            finish_file(
                &shell,
                path,
                &output,
                format!("Saved {} as a new PDF.", plural(pages.len(), "page")),
                None,
            )
        }
        ToolRequest::Split { ranges, every } => {
            let bytes = current_bytes(&shell, edits.as_ref())?;
            let worker = app.clone();
            let source = bytes.clone();
            let total =
                spawn_blocking(move || page_count(&worker.state::<Shell>().engine, &source))
                    .await
                    .map_err(|error| error.to_string())??;
            let groups = match every {
                Some(size) => every_n_pages(total, size),
                None => parse_ranges(&ranges, total)?,
            };
            let Some(folder) = pick_folder(&app, "Choose a folder for the parts").await? else {
                return Ok(ToolOutcome::Cancelled);
            };
            let files = groups
                .iter()
                .enumerate()
                .map(|(index, pages)| {
                    Ok(OutputFile {
                        name: format!("{stem} - part {}.pdf", index + 1),
                        bytes: extract_pages(&bytes, pages)?,
                    })
                })
                .collect::<Answer<Vec<_>>>()?;
            finish_folder(
                &shell,
                &folder,
                files,
                format!("Split into {}.", plural(groups.len(), "PDF")),
            )
        }
        ToolRequest::Merge {
            mut paths,
            include_current,
        } => {
            if paths.is_empty() {
                paths = pick_files(&app, &PDFS, "Choose PDFs to combine").await?;
                if paths.is_empty() {
                    return Ok(ToolOutcome::Cancelled);
                }
            }
            let suggested = if include_current {
                format!("{stem} - combined.pdf")
            } else {
                "Combined.pdf".to_owned()
            };
            let Some(path) =
                pick_destination(&app, suggested, &PDFS, "Save the combined PDF").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            let mut documents = Vec::new();
            if include_current {
                documents.push(current_bytes(&shell, edits.as_ref())?);
            }
            for source in &paths {
                documents.push(read_limited(source)?);
            }
            let count = documents.len();
            let output = spawn_blocking(move || merge_pdfs(&documents))
                .await
                .map_err(|error| error.to_string())??;
            finish_file(
                &shell,
                path,
                &output,
                format!("Combined {}.", plural(count, "PDF")),
                None,
            )
        }
        ToolRequest::ImagesToPdf { mut paths, fit_a4 } => {
            if paths.is_empty() {
                paths = pick_files(&app, &PICTURES, "Choose pictures").await?;
                if paths.is_empty() {
                    return Ok(ToolOutcome::Cancelled);
                }
            }
            let Some(path) =
                pick_destination(&app, "Pictures.pdf".into(), &PDFS, "Save the PDF").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            let pictures = paths
                .iter()
                .map(|path| read_limited(path))
                .collect::<Answer<Vec<_>>>()?;
            let count = pictures.len();
            let fit = if fit_a4 {
                PageFit::A4
            } else {
                PageFit::Original
            };
            let output = spawn_blocking(move || images_to_pdf(&pictures, fit))
                .await
                .map_err(|error| error.to_string())??;
            finish_file(
                &shell,
                path,
                &output,
                format!("Made a PDF from {}.", plural(count, "picture")),
                None,
            )
        }
        ToolRequest::WordToPdf { path: chosen } => {
            let chosen = match chosen {
                Some(path) => path,
                None => match pick_files(&app, &WORD, "Choose a Word document")
                    .await?
                    .into_iter()
                    .next()
                {
                    Some(path) => path,
                    None => return Ok(ToolOutcome::Cancelled),
                },
            };
            let suggested = format!("{}.pdf", stem_of(&chosen.to_string_lossy()));
            let Some(path) = pick_destination(&app, suggested, &PDFS, "Save the PDF").await? else {
                return Ok(ToolOutcome::Cancelled);
            };
            let source = read_limited(&chosen)?;
            let worker = app.clone();
            let output = spawn_blocking(move || {
                worker
                    .state::<Shell>()
                    .engine
                    .with_pdfium(move |pdfium| word_to_pdf(pdfium, &source))
            })
            .await
            .map_err(|error| error.to_string())??;
            finish_file(
                &shell,
                path,
                &output,
                "Made a PDF from your Word document.".into(),
                None,
            )
        }
        ToolRequest::Searchable => {
            let suggested = format!("{stem} - searchable.pdf");
            let Some(path) =
                pick_destination(&app, suggested, &PDFS, "Save a searchable copy").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            let bytes = current_bytes(&shell, edits.as_ref())?;
            let worker = app.clone();
            let produced = spawn_blocking(move || {
                searchable_copy(
                    &worker.state::<Shell>().engine,
                    &bytes,
                    &reporter(&worker, "Reading the pages"),
                )
            })
            .await
            .map_err(|error| error.to_string())?;
            let (output, read) = produced?;
            if read == 0 {
                return Err("ethos found no text to read on these pages.".into());
            }
            finish_file(
                &shell,
                path,
                &output,
                format!(
                    "Read {}. It can now be searched.",
                    plural(read as usize, "page")
                ),
                None,
            )
        }
        ToolRequest::Smaller => {
            let suggested = format!("{stem} - smaller.pdf");
            let Some(path) =
                pick_destination(&app, suggested, &PDFS, "Save a smaller copy").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            let bytes = current_bytes(&shell, edits.as_ref())?;
            let before = bytes.len();
            let output = spawn_blocking(move || compress_pdf(&bytes))
                .await
                .map_err(|error| error.to_string())??;
            let label = if output.len() < before {
                format!(
                    "{} is now {}.",
                    describe_size(before),
                    describe_size(output.len())
                )
            } else {
                "This PDF is already as small as ethos can make it.".to_owned()
            };
            finish_file(&shell, path, &output, label, None)
        }
        ToolRequest::Protect { password } => {
            let suggested = format!("{stem} - protected.pdf");
            let Some(path) =
                pick_destination(&app, suggested, &PDFS, "Save a protected copy").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            let bytes = current_bytes(&shell, edits.as_ref())?;
            let output = protect_pdf(&bytes, &password)?;
            finish_file(
                &shell,
                path,
                &output,
                "Saved a protected copy.".into(),
                None,
            )
        }
        ToolRequest::RemovePassword => {
            let (source, password) = {
                let state = shell.state();
                (state.current.clone(), state.password.clone())
            };
            let (Some(source), Some(password)) = (source, password) else {
                return Err("This PDF has no password to remove.".into());
            };
            let suggested = format!("{stem} - unlocked.pdf");
            let Some(path) =
                pick_destination(&app, suggested, &PDFS, "Save without a password").await?
            else {
                return Ok(ToolOutcome::Cancelled);
            };
            let output = remove_password(&source, &password)?;
            finish_file(
                &shell,
                path,
                &output,
                "Saved a copy that opens without a password.".into(),
                None,
            )
        }
        ToolRequest::Compare { path: chosen } => {
            let chosen = match chosen {
                Some(path) => path,
                None => match pick_pdf(&app).await? {
                    Some(path) => path,
                    None => return Ok(ToolOutcome::Cancelled),
                },
            };
            let other = read_limited(&chosen)?;
            let current = current_bytes(&shell, None)?;
            let worker = app.clone();
            let changes = spawn_blocking(move || {
                compare_documents(
                    &worker.state::<Shell>().engine,
                    &other,
                    &current,
                    &reporter(&worker, "Comparing versions"),
                )
            })
            .await
            .map_err(|error| error.to_string())?;
            let name = chosen
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            Ok(ToolOutcome::Compared {
                name,
                changes: changes?,
            })
        }
    }
}

/// A copy with redactions burned in, or an error if anything survived them.
pub fn redacted_copy(shell: &Shell, exported: Vec<u8>, edits: &Edits) -> Answer<Vec<u8>> {
    let targets = redaction_pages(edits);
    if targets.is_empty() {
        return Ok(exported);
    }
    let source = exported.clone();
    let checked: Vec<u32> = targets.iter().map(|target| target.number).collect();
    let result = shell
        .engine
        .with_temporary(exported, None, move |document| {
            apply_redactions(document, &source, &targets)
        })?;
    let verified = result.clone();
    shell.engine.with_temporary(result, None, move |document| {
        verify_redactions(document, &checked)
    })?;
    Ok(verified)
}
