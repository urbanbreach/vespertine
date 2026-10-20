// Floating surfaces and dismissal layers (CMP-002 R1-R10, R17).

import React from "react";
import { defaultFinalFocus, isTopDismissLayer, mergeStyles, pushDismissLayer, resolveFocusTarget } from "@vespertine/host";
import { flushSync, useRenderer } from "../root.js";
import { renderSlot } from "./slot.js";

/** The default fill of floating content (CMP-002 R2). */
export const FLOATING_FILL = "#1A1A1A";

/** Style properties that reach the outer anchored surface (CMP-002 R3). */
const OUTER_KEYS = [
  "visibility",
  "opacity",
  "borderRadius",
  "borderTopLeftRadius",
  "borderTopRightRadius",
  "borderBottomLeftRadius",
  "borderBottomRightRadius",
];

function pick(style, keys) {
  const picked = {};
  for (const key of keys) if (style[key] !== undefined) picked[key] = style[key];
  return picked;
}

function without(style, key) {
  const { [key]: _removed, ...rest } = style;
  return rest;
}

/**
 * Splits a caller style for the floating surface (React, CMP-002 R3). The outer anchored
 * surface receives visibility, opacity and the uniform and per-corner radii with their
 * hover and active overrides; opacity (including its hover and active overrides) is removed
 * from the inner content so it is applied exactly once; all other style stays on the inner
 * container.
 */
export function splitFloatingStyle(style = {}) {
  const outer = pick(style, OUTER_KEYS);
  const inner = without(style, "opacity");
  for (const state of ["hover", "active"]) {
    const nested = style[state];
    if (typeof nested !== "object" || nested === null) continue;
    const picked = pick(nested, OUTER_KEYS);
    if (Object.keys(picked).length > 0) outer[state] = picked;
    inner[state] = without(nested, "opacity");
  }
  return { outer, inner };
}

/**
 * The properties of the native `anchored` host for the floating options: the side offset is the
 * anchor gap, the alignment offset acts horizontally for top and bottom and vertically for left
 * and right, positions snap to the window with the collision padding as margin, and the layer
 * is deferred with priority 1.
 */
export function anchoredProps({ side = "bottom", align = "start", sideOffset = 0, alignOffset = 0, collisionPadding = 8, occlude = true } = {}) {
  const horizontal = side === "top" || side === "bottom";
  return {
    side,
    align,
    gap: sideOffset,
    offset: horizontal ? { x: alignOffset, y: 0 } : { x: 0, y: alignOffset },
    fit: "snap",
    snapMargin: collisionPadding,
    deferred: true,
    priority: 1,
    occlude,
  };
}

/**
 * Content positioned relative to its trigger (the nearest enclosing floating root). Options:
 * `side` (default bottom), `align` (start), `sideOffset` (0), `alignOffset` (0) and
 * `collisionPadding` (8). The content container defaults to an opaque #1A1A1A fill, flex display,
 * relative positioning and start cross-axis alignment; the caller's style overrides them. The
 * anchored surface occludes the pointer targets beneath it unless the caller sets `pointerEvents`
 * to `none`. Ordinary host properties go to the content container.
 */
export const FloatingLayer = React.forwardRef(function FloatingLayer(
  { side, align, sideOffset, alignOffset, collisionPadding, style, children, asChild = false, ...rest },
  ref,
) {
  const { outer, inner } = splitFloatingStyle(style);
  const content = renderSlot({
    asChild,
    children,
    ref,
    props: {
      ...rest,
      style: mergeStyles({ display: "flex", position: "relative", alignItems: "flex-start", backgroundColor: FLOATING_FILL }, inner),
    },
  });
  return React.createElement(
    "anchored",
    { ...anchoredProps({ side, align, sideOffset, alignOffset, collisionPadding, occlude: style?.pointerEvents !== "none" }), style: outer },
    content,
  );
});

/** The enclosing dismissal layer, so nested layers declare their ancestry. */
const LayerContext = React.createContext(null);

