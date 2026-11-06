//! Interface controls: icons, buttons, the modal dialog, the select menu and tooltips.

use std::sync::atomic::{AtomicU32, Ordering};

use leptos::ev;
use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use std::time::Duration;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement};

use crate::icons;

const FOCUSABLE: &str = "button:not(:disabled),input:not(:disabled),textarea:not(:disabled),select:not(:disabled),[tabindex]:not([tabindex='-1'])";

/// The drawing inside an `.svg` file, without its outer tag or license comment.
fn icon_body(file: &str) -> &str {
    let inner = file.find("<svg").and_then(|start| {
        let open = start + file[start..].find('>')? + 1;
        Some(&file[open..file.rfind("</svg>")?])
    });
    inner.unwrap_or_default()
}

#[component]
pub fn Icon(
    data: &'static str,
    #[prop(default = 24)] size: u32,
    #[prop(default = 2.0)] stroke_width: f64,
    #[prop(default = "")] class: &'static str,
) -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width=size
            height=size
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width=stroke_width
            stroke-linecap="round"
            stroke-linejoin="round"
            class=class
            aria-hidden="true"
            inner_html=icon_body(data)
        ></svg>
    }
}

#[component]
pub fn IconButton(
    #[prop(into)] label: Signal<String>,
    #[prop(optional, into)] active: Option<Signal<bool>>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(default = "")] class: &'static str,
    #[prop(into)] on_click: Callback<()>,
    children: Children,
) -> impl IntoView {
    let pressed = move || active.map(|active| active.get().to_string());
    let classes = move || {
        let state = if active.is_some_and(|active| active.get()) {
            "active"
        } else {
            ""
        };
        format!("icon-button {class} {state}")
    };
    view! {
        <button
            type="button"
            class=classes
            aria-label=label
            aria-pressed=pressed
            data-hint=label
            disabled=disabled
            on:click=move |_| on_click.run(())
        >
            {children()}
        </button>
    }
}

/// Focuses the `autofocus` element, or else the dialog itself, so Tab starts inside it
/// without drawing a focus ring around the close button.
fn focus_initial(content: &Element) {
    let target = content
        .query_selector("[autofocus]")
        .ok()
        .flatten()
        .unwrap_or_else(|| content.clone());
    if let Ok(target) = target.dyn_into::<HtmlElement>() {
        // Focus can only fail on detached elements, and this one was just mounted.
        let _ = target.focus();
    }
}

/// Keeps Tab inside the dialog.
fn trap_tab(event: &web_sys::KeyboardEvent, content: &Element) {
    let Ok(nodes) = content.query_selector_all(FOCUSABLE) else {
        return;
    };
    let first = nodes.get(0);
    let last = nodes
        .length()
        .checked_sub(1)
        .and_then(|index| nodes.get(index));
    let (Some(first), Some(last)) = (first, last) else {
        event.prevent_default();
        return;
    };
    let active = document().active_element().map(web_sys::Node::from);
    let wrap_to = if event.shift_key() && active.as_ref() == Some(&first) {
        Some(last)
    } else if !content.contains(active.as_ref())
        || (!event.shift_key() && active.as_ref() == Some(&last))
    {
        Some(first)
    } else {
        None
    };
    if let Some(target) = wrap_to.and_then(|node| node.dyn_into::<HtmlElement>().ok()) {
        event.prevent_default();
        let _ = target.focus();
    }
}

static DIALOG_IDS: AtomicU32 = AtomicU32::new(0);

