// Child composition for parts (CMP-001 R5, R6): a part renders a generic container, or with
// `asChild` merges onto its single React element child.

import React from "react";
import { composeRefs, mergeSlotProps, mergeStyles } from "@vespertine/host";

/**
 * Renders a part.
 *
 * Without `asChild` the part is a `div` (or `type`) holding `children`. With `asChild`, exactly
 * one React element child is required and the part's properties are merged onto it: child
 * handlers run before the part's, an explicit part tab index beats the child's, which beats
 * `defaults.tabIndex`, styles are merged shallowly with the part's winning, and both the child's
 * ref and `ref` are delivered. Invalid children throw.
 *
 * @param {{ asChild?: boolean, children?: unknown, type?: string, props: object,
 *           defaults?: { tabIndex?: number }, ref?: unknown }} part
 */
export function renderSlot({ asChild = false, children, type = "div", props, defaults = {}, ref }) {
  if (!asChild) {
    const tabIndex = props.tabIndex ?? defaults.tabIndex;
    const own = tabIndex === undefined ? props : { ...props, tabIndex };
    return React.createElement(type, ref === undefined ? own : { ...own, ref }, children);
  }
  const only = React.Children.toArray(children);
  if (only.length !== 1 || !React.isValidElement(only[0])) {
    throw new Error("asChild requires exactly one React element child");
  }
  const child = only[0];
  const merged = mergeSlotProps(child.props, props, { defaults, mergeStyle: mergeStyles });
  const childRef = child.props.ref ?? child.ref ?? undefined;
  const composed = composeRefs(childRef, ref);
  return React.cloneElement(child, composed === undefined ? merged : { ...merged, ref: composed });
}
