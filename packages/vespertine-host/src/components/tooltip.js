// The timing model of the compound tooltip (CMP-004 R1, R5-R9), independent of the framework.
//
// A provider remembers the last close time and the default delays; a root drives one tooltip's
// open state from hover, focus and press events. The adapters differ in two documented ways:
//
// - React cancels the previous timer of the same kind before replacing it (`replaceTimers`) and
//   reads provider settings on every use; Solid overwrites its tracked handle without cancelling
//   an already scheduled timer of the same kind and captures the provider settings and the
//   root's hoverability when the root is constructed.

/** Defaults of the provider (CMP-004 Parameters). */
export const TOOLTIP_DEFAULTS = Object.freeze({
  delayDuration: 0,
  skipDelayDuration: 300,
  disableHoverableContent: false,
});

/** Time between the pointer leaving a hoverable tooltip's trigger or content and its closing. */
export const TOOLTIP_HIDE_GRACE_MS = 80;

/**
 * The state of a tooltip provider: its settings and the last close time. `settings` may be an
 * object or a function returning one (React reads it fresh on every use).
 */
export function createTooltipProvider(settings = {}, { now = () => Date.now() } = {}) {
  const read = () => ({ ...TOOLTIP_DEFAULTS, ...(typeof settings === "function" ? settings() : settings) });
  return {
    settings: read,
    lastCloseTime: Number.NEGATIVE_INFINITY,
    now,
  };
}

/**
 * A tooltip root.
 *
 * @param {object} options
 * @param {object} options.provider a provider from `createTooltipProvider`
 * @param {(open: boolean) => void} options.requestOpen applies an open state request (the owner
 *   handles controlled and uncontrolled state)
 * @param {() => { delayDuration?: number, disableHoverableContent?: boolean }} [options.overrides]
 *   per-root overrides of the provider's delay and hoverability, read on every use
 * @param {boolean} [options.replaceTimers] cancel the earlier timer of a kind before scheduling
 *   another (React); otherwise the tracked handle is overwritten (Solid)
 * @param {{ setTimeout: Function, clearTimeout: Function }} [options.timers]
 */
export function createTooltipRoot({
  provider,
  requestOpen,
  overrides = () => ({}),
  replaceTimers = true,
  timers = {
    setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
    clearTimeout: (handle) => globalThis.clearTimeout(handle),
  },
}) {
  let openTimer = null;
  let closeTimer = null;

  const hoverable = () => {
    const own = overrides().disableHoverableContent;
    return !(own ?? provider.settings().disableHoverableContent);
  };

  const clearTimers = () => {
    if (openTimer !== null) timers.clearTimeout(openTimer);
    if (closeTimer !== null) timers.clearTimeout(closeTimer);
    openTimer = null;
    closeTimer = null;
  };

  /** Every state request clears both tracked timers; a close updates the last close time. */
  function setOpen(open) {
    clearTimers();
    if (!open) provider.lastCloseTime = provider.now();
    requestOpen(open);
  }

  function schedule(kind, delay, action) {
    const previous = kind === "open" ? openTimer : closeTimer;
    if (replaceTimers && previous !== null) timers.clearTimeout(previous);
    const handle = timers.setTimeout(() => {
      if (kind === "open") openTimer = null;
      else closeTimer = null;
      action();
    }, delay);
    if (kind === "open") openTimer = handle;
    else closeTimer = handle;
  }

  function cancel(kind) {
    const handle = kind === "open" ? openTimer : closeTimer;
    if (handle !== null) timers.clearTimeout(handle);
    if (kind === "open") openTimer = null;
    else closeTimer = null;
  }

  function leave() {
    cancel("open");
    if (hoverable()) schedule("close", TOOLTIP_HIDE_GRACE_MS, () => setOpen(false));
    else setOpen(false);
  }

  /** The effective show delay now: zero inside the skip window after a close. */
  function effectiveDelay() {
    const settings = provider.settings();
    const delay = overrides().delayDuration ?? settings.delayDuration;
    return provider.now() - provider.lastCloseTime <= settings.skipDelayDuration ? 0 : delay;
  }

  return {
    setOpen,
    effectiveDelay,
    /** The trigger was entered by the pointer. */
    triggerEnter() {
      cancel("close");
      const delay = effectiveDelay();
      if (delay <= 0) setOpen(true);
      else schedule("open", delay, () => setOpen(true));
    },
    /** The pointer left the trigger. */
    triggerLeave: leave,
    /** The pointer entered the content: only hoverable content keeps the tooltip open. */
    contentEnter() {
      if (hoverable()) cancel("close");
    },
    /** The pointer left the content. */
    contentLeave: leave,
    /** The trigger received focus. */
    focus: () => setOpen(true),
    /** The trigger lost focus. */
    blur: () => setOpen(false),
    /** A press on the trigger. */
    pointerDown: () => setOpen(false),
    /** A click on the trigger. */
    click: () => setOpen(false),
    /** Cancels the tracked timers (unmount). */
    dispose: clearTimers,
    get hasPendingOpen() {
      return openTimer !== null;
    },
    get hasPendingClose() {
      return closeTimer !== null;
    },
  };
}
