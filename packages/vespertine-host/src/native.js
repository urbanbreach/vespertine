// Loading of the native addon (vespertine_js_node).

import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

/** Candidate addon locations, in priority order. */
export function addonCandidates() {
  const here = dirname(fileURLToPath(import.meta.url));
  const candidates = [];
  if (process.env.VESPERTINE_NATIVE_ADDON) candidates.push(resolve(process.env.VESPERTINE_NATIVE_ADDON));
  candidates.push(join(here, "..", "native", "vespertine_js.node"));
  return candidates;
}

let loaded = null;

/** Loads the native addon; throws an error listing the searched locations. */
export function loadNative() {
  if (loaded) return loaded;
  const candidates = addonCandidates();
  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      loaded = require(candidate);
      return loaded;
    }
  }
  throw new Error(
    `Vespertine native addon not found; build it with \`cargo build -p vespertine_js_node\` ` +
      `and set VESPERTINE_NATIVE_ADDON. Searched: ${candidates.join(", ")}`,
  );
}
