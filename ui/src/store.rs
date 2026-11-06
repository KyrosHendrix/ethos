//! Application state and the actions that change it.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use ethos_core::{
    Change, Edits, FieldValue, FormField, InkColor, LoadOutcome, Mark, MarkKind, NOTE_LINE_HEIGHT,
    PageEntry, PageInfo, Point, RecentFile, Rect, SaveOutcome, SearchHit, StagedFile, ToolOutcome,
    ToolProgress, ToolRequest, ToolResult, display_rotation, wrap_text_note,
};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::{ScrollBehavior, ScrollIntoViewOptions, ScrollLogicalPosition};

use crate::bridge;

pub const UNDO_LIMIT: usize = 40;
pub const NOTE_LIMIT: usize = 240;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Highlight,
    Pen,
    Text,
    Sign,
    Redact,
}

impl Tool {
    pub fn class(self) -> &'static str {
        match self {
            Self::Select => "tool-select",
            Self::Highlight => "tool-highlight",
            Self::Pen => "tool-pen",
            Self::Text => "tool-text",
            Self::Sign => "tool-sign",
            Self::Redact => "tool-redact",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Select => "Select text",
            Self::Highlight => "Highlight",
            Self::Pen => "Draw",
            Self::Text => "Add text",
            Self::Sign => "Sign",
            Self::Redact => "Redact",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Select => "Select text to copy",
            Self::Highlight => "Select a passage to highlight it",
            Self::Pen => "Draw directly on the page",
            Self::Text => "Click the page to place a text note",
            Self::Sign => "Click the page to place your signature, then drag it into position",
            Self::Redact => {
                "Drag over anything to remove it. It is gone for good in the copy you save"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Pages,
    Notes,
    Search,
    Changes,
}

impl Tab {
    pub const ALL: [Self; 3] = [Self::Pages, Self::Notes, Self::Search];

    pub fn id(self) -> &'static str {
        match self {
            Self::Pages => "document-tab-pages",
            Self::Notes => "document-tab-notes",
            Self::Search => "document-tab-search",
            Self::Changes => "document-tab-changes",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pages => "Pages",
            Self::Notes => "Notes",
            Self::Search => "Search",
            Self::Changes => "Changes",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    Fit,
    Scale(f64),
}

impl Zoom {
    pub fn value(self) -> String {
        match self {
            Self::Fit => "fit".into(),
            Self::Scale(scale) => scale.to_string(),
        }
    }
}

pub struct Document {
    pub token: u64,
    pub name: String,
    pub size: u64,
    pub pages: Vec<PageInfo>,
}

impl Document {
    /// The file name without its `.pdf` ending.
    pub fn title(&self) -> String {
        let end = self.name.len().checked_sub(4).filter(|&end| {
            self.name.is_char_boundary(end) && self.name[end..].eq_ignore_ascii_case(".pdf")
        });
        end.map_or_else(|| self.name.clone(), |end| self.name[..end].to_owned())
    }

    pub fn info(&self, source: u32) -> Option<PageInfo> {
        self.pages.get(source as usize - 1).copied()
    }
}

#[derive(Clone)]
pub struct History {
    pub past: Vec<Arc<Edits>>,
    pub present: Arc<Edits>,
    pub future: Vec<Arc<Edits>>,
}

impl History {
    fn fresh(present: Arc<Edits>) -> Self {
        Self {
            past: Vec::new(),
            present,
            future: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub undoable: bool,
    /// A finished tool's file, offered to open or find.
    pub result: Option<ToolResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesKind {
    Pdfs,
    Pictures,
}

impl FilesKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdfs => "pdfs",
            Self::Pictures => "pictures",
        }
    }
}

/// A short form a tool needs before it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    Notes,
    Split,
    Extract,
    Pictures,
    Protect,
    Signature,
    Files(FilesKind, Vec<PathBuf>),
}

/// How the tools list was summoned, which decides where it appears and whether it animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteOrigin {
    Keyboard,
    Toolbar,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub name: String,
    pub changes: Vec<Change>,
}

/// Parses a page list such as `1-3, 5` into page numbers, or `None` if it does not fit.
pub fn parse_page_list(text: &str, count: u32) -> Option<Vec<u32>> {
    let mut pages = Vec::new();
    for part in text
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let (start, end) = part.split_once('-').unwrap_or((part, part));
        let start = if start.trim().is_empty() {
            1
        } else {
            start.trim().parse().ok()?
        };
        let end = if end.trim().is_empty() {
            count
        } else {
            end.trim().parse().ok()?
        };
        if start == 0 || end > count || start > end {
            return None;
        }
        pages.extend(start..=end);
    }
    (!pages.is_empty()).then_some(pages)
}

