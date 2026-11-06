//! Recovers the real path of a file dropped on the page. The web platform hides paths, so
//! the interface posts the dropped `File` objects to WebView2, which exposes them here.

use tauri::{Emitter, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2,
};
use webview2_com::{CoTaskMemPWSTR, WebMessageReceivedEventHandler};
use windows::core::{Interface, PWSTR, Result};

/// Reads the first file a web message carries, if it carries any.
unsafe fn first_file_path(
    args: &ICoreWebView2WebMessageReceivedEventArgs2,
) -> Result<Option<String>> {
    unsafe {
        let objects = args.AdditionalObjects()?;
        let mut count = 0;
        objects.Count(&mut count)?;
        if count == 0 {
            return Ok(None);
        }
        let file: ICoreWebView2File = objects.GetValueAtIndex(0)?.cast()?;
        let mut path = PWSTR::null();
        file.Path(&mut path)?;
        Ok(Some(CoTaskMemPWSTR::from(path).to_string()))
    }
}

/// Emits `file-dropped` with the path of each file the interface posts.
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
            if let Some(path) = first_file_path(&args.cast()?)? {
                // Emitting only fails while the app is shutting down.
                let _ = target.emit("file-dropped", path);
            }
            Ok(())
        }));
        let mut token = 0;
        // Without this handler drops do nothing, which a failed setup leaves as the only harm.
        let _ = core.add_WebMessageReceived(&handler, &mut token);
    })
}
