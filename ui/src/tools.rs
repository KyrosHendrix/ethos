//! The tools surface: one searchable list, the short sheets a tool may need, and progress.

use std::path::{Path, PathBuf};

use ethos_core::{ChangeKind, Point, ToolRequest, notes_markdown};
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::bridge;
use crate::controls::{Icon, Modal};
use crate::icons;
use crate::store::{FilesKind, Sheet, Store, Tab, Tool, parse_page_list};

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Notes,
    Markdown,
    Text,
    Word,
    Pictures,
    Searchable,
    Compare,
    Reorder,
    Combine,
    Split,
    Extract,
    WordToPdf,
    PicturesToPdf,
    Fill,
    Sign,
    Redact,
    Smaller,
    Protect,
    Unlock,
}

#[derive(Clone, Copy, PartialEq)]
enum Needs {
    Anything,
    Document,
    Fields,
    Protected,
}

#[derive(Clone, Copy)]
struct Entry {
    action: Action,
    group: &'static str,
    label: &'static str,
    keywords: &'static str,
    icon: &'static str,
    needs: Needs,
}

const ENTRIES: [Entry; 19] = [
    Entry {
        action: Action::Notes,
        group: "Your thinking",
        label: "Export highlights and notes",
        keywords: "markdown obsidian annotations summary",
        icon: icons::NOTEBOOK_PEN,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Markdown,
        group: "Your thinking",
        label: "Save as Markdown",
        keywords: "md convert export",
        icon: icons::FILE_CODE,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Text,
        group: "Your thinking",
        label: "Save as plain text",
        keywords: "txt convert export",
        icon: icons::FILE_TEXT,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Searchable,
        group: "Understand",
        label: "Make a scan searchable",
        keywords: "ocr recognize text scanned",
        icon: icons::SCAN_TEXT,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Compare,
        group: "Understand",
        label: "Compare with another version",
        keywords: "diff changes revision",
        icon: icons::GIT_COMPARE,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Combine,
        group: "Pages",
        label: "Combine PDFs",
        keywords: "merge join add",
        icon: icons::COMBINE,
        needs: Needs::Anything,
    },
    Entry {
        action: Action::Split,
        group: "Pages",
        label: "Split into parts",
        keywords: "separate divide",
        icon: icons::SCISSORS,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Extract,
        group: "Pages",
        label: "Save pages as a new PDF",
        keywords: "extract select copy",
        icon: icons::FILE_OUTPUT,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Reorder,
        group: "Pages",
        label: "Reorder pages",
        keywords: "move arrange organize drag thumbnails",
        icon: icons::GRIP_VERTICAL,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Word,
        group: "Convert",
        label: "Save as Word",
        keywords: "docx convert export microsoft",
        icon: icons::FILE_TYPE,
        needs: Needs::Document,
    },
    Entry {
        action: Action::WordToPdf,
        group: "Convert",
        label: "Word to PDF",
        keywords: "docx convert import microsoft",
        icon: icons::FILE_TYPE,
        needs: Needs::Anything,
    },
    Entry {
        action: Action::Pictures,
        group: "Convert",
        label: "Save pages as pictures",
        keywords: "images png jpeg jpg convert export",
        icon: icons::IMAGES,
        needs: Needs::Document,
    },
    Entry {
        action: Action::PicturesToPdf,
        group: "Convert",
        label: "Pictures to PDF",
        keywords: "images png jpeg jpg photos convert scan",
        icon: icons::IMAGE_PLUS,
        needs: Needs::Anything,
    },
    Entry {
        action: Action::Fill,
        group: "Finish",
        label: "Fill in this form",
        keywords: "fields inputs",
        icon: icons::TEXT_CURSOR_INPUT,
        needs: Needs::Fields,
    },
    Entry {
        action: Action::Sign,
        group: "Finish",
        label: "Sign",
        keywords: "signature autograph",
        icon: icons::SIGNATURE,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Redact,
        group: "Finish",
        label: "Redact",
        keywords: "remove black out hide private censor",
        icon: icons::RECTANGLE_HORIZONTAL,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Smaller,
        group: "Finish",
        label: "Make a smaller copy",
        keywords: "compress reduce size shrink email",
        icon: icons::SHRINK,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Protect,
        group: "Finish",
        label: "Protect with a password",
        keywords: "encrypt lock secure",
        icon: icons::LOCK,
        needs: Needs::Document,
    },
    Entry {
        action: Action::Unlock,
        group: "Finish",
        label: "Remove the password",
        keywords: "decrypt unlock",
        icon: icons::LOCK_OPEN,
        needs: Needs::Protected,
    },
];