#[component]
pub fn Modal(
    title: &'static str,
    #[prop(into)] on_close: Callback<()>,
    children: Children,
) -> impl IntoView {
    let content = NodeRef::<html::Div>::new();
    let title_id = format!(
        "dialog-title-{}",
        DIALOG_IDS.fetch_add(1, Ordering::Relaxed)
    );
    let heading_id = title_id.clone();
    let previous = SendWrapper::new(document().active_element());
    // A dialog can open during a pointer press, and that press's own default action would
    // take focus straight back out, so focus lands on the next frame.
    content.on_load(|node| request_animation_frame(move || focus_initial(&node)));
    let keys = window_event_listener(ev::keydown, move |event| match event.key().as_str() {
        "Escape" => {
            event.prevent_default();
            on_close.run(());
        }
        "Tab" => {
            if let Some(node) = content.get_untracked() {
                trap_tab(&event, &node);
            }
        }
        _ => {}
    });
    on_cleanup(move || {
        keys.remove();
        if let Some(node) = previous
            .take()
            .and_then(|node| node.dyn_into::<HtmlElement>().ok())
        {
            let _ = node.focus();
        }
    });
    view! {
        <div class="ui-dialog-overlay" on:click=move |_| on_close.run(())></div>
        <div
            node_ref=content
            class="ui-dialog-content modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby=title_id
            tabindex="-1"
        >
            <div class="modal-heading">
                <h2 id=heading_id>{title}</h2>
                <button
                    type="button"
                    class="icon-button"
                    aria-label="Close dialog"
                    on:click=move |_| on_close.run(())
                >
                    <Icon data=icons::X size=17 />
                </button>
            </div>
            {children()}
        </div>
    }
}

/// Where a popover goes: `(left, top, available height, transform origin)`.
struct Placement {
    left: f64,
    top: f64,
    available: f64,
    origin: &'static str,
}

/// Prefers the side below the anchor, flips when only the other side fits, and stays on screen.
fn place_below(
    anchor: &web_sys::DomRect,
    width: f64,
    height: f64,
    gap: f64,
    padding: f64,
) -> Placement {
    let window = window();
    let viewport_width = window
        .inner_width()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let viewport_height = window
        .inner_height()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let below = viewport_height - anchor.bottom() - gap - padding;
    let above = anchor.top() - gap - padding;
    let flip = below < height && above > below;
    let centered = anchor.left() + anchor.width() / 2.0 - width / 2.0;
    Placement {
        left: centered.min(viewport_width - width - padding).max(padding),
        top: if flip {
            anchor.top() - gap - height.min(above)
        } else {
            anchor.bottom() + gap
        },
        available: if flip { above } else { below },
        origin: if flip { "center bottom" } else { "center top" },
    }
}

/// Picks the next option whose label starts with what was typed in the last second.
fn typeahead(options: &[(String, String)], buffer: &str, from: usize) -> Option<usize> {
    let needle = buffer.to_lowercase();
    (0..options.len())
        .map(|offset| (from + offset) % options.len())
        .find(|&index| options[index].1.to_lowercase().starts_with(&needle))
}

