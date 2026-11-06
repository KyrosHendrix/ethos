//! The library and document sidebars and the library home screen.

use ethos_core::{Mark, MarkKind};
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsValue;

use crate::controls::{Icon, IconButton};
use crate::icons;
use crate::page::{PageDrag, Thumbnail};
use crate::store::{PaletteOrigin, Sheet, Store, Tab};
use crate::tools::ChangesPanel;

fn opened_date(opened_at: f64) -> String {
    let options = js_sys::Object::new();
    for (key, value) in [("month", "short"), ("day", "numeric"), ("year", "numeric")] {
        let _ = js_sys::Reflect::set(&options, &key.into(), &value.into());
    }
    js_sys::Date::new(&JsValue::from_f64(opened_at))
        .to_locale_date_string("default", &options)
        .into()
}

fn file_size(bytes: u64) -> String {
    let size = bytes as f64;
    if size < 1024.0 * 1024.0 {
        format!("{} KB", (size / 1024.0).round().max(1.0))
    } else {
        format!("{:.1} MB", size / 1024.0 / 1024.0)
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

#[component]
pub fn ThemeButton() -> impl IntoView {
    let store = expect_context::<Store>();
    let label = Signal::derive(move || {
        if store.dark.get() {
            "Use light appearance".to_owned()
        } else {
            "Use dark appearance".to_owned()
        }
    });
    view! {
        <IconButton label=label on_click=move || store.dark.update(|dark| *dark = !*dark)>
            {move || {
                let data = if store.dark.get() { icons::SUN } else { icons::MOON };
                view! { <Icon data=data size=16 /> }
            }}
        </IconButton>
    }
}

#[component]
pub fn LibraryView() -> impl IntoView {
    let store = expect_context::<Store>();
    let sidebar = store.sidebar;
    let recent = store.recent;
    let open_flag = move || sidebar.get().to_string();
    let closed_flag = move || (!sidebar.get()).to_string();
    view! {
        <div id="navigation-sidebar" class="sidebar-shell library-shell" data-open=open_flag>
            <nav
                class="sidebar-rail"
                aria-label="Library navigation"
                aria-hidden=open_flag
                inert=move || sidebar.get()
            >
                <IconButton label="Library" on_click=move || sidebar.set(true)>
                    <Icon data=icons::LAYOUT_GRID size=16 />
                </IconButton>
                <IconButton label="Open PDF" on_click=move || store.open()>
                    <Icon data=icons::FOLDER_OPEN size=16 />
                </IconButton>
            </nav>
            <aside class="library-sidebar" aria-hidden=closed_flag inert=move || !sidebar.get()>
                <button class="nav-item selected" aria-current="page">
                    <Icon data=icons::LAYOUT_GRID size=16 />
                    "Library"
                    <span class="nav-count">{move || recent.read().len()}</span>
                </button>
                <button class="nav-item" on:click=move |_| store.open()>
                    <Icon data=icons::FOLDER_OPEN size=16 />
                    "Open document"
                    <kbd>"Ctrl O"</kbd>
                </button>
                <button
                    class="nav-item"
                    on:click=move |_| store.palette.set(Some(PaletteOrigin::Keyboard))
                >
                    <Icon data=icons::WRENCH size=16 />
                    "Tools"
                    <kbd>"Ctrl K"</kbd>
                </button>
                <div class="sidebar-recent-heading">"Recents"</div>
                <div class="recent-nav">
                    {move || {
                        let items = recent.get();
                        if items.is_empty() {
                            view! { <p>"Documents you open will appear here."</p> }.into_any()
                        } else {
                            items
                                .into_iter()
                                .map(|item| {
                                    let id = item.id.clone();
                                    view! {
                                        <button on:click=move |_| store.open_recent(id.clone())>
                                            <Icon data=icons::FILE_TEXT size=15 />
                                            <span>{item.name}</span>
                                        </button>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }
                    }}
                </div>
                <div class="sidebar-bottom">
                    <div class="local-label">
                        <Icon data=icons::SHIELD_CHECK size=14 />
                        <span>"Files stay on your device"</span>
                    </div>
                    <div class="sidebar-bottom-actions">
                        <span>"ethos " <span class="version">"0.1"</span></span>
                        <ThemeButton />
                        <IconButton label="Keyboard shortcuts" on_click=move || store.help.set(true)>
                            <Icon data=icons::KEYBOARD size=17 />
                        </IconButton>
                    </div>
                </div>
            </aside>
        </div>
        <main class="welcome">
            <div class="welcome-content">
                <div class="library-heading">
                    <h1>"Library"</h1>
                    <div class="library-actions">
                        <button class="sample-link" on:click=move |_| store.sample()>
                            "Open sample"
                        </button>
                        <button class="primary-button" on:click=move |_| store.open()>
                            <Icon data=icons::FOLDER_OPEN size=15 />
                            "Open PDF"
                            <kbd>"Ctrl O"</kbd>
                        </button>
                    </div>
                </div>
                <section class="recent-section" aria-labelledby="recent-files-heading">
                    <h2 id="recent-files-heading">"Recent files"</h2>
                    <table class="recent-table">
                        <thead>
                            <tr>
                                <th scope="col">"Name"</th>
                                <th scope="col">"Last opened"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                recent
                                    .get()
                                    .into_iter()
                                    .map(|item| {
                                        let id = item.id.clone();
                                        let title = item.name.clone();
                                        view! {
                                            <tr>
                                                <td>
                                                    <button
                                                        class="recent-file"
                                                        title=title
                                                        on:click=move |_| store.open_recent(id.clone())
                                                    >
                                                        <Icon data=icons::FILE_TEXT size=16 />
                                                        <span>{item.name}</span>
                                                    </button>
                                                </td>
                                                <td>{opened_date(item.opened_at)}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </tbody>
                    </table>
                    <Show when=move || recent.read().is_empty()>
                        <div class="library-empty">
                            <Icon data=icons::FILE_TEXT size=20 stroke_width=1.5 />
                            <strong>"No recent files"</strong>
                            <p>"Open a PDF or drop one into this window."</p>
                        </div>
                    </Show>
                </section>
            </div>
        </main>
    }
}

fn mark_icon(mark: &Mark) -> &'static str {
    match mark.kind {
        MarkKind::Highlight { .. } => icons::HIGHLIGHTER,
        MarkKind::Pen { .. } => icons::PEN_LINE,
        MarkKind::Text { .. } => icons::TYPE,
        MarkKind::Signature { .. } => icons::SIGNATURE,
        MarkKind::Redaction { .. } => icons::RECTANGLE_HORIZONTAL,
    }
}

fn mark_summary(mark: &Mark) -> String {
    match &mark.kind {
        MarkKind::Highlight { text, .. } | MarkKind::Text { text, .. } => text.clone(),
        MarkKind::Pen { .. } => "Handwritten mark".into(),
        MarkKind::Signature { .. } => "Signature".into(),
        MarkKind::Redaction { .. } => "Area to remove".into(),
    }
}

#[component]
fn NotesPanel(#[prop(into)] marks: Signal<Vec<Mark>>) -> impl IntoView {
    let store = expect_context::<Store>();
    view! {
        <div class="notes-list">
            <div class="section-caption">
                "Annotations " <span>{move || marks.read().len()}</span>
                <Show when=move || !marks.read().is_empty()>
                    <button
                        type="button"
                        class="text-button"
                        on:click=move |_| store.sheet.set(Some(Sheet::Notes))
                    >
                        "Export"
                    </button>
                </Show>
            </div>
            {move || {
                let items = marks.get();
                if items.is_empty() {
                    view! {
                        <div class="sidebar-empty">
                            <Icon data=icons::HIGHLIGHTER size=25 stroke_width=1.3 />
                            <h3>"Keep the good parts."</h3>
                            <p>"Highlights, drawings, and text notes will appear here."</p>
                        </div>
                    }
                        .into_any()
                } else {
                    items
                        .into_iter()
                        .map(|mark| {
                            let (source, id) = (mark.page, mark.id.clone());
                            let color = format!("color:{}", mark.color.ink());
                            view! {
                                <div class="note-item">
                                    <button
                                        class="note-jump"
                                        on:click=move |_| store.source_navigate(source)
                                    >
                                        <span class="note-kind" style=color>
                                            <Icon data=mark_icon(&mark) size=13 />
                                            <span>"Page " {move || store.display_page(source)}</span>
                                        </span>
                                        <p>{mark_summary(&mark)}</p>
                                    </button>
                                    <IconButton
                                        label="Remove annotation"
                                        on_click=move || store.remove_mark(&id)
                                    >
                                        <Icon data=icons::X size=13 />
                                    </IconButton>
                                </div>
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn SearchPanel() -> impl IntoView {
    let store = expect_context::<Store>();
    let summary = move || {
        let hits = store.hits.get();
        let total: usize = hits.iter().map(|hit| hit.count).sum();
        if total == 0 {
            "No matches".to_owned()
        } else {
            format!(
                "{} on {}",
                plural(total, "match", "matches"),
                plural(hits.len(), "page", "pages")
            )
        }
    };
    view! {
        <div class="search-panel">
            <div class="search-field">
                <Icon data=icons::SEARCH size=15 />
                <input
                    node_ref=store.search_input
                    class="ui-input"
                    aria-label="Search document"
                    placeholder="Find in document"
                    prop:value=move || store.query.get()
                    on:input=move |event| store.query.set(event_target_value(&event))
                />
                <Show when=move || !store.query.read().is_empty()>
                    <IconButton label="Clear search" on_click=move || store.query.set(String::new())>
                        <Icon data=icons::X size=13 />
                    </IconButton>
                </Show>
            </div>
            <Show when=move || store.searching.get()>
                <p class="search-status">
                    <Icon data=icons::LOADER_CIRCLE size=13 class="spin" />
                    "Searching…"
                </p>
            </Show>
            <Show when=move || !store.query.read().trim().is_empty() && !store.searching.get()>
                <p class="search-status">{summary}</p>
            </Show>
            {move || {
                store
                    .hits
                    .get()
                    .into_iter()
                    .map(|hit| {
                        let source = hit.page;
                        view! {
                            <button
                                class="search-result"
                                on:click=move |_| store.source_navigate(source)
                            >
                                <span>
                                    "Page " {move || store.display_page(source)}
                                    <small>{plural(hit.count, "match", "matches")}</small>
                                </span>
                                <p>{hit.snippet}</p>
                            </button>
                        }
                    })
                    .collect_view()
            }}
            <Show when=move || store.query.read().is_empty()>
                <div class="sidebar-empty">
                    <Icon data=icons::SEARCH size=25 stroke_width=1.3 />
                    <h3>"Find your place."</h3>
                    <p>"Search the document’s selectable text."</p>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn SidebarTabs(#[prop(into)] note_count: Signal<usize>) -> impl IntoView {
    let store = expect_context::<Store>();
    let tab = store.tab;
    let on_key = move |event: ev::KeyboardEvent, current: Tab| {
        let position = Tab::ALL
            .iter()
            .position(|item| *item == current)
            .unwrap_or(0);
        let next = match event.key().as_str() {
            "ArrowRight" => (position + 1) % Tab::ALL.len(),
            "ArrowLeft" => (position + Tab::ALL.len() - 1) % Tab::ALL.len(),
            "Home" => 0,
            "End" => Tab::ALL.len() - 1,
            _ => return,
        };
        event.prevent_default();
        tab.set(Tab::ALL[next]);
        if let Some(button) = document().get_element_by_id(Tab::ALL[next].id()) {
            let _ = wasm_bindgen::JsCast::unchecked_into::<web_sys::HtmlElement>(button).focus();
        }
    };
    view! {
        <div class="sidebar-tabs" role="tablist" aria-label="Document navigation">
            {Tab::ALL
                .map(|name| {
                    let selected = move || tab.get() == name;
                    view! {
                        <button
                            role="tab"
                            id=name.id()
                            aria-selected=move || selected().to_string()
                            aria-controls="document-navigation-panel"
                            tabindex=move || if selected() { "0" } else { "-1" }
                            class=move || if selected() { "selected" } else { "" }
                            on:keydown=move |event| on_key(event, name)
                            on:click=move |_| {
                                tab.set(name);
                                if name == Tab::Search {
                                    store.focus_search();
                                }
                            }
                        >
                            {name.label()}
                            <Show when=move || { name == Tab::Notes && note_count.get() != 0 }>
                                <span>{move || note_count.get()}</span>
                            </Show>
                        </button>
                    }
                })
                .into_iter()
                .collect_view()}
        </div>
    }
}

#[component]
pub fn DocumentSidebar(#[prop(into)] visible_marks: Signal<Vec<Mark>>) -> impl IntoView {
    let store = expect_context::<Store>();
    let sidebar = store.sidebar;
    let tab = store.tab;
    let open_flag = move || sidebar.get().to_string();
    let closed_flag = move || (!sidebar.get()).to_string();
    let show = move |name: Tab| {
        tab.set(name);
        sidebar.set(true);
    };
    let pages = Memo::new(move |_| store.edits().pages.clone());
    let note_count = Signal::derive(move || visible_marks.read().len());
    let token = Signal::derive(move || store.doc.read().as_ref().map_or(0, |doc| doc.token));
    let drag = RwSignal::new(None::<PageDrag>);
    let settling = RwSignal::new(None::<(u32, f64)>);
    let selected_count = move || store.selection.read().len();
    let panel_class = move || {
        if tab.get() == Tab::Pages {
            if drag.read().is_some() {
                "sidebar-content thumbnail-grid reordering"
            } else {
                "sidebar-content thumbnail-grid"
            }
        } else {
            "sidebar-content"
        }
    };
    let footer_size = move || store.doc.read().as_ref().map(|doc| file_size(doc.size));

    view! {
        <div id="navigation-sidebar" class="sidebar-shell document-shell" data-open=open_flag>
            <nav
                class="sidebar-rail"
                aria-label="Document navigation"
                aria-hidden=open_flag
                inert=move || sidebar.get()
            >
                <IconButton label="Library" on_click=move || store.close_document()>
                    <Icon data=icons::LAYOUT_GRID size=16 />
                </IconButton>
                <IconButton label="Pages" on_click=move || show(Tab::Pages)>
                    <Icon data=icons::FILE_TEXT size=16 />
                </IconButton>
                <IconButton label="Notes" on_click=move || show(Tab::Notes)>
                    <Icon data=icons::PEN_LINE size=16 />
                </IconButton>
                <IconButton label="Search" on_click=move || show(Tab::Search)>
                    <Icon data=icons::SEARCH size=16 />
                </IconButton>
            </nav>
            <aside class="document-sidebar" aria-hidden=closed_flag inert=move || !sidebar.get()>
                <div class="document-sidebar-header">
                    <button class="text-button back-library" on:click=move |_| store.close_document()>
                        <Icon data=icons::CHEVRON_LEFT size=14 />
                        "Library"
                    </button>
                    <IconButton
                        label="Open another PDF"
                        disabled=Signal::derive(move || store.loading.get() || store.saving.get())
                        on_click=move || store.open()
                    >
                        <Icon data=icons::FOLDER_OPEN size=16 />
                    </IconButton>
                </div>
                <SidebarTabs note_count=note_count />
                <div
                    class=panel_class
                    role="tabpanel"
                    id="document-navigation-panel"
                    aria-labelledby=move || tab.get().id()
                >
                    <Show when=move || tab.get() == Tab::Pages>
                        <For
                            each=move || {
                                let token = token.get();
                                let count = store.doc.read().as_ref().map_or(0, |doc| doc.pages.len() as u32);
                                pages
                                    .get()
                                    .into_iter()
                                    .map(move |entry| (token, entry.source))
                                    .filter(move |(_, source)| *source <= count)
                            }
                            key=|key| *key
                            children=move |(token, source)| {
                                let info = store
                                    .doc
                                    .get_untracked()
                                    .and_then(|doc| doc.info(source))
                                    .expect("listed pages exist in the document");
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
                                view! {
                                    <Thumbnail
                                        token=token
                                        source=source
                                        info=info
                                        rotation=rotation
                                        index=index
                                        active=Signal::derive(move || {
                                            store.page.get() == index.get() + 1
                                        })
                                        selected=Signal::derive(move || {
                                            store.selection.read().contains(&source)
                                        })
                                        drag=drag
                                        settling=settling
                                    />
                                }
                            }
                        />
                    </Show>
                    <Show when=move || tab.get() == Tab::Notes>
                        <NotesPanel marks=visible_marks />
                    </Show>
                    <Show when=move || tab.get() == Tab::Search>
                        <SearchPanel />
                    </Show>
                    <Show when=move || tab.get() == Tab::Changes>
                        <ChangesPanel />
                    </Show>
                </div>
                <Show
                    when=move || selected_count() != 0
                    fallback=move || {
                        view! {
                            <div class="document-sidebar-footer">
                                <span>{move || plural(pages.read().len(), "page", "pages")}</span>
                                <span>{footer_size}</span>
                            </div>
                        }
                    }
                >
                    <div class="selection-bar" role="toolbar" aria-label="Selected pages">
                        <span>{move || format!("{} selected", selected_count())}</span>
                        <IconButton
                            label="Save as a new PDF"
                            on_click=move || {
                                let pages = store.selected_numbers();
                                store.selection.update(std::collections::BTreeSet::clear);
                                store.run_tool(ethos_core::ToolRequest::Extract { pages });
                            }
                        >
                            <Icon data=icons::FILE_OUTPUT size=15 />
                        </IconButton>
                        <IconButton label="Rotate" on_click=move || store.rotate_selection()>
                            <Icon data=icons::ROTATE_CW size=15 />
                        </IconButton>
                        <IconButton label="Delete" on_click=move || store.delete_selection()>
                            <Icon data=icons::TRASH size=14 />
                        </IconButton>
                    </div>
                </Show>
            </aside>
        </div>
    }
}
