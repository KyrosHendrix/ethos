import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  ArrowDownToLine,
  ArrowLeft,
  ArrowRight,
  ArrowUpRight,
  BookOpen,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  FilePlus2,
  FileText,
  FolderOpen,
  Highlighter,
  Keyboard,
  LayoutGrid,
  List,
  LoaderCircle,
  Maximize,
  Minus,
  Moon,
  MousePointer2,
  PanelLeft,
  PenLine,
  Plus,
  Redo2,
  RotateCw,
  Search,
  ShieldCheck,
  Sun,
  Trash2,
  Type,
  Undo2,
  X,
} from "lucide-react";
import {
  getDocument,
  GlobalWorkerOptions,
  PasswordResponses,
} from "pdfjs-dist";
import type { PDFDocumentLoadingTask, PDFDocumentProxy } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { PdfPage, Thumbnail } from "./PdfPage";
import type { Tool } from "./PdfPage";
import { createSample, exportPdf, inkColors, wrapTextNote } from "./pdf";
import type { Edits, InkColor, Mark, Point } from "./pdf";
import type { OpenedFile, RecentFile, Result } from "./shared";
import { Modal } from "./components/Modal";
import { FileBrowser } from "./components/FileBrowser";
import { Hint } from "./components/ui/tooltip";
import { Input, Textarea } from "./components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./components/ui/select";

GlobalWorkerOptions.workerSrc = workerUrl;

type Document = OpenedFile & { pdf: PDFDocumentProxy };
type History = { past: Edits[]; present: Edits; future: Edits[] };
type SearchHit = { page: number; snippet: string; count: number };
const empty: Edits = { marks: [], pages: [] };
const noMarks: Mark[] = [];
const toolNames: Record<Tool, string> = {
  select: "Select text",
  highlight: "Highlight",
  pen: "Draw",
  text: "Add text",
};
const toolHints: Record<Tool, string> = {
  select: "Select text to copy",
  highlight: "Select a passage to highlight it",
  pen: "Draw directly on the page",
  text: "Click the page to place a text note",
};
function unwrap<T>(result: Result<T>) {
  if (!result.ok) throw new Error(result.error);
  return result.value;
}
function message(reason: unknown) {
  return reason instanceof Error
    ? reason.message
    : "Something went wrong. Please try again.";
}

function IconButton({
  label,
  children,
  active,
  ...props
}: {
  label: string;
  children: ReactNode;
  active?: boolean;
} & React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <Hint label={label}>
      <button
        type="button"
        className={`icon-button ${active ? "active" : ""}`}
        aria-label={label}
        aria-pressed={active}
        {...props}
      >
        {children}
      </button>
    </Hint>
  );
}

function MarkLogo({ size = 28 }: { size?: number }) {
  return (
    <img
      src="./ethos.svg"
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
    />
  );
}

