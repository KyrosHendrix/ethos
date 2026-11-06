#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod dialogs;
#[cfg(windows)]
mod drop;
mod files;
mod tools;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ethos_core::{
    Edits, LoadOutcome, RecentFile, SAMPLE_NAME, SaveOutcome, SearchHit, StagedFile, TextRun,
    ToolOutcome, ToolRequest,
};
use ethos_engine::{Engine, Frame, create_sample, export_pdf, read_fields, split_marks};
use files::Recent;
use tauri::async_runtime::{Sender, channel, spawn, spawn_blocking};
use tauri::ipc::Response;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, WindowEvent};

pub(crate) type Answer<T> = Result<T, String>;

/// A file read from disk, waiting for the interface to parse it.
struct Staged {
    token: u64,
    bytes: Arc<Vec<u8>>,
    /// Content hash for files on disk. The sample has none and keeps nothing.
    key: Option<String>,
}

/// Everything shared between commands, window events and the second-instance callback.
pub(crate) struct Shell {
    pub(crate) engine: Engine,
    tokens: AtomicU64,
    pub(crate) tool_running: AtomicBool,
    pub(crate) tool_cancelled: AtomicBool,
    state: Mutex<State>,
}

#[derive(Default)]
pub(crate) struct State {
    dirty: bool,
    interface_ready: bool,
    file_operation: bool,
    pending_path: Option<PathBuf>,
    discard_request: Option<(String, Sender<bool>)>,
    recent: Vec<Recent>,
    /// Every source stays protected, even if the interface rejects a newly read PDF.
    pub(crate) sources: Vec<PathBuf>,
    staged: Option<Staged>,
    pub(crate) current: Option<Arc<Vec<u8>>>,
    /// The password that opened the current document, kept only while it is open.
    pub(crate) password: Option<String>,
    /// Files tools wrote, the only paths the interface may ask to open.
    pub(crate) outputs: Vec<PathBuf>,
    /// Which saved session the open document writes to, if it keeps one.
    session: Option<(u64, String)>,
}

impl Shell {
    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("shell state poisoned")
    }

    fn stage(&self, name: String, bytes: Vec<u8>, key: Option<String>) -> StagedFile {
        let token = self.tokens.fetch_add(1, Ordering::Relaxed) + 1;
        let size = bytes.len() as u64;
        self.state().staged = Some(Staged {
            token,
            bytes: Arc::new(bytes),
            key,
        });
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
    Ok(shell.stage(opened.name, opened.bytes, Some(opened.key)))
}

pub(crate) fn data_dir(app: &AppHandle) -> PathBuf {
    std::env::var_os("ETHOS_USER_DATA")
        .map(PathBuf::from)
        .or_else(|| app.path().data_dir().ok().map(|path| path.join("ethos")))
        .unwrap_or_else(std::env::temp_dir)
}

