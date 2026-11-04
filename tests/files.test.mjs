import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { browseDirectory } from "../electron/files.ts";

test("directory browser lists folders and PDFs with folders first and numeric names", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "ethos-files-"));
  try {
    await mkdir(path.join(directory, "Z folder"));
    await Promise.all(
      ["Page 10.PDF", "Page 2.pdf", "hidden.txt"].map((name) =>
        writeFile(path.join(directory, name), "fixture"),
      ),
    );
    const locations = [{ name: "Test folder", path: directory }];
    const listing = await browseDirectory(undefined, locations);
    assert.equal(listing.path, directory);
    assert.equal(listing.parent, path.dirname(directory));
    assert.deepEqual(listing.locations, locations);
    assert.deepEqual(
      listing.entries.map(({ name, kind }) => [name, kind]),
      [
        ["Z folder", "directory"],
        ["Page 2.pdf", "pdf"],
        ["Page 10.PDF", "pdf"],
      ],
    );
    assert.equal(listing.entries[1].size, 7);
    assert.equal(typeof listing.entries[1].modified, "number");
    const nested = await browseDirectory(
      path.join(directory, "Z folder"),
      locations,
    );
    assert.equal(nested.parent, directory);
    assert.deepEqual(nested.entries, []);
    await assert.rejects(
      browseDirectory(path.join(directory, "absent"), locations),
      /folder isn't available/,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("directory browser rejects malformed paths before reading files", async () => {
  for (const value of [null, 42, {}, "relative/path", "invalid\0folder"])
    await assert.rejects(browseDirectory(value, []), /absolute folder path/);
  await assert.rejects(browseDirectory(undefined, []), /starting folder/);
});
