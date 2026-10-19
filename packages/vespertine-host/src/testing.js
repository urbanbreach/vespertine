// Native test helpers (DEV-006): the production renderer and event path over an offscreen test
// window. `createTestRenderer` returns a renderer with the same interface adapters use in
// production (mutation batches in, native events out) plus the test host's inspection and input
// helpers (click, key presses, painted text, element bounds, screenshots, motion clock).
//
// Import this module from tests only; production code imports `@vespertine/host`.

import { createNativeRenderer } from "./renderer.js";
import { loadNative } from "./native.js";

/**
 * @param {{ width?: number, height?: number, scaleFactor?: number, native?: object,
 *           onError?: Function, onEvent?: Function }} [options]
 */
export function createTestRenderer({ width, height, scaleFactor, native, onError, onEvent } = {}) {
  const addon = native ?? loadNative();
  if (typeof addon.TestRenderer !== "function") {
    throw new Error("The native addon was built without the offscreen test host (TestRenderer)");
  }
  return createNativeRenderer({
    native: { Renderer: addon.TestRenderer },
    rendererOptions: { width, height, scaleFactor },
    onError,
    onEvent,
    serveAutomation: () => {},
  });
}

/** True when the offscreen test host can render on this machine (a GPU or software adapter exists). */
export function testHostAvailable(native) {
  const addon = native ?? loadNative();
  return typeof addon.testHostAvailable === "function" ? addon.testHostAvailable() : false;
}
