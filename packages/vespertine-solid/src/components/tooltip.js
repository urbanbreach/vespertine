// The compound tooltip (CMP-004 R5-R10): Provider, Root, Trigger and Content.
//
// Solid differences from the React adapter (documented, not normalized):
//
// - The provider settings (`delayDuration`, `skipDelayDuration`, `disableHoverableContent`) are
//   CAPTURED when the Provider is constructed, and a root's hoverability (its own
//   `disableHoverableContent`, else the provider's) is captured when the Root is constructed:
//   later property changes do not reach them. (A root's `delayDuration` override is still read
//   whenever a delay is computed.)
// - Timers use `replaceTimers: false`: scheduling a timer of a kind overwrites the tracked
//   handle WITHOUT cancelling a timer of the same kind that is already scheduled. Two hover
//   enters (with a positive delay) therefore leave two pending timers, both of which fire; only
//   the latest one stays tracked, so a leave or a state request cancels only that one. React
//   cancels the earlier timer and leaves one. This is not "cancelling every pending timeout".
// - Unmounting the root (`onCleanup`) cancels the tracked timers only.
// - Content mounts only while open (`Show`), registers a dismissal layer while open and never
//   takes focus; the exit presence follows CMP-002 R17.

import { Show, createContext, mergeProps, onCleanup, splitProps, useContext } from "solid-js";
import { composeHandlers, createTooltipProvider, createTooltipRoot, mergeStyles } from "@vespertine/host";
import { createComponent } from "../renderer.js";
import { createControllableState } from "./controlled.js";
import { DismissableLayer, FloatingLayer } from "./layer.js";
import { createPresence } from "./presence.js";
import { renderSlot } from "./slot.js";

/** Shared state of roots that have no provider. */
const defaultProvider = createTooltipProvider({});

const ProviderContext = createContext(defaultProvider);
const RootContext = createContext(null);

function useRootContext(part) {
  const context = useContext(RootContext);
  if (context === null) throw new Error(`Tooltip.${part} must be used inside Tooltip.Root`);
  return context;
}

/**
 * Settings shared by the roots below it: `delayDuration` (0 ms), `skipDelayDuration` (300 ms)
 * and `disableHoverableContent` (false), captured when the provider is constructed. The provider
 * remembers the last close time.
 */
export function Provider(props) {
  const settings = {};
  for (const name of ["delayDuration", "skipDelayDuration", "disableHoverableContent"]) {
    if (props[name] !== undefined) settings[name] = props[name];
  }
  const provider = createTooltipProvider(settings);
  return createComponent(ProviderContext.Provider, {
    value: provider,
    get children() {
      return props.children;
    },
  });
}

/**
 * One tooltip: closed by default, controlled (`open`) or uncontrolled (`defaultOpen`), with
 * per-root overrides of `delayDuration` and `disableHoverableContent` (hoverability is captured
 * at construction). Renders the floating root container the content is positioned against.
 */
export function Root(props) {
  const [local, rest] = splitProps(props, [
    "open",
    "defaultOpen",
    "onOpenChange",
    "delayDuration",
    "disableHoverableContent",
    "children",
    "style",
  ]);
  const provider = useContext(ProviderContext);
  const [open, requestOpen] = createControllableState({
    value: () => local.open,
    defaultValue: local.defaultOpen ?? false,
    onChange: (next) => local.onOpenChange?.(next),
  });
  const hoverable = !(local.disableHoverableContent ?? provider.settings().disableHoverableContent);
  const controller = createTooltipRoot({
    provider,
    overrides: () => ({ delayDuration: local.delayDuration, disableHoverableContent: !hoverable }),
    replaceTimers: false,
    requestOpen,
  });
  onCleanup(controller.dispose);
  return createComponent(RootContext.Provider, {
    value: { open, controller, hoverable },
    get children() {
      return renderSlot({
        children: () => local.children,
        props: mergeProps(rest, {
          get style() {
            return mergeStyles({ display: "flex", position: "relative", alignItems: "flex-start" }, local.style);
          },
        }),
      });
    },
  });
}

/**
 * The element the tooltip describes. Hover opens it after the effective delay, focus opens it at
 * once, blur, press and click close it; caller handlers run first. Tab index defaults to 0;
 * `asChild` merges onto the single intrinsic host element child.
 */
export function Trigger(props) {
  const { controller } = useRootContext("Trigger");
  const [local, rest] = splitProps(props, [
    "asChild",
    "children",
    "tabIndex",
    "onEnter",
    "onLeave",
    "onFocus",
    "onBlur",
    "onPointerDown",
    "onClick",
  ]);
  return renderSlot({
    asChild: local.asChild,
    children: () => local.children,
    props: mergeProps(rest, {
      get tabIndex() {
        return local.tabIndex;
      },
      get onEnter() {
        return composeHandlers(local.onEnter, controller.triggerEnter);
      },
      get onLeave() {
        return composeHandlers(local.onLeave, controller.triggerLeave);
      },
      get onFocus() {
        return composeHandlers(local.onFocus, controller.focus);
      },
      get onBlur() {
        return composeHandlers(local.onBlur, controller.blur);
      },
      get onPointerDown() {
        return composeHandlers(local.onPointerDown, controller.pointerDown);
      },
      get onClick() {
        return composeHandlers(local.onClick, controller.click);
      },
    }),
    defaults: { tabIndex: 0 },
  });
}

/**
 * The tooltip surface, mounted only while open: side top, alignment center and no offset unless
 * given (floating options follow `FloatingLayer`). It joins the dismissal stack while mounted so
 * Escape closes it, never takes focus, and is pointer-transparent unless the content is
 * hoverable. No role, color or typography is invented beyond the floating defaults.
 */
export function Content(props) {
  const { open, controller, hoverable } = useRootContext("Content");
  const [local, rest] = splitProps(props, [
    "side",
    "align",
    "sideOffset",
    "alignOffset",
    "collisionPadding",
    "style",
    "onEscapeKeyDown",
    "onEnter",
    "onLeave",
    "initial",
    "animate",
    "exit",
    "transition",
    "children",
  ]);
  const presence = createPresence(open, () => ({
    initial: local.initial,
    animate: local.animate,
    exit: local.exit,
    transition: local.transition,
  }));
  return createComponent(Show, {
    get when() {
      return presence.mounted();
    },
    get children() {
      return createComponent(
        FloatingLayer,
        mergeProps(rest, {
          get side() {
            return local.side ?? "top";
          },
          get align() {
            return local.align ?? "center";
          },
          get sideOffset() {
            return local.sideOffset;
          },
          get alignOffset() {
            return local.alignOffset;
          },
          get collisionPadding() {
            return local.collisionPadding;
          },
          get style() {
            // Closing content stays painted but inert until its exit finishes.
            const canHover = hoverable && !presence.closing();
            return mergeStyles(canHover ? undefined : { pointerEvents: "none" }, local.style);
          },
          get onEnter() {
            return composeHandlers(local.onEnter, controller.contentEnter);
          },
          get onLeave() {
            return composeHandlers(local.onLeave, controller.contentLeave);
          },
          get children() {
            return createComponent(DismissableLayer, {
              get open() {
                return open();
              },
              initialFocus: false,
              finalFocus: false,
              get onEscapeKeyDown() {
                return local.onEscapeKeyDown;
              },
              onDismiss: () => controller.setOpen(false),
              get motion() {
                return presence.motion();
              },
              get onMotionComplete() {
                return presence.motion() ? presence.onMotionComplete : undefined;
              },
              get children() {
                return local.children;
              },
            });
          },
        }),
      );
    },
  });
}