const SIGNATURE_KEY: &str = "ethos-signature";
const SIGNATURE_WIDTH: f64 = 170.0;

#[derive(Clone)]
pub struct NoteDraft {
    pub page: u32,
    pub point: Point,
    pub text: String,
}

#[derive(Clone)]
pub struct PasswordPrompt {
    pub file: StagedFile,
    pub incorrect: bool,
    pub value: String,
    pub sequence: u64,
}

fn stored_theme() -> Option<String> {
    window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|storage| storage.get_item("ethos-theme").ok().flatten())
}

fn stored_signature() -> Option<Vec<Vec<Point>>> {
    let storage = window().local_storage().ok().flatten()?;
    let text = storage.get_item(SIGNATURE_KEY).ok().flatten()?;
    let strokes: Vec<Vec<(f64, f64)>> = text
        .split(';')
        .map(|stroke| {
            stroke
                .split(' ')
                .filter_map(|pair| {
                    let (x, y) = pair.split_once(',')?;
                    Some((x.parse().ok()?, y.parse().ok()?))
                })
                .collect()
        })
        .collect();
    let strokes: Vec<Vec<Point>> = strokes
        .into_iter()
        .map(|stroke| stroke.into_iter().map(|(x, y)| Point { x, y }).collect())
        .collect();
    (!strokes.is_empty()).then_some(strokes)
}

fn random_id() -> String {
    window()
        .crypto()
        .map(|crypto| crypto.random_uuid())
        .unwrap_or_else(|_| js_sys::Date::now().to_string())
}

/// All interface state. It is `Copy`, so views and callbacks capture it freely.
#[derive(Clone, Copy)]
pub struct Store {
    pub doc: RwSignal<Option<Arc<Document>>>,
    pub history: RwSignal<History>,
    pub saved: RwSignal<Arc<Edits>>,
    pub recent: RwSignal<Vec<RecentFile>>,
    pub loading: RwSignal<bool>,
    pub saving: RwSignal<bool>,
    pub toast: RwSignal<Option<Toast>>,
    pub sidebar: RwSignal<bool>,
    pub tab: RwSignal<Tab>,
    pub page: RwSignal<usize>,
    pub page_input: RwSignal<String>,
    pub tool: RwSignal<Tool>,
    pub color: RwSignal<InkColor>,
    pub zoom: RwSignal<Zoom>,
    pub area_width: RwSignal<f64>,
    pub query: RwSignal<String>,
    pub hits: RwSignal<Vec<SearchHit>>,
    pub searching: RwSignal<bool>,
    pub note: RwSignal<Option<NoteDraft>>,
    pub note_error: RwSignal<String>,
    pub password: RwSignal<Option<PasswordPrompt>>,
    pub help: RwSignal<bool>,
    pub discard_id: RwSignal<Option<String>>,
    pub dark: RwSignal<bool>,
    pub dragging: RwSignal<bool>,
    pub fields: RwSignal<Vec<FormField>>,
    pub protected: RwSignal<bool>,
    /// Source pages selected in the sidebar.
    pub selection: RwSignal<BTreeSet<u32>>,
    pub selection_anchor: StoredValue<Option<u32>>,
    pub sheet: RwSignal<Option<Sheet>>,
    pub palette: RwSignal<Option<PaletteOrigin>>,
    pub tool_busy: RwSignal<bool>,
    pub progress: RwSignal<Option<ToolProgress>>,
    pub changes: RwSignal<Option<Comparison>>,
    pub scanned: RwSignal<bool>,
    pub scan_dismissed: StoredValue<bool>,
    pub signature: RwSignal<Option<Vec<Vec<Point>>>>,
    pub scroll: NodeRef<html::Div>,
    pub search_input: NodeRef<html::Input>,
    load_sequence: StoredValue<u64>,
    /// Bumped by every new search so older page batches stop.
    pub search_generation: StoredValue<u64>,
    scroll_frame: StoredValue<bool>,
    pub drag_depth: StoredValue<i32>,
}

