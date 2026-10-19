// Bun integration (APP-009 R10): compiles user JSX and TSX with Babel and Solid's universal
// generation preset, strips TypeScript syntax, and resolves Solid to its client reactive runtime
// and store. It is a Bun build plugin and, through `preload.js`, a runtime preload.

import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { transformAsync } from "@babel/core";
import typescript from "@babel/preset-typescript";
import solid from "babel-preset-solid";

const require = createRequire(import.meta.url);
const solidRoot = dirname(require.resolve("solid-js/package.json"));

/** Solid's client entry points by specifier: never the server builds the `node` condition picks. */
export const CLIENT_ENTRIES = Object.freeze({
  "solid-js": join(solidRoot, "dist", "solid.js"),
  "solid-js/store": join(solidRoot, "store", "dist", "store.js"),
  "solid-js/universal": join(solidRoot, "universal", "dist", "universal.js"),
});

/** The module name compiled JSX imports the universal helpers from. */
export const MODULE_NAME = "@vespertine/solid";

/** The query string (and fragment) of a module specifier never reaches the file system. */
export function stripQuery(path) {
  return path.replace(/[?#].*$/, "");
}

/**
 * Compiles one JSX or TSX source. Babel configuration files in the repository are never read.
 *
 * @param {string} code
 * @param {string} filename
 */
export async function compileSolid(code, filename) {
  const result = await transformAsync(code, {
    filename,
    configFile: false,
    babelrc: false,
    sourceMaps: "inline",
    presets: [
      [solid, { generate: "universal", hydratable: false, moduleName: MODULE_NAME }],
      // Presets run last to first: strip types before the JSX transform sees the code.
      [typescript, { isTSX: true, allExtensions: true }],
    ],
  });
  return result?.code ?? "";
}

/** The Bun plugin object (`Bun.plugin(plugin)`, or `plugins: [plugin]` of `Bun.build`). */
const plugin = {
  name: "vespertine-solid",
  setup(build) {
    // The bundler asks the plugin to resolve bare specifiers; the runtime does not (it resolves
    // packages before any plugin sees them), so the runtime gets virtual modules instead.
    build.onResolve({ filter: /^solid-js(\/store|\/universal)?$/ }, (args) => ({
      path: CLIENT_ENTRIES[args.path],
    }));
    // A query string on a relative specifier never reaches the file system (the bundler cannot
    // resolve `./view.tsx?v=2` itself).
    build.onResolve({ filter: /^[./].*\.[jt]sx\?.*$/ }, (args) => {
      const path = stripQuery(args.path);
      return { path: isAbsolute(path) ? path : resolve(dirname(args.importer), path) };
    });
    try {
      for (const [specifier, path] of Object.entries(CLIENT_ENTRIES)) {
        build.module(specifier, () => ({ contents: `export * from ${JSON.stringify(path)};`, loader: "js" }));
      }
    } catch {
      // Bun.build() does not support virtual modules; its resolver hook above covers it.
    }
    build.onLoad({ filter: /\.[jt]sx(\?.*)?$/ }, async (args) => {
      const path = stripQuery(args.path);
      const code = await readFile(path, "utf8");
      return { contents: await compileSolid(code, path), loader: "js" };
    });
  },
};

export default plugin;
export { plugin };
