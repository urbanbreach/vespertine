// Handler composition and child-composition (asChild) merging (CMP-001 R5, R6).

/** The handler properties React composes when a part merges onto its child. */
export const COMPOSED_HANDLERS = Object.freeze([
  "onClick",
  "onPointerDown",
  "onPointerUp",
  "onEnter",
  "onLeave",
  "onPointerMove",
  "onOutsideDown",
  "onKeyDown",
  "onKeyUp",
  "onFocus",
  "onBlur",
  "onWheel",
  "onScrollChange",
  "onScroll",
  "onChange",
  "onSubmit",
]);

/**
 * Runs `first` and then `second` with the same arguments. The second handler runs even when the
 * first one prevented the default of its event; either may be missing.
 */
export function composeHandlers(first, second) {
  if (typeof first !== "function") return second;
  if (typeof second !== "function") return first;
  return function composed(...args) {
    first.apply(this, args);
    return second.apply(this, args);
  };
}

/** A ref callback delivering to every ref (callback or object) that is present. */
export function composeRefs(...refs) {
  const present = refs.filter((ref) => ref !== undefined && ref !== null);
  if (present.length === 0) return undefined;
  if (present.length === 1) return present[0];
  return (value) => {
    for (const ref of present) {
      if (typeof ref === "function") ref(value);
      else ref.current = value;
    }
  };
}

/**
 * Merges a part's properties onto its child's.
 *
 * - Child handlers run before the part's; the part's replace nothing (both always run).
 * - An explicit part `tabIndex` overrides the child's, which overrides `defaults.tabIndex`.
 * - Styles are merged shallowly with the part's winning (`mergeStyle`; pass `false` to let the
 *   part's style replace the child's, as the Solid adapter does).
 * - Any other defined part property replaces the child's.
 *
 * @param {object} child the child's properties
 * @param {object} part the part's own properties (explicit tab index, handlers, style, ...)
 * @param {{ defaults?: { tabIndex?: number }, handlers?: readonly string[], mergeStyle?: Function | false }} [options]
 */
export function mergeSlotProps(child, part, { defaults = {}, handlers = COMPOSED_HANDLERS, mergeStyle } = {}) {
  const composed = new Set(handlers);
  const merged = { ...child };
  for (const [name, value] of Object.entries(part)) {
    if (value === undefined || name === "tabIndex" || name === "style") continue;
    merged[name] = composed.has(name) ? composeHandlers(child[name], value) : value;
  }
  const tabIndex = part.tabIndex ?? child.tabIndex ?? defaults.tabIndex;
  if (tabIndex !== undefined) merged.tabIndex = tabIndex;
  if (mergeStyle !== undefined && mergeStyle !== false) {
    const style = mergeStyle(child.style, part.style);
    if (style !== undefined) merged.style = style;
  } else if (part.style !== undefined) {
    merged.style = part.style;
  }
  return merged;
}
