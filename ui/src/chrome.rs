//! The title bar, reader toolbars and dialogs around the document.

use ethos_core::InkColor;
use leptos::prelude::*;

use crate::bridge;
use crate::controls::{Icon, IconButton, Modal, Select};
use crate::icons;
use crate::sidebar::ThemeButton;
use crate::store::{NOTE_LIMIT, PaletteOrigin, Store, Tool, Zoom};

const ZOOM_STEPS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
const SHORTCUTS: [(&str, &str); 13] = [
    ("Open PDF", "Ctrl O"),
    ("Save a copy", "Ctrl S"),
    ("Search document", "Ctrl F"),
    ("Undo", "Ctrl Z"),
    ("Redo", "Ctrl Shift Z"),
    ("Select text", "V"),
    ("Highlight", "H"),
    ("Draw", "D"),
    ("Add text", "T"),
    ("Sign", "S"),
    ("Redact", "R"),
    ("Find a tool", "Ctrl K"),
    ("Return to selection", "Esc"),
];

fn percent(scale: f64) -> String {
    format!("{}%", (scale * 100.0).round())
}

#[component]
pub fn TitleBar() -> impl IntoView {
    let store = expect_context::<Store>();
    let sidebar = store.sidebar;
    let name = move || store.doc.read().as_ref().map(|doc| doc.title());
    let busy = move || store.saving.get() || store.loading.get();
    view! {
        <header class="titlebar" data-tauri-drag-region>
            <button
                type="button"
                class="icon-button sidebar-toggle"
                aria-label="Toggle sidebar"
                data-hint="Toggle sidebar"
                aria-expanded=move || sidebar.get().to_string()
                aria-controls="navigation-sidebar"
                on:click=move |_| sidebar.update(|open| *open = !*open)
            >
                <Icon data=icons::PANEL_LEFT size=16 stroke_width=1.5 />
            </button>
            <Show when=move || store.doc.read().is_some()>
                <div class="document-heading">
                    <span>{name}</span>
                    <Show when=move || store.dirty()>
                        <span class="dirty-dot" aria-label="Unsaved changes"></span>
                    </Show>
                </div>
                <div class="titlebar-actions" data-tauri-drag-region>
                    <IconButton
                        label="Tools (Ctrl K)"
                        on_click=move || store.palette.set(Some(PaletteOrigin::Toolbar))
                    >
                        <Icon data=icons::WRENCH size=15 />
                    </IconButton>
                    <ThemeButton />
                    <button
                        class="primary-button save-button"
                        disabled=busy
                        on:click=move |_| store.save()
                    >
                        {move || {
                            if store.saving.get() {
                                view! { <Icon data=icons::LOADER_CIRCLE size=14 class="spin" /> }
                                    .into_any()
                            } else {
                                view! { <Icon data=icons::ARROW_DOWN_TO_LINE size=14 /> }.into_any()
                            }
                        }}
                        <span>{move || if store.saving.get() { "Saving…" } else { "Save copy" }}</span>
                    </button>
                </div>
            </Show>
            <div class="window-controls">
                <button aria-label="Minimize window" on:click=|_| bridge::window_action("minimize")>
                    <Icon data=icons::MINUS size=14 />
                </button>
                <button aria-label="Maximize window" on:click=|_| bridge::window_action("maximize")>
                    <Icon data=icons::MAXIMIZE size=12 />
                </button>
                <button
                    aria-label="Close window"
                    class="window-close"
                    on:click=|_| bridge::window_action("close")
                >
                    <Icon data=icons::X size=15 />
                </button>
            </div>
        </header>
    }
}

