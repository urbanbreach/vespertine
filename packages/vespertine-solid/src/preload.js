// Bun preload: `bun --preload @vespertine/solid/preload app.tsx` (or `preload` in bunfig.toml).
// Registering is idempotent across copies of this module: the second registration is a no-op.

import plugin from "./bun-plugin.js";

const KEY = Symbol.for("vespertine.solid.preloadRegistered");

/**
 * Registers the plugin with the running Bun once per process.
 * @returns {boolean} whether this call registered it
 */
export function registerPreload(bun = globalThis.Bun) {
  if (globalThis[KEY] === true || typeof bun?.plugin !== "function") return false;
  globalThis[KEY] = true;
  bun.plugin(plugin);
  return true;
}

registerPreload();
export default plugin;
