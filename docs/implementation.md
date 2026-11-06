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

## Tools

The rule is that tools never compete with reading. There is one list of tools, summoned from the wrench or `Ctrl K`, plus actions that appear in context. There is no grid. A tool asks only what it must, in a short sheet, then asks where to save before any work begins, so nobody waits to be asked a question. Long tools report progress in a status pill that leaves the document readable and can be cancelled.

Whole-document tools work a few pages at a time on the PDFium thread, against a temporary second document. Rendering jobs queue between those turns, which is what keeps scrolling smooth during OCR. Every tool writes a new file. The interface can only ask to open paths a tool wrote.

- **Structure** (`engine/src/structure.rs`): Word, Markdown and text share one reading of a page. Lines are grouped by baseline into headings, paragraphs, lists and tables, and pictures are cropped from a render.
- **Word to PDF** (`engine/src/word.rs`): reads the `.docx` XML, resolves style chains, lays out lines with real font metrics, and draws with PDFium using embedded system fonts. `word_round_trip` checks it against files written by Word.
- **OCR** (`engine/src/ocr.rs`): Windows' own engine reads a render, and the words go back as invisible text in a font whose codes are Unicode, so any script stays searchable.
- **Redaction** (`engine/src/media.rs`): covered pages are rebuilt as pictures with the areas painted black, unreferenced objects are pruned, and the result is reopened to confirm no text remains. If that check fails, nothing is saved.
- **Compare** (`engine/src/compare.rs`): diffs lines, so changes read the way people scan them.
- **Forms** (`engine/src/forms.rs`): values live in `Edits` like marks, so they save, restore and undo the same way.

## PDF libraries

PDFium renders because no pure-Rust renderer matches its fidelity on real documents, and it also handles password-protected files. It is the one non-Rust component. `src-tauri/pdfium/pdfium.dll` is release chromium/8138 of `bblanchon/pdfium-binaries`, SHA-256 of the archive `8d48973e3eb472dda0f5f2435e595ac043f9f2219dd71883392f549445afa8a5`. `pdfium-render` binds to the 8125 API, which that release still provides.

lopdf exports because it edits the object tree directly. Untouched objects, including form fields, stay as they were, and exports leave no dependency on PDFium's writer.

## Keeping marks

Marks never touch the original file. Edits are written to `marks/<sha256>.json` in the data folder 300 ms after each change. The content hash makes them follow a renamed or moved file, and a changed file gets none, because its coordinates would no longer fit. A restored session keeps the untouched document one undo step behind it, so a deleted page stays recoverable.

The sample and password-protected files keep no session: the sample is a fixed demo, and a session would write a protected document's text to disk unencrypted. For those, and when a write fails, leaving asks first.

Save copy writes Highlight, Ink and FreeText annotations with appearance streams, so other PDF apps can display and edit them. Page content streams are left alone. Each annotation also stores its exact mark in a private `EthosMark` key. When ethos opens a file containing that key, `split_marks` takes those annotations out and hands them to the interface as editable marks, so they are not drawn twice. Other apps' annotations are kept and rendered. If another app rewrites the file with compressed objects, its marks stop being recognized and behave as ordinary annotations.

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
