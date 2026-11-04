import { app, BrowserWindow, ipcMain } from "electron";
import {
  readFile,
  writeFile,
  rename,
  unlink,
  stat,
  mkdir,
  realpath,
} from "node:fs/promises";
import path from "node:path";
import { randomUUID } from "node:crypto";
import type {
  FileLocation,
  OpenedFile,
  RecentFile,
  Result,
  SaveOutcome,
} from "../src/shared";
import { browseDirectory } from "./files";

if (process.env.ETHOS_USER_DATA)
  app.setPath("userData", process.env.ETHOS_USER_DATA);
let window: BrowserWindow;
let dirty = false;
// Keep every source protected even if the renderer rejects a newly read PDF.
const sourcePaths = new Map<string, string>();
let rendererReady = false;
let fileOperation = false;
let quitting = false;

function trusted(event: Electron.IpcMainEvent | Electron.IpcMainInvokeEvent) {
  return (
    window &&
    !window.isDestroyed() &&
    event.sender === window.webContents &&
    event.senderFrame === window.webContents.mainFrame
  );
}

async function exclusiveFile<T>(work: () => Promise<T>): Promise<T> {
  if (fileOperation) throw new Error("Another file operation is in progress.");
  fileOperation = true;
  try {
    return await work();
  } finally {
    fileOperation = false;
  }
}
let pendingPath: string | undefined;
let recent: (RecentFile & { path: string })[] = [];
const maxFileSize = 200 * 1024 * 1024;

async function readPdf(filePath: unknown): Promise<OpenedFile> {
  if (
    typeof filePath !== "string" ||
    path.extname(filePath).toLowerCase() !== ".pdf"
  )
    throw new Error("Choose a PDF file.");
  const info = await stat(filePath);
  if (!info.isFile() || info.size > maxFileSize)
    throw new Error("Choose a PDF smaller than 200 MB.");
  const bytes = await readFile(filePath);
  if (bytes.length > maxFileSize)
    throw new Error("Choose a PDF smaller than 200 MB.");
  if (!bytes.subarray(0, 1024).includes(Buffer.from("%PDF-")))
    throw new Error("This file does not contain a valid PDF header.");
  const sourcePath = await realpath(filePath);
  sourcePaths.set(sourcePath.toLowerCase(), sourcePath);
  const previous = recent.find((item) => item.path === sourcePath);
  recent = [
    {
      id: previous?.id ?? randomUUID(),
      name: path.basename(filePath),
      path: sourcePath,
      openedAt: Date.now(),
    },
    ...recent.filter((item) => item.path !== sourcePath),
  ].slice(0, 8);
  await writeRecents().catch(() => {});
  return { name: path.basename(filePath), bytes };
}

async function writeRecents() {
  const directory = app.getPath("userData");
  await mkdir(directory, { recursive: true });
  const target = path.join(directory, "recent.json");
  const temp = `${target}.tmp`;
  await writeFile(temp, JSON.stringify(recent));
  await rename(temp, target);
}

let discardRequest: { id: string; resolve: (discard: boolean) => void } | null =
  null;
async function confirmDiscard() {
  if (!dirty) return true;
  if (discardRequest || !rendererReady || window.isDestroyed()) return false;
  return new Promise<boolean>((resolve) => {
    const id = randomUUID();
    discardRequest = { id, resolve };
    window.webContents.send("document:confirm-discard", id);
  });
}

function handle<T extends unknown[], R>(
  channel: string,
  callback: (...args: T) => Promise<R> | R,
) {
  ipcMain.handle(channel, async (event, ...args: T): Promise<Result<R>> => {
    try {
      if (!trusted(event)) throw new Error("Request denied.");
      return { ok: true, value: await callback(...args) };
    } catch (error) {
      return {
        ok: false,
        error: error instanceof Error ? error.message : "Something went wrong.",
      };
    }
  });
}

