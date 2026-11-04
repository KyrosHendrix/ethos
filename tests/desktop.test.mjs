import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";

const electron = `
export const contextBridge = {
  exposeInMainWorld(name, api) { globalThis.desktopBridgeTest = { name, api }; }
};
export const ipcRenderer = {};
export const webUtils = {};
`;
registerHooks({
  resolve(specifier, context, nextResolve) {
    return specifier === "electron"
      ? {
          url: `data:text/javascript,${encodeURIComponent(electron)}`,
          shortCircuit: true,
        }
      : nextResolve(specifier, context);
  },
});
await import("../electron/preload.ts");

test("desktop bridge exposes only document, file, and window operations", () => {
  const { name, api } = globalThis.desktopBridgeTest;
  assert.equal(name, "ethos");
  assert.deepEqual(Object.keys(api).sort(), [
    "answerDiscard",
    "browse",
    "confirmDiscard",
    "onDiscardRequest",
    "onFile",
    "onFileError",
    "open",
    "openDropped",
    "openRecent",
    "ready",
    "recent",
    "save",
    "setDirty",
    "windowAction",
  ]);
});
