// Reactive views of window geometry and text search for React (APP-010 R1, R4, R6).

import React from "react";
import {
  createTextSearchController,
  findRanges,
  observeWindowInsets,
  observeWindowSize,
  readWindowInsets,
  readWindowSize,
  sameMutationValue,
} from "@vespertine/host";
import { useRenderer } from "./root.js";

/**
 * The window size `{ width, height }`: the first render already has the renderer's value, then
 * the shared native observer keeps it current. `intervalMs` defaults to 100 ms. A change of
 * renderer or interval replaces the subscription; unmounting removes it. Requires a root.
 */
export function useWindowSize({ intervalMs } = {}) {
  const renderer = useRenderer();
  const [size, setSize] = React.useState(() => readWindowSize(renderer));
  React.useEffect(
    () => observeWindowSize(renderer, (next) => setSize((previous) => (sameMutationValue(previous, next) ? previous : next)), { intervalMs }),
    [renderer, intervalMs],
  );
  return size;
}

/**
 * The window insets (`safeArea`, `ime` and `effective` edges plus `keyboardTop`,
 * `keyboardVisible` and `visibleHeight`), observed like `useWindowSize`. Requires a root.
 */
export function useWindowInsets({ intervalMs } = {}) {
  const renderer = useRenderer();
  const [insets, setInsets] = React.useState(() => readWindowInsets(renderer));
  React.useEffect(
    () => observeWindowInsets(renderer, (next) => setInsets((previous) => (sameMutationValue(previous, next) ? previous : next)), { intervalMs }),
    [renderer, intervalMs],
  );
  return insets;
}

/**
 * Text search over a subtree: spread `result.props` (`highlight` and `onHighlight`) onto the
 * text-bearing element. One shared controller is retained per hook instance; the result carries
 * `total`, `active` and the navigation `next`, `previous` and `goTo(index)`.
 *
 * @param {{ query?: string, caseSensitive?: boolean, wholeWord?: boolean, matchIndexOffset?: number,
 *           color?: string, activeColor?: string, radius?: number }} [options]
 */
export function useTextSearch(options = {}) {
  const ref = React.useRef(null);
  if (ref.current === null) ref.current = createTextSearchController();
  const controller = ref.current;
  const revision = React.useSyncExternalStore(controller.subscribe, controller.getRevision);
  const { query, caseSensitive, wholeWord, matchIndexOffset, color, activeColor, radius } = options;
  // `revision` makes the snapshot follow the controller; the option fields follow the caller.
  return React.useMemo(
    () => controller.snapshot({ query, caseSensitive, wholeWord, matchIndexOffset, color, activeColor, radius }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [controller, revision, query, caseSensitive, wholeWord, matchIndexOffset, color, activeColor, radius],
  );
}

export { findRanges };
