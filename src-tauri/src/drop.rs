//! Recovers the real path of a file dropped on the page. The web platform hides paths, so
//! the interface posts the dropped `File` objects to WebView2, which exposes them here.

use tauri::{Emitter, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2,
};
use webview2_com::{CoTaskMemPWSTR, WebMessageReceivedEventHandler};
use windows::core::{Interface, PWSTR, Result};

/// Registers a valid IPC envelope so Tauri and the file handler can share WebView2 messages.
pub fn init(app: &tauri::AppHandle) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let key = serde_json::to_string(app.invoke_key()).expect("the invoke key is a string");
    tauri::plugin::Builder::new("drop")
        .js_init_script(format!(
            r#"
const invokeKey = {key};
window.__ETHOS__ = {{
    postDroppedFiles(files) {{
        const ipc = window.__TAURI_INTERNALS__;
        const callback = ipc.transformCallback(() => {{}}, true);
        try {{
            window.chrome.webview.postMessageWithAdditionalObjects(JSON.stringify({{
                cmd: "plugin:drop|post_files", callback, error: callback, payload: {{}},
                __TAURI_INVOKE_KEY__: invokeKey
            }}), files);
        }} catch (error) {{
            ipc.unregisterCallback(callback);
            throw error;
        }}
    }}
}};
"#
        ))
        .invoke_handler(tauri::generate_handler![post_files])
        .build()
}

// The WebMessageReceived handler reads the attached files, not the IPC command.
#[tauri::command]
fn post_files() {}

/// Reads the paths of the files a web message carries.
unsafe fn file_paths(args: &ICoreWebView2WebMessageReceivedEventArgs2) -> Result<Vec<String>> {
    unsafe {
        let objects = args.AdditionalObjects()?;
        let mut count = 0;
        objects.Count(&mut count)?;
        let mut paths = Vec::new();
        for index in 0..count {
            let file: ICoreWebView2File = objects.GetValueAtIndex(index)?.cast()?;
            let mut path = PWSTR::null();
            file.Path(&mut path)?;
            paths.push(CoTaskMemPWSTR::from(path).to_string());
        }
        Ok(paths)
    }
}

/// Emits `file-dropped` with the paths of the files the interface posts.
pub fn forward_dropped_files(window: &WebviewWindow) -> tauri::Result<()> {
    let target = window.clone();
    window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            return;
        };
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let paths = file_paths(&args.cast()?)?;
            if !paths.is_empty() {
                // Emitting only fails while the app is shutting down.
                let _ = target.emit("file-dropped", paths);
            }
            Ok(())
        }));
        let mut token = 0;
        // Without this handler drops do nothing, which a failed setup leaves as the only harm.
        let _ = core.add_WebMessageReceived(&handler, &mut token);
    })
}