impl Store {
    pub fn new() -> Self {
        let empty = Arc::new(Edits::default());
        Self {
            doc: RwSignal::new(None),
            history: RwSignal::new(History::fresh(Arc::clone(&empty))),
            saved: RwSignal::new(empty),
            recent: RwSignal::new(Vec::new()),
            loading: RwSignal::new(false),
            saving: RwSignal::new(false),
            toast: RwSignal::new(None),
            sidebar: RwSignal::new(true),
            tab: RwSignal::new(Tab::Pages),
            page: RwSignal::new(1),
            page_input: RwSignal::new("1".into()),
            tool: RwSignal::new(Tool::Select),
            color: RwSignal::new(InkColor::Yellow),
            zoom: RwSignal::new(Zoom::Fit),
            area_width: RwSignal::new(900.0),
            query: RwSignal::new(String::new()),
            hits: RwSignal::new(Vec::new()),
            searching: RwSignal::new(false),
            note: RwSignal::new(None),
            note_error: RwSignal::new(String::new()),
            password: RwSignal::new(None),
            help: RwSignal::new(false),
            discard_id: RwSignal::new(None),
            dark: RwSignal::new(stored_theme().is_some_and(|theme| theme == "dark")),
            dragging: RwSignal::new(false),
            fields: RwSignal::new(Vec::new()),
            protected: RwSignal::new(false),
            selection: RwSignal::new(BTreeSet::new()),
            selection_anchor: StoredValue::new(None),
            sheet: RwSignal::new(None),
            palette: RwSignal::new(None),
            tool_busy: RwSignal::new(false),
            progress: RwSignal::new(None),
            changes: RwSignal::new(None),
            scanned: RwSignal::new(false),
            scan_dismissed: StoredValue::new(false),
            signature: RwSignal::new(stored_signature()),
            scroll: NodeRef::new(),
            search_input: NodeRef::new(),
            load_sequence: StoredValue::new(0),
            search_generation: StoredValue::new(0),
            scroll_frame: StoredValue::new(false),
            drag_depth: StoredValue::new(0),
        }
    }

    /// Brings the appearance over from an earlier release on first launch.
    pub fn adopt_saved_theme(self) {
        if stored_theme().is_some() {
            return;
        }
        spawn_local(async move {
            if bridge::saved_theme().await.as_deref() == Some("dark") {
                self.dark.set(true);
            }
        });
    }

    pub fn edits(self) -> Arc<Edits> {
        self.history.with(|history| Arc::clone(&history.present))
    }

    pub fn dirty(self) -> bool {
        self.history.with(|history| {
            !self
                .saved
                .with(|saved| Arc::ptr_eq(&history.present, saved))
        })
    }

    pub fn notify(self, text: impl Into<String>, error: bool) {
        self.toast.set(Some(Toast {
            text: text.into(),
            error,
            undoable: false,
            result: None,
        }));
    }

    /// Runs a tool in the background; the reader stays usable while it works.
    pub fn run_tool(self, request: ToolRequest) {
        if self.tool_busy.get_untracked() {
            self.notify("Another tool is still working.", false);
            return;
        }
        let doc = self.doc.get_untracked();
        let name = doc
            .as_ref()
            .map_or_else(String::new, |doc| doc.name.clone());
        let edits = doc.is_some().then(|| {
            self.history
                .with_untracked(|history| Arc::clone(&history.present))
        });
        self.tool_busy.set(true);
        spawn_local(async move {
            let outcome = bridge::run_tool(&request, edits.as_deref(), &name).await;
            self.tool_busy.set(false);
            self.progress.set(None);
            match outcome {
                Ok(ToolOutcome::Done(result)) => self.toast.set(Some(Toast {
                    text: result.label.clone(),
                    error: false,
                    undoable: false,
                    result: Some(result),
                })),
                Ok(ToolOutcome::Compared { name, changes }) => {
                    self.changes.set(Some(Comparison { name, changes }));
                    self.sidebar.set(true);
                    self.tab.set(Tab::Changes);
                }
                Ok(ToolOutcome::Cancelled) => {}
                Err(error) => self.notify(error, true),
            }
        });
    }

    pub fn pick_files(self, kind: FilesKind) {
        spawn_local(async move {
            match bridge::pick_paths(kind.as_str()).await {
                Ok(paths) if !paths.is_empty() => self.sheet.set(Some(Sheet::Files(kind, paths))),
                Ok(_) => {}
                Err(error) => self.notify(error, true),
            }
        });
    }

    pub fn open_result(self, path: String) {
        self.toast.set(None);
        if path.to_lowercase().ends_with(".pdf") {
            let pick = async move { bridge::open_output(&path).await };
            self.open_with(pick);
        } else {
            bridge::reveal_output(&path, true);
        }
    }

