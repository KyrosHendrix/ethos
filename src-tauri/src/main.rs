#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod drop;
mod files;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ethos_core::{
    Edits, LoadOutcome, RecentFile, SAMPLE_NAME, SaveOutcome, SearchHit, StagedFile, TextRun,
};
use ethos_engine::{Engine, Frame, create_sample, export_pdf};
use files::Recent;
use tauri::async_runtime::{Sender, channel, spawn, spawn_blocking};
use tauri::ipc::Response;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, WindowEvent};

type Answer<T> = Result<T, String>;

/// Everything shared between commands, window events and the second-instance callback.
struct Shell {
    engine: Engine,
    tokens: AtomicU64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    dirty: bool,
    interface_ready: bool,
    file_operation: bool,
    pending_path: Option<PathBuf>,
    discard_request: Option<(String, Sender<bool>)>,
    recent: Vec<Recent>,
    /// Every source stays protected, even if the interface rejects a newly read PDF.
    sources: Vec<PathBuf>,
    staged: Option<(u64, Arc<Vec<u8>>)>,
    current: Option<Arc<Vec<u8>>>,
}

impl Shell {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("shell state poisoned")
    }

    fn stage(&self, name: String, bytes: Vec<u8>) -> StagedFile {
        let token = self.tokens.fetch_add(1, Ordering::Relaxed) + 1;
        let size = bytes.len() as u64;
        self.state().staged = Some((token, Arc::new(bytes)));
        StagedFile { token, name, size }
    }
}

/// Clears the single file-operation slot when the operation ends, however it ends.
struct FileOperation<'a>(&'a Shell);

impl<'a> FileOperation<'a> {
    fn begin(shell: &'a Shell) -> Answer<Self> {
        let mut state = shell.state();
        if state.file_operation {
            return Err("Another file operation is in progress.".into());
        }
        state.file_operation = true;
        Ok(Self(shell))
    }
}

impl Drop for FileOperation<'_> {
    fn drop(&mut self) {
        self.0.state().file_operation = false;
    }
}

/// Asks the interface whether unsaved edits may be dropped. Without an answer they stay.
async fn confirm_discard(app: &AppHandle) -> bool {
    let shell = app.state::<Shell>();
    let (sender, mut receiver) = channel(1);
    {
        let mut state = shell.state();
        if !state.dirty {
            return true;
        }
        if state.discard_request.is_some() || !state.interface_ready {
            return false;
        }
        let id = uuid::Uuid::new_v4().to_string();
        state.discard_request = Some((id.clone(), sender));
        if app.emit("confirm-discard", id).is_err() {
            state.discard_request = None;
            return false;
        }
    }
    receiver.recv().await.unwrap_or(false)
}

fn resolve_discard(shell: &Shell) {
    if let Some((_, sender)) = shell.state().discard_request.take() {
        // A closed receiver means the asker already gave up.
        let _ = sender.try_send(false);
    }
}

fn read_and_stage(app: &AppHandle, path: PathBuf) -> Answer<StagedFile> {
    let shell = app.state::<Shell>();
    let opened = files::read_pdf(&path)?;
    let recents = {
        let mut state = shell.state();
        files::remember(&mut state.recent, &opened);
        if !state.sources.contains(&opened.source.path) {
            state.sources.push(opened.source.path.clone());
        }
        state.recent.clone()
    };
    // A recent list that cannot be written must not stop the document opening.
    let _ = files::write_recents(&data_dir(app), &recents);
    Ok(shell.stage(opened.name, opened.bytes))
}

fn data_dir(app: &AppHandle) -> PathBuf {
    std::env::var_os("ETHOS_USER_DATA")
        .map(PathBuf::from)
        .or_else(|| app.path().data_dir().ok().map(|path| path.join("ethos")))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(not(feature = "e2e"))]
