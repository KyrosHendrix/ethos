# Implementation decisions

The app must open a real PDF, display selectable text, navigate and search pages, add annotations with undo and redo, and export a readable copy through a native save dialog. Verification uses generated PDFs and the actual desktop application.

## Architecture

Ethos is a Cargo workspace with no authored JavaScript.

| Layer | Crate | Technology | Responsibility |
| --- | --- | --- | --- |
| Interface | `ui` | Leptos, compiled to WebAssembly by Trunk | All views and interface state |
| Shared logic | `core` | Plain Rust | Edit types, page geometry, note wrapping, search matching |
| Engine | `engine` | PDFium through `pdfium-render`, lopdf | Parsing, rendering, text extraction, export, the sample PDF |
| Shell | `src-tauri` | Tauri 2 over WebView2 | Window lifecycle, native dialogs, bounded file reads, copy writes |

The interface reuses the original HTML structure and CSS in a WebView, which keeps layout, motion and typography unchanged. A native Rust GUI toolkit could not reproduce them.

The engine owns the PDF, so the interface asks for pixels and text. One thread owns PDFium because PDFium is not safe to call concurrently. Render jobs carry a ticket so pages scrolled out of view can be skipped while queued. Requests carry a document token, so answers about a replaced document fail instead of drawing the wrong page.

Page bitmaps cross the bridge as raw RGBA and go straight to a canvas without passing through WebAssembly memory. Selectable text is an invisible span per line, positioned from PDFium's line boxes and stretched to the rendered width.

File drops use HTML5 drag and drop. The web platform hides file paths, so the interface posts the dropped `File` objects to WebView2 with `postMessageWithAdditionalObjects`, and `src-tauri/src/drop.rs` reads the path from the message.

On first launch the interface asks the shell for the appearance saved by Electron releases. The shell copies Electron's Local Storage LevelDB to a temporary folder, reads it with `rusty-leveldb`, and leaves the original untouched.

The interface can only call the commands registered in `src-tauri/src/main.rs`, plus window dragging and events through `src-tauri/capabilities/default.json`.

## PDF libraries

PDFium renders because no pure-Rust renderer matches its fidelity on real documents, and it also handles password-protected files. It is the one non-Rust component. `src-tauri/pdfium/pdfium.dll` is release chromium/8138 of `bblanchon/pdfium-binaries`, SHA-256 of the archive `8d48973e3eb472dda0f5f2435e595ac043f9f2219dd71883392f549445afa8a5`. `pdfium-render` binds to the 8125 API, which that release still provides.

lopdf exports because it edits the object tree directly. Untouched objects, including form fields, stay as they were, and exports leave no dependency on PDFium's writer. Marks are appended as a new content stream, and the original content is wrapped in a save and restore so its graphics state cannot move them.

## Memory choices

- Keep one active document.
- Render page canvases only near the visible viewport. Release canvases and text layers outside it.
- Render sidebar thumbnails only near the visible sidebar region.
- Limit page bitmap resolution and undo history.
- Replace the engine's document when switching, and release it when closing.
- Cap cached search text at 4 million characters, dropping the least recently used page first.

These choices bound several sources of growth. They are not a measured RAM guarantee. The document is held twice, once for PDFium and once for export, and PDFium's caches and decoded images depend on the document.

## Verification boundaries

Unit tests cover shared logic and file handling. Engine tests run real PDFs through PDFium and lopdf. The end-to-end test drives the real app with real input through WebView2's DevTools protocol, using a temporary profile.

The `e2e` feature replaces the native file dialogs with scripted answers and leaves out the single-instance plugin, so native dialogs and second-instance file opening are outside that test. File drops go through Chromium's drag pipeline, and the title bar test checks that a press becomes a window drag, but neither performs a physical mouse gesture. Installer signing and representative memory measurements need separate verification.