    /// Asks for a signature the first time, and goes straight to placing it after that.
    pub fn start_signing(self) {
        if self.signature.read_untracked().is_some() {
            self.tool.set(Tool::Sign);
        } else {
            self.sheet.set(Some(Sheet::Signature));
        }
    }

    pub fn remember_signature(self, strokes: Vec<Vec<Point>>) {
        let text = strokes
            .iter()
            .map(|stroke| {
                let pairs: Vec<_> = stroke
                    .iter()
                    .map(|point| format!("{:.1},{:.1}", point.x, point.y))
                    .collect();
                pairs.join(" ")
            })
            .collect::<Vec<_>>()
            .join(";");
        if let Ok(Some(storage)) = window().local_storage() {
            // A profile that refuses storage only means drawing it again next time.
            let _ = storage.set_item(SIGNATURE_KEY, &text);
        }
        self.signature.set(Some(strokes));
    }

    /// The saved signature with its middle at `center`, sized for the page.
    pub fn signature_at(self, center: Point) -> Option<Vec<Vec<Point>>> {
        let strokes = self.signature.read_untracked();
        let strokes = strokes.as_ref()?;
        let points = || strokes.iter().flatten();
        let left = points().map(|point| point.x).fold(f64::MAX, f64::min);
        let right = points().map(|point| point.x).fold(f64::MIN, f64::max);
        let top = points().map(|point| point.y).fold(f64::MAX, f64::min);
        let bottom = points().map(|point| point.y).fold(f64::MIN, f64::max);
        let scale = SIGNATURE_WIDTH / (right - left).max(1.0);
        let placed = strokes
            .iter()
            .map(|stroke| {
                stroke
                    .iter()
                    .map(|point| Point {
                        x: center.x + (point.x - (left + right) / 2.0) * scale,
                        y: center.y - (point.y - (top + bottom) / 2.0) * scale,
                    })
                    .collect()
            })
            .collect();
        Some(placed)
    }

    /// Adds the saved signature with its middle at `center`, on page `source`.
    pub fn place_signature(self, source: u32, center: Point) {
        match self.signature_at(center) {
            Some(strokes) => {
                self.add_mark(self.new_mark(source, MarkKind::Signature { strokes }));
                self.tool.set(Tool::Select);
            }
            None => self.sheet.set(Some(Sheet::Signature)),
        }
    }

    /// Moves a placed signature, as one undo step.
    pub fn move_signature(self, id: &str, by: Point) {
        self.commit(|edits| {
            let mut next = edits.clone();
            for mark in next.marks.iter_mut().filter(|mark| mark.id == id) {
                if let MarkKind::Signature { strokes } = &mut mark.kind {
                    for point in strokes.iter_mut().flatten() {
                        point.x += by.x;
                        point.y += by.y;
                    }
                }
            }
            next
        });
    }

    pub fn add_redaction(self, source: u32, rect: Rect) {
        self.add_mark(self.new_mark(source, MarkKind::Redaction { rect }));
    }

    pub fn field_value(self, field: &FormField) -> String {
        self.history.with(|history| {
            history
                .present
                .fields
                .iter()
                .find(|value| value.name == field.name)
                .map_or_else(|| field.value.clone(), |value| value.value.clone())
        })
    }

    pub fn set_field(self, name: String, value: String) {
        self.commit(|edits| {
            let mut next = edits.clone();
            next.fields.retain(|field| field.name != name);
            next.fields.push(FieldValue { name, value });
            next
        });
    }

    pub fn focus_first_field(self) {
        let Some(first) = self.fields.read_untracked().first().map(|field| field.page) else {
            return;
        };
        self.source_navigate(first);
        request_animation_frame(move || {
            if let Ok(Some(field)) = document()
                .query_selector(".form-field input, .form-field textarea, .form-field select")
                && let Ok(field) = wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(field)
            {
                let _ = field.focus();
            }
        });
    }

    /// A click selects one page, Ctrl adds or removes, and Shift extends from the last click.
    pub fn select_page(self, source: u32, adding: bool, extending: bool) {
        let pages = self.edits().pages.clone();
        let position = |page: u32| pages.iter().position(|entry| entry.source == page);
        self.selection.update(|selection| {
            let anchor = self.selection_anchor.get_value();
            match (extending, anchor.and_then(position), position(source)) {
                (true, Some(from), Some(to)) => {
                    let (low, high) = (from.min(to), from.max(to));
                    selection.clear();
                    selection.extend(pages[low..=high].iter().map(|entry| entry.source));
                }
                _ if adding => {
                    if !selection.remove(&source) {
                        selection.insert(source);
                    }
                }
                _ => selection.clear(),
            }
        });
        if !extending {
            self.selection_anchor.set_value(Some(source));
        }
    }