fn perform(store: Store, action: Action) {
    store.palette.set(None);
    match action {
        Action::Notes => store.sheet.set(Some(Sheet::Notes)),
        Action::Markdown => store.run_tool(ToolRequest::Markdown),
        Action::Text => store.run_tool(ToolRequest::Text),
        Action::Word => store.run_tool(ToolRequest::Word),
        Action::Pictures => store.sheet.set(Some(Sheet::Pictures)),
        Action::Searchable => store.run_tool(ToolRequest::Searchable),
        Action::Compare => store.run_tool(ToolRequest::Compare { path: None }),
        Action::Reorder => {
            store.sidebar.set(true);
            store.tab.set(Tab::Pages);
            store.notify("Drag a page in the sidebar to move it.", false);
        }
        Action::Combine => store.pick_files(FilesKind::Pdfs),
        Action::Split => store.sheet.set(Some(Sheet::Split)),
        Action::Extract => store.sheet.set(Some(Sheet::Extract)),
        Action::WordToPdf => store.run_tool(ToolRequest::WordToPdf { path: None }),
        Action::PicturesToPdf => store.pick_files(FilesKind::Pictures),
        Action::Fill => store.focus_first_field(),
        Action::Sign => store.start_signing(),
        Action::Redact => store.tool.set(Tool::Redact),
        Action::Smaller => store.run_tool(ToolRequest::Smaller),
        Action::Protect => store.sheet.set(Some(Sheet::Protect)),
        Action::Unlock => store.run_tool(ToolRequest::RemovePassword),
    }
}

fn available(store: Store) -> Vec<Entry> {
    let has_document = store.doc.read_untracked().is_some();
    let protected = store.protected.get_untracked();
    let has_fields = !store.fields.read_untracked().is_empty();
    ENTRIES
        .into_iter()
        .filter(|entry| match entry.needs {
            Needs::Anything => true,
            Needs::Document => has_document && !protected,
            Needs::Fields => has_fields,
            Needs::Protected => protected,
        })
        .collect()
}

/// Indexes of the entries matching `query`, best first: names beat keywords, and names that
/// start with the query beat names that merely contain it.
fn ranked(entries: &[Entry], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut found: Vec<(u8, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let label = entry.label.to_lowercase();
            let rank = if label.starts_with(&needle) {
                0
            } else if label.contains(&needle) {
                1
            } else if entry.keywords.contains(&needle) {
                2
            } else {
                return None;
            };
            Some((rank, index))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, index)| index).collect()
}

