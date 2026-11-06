//! The application root: wiring between signals, native events and the views.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ethos_core::{Mark, PageInfo, StagedFile, ToolProgress};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlElement, ResizeObserver, ResizeObserverEntry};

use crate::bridge;
use crate::chrome::{
    DiscardDialog, FloatingTools, HelpDialog, NoteDialog, PasswordDialog, ReaderStatus, TitleBar,
};
use crate::controls::{Icon, IconButton, Tooltips};
use crate::icons;
use crate::page::PdfPage;
use crate::sidebar::{DocumentSidebar, LibraryView};
use crate::store::{Document, PaletteOrigin, Store, Tab, Tool};
use crate::tools::{Palette, ScanHint, Sheets, ToolStatus};

/// Pages searched between progress updates.
const SEARCH_BATCH: usize = 16;
const AUTOSAVE_DELAY_MS: u64 = 300;

fn on_key(store: Store, event: &web_sys::KeyboardEvent) {
    if document()
        .query_selector("[role=\"dialog\"], [role=\"listbox\"]")
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    let key = event.key();
    let lower = key.to_lowercase();
    let modifier = event.ctrl_key() || event.meta_key();
    // WebView2 would otherwise act on these browser shortcuts inside a desktop app.
    if key == "F3" || key == "F7" || (modifier && matches!(lower.as_str(), "p" | "g" | "u" | "j")) {
        event.prevent_default();
    }
    if modifier && lower == "k" {
        event.prevent_default();
        store.palette.set(Some(PaletteOrigin::Keyboard));
    }
    if modifier && lower == "o" {
        event.prevent_default();
        store.open();
    }
    if modifier && lower == "s" {
        event.prevent_default();
        store.save();
    }
    let typing = event
        .target()
        .and_then(|target| target.dyn_into::<HtmlElement>().ok())
        .is_some_and(|element| {
            matches!(element.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                || element.is_content_editable()
        });
    if typing {
        return;
    }
    if modifier && lower == "z" {
        event.prevent_default();
        if event.shift_key() {
            store.redo();
        } else {
            store.undo();
        }
    }
    if modifier && lower == "y" {
        event.prevent_default();
        store.redo();
    }
    if modifier && lower == "f" {
        event.prevent_default();
        store.show_search();
    }
    if !modifier && !event.alt_key() {
        match key.as_str() {
            "v" | "Escape" => {
                store.tool.set(Tool::Select);
                store.selection.update(std::collections::BTreeSet::clear);
            }
            "h" => store.tool.set(Tool::Highlight),
            "d" => store.tool.set(Tool::Pen),
            "t" => store.tool.set(Tool::Text),
            "s" if store.doc.read_untracked().is_some() => store.start_signing(),
            "r" if store.doc.read_untracked().is_some() => store.tool.set(Tool::Redact),
            "?" => store.help.set(true),
            _ => {}
        }
    }
}

/// Runs a search in batches, so results appear as they arrive and newer queries stop older ones.
async fn run_search(store: Store, token: u64, query: String, sources: Vec<u32>, generation: u64) {
    let current = move || generation == store.search_generation.get_value();
    let mut hits = Vec::new();
    for batch in sources.chunks(SEARCH_BATCH) {
        match bridge::search_pages(token, &query, batch).await {
            Ok(found) => {
                if !current() {
                    return;
                }
                if !found.is_empty() {
                    hits.extend(found);
                    store.hits.set(hits.clone());
                }
            }
            Err(error) => {
                if current() {
                    store.notify(error, true);
                    store.searching.set(false);
                }
                return;
            }
        }
    }
    if current() {
        store.searching.set(false);
    }
}

fn watch_search(store: Store, pages: Memo<Vec<u32>>) {
    Effect::new(move |_| {
        let doc = store.doc.get();
        let query = store.query.get();
        let sources = pages.get();
        let searchable = store.tab.get() == Tab::Search && store.sidebar.get();
        store
            .search_generation
            .update_value(|generation| *generation += 1);
        let generation = store.search_generation.get_value();
        let Some(doc) = doc.filter(|_| !query.trim().is_empty()) else {
            store.hits.set(Vec::new());
            store.searching.set(false);
            return;
        };
        if !searchable {
            store.searching.set(false);
            return;
        }
        store.searching.set(true);
        store.hits.set(Vec::new());
        let token = doc.token;
        let start = move || spawn_local(run_search(store, token, query, sources, generation));
        if let Ok(timer) = set_timeout_with_handle(start, Duration::from_millis(180)) {
            on_cleanup(move || timer.clear());
        }
    });
}

/// Keeps `area_width` in step with the document scroller for fit-width zoom.
fn watch_area(store: Store) {
    Effect::new(move |_| {
        store.doc.track();
        let Some(node) = store.scroll.get() else {
            return;
        };
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            if let Ok(entry) = entries.get(0).dyn_into::<ResizeObserverEntry>() {
                store.area_width.set(entry.content_rect().width());
            }
        });
        let Ok(observer) = ResizeObserver::new(callback.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&node);
        let keep = SendWrapper::new((observer, callback));
        on_cleanup(move || keep.0.disconnect());
    });
}

