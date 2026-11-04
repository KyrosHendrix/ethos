import path from "node:path";
import { readdir, stat } from "node:fs/promises";
import type { FileEntry, FileListing, FileLocation } from "../src/shared";

export async function browseDirectory(
  value: unknown,
  locations: FileLocation[],
): Promise<FileListing> {
  if (
    value !== undefined &&
    (typeof value !== "string" ||
      !path.isAbsolute(value) ||
      value.includes("\0"))
  )
    throw new Error("Enter an absolute folder path.");
  const directory =
    typeof value === "string" ? path.resolve(value) : locations[0]?.path;
  if (!directory) throw new Error("No starting folder is available.");
  const items = await readdir(directory, { withFileTypes: true }).catch(
    (error: unknown) => {
      if (
        error instanceof Error &&
        "code" in error &&
        (error.code === "EACCES" || error.code === "EPERM")
      )
        throw new Error(
          "You don't have permission to open this folder. Choose another location.",
        );
      throw new Error(
        "This folder isn't available. Check its path or choose another location.",
      );
    },
  );
  const entries = await Promise.all(
    items
      .filter(
        (item) =>
          item.isDirectory() ||
          item.isSymbolicLink() ||
          path.extname(item.name).toLowerCase() === ".pdf",
      )
      .map(async (item): Promise<FileEntry | null> => {
        const filePath = path.join(directory, item.name);
        try {
          const info = await stat(filePath);
          if (
            !info.isDirectory() &&
            (!info.isFile() || path.extname(item.name).toLowerCase() !== ".pdf")
          )
            return null;
          return {
            name: item.name,
            path: filePath,
            kind: info.isDirectory() ? "directory" : "pdf",
            size: info.isFile() ? info.size : null,
            modified: info.mtimeMs,
          };
        } catch {
          return item.isDirectory()
            ? {
                name: item.name,
                path: filePath,
                kind: "directory",
                size: null,
                modified: null,
              }
            : null;
        }
      }),
  );
  const parent = path.dirname(directory);
  return {
    path: directory,
    parent: parent === directory ? null : parent,
    locations,
    entries: entries
      .filter((entry) => entry !== null)
      .sort(
        (a, b) =>
          (a.kind === b.kind ? 0 : a.kind === "directory" ? -1 : 1) ||
          a.name.localeCompare(b.name, undefined, { numeric: true }),
      ),
  };
}