/// The one list of tools. From the toolbar it grows out of its button; `Ctrl K` shows it at
/// once, because anything summoned from the keyboard should not make a person wait.
#[component]
pub fn Palette(anchored: bool) -> impl IntoView {
    let store = expect_context::<Store>();
    let query = RwSignal::new(String::new());
    let highlighted = RwSignal::new(0_usize);
    let input = NodeRef::<html::Input>::new();
    let entries = available(store);
    let matches = Memo::new(move |_| ranked(&available(store), &query.read()));
    Effect::new(move |_| {
        if let Some(node) = input.get() {
            let _ = node.focus();
        }
    });
    Effect::new(move |_| {
        query.track();
        highlighted.set(0);
    });
    Effect::new(move |_| {
        let index = highlighted.get();
        if let Ok(Some(row)) = document().query_selector(&format!("[data-tool-index=\"{index}\"]"))
        {
            let options = web_sys::ScrollIntoViewOptions::new();
            options.set_block(web_sys::ScrollLogicalPosition::Nearest);
            row.scroll_into_view_with_scroll_into_view_options(&options);
        }
    });
    let list = entries.clone();
    let on_key = move |event: ev::KeyboardEvent| {
        let found = matches.get_untracked();
        match event.key().as_str() {
            "ArrowDown" if !found.is_empty() => {
                highlighted.update(|index| *index = (*index + 1) % found.len());
            }
            "ArrowUp" if !found.is_empty() => {
                highlighted.update(|index| *index = (*index + found.len() - 1) % found.len());
            }
            "Enter" => {
                if let Some(&index) = found.get(highlighted.get_untracked()) {
                    perform(store, list[index].action);
                }
            }
            "Escape" => store.palette.set(None),
            _ => return,
        }
        event.prevent_default();
        event.stop_propagation();
    };
    let rows = move || {
        let found = matches.get();
        let searching = query.read().trim().to_owned();
        let mut last_group = "";
        let mut views = Vec::new();
        for (position, index) in found.iter().enumerate() {
            let entry = entries[*index];
            // Results are ranked while searching, so groups would only repeat.
            if searching.is_empty() && entry.group != last_group {
                last_group = entry.group;
                views.push(view! { <div class="tool-group-label">{entry.group}</div> }.into_any());
            }
            views.push(
                view! {
                    <button
                        type="button"
                        class="tool-row"
                        role="option"
                        tabindex="-1"
                        data-tool-index=position
                        data-highlighted=move || (highlighted.get() == position).then_some("")
                        on:pointermove=move |_| highlighted.set(position)
                        on:click=move |_| perform(store, entry.action)
                    >
                        <Icon data=entry.icon size=16 />
                        <span>{entry.label}</span>
                    </button>
                }
                .into_any(),
            );
        }
        views
    };
    view! {
        <div class="palette-backdrop" on:pointerdown=move |_| store.palette.set(None)></div>
        <div
            class="palette"
            data-anchored=anchored.to_string()
            role="dialog"
            aria-label="Tools"
            on:keydown=on_key
        >
            <div class="palette-field">
                <Icon data=icons::SEARCH size=15 />
                <input
                    node_ref=input
                    aria-label="Find a tool"
                    placeholder="Find a tool"
                    prop:value=move || query.get()
                    on:input=move |event| query.set(event_target_value(&event))
                />
            </div>
            <div class="palette-list" role="listbox" aria-label="Tools">
                {rows}
                <Show when=move || matches.read().is_empty()>
                    <p class="palette-empty">"No tool matches that."</p>
                </Show>
            </div>
        </div>
    }
}

/// Live status while a tool works. The document stays readable underneath.
#[component]
pub fn ToolStatus() -> impl IntoView {
    let store = expect_context::<Store>();
    let label = move || {
        store.progress.read().as_ref().map_or_else(
            || "Working".to_owned(),
            |progress| {
                if progress.total > 1 {
                    format!(
                        "{} {} of {}",
                        progress.label,
                        progress.done.min(progress.total),
                        progress.total
                    )
                } else {
                    progress.label.clone()
                }
            },
        )
    };
    let bar = move || {
        let fraction = store
            .progress
            .read()
            .as_ref()
            .filter(|progress| progress.total > 0)
            .map_or(0.0, |progress| {
                f64::from(progress.done) / f64::from(progress.total)
            });
        format!("transform:scaleX({fraction})")
    };
    view! {
        <div class="tool-status" role="status">
            <span class="tool-status-label">{label}</span>
            <button type="button" class="text-button" on:click=|_| bridge::cancel_tool()>
                "Cancel"
            </button>
            <span class="tool-status-track">
                <span class="tool-status-bar" style=bar></span>
            </span>
        </div>
    }
}

#[component]
fn NotesSheet() -> impl IntoView {
    let store = expect_context::<Store>();
    let title = store
        .doc
        .read_untracked()
        .as_ref()
        .map_or_else(String::new, |doc| doc.title());
    let markdown = notes_markdown(&title, &store.edits());
    let empty = markdown.lines().count() <= 1;
    let copied = RwSignal::new(false);
    let text = StoredValue::new(markdown.clone());
    let copy = move |_| {
        spawn_local(async move {
            let clipboard = window().navigator().clipboard();
            let written = text.with_value(|text| clipboard.write_text(text));
            if wasm_bindgen_futures::JsFuture::from(written).await.is_ok() {
                copied.set(true);
            }
        });
    };
    let save = move |_| {
        let suggested = format!("{title} notes.md");
        store.sheet.set(None);
        store.run_tool(ToolRequest::SaveText {
            suggested,
            text: text.get_value(),
            kind: "md".into(),
        });
    };
    view! {
        <Modal title="Your highlights and notes" on_close=move || store.sheet.set(None)>
            <p class="modal-description">
                {if empty {
                    "Highlight a passage or add a note, and it will appear here, ready for your notes app."
                } else {
                    "Clean Markdown, in page order, ready for your notes app."
                }}
            </p>
            <pre class="notes-preview" tabindex="0">{markdown}</pre>
            <div class="modal-actions">
                <button type="button" class="subtle-button" disabled=empty on:click=save>
                    "Save as a file"
                </button>
                <button
                    type="button"
                    class="primary-button"
                    autofocus=!empty
                    disabled=empty
                    on:click=copy
                >
                    {move || if copied.get() { "Copied" } else { "Copy" }}
                </button>
            </div>
        </Modal>
    }
}

