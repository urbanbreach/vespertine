// Sample data for the mosaic: a deterministic generated tree and an abortable fixture scanner.
// Producing the data (a filesystem scanner, file-type classification and coloring) is sample data
// production, not a framework capability.

import { createRandom } from "../editing/project.js";

/**
 * A node: `{ name, directory, allocated, apparent, files, mtime, children? }` where `allocated`
 * and `apparent` are byte counts, `files` the number of files below (1 for a file) and `mtime`
 * a timestamp in milliseconds.
 */
export function generateFixtureTree({ seed = 1, now = 1_700_000_000_000, depth = 4, breadth = 6 } = {}) {
  const random = createRandom(seed);
  const DAY = 86_400_000;
  const make = (name, level) => {
    if (level >= depth || (level > 0 && random() < 0.25)) {
      const apparent = Math.floor(10 ** (2 + random() * 5));
      return { name, directory: false, allocated: Math.ceil(apparent / 4096) * 4096, apparent, files: 1, mtime: now - Math.floor(random() * 500 * DAY) };
    }
    const count = 2 + Math.floor(random() * breadth);
    const children = Array.from({ length: count }, (_, index) => make(`${name}-${index}`, level + 1));
    return {
      name,
      directory: true,
      allocated: children.reduce((sum, child) => sum + child.allocated, 0),
      apparent: children.reduce((sum, child) => sum + child.apparent, 0),
      files: children.reduce((sum, child) => sum + child.files, 0),
      mtime: Math.max(...children.map((child) => child.mtime)),
      children,
    };
  };
  return make("root", 0);
}

/**
 * A scanner over a fixture tree: walks it in chunks (yielding to the event loop between chunks),
 * updates the live `counters` and resolves with the tree (a copy, so every scan yields a new tree
 * reference). Aborting rejects with an `AbortError`.
 */
export function createFixtureScanner({ tree, chunk = 64, yieldMs = 0 }) {
  return function scan({ signal, counters, includeHidden = true }) {
    return new Promise((resolve, reject) => {
      const queue = [tree];
      const aborted = () => {
        const error = new Error("scan aborted");
        error.name = "AbortError";
        reject(error);
      };
      if (signal?.aborted) return aborted();
      const step = () => {
        if (signal?.aborted) return aborted();
        for (let i = 0; i < chunk && queue.length > 0; i++) {
          const node = queue.pop();
          if (!includeHidden && node.name.startsWith(".")) continue;
          if (node.directory) {
            counters.directories++;
            queue.push(...node.children);
          } else {
            counters.files++;
            counters.bytes += node.allocated;
          }
        }
        if (queue.length > 0) setTimeout(step, yieldMs);
        else resolve(structuredClone(tree));
      };
      setTimeout(step, yieldMs);
    });
  };
}