    /// Moves a page, or the whole selection it belongs to, so it sits at `index`.
    pub fn move_pages(self, source: u32, index: usize) {
        let selection = self.selection.get_untracked();
        let moving = if selection.contains(&source) {
            selection
        } else {
            BTreeSet::from([source])
        };
        self.commit(|edits| {
            let mut next = edits.clone();
            let before = edits.pages[..index.min(edits.pages.len())]
                .iter()
                .filter(|entry| moving.contains(&entry.source))
                .count();
            let (moved, mut rest): (Vec<_>, Vec<_>) = edits
                .pages
                .iter()
                .copied()
                .partition(|entry| moving.contains(&entry.source));
            let at = index.saturating_sub(before).min(rest.len());
            rest.splice(at..at, moved);
            next.pages = rest;
            next
        });
    }

    pub fn selected_numbers(self) -> Vec<u32> {
        let selection = self.selection.get_untracked();
        let edits = self.edits();
        edits
            .pages
            .iter()
            .enumerate()
            .filter(|(_, entry)| selection.contains(&entry.source))
            .map(|(index, _)| index as u32 + 1)
            .collect()
    }

    pub fn rotate_selection(self) {
        let Some(doc) = self.doc.get_untracked() else {
            return;
        };
        let selection = self.selection.get_untracked();
        self.commit(|edits| {
            let mut next = edits.clone();
            for entry in next
                .pages
                .iter_mut()
                .filter(|entry| selection.contains(&entry.source))
            {
                if let Some(info) = doc.info(entry.source) {
                    entry.rotation = Some((display_rotation(*entry, info) + 90) % 360);
                }
            }
            next
        });
    }

    pub fn delete_selection(self) {
        let selection = self.selection.get_untracked();
        let remaining = self.history.with_untracked(|history| {
            history
                .present
                .pages
                .iter()
                .filter(|entry| !selection.contains(&entry.source))
                .count()
        });
        if remaining == 0 {
            self.notify("A document needs at least one page.", false);
            return;
        }
        self.commit(|edits| {
            let mut next = edits.clone();
            next.pages
                .retain(|entry| !selection.contains(&entry.source));
            next
        });
        self.selection.update(BTreeSet::clear);
        self.page.update(|page| *page = (*page).min(remaining));
        self.toast.set(Some(Toast {
            text: "Pages removed.".into(),
            error: false,
            undoable: true,
            result: None,
        }));
    }

    pub fn commit(self, update: impl FnOnce(&Edits) -> Edits) {
        self.history.update(|history| {
            let next = Arc::new(update(&history.present));
            history
                .past
                .push(std::mem::replace(&mut history.present, next));
            if history.past.len() > UNDO_LIMIT {
                history.past.remove(0);
            }
            history.future.clear();
        });
    }

    pub fn add_mark(self, mark: Mark) {
        self.commit(|edits| {
            let mut next = edits.clone();
            next.marks.push(mark);
            next
        });
    }

    pub fn new_mark(self, page: u32, kind: MarkKind) -> Mark {
        Mark {
            id: random_id(),
            page,
            color: self.color.get_untracked(),
            kind,
        }
    }

    pub fn remove_mark(self, id: &str) {
        self.commit(|edits| {
            let mut next = edits.clone();
            next.marks.retain(|mark| mark.id != id);
            next
        });
    }

    pub fn begin_note(self, page: u32, point: Point) {
        self.note_error.set(String::new());
        self.note.set(Some(NoteDraft {
            page,
            point,
            text: String::new(),
        }));
    }

    pub fn load_file(self, file: StagedFile) {
        self.load_sequence.update_value(|sequence| *sequence += 1);
        let sequence = self.load_sequence.get_value();
        self.loading.set(true);
        self.toast.set(None);
        spawn_local(async move { self.parse(file, None, sequence).await });
    }

    pub fn submit_password(self, prompt: PasswordPrompt) {
        let password = Some(prompt.value);
        self.password.set(None);
        spawn_local(async move {
            self.parse(prompt.file, password, prompt.sequence).await;
        });
    }

