// The compound tooltip (CMP-004 R5-R10): Provider, Root, Trigger and Content.

import React from "react";
import { composeHandlers, createTooltipProvider, createTooltipRoot, mergeStyles } from "@vespertine/host";
import { useControllableState } from "./controlled.js";
import { DismissableLayer, FloatingLayer } from "./layer.js";
import { usePresence } from "./presence.js";
import { renderSlot } from "./slot.js";

/** Shared state of roots that have no provider. */
const defaultProvider = createTooltipProvider({});

const ProviderContext = React.createContext(defaultProvider);
const RootContext = React.createContext(null);

function useRootContext(part) {
  const context = React.useContext(RootContext);
  if (context === null) throw new Error(`Tooltip.${part} must be used inside Tooltip.Root`);
  return context;
}

/**
 * Settings shared by the roots below it: `delayDuration` (0 ms), `skipDelayDuration` (300 ms)
 * and `disableHoverableContent` (false). The provider remembers the last close time. React
 * propagates changed settings to its roots.
 */
export function Provider({ delayDuration, skipDelayDuration, disableHoverableContent, children }) {
  const settings = React.useRef({});
  settings.current = { delayDuration, skipDelayDuration, disableHoverableContent };
  const [provider] = React.useState(() =>
    createTooltipProvider(() => Object.fromEntries(Object.entries(settings.current).filter(([, value]) => value !== undefined))),
  );
  return React.createElement(ProviderContext.Provider, { value: provider }, children);
}

/**
 * One tooltip: closed by default, controlled (`open`) or uncontrolled (`defaultOpen`), with
 * per-root overrides of `delayDuration` and `disableHoverableContent`. Renders the floating
 * root container the content is positioned against.
 */
export const Root = React.forwardRef(function Root(
  { open: openProp, defaultOpen = false, onOpenChange, delayDuration, disableHoverableContent, children, style, ...rest },
  ref,
) {
  const provider = React.useContext(ProviderContext);
  const [open, requestOpen] = useControllableState({ value: openProp, defaultValue: defaultOpen, onChange: onOpenChange });
  const overrides = React.useRef({});
  overrides.current = { delayDuration, disableHoverableContent };
  const request = React.useRef(requestOpen);
  request.current = requestOpen;
  const controller = React.useMemo(
    () =>
      createTooltipRoot({
        provider,
        overrides: () => overrides.current,
        replaceTimers: true,
        requestOpen: (next) => request.current(next),
      }),
    [provider],
  );
  React.useEffect(() => controller.dispose, [controller]);
  const context = React.useMemo(() => ({ open, controller, hoverable: () => !(disableHoverableContent ?? provider.settings().disableHoverableContent) }), [open, controller, disableHoverableContent, provider]);
  return React.createElement(
    RootContext.Provider,
    { value: context },
    React.createElement(
      "div",
      { ...rest, ref, style: mergeStyles({ display: "flex", position: "relative", alignItems: "flex-start" }, style) },
      children,
    ),
  );
});

/**
 * The element the tooltip describes. Hover opens it after the effective delay, focus opens it at
 * once, blur, press and click close it; caller handlers run first. Tab index defaults to 0;
 * `asChild` merges onto the single element child.
 */
export const Trigger = React.forwardRef(function Trigger(
  { asChild = false, children, tabIndex, onEnter, onLeave, onFocus, onBlur, onPointerDown, onClick, ...rest },
  ref,
) {
  const { controller } = useRootContext("Trigger");
  return renderSlot({
    asChild,
    children,
    ref,
    props: {
      ...rest,
      tabIndex,
      onEnter: composeHandlers(onEnter, controller.triggerEnter),
      onLeave: composeHandlers(onLeave, controller.triggerLeave),
      onFocus: composeHandlers(onFocus, controller.focus),
      onBlur: composeHandlers(onBlur, controller.blur),
      onPointerDown: composeHandlers(onPointerDown, controller.pointerDown),
      onClick: composeHandlers(onClick, controller.click),
    },
    defaults: { tabIndex: 0 },
  });
});

/**
 * The tooltip surface, mounted only while open: side top, alignment center and no offset unless
 * given (floating options follow `FloatingLayer`). It joins the dismissal stack while mounted so
 * Escape closes it, never takes focus, and is pointer-transparent unless the content is
 * hoverable. No role, color or typography is invented beyond the floating defaults.
 */
export const Content = React.forwardRef(function Content(
  { side = "top", align = "center", sideOffset, alignOffset, collisionPadding, style, onEscapeKeyDown, onEnter, onLeave, initial, animate, exit, transition, children, ...rest },
  ref,
) {
  const { open, controller, hoverable } = useRootContext("Content");
  const presence = usePresence(open, { initial, animate, exit, transition });
  if (!presence.mounted) return null;
  // Closing content stays painted but inert until its exit finishes.
  const canHover = hoverable() && !presence.closing;
  return React.createElement(
    FloatingLayer,
    {
      ...rest,
      ref,
      side,
      align,
      sideOffset,
      alignOffset,
      collisionPadding,
      style: mergeStyles(canHover ? undefined : { pointerEvents: "none" }, style),
      onEnter: composeHandlers(onEnter, controller.contentEnter),
      onLeave: composeHandlers(onLeave, controller.contentLeave),
    },
    React.createElement(
      DismissableLayer,
      {
        open,
        initialFocus: false,
        finalFocus: false,
        onEscapeKeyDown,
        onDismiss: () => controller.setOpen(false),
        motion: presence.motion,
        onMotionComplete: presence.motion ? presence.onMotionComplete : undefined,
      },
      children,
    ),
  );
});
