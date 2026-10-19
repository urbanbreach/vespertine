// Child composition for parts (CMP-001 R5, R6): a part renders a generic container, or with
// `asChild` merges onto its single intrinsic host element child.
//
// Differences from the React adapter (documented, not normalized):
//
// - `asChild` requires exactly one intrinsic host element (a host node of the universal
//   renderer that is not a raw text node). Zero, several, text or non-host children throw
//   `asChild requires exactly one intrinsic host element child`. The child's own children are
//   preserved and the child is resolved once, when the part is created.
// - The part's properties are spread onto the existing element with the renderer's `spread` and
//   `mergeProps`, so property getters of the part stay reactive.
// - Only function-valued `on`-prefixed properties supplied by the part and already present on
//   the child are composed (child handler first, then the part's; the second always runs even
//   when the first prevented the default). Every other property spread onto the element, the
//   `style` included, REPLACES the child's: there is no shallow style merge.
// - The tab index precedence is the React one: explicit part tab index, then the child's, then
//   the part default.
// - Refs are host-element callbacks: the part's `ref` is called with the element; the child's
//   own `ref` already ran when the child was created.

import { children as resolveChildren } from "solid-js";
import { composeHandlers, isHostNode } from "@vespertine/host";
import { createElement, mergeProps, spread } from "../renderer.js";

const EVENT_PROP = /^on[A-Z]/;

/** Handler getters of `part` for the handlers the child already has: child first, then the part's. */
function composedHandlers(child, part) {
  const composed = {};
  for (const name of Object.keys(part)) {
    if (!EVENT_PROP.test(name)) continue;
    const existing = child.props[name];
    if (typeof existing !== "function") continue;
    Object.defineProperty(composed, name, {
      enumerable: true,
      get: () => composeHandlers(existing, part[name]),
    });
  }
  return composed;
}

/**
 * Renders a part.
 *
 * Without `asChild` the part is a new `type` element (default `div`) holding the children and
 * the part's properties. With `asChild` the part's properties are merged onto the single
 * intrinsic host element child, which is returned.
 *
 * `children` is a function returning the children (it is read lazily, once, through Solid's
 * `children` helper); `props` may carry getters and a `ref` callback. `defaults.tabIndex` may
 * be a getter.
 *
 * @param {{ asChild?: boolean, children?: () => unknown, type?: string, props?: object,
 *           defaults?: { tabIndex?: number } }} part
 * @returns the host element
 */
export function renderSlot({ asChild = false, children = () => undefined, type = "div", props = {}, defaults = {} }) {
  const resolved = resolveChildren(children);
  const tabIndex = () => props.tabIndex ?? defaults.tabIndex;
  if (!asChild) {
    const element = createElement(type);
    spread(
      element,
      mergeProps(props, {
        get tabIndex() {
          return tabIndex();
        },
        get children() {
          return resolved;
        },
      }),
    );
    return element;
  }
  const only = resolved.toArray();
  if (only.length !== 1 || !isHostNode(only[0]) || only[0].rawText) {
    throw new Error("asChild requires exactly one intrinsic host element child");
  }
  const child = only[0];
  const childTabIndex = child.props.tabIndex;
  spread(
    child,
    mergeProps(props, composedHandlers(child, props), {
      get tabIndex() {
        return props.tabIndex ?? childTabIndex ?? defaults.tabIndex;
      },
    }),
    true,
  );
  return child;
}
