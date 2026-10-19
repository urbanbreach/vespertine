// The behavior of a button (CMP-001 R1, R2, R4): role, tab index and the activation rules for
// pointer, Enter and Space, shared by every control that wants them.

const ENTER = "enter";
const SPACE_KEYS = new Set(["space", " ", "spacebar"]);

const keyName = (event) => String(event.key ?? "").toLowerCase();

function hasModifier(event) {
  return Boolean(event.ctrl || event.alt || event.cmd || event.shift);
}

/**
 * The per-control latch of the Enter activation: set on the first Enter key-down, cleared by
 * the matching key-up. It guards against key-downs of a held key that the platform does not flag
 * as held (X11), in addition to ignoring events flagged as held.
 */
export function createButtonLatch() {
  return { enter: false };
}

/** The tab index of a button: explicit, else -1 while disabled (unless focusable), else 0. */
export function buttonTabIndex({ disabled = false, focusableWhenDisabled = false, tabIndex } = {}) {
  if (tabIndex !== undefined && tabIndex !== null) return tabIndex;
  return disabled && !focusableWhenDisabled ? -1 : 0;
}

/**
 * The activation properties of a button-like control: `role`, `tabIndex`, `onClick`,
 * `onKeyDown` and `onKeyUp`.
 *
 * - A primary-button click on an enabled control activates; the click's default is not inspected.
 * - Enter activates on the first non-held key-down and consumes the key-down default, so exactly
 *   one activation happens per physical press.
 * - Space prevents the default on key-down and activates on key-up.
 * - Caller key handlers run before the internal ones. A disabled control, a key event whose
 *   default was already prevented and a key with modifiers never activate.
 * - `disabled` suppresses activation; `focusableWhenDisabled` only keeps the control reachable.
 *
 * `latch` (see `createButtonLatch`) keeps the Enter latch across calls; supply one that lives as
 * long as the control. `onClick` is the activation callback and receives the originating event.
 *
 * @param {{ disabled?: boolean, focusableWhenDisabled?: boolean, tabIndex?: number,
 *           onClick?: Function, onKeyDown?: Function, onKeyUp?: Function, latch?: object }} [options]
 */
export function buttonProps(options = {}) {
  const { disabled = false, onClick, onKeyDown, onKeyUp, latch = createButtonLatch() } = options;
  const activate = (event) => {
    if (!disabled) onClick?.(event);
  };
  return {
    role: "button",
    tabIndex: buttonTabIndex(options),
    onClick(event) {
      if (event?.button !== undefined && event.button !== 0) return;
      activate(event);
    },
    onKeyDown(event) {
      onKeyDown?.(event);
      if (disabled || event.defaultPrevented || hasModifier(event)) return;
      const key = keyName(event);
      if (key === ENTER) {
        // The default is consumed in every case, so the platform's keyboard click (which fires
        // on key release) can never activate a second time.
        const repeat = event.held === true || latch.enter;
        latch.enter = true;
        event.preventDefault?.();
        if (!repeat) activate(event);
      } else if (SPACE_KEYS.has(key)) {
        event.preventDefault?.();
      }
    },
    onKeyUp(event) {
      onKeyUp?.(event);
      const key = keyName(event);
      if (key === ENTER) latch.enter = false;
      if (disabled || event.defaultPrevented || hasModifier(event)) return;
      if (SPACE_KEYS.has(key)) activate(event);
    },
  };
}