    async fn parse(self, file: StagedFile, password: Option<String>, sequence: u64) {
        let current = move || sequence == self.load_sequence.get_value();
        match bridge::load_document(file.token, password).await {
            // shortcut: a stale parse still replaces the engine's document, so if a newer
            // file then fails to load the old document's pages stop rendering. The window
            // is one open racing another, which only external opens can trigger.
            Ok(LoadOutcome::Ready {
                pages,
                marks,
                restored,
                fields,
                protected,
            }) if current() => {
                self.fields.set(fields);
                self.protected.set(protected);
                self.install(file, pages, marks, restored).await;
            }
            Ok(LoadOutcome::Password { incorrect }) if current() => {
                self.password.set(Some(PasswordPrompt {
                    file,
                    incorrect,
                    value: String::new(),
                    sequence,
                }));
                return;
            }
            Err(error) if current() => self.notify(error, true),
            _ => return,
        }
        if current() {
            self.loading.set(false);
        }
    }

    async fn install(
        self,
        file: StagedFile,
        pages: Vec<PageInfo>,
        marks: Vec<Mark>,
        restored: Option<Edits>,
    ) {
        let entries = (1..=pages.len() as u32)
            .map(|source| PageEntry {
                source,
                rotation: None,
            })
            .collect();
        let initial = Arc::new(Edits {
            marks,
            pages: entries,
            fields: Vec::new(),
        });
        self.selection.update(BTreeSet::clear);
        self.changes.set(None);
        self.scanned.set(false);
        self.scan_dismissed.set_value(false);
        let restored = restored.filter(|edits| *edits != *initial);
        let came_back = restored.is_some();
        let history = match restored {
            // Undo stays available, so a deleted page is never out of reach.
            Some(edits) => History {
                past: vec![initial],
                present: Arc::new(edits),
                future: Vec::new(),
            },
            None => History::fresh(initial),
        };
        self.saved.set(Arc::clone(&history.present));
        self.history.set(history);
        let title = format!("{} · ethos", file.name);
        self.doc.set(Some(Arc::new(Document {
            token: file.token,
            name: file.name,
            size: file.size,
            pages,
        })));
        self.page.set(1);
        self.tool.set(Tool::Select);
        self.query.set(String::new());
        self.hits.set(Vec::new());
        self.zoom.set(Zoom::Fit);
        self.tab.set(Tab::Pages);
        bridge::set_dirty(false);
        document().set_title(&title);
        bridge::set_title(&title);
        if let Ok(list) = bridge::recent_files().await {
            self.recent.set(list);
        }
        if let Some(scroll) = self.scroll.get_untracked() {
            scroll.set_scroll_top(0);
        }
        if came_back {
            self.toast.set(Some(Toast {
                text: "Your marks are back.".into(),
                error: false,
                undoable: true,
                result: None,
            }));
        }
    }

    /// Writes edits until the saved state is current. False means they are still at risk,
    /// because the document keeps no session or the write failed.
    pub async fn persist(self) -> bool {
        loop {
            let Some(doc) = self.doc.get_untracked() else {
                return false;
            };
            let snapshot = self
                .history
                .with_untracked(|history| Arc::clone(&history.present));
            match bridge::save_session(doc.token, &snapshot).await {
                Ok(true) => {}
                _ => return false,
            }
            let unchanged = self
                .history
                .with_untracked(|history| Arc::ptr_eq(&history.present, &snapshot));
            if unchanged {
                self.saved.set(snapshot);
                return true;
            }
        }
    }

    async fn can_leave(self) -> bool {
        let dirty = self.dirty();
        if !dirty {
            return true;
        }
        let answer = match bridge::sync_dirty(dirty).await {
            Ok(()) => bridge::confirm_leave().await,
            Err(error) => Err(error),
        };
        match answer {
            Ok(leave) => leave,
            Err(error) => {
                self.notify(error, true);
                false
            }
        }
    }

    pub fn cancel_load(self) {
        self.load_sequence.update_value(|sequence| *sequence += 1);
        bridge::cancel_load();
        self.password.set(None);
        self.loading.set(false);
    }

    fn busy(self) -> bool {
        self.loading.get_untracked() || self.saving.get_untracked()
    }

    fn open_with(self, pick: impl Future<Output = Result<Option<StagedFile>, String>> + 'static) {
        spawn_local(async move {
            let picked = match bridge::sync_dirty(self.dirty()).await {
                Ok(()) => pick.await,
                Err(error) => Err(error),
            };
            match picked {
                Ok(Some(file)) => self.load_file(file),
                Ok(None) => {}
                Err(error) => self.notify(error, true),
            }
        });
    }

    pub fn open(self) {
        if !self.busy() {
            self.open_with(bridge::open_file());
        }
    }

    pub fn open_recent(self, id: String) {
        if !self.busy() {
            self.open_with(bridge::open_recent(id));
        }
    }