#[tauri::command]
async fn open_file(app: AppHandle) -> Answer<Option<StagedFile>> {
    let shell = app.state::<Shell>();
    let _operation = FileOperation::begin(&shell)?;
    if !confirm_discard(&app).await {
        return Ok(None);
    }
    let Some(path) = dialogs::pick_pdf(&app).await? else {
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
    Ok(app
        .state::<Shell>()
        .stage(SAMPLE_NAME.to_owned(), bytes, None))
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
    let (bytes, key) = match &shell.state().staged {
        Some(staged) if staged.token == token => (Arc::clone(&staged.bytes), staged.key.clone()),
        _ => return Err("This file is no longer waiting to open.".into()),
    };
    // Saved edits hold document text, so a protected file's are never written to disk.
    let key = key.filter(|_| password.is_none());
    let unlocked_with = password.clone();
    let worker = app.clone();
    let (mut outcome, bytes, carried) = spawn_blocking(move || {
        let (clean, carried) = split_marks(&bytes);
        let bytes = clean.map_or(bytes, Arc::new);
        let shell = worker.state::<Shell>();
        let outcome = shell.engine.load(token, bytes.to_vec(), password);
        outcome.map(|outcome| (outcome, bytes, carried))
    })
    .await
    .map_err(|error| error.to_string())??;
    if let LoadOutcome::Ready {
        pages,
        marks,
        restored,
        fields,
        protected,
    } = &mut outcome
    {
        *marks = carried;
        *fields = read_fields(&bytes);
        *protected = unlocked_with.is_some();
        *restored = key
            .as_ref()
            .and_then(|key| files::read_session(&data_dir(&app), key, pages.len()));
        let mut state = shell.state();
        state.current = Some(bytes);
        state.password = unlocked_with;
        state.staged = None;
        state.session = key.map(|key| (token, key));
    }
    Ok(outcome)
}

/// Records the open document's edits. Returns false when it does not keep a session.
#[tauri::command]
fn save_session(app: AppHandle, token: u64, edits: Edits) -> Answer<bool> {
    let session = app.state::<Shell>().state().session.clone();
    match session {
        Some((current, key)) if current == token => {
            files::write_session(&data_dir(&app), &key, &edits).map(|()| true)
        }
        _ => Ok(false),
    }
}

#[tauri::command]
fn cancel_load(shell: tauri::State<'_, Shell>) {
    shell.state().staged = None;
}

#[tauri::command]
fn close_document(shell: tauri::State<'_, Shell>) {
    {
        let mut state = shell.state();
        state.current = None;
        state.password = None;
        state.session = None;
    }
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
    let worker = app.clone();
    let bytes = spawn_blocking(move || {
        let exported = export_pdf(&source, &edits)?;
        tools::redacted_copy(&worker.state::<Shell>(), exported, &edits, |_, _| true)
    })
    .await
    .map_err(|error| error.to_string())??;
    let suggested = files::suggested_copy_name(&name);
    let filter = dialogs::Filter {
        name: "PDF documents",
        extensions: &["pdf"],
    };
    let Some(destination) =
        dialogs::pick_destination(&app, suggested, &filter, "Save a copy").await?
    else {
        return Ok(SaveOutcome::Cancelled);
    };
    let sources = shell.state().sources.clone();
    files::write_copy(&destination, &bytes, &sources)?;
    Ok(SaveOutcome::Saved)
}

#[tauri::command]
async fn run_tool(
    app: AppHandle,
    request: ToolRequest,
    edits: Option<Edits>,
    name: String,
) -> Answer<ToolOutcome> {
    match tools::run(app, request, edits, name).await {
        Err(error) if error == ethos_engine::CANCELLED => Ok(ToolOutcome::Cancelled),
        outcome => outcome,
    }
}

#[tauri::command]
async fn pick_paths(app: AppHandle, kind: String) -> Answer<Vec<PathBuf>> {
    let filter = if kind == "pictures" {
        tools::PICTURES
    } else {
        tools::PDFS
    };
    dialogs::pick_files(&app, &filter, "Choose files").await
}

#[tauri::command]
fn cancel_tool(shell: tauri::State<'_, Shell>) {
    shell.tool_cancelled.store(true, Ordering::SeqCst);
}

/// Shows a tool's result. Only paths a tool wrote are accepted.
#[tauri::command]
fn reveal_output(shell: tauri::State<'_, Shell>, path: PathBuf, open: bool) -> Answer<()> {
    if !shell.state().outputs.contains(&path) {
        return Err("This file is no longer available.".into());
    }
    let mut command = std::process::Command::new("explorer");
    if open || path.is_dir() {
        command.arg(&path);
    } else {
        command.arg("/select,").arg(&path);
    }
    // Explorer reports a failure exit code even when it succeeds.
    command.spawn().map(drop).map_err(|error| error.to_string())
}

#[tauri::command]
async fn open_output(app: AppHandle, path: PathBuf) -> Answer<Option<StagedFile>> {
    let shell = app.state::<Shell>();
    if !shell.state().outputs.contains(&path) {
        return Err("This file is no longer available.".into());
    }
    let _operation = FileOperation::begin(&shell)?;
    if !confirm_discard(&app).await {
        return Ok(None);
    }
    read_and_stage(&app, path).map(Some)
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
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let library = app.path().resource_dir()?.join("pdfium.dll");
            let recent = files::read_recents(&data_dir(app.handle()));
            app.manage(Shell {
                engine: Engine::start(library),
                tokens: AtomicU64::new(0),
                tool_running: AtomicBool::new(false),
                tool_cancelled: AtomicBool::new(false),
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
            cancel_tool,
            close_document,
            confirm_leave,
            load_document,
            open_dropped,
            open_file,
            open_output,
            open_recent,
            open_sample,
            page_text,
            pick_paths,
            recent_files,
            render_page,
            reveal_output,
            run_tool,
            save_copy,
            save_session,
            saved_theme,
            search_pages,
            set_dirty,
            set_title,
            window_action,
        ])
        .build(tauri::generate_context!())
        .expect("ethos failed to start");
    #[cfg(windows)]
    app.handle()
        .plugin(drop::init(app.handle()))
        .expect("file drops failed to initialize");
    app.run(|_, _| {});
}
