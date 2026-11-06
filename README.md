# ethos

A desktop PDF workspace written in Rust. The native shell uses Tauri, the interface is Leptos compiled to WebAssembly, and PDFium renders pages.

## Requirements

- Rust stable with the WebAssembly target: `rustup target add wasm32-unknown-unknown`
- [Trunk](https://trunkrs.dev) and the Tauri CLI: `cargo install trunk tauri-cli`
- On Windows, either the MSVC build tools, or the `x86_64-pc-windows-gnu` toolchain with MinGW on `PATH`
- The WebView2 runtime, which ships with Windows 11

## Run locally

```sh
cargo tauri dev
```

## Use the reader

Open a PDF with **Open PDF**, drag it into the window, or use the built-in sample. Use the sidebar for page navigation, annotations, and text search. The floating toolbar provides text selection, highlighting, drawing, and text notes. Annotations and page rotation or deletion support undo and redo.

Open and Save copy use your operating system's native file dialogs, including its folder shortcuts and overwrite confirmation.

**Save copy** writes your changes into a separate PDF. It preserves the original file. New annotations become page content in the exported copy. They remain editable in ethos until you close the current document, but they are not editable annotation objects when you reopen the exported PDF.

Changes are held in memory. Save before closing. Ethos prompts before discarding unsaved edits. Recent document paths are stored in `recent.json` in `%APPDATA%\ethos`, and your appearance preference is stored in the WebView2 profile. The first launch after an Electron release adopts its saved appearance. Recent PDFs are not duplicated into an app library.

This version supports a single active PDF, files up to 200 MB, and up to 2,000 pages. Protected documents can be read with their password; exporting them is not supported. New text notes use Helvetica and support Western European characters.

## Verify

```sh
cargo test -p ethos-core -p ethos-engine -p create-icon
cargo test -p ethos --bin ethos
cargo clippy --workspace --all-targets
cd ui && trunk build --release && cd ..
cargo test -p ethos --features e2e --test e2e
```

The end-to-end test launches the real app and drives it through WebView2's DevTools protocol. It uses a temporary profile and generated PDFs, and scripts the native file dialogs. It does not touch your normal app profile. Screenshots land in `target/tmp/e2e/`.

## Build a Windows installer

```sh
cargo tauri build
```

The installer is written to `target/release/bundle/nsis/`. Code signing and automatic updates are not configured.

## Layout

| Path | Contents |
| --- | --- |
| `core/` | Types and logic shared by the interface and the engine: geometry, note wrapping, search matching |
| `engine/` | PDF parsing, rendering and text extraction with PDFium; export and the sample PDF with lopdf |
| `src-tauri/` | The desktop shell: windows, file dialogs, bounded file access, the recent list |
| `ui/` | The Leptos interface and its CSS |
| `tools/create-icon/` | Generates the app icons from the approved artwork |

## Current scope

This first release reads PDFs and adds markup. Editing existing PDF text or images, OCR, form filling, signatures, merging files, and printing are not implemented yet.
