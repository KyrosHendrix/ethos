//! Typed calls into the native shell.

use ethos_core::{Edits, LoadOutcome, RecentFile, SaveOutcome, SearchHit, StagedFile, TextRun};
use js_sys::{ArrayBuffer, Uint8ClampedArray};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["__TAURI__", "core"], js_name = invoke)]
    async fn tauri_invoke(command: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["__TAURI__", "event"], js_name = listen)]
    fn tauri_listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>) -> js_sys::Promise;

    #[wasm_bindgen(catch, js_namespace = ["chrome", "webview"], js_name = postMessageWithAdditionalObjects)]
    fn post_with_objects(message: &str, objects: &web_sys::FileList) -> Result<(), JsValue>;
}

fn encode(value: &impl Serialize) -> Result<JsValue, String> {
    value
        .serialize(&Serializer::json_compatible())
        .map_err(|error| error.to_string())
}

async fn raw_call(command: &str, args: serde_json::Value) -> Result<JsValue, String> {
    tauri_invoke(command, encode(&args)?)
        .await
        .map_err(|error| {
            error
                .as_string()
                .unwrap_or_else(|| "Something went wrong.".into())
        })
}

async fn call<T: DeserializeOwned>(command: &str, args: serde_json::Value) -> Result<T, String> {
    serde_wasm_bindgen::from_value(raw_call(command, args).await?)
        .map_err(|error| error.to_string())
}

/// Fires a command whose failure the interface cannot act on.
fn notify_shell(command: &'static str, args: serde_json::Value) {
    wasm_bindgen_futures::spawn_local(async move {
        let _ = raw_call(command, args).await;
    });
}

/// Calls `handler` for every `event` for the rest of the page's life.
pub fn listen<T: DeserializeOwned + 'static>(event: &str, mut handler: impl FnMut(T) + 'static) {
    let closure = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let payload = js_sys::Reflect::get(&event, &"payload".into()).unwrap_or(JsValue::NULL);
        if let Ok(payload) = serde_wasm_bindgen::from_value(payload) {
            handler(payload);
        }
    });
    let _ = tauri_listen(event, &closure);
    closure.forget();
}

pub async fn open_file() -> Result<Option<StagedFile>, String> {
    call("open_file", json!({})).await
}

pub async fn open_recent(id: String) -> Result<Option<StagedFile>, String> {
    call("open_recent", json!({ "id": id })).await
}

pub async fn open_dropped(path: String) -> Result<Option<StagedFile>, String> {
    call("open_dropped", json!({ "path": path })).await
}

/// Hands dropped files to the shell, which answers with a `file-dropped` event.
pub fn post_dropped_files(files: &web_sys::FileList) -> Result<(), String> {
    post_with_objects("ethos-dropped-files", files)
        .map_err(|_| "Could not open this PDF.".to_owned())
}

pub async fn saved_theme() -> Option<String> {
    call("saved_theme", json!({})).await.ok().flatten()
}

pub async fn open_sample() -> Result<StagedFile, String> {
    call("open_sample", json!({})).await
}

pub async fn recent_files() -> Result<Vec<RecentFile>, String> {
    call("recent_files", json!({})).await
}

pub async fn load_document(token: u64, password: Option<String>) -> Result<LoadOutcome, String> {
    call(
        "load_document",
        json!({ "token": token, "password": password }),
    )
    .await
}

pub fn cancel_load() {
    notify_shell("cancel_load", json!({}));
}

pub async fn close_document() {
    let _ = raw_call("close_document", json!({})).await;
}

pub async fn confirm_leave() -> Result<bool, String> {
    call("confirm_leave", json!({})).await
}

pub fn answer_discard(id: String, discard: bool) {
    notify_shell("answer_discard", json!({ "id": id, "discard": discard }));
}

pub async fn save_copy(edits: &Edits, name: &str) -> Result<SaveOutcome, String> {
    let edits = serde_json::to_value(edits).map_err(|error| error.to_string())?;
    call("save_copy", json!({ "edits": edits, "name": name })).await
}

/// One rendered page bitmap request.
#[derive(Clone, Copy, Serialize)]
pub struct RenderRequest {
    pub ticket: u64,
    pub token: u64,
    pub source: u32,
    pub rotation: u16,
    pub width: u32,
    pub height: u32,
}

/// Returns RGBA pixels without copying them through WebAssembly memory.
pub async fn render_page(request: RenderRequest) -> Result<Uint8ClampedArray, String> {
    let args = serde_json::to_value(request).map_err(|error| error.to_string())?;
    let buffer: ArrayBuffer = raw_call("render_page", args)
        .await?
        .dyn_into()
        .map_err(|_| "Page data was not binary.".to_owned())?;
    Ok(Uint8ClampedArray::new(&buffer))
}

pub fn cancel_render(ticket: u64) {
    notify_shell("cancel_render", json!({ "ticket": ticket }));
}

pub async fn page_text(token: u64, source: u32) -> Result<Vec<TextRun>, String> {
    call("page_text", json!({ "token": token, "source": source })).await
}

pub async fn search_pages(
    token: u64,
    query: &str,
    sources: &[u32],
) -> Result<Vec<SearchHit>, String> {
    let args = json!({ "token": token, "query": query, "sources": sources });
    call("search_pages", args).await
}

pub fn set_dirty(dirty: bool) {
    notify_shell("set_dirty", json!({ "dirty": dirty }));
}

/// Finishes before returning, so a following leave request sees the current state.
pub async fn sync_dirty(dirty: bool) -> Result<(), String> {
    raw_call("set_dirty", json!({ "dirty": dirty }))
        .await
        .map(drop)
}

pub fn set_title(title: &str) {
    notify_shell("set_title", json!({ "title": title }));
}

pub fn window_action(action: &'static str) {
    notify_shell("window_action", json!({ "action": action }));
}

pub fn app_ready() {
    notify_shell("app_ready", json!({}));
}