async fn pick_pdf(app: &AppHandle) -> Answer<Option<PathBuf>> {
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Open a PDF")
        .add_filter("PDF documents", &["pdf"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let picked = spawn_blocking(move || dialog.blocking_pick_file())
        .await
        .map_err(|error| error.to_string())?;
    picked
        .map(|path| path.into_path().map_err(|error| error.to_string()))
        .transpose()
}

#[cfg(not(feature = "e2e"))]
async fn pick_destination(app: &AppHandle, suggested: String) -> Answer<Option<PathBuf>> {
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save a copy")
        .set_file_name(suggested)
        .add_filter("PDF documents", &["pdf"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let picked = spawn_blocking(move || dialog.blocking_save_file())
        .await
        .map_err(|error| error.to_string())?;
    picked
        .map(|path| path.into_path().map_err(|error| error.to_string()))
        .transpose()
}

/// Reads the scripted answer the end-to-end test wrote; an empty file cancels the dialog.
#[cfg(feature = "e2e")]
fn scripted_dialog(app: &AppHandle, file: &str) -> Answer<Option<PathBuf>> {
    let answer = std::fs::read_to_string(data_dir(app).join(file))
        .map_err(|_| format!("Native dialog called without a script: {file}"))?;
    Ok((!answer.is_empty()).then(|| PathBuf::from(answer)))
}

#[cfg(feature = "e2e")]
async fn pick_pdf(app: &AppHandle) -> Answer<Option<PathBuf>> {
    scripted_dialog(app, "open-dialog.txt")
}

#[cfg(feature = "e2e")]
async fn pick_destination(app: &AppHandle, suggested: String) -> Answer<Option<PathBuf>> {
    std::fs::write(data_dir(app).join("save-suggestion.txt"), suggested)
        .map_err(|error| error.to_string())?;
    scripted_dialog(app, "save-dialog.txt")
}

#[tauri::command]
async fn open_file(app: AppHandle) -> Answer<Option<StagedFile>> {
    let shell = app.state::<Shell>();
    let _operation = FileOperation::begin(&shell)?;
    if !confirm_discard(&app).await {
        return Ok(None);
    }
    let Some(path) = pick_pdf(&app).await? else {
        return Ok(None);
    };
    read_and_stage(&app, path).map(Some)
}

#[tauri::command]
async fn open_dropped(app: AppHandle, path: PathBuf) -> Answer<Option<StagedFile>> {
    let shell = app.state::<Shell>();
    let _operation = FileOperation::begin(&shell)?;
    if !confirm_discard(&app).await {
        return Ok(None);
    }
    read_and_stage(&app, path).map(Some)
}

#[tauri::command]
async fn open_recent(app: AppHandle, id: String) -> Answer<Option<StagedFile>> {
    let shell = app.state::<Shell>();
    let _operation = FileOperation::begin(&shell)?;
    if !confirm_discard(&app).await {
        return Ok(None);
    }
    let path = shell
        .state()
        .recent
        .iter()
        .find(|item| item.id == id)
        .map(|item| item.path.clone())
        .ok_or("This document is no longer in your recent list.")?;
    read_and_stage(&app, path).map(Some)
}

#[tauri::command]
async fn open_sample(app: AppHandle) -> Answer<StagedFile> {
    let bytes = spawn_blocking(create_sample)
        .await
        .map_err(|error| error.to_string())??;
    Ok(app.state::<Shell>().stage(SAMPLE_NAME.to_owned(), bytes))
}

#[tauri::command]
fn recent_files(shell: tauri::State<'_, Shell>) -> Vec<RecentFile> {
    shell.state().recent.iter().map(Recent::summary).collect()
}

#[tauri::command]
async fn load_document(
    app: AppHandle,
    token: u64,
    password: Option<String>,
) -> Answer<LoadOutcome> {
    let shell = app.state::<Shell>();
    let bytes = match &shell.state().staged {
        Some((staged, bytes)) if *staged == token => Arc::clone(bytes),
        _ => return Err("This file is no longer waiting to open.".into()),
    };
    let worker = app.clone();
    let source = Arc::clone(&bytes);
    let outcome = spawn_blocking(move || {
        worker
            .state::<Shell>()
            .engine
            .load(token, source.to_vec(), password)
    })
    .await
    .map_err(|error| error.to_string())??;
    if matches!(outcome, LoadOutcome::Ready { .. }) {
        let mut state = shell.state();
        state.current = Some(bytes);
        state.staged = None;
    }
    Ok(outcome)
}

#[tauri::command]
fn cancel_load(shell: tauri::State<'_, Shell>) {
    shell.state().staged = None;
}

#[tauri::command]
fn close_document(shell: tauri::State<'_, Shell>) {
    shell.state().current = None;
    shell.engine.close();
}

#[tauri::command]
#[allow(
    clippy::too_many_arguments,
    reason = "command arguments are the IPC schema"
)]
async fn render_page(
    app: AppHandle,
    ticket: u64,
    token: u64,
    source: u32,
    rotation: u16,
    width: u32,
    height: u32,
) -> Answer<Response> {
    let frame = Frame {
        token,
        source,
        rotation,
        width,
        height,
    };
    let rgba = spawn_blocking(move || app.state::<Shell>().engine.render(ticket, frame))
        .await
        .map_err(|error| error.to_string())??;
    Ok(Response::new(rgba))
}

#[tauri::command]
fn cancel_render(shell: tauri::State<'_, Shell>, ticket: u64) {
    shell.engine.cancel(ticket);
}

#[tauri::command]
async fn page_text(app: AppHandle, token: u64, source: u32) -> Answer<Vec<TextRun>> {
    spawn_blocking(move || app.state::<Shell>().engine.text_runs(token, source))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn search_pages(
    app: AppHandle,
    token: u64,
    query: String,
    sources: Vec<u32>,
) -> Answer<Vec<SearchHit>> {
    spawn_blocking(move || app.state::<Shell>().engine.search(token, query, sources))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn saved_theme(app: AppHandle) -> Option<String> {
    files::read_saved_theme(&data_dir(&app))
}

#[tauri::command]
async fn confirm_leave(app: AppHandle) -> bool {
    confirm_discard(&app).await
}

#[tauri::command]
fn answer_discard(shell: tauri::State<'_, Shell>, id: String, discard: bool) {
    let mut state = shell.state();
    if state
        .discard_request
        .as_ref()
        .is_some_and(|(pending, _)| *pending == id)
        && let Some((_, sender)) = state.discard_request.take()
    {
        // A closed receiver means the asker already gave up.
        let _ = sender.try_send(discard);
    }
}

#[tauri::command]
async fn save_copy(app: AppHandle, edits: Edits, name: String) -> Answer<SaveOutcome> {
    let shell = app.state::<Shell>();
    let _operation = FileOperation::begin(&shell)?;
    if name.trim().is_empty() {
        return Err("Choose a filename for your PDF copy.".into());
    }
    let source = shell
        .state()
        .current
        .clone()
        .ok_or("No document is open.")?;
    let bytes = spawn_blocking(move || export_pdf(&source, &edits))
        .await
        .map_err(|error| error.to_string())??;
    let Some(destination) = pick_destination(&app, files::suggested_copy_name(&name)).await? else {
        return Ok(SaveOutcome::Cancelled);
    };
    let sources = shell.state().sources.clone();
    files::write_copy(&destination, &bytes, &sources)?;
    Ok(SaveOutcome::Saved)
}

#[tauri::command]
fn set_dirty(shell: tauri::State<'_, Shell>, dirty: bool) {
    shell.state().dirty = dirty;
}

#[tauri::command]
fn set_title(window: WebviewWindow, title: String) -> Answer<()> {
    window.set_title(&title).map_err(|error| error.to_string())
}

#[tauri::command]
fn window_action(window: WebviewWindow, action: String) -> Answer<()> {
    let result = match action.as_str() {
        "close" => window.close(),
        "minimize" => window.minimize(),
        "maximize" => match window.is_maximized() {
            Ok(true) => window.unmaximize(),
            _ => window.maximize(),
        },
        _ => return Err("Unknown window action.".into()),
    };
    result.map_err(|error| error.to_string())
}

fn open_external(app: AppHandle, path: PathBuf) {
    spawn(async move {
        let shell = app.state::<Shell>();
        let outcome = match FileOperation::begin(&shell) {
            Ok(_operation) if !confirm_discard(&app).await => return,
            Ok(_operation) => read_and_stage(&app, path),
            Err(error) => Err(error),
        };
        // Emitting only fails while the app is shutting down.
        let _ = match outcome {
            Ok(file) => app.emit("file-opened", file),
            Err(error) => app.emit("file-error", error),
        };
    });
}

#[tauri::command]
fn app_ready(app: AppHandle) {
    let path = {
        let shell = app.state::<Shell>();
        let mut state = shell.state();
        if state.interface_ready {
            return;
        }
        state.interface_ready = true;
        state.pending_path.take()
    };
    if let Some(path) = path.or_else(|| files::pdf_argument(std::env::args().skip(1))) {
        open_external(app, path);
    }
}

#[cfg(not(feature = "e2e"))]
fn second_instance(app: &AppHandle, args: Vec<String>) {
    if let Some(window) = app.get_webview_window("main") {
        // Failing to raise the window must not lose the requested file.
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let Some(path) = files::pdf_argument(args.into_iter().skip(1)) else {
        return;
    };
    let shell = app.state::<Shell>();
    let ready = {
        let mut state = shell.state();
        if !state.interface_ready {
            state.pending_path = Some(path.clone());
        }
        state.interface_ready
    };
    if ready {
        open_external(app.clone(), path);
    }
}

fn window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            let app = window.app_handle().clone();
            let (busy, dirty) = {
                let shell = app.state::<Shell>();
                let state = shell.state();
                (state.file_operation, state.dirty)
            };
            if !busy && !dirty {
                return;
            }
            api.prevent_close();
            if busy {
                return;
            }
            let window = window.clone();
            spawn(async move {
                if confirm_discard(&app).await {
                    app.state::<Shell>().state().dirty = false;
                    // The window is already closing if this fails.
                    let _ = window.close();
                }
            });
        }
        WindowEvent::Destroyed => resolve_discard(&window.state::<Shell>()),
        _ => {}
    }
}

fn main() {
    let builder = tauri::Builder::default();
    // End-to-end runs must not hand their window to a real ethos, or to each other.
    #[cfg(not(feature = "e2e"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _| {
        second_instance(app, args);
    }));
    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let library = app.path().resource_dir()?.join("pdfium.dll");
            let recent = files::read_recents(&data_dir(app.handle()));
            app.manage(Shell {
                engine: Engine::start(library),
                tokens: AtomicU64::new(0),
                state: Mutex::new(State {
                    recent,
                    ..State::default()
                }),
            });
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                drop::forward_dropped_files(&window)?;
            }
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started
                && let Some(shell) = webview.try_state::<Shell>()
            {
                resolve_discard(&shell);
            }
        })
        .on_window_event(window_event)
        .invoke_handler(tauri::generate_handler![
            answer_discard,
            app_ready,
            cancel_load,
            cancel_render,
            close_document,
            confirm_leave,
            load_document,
            open_dropped,
            open_file,
            open_recent,
            open_sample,
            page_text,
            recent_files,
            render_page,
            save_copy,
            saved_theme,
            search_pages,
            set_dirty,
            set_title,
            window_action,
        ])
        .run(tauri::generate_context!())
        .expect("ethos failed to start");
}
