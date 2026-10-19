// Window geometry and selection helpers (APP-007 R7).

const FALLBACK_SIZE = { width: 800, height: 600 };
const MIN_INTERVAL_MS = 16;
const DEFAULT_INTERVAL_MS = 100;

const zeroEdges = () => ({ top: 0, right: 0, bottom: 0, left: 0 });

function edges(value) {
  return {
    top: Number(value?.top) || 0,
    right: Number(value?.right) || 0,
    bottom: Number(value?.bottom) || 0,
    left: Number(value?.left) || 0,
  };
}

export function readWindowSize(renderer) {
  try {
    const size = renderer?.getWindowSize?.();
    if (size && size.width > 0 && size.height > 0) return { width: size.width, height: size.height };
  } catch {
    // The window may still be opening.
  }
  return { ...FALLBACK_SIZE };
}

export function readWindowInsets(renderer) {
  const size = readWindowSize(renderer);
  let safeArea = zeroEdges();
  let ime = zeroEdges();
  let effective = zeroEdges();
  try {
    const insets = renderer?.getWindowInsets?.();
    if (insets) {
      safeArea = edges(insets.safeArea);
      ime = edges(insets.ime);
      effective = edges(insets.effective);
    }
  } catch {
    // Unavailable reads keep zero edges.
  }
  return {
    safeArea,
    ime,
    effective,
    keyboardTop: size.height - ime.bottom,
    keyboardVisible: ime.bottom > 0,
    visibleHeight: size.height - effective.top - effective.bottom,
  };
}

const sameEdges = (a, b) =>
  a.top === b.top && a.right === b.right && a.bottom === b.bottom && a.left === b.left;

const sameSize = (a, b) => a.width === b.width && a.height === b.height;

const sameInsets = (a, b) =>
  sameEdges(a.safeArea, b.safeArea) &&
  sameEdges(a.ime, b.ime) &&
  sameEdges(a.effective, b.effective) &&
  a.keyboardTop === b.keyboardTop &&
  a.keyboardVisible === b.keyboardVisible &&
  a.visibleHeight === b.visibleHeight;

function observe(read, same, callback, intervalMs) {
  let last = read();
  callback(last);
  if (intervalMs === false) return () => {};
  const period = Math.max(MIN_INTERVAL_MS, intervalMs ?? DEFAULT_INTERVAL_MS);
  const timer = globalThis.setInterval(() => {
    const next = read();
    if (same(last, next)) return;
    last = next;
    callback(next);
  }, period);
  return () => globalThis.clearInterval(timer);
}

export function observeWindowSize(renderer, callback, { intervalMs } = {}) {
  return observe(() => readWindowSize(renderer), sameSize, callback, intervalMs);
}

export function observeWindowInsets(renderer, callback, { intervalMs } = {}) {
  return observe(() => readWindowInsets(renderer), sameInsets, callback, intervalMs);
}

/**
 * Delivers the current selected text, then the text of every selection-change event the
 * `subscribe` function reports; returns the unsubscribe function.
 */
export function observeSelectedText(renderer, subscribe, callback) {
  let current = null;
  try {
    current = renderer?.getSelectedText?.() ?? null;
  } catch {
    current = null;
  }
  callback(current);
  return subscribe((event) => callback(typeof event === "string" ? event : (event?.text ?? null)));
}
