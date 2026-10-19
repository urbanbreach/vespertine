// Native test helpers for Solid (DEV-006): mounts Solid trees into the offscreen test host
// through the production renderer, root and event path. Root dispatch flushes synchronously.
// Import from tests only.

import { createTestRenderer, testHostAvailable } from "@vespertine/host/testing";
import { createRoot } from "./root.js";

export { testHostAvailable };

/**
 * @param {{ width?: number, height?: number, scaleFactor?: number, native?: object } & object} [options]
 *   root options (`onEvent`, `onWindowKeyDown`, ...) are passed to `createRoot`
 */
export function createTestHost({ width, height, scaleFactor, native, onError, ...rootOptions } = {}) {
  const errors = [];
  const renderer = createTestRenderer({ width, height, scaleFactor, native, onError: onError ?? ((error) => errors.push(error)) });
  const root = createRoot(renderer, rootOptions);
  return {
    renderer,
    root,
    errors,
    /** Renders `code` (a function returning the UI); flushes immediately. */
    render(code) {
      root.render(code);
      renderer.flush?.();
    },
    unmount() {
      root.unmount();
    },
  };
}
