//! A rendered PDF page with its text layer and annotations, and the sidebar thumbnail.

use std::cell::OnceCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ethos_core::{InkColor, Mark, MarkKind, PageInfo, Point, Rect, TextRun, Viewport};
use js_sys::{Object, Reflect};
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::prelude::*;
use web_sys::{
    CanvasRenderingContext2d, Element, HtmlCanvasElement, HtmlElement, ImageData,
    IntersectionObserver, IntersectionObserverEntry, IntersectionObserverInit,
};

use crate::bridge::{self, RenderRequest};
use crate::store::{Store, Tool};

const MAX_CANVAS_PIXELS: f64 = 4_000_000.0;
const THUMBNAIL_WIDTH: f64 = 112.0;
const MAX_STROKE_POINTS: usize = 2000;

static TICKETS: AtomicU64 = AtomicU64::new(0);

/// Tracks whether `node` is within `margin` of the scroll area matching `root`.
fn watch_visibility(
    node: &Element,
    root: &str,
    margin: &str,
    visible: RwSignal<bool>,
) -> Result<(), JsValue> {
    let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
        if let Ok(entry) = entries
            .get(entries.length() - 1)
            .dyn_into::<IntersectionObserverEntry>()
        {
            visible.set(entry.is_intersecting());
        }
    });
    let options = IntersectionObserverInit::new();
    options.set_root(node.closest(root)?.as_ref());
    options.set_root_margin(margin);
    let observer =
        IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &options)?;
    observer.observe(node);
    let keep = SendWrapper::new((observer, callback));
    on_cleanup(move || keep.0.disconnect());
    Ok(())
}

fn opaque_context(canvas: &HtmlCanvasElement) -> Result<CanvasRenderingContext2d, String> {
    let options = Object::new();
    let _ = Reflect::set(&options, &"alpha".into(), &false.into());
    canvas
        .get_context_with_context_options("2d", &options)
        .ok()
        .flatten()
        .and_then(|context| context.dyn_into().ok())
        .ok_or_else(|| "Could not create a page canvas.".to_owned())
}

/// Sizes `canvas` to `request` and paints the page into it.
async fn paint(canvas: &HtmlCanvasElement, request: RenderRequest) -> Result<(), String> {
    let pixels = bridge::render_page(request).await?;
    canvas.set_width(request.width);
    canvas.set_height(request.height);
    let context = opaque_context(canvas)?;
    let image =
        ImageData::new_with_js_u8_clamped_array_and_sh(&pixels, request.width, request.height)
            .map_err(|_| "This page could not be displayed.".to_owned())?;
    context
        .put_image_data(&image, 0.0, 0.0)
        .map_err(|_| "This page could not be displayed.".to_owned())
}

thread_local! {
    static MEASURE: OnceCell<Option<CanvasRenderingContext2d>> = const { OnceCell::new() };
}

/// The width `text` occupies in the text layer's font at `size` pixels.
fn measured_width(text: &str, size: f64) -> Option<f64> {
    MEASURE.with(|cell| {
        let context = cell.get_or_init(|| {
            let canvas: HtmlCanvasElement =
                document().create_element("canvas").ok()?.dyn_into().ok()?;
            canvas.get_context("2d").ok().flatten()?.dyn_into().ok()
        });
        let context = context.as_ref()?;
        context.set_font(&format!("{size}px sans-serif"));
        context
            .measure_text(text)
            .ok()
            .map(|metrics| metrics.width())
    })
}

/// Adds one invisible, selectable span per line, stretched to match the rendered glyphs.
fn build_text_layer(
    layer: &HtmlElement,
    runs: &[TextRun],
    viewport: Viewport,
) -> Result<(), JsValue> {
    let style = layer.style();
    style.set_property("--total-scale-factor", &viewport.scale.to_string())?;
    style.set_property("--scale-factor", &viewport.scale.to_string())?;
    layer.replace_children_with_node_0();
    let document = document();
    for run in runs {
        let placed = viewport.layout_run(run);
        let span: HtmlElement = document.create_element("span")?.dyn_into()?;
        span.set_text_content(Some(&run.text));
        let style = span.style();
        style.set_property(
            "left",
            &format!("{}%", 100.0 * placed.left / viewport.width),
        )?;
        style.set_property("top", &format!("{}%", 100.0 * placed.top / viewport.height))?;
        style.set_property(
            "--font-height",
            &format!("{}px", placed.height / viewport.scale),
        )?;
        style.set_property("--rotate", &format!("{}deg", placed.rotate))?;
        style.set_property("font-family", "sans-serif")?;
        if let Some(width) = measured_width(&run.text, placed.height).filter(|width| *width > 0.0) {
            style.set_property("--scale-x", &(placed.length / width).to_string())?;
        }
        layer.append_child(&span)?;
        let line_break = document.create_element("br")?;
        line_break.set_attribute("role", "presentation")?;
        layer.append_child(&line_break)?;
    }
    let end = document.create_element("div")?;
    end.set_class_name("endOfContent");
    layer.append_child(&end)?;
    Ok(())
}

