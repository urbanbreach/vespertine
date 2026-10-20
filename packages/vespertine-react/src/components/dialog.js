// The headless compound Dialog (CMP-003 R1-R10): Root, Trigger, Portal, Backdrop, Popup, Title,
// Description and Close.
//
// Root renders no element of its own; it only provides state. The portal and the popup mount only
// while open. The popup is a dismissal layer: Escape closes it only when it is the top layer (a
// select opened from inside closes first), a modal popup traps Tab, and closing returns focus to
// the trigger (or the element focused before opening).

import React from "react";
import {
  DIALOG_TITLE_LEVEL,
  backdropStyle,
  buttonProps as behavior,
  composeHandlers,
  createPressFlag,
  dismissalCloses,
  mergeStyles,
  popupStyle,
  portalProps,
  resolveStyle,
  trapTab,
} from "@vespertine/host";
import { useButtonLatch } from "./button.js";
import { useControllableState } from "./controlled.js";
import { DismissableLayer } from "./layer.js";
import { renderSlot } from "./slot.js";

const DialogContext = React.createContext(null);

function useDialog(part) {
  const context = React.useContext(DialogContext);
  if (context === null) throw new Error(`Dialog.${part} must be used inside Dialog.Root`);
  return context;
}

/**
 * The dialog state: `open`/`defaultOpen`/`onOpenChange` (closed by default; controlled when `open`
 * is given), `modal` (true) and `disablePointerDismissal` (false), plus `disableEscapeDismissal`
 * (false: Escape still never reaches the page, it just does not close the dialog). Renders no
 * element.
 */
export function Root({ open: openProp, defaultOpen = false, onOpenChange, modal = true, disablePointerDismissal = false, disableEscapeDismissal = false, children }) {
  const [open, requestOpen] = useControllableState({ value: openProp, defaultValue: defaultOpen, onChange: onOpenChange });
  const trigger = React.useRef(null);
  const [pressFlag] = React.useState(createPressFlag);
  const context = { open, requestOpen, modal, disablePointerDismissal, disableEscapeDismissal, trigger, pressFlag };
  return React.createElement(DialogContext.Provider, { value: context }, children);
}

/** A button-like part that requests an open state after the caller's activation. */
function activationPart(name, opens) {
  const Part = React.forwardRef(function Part(
    { disabled = false, asChild = false, children, style, tabIndex, onClick, onKeyDown, onKeyUp, onPointerDown, ...rest },
    ref,
  ) {
    const dialog = useDialog(name);
    const latch = useButtonLatch();
    const button = behavior({
      disabled,
      tabIndex,
      onClick: (event) => {
        onClick?.(event);
        dialog.requestOpen(opens);
      },
      onKeyDown,
      onKeyUp,
      latch,
    });
    const setRef = React.useCallback(
      (node) => {
        if (opens) dialog.trigger.current = node;
        if (typeof ref === "function") ref(node);
        else if (ref) ref.current = node;
      },
      // eslint-disable-next-line react-hooks/exhaustive-deps
      [ref, dialog.trigger],
    );
    return renderSlot({
      asChild,
      children,
      ref: setRef,
      props: {
        ...rest,
        role: button.role,
        ...(opens ? { "aria-expanded": dialog.open } : {}),
        tabIndex,
        style: resolveStyle(style, { open: dialog.open, disabled }),
        onClick: button.onClick,
        onKeyDown: button.onKeyDown,
        onKeyUp: button.onKeyUp,
        onPointerDown: opens ? composeHandlers(onPointerDown, () => dialog.pressFlag.raise()) : onPointerDown,
      },
      defaults: { tabIndex: button.tabIndex },
    });
  });
  Part.displayName = `Dialog.${name}`;
  return Part;
}

/**
 * The control that opens the dialog: button keyboard behavior, `aria-expanded` for the open
 * state, the caller's activation first and then a request for `open = true` (it never toggles).
 * `style` is an object or a function of `{ open, disabled }`; `asChild` merges onto the element
 * child.
 */
