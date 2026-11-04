import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import electron from "electron";
import { _electron } from "playwright-core";

const temporary = await mkdtemp(path.join(tmpdir(), "ethos-shortcuts-"));
const env = { ...process.env, ETHOS_USER_DATA: temporary };
delete env.ELECTRON_RUN_AS_NODE;
const warnings = [];
let app;
try {
  app = await _electron.launch({ executablePath: electron, args: ["."], env });
  const page = await app.firstWindow();
  page.on("console", (message) => {
    if (
      message.type() === "error" &&
      /same key|unique.*key/i.test(message.text())
    )
      warnings.push(message.text());
  });
  await app.evaluate(({ ipcMain }, directory) => {
    ipcMain.removeHandler("file:browse");
    ipcMain.handle("file:browse", () => ({
      ok: true,
      value: {
        path: directory,
        parent: null,
        entries: [],
        locations: ["Documents", "Downloads", "D:"].map((name) => ({
          name,
          path: directory,
        })),
      },
    }));
  }, temporary);
  await page.getByText(/take a look around with a sample document/).waitFor();
  await page.keyboard.press("Control+o");
  await page.locator('.browser-files[aria-busy="false"]').waitFor();
  for (const name of ["Documents", "Downloads", "D:"]) {
    const shortcut = page
      .getByRole("complementary", { name: "File locations" })
      .getByRole("button", { name, exact: true });
    assert.equal(await shortcut.count(), 1);
    await shortcut.click();
    await page.locator('.browser-files[aria-busy="false"]').waitFor();
    assert.equal(await shortcut.count(), 1);
  }
  await page.keyboard.press("Escape");
  assert.deepEqual(warnings, [], "React duplicate shortcut key warnings");
  console.log(
    "PASS matching-path shortcuts stay distinct and usable; 1 workflow; 0 duplicate-key warnings",
  );
} catch (error) {
  console.error("FAIL matching-path shortcuts", error);
  process.exitCode = 1;
} finally {
  if (app) await app.close();
  await rm(temporary, { recursive: true, force: true });
}
