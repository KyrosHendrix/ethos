import { contextBridge, ipcRenderer, webUtils } from "electron";
import type { Desktop, OpenedFile } from "../src/shared";

const desktop: Desktop = {
  open: (path) => ipcRenderer.invoke("file:open", path),
  browse: (path) => ipcRenderer.invoke("file:browse", path),
  openDropped: (file) =>
    ipcRenderer.invoke("file:drop", webUtils.getPathForFile(file)),
  recent: () => ipcRenderer.invoke("file:recent"),
  openRecent: (id) => ipcRenderer.invoke("file:open-recent", id),
  save: (bytes, path, overwrite) =>
    ipcRenderer.invoke("file:save", bytes, path, overwrite),
  confirmDiscard: () => ipcRenderer.invoke("document:discard"),
  setDirty: (dirty) => ipcRenderer.send("document:dirty", dirty),
  windowAction: (action) => ipcRenderer.send("window:action", action),
  onFile: (callback) => {
    const listener = (_event: Electron.IpcRendererEvent, file: OpenedFile) =>
      callback(file);
    ipcRenderer.on("file:opened", listener);
    return () => ipcRenderer.removeListener("file:opened", listener);
  },
  ready: () => ipcRenderer.send("app:ready"),
  onDiscardRequest: (callback) => {
    const listener = (_event: Electron.IpcRendererEvent, id: string) =>
      callback(id);
    ipcRenderer.on("document:confirm-discard", listener);
    return () =>
      ipcRenderer.removeListener("document:confirm-discard", listener);
  },
  answerDiscard: (id, discard) =>
    ipcRenderer.send("document:discard-response", id, discard),
  onFileError: (callback) => {
    const listener = (_event: Electron.IpcRendererEvent, error: string) =>
      callback(error);
    ipcRenderer.on("file:error", listener);
    return () => ipcRenderer.removeListener("file:error", listener);
  },
};
contextBridge.exposeInMainWorld("ethos", desktop);