/// A select menu. `options` pairs each value with its visible label.
#[component]
pub fn Select(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(into)] on_change: Callback<String>,
    label: &'static str,
    #[prop(default = "")] class: &'static str,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let highlighted = RwSignal::new(0_usize);
    let trigger = NodeRef::<html::Button>::new();
    let content = NodeRef::<html::Div>::new();
    let typed = StoredValue::new(String::new());
    let typed_at = StoredValue::new(0.0_f64);

    let selected_label = move || {
        let current = value.get();
        options
            .read()
            .iter()
            .find(|(option, _)| *option == current)
            .map(|(_, label)| label.clone())
            .unwrap_or_default()
    };
    let show = move || {
        let current = value.get_untracked();
        let index = options
            .read_untracked()
            .iter()
            .position(|(option, _)| *option == current);
        highlighted.set(index.unwrap_or(0));
        open.set(true);
    };
    let close = move || {
        open.set(false);
        if let Some(node) = trigger.get_untracked() {
            let _ = node.focus();
        }
    };
    let choose = move |index: usize| {
        if let Some((option, _)) = options.read_untracked().get(index) {
            on_change.run(option.clone());
        }
        close();
    };

    let on_trigger_key = move |event: ev::KeyboardEvent| {
        if matches!(
            event.key().as_str(),
            "Enter" | " " | "ArrowDown" | "ArrowUp"
        ) {
            event.prevent_default();
            show();
        }
    };
    let on_menu_key = move |event: ev::KeyboardEvent| {
        let count = options.read_untracked().len();
        let key = event.key();
        match key.as_str() {
            "ArrowDown" => highlighted.update(|index| *index = (*index + 1).min(count - 1)),
            "ArrowUp" => highlighted.update(|index| *index = index.saturating_sub(1)),
            "Home" => highlighted.set(0),
            "End" => highlighted.set(count - 1),
            "Enter" | " " => choose(highlighted.get_untracked()),
            "Escape" => close(),
            "Tab" => {}
            single if single.chars().count() == 1 => {
                let now = js_sys::Date::now();
                if now - typed_at.get_value() > 1000.0 {
                    typed.set_value(String::new());
                }
                typed_at.set_value(now);
                typed.update_value(|buffer| buffer.push_str(single));
                let found = typed.with_value(|buffer| {
                    typeahead(
                        &options.read_untracked(),
                        buffer,
                        highlighted.get_untracked(),
                    )
                });
                if let Some(index) = found {
                    highlighted.set(index);
                }
            }
            _ => return,
        }
        event.prevent_default();
        event.stop_propagation();
    };

    // Runs for every mount, and the portal attaches its children after this, so wait a frame.
    Effect::new(move |_| {
        let Some(node) = content.get() else {
            return;
        };
        request_animation_frame(move || {
            let Some(anchor) = trigger.get_untracked() else {
                return;
            };
            let anchor = anchor.get_bounding_client_rect();
            let style = HtmlElement::style(&node);
            let _ = style.set_property("--select-trigger-width", &format!("{}px", anchor.width()));
            let bounds = node.get_bounding_client_rect();
            let placed = place_below(
                &anchor,
                bounds.width(),
                node.offset_height().into(),
                6.0,
                12.0,
            );
            let _ = style.set_property(
                "--select-available-height",
                &format!("{}px", placed.available),
            );
            let _ = style.set_property("--select-transform-origin", placed.origin);
            let _ = style.set_property("left", &format!("{}px", placed.left));
            let _ = style.set_property("top", &format!("{}px", placed.top));
            let _ = node.focus();
        });
    });

    view! {
        <button
            node_ref=trigger
            type="button"
            role="combobox"
            class=format!("ui-select-trigger {class}")
            aria-label=label
            aria-expanded=move || open.get().to_string()
            aria-haspopup="listbox"
            data-state=move || if open.get() { "open" } else { "closed" }
            on:click=move |_| show()
            on:keydown=on_trigger_key
        >
            <span>{selected_label}</span>
            <Icon data=icons::CHEVRON_DOWN size=14 />
        </button>
        <Show when=move || open.get()>
            <Portal>
                <div class="ui-select-backdrop" on:pointerdown=move |_| close()></div>
                <div
                    node_ref=content
                    class="ui-select-content"
                    role="listbox"
                    tabindex="-1"
                    aria-label=label
                    on:keydown=on_menu_key
                >
                    <div class="ui-select-viewport">
                        {move || {
                            let current = value.get();
                            options
                                .get()
                                .into_iter()
                                .enumerate()
                                .map(|(index, (option, text))| {
                                    let selected = option == current;
                                    view! {
                                        <div
                                            class="ui-select-item"
                                            role="option"
                                            aria-selected=selected.to_string()
                                            data-highlighted=move || {
                                                (highlighted.get() == index).then_some("")
                                            }
                                            on:pointermove=move |_| highlighted.set(index)
                                            on:click=move |_| choose(index)
                                        >
                                            <span>{text}</span>
                                            {selected
                                                .then(|| {
                                                    view! {
                                                        <span class="ui-select-check">
                                                            <Icon data=icons::CHECK size=14 />
                                                        </span>
                                                    }
                                                })}
                                        </div>
                                    }
                                })
                                .collect_view()
                        }}
                    </div>
                </div>
            </Portal>
        </Show>
    }
}