export const Trigger = activationPart("Trigger", true);

/** Closes the dialog: the caller's activation first and then a request for `open = false`. */
export const Close = activationPart("Close", false);

/**
 * The full-window layer holding the backdrop and the popup, mounted only while open: deferred at
 * priority 0 (below priority-1 menus), centering its children with flex layout on an explicitly
 * transparent background (the caller's style may override). A modal portal occludes the page; a
 * nonmodal one disables pointer events on its surface so the page stays interactive.
 */
export const Portal = React.forwardRef(function Portal({ style, children, ...rest }, ref) {
  const dialog = useDialog("Portal");
  if (!dialog.open) return null;
  const base = portalProps({ modal: dialog.modal });
  return renderSlot({
    children,
    type: "anchored",
    ref,
    props: { ...rest, fill: base.fill, deferred: base.deferred, priority: base.priority, occlude: base.occlude, style: mergeStyles(base.style, style) },
  });
});

/**
 * The backdrop: absolute with all four insets zero. A pointer press runs the caller's
 * `onPointerDown` first and then closes the dialog unless `disablePointerDismissal`. No color is
 * imposed.
 */
export const Backdrop = React.forwardRef(function Backdrop({ style, onPointerDown, asChild = false, children, ...rest }, ref) {
  const dialog = useDialog("Backdrop");
  return renderSlot({
    asChild,
    children,
    ref,
    props: {
      ...rest,
      style: mergeStyles(backdropStyle({ modal: dialog.modal }), style),
      onPointerDown: composeHandlers(onPointerDown, () => {
        if (!dialog.disablePointerDismissal) dialog.requestOpen(false);
      }),
    },
  });
});

/**
 * The dialog surface, mounted only while open. Defaults: role `dialog`, tab index -1, pointer
 * events on (a press inside never reaches the backdrop) and no keyboard focus dim; nothing else
 * is styled. `initialFocus` (default: the popup itself) and `finalFocus` (default: the trigger
 * when mounted, else the element focused before opening) follow the focus-target contract.
 * A modal popup traps Tab inside itself after the caller's key-down; a nonmodal one does not.
 */
export const Popup = React.forwardRef(function Popup(
  { style, initialFocus, finalFocus, onEscapeKeyDown, onKeyDown, children, ...rest },
  ref,
) {
  const dialog = useDialog("Popup");
  const nodeRef = React.useRef(null);
  const setRef = React.useCallback(
    (node) => {
      nodeRef.current = node;
      if (typeof ref === "function") ref(node);
      else if (ref) ref.current = node;
    },
    [ref],
  );
  if (!dialog.open) return null;
  return React.createElement(
    DismissableLayer,
    {
      role: "dialog",
      tabIndex: -1,
      ...rest,
      ref: setRef,
      open: true,
      style: mergeStyles(popupStyle(), style),
      initialFocus,
      finalFocus,
      defaultFocus: (node) => node?.id ?? null,
      preferTrigger: true,
      trigger: () => dialog.trigger.current,
      pressFlag: dialog.pressFlag,
      onEscapeKeyDown,
      onDismiss: (reason, info) => {
        if (dismissalCloses(reason, dialog, info)) dialog.requestOpen(false);
      },
      onKeyDown: composeHandlers(onKeyDown, (event) => {
        if (dialog.modal) trapTab(event, nodeRef.current);
      }),
    },
    children,
  );
});

/** The title: role `heading` at level 2 unless the caller's properties say otherwise. */
export const Title = React.forwardRef(function Title({ asChild = false, children, ...rest }, ref) {
  useDialog("Title");
  return renderSlot({ asChild, children, ref, props: { role: "heading", "aria-level": DIALOG_TITLE_LEVEL, ...rest } });
});

/** The description: an ordinary container. */
export const Description = React.forwardRef(function Description({ asChild = false, children, ...rest }, ref) {
  useDialog("Description");
  return renderSlot({ asChild, children, ref, props: rest });
});