fn polyline_points(points: &[Point], viewport: Viewport) -> String {
    points
        .iter()
        .map(|point| {
            let view = viewport.to_view(*point);
            format!("{},{}", view.x, view.y)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn mark_view(mark: Mark, viewport: Viewport) -> impl IntoView + use<> {
    let scale = viewport.scale;
    match mark.kind {
        MarkKind::Highlight { rects, .. } => {
            let ink = mark.color.ink();
            view! {
                <g>
                    {rects
                        .into_iter()
                        .map(|rect| {
                            let a = viewport.to_view(Point { x: rect.x, y: rect.y });
                            let b = viewport.to_view(Point {
                                x: rect.x + rect.width,
                                y: rect.y + rect.height,
                            });
                            view! {
                                <rect
                                    x=a.x.min(b.x)
                                    y=a.y.min(b.y)
                                    width=(a.x - b.x).abs()
                                    height=(a.y - b.y).abs()
                                    fill=ink
                                    opacity=".33"
                                ></rect>
                            }
                        })
                        .collect_view()}
                </g>
            }
            .into_any()
        }
        MarkKind::Pen { points } => view! {
            <polyline
                points=polyline_points(&points, viewport)
                fill="none"
                stroke=mark.color.stroke()
                stroke-width=1.8 * scale
                stroke-linecap="round"
                stroke-linejoin="round"
            ></polyline>
        }
        .into_any(),
        MarkKind::Text { point, text } => {
            let at = viewport.to_view(point);
            let transform = format!("rotate({} {} {})", viewport.rotation, at.x, at.y);
            let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
            view! {
                <text
                    x=at.x
                    y=at.y
                    transform=transform
                    fill=mark.color.stroke()
                    font-size=12.0 * scale
                    font-family="Arial, sans-serif"
                >
                    {lines
                        .into_iter()
                        .enumerate()
                        .map(|(index, line)| {
                            let advance = if index == 0 { 0.0 } else { 17.0 * scale };
                            view! {
                                <tspan x=at.x dy=advance>
                                    {line}
                                </tspan>
                            }
                        })
                        .collect_view()}
                </text>
            }
            .into_any()
        }
    }
}

/// Converts a pointer position to PDF points, clamped to the page.
fn to_pdf(container: &Element, viewport: Viewport, x: f64, y: f64) -> Point {
    let bounds = container.get_bounding_client_rect();
    viewport.to_pdf(Point {
        x: (x - bounds.left()).clamp(0.0, bounds.width()),
        y: (y - bounds.top()).clamp(0.0, bounds.height()),
    })
}

/// Turns the current text selection on this page into a highlight.
fn highlight_selection(
    store: Store,
    container: &Element,
    layer: &Element,
    viewport: Viewport,
    source: u32,
) {
    let Some(selected) = window().get_selection().ok().flatten() else {
        return;
    };
    let text = String::from(selected.to_string());
    if selected.range_count() == 0 || text.trim().is_empty() {
        return;
    }
    let Ok(range) = selected.get_range_at(0) else {
        return;
    };
    let inside = range
        .common_ancestor_container()
        .is_ok_and(|node| layer.contains(Some(&node)));
    if !inside || store.tool.get_untracked() != Tool::Highlight {
        return;
    }
    let mut rects = Vec::new();
    if let Some(list) = range.get_client_rects() {
        for index in 0..list.length() {
            let Some(rect) = list
                .get(index)
                .filter(|rect| rect.width() > 1.0 && rect.height() > 1.0)
            else {
                continue;
            };
            let a = to_pdf(container, viewport, rect.left(), rect.top());
            let b = to_pdf(container, viewport, rect.right(), rect.bottom());
            rects.push(Rect {
                x: a.x.min(b.x),
                y: a.y.min(b.y),
                width: (b.x - a.x).abs(),
                height: (b.y - a.y).abs(),
            });
        }
    }
    if !rects.is_empty() {
        let text: String = text.chars().take(3000).collect();
        store.add_mark(store.new_mark(source, MarkKind::Highlight { rects, text }));
    }
    // Clearing can only fail if the selection vanished, which is the goal anyway.
    let _ = selected.remove_all_ranges();
}

#[component]
pub fn PdfPage(
    token: u64,
    source: u32,
    info: PageInfo,
    #[prop(into)] index: Signal<usize>,
    #[prop(into)] rotation: Signal<u16>,
    #[prop(into)] width: Signal<f64>,
    #[prop(into)] marks: Signal<Vec<Mark>>,
) -> impl IntoView {
    let store = expect_context::<Store>();
    let container = NodeRef::<html::Div>::new();
    let canvas = NodeRef::<html::Canvas>::new();
    let text = NodeRef::<html::Div>::new();
    let visible = RwSignal::new(false);
    let rendered = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let draft = RwSignal::new(Vec::<Point>::new());
    let capturing = StoredValue::new(false);
    let viewport = Memo::new(move |_| Viewport::fit_width(info.view, rotation.get(), width.get()));

    container.on_load(move |node| {
        if watch_visibility(&node, ".document-scroll", "600px 0px", visible).is_err() {
            visible.set(true);
        }
    });

    Effect::new(move |_| {
        let view = viewport.get();
        if !visible.get() {
            rendered.set(false);
            return;
        }
        let (Some(canvas), Some(layer)) = (canvas.get(), text.get()) else {
            return;
        };
        rendered.set(false);
        error.set(String::new());
        let scale = window()
            .device_pixel_ratio()
            .min(1.5)
            .min((MAX_CANVAS_PIXELS / (view.width * view.height)).sqrt());
        let ticket = TICKETS.fetch_add(1, Ordering::Relaxed);
        let request = RenderRequest {
            ticket,
            token,
            source,
            rotation: view.rotation,
            width: (view.width * scale) as u32,
            height: (view.height * scale) as u32,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let painted = Arc::new(AtomicBool::new(false));
        let (task_cancelled, task_painted) = (Arc::clone(&cancelled), Arc::clone(&painted));
        let nodes = SendWrapper::new((canvas.clone(), layer.clone()));
        spawn_local(async move {
            let outcome = async {
                paint(&canvas, request).await?;
                task_painted.store(true, Ordering::Relaxed);
                if task_cancelled.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let runs = bridge::page_text(token, source).await?;
                if !task_cancelled.load(Ordering::Relaxed) {
                    build_text_layer(&layer, &runs, view)
                        .map_err(|_| "This page could not be displayed.".to_owned())?;
                }
                Ok::<_, String>(())
            }
            .await;
            if task_cancelled.load(Ordering::Relaxed) {
                return;
            }
            match outcome {
                Ok(()) => rendered.set(true),
                Err(reason) => error.set(reason),
            }
        });
        on_cleanup(move || {
            cancelled.store(true, Ordering::Relaxed);
            if !painted.load(Ordering::Relaxed) {
                bridge::cancel_render(ticket);
            }
            let (canvas, layer) = nodes.take();
            layer.replace_children_with_node_0();
            canvas.set_width(0);
            canvas.set_height(0);
        });
    });

    Effect::new(move |_| {
        rendered.track();
        let query = store.query.with(|query| query.trim().to_lowercase());
        let Some(layer) = text.get() else {
            return;
        };
        let Ok(spans) = layer.query_selector_all("span") else {
            return;
        };
        for index in 0..spans.length() {
            if let Some(span) = spans
                .get(index)
                .and_then(|node| node.dyn_into::<Element>().ok())
            {
                let hit = !query.is_empty()
                    && span
                        .text_content()
                        .is_some_and(|text| text.to_lowercase().contains(&query));
                let _ = span.class_list().toggle_with_force("search-hit", hit);
            }
        }
    });

    let on_mouse_up = move |_: ev::MouseEvent| {
        if let (Some(node), Some(layer)) = (container.get_untracked(), text.get_untracked()) {
            let _ = layer.class_list().remove_1("selecting");
            highlight_selection(store, &node, &layer, viewport.get_untracked(), source);
        }
    };
    let on_text_down = move |_: ev::MouseEvent| {
        if let Some(layer) = text.get_untracked() {
            let _ = layer.class_list().add_1("selecting");
        }
    };

    let point_of = move |event: &ev::PointerEvent| {
        container.get_untracked().map_or(Point::default(), |node| {
            to_pdf(
                &node,
                viewport.get_untracked(),
                f64::from(event.client_x()),
                f64::from(event.client_y()),
            )
        })
    };
    let target_of = |event: &ev::PointerEvent| {
        event
            .current_target()
            .and_then(|target| target.dyn_into::<Element>().ok())
    };
    let on_pointer_down = move |event: ev::PointerEvent| {
        if event.button() != 0 {
            return;
        }
        if store.tool.get_untracked() == Tool::Text {
            store.begin_note(source, point_of(&event));
            return;
        }
        if let Some(target) = target_of(&event) {
            let _ = target.set_pointer_capture(event.pointer_id());
        }
        capturing.set_value(true);
        draft.set(vec![point_of(&event)]);
    };
    let on_pointer_move = move |event: ev::PointerEvent| {
        if capturing.get_value() && draft.with_untracked(Vec::len) < MAX_STROKE_POINTS {
            draft.update(|points| points.push(point_of(&event)));
        }
    };
    let on_pointer_up = move |event: ev::PointerEvent| {
        if !capturing.get_value() {
            return;
        }
        capturing.set_value(false);
        if let Some(target) = target_of(&event) {
            let _ = target.release_pointer_capture(event.pointer_id());
        }
        let points = draft.get_untracked();
        if points.len() > 1 {
            store.add_mark(store.new_mark(source, MarkKind::Pen { points }));
        }
        draft.set(Vec::new());
    };
    let on_pointer_cancel = move |_: ev::PointerEvent| {
        capturing.set_value(false);
        draft.set(Vec::new());
    };

    let classes = move || format!("pdf-page {}", store.tool.get().class());
    let size = move || {
        let view = viewport.get();
        format!("width:{}px;height:{}px", view.width, view.height)
    };
    let draws = move || matches!(store.tool.get(), Tool::Pen | Tool::Text) && rendered.get();
    let draft_stroke =
        move || -> Option<InkColor> { (!draft.with(Vec::is_empty)).then(|| store.color.get()) };

    view! {
        <div
            class=classes
            data-page=move || index.get() + 1
            node_ref=container
            style=size
            on:mouseup=on_mouse_up
        >
            <Show when=move || visible.get()>
                <canvas node_ref=canvas class="page-canvas"></canvas>
                <div node_ref=text class="textLayer" on:mousedown=on_text_down></div>
                <svg
                    class="annotation-layer"
                    width=move || viewport.get().width
                    height=move || viewport.get().height
                    aria-hidden="true"
                >
                    {move || {
                        let view = viewport.get();
                        marks.get().into_iter().map(|mark| mark_view(mark, view)).collect_view()
                    }}
                    {move || {
                        draft_stroke()
                            .map(|color| {
                                view! {
                                    <polyline
                                        points=move || {
                                            draft.with(|points| polyline_points(points, viewport.get()))
                                        }
                                        fill="none"
                                        stroke=color.stroke()
                                        stroke-width="2"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                    ></polyline>
                                }
                            })
                    }}
                </svg>
                <Show when=draws>
                    <div
                        class="drawing-layer"
                        on:pointerdown=on_pointer_down
                        on:pointermove=on_pointer_move
                        on:pointerup=on_pointer_up
                        on:pointercancel=on_pointer_cancel
                    ></div>
                </Show>
            </Show>
            <Show when=move || !rendered.get()>
                <div class="page-loading">
                    {move || {
                        let reason = error.get();
                        if reason.is_empty() { format!("Page {}", index.get() + 1) } else { reason }
                    }}
                </div>
            </Show>
        </div>
    }
}

#[component]
pub fn Thumbnail(
    token: u64,
    source: u32,
    info: PageInfo,
    #[prop(into)] rotation: Signal<u16>,
    #[prop(into)] index: Signal<usize>,
    #[prop(into)] active: Signal<bool>,
    #[prop(into)] on_click: Callback<()>,
) -> impl IntoView {
    let button = NodeRef::<html::Button>::new();
    let canvas = NodeRef::<html::Canvas>::new();
    let visible = RwSignal::new(false);

    button.on_load(move |node| {
        if watch_visibility(&node, ".sidebar-content", "120px", visible).is_err() {
            visible.set(true);
        }
    });

    Effect::new(move |_| {
        let rotation = rotation.get();
        if !visible.get() {
            return;
        }
        let Some(canvas) = canvas.get() else {
            return;
        };
        let view = Viewport::fit_width(info.view, rotation, THUMBNAIL_WIDTH);
        let ticket = TICKETS.fetch_add(1, Ordering::Relaxed);
        let request = RenderRequest {
            ticket,
            token,
            source,
            rotation,
            width: view.width as u32,
            height: view.height as u32,
        };
        let done = Arc::new(AtomicBool::new(false));
        let task_done = Arc::clone(&done);
        let node = SendWrapper::new(canvas.clone());
        spawn_local(async move {
            // A missing thumbnail leaves blank paper; the page view reports the error.
            let _ = paint(&canvas, request).await;
            task_done.store(true, Ordering::Relaxed);
        });
        on_cleanup(move || {
            if !done.load(Ordering::Relaxed) {
                bridge::cancel_render(ticket);
            }
            let canvas = node.take();
            canvas.set_width(0);
            canvas.set_height(0);
        });
    });

    view! {
        <button
            node_ref=button
            class=move || if active.get() { "thumbnail active" } else { "thumbnail " }
            aria-label=move || format!("Go to page {}", index.get() + 1)
            aria-current=move || active.get().then_some("page")
            on:click=move |_| on_click.run(())
        >
            <div class="thumbnail-paper">
                <Show when=move || visible.get()>
                    <canvas node_ref=canvas></canvas>
                </Show>
            </div>
            <span>{move || index.get() + 1}</span>
        </button>
    }
}