    pub fn drop_files(self, files: &web_sys::FileList) {
        if files.length() == 0 || self.busy() {
            return;
        }
        if let Err(error) = bridge::post_dropped_files(files) {
            self.notify(error, true);
        }
    }

    /// One PDF opens. Several, or any pictures, go to the sheet that can use them together.
    pub fn receive_drop(self, paths: Vec<String>) {
        let is_pdf = |path: &String| path.to_lowercase().ends_with(".pdf");
        match &paths[..] {
            [only] if is_pdf(only) => self.open_dropped(only.clone()),
            _ => {
                let kind = if paths.iter().all(is_pdf) {
                    FilesKind::Pdfs
                } else {
                    FilesKind::Pictures
                };
                let paths = paths.into_iter().map(PathBuf::from).collect();
                self.sheet.set(Some(Sheet::Files(kind, paths)));
            }
        }
    }

    pub fn open_dropped(self, path: String) {
        if !self.busy() {
            self.open_with(bridge::open_dropped(path));
        }
    }

    pub fn sample(self) {
        if self.loading.get_untracked() {
            return;
        }
        spawn_local(async move {
            if !self.can_leave().await {
                return;
            }
            match bridge::open_sample().await {
                Ok(file) => self.load_file(file),
                Err(error) => self.notify(error, true),
            }
        });
    }

    pub fn close_document(self) {
        if self.busy() {
            return;
        }
        spawn_local(async move {
            if !self.can_leave().await {
                return;
            }
            bridge::close_document().await;
            let empty = Arc::new(Edits::default());
            self.saved.set(Arc::clone(&empty));
            self.history.set(History::fresh(empty));
            self.doc.set(None);
            self.fields.set(Vec::new());
            self.protected.set(false);
            self.changes.set(None);
            self.scanned.set(false);
            self.query.set(String::new());
            self.toast.set(None);
            bridge::set_dirty(false);
            document().set_title("ethos");
            bridge::set_title("ethos");
        });
    }

    pub fn save(self) {
        let Some(doc) = self.doc.get_untracked() else {
            return;
        };
        if self.busy() {
            return;
        }
        let snapshot = self
            .history
            .with_untracked(|history| Arc::clone(&history.present));
        self.saving.set(true);
        spawn_local(async move {
            match bridge::save_copy(&snapshot, &doc.name).await {
                Ok(SaveOutcome::Saved) => {
                    let redacted = snapshot
                        .marks
                        .iter()
                        .filter(|mark| matches!(mark.kind, MarkKind::Redaction { .. }))
                        .count();
                    self.saved.set(snapshot);
                    self.notify(
                        match redacted {
                            0 => "Copy saved. Your original is untouched.".to_owned(),
                            1 => "Copy saved, with 1 area removed for good.".to_owned(),
                            count => format!("Copy saved, with {count} areas removed for good."),
                        },
                        false,
                    );
                }
                Ok(SaveOutcome::Cancelled) => {}
                Err(error) => self.notify(error, true),
            }
            self.saving.set(false);
        });
    }

    pub fn submit_note(self) {
        let (Some(note), Some(doc)) = (self.note.get_untracked(), self.doc.get_untracked()) else {
            return;
        };
        if note.text.trim().is_empty() {
            return;
        }
        let Some(info) = doc.info(note.page) else {
            return;
        };
        let width = (info.view[2] - note.point.x - 16.0).max(30.0);
        match wrap_text_note(&note.text, width) {
            Ok(text) => {
                let lines = text.split('\n').count() as f64;
                let point = Point {
                    x: note.point.x,
                    y: note
                        .point
                        .y
                        .max(info.view[1] + lines * NOTE_LINE_HEIGHT + 8.0),
                };
                self.add_mark(self.new_mark(note.page, MarkKind::Text { point, text }));
                self.note.set(None);
                self.tool.set(Tool::Select);
            }
            Err(error) => self.note_error.set(error),
        }
    }

    pub fn rotate_page(self) {
        let Some(doc) = self.doc.get_untracked() else {
            return;
        };
        let edits = self
            .history
            .with_untracked(|history| Arc::clone(&history.present));
        let Some(&entry) = edits.pages.get(self.page.get_untracked() - 1) else {
            return;
        };
        let Some(info) = doc.info(entry.source) else {
            return;
        };
        let rotation = (display_rotation(entry, info) + 90) % 360;
        self.commit(|edits| {
            let mut next = edits.clone();
            for item in next
                .pages
                .iter_mut()
                .filter(|item| item.source == entry.source)
            {
                item.rotation = Some(rotation);
            }
            next
        });
    }

