// The headless compound Dialog (CMP-003 R1-R10): Root, Trigger, Portal, Backdrop, Popup, Title,
// Description and Close.
//
// Root renders no element of its own; it only provides state. The portal and the popup mount only
// while open. The popup is a dismissal layer: Escape closes it only when it is the top layer (a
// select opened from inside closes first), a modal popup traps Tab, and closing returns focus to
// the trigger (or the element focused before opening).
//
// Solid notes: the parts read the root's options reactively (a later change of `modal` or of a
// dismissal switch applies at once); `initialFocus` and `finalFocus` accept the host element a
// `ref` callback delivered, a function, a boolean or null.

import { Show, createContext, createMemo, mergeProps, splitProps, useContext } from "solid-js";
import {
  DIALOG_TITLE_LEVEL,
  backdropStyle,
  buttonProps as behavior,
  composeHandlers,
  createButtonLatch,
  createPressFlag,
  dismissalCloses,
  mergeStyles,
  popupStyle,
  portalProps,
  resolveStyle,
  trapTab,
} from "@vespertine/host";
import { createComponent } from "../renderer.js";
import { createControllableState } from "./controlled.js";
import { DismissableLayer } from "./layer.js";
import { renderSlot } from "./slot.js";

const DialogContext = createContext(null);

function useDialog(part) {
  const context = useContext(DialogContext);
  if (context === null) throw new Error(`Dialog.${part} must be used inside Dialog.Root`);
  return context;
}

/**
 * The dialog state: `open`/`defaultOpen`/`onOpenChange` (closed by default; controlled when `open`
 * is given), `modal` (true) and `disablePointerDismissal` (false), plus `disableEscapeDismissal`
 * (false: Escape still never reaches the page, it just does not close the dialog). Renders no
 * element.
 */
export function Root(props) {
  const [open, requestOpen] = createControllableState({
    value: () => props.open,
    defaultValue: props.defaultOpen ?? false,
    onChange: (next) => props.onOpenChange?.(next),
  });
  const context = {
    open,
    requestOpen,
    modal: () => props.modal ?? true,
    policy: () => ({
      modal: props.modal ?? true,
      disablePointerDismissal: props.disablePointerDismissal ?? false,
      disableEscapeDismissal: props.disableEscapeDismissal ?? false,
    }),
    disablePointerDismissal: () => props.disablePointerDismissal ?? false,
    state: { trigger: null },
    pressFlag: createPressFlag(),
  };
  return createComponent(DialogContext.Provider, {
    value: context,
    get children() {
      return props.children;
    },
  });
}

/** A button-like part that requests an open state after the caller's activation. */
function activationPart(name, opens) {
  const Part = (props) => {
    const dialog = useDialog(name);
    const [local, rest] = splitProps(props, [
      "disabled",
      "asChild",
      "children",
      "style",
      "tabIndex",
      "onClick",
      "onKeyDown",
      "onKeyUp",
      "onPointerDown",
      "ref",
    ]);
    const latch = createButtonLatch();
    const disabled = () => local.disabled ?? false;
    // Caller handlers are read when the event arrives, so the memo only follows the options.
    const button = createMemo(() =>
      behavior({
        disabled: disabled(),
        tabIndex: local.tabIndex,
        latch,
        onClick: (event) => {
          local.onClick?.(event);
          dialog.requestOpen(opens);
        },
        onKeyDown: (event) => local.onKeyDown?.(event),
        onKeyUp: (event) => local.onKeyUp?.(event),
      }),
    );
    const triggerOnly = opens
      ? {
          get "aria-expanded"() {
            return dialog.open();
          },
          get onPointerDown() {
            return composeHandlers(local.onPointerDown, () => dialog.pressFlag.raise());
          },
        }
      : {
          get onPointerDown() {
            return local.onPointerDown;
          },
        };
    return renderSlot({
      asChild: local.asChild,
      children: () => local.children,
      props: mergeProps(rest, triggerOnly, {
        role: "button",
        get tabIndex() {
          return local.tabIndex;
        },
        get style() {
          return resolveStyle(local.style, { open: dialog.open(), disabled: disabled() });
        },
        ref(node) {
          if (opens) dialog.state.trigger = node;
          local.ref?.(node);
        },
        get onClick() {
          return button().onClick;
        },
        get onKeyDown() {
          return button().onKeyDown;
        },
        get onKeyUp() {
          return button().onKeyUp;
        },
      }),
      defaults: {
        get tabIndex() {
          return button().tabIndex;
        },
      },
    });
  };
  Object.defineProperty(Part, "name", { value: name });
  return Part;
}