/**
 * A dismissal layer: it joins the renderer's window-scoped stack while `open` and renders a
 * container for its children (host properties are forwarded to it).
 *
 * - Only the top layer receives an unprevented Escape. `onEscapeKeyDown(event)` runs first and
 *   may `preventDefault()` to keep the layer open; otherwise `onDismiss("escape")` runs.
 * - An outside press calls `onDismiss("outside", { top })` (`top`: whether the layer is the top
 *   of the stack, so presses belonging to a nested layer can be told apart); `pressFlag` (a
 *   trigger's press flag) makes it ignore a press on the layer's own trigger.
 * - `initialFocus` and `finalFocus` accept a node, a node reference, a boolean or a function
 *   (`finalFocus` receives the focus owner captured at open time); unset or `true` selects the
 *   default: `defaultFocus()` for the initial focus, and for the final focus the captured owner
 *   if focus is still inside the layer or nowhere (falling back to `trigger()`), so a press that
 *   moved focus elsewhere keeps it; with `preferTrigger` a mounted trigger comes first.
 * - While `open` turns false the layer closes at once (focus restoration included) even if the
 *   caller keeps the content mounted for an exit animation.
 *
 * @param {object} props
 */
export const DismissableLayer = React.forwardRef(function DismissableLayer(
  { open = true, children, onEscapeKeyDown, onDismiss, initialFocus, finalFocus, defaultFocus, trigger, preferTrigger, pressFlag, onOutsideDown, ...rest },
  ref,
) {
  const renderer = useRenderer();
  const parent = React.useContext(LayerContext);
  const nodeRef = React.useRef(null);
  // The layer object exists from the first render so that a child registering before its
  // parent (child effects run first) can still declare its ancestry.
  const [layer] = React.useState(() => ({ parent, onEscape() {}, initialFocus: () => null, finalFocus: () => null }));
  const options = React.useRef(null);
  options.current = { onEscapeKeyDown, onDismiss, initialFocus, finalFocus, defaultFocus, trigger, preferTrigger, pressFlag, onOutsideDown };

  React.useEffect(() => {
    if (!open) return undefined;
    layer.onEscape = () => {
      const { onEscapeKeyDown, onDismiss } = options.current;
      let prevented = false;
      const event = {
        key: "escape",
        preventDefault() {
          prevented = true;
        },
        get defaultPrevented() {
          return prevented;
        },
      };
      onEscapeKeyDown?.(event);
      if (!prevented) onDismiss?.("escape");
    };
    layer.initialFocus = () =>
      resolveFocusTarget(options.current.initialFocus, () => options.current.defaultFocus?.(nodeRef.current) ?? null);
    layer.finalFocus = (previous) => {
      const fallback = defaultFinalFocus(renderer, {
        layer: () => nodeRef.current,
        trigger: options.current.trigger,
        triggerFirst: options.current.preferTrigger === true,
      });
      const choice = options.current.finalFocus;
      const target = typeof choice === "function" ? () => choice(previous) : choice;
      return resolveFocusTarget(target, () => fallback(previous));
    };
    // The focus target must exist natively before the layer moves focus to it (a layer that is
    // open on the very first commit registers before that commit's batch was flushed).
    nodeRef.current?.tree?.flush();
    return pushDismissLayer(renderer, layer);
  }, [open, renderer, layer]);

  const handleOutside = React.useCallback((event) => {
    const { onOutsideDown, onDismiss, pressFlag } = options.current;
    onOutsideDown?.(event);
    const info = { top: isTopDismissLayer(renderer, layer) };
    // The deferred dismissal runs outside the event dispatch, so it applies synchronously itself.
    if (pressFlag) pressFlag.defer(() => flushSync(() => onDismiss?.("outside", info)));
    else onDismiss?.("outside", info);
  }, [renderer, layer]);

  const setRef = React.useCallback(
    (node) => {
      nodeRef.current = node;
      if (typeof ref === "function") ref(node);
      else if (ref) ref.current = node;
    },
    [ref],
  );

  return React.createElement(
    LayerContext.Provider,
    { value: layer },
    React.createElement("div", { ...rest, ref: setRef, onOutsideDown: open ? handleOutside : rest.onOutsideDown }, children),
  );
});
