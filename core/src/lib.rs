//! Types and pure logic shared by the WASM interface and the native engine.

mod geometry;
mod helvetica;
mod search;

pub use geometry::{RunLayout, Viewport, display_rotation};
pub use helvetica::{encode_win_ansi, wrap_text_note};
pub use search::find_hit;

use serde::{Deserialize, Serialize};

pub const MAX_PAGES: usize = 2000;
pub const MAX_FILE_SIZE: u64 = 200 * 1024 * 1024;
/// Line advance for text notes, in PDF points.
pub const NOTE_LINE_HEIGHT: f64 = 17.0;
pub const NOTE_FONT_SIZE: f64 = 12.0;
pub const SAMPLE_NAME: &str = "A little more room to think.pdf";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InkColor {
    Yellow,
    Mint,
    Blue,
    Rose,
}

impl InkColor {
    pub const ALL: [Self; 4] = [Self::Yellow, Self::Mint, Self::Blue, Self::Rose];

    pub fn name(self) -> &'static str {
        match self {
            Self::Yellow => "yellow",
            Self::Mint => "mint",
            Self::Blue => "blue",
            Self::Rose => "rose",
        }
    }

    /// Fill used for highlights and swatches.
    pub fn ink(self) -> &'static str {
        match self {
            Self::Yellow => "#f4ce56",
            Self::Mint => "#78bda3",
            Self::Blue => "#86aee8",
            Self::Rose => "#db96ac",
        }
    }

    /// Darker color used for pen strokes and note text.
    pub fn stroke(self) -> &'static str {
        match self {
            Self::Yellow => "#9c720b",
            Self::Mint => "#34785c",
            Self::Blue => "#365f9d",
            Self::Rose => "#9c4263",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MarkKind {
    Highlight { rects: Vec<Rect>, text: String },
    Pen { points: Vec<Point> },
    Text { point: Point, text: String },
}

/// An annotation, addressed by its source page so it survives page deletion and undo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub id: String,
    pub page: u32,
    pub color: InkColor,
    #[serde(flatten)]
    pub kind: MarkKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageEntry {
    pub source: u32,
    /// `None` keeps the rotation stored in the document.
    pub rotation: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edits {
    pub marks: Vec<Mark>,
    pub pages: Vec<PageEntry>,
}

/// Page geometry as stored in the PDF: `view` is `[left, bottom, right, top]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    pub view: [f64; 4],
    pub rotate: u16,
}

/// A file read from disk and waiting to be parsed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StagedFile {
    pub token: u64,
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum LoadOutcome {
    Ready { pages: Vec<PageInfo> },
    Password { incorrect: bool },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFile {
    pub id: String,
    pub name: String,
    pub opened_at: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveOutcome {
    Saved,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub page: u32,
    pub snippet: String,
    pub count: usize,
}

/// One line of page text in PDF coordinates; `rect` spans the line's glyph boxes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    pub text: String,
    pub rect: Rect,
    /// Reading direction on the unrotated page, counterclockwise: 0, 90, 180 or 270.
    pub direction: u16,
}