/**
 * The control that opens the dialog: button keyboard behavior, `aria-expanded` for the open
 * state, the caller's activation first and then a request for `open = true` (it never toggles).
 * `style` is an object or a function of `{ open, disabled }`; `asChild` merges onto the host
 * element child.
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
export function Portal(props) {
  const dialog = useDialog("Portal");
  const [local, rest] = splitProps(props, ["style", "children"]);
  const base = createMemo(() => portalProps({ modal: dialog.modal() }));
  return createComponent(Show, {
    get when() {
      return dialog.open();
    },
    get children() {
      return renderSlot({
        type: "anchored",
        children: () => local.children,
        props: mergeProps(rest, {
          get fill() {
            return base().fill;
          },
          get deferred() {
            return base().deferred;
          },
          get priority() {
            return base().priority;
          },
          get occlude() {
            return base().occlude;
          },
          get style() {
            return mergeStyles(base().style, local.style);
          },
        }),
      });
    },
  });
}

/**
 * The backdrop: absolute with all four insets zero. A pointer press runs the caller's
 * `onPointerDown` first and then closes the dialog unless `disablePointerDismissal`. No color is
 * imposed.
 */
export function Backdrop(props) {
  const dialog = useDialog("Backdrop");
  const [local, rest] = splitProps(props, ["style", "onPointerDown", "asChild", "children"]);
  return renderSlot({
    asChild: local.asChild,
    children: () => local.children,
    props: mergeProps(rest, {
      get style() {
        return mergeStyles(backdropStyle({ modal: dialog.modal() }), local.style);
      },
      get onPointerDown() {
        return composeHandlers(local.onPointerDown, () => {
          if (!dialog.disablePointerDismissal()) dialog.requestOpen(false);
        });
      },
    }),
  });
}

/**
 * The dialog surface, mounted only while open. Defaults: role `dialog`, tab index -1, pointer
 * events on (a press inside never reaches the backdrop) and no keyboard focus dim; nothing else
 * is styled. `initialFocus` (default: the popup itself) and `finalFocus` (default: the trigger
 * when mounted, else the element focused before opening) follow the focus-target contract.
 * A modal popup traps Tab inside itself after the caller's key-down; a nonmodal one does not.
 */
export function Popup(props) {
  const dialog = useDialog("Popup");
  const [local, rest] = splitProps(props, ["style", "initialFocus", "finalFocus", "onEscapeKeyDown", "onKeyDown", "children", "ref"]);
  let node = null;
  return createComponent(Show, {
    get when() {
      return dialog.open();
    },
    get children() {
      return createComponent(
        DismissableLayer,
        mergeProps({ role: "dialog", tabIndex: -1 }, rest, {
          open: true,
          get style() {
            return mergeStyles(popupStyle(), local.style);
          },
          get initialFocus() {
            return local.initialFocus;
          },
          get finalFocus() {
            return local.finalFocus;
          },
          defaultFocus: (element) => element?.id ?? null,
          preferTrigger: true,
          trigger: () => dialog.state.trigger,
          pressFlag: dialog.pressFlag,
          get onEscapeKeyDown() {
            return local.onEscapeKeyDown;
          },
          onDismiss: (reason, info) => {
            if (dismissalCloses(reason, dialog.policy(), info)) dialog.requestOpen(false);
          },
          ref(element) {
            node = element;
            local.ref?.(element);
          },
          get onKeyDown() {
            return composeHandlers(local.onKeyDown, (event) => {
              if (dialog.modal()) trapTab(event, node);
            });
          },
          get children() {
            return local.children;
          },
        }),
      );
    },
  });
}

/** The title: role `heading` at level 2 unless the caller's properties say otherwise. */
export function Title(props) {
  useDialog("Title");
  const [local, rest] = splitProps(props, ["asChild", "children"]);
  return renderSlot({
    asChild: local.asChild,
    children: () => local.children,
    props: mergeProps({ role: "heading", "aria-level": DIALOG_TITLE_LEVEL }, rest),
  });
}

/** The description: an ordinary container. */
export function Description(props) {
  useDialog("Description");
  const [local, rest] = splitProps(props, ["asChild", "children"]);
  return renderSlot({ asChild: local.asChild, children: () => local.children, props: rest });
}