#[component]
pub fn FloatingTools() -> impl IntoView {
    let store = expect_context::<Store>();
    let tools = [
        (Tool::Select, icons::MOUSE_POINTER_2, "V"),
        (Tool::Highlight, icons::HIGHLIGHTER, "H"),
        (Tool::Pen, icons::PEN_LINE, "D"),
        (Tool::Text, icons::TYPE, "T"),
    ];
    let finishing = [
        (Tool::Sign, icons::SIGNATURE, "S"),
        (Tool::Redact, icons::RECTANGLE_HORIZONTAL, "R"),
    ];
    view! {
        <div class="floating-tools" role="toolbar" aria-label="Annotation tools">
            <div class="tool-group">
                {tools
                    .map(|(tool, data, shortcut)| {
                        view! {
                            <IconButton
                                label=format!("{} ({shortcut})", tool.name())
                                active=Signal::derive(move || store.tool.get() == tool)
                                on_click=move || store.tool.set(tool)
                            >
                                <Icon data=data size=18 stroke_width=1.7 />
                            </IconButton>
                        }
                    })
                    .into_iter()
                    .collect_view()}
            </div>
            <span class="toolbar-divider"></span>
            <div class="tool-group">
                {finishing
                    .map(|(tool, data, shortcut)| {
                        view! {
                            <IconButton
                                label=format!("{} ({shortcut})", tool.name())
                                active=Signal::derive(move || store.tool.get() == tool)
                                on_click=move || {
                                    if tool == Tool::Sign {
                                        store.start_signing();
                                    } else {
                                        store.tool.set(tool);
                                    }
                                }
                            >
                                <Icon data=data size=18 stroke_width=1.7 />
                            </IconButton>
                        }
                    })
                    .into_iter()
                    .collect_view()}
            </div>
            <span class="toolbar-divider"></span>
            <div class="ink-palette" aria-label="Annotation color">
                {InkColor::ALL
                    .map(|ink| {
                        let label = format!("{} ink", ink.name());
                        let selected = move || store.color.get() == ink;
                        view! {
                            <button
                                aria-label=label.clone()
                                data-hint=label
                                aria-pressed=move || selected().to_string()
                                class=move || {
                                    if selected() { "ink-swatch selected" } else { "ink-swatch " }
                                }
                                style=format!("background:{}", ink.ink())
                                on:click=move |_| store.color.set(ink)
                            >
                                <Show when=selected>
                                    <Icon data=icons::CHECK size=11 />
                                </Show>
                            </button>
                        }
                    })
                    .into_iter()
                    .collect_view()}
            </div>
            <span class="toolbar-divider"></span>
            <IconButton
                label="Undo (Ctrl Z)"
                disabled=Signal::derive(move || store.history.read().past.is_empty())
                on_click=move || store.undo()
            >
                <Icon data=icons::UNDO_2 size=17 />
            </IconButton>
            <IconButton
                label="Redo (Ctrl Shift Z)"
                disabled=Signal::derive(move || store.history.read().future.is_empty())
                on_click=move || store.redo()
            >
                <Icon data=icons::REDO_2 size=17 />
            </IconButton>
        </div>
    }
}

#[component]
pub fn ReaderStatus() -> impl IntoView {
    let store = expect_context::<Store>();
    let page = store.page;
    let page_input = store.page_input;
    let count = Memo::new(move |_| store.edits().pages.len());
    let zoom = store.zoom;
    let submit_page = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        match page_input.get_untracked().trim().parse::<usize>() {
            Ok(number) if (1..=count.get_untracked()).contains(&number) => store.navigate(number),
            _ => page_input.set(page.get_untracked().to_string()),
        }
    };
    let options = Signal::derive(move || {
        let mut options = vec![("fit".to_owned(), "Fit width".to_owned())];
        options.extend(ZOOM_STEPS.map(|step| (step.to_string(), percent(step))));
        if let Zoom::Scale(scale) = zoom.get()
            && !ZOOM_STEPS.contains(&scale)
        {
            options.push((scale.to_string(), percent(scale)));
        }
        options
    });
    let scale_at = move |limit: fn(f64) -> bool| {
        Signal::derive(move || matches!(zoom.get(), Zoom::Scale(scale) if limit(scale)))
    };
    view! {
        <footer class="reader-status">
            <span class="tool-hint">{move || store.tool.get().hint()}</span>
            <div class="page-controls">
                <IconButton
                    label="Previous page"
                    disabled=Signal::derive(move || page.get() <= 1)
                    on_click=move || store.navigate(page.get_untracked().saturating_sub(1))
                >
                    <Icon data=icons::CHEVRON_LEFT size=15 />
                </IconButton>
                <form on:submit=submit_page>
                    <input
                        class="ui-input"
                        aria-label="Page number"
                        inputmode="numeric"
                        prop:value=move || page_input.get()
                        on:input=move |event| page_input.set(event_target_value(&event))
                        on:blur=move |_| page_input.set(page.get_untracked().to_string())
                    />
                    <span>" / " {move || count.get()}</span>
                </form>
                <IconButton
                    label="Next page"
                    disabled=Signal::derive(move || page.get() >= count.get())
                    on_click=move || store.navigate(page.get_untracked() + 1)
                >
                    <Icon data=icons::CHEVRON_RIGHT size=15 />
                </IconButton>
            </div>
            <div class="view-controls">
                <IconButton label="Rotate current page" on_click=move || store.rotate_page()>
                    <Icon data=icons::ROTATE_CW size=15 />
                </IconButton>
                <IconButton
                    label="Delete current page"
                    disabled=Signal::derive(move || count.get() <= 1)
                    on_click=move || store.delete_page()
                >
                    <Icon data=icons::TRASH size=14 />
                </IconButton>
                <span class="toolbar-divider"></span>
                <IconButton
                    label="Zoom out"
                    disabled=scale_at(|scale| scale <= 0.5)
                    on_click=move || store.step_zoom(-0.15)
                >
                    <Icon data=icons::MINUS size=15 />
                </IconButton>
                <Select
                    value=Signal::derive(move || zoom.get().value())
                    options=options
                    label="Zoom level"
                    class="zoom-trigger"
                    on_change=move |value: String| {
                        zoom.set(value.parse().map_or(Zoom::Fit, Zoom::Scale))
                    }
                />
                <IconButton
                    label="Zoom in"
                    disabled=scale_at(|scale| scale >= 2.5)
                    on_click=move || store.step_zoom(0.15)
                >
                    <Icon data=icons::PLUS size=15 />
                </IconButton>
            </div>
        </footer>
    }
}