#[derive(Clone)]
struct Tip {
    label: String,
    anchor: SendWrapper<web_sys::DomRect>,
    instant: bool,
}

const OPEN_DELAY: Duration = Duration::from_millis(450);
const SKIP_DELAY: f64 = 150.0;

/// One tooltip for the whole app. Any element with `data-hint` gets it, with no wrapper element.
#[component]
pub fn Tooltips() -> impl IntoView {
    let tip = RwSignal::new(None::<Tip>);
    let waiting = StoredValue::new(None::<TimeoutHandle>);
    let closed_at = StoredValue::new(f64::NEG_INFINITY);
    let content = NodeRef::<html::Div>::new();

    let hide = move || {
        if let Some(timer) = waiting.get_value() {
            timer.clear();
            waiting.set_value(None);
        }
        if tip.get_untracked().is_some() {
            closed_at.set_value(js_sys::Date::now());
            tip.set(None);
        }
    };
    let show = move |anchor: Element, delayed: bool| {
        let Some(label) = anchor.get_attribute("data-hint") else {
            return;
        };
        let rect = SendWrapper::new(anchor.get_bounding_client_rect());
        let instant = js_sys::Date::now() - closed_at.get_value() < SKIP_DELAY;
        if let Some(timer) = waiting.get_value() {
            timer.clear();
            waiting.set_value(None);
        }
        let present = move || {
            tip.set(Some(Tip {
                label,
                anchor: rect,
                instant,
            }));
        };
        if delayed && !instant {
            waiting.set_value(set_timeout_with_handle(present, OPEN_DELAY).ok());
        } else {
            present();
        }
    };
    let hinted = |target: Option<web_sys::EventTarget>| {
        target
            .and_then(|target| target.dyn_into::<Element>().ok())
            .and_then(|element| element.closest("[data-hint]").ok().flatten())
    };

    let handles = [
        window_event_listener(ev::pointerover, move |event| {
            if let Some(anchor) = hinted(event.target()) {
                show(anchor, true);
            }
        }),
        window_event_listener(ev::pointerout, move |event| {
            if hinted(event.target()).is_some() {
                hide();
            }
        }),
        window_event_listener(ev::focusin, move |event| {
            let visible = |anchor: &Element| anchor.matches(":focus-visible").unwrap_or(false);
            if let Some(anchor) = hinted(event.target()).filter(visible) {
                show(anchor, false);
            }
        }),
        window_event_listener(ev::focusout, move |_| hide()),
        window_event_listener(ev::pointerdown, move |_| hide()),
        window_event_listener(ev::keydown, move |event| {
            if event.key() == "Escape" {
                hide();
            }
        }),
    ];
    on_cleanup(move || handles.into_iter().for_each(|handle| handle.remove()));

    Effect::new(move |_| {
        let (Some(node), Some(tip)) = (content.get(), tip.get_untracked()) else {
            return;
        };
        let bounds = node.get_bounding_client_rect();
        let style = HtmlElement::style(&node);
        let height = bounds.height();
        let viewport = window()
            .inner_width()
            .ok()
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0);
        let anchor = &*tip.anchor;
        let flip = anchor.top() - 7.0 - height < 8.0;
        let top = if flip {
            anchor.bottom() + 7.0
        } else {
            anchor.top() - 7.0 - height
        };
        let centered = anchor.left() + anchor.width() / 2.0 - bounds.width() / 2.0;
        let left = centered.min(viewport - bounds.width() - 8.0).max(8.0);
        let origin = if flip { "center top" } else { "center bottom" };
        let _ = style.set_property("left", &format!("{left}px"));
        let _ = style.set_property("top", &format!("{top}px"));
        let _ = style.set_property("--tooltip-transform-origin", origin);
    });

    view! {
        {move || {
            tip.get()
                .map(|tip| {
                    let state = if tip.instant { "instant-open" } else { "delayed-open" };
                    view! {
                        <div node_ref=content class="ui-tooltip" data-state=state>
                            {tip.label}
                        </div>
                    }
                })
        }}
    }
}
