# ethos identity

The approved mark is **01, Rising knot**: an ascending white ribbon with two elongated loops on a red square. The symbol is abstract and contains no letter or monogram.

The original approved artwork lives in `src-tauri/icons/rising-knot.png`. It is the source for every app icon. The generator applies a superellipse mask with transparent corners, preserving the ribbon's shape and colors.

App accents use red `#b90d00` in light mode and coral `#ff9a90` in dark mode for readable controls against dark surfaces. Neutral backgrounds keep the documents in focus.

Run `cargo run -p create-icon` from the repository root to regenerate the assets:

- `ui/public/ethos.svg`, used by the app and favicon, embeds the rounded PNG. It is an SVG container, not vector artwork.
- `ui/public/ethos.png` and `src-tauri/icons/icon.png` are 512 px PNGs.
- `ui/public/ethos.ico` and `src-tauri/icons/icon.ico` contain 14 sizes, from 16 to 256 px, for Windows display scales and packaging.

The generator checks that each ICO holds all 14 frames, and that every frame has transparent corners and an opaque center. It only generates artwork and does not build, package or launch the app.

The wordmark remains lowercase `ethos` in the app's system sans serif.
