// Floating surfaces and dismissal layers (CMP-002 R1-R10, R17).
//
// Solid difference (CMP-002 R3): the caller `style` applies ONLY to the inner content
// container. The outer anchored surface receives no style: opacity, visibility and radii are
// not routed to it (React splits them out; see the React adapter's `splitFloatingStyle`). The
// anchored property mapping and the defaults are otherwise identical.
//
// The layer registers after its content is attached: the native tree is flushed first so the
// focus target exists natively, and the focus move of a closing layer happens when `open`
// turns false (or the layer is disposed), not when the content is finally removed.

import { batch, createContext, createEffect, mergeProps, onCleanup, splitProps, untrack, useContext } from "solid-js";
import { defaultFinalFocus, mergeStyles, pushDismissLayer, resolveFocusTarget } from "@vespertine/host";
import { createComponent, createElement, insert, spread } from "../renderer.js";
import { useRenderer } from "../root.js";
import { renderSlot } from "./slot.js";

/** The default fill of floating content (CMP-002 R2). */
export const FLOATING_FILL = "#1A1A1A";

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
 * relative positioning and start cross-axis alignment; the caller's style overrides them (and is
 * the only style: the outer anchored surface gets none). The anchored surface occludes the
 * pointer targets beneath it unless the caller sets `pointerEvents` to `none`. Ordinary host
 * properties go to the content container.
 */
export function FloatingLayer(props) {
  const [local, rest] = splitProps(props, ["side", "align", "sideOffset", "alignOffset", "collisionPadding", "style", "children", "asChild"]);
  const anchored = createElement("anchored");
  spread(
    anchored,
    () =>
      anchoredProps({
        side: local.side,
        align: local.align,
        sideOffset: local.sideOffset,
        alignOffset: local.alignOffset,
        collisionPadding: local.collisionPadding,
        occlude: local.style?.pointerEvents !== "none",
      }),
    true,
  );
  const content = renderSlot({
    asChild: local.asChild,
    children: () => local.children,
    props: mergeProps(rest, {
      get style() {
        return mergeStyles({ display: "flex", position: "relative", alignItems: "flex-start", backgroundColor: FLOATING_FILL }, local.style);
      },
    }),
  });
  insert(anchored, content);
  return anchored;
}

/** The enclosing dismissal layer, so nested layers declare their ancestry. */
const LayerContext = createContext(null);

/**
 * A dismissal layer: it joins the renderer's window-scoped stack while `open` (default true) and
 * renders a container for its children (host properties are forwarded to it).
 *
 * - Only the top layer receives an unprevented Escape. `onEscapeKeyDown(event)` runs first and
 *   may `preventDefault()` to keep the layer open; otherwise `onDismiss("escape")` runs.
 * - An outside press calls `onDismiss("outside")`; `pressFlag` (a trigger's press flag) makes it
 *   ignore a press on the layer's own trigger.
 * - `initialFocus` and `finalFocus` accept a node, a node reference, a boolean or a function
 *   (`finalFocus` receives the focus owner captured at open time); unset or `true` selects the
 *   default: `defaultFocus()` for the initial focus, and for the final focus the captured owner
 *   if focus is still inside the layer or nowhere (falling back to `trigger()`), so a press that
 *   moved focus elsewhere keeps it.
 * - While `open` turns false the layer closes at once (focus restoration included) even if the
 *   caller keeps the content mounted for an exit animation.
 *
 * @param {object} props
 */
export function DismissableLayer(props) {
  const [local, rest] = splitProps(props, [
    "open",
    "children",
    "onEscapeKeyDown",
    "onDismiss",
    "initialFocus",
    "finalFocus",
    "defaultFocus",
    "trigger",
    "pressFlag",
    "onOutsideDown",
    "ref",
  ]);
  const renderer = useRenderer();
  const isOpen = () => local.open ?? true;
  let node = null;
  // The layer object exists before the content so that a child registering before its parent
  // can still declare its ancestry.
  const layer = { parent: useContext(LayerContext), onEscape() {}, initialFocus: () => null, finalFocus: () => null };

  createEffect(() => {
    if (!isOpen()) return;
    layer.onEscape = () => {
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
      local.onEscapeKeyDown?.(event);
      if (!prevented) local.onDismiss?.("escape");
    };
    layer.initialFocus = () => resolveFocusTarget(local.initialFocus, () => local.defaultFocus?.(node) ?? null);
    layer.finalFocus = (previous) => {
      const fallback = defaultFinalFocus(renderer, { layer: () => node, trigger: local.trigger });
      const choice = local.finalFocus;
      const target = typeof choice === "function" ? () => choice(previous) : choice;
      return resolveFocusTarget(target, () => fallback(previous));
    };
    // The focus target must exist natively before the layer moves focus to it.
    node?.tree?.flush();
    const close = untrack(() => pushDismissLayer(renderer, layer));
    onCleanup(() => untrack(close));
  });

  const handleOutside = (event) => {
    local.onOutsideDown?.(event);
    // The deferred dismissal runs outside the event dispatch, so it applies synchronously itself.
    const tree = node?.tree;
    const dismiss = () => {
      try {
        batch(() => local.onDismiss?.("outside"));
      } finally {
        tree?.flush();
      }
    };
    if (local.pressFlag) local.pressFlag.defer(dismiss);
    else local.onDismiss?.("outside");
  };

  return createComponent(LayerContext.Provider, {
    value: layer,
    get children() {
      return renderSlot({
        children: () => local.children,
        props: mergeProps(rest, {
          ref(element) {
            node = element;
            local.ref?.(element);
          },
          get onOutsideDown() {
            return isOpen() ? handleOutside : undefined;
          },
        }),
      });
    },
  });
}
