// The convenience render entry point (APP-009 R9). Errors are forwarded and logged; the Solid
// adapter installs no process-level handlers and shows no error view.

import { startFrameLoop } from "@vespertine/host";
import { createRenderer, createRoot } from "./root.js";

/** The single retained renderer, window, frame loop and root. */
let session = null;

const report = (error) => console.error(error);

/**
 * Renders `code` (a function returning the UI) into the single window of this process, creating
 * the native renderer, opening the window and starting the frame loop on the first call. Later
 * calls unmount the previous tree first and ignore window options.
 *
 * Options: `renderer` (an externally owned renderer: never initialized nor pumped), `native`,
 * `window`, `debugOverlay` (`"hidden" | "minimal" | "full"`), `frameMs`, and every root option of
 * `createRoot`.
 *
 * @returns the Solid root
 */
export function render(code, options = {}) {
  if (session === null) {
    const injected = options.renderer ?? null;
    const owned = injected === null;
    const renderer = owned ? createRenderer(undefined, { native: options.native, onError: report }) : injected;
    session = { renderer, loop: null, root: null };
    if (owned) {
      renderer.init(options.window ?? {});
      session.loop = startFrameLoop(renderer, {
        frameMs: options.frameMs,
        onError: report,
        onTerminated: () => globalThis.process?.exit?.(0),
      });
    }
  }
  if (options.debugOverlay !== undefined) session.renderer.setDebugOverlay?.(options.debugOverlay);
  session.root?.unmount();
  session.root = createRoot(session.renderer, options);
  session.root.render(code);
  return session.root;
}

/** Stops the frame loop and unmounts the current root. */
export function resetRender() {
  const current = session;
  session = null;
  if (current === null) return;
  current.loop?.stop();
  current.root?.unmount();
}