export function App() {
  const [doc, setDoc] = useState<Document | null>(null);
  const docRef = useRef<Document | null>(null);
  const loadingTask = useRef<PDFDocumentLoadingTask | null>(null);
  const [history, setHistory] = useState<History>({
    past: [],
    present: empty,
    future: [],
  });
  const saved = useRef(empty);
  const edits = history.present;
  const dirty = edits !== saved.current;
  const [recent, setRecent] = useState<RecentFile[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [toast, setToast] = useState<{ text: string; error: boolean } | null>(
    null,
  );
  const [sidebar, setSidebar] = useState(true);
  const [tab, setTab] = useState<"pages" | "notes" | "search">("pages");
  const [page, setPage] = useState(1);
  const [pageInput, setPageInput] = useState("1");
  const [tool, setTool] = useState<Tool>("select");
  const [color, setColor] = useState<InkColor>("yellow");
  const [zoom, setZoom] = useState<number | "fit">("fit");
  const [areaWidth, setAreaWidth] = useState(900);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [note, setNote] = useState<{
    page: number;
    point: Point;
    text: string;
  } | null>(null);
  const [noteError, setNoteError] = useState("");
  const [password, setPassword] = useState<{
    update: (password: string) => void;
    incorrect: boolean;
    value: string;
  } | null>(null);
  const [help, setHelp] = useState(false);
  const [fileBrowser, setFileBrowser] = useState<"open" | "save" | null>(null);
  const [discardId, setDiscardId] = useState<string | null>(null);
  const [dark, setDark] = useState(
    () => localStorage.getItem("ethos-theme") === "dark",
  );
  const [dragging, setDragging] = useState(false);
  const scroll = useRef<HTMLDivElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);
  const scrollFrame = useRef(0);
  const loadSequence = useRef(0);
  const actions = useRef({
    open: () => {},
    save: () => {},
    undo: () => {},
    redo: () => {},
  });

  const notify = useCallback(
    (text: string, error = false) => setToast({ text, error }),
    [],
  );
  const commit = useCallback(
    (update: (edits: Edits) => Edits) =>
      setHistory((old) => ({
        past: [...old.past, old.present].slice(-40),
        present: update(old.present),
        future: [],
      })),
    [],
  );
  const addMark = useCallback(
    (mark: Mark) =>
      commit((current) => ({ ...current, marks: [...current.marks, mark] })),
    [commit],
  );
  const addText = useCallback((number: number, point: Point) => {
    setNoteError("");
    setNote({ page: number, point, text: "" });
  }, []);
  const marksByPage = useMemo(() => {
    const groups = new Map<number, Mark[]>();
    for (const mark of edits.marks) {
      const group = groups.get(mark.page);
      if (group) group.push(mark);
      else groups.set(mark.page, [mark]);
    }
    return groups;
  }, [edits.marks]);

  useEffect(() => {
    document.documentElement.dataset.theme = dark ? "dark" : "light";
    localStorage.setItem("ethos-theme", dark ? "dark" : "light");
  }, [dark]);
  useEffect(() => {
    window.ethos.setDirty(dirty);
  }, [dirty]);
  useEffect(() => {
    setPageInput(String(page));
  }, [page]);
  useEffect(() => {
    if (!toast || toast.error) return;
    const timer = setTimeout(() => setToast(null), 3800);
    return () => clearTimeout(timer);
  }, [toast]);
  useEffect(() => {
    if (!scroll.current) return;
    const observer = new ResizeObserver(([entry]) =>
      setAreaWidth(entry.contentRect.width),
    );
    observer.observe(scroll.current);
    return () => observer.disconnect();
  }, [doc]);
  useEffect(() => {
    void window.ethos.recent().then((result) => {
      if (result.ok) setRecent(result.value);
    });
    const unsubscribe = window.ethos.onFile((file) => {
      void loadFile(file);
    });
    const stopDiscard = window.ethos.onDiscardRequest(setDiscardId);
    const stopError = window.ethos.onFileError((error) => notify(error, true));
    window.ethos.ready();
    return () => {
      unsubscribe();
      stopDiscard();
      stopError();
      void loadingTask.current?.destroy();
      void docRef.current?.pdf.loadingTask.destroy();
    };
  }, []);

  async function loadFile(file: OpenedFile) {
    const sequence = ++loadSequence.current;
    setLoading(true);
    setToast(null);
    await loadingTask.current?.destroy();
    const task = getDocument({
      data: file.bytes.slice(),
      cMapUrl: new URL("./pdf-assets/cmaps/", window.location.href).href,
      cMapPacked: true,
      standardFontDataUrl: new URL(
        "./pdf-assets/standard_fonts/",
        window.location.href,
      ).href,
      wasmUrl: new URL("./pdf-assets/wasm/", window.location.href).href,
    });
    loadingTask.current = task;
    task.onPassword = (update: (password: string) => void, reason: number) =>
      setPassword({
        update,
        incorrect: reason === PasswordResponses.INCORRECT_PASSWORD,
        value: "",
      });
    try {
      const pdf = await task.promise;
      setPassword(null);
      if (sequence !== loadSequence.current) {
        await pdf.loadingTask.destroy();
        return;
      }
      if (pdf.numPages > 2000) {
        await pdf.loadingTask.destroy();
        throw new Error("This version supports documents up to 2,000 pages.");
      }
      const pages: Edits["pages"] = [];
      for (let i = 1; i <= pdf.numPages; i++)
        pages.push({ source: i, rotation: (await pdf.getPage(i)).rotate });
      const initial: Edits = { marks: [], pages };
      await docRef.current?.pdf.loadingTask.destroy();
      const next = { ...file, pdf };
      docRef.current = next;
      saved.current = initial;
      setHistory({ past: [], present: initial, future: [] });
      setDoc(next);
      setPage(1);
      setTool("select");
      setQuery("");
      setHits([]);
      setZoom("fit");
      setTab("pages");
      window.ethos.setDirty(false);
      document.title = `${file.name} · ethos`;
      const list = await window.ethos.recent();
      if (list.ok) setRecent(list.value);
      scroll.current?.scrollTo({ top: 0 });
    } catch (reason) {
      if (sequence === loadSequence.current) notify(message(reason), true);
    } finally {
      if (sequence === loadSequence.current) {
        setLoading(false);
        loadingTask.current = null;
        setPassword(null);
      }
    }
  }

  async function canLeave() {
    try {
      return !dirty || unwrap(await window.ethos.confirmDiscard());
    } catch (reason) {
      notify(message(reason), true);
      return false;
    }
  }

  function cancelLoad() {
    loadSequence.current++;
    const task = loadingTask.current;
    loadingTask.current = null;
    void task?.destroy();
    setPassword(null);
    setLoading(false);
  }

  async function open(recentId?: string) {
    if (loading || saving) return;
    if (!recentId) {
      setFileBrowser("open");
      return;
    }
    try {
      const file = unwrap(await window.ethos.openRecent(recentId));
      if (file) await loadFile(file);
    } catch (reason) {
      notify(message(reason), true);
    }
  }

  async function sample() {
    if (loading || !(await canLeave())) return;
    try {
      await loadFile({
        name: "A little more room to think.pdf",
        bytes: await createSample(),
      });
    } catch (reason) {
      notify(message(reason), true);
    }
  }

  async function closeDocument() {
    if (loading || saving || !(await canLeave())) return;
    await docRef.current?.pdf.loadingTask.destroy();
    docRef.current = null;
    saved.current = empty;
    setHistory({ past: [], present: empty, future: [] });
    setDoc(null);
    setQuery("");
    window.ethos.setDirty(false);
    document.title = "ethos";
  }

  async function save() {
    if (!doc || saving || loading) return;
    setFileBrowser("save");
  }

  async function saveTo(destination: string, overwrite: boolean) {
    if (!doc) throw new Error("Open a document first.");
    const snapshot = edits;
    setSaving(true);
    try {
      const bytes = await exportPdf(doc.bytes, snapshot);
      const outcome = unwrap(
        await window.ethos.save(bytes, destination, overwrite),
      );
      if (outcome === "saved") {
        saved.current = snapshot;
        setHistory((current) => ({ ...current }));
        notify("Copy saved. Your original is untouched.");
      }
      return outcome;
    } finally {
      setSaving(false);
    }
  }

  async function submitNote() {
    if (!note?.text.trim() || !doc) return;
    try {
      const pdfPage = await doc.pdf.getPage(note.page);
      const width = Math.max(30, pdfPage.view[2] - note.point.x - 16);
      const text = await wrapTextNote(note.text, width);
      const lines = text.split("\n").length;
      const point = {
        ...note.point,
        y: Math.max(note.point.y, pdfPage.view[1] + lines * 17 + 8),
      };
      addMark({
        id: crypto.randomUUID(),
        kind: "text",
        page: note.page,
        color,
        point,
        text,
      });
      setNote(null);
      setTool("select");
    } catch (reason) {
      setNoteError(message(reason));
    }
  }

  function undo() {
    setHistory((old) =>
      old.past.length
        ? {
            past: old.past.slice(0, -1),
            present: old.past[old.past.length - 1],
            future: [old.present, ...old.future],
          }
        : old,
    );
  }
  function redo() {
    setHistory((old) =>
      old.future.length
        ? {
            past: [...old.past, old.present],
            present: old.future[0],
            future: old.future.slice(1),
          }
        : old,
    );
  }
  actions.current = {
    open: () => void open(),
    save: () => void save(),
    undo,
    redo,
  };

  function navigate(number: number) {
    const valid = Math.max(1, Math.min(edits.pages.length, number));
    scroll.current
      ?.querySelector(`[data-page="${valid}"]`)
      ?.scrollIntoView({ block: "start", behavior: "instant" });
    setPage(valid);
  }

  function sourceNavigate(source: number) {
    const index = edits.pages.findIndex((entry) => entry.source === source);
    if (index >= 0) navigate(index + 1);
  }

  function trackPage() {
    if (scrollFrame.current) return;
    scrollFrame.current = requestAnimationFrame(() => {
      scrollFrame.current = 0;
      const node = scroll.current;
      if (!node) return;
      const nodes = node.querySelectorAll<HTMLElement>("[data-page]");
      const top = node.getBoundingClientRect().top + 110;
      let left = 0;
      let right = nodes.length - 1;
      while (left < right) {
        const middle = Math.floor((left + right) / 2);
        if (nodes[middle].getBoundingClientRect().bottom <= top)
          left = middle + 1;
        else right = middle;
      }
      if (nodes[left]) setPage(Number(nodes[left].dataset.page));
    });
  }

  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (document.querySelector('[role="dialog"], [role="listbox"]')) return;
      const input =
        event.target instanceof HTMLElement &&
        (["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName) ||
          event.target.isContentEditable);
      const modifier = event.ctrlKey || event.metaKey;
      if (modifier && event.key.toLowerCase() === "o") {
        event.preventDefault();
        actions.current.open();
      }
      if (modifier && event.key.toLowerCase() === "s") {
        event.preventDefault();
        actions.current.save();
      }
      if (input) return;
      if (modifier && event.key.toLowerCase() === "z") {
        event.preventDefault();
        event.shiftKey ? actions.current.redo() : actions.current.undo();
      }
      if (modifier && event.key.toLowerCase() === "y") {
        event.preventDefault();
        actions.current.redo();
      }
      if (modifier && event.key.toLowerCase() === "f") {
        event.preventDefault();
        setSidebar(true);
        setTab("search");
        requestAnimationFrame(() => searchInput.current?.focus());
      }
      if (!modifier && !event.altKey) {
        if (event.key === "v" || event.key === "Escape") setTool("select");
        if (event.key === "h") setTool("highlight");
        if (event.key === "d") setTool("pen");
        if (event.key === "t") setTool("text");
        if (event.key === "?") setHelp(true);
      }
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, []);

  useEffect(() => {
    if (!doc || !query.trim()) {
      setHits([]);
      setSearching(false);
      return;
    }
    let canceled = false;
    setSearching(true);
    const timer = setTimeout(() => {
      void (async () => {
        const results: SearchHit[] = [];
        const needle = query.trim().toLowerCase();
        for (const entry of edits.pages) {
          if (canceled) return;
          const pdfPage = await doc.pdf.getPage(entry.source);
          const content = await pdfPage.getTextContent();
          const text = content.items
            .map((item) => ("str" in item ? item.str : ""))
            .join(" ");
          const lower = text.toLowerCase();
          const index = lower.indexOf(needle);
          if (index >= 0) {
            let count = 0;
            let cursor = 0;
            while ((cursor = lower.indexOf(needle, cursor)) >= 0) {
              count++;
              cursor += needle.length;
            }
            results.push({
              page: entry.source,
              snippet: `${index > 35 ? "…" : ""}${text.slice(Math.max(0, index - 35), index + needle.length + 85)}…`,
              count,
            });
          }
          if (!canceled) setHits([...results]);
        }
      })()
        .catch((reason: unknown) => {
          if (!canceled) notify(message(reason), true);
        })
        .finally(() => {
          if (!canceled) setSearching(false);
        });
    }, 180);
    return () => {
      canceled = true;
      clearTimeout(timer);
    };
  }, [doc, query, edits.pages, notify]);

  const pageWidth =
    zoom === "fit" ? Math.max(280, Math.min(860, areaWidth - 96)) : 612 * zoom;
  const visibleMarks = edits.marks.filter((mark) =>
    edits.pages.some((entry) => entry.source === mark.page),
  );

  return (
    <div
      className={`app ${doc ? "has-document" : ""}`}
      onDragEnter={(event) => {
        event.preventDefault();
        if (event.dataTransfer.types.includes("Files")) {
          dragDepth.current++;
          setDragging(true);
        }
      }}
      onDragOver={(event) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
      }}
      onDragLeave={(event) => {
        event.preventDefault();
        dragDepth.current--;
        if (dragDepth.current <= 0) setDragging(false);
      }}
      onDrop={(event) => {
        event.preventDefault();
        dragDepth.current = 0;
        setDragging(false);
        const file = event.dataTransfer.files[0];
        if (!file || loading || saving) return;
        void (async () => {
          try {
            const opened = unwrap(await window.ethos.openDropped(file));
            if (opened) await loadFile(opened);
          } catch (reason) {
            notify(message(reason), true);
          }
        })();
      }}
    >
      <header className="titlebar">
        <button
          className="brand"
          onClick={() => void closeDocument()}
          aria-label="Return to library"
        >
          <MarkLogo size={25} />
          <span>ethos</span>
        </button>
        <div className="titlebar-center">
          {doc ? (
            <>
              <FileText size={12} />
              <span>{doc.name}</span>
              {dirty && (
                <Hint label="Unsaved changes">
                  <span
                    className="dirty-dot"
                    aria-label="Unsaved changes"
                    tabIndex={0}
                  />
                </Hint>
              )}
            </>
          ) : (
            <span>Your document workspace</span>
          )}
        </div>
        <div className="window-controls">
          <button
            aria-label="Minimize window"
            onClick={() => window.ethos.windowAction("minimize")}
          >
            <Minus size={14} />
          </button>
          <button
            aria-label="Maximize window"
            onClick={() => window.ethos.windowAction("maximize")}
          >
            <Maximize size={12} />
          </button>
          <button
            aria-label="Close window"
            className="window-close"
            onClick={() => window.ethos.windowAction("close")}
          >
            <X size={15} />
          </button>
        </div>
      </header>
      <div className="workspace">
        {!doc ? (
          <>
            <aside className="library-sidebar">
              <div className="sidebar-topline">WORKSPACE</div>
              <button className="nav-item selected">
                <LayoutGrid size={16} />
                Library<span className="nav-count">{recent.length}</span>
              </button>
              <button className="nav-item" onClick={() => void open()}>
                <FolderOpen size={16} />
                Open document<kbd>Ctrl O</kbd>
              </button>
              <div className="sidebar-recent-heading">RECENT DOCUMENTS</div>
              <div className="recent-nav">
                {recent.length ? (
                  recent.map((item) => (
                    <button key={item.id} onClick={() => void open(item.id)}>
                      <FileText size={15} />
                      <span>{item.name}</span>
                    </button>
                  ))
                ) : (
                  <p>
                    The documents you open
                    <br />
                    will appear here.
                  </p>
                )}
              </div>
              <div className="sidebar-bottom">
                <div className="local-label">
                  <ShieldCheck size={14} />
                  <span>Files stay on your device</span>
                </div>
                <div className="sidebar-bottom-actions">
                  <span>
                    ethos <span className="version">0.1</span>
                  </span>
                  <IconButton
                    label={
                      dark ? "Use light appearance" : "Use dark appearance"
                    }
                    onClick={() => setDark(!dark)}
                  >
                    {dark ? <Sun size={16} /> : <Moon size={16} />}
                  </IconButton>
                  <IconButton
                    label="Keyboard shortcuts"
                    onClick={() => setHelp(true)}
                  >
                    <Keyboard size={17} />
                  </IconButton>
                </div>
              </div>
            </aside>
            <main className="welcome">
              <div className="welcome-top">
                <span>Library</span>
                <button className="subtle-button" onClick={() => void open()}>
                  <Plus size={15} />
                  Open PDF
                </button>
              </div>
              <div className="welcome-content">
                <div className="welcome-icon">
                  <MarkLogo size={50} />
                </div>
                <div className="eyebrow">A LITTLE MORE ROOM TO THINK</div>
                <h1>
                  Your documents.
                  <br />
                  <span>Your space.</span>
                </h1>
                <p className="welcome-description">
                  Read closely. Make your mark.
                  <br />A quieter way to work with PDFs.
                </p>
                <div
                  className="open-card"
                  onClick={() => void open()}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      void open();
                    }
                  }}
                >
                  <div className="open-card-icon">
                    <FilePlus2 size={25} strokeWidth={1.4} />
                  </div>
                  <div>
                    <strong>Bring a document into focus</strong>
                    <p>Drop a PDF here, or browse your files</p>
                  </div>
                  <span className="primary-button">
                    Open PDF <ArrowUpRight size={15} />
                  </span>
                </div>
                <button className="sample-link" onClick={() => void sample()}>
                  Or take a look around with a sample document{" "}
                  <ArrowRight size={14} />
                </button>
                <div className="welcome-details">
                  <span>
                    <BookOpen size={14} />
                    Read without distractions
                  </span>
                  <span>
                    <PenLine size={14} />
                    Make it yours
                  </span>
                </div>
                {recent.length > 0 && (
                  <section className="recent-section">
                    <div>
                      <h2>Pick up where you left off</h2>
                      <span>
                        {recent.length} recent{" "}
                        {recent.length === 1 ? "document" : "documents"}
                      </span>
                    </div>
                    <div className="recent-cards">
                      {recent.slice(0, 3).map((item) => (
                        <button
                          key={item.id}
                          onClick={() => void open(item.id)}
                        >
                          <div className="recent-file-icon">
                            <FileText size={24} strokeWidth={1.25} />
                          </div>
                          <strong>{item.name}</strong>
                          <span>
                            {new Date(item.openedAt).toLocaleDateString(
                              undefined,
                              { month: "short", day: "numeric" },
                            )}
                          </span>
                          <ArrowUpRight size={14} />
                        </button>
                      ))}
                    </div>
                  </section>
                )}
              </div>
              <footer className="welcome-footer">
                <span>Made for the way you think.</span>
                <span>Local files. No ethos account.</span>
              </footer>
            </main>
          </>
        ) : (
          <>
            {sidebar && (
              <aside className="document-sidebar">
                <div className="document-sidebar-header">
                  <button
                    className="text-button back-library"
                    onClick={() => void closeDocument()}
                  >
                    <ChevronLeft size={14} />
                    Library
                  </button>
                  <IconButton
                    label="Open another PDF"
                    disabled={loading || saving}
                    onClick={() => void open()}
                  >
                    <FolderOpen size={16} />
                  </IconButton>
                </div>
                <div
                  className="sidebar-tabs"
                  role="tablist"
                  aria-label="Document navigation"
                >
                  {(
                    [
                      ["pages", LayoutGrid],
                      ["notes", List],
                      ["search", Search],
                    ] satisfies [typeof tab, typeof LayoutGrid][]
                  ).map(([name, Icon]) => (
                    <button
                      key={name}
                      role="tab"
                      id={`document-tab-${name}`}
                      aria-selected={tab === name}
                      aria-controls="document-navigation-panel"
                      tabIndex={tab === name ? 0 : -1}
                      className={tab === name ? "selected" : ""}
                      onKeyDown={(event) => {
                        if (
                          !["ArrowLeft", "ArrowRight", "Home", "End"].includes(
                            event.key,
                          )
                        )
                          return;
                        event.preventDefault();
                        const tabs: (typeof tab)[] = [
                          "pages",
                          "notes",
                          "search",
                        ];
                        const index = tabs.indexOf(name);
                        const next =
                          event.key === "Home"
                            ? 0
                            : event.key === "End"
                              ? 2
                              : (index + (event.key === "ArrowRight" ? 1 : 2)) %
                                tabs.length;
                        setTab(tabs[next]);
                        document
                          .getElementById(`document-tab-${tabs[next]}`)
                          ?.focus();
                      }}
                      onClick={() => {
                        setTab(name);
                        if (name === "search")
                          requestAnimationFrame(() =>
                            searchInput.current?.focus(),
                          );
                      }}
                    >
                      <Icon size={14} />
                      {name[0].toUpperCase() + name.slice(1)}
                      {name === "notes" && !!visibleMarks.length && (
                        <span>{visibleMarks.length}</span>
                      )}
                    </button>
                  ))}
                </div>
                <div
                  className={`sidebar-content ${tab === "pages" ? "thumbnail-grid" : ""}`}
                  role="tabpanel"
                  id="document-navigation-panel"
                  aria-labelledby={`document-tab-${tab}`}
                >
                  {tab === "pages" &&
                    edits.pages.map((entry, index) => (
                      <Thumbnail
                        key={entry.source}
                        pdf={doc.pdf}
                        source={entry.source}
                        rotation={entry.rotation}
                        index={index}
                        active={page === index + 1}
                        onClick={() => navigate(index + 1)}
                      />
                    ))}
                  {tab === "notes" && (
                    <div className="notes-list">
                      <div className="section-caption">
                        YOUR ANNOTATIONS <span>{visibleMarks.length}</span>
                      </div>
                      {visibleMarks.length ? (
                        visibleMarks.map((mark) => (
                          <div className="note-item" key={mark.id}>
                            <button
                              className="note-jump"
                              onClick={() => sourceNavigate(mark.page)}
                            >
                              <span
                                className="note-kind"
                                style={{ color: inkColors[mark.color] }}
                              >
                                {mark.kind === "highlight" ? (
                                  <Highlighter size={13} />
                                ) : mark.kind === "pen" ? (
                                  <PenLine size={13} />
                                ) : (
                                  <Type size={13} />
                                )}
                                <span>
                                  Page{" "}
                                  {edits.pages.findIndex(
                                    (entry) => entry.source === mark.page,
                                  ) + 1}
                                </span>
                              </span>
                              <p>
                                {mark.kind === "pen"
                                  ? "Handwritten mark"
                                  : mark.text}
                              </p>
                            </button>
                            <IconButton
                              label="Remove annotation"
                              onClick={() =>
                                commit((current) => ({
                                  ...current,
                                  marks: current.marks.filter(
                                    (item) => item.id !== mark.id,
                                  ),
                                }))
                              }
                            >
                              <X size={13} />
                            </IconButton>
                          </div>
                        ))
                      ) : (
                        <div className="sidebar-empty">
                          <Highlighter size={25} strokeWidth={1.3} />
                          <h3>Keep the good parts.</h3>
                          <p>
                            Highlights, drawings, and text notes will appear
                            here.
                          </p>
                        </div>
                      )}
                    </div>
                  )}
                  {tab === "search" && (
                    <div className="search-panel">
                      <div className="search-field">
                        <Search size={15} />
                        <Input
                          ref={searchInput}
                          aria-label="Search document"
                          placeholder="Find in document"
                          value={query}
                          onChange={(event) => setQuery(event.target.value)}
                        />
                        {query && (
                          <IconButton
                            label="Clear search"
                            onClick={() => setQuery("")}
                          >
                            <X size={13} />
                          </IconButton>
                        )}
                      </div>
                      {searching && (
                        <p className="search-status">
                          <LoaderCircle size={13} className="spin" />
                          Searching…
                        </p>
                      )}
                      {query.trim() && !searching && (
                        <p className="search-status">
                          {hits.reduce((sum, hit) => sum + hit.count, 0)}{" "}
                          matches in {hits.length} pages
                        </p>
                      )}
                      {hits.map((hit) => (
                        <button
                          className="search-result"
                          key={hit.page}
                          onClick={() => sourceNavigate(hit.page)}
                        >
                          <span>
                            Page{" "}
                            {edits.pages.findIndex(
                              (entry) => entry.source === hit.page,
                            ) + 1}
                            <small>
                              {hit.count}{" "}
                              {hit.count === 1 ? "match" : "matches"}
                            </small>
                          </span>
                          <p>{hit.snippet}</p>
                        </button>
                      ))}
                      {!query && (
                        <div className="sidebar-empty">
                          <Search size={25} strokeWidth={1.3} />
                          <h3>Find your place.</h3>
                          <p>Search the document’s selectable text.</p>
                        </div>
                      )}
                    </div>
                  )}
                </div>
                <div className="document-sidebar-footer">
                  <span>{edits.pages.length} pages</span>
                  <span>
                    {(doc.bytes.byteLength / 1024 / 1024).toFixed(1)} MB
                  </span>
                </div>
              </aside>
            )}
            <main className="reader">
              <div className="reader-toolbar">
                <div className="toolbar-start">
                  <IconButton
                    label="Toggle sidebar"
                    active={sidebar}
                    onClick={() => setSidebar(!sidebar)}
                  >
                    <PanelLeft size={18} />
                  </IconButton>
                  <span className="toolbar-divider" />
                  <div className="document-heading">
                    <span>{doc.name.replace(/\.pdf$/i, "")}</span>
                    <small>{dirty ? "Unsaved changes" : "PDF document"}</small>
                  </div>
                </div>
                <div className="toolbar-end">
                  <IconButton
                    label={
                      dark ? "Use light appearance" : "Use dark appearance"
                    }
                    onClick={() => setDark(!dark)}
                  >
                    {dark ? <Sun size={16} /> : <Moon size={16} />}
                  </IconButton>
                  <button
                    className="primary-button save-button"
                    disabled={saving || loading}
                    onClick={() => void save()}
                  >
                    {saving ? (
                      <LoaderCircle size={15} className="spin" />
                    ) : (
                      <ArrowDownToLine size={15} />
                    )}
                    <span>{saving ? "Saving…" : "Save copy"}</span>
                  </button>
                </div>
              </div>
              <div
                className="document-scroll"
                ref={scroll}
                onScroll={trackPage}
              >
                <div className="document-pages">
                  {edits.pages.map((entry, index) => (
                    <PdfPage
                      key={entry.source}
                      pdf={doc.pdf}
                      source={entry.source}
                      index={index}
                      rotation={entry.rotation}
                      width={pageWidth}
                      marks={marksByPage.get(entry.source) ?? noMarks}
                      tool={tool}
                      color={color}
                      query={query.trim()}
                      onMark={addMark}
                      onText={addText}
                    />
                  ))}
                  <div className="document-end">
                    <Check size={13} />
                    You’ve reached the end.
                  </div>
                </div>
              </div>
              <div
                className="floating-tools"
                role="toolbar"
                aria-label="Annotation tools"
              >
                <div className="tool-group">
                  {(
                    [
                      ["select", MousePointer2, "V"],
                      ["highlight", Highlighter, "H"],
                      ["pen", PenLine, "D"],
                      ["text", Type, "T"],
                    ] satisfies [Tool, typeof MousePointer2, string][]
                  ).map(([name, Icon, shortcut]) => (
                    <IconButton
                      key={name}
                      label={`${toolNames[name]} (${shortcut})`}
                      active={tool === name}
                      onClick={() => setTool(name)}
                    >
                      <Icon size={18} strokeWidth={1.7} />
                    </IconButton>
                  ))}
                </div>
                <span className="toolbar-divider" />
                <div className="ink-palette" aria-label="Annotation color">
                  {(
                    ["yellow", "mint", "blue", "rose"] satisfies InkColor[]
                  ).map((name) => (
                    <Hint key={name} label={`${name} ink`}>
                      <button
                        aria-label={`${name} ink`}
                        aria-pressed={color === name}
                        className={`ink-swatch ${color === name ? "selected" : ""}`}
                        style={{ background: inkColors[name] }}
                        onClick={() => setColor(name)}
                      >
                        {color === name && <Check size={11} />}
                      </button>
                    </Hint>
                  ))}
                </div>
                <span className="toolbar-divider" />
                <IconButton
                  label="Undo (Ctrl Z)"
                  disabled={!history.past.length}
                  onClick={undo}
                >
                  <Undo2 size={17} />
                </IconButton>
                <IconButton
                  label="Redo (Ctrl Shift Z)"
                  disabled={!history.future.length}
                  onClick={redo}
                >
                  <Redo2 size={17} />
                </IconButton>
              </div>
              <footer className="reader-status">
                <span className="tool-hint">{toolHints[tool]}</span>
                <div className="page-controls">
                  <IconButton
                    label="Previous page"
                    disabled={page <= 1}
                    onClick={() => navigate(page - 1)}
                  >
                    <ChevronLeft size={15} />
                  </IconButton>
                  <form
                    onSubmit={(event) => {
                      event.preventDefault();
                      const number = Number(pageInput);
                      if (
                        Number.isInteger(number) &&
                        number >= 1 &&
                        number <= edits.pages.length
                      )
                        navigate(number);
                      else setPageInput(String(page));
                    }}
                  >
                    <Input
                      aria-label="Page number"
                      value={pageInput}
                      inputMode="numeric"
                      onChange={(event) => setPageInput(event.target.value)}
                      onBlur={() => setPageInput(String(page))}
                    />
                    <span> / {edits.pages.length}</span>
                  </form>
                  <IconButton
                    label="Next page"
                    disabled={page >= edits.pages.length}
                    onClick={() => navigate(page + 1)}
                  >
                    <ChevronRight size={15} />
                  </IconButton>
                </div>
                <div className="view-controls">
                  <IconButton
                    label="Rotate current page"
                    onClick={() =>
                      commit((current) => ({
                        ...current,
                        pages: current.pages.map((entry, index) =>
                          index === page - 1
                            ? {
                                ...entry,
                                rotation: (entry.rotation + 90) % 360,
                              }
                            : entry,
                        ),
                      }))
                    }
                  >
                    <RotateCw size={15} />
                  </IconButton>
                  <IconButton
                    label="Delete current page"
                    disabled={edits.pages.length <= 1}
                    onClick={() => {
                      commit((current) => ({
                        ...current,
                        pages: current.pages.filter(
                          (_entry, index) => index !== page - 1,
                        ),
                      }));
                      setPage(Math.min(page, edits.pages.length - 1));
                      notify("Page removed. Undo to bring it back.");
                    }}
                  >
                    <Trash2 size={14} />
                  </IconButton>
                  <span className="toolbar-divider" />
                  <IconButton
                    label="Zoom out"
                    disabled={typeof zoom === "number" && zoom <= 0.5}
                    onClick={() =>
                      setZoom(
                        Math.max(
                          0.5,
                          (typeof zoom === "number" ? zoom : pageWidth / 612) -
                            0.15,
                        ),
                      )
                    }
                  >
                    <Minus size={15} />
                  </IconButton>
                  <Select
                    value={String(zoom)}
                    onValueChange={(value) =>
                      setZoom(value === "fit" ? "fit" : Number(value))
                    }
                  >
                    <SelectTrigger
                      aria-label="Zoom level"
                      className="zoom-trigger"
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent align="center">
                      <SelectItem value="fit">Fit width</SelectItem>
                      {[0.5, 0.75, 1, 1.25, 1.5, 2].map((value) => (
                        <SelectItem key={value} value={String(value)}>
                          {Math.round(value * 100)}%
                        </SelectItem>
                      ))}
                      {typeof zoom === "number" &&
                        ![0.5, 0.75, 1, 1.25, 1.5, 2].includes(zoom) && (
                          <SelectItem value={String(zoom)}>
                            {Math.round(zoom * 100)}%
                          </SelectItem>
                        )}
                    </SelectContent>
                  </Select>
                  <IconButton
                    label="Zoom in"
                    disabled={typeof zoom === "number" && zoom >= 2.5}
                    onClick={() =>
                      setZoom(
                        Math.min(
                          2.5,
                          (typeof zoom === "number" ? zoom : pageWidth / 612) +
                            0.15,
                        ),
                      )
                    }
                  >
                    <Plus size={15} />
                  </IconButton>
                </div>
              </footer>
            </main>
          </>
        )}
      </div>
      {toast && (
        <div
          className={`toast ${toast.error ? "error" : ""}`}
          role={toast.error ? "alert" : "status"}
        >
          {toast.error ? (
            <span className="error-indicator">!</span>
          ) : (
            <Check size={16} />
          )}
          <span>{toast.text}</span>
          <IconButton label="Dismiss message" onClick={() => setToast(null)}>
            <X size={14} />
          </IconButton>
        </div>
      )}
      {loading && !password && (
        <div className="loading-overlay" role="status">
          <div>
            <LoaderCircle size={24} className="spin" />
            <strong>Opening your document…</strong>
            <span>Getting everything in place.</span>
          </div>
        </div>
      )}
      {dragging && (
        <div className="drop-overlay">
          <div>
            <FilePlus2 size={38} strokeWidth={1.25} />
            <h2>Make room for a new idea.</h2>
            <p>Drop your PDF to open it in ethos.</p>
          </div>
        </div>
      )}
      {note && (
        <Modal title="Leave a thought" onClose={() => setNote(null)}>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void submitNote();
            }}
          >
            <p className="modal-description">
              Add a text note at the place you selected.
            </p>
            <Textarea
              className="note-input"
              aria-label="Text note"
              autoFocus
              placeholder="What’s on your mind?"
              value={note.text}
              maxLength={240}
              onChange={(event) => {
                setNoteError("");
                setNote({
                  ...note,
                  text: event.target.value.replace(/[\r\n]/g, " "),
                });
              }}
            />
            {noteError && (
              <p className="inline-error" role="alert">
                {noteError}
              </p>
            )}
            <div className="modal-actions">
              <span>{note.text.length}/240</span>
              <button
                type="button"
                className="subtle-button"
                onClick={() => setNote(null)}
              >
                Cancel
              </button>
              <button className="primary-button" disabled={!note.text.trim()}>
                Add text
              </button>
            </div>
          </form>
        </Modal>
      )}
      {password && (
        <Modal title="A little privacy" onClose={cancelLoad}>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              password.update(password.value);
              setPassword(null);
            }}
          >
            <p className="modal-description">
              {password.incorrect
                ? "That password didn’t work. Try again."
                : "This PDF is protected. Enter its password to read it."}
            </p>
            <Input
              className="password-input"
              type="password"
              autoFocus
              aria-label="PDF password"
              value={password.value}
              onChange={(event) =>
                setPassword({ ...password, value: event.target.value })
              }
            />
            <div className="modal-actions">
              <button className="primary-button" disabled={!password.value}>
                Open document
              </button>
            </div>
          </form>
        </Modal>
      )}
      {help && (
        <Modal title="A few useful shortcuts" onClose={() => setHelp(false)}>
          <div className="shortcuts">
            {[
              ["Open PDF", "Ctrl O"],
              ["Save a copy", "Ctrl S"],
              ["Search document", "Ctrl F"],
              ["Undo", "Ctrl Z"],
              ["Redo", "Ctrl Shift Z"],
              ["Select text", "V"],
              ["Highlight", "H"],
              ["Draw", "D"],
              ["Add text", "T"],
              ["Return to selection", "Esc"],
            ].map(([label, shortcut]) => (
              <div key={label}>
                <span>{label}</span>
                <kbd>{shortcut}</kbd>
              </div>
            ))}
          </div>
        </Modal>
      )}
      {fileBrowser === "open" && (
        <FileBrowser
          mode="open"
          onClose={() => setFileBrowser(null)}
          onOpen={async (path) => {
            const file = unwrap(await window.ethos.open(path));
            if (file) {
              setFileBrowser(null);
              await loadFile(file);
            }
          }}
        />
      )}
      {fileBrowser === "save" && doc && (
        <FileBrowser
          mode="save"
          name={doc.name}
          onClose={() => setFileBrowser(null)}
          onSave={saveTo}
        />
      )}
      {discardId && (
        <Modal
          title="Keep your changes?"
          onClose={() => {
            window.ethos.answerDiscard(discardId, false);
            setDiscardId(null);
          }}
        >
          <p className="modal-description">
            This document has unsaved changes. Save a copy before leaving to
            keep your annotations and page changes.
          </p>
          <div className="modal-actions">
            <button
              type="button"
              className="subtle-button danger-button"
              onClick={() => {
                window.ethos.answerDiscard(discardId, true);
                setDiscardId(null);
              }}
            >
              Discard changes
            </button>
            <button
              type="button"
              autoFocus
              className="primary-button"
              onClick={() => {
                window.ethos.answerDiscard(discardId, false);
                setDiscardId(null);
              }}
            >
              Keep editing
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
