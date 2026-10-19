// Renderer creation (APP-007 R8).

import { tty } from "./stdio.js";
import { loadNative } from "./native.js";
import { createRendererState } from "./renderer-state.js";

/**
 * True when standard input exists and is not a terminal (for example a pipe).
 */
export function isNonInteractive() {
  return tty.stdinExists() && !tty.stdinIsTerminal();
}

/**
 * Creates the native renderer. Native events are routed to the active root's dispatch;
 * callback errors and exceptions thrown during dispatch go to `onError`, handled events to
 * `onEvent`. When the process is non-interactive, live automation is served over standard
 * input and output after `init` succeeds.
 *
 * @param {{ onEvent?: Function, onError?: Function, native?: object,
 *           serveAutomation?: (renderer: object) => void, rendererOptions?: object }} [options]
 */
export function createNativeRenderer({ onEvent, onError, native, serveAutomation, rendererOptions } = {}) {
  const addon = native ?? loadNative();
  let renderer = null;
  renderer = new addon.Renderer((error, event) => {
    if (error) {
      onError?.(error);
      return;
    }
    try {
      if (createRendererState(renderer).dispatch(event)) onEvent?.(event);
    } catch (thrown) {
      onError?.(thrown);
    }
  }, rendererOptions);

  if (typeof renderer.init === "function" && isNonInteractive()) {
    const init = renderer.init.bind(renderer);
    renderer.init = (options) => {
      const result = init(options);
      (serveAutomation ?? ((target) => target.serveAutomation?.()))(renderer);
      return result;
    };
  }
  return renderer;
}
