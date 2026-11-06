//! A rendered PDF page with its text layer and annotations, and the sidebar thumbnail.

use std::cell::OnceCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ethos_core::{
    FieldKind, FormField, InkColor, Mark, MarkKind, PageInfo, Point, Rect, TextRun, Viewport,
};
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
use crate::controls::Icon;
use crate::icons;
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
async fn paint(canvas: &HtmlCanvasElement, request: RenderRequest) -> Result<bool, String> {
    const SAMPLES: u32 = 4000;
    let pixels = bridge::render_page(request).await?;
    let step = (pixels.length() / 4 / SAMPLES).max(1) * 4;
    let has_ink = (0..pixels.length() / step).any(|index| pixels.get_index(index * step) < 225);
    canvas.set_width(request.width);
    canvas.set_height(request.height);
    let context = opaque_context(canvas)?;
    let image =
        ImageData::new_with_js_u8_clamped_array_and_sh(&pixels, request.width, request.height)
            .map_err(|_| "This page could not be displayed.".to_owned())?;
    context
        .put_image_data(&image, 0.0, 0.0)
        .map_err(|_| "This page could not be displayed.".to_owned())?;
    Ok(has_ink)
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

fn view_corners(rect: &Rect, viewport: Viewport) -> (Point, Point) {
    (
        viewport.to_view(Point {
            x: rect.x,
            y: rect.y,
        }),
        viewport.to_view(Point {
            x: rect.x + rect.width,
            y: rect.y + rect.height,
        }),
    )
}

/// A placed signature, which follows the pointer 1:1 from wherever it was grabbed.
#[component]
fn SignatureMark(id: String, strokes: Vec<Vec<Point>>, viewport: Viewport) -> impl IntoView {
    let store = expect_context::<Store>();
    let offset = RwSignal::new((0.0_f64, 0.0_f64));
    let start = StoredValue::new(None::<(f64, f64)>);
    let id = StoredValue::new(id);
    let down = move |event: ev::PointerEvent| {
        if event.button() != 0 {
            return;
        }
        event.stop_propagation();
        if let Some(target) = event
            .current_target()
            .and_then(|target| target.dyn_into::<Element>().ok())
        {
            let _ = target.set_pointer_capture(event.pointer_id());
        }
        start.set_value(Some((
            f64::from(event.client_x()),
            f64::from(event.client_y()),
        )));
    };
    let moved = move |event: ev::PointerEvent| {
        if let Some((x, y)) = start.get_value() {
            offset.set((
                f64::from(event.client_x()) - x,
                f64::from(event.client_y()) - y,
            ));
        }
    };
    let up = move |_: ev::PointerEvent| {
        if start.get_value().is_none() {
            return;
        }
        start.set_value(None);
        let (dx, dy) = offset.get_untracked();
        if dx != 0.0 || dy != 0.0 {
            let origin = viewport.to_pdf(Point { x: 0.0, y: 0.0 });
            let target = viewport.to_pdf(Point { x: dx, y: dy });
            store.move_signature(
                &id.get_value(),
                Point {
                    x: target.x - origin.x,
                    y: target.y - origin.y,
                },
            );
        }
        offset.set((0.0, 0.0));
    };
    let paths = strokes
        .iter()
        .map(|stroke| polyline_points(stroke, viewport))
        .collect::<Vec<_>>();
    let outline = paths.clone();
    view! {
        <g
            class="signature-mark"
            transform=move || {
                let (x, y) = offset.get();
                format!("translate({x} {y})")
            }
            on:pointerdown=down
            on:pointermove=moved
            on:pointerup=up
        >
            {outline
                .into_iter()
                .map(|points| view! { <polyline class="signature-hit" points=points></polyline> })
                .collect_view()}
            {paths
                .into_iter()
                .map(|points| {
                    view! {
                        <polyline
                            points=points
                            fill="none"
                            stroke="#1b2a4a"
                            stroke-width=1.4 * viewport.scale
                            stroke-linecap="round"
                            stroke-linejoin="round"
                        ></polyline>
                    }
                })
                .collect_view()}
        </g>
    }
}

fn input_style(field: &FormField, viewport: Viewport) -> String {
    let (a, b) = view_corners(&field.rect, viewport);
    format!(
        "left:{}px;top:{}px;width:{}px;height:{}px;font-size:{}px",
        a.x.min(b.x),
        a.y.min(b.y),
        (a.x - b.x).abs(),
        (a.y - b.y).abs(),
        ((a.y - b.y).abs() * 0.6).clamp(9.0, 14.0)
    )
}

/// One form field drawn over the page. Each finished change is a single undo step.
#[component]
fn FieldInput(field: FormField, #[prop(into)] viewport: Signal<Viewport>) -> impl IntoView {
    let store = expect_context::<Store>();
    let name = field.name.clone();
    let style = {
        let field = field.clone();
        move || input_style(&field, viewport.get())
    };
    let value = {
        let field = field.clone();
        move || store.field_value(&field)
    };
    let commit = move |text: String| store.set_field(name.clone(), text);
    let control = match field.kind.clone() {
        FieldKind::Text { multiline: true } => view! {
            <textarea
                aria-label=field.name.clone()
                prop:value=value
                on:change=move |event| commit(event_target_value(&event))
            ></textarea>
        }
        .into_any(),
        FieldKind::Text { multiline: false } => view! {
            <input
                aria-label=field.name.clone()
                prop:value=value
                on:change=move |event| commit(event_target_value(&event))
            />
        }
        .into_any(),
        FieldKind::Check { on } => {
            let state = on.clone();
            let checked = move || value() == state;
            view! {
                <input
                    type="checkbox"
                    aria-label=field.name.clone()
                    prop:checked=checked
                    on:change=move |event| {
                        let target: web_sys::HtmlInputElement = event_target(&event);
                        commit(if target.checked() { on.clone() } else { String::new() });
                    }
                />
            }
            .into_any()
        }
        FieldKind::Radio { on } => {
            let state = on.clone();
            let checked = move || value() == state;
            view! {
                <input
                    type="radio"
                    name=field.name.clone()
                    aria-label=field.name.clone()
                    prop:checked=checked
                    on:change=move |_| commit(on.clone())
                />
            }
            .into_any()
        }
        FieldKind::Choice { options } => {
            let current = value.clone();
            view! {
                <select
                    aria-label=field.name.clone()
                    prop:value=current
                    on:change=move |event| commit(event_target_value(&event))
                >
                    <option value=""></option>
                    {options
                        .into_iter()
                        .map(|option| view! { <option value=option.value>{option.label}</option> })
                        .collect_view()}
                </select>
            }
            .into_any()
        }
    };
    view! {
        <div class="form-field" style=style>
            {control}
        </div>
    }
}

fn mark_view(mark: Mark, viewport: Viewport) -> impl IntoView + use<> {
    let scale = viewport.scale;
    match mark.kind {
        MarkKind::Signature { strokes } => {
            view! { <SignatureMark id=mark.id strokes=strokes viewport=viewport /> }.into_any()
        }
        MarkKind::Redaction { rect } => {
            let (a, b) = view_corners(&rect, viewport);
            view! {
                <rect
                    class="redaction-mark"
                    x=a.x.min(b.x)
                    y=a.y.min(b.y)
                    width=(a.x - b.x).abs()
                    height=(a.y - b.y).abs()
                ></rect>
            }
            .into_any()
        }
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
                let has_ink = paint(&canvas, request).await?;
                task_painted.store(true, Ordering::Relaxed);
                if task_cancelled.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let runs = bridge::page_text(token, source).await?;
                if runs.is_empty() && has_ink && !store.scan_dismissed.get_value() {
                    store.scanned.set(true);
                }
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
        match store.tool.get_untracked() {
            Tool::Text => {
                store.begin_note(source, point_of(&event));
                return;
            }
            Tool::Sign => {
                store.place_signature(source, point_of(&event));
                return;
            }
            _ => {}
        }
        if let Some(target) = target_of(&event) {
            let _ = target.set_pointer_capture(event.pointer_id());
        }
        capturing.set_value(true);
        draft.set(vec![point_of(&event)]);
    };
    let hovering = RwSignal::new(None::<Point>);
    let on_pointer_move = move |event: ev::PointerEvent| {
        if store.tool.get_untracked() == Tool::Sign {
            hovering.set(Some(point_of(&event)));
            return;
        }
        if !capturing.get_value() {
            return;
        }
        if store.tool.get_untracked() == Tool::Redact {
            // A redaction is a box, so only its two corners matter.
            let corner = point_of(&event);
            draft.update(|points| {
                points.truncate(1);
                points.push(corner);
            });
        } else if draft.with_untracked(Vec::len) < MAX_STROKE_POINTS {
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
        if store.tool.get_untracked() == Tool::Redact {
            if let [from, to] = points[..] {
                let rect = Rect {
                    x: from.x.min(to.x),
                    y: from.y.min(to.y),
                    width: (from.x - to.x).abs(),
                    height: (from.y - to.y).abs(),
                };
                if rect.width > 2.0 && rect.height > 2.0 {
                    store.add_redaction(source, rect);
                }
            }
        } else if points.len() > 1 {
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
    let draws = move || {
        matches!(
            store.tool.get(),
            Tool::Pen | Tool::Text | Tool::Sign | Tool::Redact
        ) && rendered.get()
    };
    let fields = Memo::new(move |_| {
        let page_fields = store.fields.read();
        page_fields
            .iter()
            .filter(|field| field.page == source)
            .cloned()
            .collect::<Vec<_>>()
    });
    let changed = Memo::new(move |_| {
        store
            .changes
            .read()
            .as_ref()
            .map_or_else(Vec::new, |comparison| {
                comparison
                    .changes
                    .iter()
                    .filter(|change| change.page == source)
                    .flat_map(|change| change.rects.clone())
                    .collect::<Vec<_>>()
            })
    });
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
                        let view = viewport.get();
                        changed
                            .get()
                            .iter()
                            .map(|rect| {
                                let (a, b) = view_corners(rect, view);
                                view! {
                                    <rect
                                        class="change-mark"
                                        x=a.x.min(b.x) - 2.0
                                        y=a.y.min(b.y) - 1.0
                                        width=(a.x - b.x).abs() + 4.0
                                        height=(a.y - b.y).abs() + 2.0
                                        rx="2"
                                    ></rect>
                                }
                            })
                            .collect_view()
                    }}
                    {move || {
                        let center = hovering.get().filter(|_| store.tool.get() == Tool::Sign)?;
                        let view = viewport.get();
                        let strokes = store.signature_at(center)?;
                        let paths = strokes
                            .iter()
                            .map(|stroke| {
                                view! {
                                    <polyline
                                        class="signature-preview"
                                        points=polyline_points(stroke, view)
                                        stroke-width=1.4 * view.scale
                                    ></polyline>
                                }
                            })
                            .collect_view();
                        Some(paths)
                    }}
                    {move || {
                        let box_points = (store.tool.get() == Tool::Redact).then(|| draft.get()).filter(|points| points.len() == 2);
                        box_points.map(|points| {
                            let view = viewport.get();
                            let (a, b) = (view.to_view(points[0]), view.to_view(points[1]));
                            view! {
                                <rect
                                    class="redaction-draft"
                                    x=a.x.min(b.x)
                                    y=a.y.min(b.y)
                                    width=(a.x - b.x).abs()
                                    height=(a.y - b.y).abs()
                                ></rect>
                            }
                        })
                    }}
                    {move || {
                        draft_stroke()
                            .filter(|_| store.tool.get() != Tool::Redact)
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
                <For
                    each=move || fields.get()
                    key=|field| (field.name.clone(), (field.rect.x * 10.0) as i64)
                    children=move |field| view! { <FieldInput field=field viewport=viewport /> }
                />
                <Show when=draws>
                    <div
                        class="drawing-layer"
                        on:pointerdown=on_pointer_down
                        on:pointermove=on_pointer_move
                        on:pointerup=on_pointer_up
                        on:pointercancel=on_pointer_cancel
                        on:pointerleave=move |_| hovering.set(None)
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

/// A thumbnail being dragged to a new place in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct PageDrag {
    pub source: u32,
    pub from: usize,
    pub to: usize,
    pub offset: f64,
    pub slot: f64,
}

/// Where the page with center `center` belongs among thumbnails with the given centers.
fn nearest_slot(centers: &[f64], center: f64) -> usize {
    centers
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - center).abs().total_cmp(&(*b - center).abs()))
        .map_or(0, |(index, _)| index)
}

#[component]
pub fn Thumbnail(
    token: u64,
    source: u32,
    info: PageInfo,
    #[prop(into)] rotation: Signal<u16>,
    #[prop(into)] index: Signal<usize>,
    #[prop(into)] active: Signal<bool>,
    #[prop(into)] selected: Signal<bool>,
    drag: RwSignal<Option<PageDrag>>,
    settling: RwSignal<Option<(u32, f64)>>,
) -> impl IntoView {
    const DRAG_THRESHOLD: f64 = 6.0;
    const EDGE: f64 = 36.0;
    let store = expect_context::<Store>();
    let pressed = StoredValue::new(None::<f64>);
    let centers = StoredValue::new(Vec::<f64>::new());
    let dragged_last = StoredValue::new(false);
    let target_of = |event: &ev::PointerEvent| {
        event
            .current_target()
            .and_then(|target| target.dyn_into::<Element>().ok())
    };
    let on_down = move |event: ev::PointerEvent| {
        dragged_last.set_value(false);
        if event.button() == 0 && !event.ctrl_key() && !event.shift_key() {
            pressed.set_value(Some(f64::from(event.client_y())));
            if let Some(target) = target_of(&event) {
                let _ = target.set_pointer_capture(event.pointer_id());
            }
        }
    };
    let on_move = move |event: ev::PointerEvent| {
        let Some(start) = pressed.get_value() else {
            return;
        };
        let offset = f64::from(event.client_y()) - start;
        if drag.read_untracked().is_none() {
            if offset.abs() < DRAG_THRESHOLD {
                return;
            }
            let Ok(nodes) = document().query_selector_all(".thumbnail") else {
                return;
            };
            let rects: Vec<_> = (0..nodes.length())
                .filter_map(|index| nodes.get(index)?.dyn_into::<Element>().ok())
                .map(|node| node.get_bounding_client_rect())
                .collect();
            let middle = rects
                .iter()
                .map(|rect| rect.top() + rect.height() / 2.0)
                .collect();
            centers.set_value(middle);
            let from = index.get_untracked();
            let slot = rects
                .get(from + 1)
                .or(rects.get(from.saturating_sub(1)))
                .zip(rects.get(from))
                .map_or(200.0, |(other, own)| (other.top() - own.top()).abs());
            drag.set(Some(PageDrag {
                source,
                from,
                to: from,
                offset,
                slot,
            }));
        }
        let to = centers.with_value(|centers| {
            let own = centers.get(index.get_untracked()).copied().unwrap_or(0.0);
            nearest_slot(centers, own + offset)
        });
        drag.update(|state| {
            if let Some(state) = state {
                state.offset = offset;
                state.to = to;
            }
        });
        // Dragging to an edge of the list keeps it moving.
        if let Ok(Some(list)) = document().query_selector(".sidebar-content") {
            let bounds = list.get_bounding_client_rect();
            let y = f64::from(event.client_y());
            if y < bounds.top() + EDGE {
                list.scroll_by_with_x_and_y(0.0, -8.0);
            } else if y > bounds.bottom() - EDGE {
                list.scroll_by_with_x_and_y(0.0, 8.0);
            }
        }
    };
    let finish = move |commit: bool| {
        pressed.set_value(None);
        let Some(state) = drag.get_untracked() else {
            return;
        };
        drag.set(None);
        dragged_last.set_value(true);
        if commit && state.to != state.from {
            let landed = (state.to as f64 - state.from as f64) * state.slot;
            settling.set(Some((source, state.offset - landed)));
            let insertion = if state.to > state.from {
                state.to + 1
            } else {
                state.to
            };
            store.move_pages(source, insertion);
        } else {
            settling.set(Some((source, state.offset)));
        }
    };
    let style = move || {
        if let Some((page, residual)) = settling.get().filter(|(page, _)| *page == source) {
            debug_assert_eq!(page, source);
            return format!("--settle:{residual}px");
        }
        let state = drag.read();
        let Some(state) = state.as_ref() else {
            return String::new();
        };
        if state.source == source {
            return format!("transform:translateY({}px) scale(1.04)", state.offset);
        }
        let here = index.get();
        if state.from < here && here <= state.to {
            format!("transform:translateY({}px)", -state.slot)
        } else if state.to <= here && here < state.from {
            format!("transform:translateY({}px)", state.slot)
        } else {
            String::new()
        }
    };
    let classes = move || {
        let mut names = String::from("thumbnail");
        if active.get() {
            names.push_str(" active");
        }
        if selected.get() {
            names.push_str(" selected");
        }
        if drag
            .read()
            .as_ref()
            .is_some_and(|state| state.source == source)
        {
            names.push_str(" lifted");
        }
        if settling.get().is_some_and(|(page, _)| page == source) {
            names.push_str(" settling");
        }
        names
    };
    let on_click = move |event: ev::MouseEvent| {
        if dragged_last.get_value() {
            dragged_last.set_value(false);
            return;
        }
        let (adding, extending) = (event.ctrl_key() || event.meta_key(), event.shift_key());
        store.select_page(source, adding, extending);
        if !adding && !extending {
            store.navigate(index.get_untracked() + 1);
        }
    };
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
            class=classes
            style=style
            aria-label=move || format!("Go to page {}", index.get() + 1)
            aria-current=move || active.get().then_some("page")
            aria-pressed=move || selected.get().to_string()
            on:pointerdown=on_down
            on:pointermove=on_move
            on:pointerup=move |_| finish(true)
            on:pointercancel=move |_| finish(false)
            on:click=on_click
            on:animationend=move |_| settling.set(None)
        >
            <div class="thumbnail-paper">
                <Show when=move || visible.get()>
                    <canvas node_ref=canvas></canvas>
                </Show>
                <Show when=move || selected.get()>
                    <span class="thumbnail-check">
                        <Icon data=icons::CHECK size=11 />
                    </span>
                </Show>
            </div>
            <span>{move || index.get() + 1}</span>
        </button>
    }
}
