// The framework-neutral parts of the headless Dialog (CMP-003 R3-R10): the portal, backdrop and
// popup defaults, the modal Tab trap and the dismissal policy.

/** The deferred paint priority of the portal: below priority-1 menus (CMP-003 Parameters). */
export const DIALOG_PORTAL_PRIORITY = 0;

/** The heading level of a dialog title (CMP-003 R8). */
export const DIALOG_TITLE_LEVEL = 2;

/**
 * The properties of the `anchored` host that forms the portal: it fills the window, paints
 * deferred at priority 0, occludes the page beneath it when modal, centers its children with
 * flex layout and has an explicitly transparent background. A nonmodal portal disables pointer
 * events on its full-window surface (the popup still receives input, see `popupStyle`). The
 * caller's style is merged over `style` by the adapter.
 *
 * @param {{ modal?: boolean }} [options]
 */
export function portalProps({ modal = true } = {}) {
  return {
    fill: "window",
    deferred: true,
    priority: DIALOG_PORTAL_PRIORITY,
    occlude: modal,
    style: {
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      backgroundColor: "transparent",
      ...(modal ? {} : { pointerEvents: "none" }),
    },
  };
}

/**
 * The default style of the backdrop: absolute with all four insets zero so it covers the window.
 * Its absolute position makes it block the pointer beneath it; a nonmodal dialog's backdrop
 * lets pointer input through to the page, as its portal does. No color is imposed.
 */
export function backdropStyle({ modal = true } = {}) {
  return { position: "absolute", top: 0, left: 0, right: 0, bottom: 0, ...(modal ? {} : { pointerEvents: "none" }) };
}

/**
 * The default style of the popup (CMP-003 R5): pointer events are on (so a press inside it never
 * reaches the backdrop behind it) and the keyboard focus dim is off.
 */
export function popupStyle() {
  return { pointerEvents: "auto", keyboardFocusDim: false };
}

/**
 * The modal Tab handling of the popup (CMP-003 R7). Call it from the popup's key-down after the
 * caller's handler ran: for an unprevented Tab without command modifiers it prevents the default
 * and moves focus to the next (Shift: previous) focusable descendant of `popup`, wrapping
 * inside it. Returns whether the key was handled.
 *
 * @param {{ key?: string, shift?: boolean, ctrl?: boolean, alt?: boolean, cmd?: boolean,
 *           defaultPrevented?: boolean, preventDefault?: () => void }} event
 * @param {{ focusNextWithin: () => unknown, focusPreviousWithin: () => unknown } | null} popup
 */
export function trapTab(event, popup) {
  if (event.defaultPrevented || String(event.key ?? "").toLowerCase() !== "tab") return false;
  if (event.ctrl || event.alt || event.cmd) return false;
  event.preventDefault?.();
  if (event.shift) popup?.focusPreviousWithin();
  else popup?.focusNextWithin();
  return true;
}

/**
 * Whether a dismissal attempt closes the dialog (CMP-003 R4, R9, R10).
 *
 * - Escape closes unless escape dismissal is disabled (the layer consumes it either way).
 * - An outside press is only meaningful for a nonmodal dialog (a modal one is dismissed by a
 *   press on its backdrop) and only while the dialog is the top layer, so presses belonging to
 *   a nested layer opened from it leave it open; `disablePointerDismissal` forbids it.
 *
 * @param {"escape" | "outside"} reason
 * @param {{ modal: boolean, disablePointerDismissal: boolean, disableEscapeDismissal: boolean }} policy
 * @param {{ top?: boolean }} [info]
 */
export function dismissalCloses(reason, { modal, disablePointerDismissal, disableEscapeDismissal }, { top = true } = {}) {
  if (reason === "escape") return !disableEscapeDismissal;
  return !modal && !disablePointerDismissal && top;
}
