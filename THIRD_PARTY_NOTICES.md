# Third-party notices

Ethos includes or derives from the following work.

## PDFium

`src-tauri/pdfium/pdfium.dll` is a build of [PDFium](https://pdfium.googlesource.com/pdfium/) from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries), release chromium/8138.

- The build scripts are MIT licensed. See `src-tauri/pdfium/LICENSE`.
- PDFium and the libraries compiled into it use the licenses in `src-tauri/pdfium/licenses/`.

The installer ships these files next to the app.

## Lucide icons

The `.svg` files in `ui/icons/` come from [Lucide](https://lucide.dev), ISC licensed.

```text
ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

## Helvetica metrics

`core/src/helvetica.rs` holds character widths from the Adobe Core 14 font metrics. They measure note text; no font is included.

## Rust crates

Cargo downloads the crates listed in `Cargo.lock`, each under its own license. Published binaries must carry the notices those licenses require.
