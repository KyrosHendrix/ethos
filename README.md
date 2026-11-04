# ethos

A desktop PDF workspace built from scratch with Electron, React, TypeScript, and Vite.

## Run locally

Requires Node.js 22.12 or newer and npm.

```sh
npm install
npm run dev
```

To run a production build:

```sh
npm run build
npm start
```

If your terminal sets `ELECTRON_RUN_AS_NODE`, remove that variable before starting Electron.

## Use the reader

Open a PDF with **Open PDF**, drag it into the window, or use the built-in sample. Use the sidebar for page navigation, annotations, and text search. The floating toolbar provides text selection, highlighting, drawing, and text notes. Annotations and page rotation or deletion support undo and redo.

**Save copy** writes your changes into a separate PDF. It preserves the original file. New annotations become page content in the exported copy. They remain editable in ethos until you close the current document, but they are not editable annotation objects when you reopen the exported PDF.

Changes are held in memory. Save before closing. Ethos prompts before discarding unsaved edits. Recent document paths and your appearance preference are stored locally. Recent PDFs are not duplicated into an app library.

This version supports a single active PDF, files up to 200 MB, and up to 2,000 pages. Protected documents can be read with their password; exporting them is not supported. New text notes use Helvetica and support Western European characters.

## Verify

```sh
npm run typecheck
npm test
npm run test:e2e
```

The Electron smoke test uses a temporary app profile and generated PDFs. It does not touch your normal app profile.

## Build a Windows installer

```sh
npm run dist
```

Build artifacts are written to `release/`. Code signing and automatic updates are not configured.

## Current scope

This first release reads PDFs and adds markup. Editing existing PDF text or images, OCR, form filling, signatures, merging files, and printing are not implemented yet.
