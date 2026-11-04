# ethos identity

Approved direction: **A, Margin**, in **Forest**.

The mark separates a vertical page margin from three text lines to form an E. The shorter middle line preserves recognition at toolbar sizes.

- Forest: `#284d3c`
- Paper: `#faf9f4`
- Wordmark: lowercase `ethos`, using the app's system sans serif.

`scripts/create-icon.py` is the source of the mark's geometry: a superellipse tile with a top-lit forest gradient. It writes `public/ethos.svg`, `public/ethos.png`, `public/ethos.ico` and `build/icon.{png,ico}`, rendering each `.ico` frame (16 to 256 px) on its own pixel grid so the bars stay crisp on the taskbar at every display scale. Run `python scripts/create-icon.py` to regenerate them. This command only generates artwork; it does not build or package the app.
