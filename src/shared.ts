export type OpenedFile = { name: string; bytes: Uint8Array };
export type RecentFile = { id: string; name: string; openedAt: number };
export type FileLocation = { name: string; path: string };
export type FileEntry = {
  name: string;
  path: string;
  kind: "directory" | "pdf";
  size: number | null;
  modified: number | null;
};
export type FileListing = {
  path: string;
  parent: string | null;
  entries: FileEntry[];
  locations: FileLocation[];
};
export type SaveOutcome = "saved" | "exists";
export type Result<T> = { ok: true; value: T } | { ok: false; error: string };

export type Desktop = {
  open: (path: string) => Promise<Result<OpenedFile | null>>;
  browse: (path?: string) => Promise<Result<FileListing>>;
  openDropped: (file: File) => Promise<Result<OpenedFile | null>>;
  recent: () => Promise<Result<RecentFile[]>>;
  openRecent: (id: string) => Promise<Result<OpenedFile | null>>;
  save: (
    bytes: Uint8Array,
    path: string,
    overwrite: boolean,
  ) => Promise<Result<SaveOutcome>>;
  confirmDiscard: () => Promise<Result<boolean>>;
  setDirty: (dirty: boolean) => void;
  windowAction: (action: "minimize" | "maximize" | "close") => void;
  onFile: (callback: (file: OpenedFile) => void) => () => void;
  onDiscardRequest: (callback: (id: string) => void) => () => void;
  answerDiscard: (id: string, discard: boolean) => void;
  onFileError: (callback: (error: string) => void) => () => void;
  ready: () => void;
};

declare global {
  interface Window {
    ethos: Desktop;
  }
}