    pub fn delete_page(self) {
        let page = self.page.get_untracked();
        let remaining = self
            .history
            .with_untracked(|history| history.present.pages.len())
            - 1;
        self.commit(|edits| {
            let mut next = edits.clone();
            next.pages.remove(page - 1);
            next
        });
        self.page.set(page.min(remaining));
        self.toast.set(Some(Toast {
            text: "Page removed.".into(),
            error: false,
            undoable: true,
            result: None,
        }));
    }

    pub fn undo(self) {
        // An undo prompt is stale once the user has undone.
        self.toast.update(|toast| {
            if toast.as_ref().is_some_and(|toast| toast.undoable) {
                *toast = None;
            }
        });
        self.history.update(|history| {
            if let Some(previous) = history.past.pop() {
                history
                    .future
                    .insert(0, std::mem::replace(&mut history.present, previous));
            }
        });
    }

    pub fn redo(self) {
        self.history.update(|history| {
            if !history.future.is_empty() {
                let next = history.future.remove(0);
                history
                    .past
                    .push(std::mem::replace(&mut history.present, next));
            }
        });
    }

    pub fn navigate(self, number: usize) {
        let count = self
            .history
            .with_untracked(|history| history.present.pages.len());
        let valid = number.min(count).max(1);
        let target = self.scroll.get_untracked().and_then(|scroll| {
            scroll
                .query_selector(&format!("[data-page=\"{valid}\"]"))
                .ok()
                .flatten()
        });
        if let Some(target) = target {
            let options = ScrollIntoViewOptions::new();
            options.set_block(ScrollLogicalPosition::Start);
            options.set_behavior(ScrollBehavior::Instant);
            target.scroll_into_view_with_scroll_into_view_options(&options);
        }
        self.page.set(valid);
    }

    pub fn source_navigate(self, source: u32) {
        let index = self.history.with_untracked(|history| {
            history
                .present
                .pages
                .iter()
                .position(|entry| entry.source == source)
        });
        if let Some(index) = index {
            self.navigate(index + 1);
        }
    }

    /// The display number of a source page, or 0 if it was deleted.
    pub fn display_page(self, source: u32) -> usize {
        self.history.with(|history| {
            history
                .present
                .pages
                .iter()
                .position(|entry| entry.source == source)
                .map_or(0, |index| index + 1)
        })
    }

    /// Finds the page crossing the reading line, at most once per animation frame.
    pub fn track_page(self) {
        if self.scroll_frame.get_value() {
            return;
        }
        self.scroll_frame.set_value(true);
        request_animation_frame(move || {
            self.scroll_frame.set_value(false);
            let Some(scroll) = self.scroll.get_untracked() else {
                return;
            };
            let Ok(nodes) = scroll.query_selector_all("[data-page]") else {
                return;
            };
            let bottom = |index: u32| {
                nodes
                    .get(index)
                    .and_then(|node| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(node).ok())
                    .map_or(0.0, |element| element.get_bounding_client_rect().bottom())
            };
            let top = scroll.get_bounding_client_rect().top() + 110.0;
            let (mut left, mut right) = (0, nodes.length().saturating_sub(1));
            while left < right {
                let middle = (left + right) / 2;
                if bottom(middle) <= top {
                    left = middle + 1;
                } else {
                    right = middle;
                }
            }
            let number = nodes
                .get(left)
                .and_then(|node| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(node).ok())
                .and_then(|element| element.get_attribute("data-page"))
                .and_then(|page| page.parse().ok());
            if let Some(number) = number {
                self.page.set(number);
            }
        });
    }

    pub fn page_width(self) -> f64 {
        match self.zoom.get() {
            Zoom::Fit => (self.area_width.get() - 96.0).clamp(280.0, 860.0),
            Zoom::Scale(scale) => 612.0 * scale,
        }
    }

    pub fn step_zoom(self, delta: f64) {
        let current = match self.zoom.get_untracked() {
            Zoom::Scale(scale) => scale,
            Zoom::Fit => (self.area_width.get_untracked() - 96.0).clamp(280.0, 860.0) / 612.0,
        };
        self.zoom
            .set(Zoom::Scale((current + delta).clamp(0.5, 2.5)));
    }

    pub fn focus_search(self) {
        request_animation_frame(move || {
            if let Some(input) = self.search_input.get_untracked() {
                let _ = input.focus();
            }
        });
    }

    pub fn show_search(self) {
        self.sidebar.set(true);
        self.tab.set(Tab::Search);
        self.focus_search();
    }
}
