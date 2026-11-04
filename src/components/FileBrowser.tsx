import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowUp,
  FileText,
  Folder,
  FolderOpen,
  HardDrive,
  Search,
  LoaderCircle,
} from "lucide-react";
import type { FileEntry, FileListing, SaveOutcome } from "../shared";
import { Modal } from "./Modal";
import { Input } from "./ui/input";
import { Hint } from "./ui/tooltip";

type Props = { onClose: () => void } & (
  | { mode: "open"; onOpen: (path: string) => Promise<void> }
  | {
      mode: "save";
      name: string;
      onSave: (path: string, overwrite: boolean) => Promise<SaveOutcome>;
    }
);

function sizeLabel(size: number | null) {
  if (size === null) return "Folder";
  return size < 1024 * 1024
    ? `${Math.max(1, Math.round(size / 1024))} KB`
    : `${(size / 1024 / 1024).toFixed(1)} MB`;
}

export function FileBrowser(props: Props) {
  const [listing, setListing] = useState<FileListing | null>(null);
  const [address, setAddress] = useState("");
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState("");
  const [name, setName] = useState(
    props.mode === "save"
      ? props.name.replace(/\.pdf$/i, "") + " — edited.pdf"
      : "",
  );
  const [back, setBack] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [replace, setReplace] = useState<string | null>(null);
  const sequence = useRef(0);
  const locationInput = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const rows =
    listing?.entries.filter((entry) =>
      entry.name.toLocaleLowerCase().includes(filter.toLocaleLowerCase()),
    ) ?? [];
  const entry = rows.find((entry) => entry.path === selected);

  async function navigate(directory?: string, remember = true) {
    const id = ++sequence.current;
    setLoading(true);
    setError("");
    setReplace(null);
    try {
      const result = await window.ethos.browse(directory);
      if (id !== sequence.current) return;
      if (!result.ok) throw new Error(result.error);
      if (remember && listing && listing.path !== result.value.path)
        setBack((old) => [...old, listing.path]);
      setListing(result.value);
      setAddress(result.value.path);
      setFilter("");
      setSelected("");
    } catch (reason) {
      if (id === sequence.current)
        setError(
          reason instanceof Error
            ? reason.message
            : "Could not read this folder.",
        );
    } finally {
      if (id === sequence.current) setLoading(false);
    }
  }

  useEffect(() => {
    void navigate();
    return () => {
      sequence.current++;
    };
  }, []);

  async function activate(item: FileEntry) {
    if (item.kind === "directory") {
      await navigate(item.path);
      return;
    }
    if (props.mode === "save") {
      setName(item.name);
      setReplace(null);
      return;
    }
    setBusy(true);
    setError("");
    try {
      await props.onOpen(item.path);
      props.onClose();
    } catch (reason) {
      setError(
        reason instanceof Error ? reason.message : "Could not open this PDF.",
      );
    } finally {
      setBusy(false);
    }
  }

  async function submit() {
    if (busy || loading) return;
    if (entry?.kind === "directory") {
      await navigate(entry.path);
      return;
    }
    if (props.mode === "open") {
      if (entry) await activate(entry);
      return;
    }
    if (!listing) return;
    if (
      !name.trim() ||
      /[\\/:*?"<>|\x00-\x1f]/.test(name) ||
      name.length > 250
    ) {
      setError("Enter a filename without slashes or reserved characters.");
      return;
    }
    const filename = /\.pdf$/i.test(name.trim())
      ? name.trim()
      : `${name.trim()}.pdf`;
    const destination = `${listing.path.replace(/[\\/]+$/, "")}/${filename}`;
    setBusy(true);
    setError("");
    try {
      const outcome = await props.onSave(destination, replace === destination);
      if (outcome === "exists") setReplace(destination);
      else props.onClose();
    } catch (reason) {
      setError(
        reason instanceof Error ? reason.message : "Could not save this PDF.",
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      title={props.mode === "open" ? "Open a PDF" : "Save a copy"}
      className="file-browser"
      onClose={() => {
        if (!busy) props.onClose();
      }}
    >
      <div
        className="browser-layout"
        onKeyDown={(event) => {
          if (
            (event.ctrlKey || event.metaKey) &&
            event.key.toLowerCase() === "l"
          ) {
            event.preventDefault();
            locationInput.current?.focus();
            locationInput.current?.select();
          }
        }}
      >
        <aside className="browser-sidebar" aria-label="File locations">
          <span className="eyebrow">Locations</span>
          {listing?.locations.map((location) => (
            <button
              key={location.name}
              type="button"
              disabled={busy}
              className={
                // Known folders can share a path (e.g. Documents redirected to D:\); mark only the first.
                location ===
                listing.locations.find((item) => item.path === listing.path)
                  ? "active"
                  : ""
              }
              onClick={() => void navigate(location.path)}
            >
              {location.name.endsWith(":") ? (
                <HardDrive size={16} />
              ) : (
                <FolderOpen size={16} />
              )}
              <span>{location.name}</span>
            </button>
          ))}
          <div className="browser-sidebar-note">
            <FileText size={17} />
            <span>
              PDF documents
              <br />
              <small>Browse files on your device</small>
            </span>
          </div>
        </aside>
        <div className="browser-main">
          <div className="browser-toolbar">
            <Hint label="Previous folder">
              <button
                type="button"
                aria-label="Previous folder"
                className="icon-button"
                disabled={!back.length || loading || busy}
                onClick={() => {
                  const previous = back.at(-1);
                  if (!previous) return;
                  setBack((old) => old.slice(0, -1));
                  void navigate(previous, false);
                }}
              >
                <ArrowLeft size={16} />
              </button>
            </Hint>
            <Hint label="Enclosing folder">
              <button
                type="button"
                aria-label="Enclosing folder"
                className="icon-button"
                disabled={!listing?.parent || loading || busy}
                onClick={() => {
                  if (listing?.parent) void navigate(listing.parent);
                }}
              >
                <ArrowUp size={16} />
              </button>
            </Hint>
            <form
              className="browser-address"
              onSubmit={(event) => {
                event.preventDefault();
                void navigate(address);
              }}
            >
              <Folder size={15} />
              <Input
                ref={locationInput}
                aria-label="Folder path"
                value={address}
                disabled={busy}
                onChange={(event) => setAddress(event.target.value)}
              />
            </form>
            <div className="browser-search">
              <Search size={15} />
              <Input
                aria-label="Filter files"
                placeholder="Filter files"
                value={filter}
                disabled={busy}
                onChange={(event) => {
                  setFilter(event.target.value);
                  setSelected("");
                }}
              />
            </div>
          </div>
          <div className="browser-columns" aria-hidden="true">
            <span>Name</span>
            <span>Modified</span>
            <span>Size</span>
          </div>
          <div
            className="browser-files"
            ref={list}
            role="listbox"
            aria-label="Files and folders"
            aria-busy={loading}
            tabIndex={0}
            aria-activedescendant={
              selected
                ? `browser-row-${rows.findIndex((row) => row.path === selected)}`
                : undefined
            }
            onKeyDown={(event) => {
              if (busy || loading) return;
              const index = rows.findIndex((row) => row.path === selected);
              if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
                event.preventDefault();
                const next =
                  event.key === "Home"
                    ? 0
                    : event.key === "End"
                      ? rows.length - 1
                      : event.key === "ArrowDown"
                        ? Math.min(rows.length - 1, index + 1)
                        : Math.max(0, index - 1);
                const item = rows[next];
                if (item) {
                  setSelected(item.path);
                  list.current
                    ?.querySelector(`#browser-row-${next}`)
                    ?.scrollIntoView({ block: "nearest" });
                }
              }
              if (event.key === "Enter" && entry) {
                event.preventDefault();
                void activate(entry);
              }
            }}
          >
            {loading ? (
              <div className="browser-empty" role="status">
                <LoaderCircle className="spin" size={22} />
                <p>Reading folder…</p>
              </div>
            ) : rows.length ? (
              rows.map((item, index) => (
                <div
                  id={`browser-row-${index}`}
                  role="option"
                  aria-selected={selected === item.path}
                  key={item.path}
                  className="browser-file"
                  onClick={() => {
                    if (busy) return;
                    setSelected(item.path);
                    list.current?.focus();
                    if (props.mode === "save" && item.kind === "pdf")
                      setName(item.name);
                    setReplace(null);
                  }}
                  onDoubleClick={() => {
                    if (!busy) void activate(item);
                  }}
                >
                  <span className="browser-file-name">
                    {item.kind === "directory" ? (
                      <Folder size={19} />
                    ) : (
                      <FileText size={19} />
                    )}
                    <span>{item.name}</span>
                  </span>
                  <span>
                    {item.modified
                      ? new Intl.DateTimeFormat(undefined, {
                          month: "short",
                          day: "numeric",
                          year: "numeric",
                        }).format(item.modified)
                      : "—"}
                  </span>
                  <span>{sizeLabel(item.size)}</span>
                </div>
              ))
            ) : (
              <div className="browser-empty">
                <FolderOpen size={29} />
                <strong>
                  {filter ? "No matching files" : "No PDF documents here"}
                </strong>
                <p>
                  {filter
                    ? "Try a different name."
                    : "Choose another folder from the sidebar or enter its path."}
                </p>
              </div>
            )}
          </div>
          <div className="browser-status">
            {!loading &&
              `${rows.length} ${rows.length === 1 ? "item" : "items"}`}
            <span>Folders and PDF documents</span>
          </div>
        </div>
      </div>
      <div className="browser-footer">
        {props.mode === "save" && (
          <label className="browser-filename">
            <span>Save as</span>
            <Input
              aria-label="Save as"
              autoComplete="off"
              value={name}
              disabled={busy}
              onChange={(event) => {
                setName(event.target.value);
                setSelected("");
                setReplace(null);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") void submit();
              }}
            />
          </label>
        )}
        {error && (
          <p className="inline-error" role="alert">
            {error}
          </p>
        )}
        {replace && (
          <p className="browser-replace" role="alert">
            A file with this name already exists. Replace it with this copy?
          </p>
        )}
        <div className="browser-actions">
          <span>
            {props.mode === "open"
              ? "Your files stay on this device."
              : "Your original PDF is protected."}
          </span>
          <button
            type="button"
            className="subtle-button"
            disabled={busy}
            onClick={props.onClose}
          >
            Cancel
          </button>
          <button
            type="button"
            className="primary-button"
            disabled={
              busy || loading || (props.mode === "open" ? !entry : !name.trim())
            }
            onClick={() => void submit()}
          >
            {busy ? <LoaderCircle size={15} className="spin" /> : null}
            {entry?.kind === "directory"
              ? "Open folder"
              : props.mode === "open"
                ? "Open PDF"
                : replace
                  ? "Replace file"
                  : "Save copy"}
          </button>
        </div>
      </div>
    </Modal>
  );
}
