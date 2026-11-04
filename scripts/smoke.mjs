import assert from "node:assert/strict";
import {
  access,
  mkdtemp,
  mkdir,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import electron from "electron";
import { _electron } from "playwright-core";
import { PDFDocument } from "pdf-lib";
import { createSample } from "../src/pdf.ts";

// Accept an isolated build entry without rebuilding the app here.
const entry = path.resolve(process.argv[2] || "out/main/index.js");
await access(entry);
if (!process.env.ELECTRON_RENDERER_URL)
  await access(path.join(path.dirname(entry), "../renderer/index.html"));
const temporary = await mkdtemp(path.join(tmpdir(), "ethos-smoke-"));
const artifacts = path.resolve("artifacts");
await mkdir(artifacts, { recursive: true });
const fixture = path.join(temporary, "Smoke fixture.pdf");
const output = path.join(temporary, "export.pdf");
const original = await createSample();
await writeFile(fixture, original);
await mkdir(path.join(temporary, "Nested folder"));
await writeFile(
  path.join(temporary, "not-a-pdf.txt"),
  "hidden by PDF filtering",
);
const env = {
  ...process.env,
  ETHOS_USER_DATA: path.join(temporary, "profile"),
};
delete env.ELECTRON_RUN_AS_NODE;
let app;
let page;
let count = 0;
const errors = [];
async function check(name, run) {
  await run();
  count++;
  console.log(`PASS ${name}`);
}
const button = (name) => page.getByRole("button", { name, exact: true });
async function visible(locator) {
  await locator.waitFor({ state: "visible", timeout: 20000 });
}
async function choose(name, option) {
  await page.getByRole("combobox", { name, exact: true }).click();
  await page.getByRole("option", { name: option, exact: true }).click();
}
async function browseTo(directory) {
  await visible(page.locator('.browser-files[aria-busy="false"]'));
  const address = page.getByRole("textbox", {
    name: "Folder path",
    exact: true,
  });
  await address.fill(directory);
  await address.press("Enter");
  await visible(page.locator('.browser-files[aria-busy="false"]'));
}
async function value(locator, expected) {
  await page.waitForFunction(
    ({ selector, expected }) =>
      document.querySelector(selector)?.value === expected,
    {
      selector: await locator.evaluate(
        (el) => `[aria-label="${el.getAttribute("aria-label")}"]`,
      ),
      expected,
    },
  );
}
async function noteCount(expected) {
  await page.waitForFunction(
    (expected) => document.querySelectorAll(".note-item").length === expected,
    expected,
  );
}
async function rendered() {
  await page.waitForFunction(
    () =>
      [...document.querySelectorAll(".page-canvas")].some(
        (canvas) => canvas.width > 100,
      ) && document.querySelector(".textLayer")?.textContent.length > 30,
  );
}
try {
  app = await _electron.launch({
    executablePath: electron,
    args: [entry],
    env,
    timeout: 30000,
  });
  page = await app.firstWindow();
  page.setDefaultTimeout(15000);
  page.on("pageerror", (error) => errors.push(error.stack || error.message));
  // Browsing, opening and saving use real files and IPC.
  await app.evaluate(({ dialog }) => {
    for (const method of [
      "showMessageBoxSync",
      "showMessageBox",
      "showOpenDialog",
      "showSaveDialog",
    ])
      dialog[method] = () => {
        throw new Error(`Native dialog called: ${method}`);
      };
  });
  await check("welcome and screenshot", async () => {
    await visible(page.getByText(/take a look around with a sample document/));
    await page.screenshot({ path: path.join(artifacts, "welcome.png") });
  });
  await check(
    "sample renders real canvas and selectable PDF text",
    async () => {
      await page.getByText(/take a look around with a sample document/).click();
      await rendered();
      const selection = await page
        .locator(".textLayer")
        .first()
        .evaluate((el) => {
          const range = document.createRange();
          range.selectNodeContents(el);
          const selection = window.getSelection();
          selection.removeAllRanges();
          selection.addRange(range);
          const result = selection.toString();
          selection.removeAllRanges();
          return result;
        });
      assert.match(selection, /ethos/);
      await page.screenshot({ path: path.join(artifacts, "reader.png") });
    },
  );
  await check("search shortcut finds actual PDF text", async () => {
    await page.keyboard.press("Control+f");
    await page
      .getByRole("textbox", { name: "Search document" })
      .fill("attention");
    await visible(page.locator(".search-result").first());
  });
  await check("page jump and zoom controls", async () => {
    await page.getByLabel("Page number", { exact: true }).fill("3");
    await page.getByLabel("Page number", { exact: true }).press("Enter");
    await value(page.getByLabel("Page number", { exact: true }), "3");
    await choose("Zoom level", "100%");
    assert.equal(
      await page.getByLabel("Zoom level", { exact: true }).innerText(),
      "100%",
    );
    await button("Zoom in").click();
    assert.ok(
      Number(
        (
          await page.getByLabel("Zoom level", { exact: true }).innerText()
        ).replace("%", ""),
      ) > 100,
    );
    await choose("Zoom level", "Fit width");
    await page.getByRole("tab", { name: "Pages", exact: true }).click();
    await page.getByLabel("Page number", { exact: true }).fill("1");
    await page.getByLabel("Page number", { exact: true }).press("Enter");
  });
  await check(
    "highlight selectable text with the annotation tool",
    async () => {
      await button("Highlight (H)").click();
      const passage = page
        .locator(".pdf-page")
        .first()
        .locator(".textLayer span")
        .filter({ hasText: "The best tools give you space." })
        .first();
      await visible(passage);
      await passage.evaluate((element) => {
        const range = document.createRange();
        range.selectNodeContents(element);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        element.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
      });
      await page.waitForFunction(
        () => document.querySelectorAll(".annotation-layer rect").length > 0,
      );
      await button("Undo (Ctrl Z)").click();
      assert.equal(await page.locator(".annotation-layer rect").count(), 0);
    },
  );
  await check("draw a stroke with pointer capture and undo it", async () => {
    await button("Draw (D)").click();
    const bounds = await page.locator(".pdf-page").first().boundingBox();
    assert.ok(bounds);
    await page.mouse.move(bounds.x + 100, bounds.y + 180);
    await page.mouse.down();
    await page.mouse.move(bounds.x + 130, bounds.y + 205, { steps: 5 });
    await page.mouse.move(bounds.x + 170, bounds.y + 180, { steps: 5 });
    await page.mouse.up();
    assert.equal(await page.locator(".annotation-layer polyline").count(), 1);
    await button("Undo (Ctrl Z)").click();
    assert.equal(await page.locator(".annotation-layer polyline").count(), 0);
  });
  await check("text note via page click and modal", async () => {
    await page.locator(".document-heading").click();
    await page.keyboard.press("t");
    await page
      .locator(".pdf-page")
      .first()
      .click({ position: { x: 150, y: 220 } });
    await page
      .getByLabel("Text note", { exact: true })
      .fill("Smoke annotation");
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Add text", exact: true })
      .click();
    await page.getByRole("tab", { name: /^Notes/ }).click();
    await noteCount(1);
    assert.match(
      await page.locator(".note-item").innerText(),
      /Smoke annotation/,
    );
  });
  await check("undo redo shortcuts and note list delete", async () => {
    await page.locator(".document-heading").click();
    await page.keyboard.press("Control+z");
    await noteCount(0);
    await page.keyboard.press("Control+Shift+z");
    await noteCount(1);
    await button("Remove annotation").click();
    await noteCount(0);
    await button("Undo (Ctrl Z)").click();
    await noteCount(1);
  });
  await check("dirty close keeps edits by default", async () => {
    await button("Library").click();
    await visible(page.getByRole("dialog", { name: "Keep your changes?" }));
    await page.keyboard.press("Escape");
    await noteCount(1);
    await app.evaluate(({ BrowserWindow }) =>
      BrowserWindow.getAllWindows()[0].close(),
    );
    assert.equal(page.isClosed(), false);
    await visible(page.getByRole("dialog", { name: "Keep your changes?" }));
    await button("Keep editing").click();
    await noteCount(1);
    assert.equal(await page.getByRole("dialog").count(), 0);
  });
  await check("page rotate/delete and undo", async () => {
    await page.getByRole("tab", { name: "Pages", exact: true }).click();
    await button("Rotate current page").click();
    await button("Undo (Ctrl Z)").click();
    await button("Redo (Ctrl Shift Z)").click();
    await button("Delete current page").click();
    assert.equal(await page.locator(".thumbnail").count(), 3);
    await button("Undo (Ctrl Z)").click();
    assert.equal(await page.locator(".thumbnail").count(), 4);
  });
  await check("reader exposes only document controls", async () => {
    assert.equal(await page.getByText(/Copilot/i).count(), 0);
    assert.equal(
      await page
        .getByRole("textbox", { name: "Ask about your document" })
        .count(),
      0,
    );
    assert.deepEqual(
      await page.evaluate(() => Object.keys(window.ethos).sort()),
      [
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
      ],
    );
  });
  await check(
    "in-app save browser writes a readable annotated rotated PDF",
    async () => {
      await page.keyboard.press("Control+s");
      await visible(page.getByRole("dialog", { name: "Save a copy" }));
      await browseTo(temporary);
      await page.screenshot({
        path: path.join(artifacts, "file-browser-light.png"),
      });
      await page
        .getByRole("textbox", { name: "Save as", exact: true })
        .fill("export.pdf");
      await page
        .getByRole("dialog", { name: "Save a copy" })
        .getByRole("button", { name: "Save copy", exact: true })
        .click();
      await visible(page.getByText("Copy saved. Your original is untouched."));
      const pdf = await PDFDocument.load(await readFile(output));
      assert.equal(pdf.getPageCount(), 4);
      assert.equal(pdf.getPage(0).getRotation().angle, 90);
      assert.equal(pdf.getTitle(), "A little more room to think");
      const { getDocument } = await import("pdfjs-dist/legacy/build/pdf.mjs");
      const task = getDocument({
        data: Uint8Array.from(await readFile(output)),
        useSystemFonts: true,
      });
      try {
        const doc = await task.promise;
        const text = await (await doc.getPage(1)).getTextContent();
        assert.match(
          text.items.map((i) => i.str ?? "").join(" "),
          /Smoke annotation/,
        );
      } finally {
        await task.destroy();
      }
    },
  );
  await check(
    "open fixture through in-app browser shortcut and reopen recent",
    async () => {
      await page.keyboard.press("Control+o");
      await visible(
        page.getByRole("dialog", { name: "Open a PDF", exact: true }),
      );
      await browseTo(temporary);
      await page.getByRole("option", { name: /Smoke fixture.pdf/ }).dblclick();
      await visible(
        page
          .locator(".document-heading")
          .getByText("Smoke fixture", { exact: true }),
      );
      await rendered();
      assert.deepEqual(await readFile(fixture), Buffer.from(original));
      await button("Library").click();
      await page
        .getByRole("button", { name: "Smoke fixture.pdf", exact: true })
        .click();
      await rendered();
    },
  );
  await check(
    "file browser filters, handles invalid paths and supports keyboard navigation",
    async () => {
      await page.keyboard.press("Control+o");
      await visible(
        page.getByRole("dialog", { name: "Open a PDF", exact: true }),
      );
      await browseTo(temporary);
      assert.equal(
        await page.getByRole("option", { name: /not-a-pdf.txt/ }).count(),
        0,
      );
      await page
        .getByRole("textbox", { name: "Filter files", exact: true })
        .fill("no-match-4729");
      await visible(page.getByText("No matching files", { exact: true }));
      await browseTo(path.join(temporary, "missing-folder"));
      await visible(
        page
          .getByRole("alert")
          .filter({ hasText: "This folder isn't available" }),
      );
      await browseTo(temporary);
      const files = page.getByRole("listbox", {
        name: "Files and folders",
        exact: true,
      });
      await files.focus();
      await files.press("Home");
      await files.press("Enter");
      await visible(page.getByText("No PDF documents here", { exact: true }));
      await button("Enclosing folder").click();
      await visible(page.getByRole("option", { name: /Smoke fixture.pdf/ }));
      await button("Previous folder").click();
      await visible(page.getByText("No PDF documents here", { exact: true }));
      await page.keyboard.press("Escape");
      assert.equal(await page.getByRole("dialog").count(), 0);
    },
  );
  await check(
    "save browser protects source files and confirms replacement",
    async () => {
      await page.keyboard.press("Control+s");
      await visible(
        page.getByRole("dialog", { name: "Save a copy", exact: true }),
      );
      await browseTo(temporary);
      const filename = page.getByRole("textbox", {
        name: "Save as",
        exact: true,
      });
      const saveButton = page
        .getByRole("dialog", { name: "Save a copy", exact: true })
        .getByRole("button", { name: "Save copy", exact: true });
      await page.getByRole("option", { name: /Nested folder/ }).click();
      await filename.fill("Smoke fixture.pdf");
      await saveButton.click();
      await visible(
        page.getByRole("alert").filter({ hasText: "preserve your original" }),
      );
      assert.deepEqual(await readFile(fixture), Buffer.from(original));
      await filename.fill("export.pdf");
      const before = await readFile(output);
      await saveButton.click();
      await visible(page.getByText(/A file with this name already exists/));
      assert.deepEqual(await readFile(output), before);
      await button("Replace file").click();
      await visible(page.getByText("Copy saved. Your original is untouched."));
      assert.equal(
        (await PDFDocument.load(await readFile(output))).getPageCount(),
        4,
      );
      assert.deepEqual(await readFile(fixture), Buffer.from(original));
    },
  );
  await check(
    "dark appearance persists on reload and responsive screenshot",
    async () => {
      await button("Use dark appearance").click();
      await page.reload();
      await visible(
        page.getByText(/take a look around with a sample document/),
      );
      assert.equal(
        await page.locator("html").getAttribute("data-theme"),
        "dark",
      );
      await page.getByText(/take a look around with a sample document/).click();
      await rendered();
      await app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()[0].setSize(1100, 800),
      );
      await page.screenshot({
        path: path.join(artifacts, "reader-1100-dark.png"),
      });
      await page.keyboard.press("Control+o");
      await visible(
        page.getByRole("dialog", { name: "Open a PDF", exact: true }),
      );
      await browseTo(temporary);
      await page.emulateMedia({ reducedMotion: "reduce", contrast: "more" });
      await page.screenshot({
        path: path.join(artifacts, "file-browser-dark.png"),
      });
      assert.equal(
        await page
          .getByRole("dialog")
          .evaluate((element) => element.scrollWidth > element.clientWidth),
        false,
      );
      await page.keyboard.press("Escape");
      assert.ok(
        await page
          .locator("body")
          .evaluate((el) => el.scrollWidth <= window.innerWidth),
      );
    },
  );
  await check(
    "discard requires an explicit choice in the in-app prompt",
    async () => {
      await button("Rotate current page").click();
      await button("Library").click();
      await visible(page.getByRole("dialog", { name: "Keep your changes?" }));
      assert.equal(
        await button("Keep editing").evaluate(
          (button) => button === document.activeElement,
        ),
        true,
      );
      await button("Discard changes").click();
      await visible(
        page.getByText(/take a look around with a sample document/),
      );
      assert.equal(await page.getByRole("dialog").count(), 0);
    },
  );
  assert.deepEqual(errors, [], "Renderer pageerrors");
  console.log(`PASS Electron smoke: ${count} workflows; 0 renderer pageerrors`);
} catch (error) {
  if (page && !page.isClosed())
    await page
      .screenshot({ path: path.join(artifacts, "smoke-failure.png") })
      .catch(() => {});
  console.error(`FAIL Electron smoke after ${count} workflows`, error);
  console.error("Renderer pageerrors:", errors);
  process.exitCode = 1;
} finally {
  if (app) {
    if (page && !page.isClosed())
      await page.evaluate(() => window.ethos.setDirty(false)).catch(() => {});
    await app.close();
  }
  // Only remove the uniquely allocated temporary folder, never an app profile.
  assert.ok(
    path.dirname(temporary) === tmpdir() &&
      path.basename(temporary).startsWith("ethos-smoke-"),
  );
  await rm(temporary, { recursive: true, force: true });
}
