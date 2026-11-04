import { cp, mkdir, readdir } from "node:fs/promises";
import assert from "node:assert/strict";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
for (const folder of ["cmaps", "standard_fonts", "wasm"]) {
  const destination = path.join(root, "public", "pdf-assets", folder);
  await mkdir(destination, { recursive: true });
  await cp(path.join(root, "node_modules", "pdfjs-dist", folder), destination, {
    recursive: true,
  });
  assert(
    (await readdir(destination)).length > 0,
    `PDF assets missing: ${folder}`,
  );
}
