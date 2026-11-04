import { memo, useEffect, useRef, useState } from "react";
import { TextLayer } from "pdfjs-dist";
import type {
  PDFDocumentProxy,
  PDFPageProxy,
  PageViewport,
  RenderTask,
} from "pdfjs-dist";
import { inkColors, strokeColors } from "./pdf";
import type { InkColor, Mark, Point } from "./pdf";

export type Tool = "select" | "highlight" | "pen" | "text";
type Props = {
  pdf: PDFDocumentProxy;
  source: number;
  index: number;
  rotation: number;
  width: number;
  marks: Mark[];
  tool: Tool;
  color: InkColor;
  query: string;
  onMark: (mark: Mark) => void;
  onText: (page: number, point: Point) => void;
};

export const PdfPage = memo(function PdfPage({
  pdf,
  source,
  index,
  rotation,
  width,
  marks,
  tool,
  color,
  query,
  onMark,
  onText,
}: Props) {
  const container = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const text = useRef<HTMLDivElement>(null);
  const viewport = useRef<PageViewport | null>(null);
  const stroke = useRef<Point[]>([]);
  const [draft, setDraft] = useState<Point[]>([]);
  const [visible, setVisible] = useState(false);
  const [height, setHeight] = useState(
    width * (rotation % 180 ? 612 / 792 : 792 / 612),
  );
  const [error, setError] = useState("");
  const [rendered, setRendered] = useState(false);

  useEffect(() => {
    const node = container.current;
    if (!node) return;
    const observer = new IntersectionObserver(
      ([entry]) => setVisible(entry.isIntersecting),
      { root: node.closest(".document-scroll"), rootMargin: "600px 0px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!visible) {
      setRendered(false);
      return;
    }
    let canceled = false;
    let page: PDFPageProxy | undefined;
    let render: RenderTask | undefined;
    let layer: TextLayer | undefined;
    setRendered(false);
    setError("");
    void (async () => {
      page = await pdf.getPage(source);
      if (canceled || !canvas.current || !text.current) return;
      const natural = page.getViewport({ scale: 1, rotation });
      const view = page.getViewport({ scale: width / natural.width, rotation });
      viewport.current = view;
      setHeight(view.height);
      const scale = Math.min(
        window.devicePixelRatio || 1,
        1.5,
        Math.sqrt(4000000 / (view.width * view.height)),
      );
      const node = canvas.current;
      node.width = Math.floor(view.width * scale);
      node.height = Math.floor(view.height * scale);
      const context = node.getContext("2d", { alpha: false });
      if (!context) throw new Error("Could not create a page canvas.");
      render = page.render({
        canvasContext: context,
        canvas: node,
        viewport: view,
        transform: [scale, 0, 0, scale, 0, 0],
      });
      await render.promise;
      if (canceled) return;
      text.current.replaceChildren();
      text.current.style.setProperty(
        "--total-scale-factor",
        String(view.scale),
      );
      text.current.style.setProperty("--scale-factor", String(view.scale));
      layer = new TextLayer({
        textContentSource: page.streamTextContent(),
        container: text.current,
        viewport: view,
      });
      await layer.render();
      if (!canceled) setRendered(true);
    })().catch((reason: unknown) => {
      if (!canceled)
        setError(
          reason instanceof Error
            ? reason.message
            : "This page could not be displayed.",
        );
    });
    return () => {
      canceled = true;
      render?.cancel();
      layer?.cancel();
      if (text.current) text.current.replaceChildren();
      if (canvas.current) {
        canvas.current.width = 0;
        canvas.current.height = 0;
      }
      page?.cleanup();
    };
  }, [pdf, source, visible, width, rotation]);

  useEffect(() => {
    text.current
      ?.querySelectorAll("span")
      .forEach((span) =>
        span.classList.toggle(
          "search-hit",
          !!query &&
            !!span.textContent?.toLowerCase().includes(query.toLowerCase()),
        ),
      );
  }, [query, rendered]);

  function toPdf(clientX: number, clientY: number) {
    const bounds = container.current?.getBoundingClientRect();
    const view = viewport.current;
    if (!bounds || !view) return { x: 0, y: 0 };
    const [x, y] = view.convertToPdfPoint(
      Math.max(0, Math.min(bounds.width, clientX - bounds.left)),
      Math.max(0, Math.min(bounds.height, clientY - bounds.top)),
    );
    return { x, y };
  }

  function selection() {
    const selected = window.getSelection();
    if (!selected?.rangeCount || !text.current || !selected.toString().trim())
      return;
    const range = selected.getRangeAt(0);
    if (!text.current.contains(range.commonAncestorContainer)) return;
    if (tool !== "highlight") return;
    const rects = Array.from(range.getClientRects())
      .filter((rect) => rect.width > 1 && rect.height > 1)
      .map((rect) => {
        const a = toPdf(rect.left, rect.top);
        const b = toPdf(rect.right, rect.bottom);
        return {
          x: Math.min(a.x, b.x),
          y: Math.min(a.y, b.y),
          width: Math.abs(b.x - a.x),
          height: Math.abs(b.y - a.y),
        };
      });
    if (rects.length)
      onMark({
        id: crypto.randomUUID(),
        kind: "highlight",
        page: source,
        color,
        rects,
        text: selected.toString().slice(0, 3000),
      });
    selected.removeAllRanges();
  }

  function viewPoint(point: Point) {
    const converted = viewport.current?.convertToViewportPoint(
      point.x,
      point.y,
    );
    return converted ? { x: converted[0], y: converted[1] } : { x: 0, y: 0 };
  }

  return (
    <div
      className={`pdf-page tool-${tool}`}
      data-page={index + 1}
      ref={container}
      style={{ width, height }}
      onMouseUp={selection}
    >
      {visible && (
        <>
          <canvas ref={canvas} className="page-canvas" />
          <div ref={text} className="textLayer" />
          <svg
            className="annotation-layer"
            width={width}
            height={height}
            aria-hidden="true"
          >
            {marks.map((mark) => {
              if (mark.kind === "highlight")
                return (
                  <g key={mark.id}>
                    {mark.rects.map((rect, i) => {
                      const a = viewPoint(rect);
                      const b = viewPoint({
                        x: rect.x + rect.width,
                        y: rect.y + rect.height,
                      });
                      return (
                        <rect
                          key={i}
                          x={Math.min(a.x, b.x)}
                          y={Math.min(a.y, b.y)}
                          width={Math.abs(a.x - b.x)}
                          height={Math.abs(a.y - b.y)}
                          fill={inkColors[mark.color]}
                          opacity=".33"
                        />
                      );
                    })}
                  </g>
                );
              if (mark.kind === "pen")
                return (
                  <polyline
                    key={mark.id}
                    points={mark.points
                      .map(viewPoint)
                      .map((point) => `${point.x},${point.y}`)
                      .join(" ")}
                    fill="none"
                    stroke={strokeColors[mark.color]}
                    strokeWidth={1.8 * (viewport.current?.scale ?? 1)}
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                );
              const point = viewPoint(mark.point);
              return (
                <text
                  key={mark.id}
                  x={point.x}
                  y={point.y}
                  transform={`rotate(${rotation} ${point.x} ${point.y})`}
                  fill={strokeColors[mark.color]}
                  fontSize={12 * (viewport.current?.scale ?? 1)}
                  fontFamily="Arial, sans-serif"
                >
                  {mark.text.split("\n").map((line, i) => (
                    <tspan
                      key={i}
                      x={point.x}
                      dy={i ? 17 * (viewport.current?.scale ?? 1) : 0}
                    >
                      {line}
                    </tspan>
                  ))}
                </text>
              );
            })}
            {!!draft.length && (
              <polyline
                points={draft
                  .map(viewPoint)
                  .map((point) => `${point.x},${point.y}`)
                  .join(" ")}
                fill="none"
                stroke={strokeColors[color]}
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            )}
          </svg>
          {(tool === "pen" || tool === "text") && rendered && (
            <div
              className="drawing-layer"
              onPointerDown={(event) => {
                if (event.button !== 0) return;
                if (tool === "text") {
                  onText(source, toPdf(event.clientX, event.clientY));
                  return;
                }
                event.currentTarget.setPointerCapture(event.pointerId);
                stroke.current = [toPdf(event.clientX, event.clientY)];
                setDraft([...stroke.current]);
              }}
              onPointerMove={(event) => {
                if (
                  !event.currentTarget.hasPointerCapture(event.pointerId) ||
                  stroke.current.length >= 2000
                )
                  return;
                stroke.current.push(toPdf(event.clientX, event.clientY));
                setDraft([...stroke.current]);
              }}
              onPointerUp={(event) => {
                if (!event.currentTarget.hasPointerCapture(event.pointerId))
                  return;
                event.currentTarget.releasePointerCapture(event.pointerId);
                if (stroke.current.length > 1)
                  onMark({
                    id: crypto.randomUUID(),
                    kind: "pen",
                    page: source,
                    color,
                    points: [...stroke.current],
                  });
                stroke.current = [];
                setDraft([]);
              }}
              onPointerCancel={() => {
                stroke.current = [];
                setDraft([]);
              }}
            />
          )}
        </>
      )}
      {!rendered && (
        <div className="page-loading">{error || `Page ${index + 1}`}</div>
      )}
    </div>
  );
});

export function Thumbnail({
  pdf,
  source,
  rotation,
  index,
  active,
  onClick,
}: {
  pdf: PDFDocumentProxy;
  source: number;
  rotation: number;
  index: number;
  active: boolean;
  onClick: () => void;
}) {
  const button = useRef<HTMLButtonElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!button.current) return;
    const observer = new IntersectionObserver(
      ([entry]) => setVisible(entry.isIntersecting),
      { root: button.current.closest(".sidebar-content"), rootMargin: "120px" },
    );
    observer.observe(button.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    let canceled = false;
    let task: RenderTask | undefined;
    if (visible)
      void pdf
        .getPage(source)
        .then((page) => {
          if (canceled || !canvas.current) return;
          const natural = page.getViewport({ scale: 1, rotation });
          const view = page.getViewport({
            scale: 112 / natural.width,
            rotation,
          });
          canvas.current.width = view.width;
          canvas.current.height = view.height;
          const context = canvas.current.getContext("2d", { alpha: false });
          if (!context) return;
          task = page.render({
            canvasContext: context,
            canvas: canvas.current,
            viewport: view,
          });
          return task.promise;
        })
        .catch(() => {});
    return () => {
      canceled = true;
      task?.cancel();
      if (canvas.current) {
        canvas.current.width = 0;
        canvas.current.height = 0;
      }
    };
  }, [pdf, source, rotation, visible]);
  return (
    <button
      ref={button}
      className={`thumbnail ${active ? "active" : ""}`}
      aria-label={`Go to page ${index + 1}`}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
    >
      <div className="thumbnail-paper">
        {visible && <canvas ref={canvas} />}
      </div>
      <span>{index + 1}</span>
    </button>
  );
}
