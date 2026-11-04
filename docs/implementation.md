# First release decisions

The first working release must open a real PDF, display selectable text, navigate and search pages, add annotations with undo and redo, and export a readable copy through a native save dialog. Verification uses generated PDFs and the actual Electron application.

## Architecture choice

Two designs were considered before implementation:

| Design                                 | File ownership | Rendering and edits                                   | Tradeoff                                                                            |
| -------------------------------------- | -------------- | ----------------------------------------------------- | ----------------------------------------------------------------------------------- |
| Main process owns the PDF engine       | Main process   | Stream page images and edit commands to the interface | More IPC, bitmap transfers, and separate text-selection plumbing                    |
| Sandboxed renderer owns the PDF engine | Main process   | PDF.js and pdf-lib in the renderer                    | Keeps coordinates, selection, and previews together; one worker handles PDF parsing |

The second design keeps the native interface small. The main process handles file dialogs, bounded file reads, copy writes, and window lifecycle. The preload exposes explicit operations. The renderer has no Node.js access. Review of the alternatives ran inline without a separate judge.

## Memory choices

- Keep one active document.
- Render page canvases only near the visible viewport. Release canvases and text layers outside it.
- Render sidebar thumbnails only near the visible sidebar region.
- Limit page bitmap resolution and undo history.
- Destroy the previous PDF worker when switching or closing documents.

These choices bound several sources of growth. They are not a measured RAM guarantee. PDF bytes, PDF.js caches, and decoded images still depend on the document.

## Verification boundaries

Type checking checks the integration contracts. PDF roundtrip tests check output documents. Electron smoke coverage exercises the interface and native IPC using a temporary user profile. Installer signing and representative memory measurements need separate verification.