/// A two-way choice shown as a segmented control.
#[component]
fn Segments(
    label: &'static str,
    first: &'static str,
    second: &'static str,
    #[prop(into)] selected: Signal<bool>,
    #[prop(into)] on_change: Callback<bool>,
) -> impl IntoView {
    view! {
        <div class="segments" role="radiogroup" aria-label=label>
            <button
                type="button"
                role="radio"
                aria-checked=move || (!selected.get()).to_string()
                class=move || if selected.get() { "" } else { "selected" }
                on:click=move |_| on_change.run(false)
            >
                {first}
            </button>
            <button
                type="button"
                role="radio"
                aria-checked=move || selected.get().to_string()
                class=move || if selected.get() { "selected" } else { "" }
                on:click=move |_| on_change.run(true)
            >
                {second}
            </button>
        </div>
    }
}

#[component]
fn SplitSheet() -> impl IntoView {
    let store = expect_context::<Store>();
    let count = store.edits().pages.len() as u32;
    let by_ranges = RwSignal::new(false);
    let every = RwSignal::new(1_u32);
    let ranges = RwSignal::new(String::new());
    let parts = move || {
        if by_ranges.get() {
            parse_page_list(&ranges.get(), count).map(|_| {
                ranges
                    .read()
                    .split(',')
                    .filter(|part| !part.trim().is_empty())
                    .count() as u32
            })
        } else {
            Some(count.div_ceil(every.get().max(1)))
        }
    };
    let summary = move || match parts() {
        Some(1) => "Makes 1 PDF".to_owned(),
        Some(parts) => format!("Makes {parts} PDFs"),
        None => format!("Use pages 1 to {count}, like 1-3, 5"),
    };
    let submit = move |event: ev::SubmitEvent| {
        event.prevent_default();
        if parts().is_none() {
            return;
        }
        store.sheet.set(None);
        store.run_tool(ToolRequest::Split {
            ranges: ranges.get_untracked(),
            every: (!by_ranges.get_untracked()).then(|| every.get_untracked()),
        });
    };
    view! {
        <Modal title="Split into parts" on_close=move || store.sheet.set(None)>
            <form on:submit=submit class="sheet-form">
                <Segments
                    label="Split by"
                    first="Every few pages"
                    second="Chosen pages"
                    selected=by_ranges
                    on_change=move |value| by_ranges.set(value)
                />
                <Show
                    when=move || by_ranges.get()
                    fallback=move || {
                        view! {
                            <label class="sheet-field">
                                <span>"Pages in each part"</span>
                                <input
                                    class="ui-input"
                                    type="number"
                                    min="1"
                                    max=count
                                    autofocus
                                    prop:value=move || every.get().to_string()
                                    on:input=move |event| {
                                        every.set(event_target_value(&event).parse().unwrap_or(1).clamp(1, count.max(1)));
                                    }
                                />
                            </label>
                        }
                    }
                >
                    <label class="sheet-field">
                        <span>"One part for each range"</span>
                        <input
                            class="ui-input"
                            autofocus
                            placeholder="1-3, 4-8, 9-"
                            prop:value=move || ranges.get()
                            on:input=move |event| ranges.set(event_target_value(&event))
                        />
                    </label>
                </Show>
                <div class="modal-actions">
                    <span>{summary}</span>
                    <button class="primary-button" disabled=move || parts().is_none()>
                        "Choose a folder"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
fn ExtractSheet() -> impl IntoView {
    let store = expect_context::<Store>();
    let count = store.edits().pages.len() as u32;
    let current = store.page.get_untracked().to_string();
    let chosen = RwSignal::new(current);
    let pages = move || parse_page_list(&chosen.get(), count);
    let summary = move || match pages() {
        Some(pages) if pages.len() == 1 => "1 page".to_owned(),
        Some(pages) => format!("{} pages", pages.len()),
        None => format!("Use pages 1 to {count}, like 1-3, 5"),
    };
    let submit = move |event: ev::SubmitEvent| {
        event.prevent_default();
        if let Some(pages) = pages() {
            store.sheet.set(None);
            store.run_tool(ToolRequest::Extract { pages });
        }
    };
    view! {
        <Modal title="Save pages as a new PDF" on_close=move || store.sheet.set(None)>
            <form on:submit=submit class="sheet-form">
                <label class="sheet-field">
                    <span>"Which pages"</span>
                    <input
                        class="ui-input"
                        autofocus
                        placeholder="1-3, 5"
                        prop:value=move || chosen.get()
                        on:input=move |event| chosen.set(event_target_value(&event))
                    />
                </label>
                <p class="sheet-tip">"You can also select thumbnails in the sidebar with Ctrl or Shift."</p>
                <div class="modal-actions">
                    <span>{summary}</span>
                    <button class="primary-button" disabled=move || pages().is_none()>
                        "Save"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
fn PicturesSheet() -> impl IntoView {
    let store = expect_context::<Store>();
    let jpeg = RwSignal::new(false);
    let high = RwSignal::new(false);
    let submit = move |event: ev::SubmitEvent| {
        event.prevent_default();
        store.sheet.set(None);
        store.run_tool(ToolRequest::Images {
            jpeg: jpeg.get_untracked(),
            dpi: if high.get_untracked() { 300 } else { 150 },
        });
    };
    view! {
        <Modal title="Save pages as pictures" on_close=move || store.sheet.set(None)>
            <form on:submit=submit class="sheet-form">
                <div class="sheet-field">
                    <span>"Format"</span>
                    <Segments
                        label="Format"
                        first="PNG"
                        second="JPEG"
                        selected=jpeg
                        on_change=move |value| jpeg.set(value)
                    />
                </div>
                <div class="sheet-field">
                    <span>"Detail"</span>
                    <Segments
                        label="Detail"
                        first="Standard"
                        second="High"
                        selected=high
                        on_change=move |value| high.set(value)
                    />
                </div>
                <div class="modal-actions">
                    <button class="primary-button">"Choose a folder"</button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
fn ProtectSheet() -> impl IntoView {
    let store = expect_context::<Store>();
    let password = RwSignal::new(String::new());
    let confirmation = RwSignal::new(String::new());
    let mismatch = move || !confirmation.read().is_empty() && password.get() != confirmation.get();
    let ready = move || !password.read().is_empty() && password.get() == confirmation.get();
    let submit = move |event: ev::SubmitEvent| {
        event.prevent_default();
        if ready() {
            store.sheet.set(None);
            store.run_tool(ToolRequest::Protect {
                password: password.get_untracked(),
            });
        }
    };
    view! {
        <Modal title="Protect with a password" on_close=move || store.sheet.set(None)>
            <form on:submit=submit class="sheet-form">
                <p class="modal-description">
                    "This saves a protected copy. There is no way to recover a forgotten password."
                </p>
                <label class="sheet-field">
                    <span>"Password"</span>
                    <input
                        class="ui-input"
                        type="password"
                        autofocus
                        prop:value=move || password.get()
                        on:input=move |event| password.set(event_target_value(&event))
                    />
                </label>
                <label class="sheet-field">
                    <span>"Repeat it"</span>
                    <input
                        class="ui-input"
                        type="password"
                        prop:value=move || confirmation.get()
                        on:input=move |event| confirmation.set(event_target_value(&event))
                    />
                </label>
                <div class="modal-actions">
                    <span class="inline-error">
                        {move || mismatch().then_some("These don't match yet.")}
                    </span>
                    <button class="primary-button" disabled=move || !ready()>
                        "Save protected copy"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

fn file_name(path: &Path) -> String {
    path.to_string_lossy()
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// An ordered list of chosen files, for combining PDFs or making one from pictures.
#[component]
fn FilesSheet(kind: FilesKind, paths: Vec<PathBuf>) -> impl IntoView {
    let store = expect_context::<Store>();
    let paths = RwSignal::new(paths);
    let fit_a4 = RwSignal::new(true);
    let has_document = store.doc.read_untracked().is_some();
    let include_current =
        kind == FilesKind::Pdfs && has_document && !store.protected.get_untracked();
    let title = match kind {
        FilesKind::Pdfs => "Combine PDFs",
        FilesKind::Pictures => "Pictures to PDF",
    };
    let shift = move |index: usize, direction: isize| {
        paths.update(|paths| {
            let target = index as isize + direction;
            if target >= 0 && (target as usize) < paths.len() {
                paths.swap(index, target as usize);
            }
        });
    };
    let add = move |_| {
        spawn_local(async move {
            if let Ok(more) = bridge::pick_paths(kind.as_str()).await {
                paths.update(|paths| paths.extend(more));
            }
        });
    };
    let submit = move |event: ev::SubmitEvent| {
        event.prevent_default();
        let chosen = paths.get_untracked();
        store.sheet.set(None);
        store.run_tool(match kind {
            FilesKind::Pdfs => ToolRequest::Merge {
                paths: chosen,
                include_current,
            },
            FilesKind::Pictures => ToolRequest::ImagesToPdf {
                paths: chosen,
                fit_a4: fit_a4.get_untracked(),
            },
        });
    };
    let current_name = store
        .doc
        .read_untracked()
        .as_ref()
        .map(|doc| doc.name.clone());
    view! {
        <Modal title=title on_close=move || store.sheet.set(None)>
            <form on:submit=submit class="sheet-form">
                <ol class="file-list">
                    {include_current
                        .then(|| {
                            view! {
                                <li class="file-row current">
                                    <span>{current_name}</span>
                                    <small>"Open now"</small>
                                </li>
                            }
                        })}
                    {move || {
                        let total = paths.read().len();
                        paths
                            .get()
                            .iter()
                            .enumerate()
                            .map(|(index, path)| {
                                view! {
                                    <li class="file-row">
                                        <span title=path.to_string_lossy().into_owned()>{file_name(path)}</span>
                                        <button
                                            type="button"
                                            class="icon-button"
                                            aria-label="Move up"
                                            disabled=index == 0
                                            on:click=move |_| shift(index, -1)
                                        >
                                            <Icon data=icons::CHEVRON_UP size=14 />
                                        </button>
                                        <button
                                            type="button"
                                            class="icon-button"
                                            aria-label="Move down"
                                            disabled=index + 1 == total
                                            on:click=move |_| shift(index, 1)
                                        >
                                            <Icon data=icons::CHEVRON_DOWN size=14 />
                                        </button>
                                        <button
                                            type="button"
                                            class="icon-button"
                                            aria-label="Remove file"
                                            on:click=move |_| paths.update(|paths| drop(paths.remove(index)))
                                        >
                                            <Icon data=icons::X size=13 />
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()
                    }}
                </ol>
                {(kind == FilesKind::Pictures)
                    .then(|| {
                        view! {
                            <Segments
                                label="Page size"
                                first="Picture size"
                                second="A4 pages"
                                selected=fit_a4
                                on_change=move |value| fit_a4.set(value)
                            />
                        }
                    })}
                <div class="modal-actions">
                    <button type="button" class="subtle-button" on:click=add>
                        "Add more"
                    </button>
                    <button class="primary-button" disabled=move || paths.read().is_empty()>
                        "Save"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

/// Smooths a stroke into an SVG path through the midpoints of its samples.
fn stroke_path(points: &[Point]) -> String {
    let Some(first) = points.first() else {
        return String::new();
    };
    let mut path = format!("M{} {}", first.x, first.y);
    for pair in points.windows(2) {
        let middle = ((pair[0].x + pair[1].x) / 2.0, (pair[0].y + pair[1].y) / 2.0);
        path.push_str(&format!(
            " Q{} {} {} {}",
            pair[0].x, pair[0].y, middle.0, middle.1
        ));
    }
    if points.len() == 1 {
        path.push_str(&format!(" L{} {}", first.x + 0.1, first.y));
    }
    path
}

#[component]
fn SignatureSheet() -> impl IntoView {
    const WIDTH: f64 = 440.0;
    const HEIGHT: f64 = 170.0;
    let store = expect_context::<Store>();
    let strokes = RwSignal::new(Vec::<Vec<Point>>::new());
    let drawing = StoredValue::new(false);
    let pad = NodeRef::<leptos::svg::Svg>::new();
    let position = move |event: &ev::PointerEvent| {
        pad.get_untracked().map_or(Point::default(), |node| {
            let bounds: web_sys::Element = node.unchecked_into();
            let rect = bounds.get_bounding_client_rect();
            Point {
                x: (f64::from(event.client_x()) - rect.left()) * WIDTH / rect.width(),
                y: (f64::from(event.client_y()) - rect.top()) * HEIGHT / rect.height(),
            }
        })
    };
    let begin = move |event: ev::PointerEvent| {
        if let Some(target) = event
            .current_target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        {
            let _ = target.set_pointer_capture(event.pointer_id());
        }
        drawing.set_value(true);
        strokes.update(|strokes| strokes.push(vec![position(&event)]));
    };
    let extend = move |event: ev::PointerEvent| {
        if drawing.get_value() {
            strokes.update(|strokes| {
                if let Some(stroke) = strokes.last_mut() {
                    stroke.push(position(&event));
                }
            });
        }
    };
    let end = move |_: ev::PointerEvent| drawing.set_value(false);
    let accept = move |_| {
        store.remember_signature(strokes.get_untracked());
        store.sheet.set(None);
        store.tool.set(Tool::Sign);
    };
    view! {
        <Modal title="Add your signature" on_close=move || store.sheet.set(None)>
            <p class="modal-description">"Draw it once. ethos keeps it on this device for next time."</p>
            <svg
                node_ref=pad
                class="signature-pad"
                viewBox=format!("0 0 {WIDTH} {HEIGHT}")
                role="img"
                aria-label="Signature pad"
                on:pointerdown=begin
                on:pointermove=extend
                on:pointerup=end
                on:pointercancel=end
            >
                <line class="signature-line" x1="24" y1="128" x2="416" y2="128"></line>
                {move || {
                    strokes
                        .get()
                        .iter()
                        .map(|stroke| view! { <path d=stroke_path(stroke)></path> })
                        .collect_view()
                }}
            </svg>
            <div class="modal-actions">
                <button
                    type="button"
                    class="subtle-button"
                    disabled=move || strokes.read().is_empty()
                    on:click=move |_| strokes.set(Vec::new())
                >
                    "Clear"
                </button>
                <button
                    type="button"
                    class="primary-button"
                    disabled=move || strokes.read().is_empty()
                    on:click=accept
                >
                    "Use this signature"
                </button>
            </div>
        </Modal>
    }
}

#[component]
pub fn Sheets() -> impl IntoView {
    let store = expect_context::<Store>();
    move || {
        store.sheet.get().map(|sheet| match sheet {
            Sheet::Notes => view! { <NotesSheet /> }.into_any(),
            Sheet::Split => view! { <SplitSheet /> }.into_any(),
            Sheet::Extract => view! { <ExtractSheet /> }.into_any(),
            Sheet::Pictures => view! { <PicturesSheet /> }.into_any(),
            Sheet::Protect => view! { <ProtectSheet /> }.into_any(),
            Sheet::Signature => view! { <SignatureSheet /> }.into_any(),
            Sheet::Files(kind, paths) => view! { <FilesSheet kind=kind paths=paths /> }.into_any(),
        })
    }
}

/// Splits two strings into their shared start, differing middles and shared end.
fn changed_part<'a>(before: &'a str, after: &'a str) -> (&'a str, &'a str, &'a str, &'a str) {
    let start = before
        .char_indices()
        .zip(after.chars())
        .take_while(|((_, first), second)| first == second)
        .last()
        .map_or(0, |((index, character), _)| index + character.len_utf8());
    let (old_rest, new_rest) = (&before[start..], &after[start..]);
    let end = old_rest
        .chars()
        .rev()
        .zip(new_rest.chars().rev())
        .take_while(|(first, second)| first == second)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    (
        &before[..start],
        &old_rest[..old_rest.len() - end],
        &new_rest[..new_rest.len() - end],
        &old_rest[old_rest.len() - end..],
    )
}

/// The list of differences found by Compare. Each row jumps to its page.
#[component]
pub fn ChangesPanel() -> impl IntoView {
    let store = expect_context::<Store>();
    let close = move |_| {
        store.changes.set(None);
        store.tab.set(Tab::Pages);
    };
    view! {
        <div class="changes-panel">
            <div class="section-caption">
                <span>
                    {move || {
                        store.changes.read().as_ref().map(|comparison| {
                            match comparison.changes.len() {
                                1 => format!("1 change from {}", comparison.name),
                                count => format!("{count} changes from {}", comparison.name),
                            }
                        })
                    }}
                </span>
                <button type="button" class="text-button" on:click=close>"Done"</button>
            </div>
            {move || {
                let changes = store
                    .changes
                    .read()
                    .as_ref()
                    .map(|comparison| comparison.changes.clone())
                    .unwrap_or_default();
                if changes.is_empty() {
                    return view! {
                        <div class="sidebar-empty">
                            <Icon data=icons::CHECK size=25 stroke_width=1.3 />
                            <h3>"No differences."</h3>
                            <p>"The text of these two versions is the same."</p>
                        </div>
                    }
                        .into_any();
                }
                changes
                    .into_iter()
                    .map(|change| {
                        let page = change.page;
                        let label = match change.kind {
                            ChangeKind::Added => "Added",
                            ChangeKind::Removed => "Removed",
                            ChangeKind::Changed => "Changed",
                        };
                        let (shared, old, new, ending) = changed_part(&change.before, &change.after);
                        let body = match change.kind {
                            ChangeKind::Added => view! { <ins>{change.after.clone()}</ins> }.into_any(),
                            ChangeKind::Removed => view! { <del>{change.before.clone()}</del> }.into_any(),
                            ChangeKind::Changed => view! {
                                {shared.to_owned()}
                                <del>{old.to_owned()}</del>
                                <ins>{new.to_owned()}</ins>
                                {ending.to_owned()}
                            }
                                .into_any(),
                        };
                        view! {
                            <button
                                class="change-row"
                                data-kind=label.to_lowercase()
                                on:click=move |_| store.source_navigate(page)
                            >
                                <span>
                                    {label}
                                    <small>"Page " {move || store.display_page(page)}</small>
                                </span>
                                <p>{body}</p>
                            </button>
                        }
                    })
                    .collect_view()
                    .into_any()
            }}
        </div>
    }
}

/// A quiet offer shown on pages that are pictures of text.
#[component]
pub fn ScanHint() -> impl IntoView {
    let store = expect_context::<Store>();
    view! {
        <div class="scan-hint" role="status">
            <Icon data=icons::SCAN_TEXT size=15 />
            <span>"This page is a scan, so its text can't be selected yet."</span>
            <button
                type="button"
                class="text-button"
                on:click=move |_| {
                    store.scanned.set(false);
                    store.run_tool(ToolRequest::Searchable);
                }
            >
                "Make searchable"
            </button>
            <button
                type="button"
                class="icon-button"
                aria-label="Dismiss"
                on:click=move |_| {
                    store.scanned.set(false);
                    store.scan_dismissed.set_value(true);
                }
            >
                <Icon data=icons::X size=13 />
            </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str) -> &'static str {
        ENTRIES[ranked(&ENTRIES, query)[0]].label
    }

    #[test]
    fn names_outrank_keywords() {
        assert_eq!(top("markdown"), "Save as Markdown");
        assert_eq!(top("word"), "Word to PDF");
        assert_eq!(top("merge"), "Combine PDFs");
        assert_eq!(top("ocr"), "Make a scan searchable");
        assert!(ranked(&ENTRIES, "zebra").is_empty());
        assert_eq!(ranked(&ENTRIES, " ").len(), ENTRIES.len());
    }

    #[test]
    fn only_the_changed_words_stand_out() {
        assert_eq!(
            changed_part("The fee is 10 dollars", "The fee is 12 dollars"),
            ("The fee is 1", "0", "2", " dollars")
        );
        assert_eq!(changed_part("same", "same"), ("same", "", "", ""));
        assert_eq!(changed_part("é1", "é2"), ("é", "1", "2", ""));
    }
}
