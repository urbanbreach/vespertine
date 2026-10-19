// Native test helpers for React (DEV-006): mounts React trees into the offscreen test host through
// the production reconciler, root and event path. Input dispatch flushes React work
// synchronously. Import from tests only.

import { createTestRenderer, testHostAvailable } from "@vespertine/host/testing";
import { createRoot } from "./root.js";

export { testHostAvailable };

/**
 * Creates a test host: `host.renderer` is the native test renderer (click, keyboard, painted text,
 * element bounds, screenshots, the motion clock, ...); `host.render(node)` mounts synchronously.
 *
 * @param {{ width?: number, height?: number, scaleFactor?: number, native?: object } & object} [options]
 *   root options (`onEvent`, `onWindowKeyDown`, ...) are passed to `createRoot`
 */
export function createTestHost({ width, height, scaleFactor, native, onError, ...rootOptions } = {}) {
  const errors = [];
  const renderer = createTestRenderer({ width, height, scaleFactor, native, onError: onError ?? ((error) => errors.push(error)) });
  const root = createRoot(renderer, { onUncaughtError: (error) => errors.push(error), ...rootOptions });
  return {
    renderer,
    root,
    /** Errors thrown by event handlers or reported by React. */
    errors,
    /** Renders `node` and flushes React and the native tree synchronously. */
    render(node) {
      root.renderSync(node);
      renderer.flush?.();
    },
    unmount() {
      root.unmount();
    },
  };
}