#[component]
pub fn NoteDialog() -> impl IntoView {
    let store = expect_context::<Store>();
    let note = store.note;
    let text = move || {
        note.read()
            .as_ref()
            .map(|draft| draft.text.clone())
            .unwrap_or_default()
    };
    view! {
        <Modal title="Leave a thought" on_close=move || note.set(None)>
            <form on:submit=move |event| {
                event.prevent_default();
                store.submit_note();
            }>
                <p class="modal-description">"Add a text note at the place you selected."</p>
                <textarea
                    class="ui-input note-input"
                    aria-label="Text note"
                    autofocus
                    placeholder="What’s on your mind?"
                    maxlength=NOTE_LIMIT
                    prop:value=text
                    on:input=move |event| {
                        store.note_error.set(String::new());
                        let value = event_target_value(&event).replace(['\r', '\n'], " ");
                        note.update(|draft| {
                            if let Some(draft) = draft {
                                draft.text = value;
                            }
                        });
                    }
                ></textarea>
                <Show when=move || !store.note_error.read().is_empty()>
                    <p class="inline-error" role="alert">
                        {move || store.note_error.get()}
                    </p>
                </Show>
                <div class="modal-actions">
                    <span>{move || format!("{}/{NOTE_LIMIT}", text().encode_utf16().count())}</span>
                    <button type="button" class="subtle-button" on:click=move |_| note.set(None)>
                        "Cancel"
                    </button>
                    <button class="primary-button" disabled=move || text().trim().is_empty()>
                        "Add text"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
pub fn PasswordDialog() -> impl IntoView {
    let store = expect_context::<Store>();
    let password = store.password;
    let value = move || {
        password
            .read()
            .as_ref()
            .map(|prompt| prompt.value.clone())
            .unwrap_or_default()
    };
    let incorrect = move || {
        password
            .read()
            .as_ref()
            .is_some_and(|prompt| prompt.incorrect)
    };
    view! {
        <Modal title="A little privacy" on_close=move || store.cancel_load()>
            <form on:submit=move |event| {
                event.prevent_default();
                if let Some(prompt) = password.get_untracked() {
                    store.submit_password(prompt);
                }
            }>
                <p class="modal-description">
                    {move || {
                        if incorrect() {
                            "That password didn’t work. Try again."
                        } else {
                            "This PDF is protected. Enter its password to read it."
                        }
                    }}
                </p>
                <input
                    class="ui-input password-input"
                    type="password"
                    autofocus
                    aria-label="PDF password"
                    prop:value=value
                    on:input=move |event| {
                        let text = event_target_value(&event);
                        password.update(|prompt| {
                            if let Some(prompt) = prompt {
                                prompt.value = text;
                            }
                        });
                    }
                />
                <div class="modal-actions">
                    <button class="primary-button" disabled=move || value().is_empty()>
                        "Open document"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

#[component]
pub fn HelpDialog() -> impl IntoView {
    let store = expect_context::<Store>();
    view! {
        <Modal title="A few useful shortcuts" on_close=move || store.help.set(false)>
            <div class="shortcuts">
                {SHORTCUTS
                    .map(|(label, shortcut)| {
                        view! {
                            <div>
                                <span>{label}</span>
                                <kbd>{shortcut}</kbd>
                            </div>
                        }
                    })
                    .into_iter()
                    .collect_view()}
            </div>
        </Modal>
    }
}

#[component]
pub fn DiscardDialog() -> impl IntoView {
    let store = expect_context::<Store>();
    let answer = move |discard: bool| {
        if let Some(id) = store.discard_id.get_untracked() {
            bridge::answer_discard(id, discard);
        }
        store.discard_id.set(None);
    };
    view! {
        <Modal title="Keep your changes?" on_close=move || answer(false)>
            <p class="modal-description">
                "This document has unsaved changes. Save a copy before leaving to keep your annotations and page changes."
            </p>
            <div class="modal-actions">
                <button
                    type="button"
                    class="subtle-button danger-button"
                    on:click=move |_| answer(true)
                >
                    "Discard changes"
                </button>
                <button
                    type="button"
                    autofocus
                    class="primary-button"
                    on:click=move |_| answer(false)
                >
                    "Keep editing"
                </button>
            </div>
        </Modal>
    }
}
