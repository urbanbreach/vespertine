// Reactive views of window geometry, the selection and text search for Solid (APP-010 R2, R3, R5).

import { createMemo, createSignal, onCleanup } from "solid-js";
import {
  createTextSearchController,
  findRanges,
  observeWindowInsets,
  observeWindowSize,
  readWindowInsets,
  readWindowSize,
  sameMutationValue,
} from "@vespertine/host";
import { useRendererContext } from "./root.js";

function observed(read, observe, options) {
  const { renderer } = useRendererContext();
  const [value, setValue] = createSignal(read(renderer), { equals: sameMutationValue });
  onCleanup(observe(renderer, setValue, options));
  return value;
}

/**
 * The window size as an accessor of `{ width, height }`. Requires a renderer context; the
 * observation options (`intervalMs`, default 100 ms) go to the shared native observer and the
 * subscription ends with the current owner.
 */
export function createWindowSize(options = {}) {
  return observed(readWindowSize, observeWindowSize, options);
}

/** The window insets as an accessor, observed like `createWindowSize`. */
export function createWindowInsets(options = {}) {
  return observed(readWindowInsets, observeWindowInsets, options);
}

/**
 * The document-wide selected text as an accessor (`null` when nothing is selected). Requires a
 * root context; subscribes to the root's selection notifications (the current selection is
 * reported immediately) and unsubscribes with the owner.
 */
export function createSelectedText() {
  const { renderer, subscribeSelection } = useRendererContext();
  const [text, setText] = createSignal(renderer.getSelectedText?.() ?? null);
  onCleanup(subscribeSelection(setText));
  return text;
}

/**
 * Text search over a subtree. `options` is an accessor of the search options (`query`,
 * `caseSensitive`, `wholeWord`, ...); the result has reactive getters `props` (spread onto the
 * text-bearing element), `total` and `active` plus `next`, `previous` and `goTo(index)`.
 */
export function createTextSearch(options) {
  const controller = createTextSearchController();
  const [revision, setRevision] = createSignal(controller.getRevision());
  onCleanup(controller.subscribe(() => setRevision(controller.getRevision())));
  const snapshot = createMemo(() => {
    revision();
    return controller.snapshot(options());
  });
  return {
    get props() {
      return snapshot().props;
    },
    get total() {
      return snapshot().total;
    },
    get active() {
      return snapshot().active;
    },
    next: controller.next,
    previous: controller.previous,
    goTo: controller.goTo,
    controller,
  };
}

export { findRanges };