fn wire_shell(store: Store) {
    spawn_local(async move {
        if let Ok(list) = bridge::recent_files().await {
            store.recent.set(list);
        }
    });
    bridge::listen("file-opened", move |file: StagedFile| store.load_file(file));
    bridge::listen("tool-progress", move |progress: ToolProgress| {
        store.progress.set(Some(progress));
    });
    // Edits that reach disk are safe, so the question only appears when they cannot.
    bridge::listen("confirm-discard", move |id: String| {
        spawn_local(async move {
            if store.persist().await {
                bridge::answer_discard(id, true);
            } else {
                store.discard_id.set(Some(id));
            }
        });
    });
    bridge::listen("file-error", move |error: String| store.notify(error, true));
    bridge::listen("file-dropped", move |paths: Vec<String>| {
        store.receive_drop(paths)
    });
    bridge::app_ready();
}

#[component]
fn Reader(
    doc: Arc<Document>,
    #[prop(into)] marks: Signal<HashMap<u32, Vec<Mark>>>,
) -> impl IntoView {
    let store = expect_context::<Store>();
    let token = doc.token;
    let infos: Arc<[PageInfo]> = doc.pages.clone().into();
    let pages = Memo::new(move |_| store.edits().pages.clone());
    let width = Memo::new(move |_| store.page_width());
    view! {
        <main class="reader">
            <div class="document-scroll" node_ref=store.scroll on:scroll=move |_| store.track_page()>
                <div class="document-pages">
                    <For
                        each={
                            let count = infos.len() as u32;
                            // Page lists and documents swap one signal apart; only ever
                            // draw pages this document has.
                            move || {
                                pages
                                    .get()
                                    .into_iter()
                                    .map(|entry| entry.source)
                                    .filter(move |source| *source <= count)
                            }
                        }
                        key=|source| *source
                        children=move |source| {
                            let info = infos[source as usize - 1];
                            let index = Memo::new(move |_| {
                                pages
                                    .read()
                                    .iter()
                                    .position(|entry| entry.source == source)
                                    .unwrap_or(0)
                            });
                            let rotation = Memo::new(move |_| {
                                pages
                                    .read()
                                    .iter()
                                    .find(|entry| entry.source == source)
                                    .and_then(|entry| entry.rotation)
                                    .unwrap_or(info.rotate)
                            });
                            let page_marks = Memo::new(move |_| {
                                marks.read().get(&source).cloned().unwrap_or_default()
                            });
                            view! {
                                <PdfPage
                                    token=token
                                    source=source
                                    info=info
                                    index=index
                                    rotation=rotation
                                    width=width
                                    marks=page_marks
                                />
                            }
                        }
                    />
                    <div class="document-end">
                        <Icon data=icons::CHECK size=13 />
                        "You’ve reached the end."
                    </div>
                </div>
            </div>
            <Show when=move || store.scanned.get()>
                <ScanHint />
            </Show>
            <FloatingTools />
            <ReaderStatus />
        </main>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let store = Store::new();
    provide_context(store);
    store.adopt_saved_theme();

    let edits = Memo::new(move |_| store.edits());
    let sources = Memo::new(move |_| {
        edits
            .read()
            .pages
            .iter()
            .map(|entry| entry.source)
            .collect::<Vec<_>>()
    });
    let marks_by_page = Memo::new(move |_| {
        let mut groups = HashMap::<u32, Vec<Mark>>::new();
        for mark in &edits.read().marks {
            groups.entry(mark.page).or_default().push(mark.clone());
        }
        groups
    });
    let visible_marks = Signal::derive(move || {
        let edits = edits.read();
        edits
            .marks
            .iter()
            .filter(|mark| edits.pages.iter().any(|entry| entry.source == mark.page))
            .cloned()
            .collect::<Vec<_>>()
    });

    Effect::new(move |_| {
        let dark = store.dark.get();
        let theme = if dark { "dark" } else { "light" };
        if let Some(root) = document().document_element() {
            let _ = root.set_attribute("data-theme", theme);
        }
        if let Ok(Some(storage)) = window().local_storage() {
            // A browser profile that refuses storage only loses the preference.
            let _ = storage.set_item("ethos-theme", theme);
        }
    });
    Effect::new(move |_| bridge::set_dirty(store.dirty()));
    Effect::new(move |_| {
        if !store.dirty() {
            return;
        }
        let write = move || {
            spawn_local(async move {
                store.persist().await;
            });
        };
        if let Ok(timer) = set_timeout_with_handle(write, Duration::from_millis(AUTOSAVE_DELAY_MS))
        {
            on_cleanup(move || timer.clear());
        }
    });
    Effect::new(move |_| store.page_input.set(store.page.get().to_string()));
    Effect::new(move |_| {
        // A toast with actions has to stay long enough to reach them.
        let linger = store.toast.with(|toast| match toast {
            Some(toast) if toast.error => None,
            Some(toast) if toast.result.is_some() => Some(8000),
            Some(_) => Some(3800),
            None => None,
        });
        if let Some(linger) = linger
            && let Ok(timer) = set_timeout_with_handle(
                move || store.toast.set(None),
                Duration::from_millis(linger),
            )
        {
            on_cleanup(move || timer.clear());
        }
    });
    watch_area(store);
    watch_search(store, sources);
    wire_shell(store);

    let keys = window_event_listener(ev::keydown, move |event| on_key(store, &event));
    // WebView2's page menu would offer browser actions such as Reload and Save as.
    let menu = window_event_listener(ev::contextmenu, |event| event.prevent_default());
    on_cleanup(move || {
        keys.remove();
        menu.remove();
    });

    let carries_files = |event: &ev::DragEvent| {
        event
            .data_transfer()
            .is_some_and(|transfer| transfer.types().iter().any(|kind| kind == "Files"))
    };
    let on_drag_enter = move |event: ev::DragEvent| {
        event.prevent_default();
        if carries_files(&event) {
            store.drag_depth.update_value(|depth| *depth += 1);
            store.dragging.set(true);
        }
    };
    let on_drag_over = |event: ev::DragEvent| {
        event.prevent_default();
        if let Some(transfer) = event.data_transfer() {
            transfer.set_drop_effect("copy");
        }
    };
    let on_drag_leave = move |event: ev::DragEvent| {
        event.prevent_default();
        store.drag_depth.update_value(|depth| *depth -= 1);
        if store.drag_depth.get_value() <= 0 {
            store.dragging.set(false);
        }
    };
    let on_drop = move |event: ev::DragEvent| {
        event.prevent_default();
        store.drag_depth.set_value(0);
        store.dragging.set(false);
        if let Some(files) = event.data_transfer().and_then(|transfer| transfer.files()) {
            store.drop_files(&files);
        }
    };

    let app_class = move || {
        if store.doc.read().is_some() {
            "app has-document"
        } else {
            "app "
        }
    };

    view! {
        <div
            class=app_class
            on:dragenter=on_drag_enter
            on:dragover=on_drag_over
            on:dragleave=on_drag_leave
            on:drop=on_drop
        >
            <TitleBar />
            <div class="workspace">
                {move || match store.doc.get() {
                    None => view! { <LibraryView /> }.into_any(),
                    Some(doc) => {
                        view! {
                            <DocumentSidebar visible_marks=visible_marks />
                            <Reader doc=doc marks=marks_by_page />
                        }
                            .into_any()
                    }
                }}
            </div>
            {move || {
                store
                    .toast
                    .get()
                    .map(|toast| {
                        let (class, role) = if toast.error {
                            ("toast error", "alert")
                        } else {
                            ("toast ", "status")
                        };
                        view! {
                            <div class=class role=role>
                                {if toast.error {
                                    view! { <span class="error-indicator">"!"</span> }.into_any()
                                } else {
                                    view! { <Icon data=icons::CHECK size=16 /> }.into_any()
                                }}
                                <span>{toast.text}</span>
                                {toast
                                    .result
                                    .map(|result| {
                                        let (open, find) = (result.path.clone(), result.path);
                                        view! {
                                            <button
                                                class="text-button"
                                                on:click=move |_| store.open_result(open.clone())
                                            >
                                                "Open"
                                            </button>
                                            <button
                                                class="text-button"
                                                on:click=move |_| bridge::reveal_output(&find, false)
                                            >
                                                "Show in folder"
                                            </button>
                                        }
                                    })}
                                {toast
                                    .undoable
                                    .then(|| {
                                        view! {
                                            <button class="text-button" on:click=move |_| store.undo()>
                                                "Undo"
                                            </button>
                                        }
                                    })}
                                <IconButton
                                    label="Dismiss message"
                                    on_click=move || store.toast.set(None)
                                >
                                    <Icon data=icons::X size=14 />
                                </IconButton>
                            </div>
                        }
                    })
            }}
            <Show when=move || store.loading.get() && store.password.read().is_none()>
                <div class="loading-overlay" role="status">
                    <div>
                        <Icon data=icons::LOADER_CIRCLE size=24 class="spin" />
                        <strong>"Opening your document…"</strong>
                        <span>"Getting everything in place."</span>
                    </div>
                </div>
            </Show>
            <Show when=move || store.dragging.get()>
                <div class="drop-overlay">
                    <div>
                        <Icon data=icons::FILE_PLUS_CORNER size=38 stroke_width=1.25 />
                        <h2>"Make room for a new idea."</h2>
                        <p>"Drop your PDF to open it in ethos."</p>
                    </div>
                </div>
            </Show>
            <Show when=move || store.note.read().is_some()>
                <NoteDialog />
            </Show>
            <Show when=move || store.password.read().is_some()>
                <PasswordDialog />
            </Show>
            <Show when=move || store.help.get()>
                <HelpDialog />
            </Show>
            <Show when=move || store.discard_id.read().is_some()>
                <DiscardDialog />
            </Show>
            <Show when=move || store.tool_busy.get()>
                <ToolStatus />
            </Show>
            <Sheets />
            {move || {
                store.palette.get().map(|origin| {
                    view! { <Palette anchored=origin == PaletteOrigin::Toolbar /> }
                })
            }}
            <Tooltips />
        </div>
    }
}