async function openExternalFile(filePath: string) {
  try {
    await exclusiveFile(async () => {
      if (!(await confirmDiscard())) return;
      window.webContents.send("file:opened", await readPdf(filePath));
    });
  } catch (error) {
    window.webContents.send(
      "file:error",
      error instanceof Error ? error.message : "Could not open this PDF.",
    );
  }
}

app.on("open-file", (event, filePath) => {
  event.preventDefault();
  if (window && !window.isDestroyed() && rendererReady)
    void openExternalFile(filePath);
  else pendingPath = filePath;
});

if (!app.requestSingleInstanceLock()) app.quit();
else {
  app.on("second-instance", (_event, argv) => {
    if (!window || window.isDestroyed()) return;
    if (window.isMinimized()) window.restore();
    window.focus();
    const filePath = argv.find((arg) => arg.toLowerCase().endsWith(".pdf"));
    if (filePath) {
      if (rendererReady) void openExternalFile(filePath);
      else pendingPath = filePath;
    }
  });

  void app.whenReady().then(async () => {
    if (process.platform === "win32") app.setAppUserModelId("app.ethos.reader");
    try {
      const stored: unknown = JSON.parse(
        await readFile(
          path.join(app.getPath("userData"), "recent.json"),
          "utf8",
        ),
      );
      if (Array.isArray(stored))
        recent = stored
          .filter(
            (item): item is (typeof recent)[number] =>
              typeof item === "object" &&
              item !== null &&
              typeof item.id === "string" &&
              typeof item.name === "string" &&
              typeof item.path === "string" &&
              typeof item.openedAt === "number",
          )
          .slice(0, 8);
    } catch {
      /* A missing or damaged recent list starts empty. */
    }

    window = new BrowserWindow({
      width: 1440,
      height: 960,
      minWidth: 960,
      minHeight: 680,
      title: "ethos",
      backgroundColor: "#f5f4f1",
      titleBarStyle: "hidden",
      icon: process.env.ELECTRON_RENDERER_URL
        ? path.join(app.getAppPath(), "build/icon.ico")
        : path.join(import.meta.dirname, "../renderer/ethos.png"),
      trafficLightPosition: { x: 18, y: 18 },
      webPreferences: {
        preload: path.join(import.meta.dirname, "../preload/index.cjs"),
        sandbox: true,
        contextIsolation: true,
        nodeIntegration: false,
      },
    });
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (event) => event.preventDefault());
    window.on("close", (event) => {
      if (fileOperation || dirty) {
        event.preventDefault();
        if (!fileOperation)
          void confirmDiscard().then((discard) => {
            if (discard && !window.isDestroyed()) {
              dirty = false;
              window.close();
            }
          });
      }
    });
    window.webContents.on("did-start-loading", () => {
      discardRequest?.resolve(false);
      discardRequest = null;
    });
    window.on("closed", () => {
      discardRequest?.resolve(false);
      discardRequest = null;
    });
    window.setMenuBarVisibility(false);

    const locations: FileLocation[] = [
      { name: "Documents", path: app.getPath("documents") },
      { name: "Downloads", path: app.getPath("downloads") },
      { name: "Desktop", path: app.getPath("desktop") },
      { name: "Home", path: app.getPath("home") },
    ];
    if (process.platform === "win32") {
      const drives = await Promise.all(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("").map(async (letter) => {
          const drive = `${letter}:\\`;
          return (await stat(drive).catch(() => null))?.isDirectory()
            ? { name: `${letter}:`, path: drive }
            : null;
        }),
      );
      locations.push(...drives.filter((drive) => drive !== null));
    } else locations.push({ name: "Computer", path: "/" });
    handle("file:browse", (directory: unknown) =>
      browseDirectory(directory, locations),
    );
    handle("file:open", (filePath: unknown) =>
      exclusiveFile(async () => {
        if (!(await confirmDiscard())) return null;
        return readPdf(filePath);
      }),
    );
    handle("file:drop", (filePath: unknown) =>
      exclusiveFile(async () => {
        if (!(await confirmDiscard())) return null;
        return readPdf(filePath);
      }),
    );
    handle("file:recent", () =>
      recent.map(({ id, name, openedAt }) => ({ id, name, openedAt })),
    );
    handle("file:open-recent", (id: unknown) =>
      exclusiveFile(async () => {
        if (typeof id !== "string") throw new Error("Invalid recent document.");
        if (!(await confirmDiscard())) return null;
        const entry = recent.find((item) => item.id === id);
        if (!entry)
          throw new Error("This document is no longer in your recent list.");
        return readPdf(entry.path);
      }),
    );
    handle("document:discard", confirmDiscard);
    handle(
      "file:save",
      (bytes: unknown, filePath: unknown, overwrite: unknown) =>
        exclusiveFile(async (): Promise<SaveOutcome> => {
          if (
            !(bytes instanceof Uint8Array) ||
            bytes.byteLength > maxFileSize * 2 ||
            Buffer.from(bytes.subarray(0, 5)).toString() !== "%PDF-"
          )
            throw new Error("Invalid PDF export.");
          if (
            typeof filePath !== "string" ||
            !path.isAbsolute(filePath) ||
            path.extname(filePath).toLowerCase() !== ".pdf" ||
            typeof overwrite !== "boolean"
          )
            throw new Error("Choose an absolute PDF destination.");
          const destination = path.resolve(filePath);
          const canonical = await realpath(destination).catch(
            () => destination,
          );
          const targetInfo = await stat(destination).catch(() => null);
          if (targetInfo && !targetInfo.isFile())
            throw new Error(
              "A folder uses this name. Choose a different filename.",
            );
          for (const [original, originalPath] of sourcePaths) {
            const originalInfo = targetInfo
              ? await stat(originalPath).catch(() => null)
              : null;
            if (
              canonical.toLowerCase() === original ||
              (targetInfo &&
                originalInfo &&
                targetInfo.dev === originalInfo.dev &&
                targetInfo.ino === originalInfo.ino)
            ) {
              throw new Error(
                "Choose a different filename to preserve your original PDF.",
              );
            }
          }
          if (targetInfo && !overwrite) return "exists";
          const temp = `${destination}.${randomUUID()}.tmp`;
          try {
            await writeFile(temp, bytes, { flag: "wx" });
            await rename(temp, destination);
          } catch (error) {
            await unlink(temp).catch(() => {});
            throw error;
          }
          return "saved";
        }),
    );
    ipcMain.on(
      "document:discard-response",
      (event, id: unknown, discard: unknown) => {
        if (
          !trusted(event) ||
          id !== discardRequest?.id ||
          typeof discard !== "boolean"
        )
          return;
        const request = discardRequest;
        discardRequest = null;
        request?.resolve(discard);
      },
    );
    ipcMain.on("document:dirty", (event, value: unknown) => {
      if (trusted(event) && typeof value === "boolean") dirty = value;
    });
    ipcMain.on("window:action", (event, action: unknown) => {
      if (!trusted(event)) return;
      if (action === "close") window.close();
      if (action === "minimize") window.minimize();
      if (action === "maximize")
        window.isMaximized() ? window.unmaximize() : window.maximize();
    });
    ipcMain.on("app:ready", (event) => {
      if (!trusted(event) || rendererReady) return;
      rendererReady = true;
      const filePath =
        pendingPath ??
        process.argv.find((arg) => arg.toLowerCase().endsWith(".pdf"));
      pendingPath = undefined;
      if (filePath) void openExternalFile(filePath);
    });
    if (process.env.ELECTRON_RENDERER_URL)
      await window.loadURL(process.env.ELECTRON_RENDERER_URL);
    else
      await window.loadFile(
        path.join(import.meta.dirname, "../renderer/index.html"),
      );
  });
}
app.on("before-quit", (event) => {
  if (quitting) return;
  event.preventDefault();
  if (fileOperation) return;
  void (async () => {
    if (window && !window.isDestroyed() && !(await confirmDiscard())) return;
    dirty = false;
    quitting = true;
    app.quit();
  })();
});
app.on("window-all-closed", () => {
  app.quit();
});
