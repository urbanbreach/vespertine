import { watch as nodeWatch } from "node:fs";
import path from "node:path";

/** Whether a changed path (relative to the watched directory) is Rust source. */
export function isRustSource(relativePath) {
  if (!relativePath || !relativePath.endsWith(".rs")) return false;
  return !relativePath.split(path.sep).includes("target");
}

/**
 * Watches Rust sources below `directories` (recursively; `fs.watch` supports this on Linux
 * with Node >= 20 and Bun) and calls `onChange(path)` for each change event.
 *
 * @returns {{ close(): void }}
 */
export function watchRustSources(directories, onChange, { watch = nodeWatch } = {}) {
  const watchers = directories.map((directory) =>
    watch(directory, { recursive: true }, (_event, filename) => {
      const name = filename === null || filename === undefined ? "" : String(filename);
      if (isRustSource(name)) onChange(path.join(directory, name));
    }),
  );
  return {
    close() {
      for (const watcher of watchers) watcher.close();
    },
  };
}
