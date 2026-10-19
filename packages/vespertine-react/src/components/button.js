// The headless Button and the behavior-only buttonProps helper (CMP-001 R1-R6).

import React from "react";
import { buttonProps as behavior, createButtonLatch, resolveStyle } from "@vespertine/host";
import { renderSlot } from "./slot.js";

/** The per-control Enter latch (CMP-001 R2) that lives as long as the component. */
export function useButtonLatch() {
  return React.useState(createButtonLatch)[0];
}

/**
 * Behavior-only helper: the role, tab index, disabled behavior and events of a button for any
 * other control (see the host package's `buttonProps`; pass `latch` from `useButtonLatch`).
 */
export function buttonProps(options) {
  return behavior(options);
}

/**
 * A style-free button: a focusable generic container with role `button`, tab index 0 (-1 while
 * disabled unless `focusableWhenDisabled`), activation by click, Enter and Space, and no
 * imposed colors, dimensions, borders or typography. `style` is an object or a function of
 * `{ disabled }`. `asChild` merges the part onto its single element child.
 */
export const Button = React.forwardRef(function Button(
  { disabled = false, focusableWhenDisabled = false, tabIndex, asChild = false, children, style, onClick, onKeyDown, onKeyUp, ...rest },
  ref,
) {
  const latch = useButtonLatch();
  const props = behavior({ disabled, focusableWhenDisabled, tabIndex, onClick, onKeyDown, onKeyUp, latch });
  return renderSlot({
    asChild,
    children,
    ref,
    props: {
      ...rest,
      role: props.role,
      tabIndex,
      onClick: props.onClick,
      onKeyDown: props.onKeyDown,
      onKeyUp: props.onKeyUp,
      style: resolveStyle(style, { disabled }),
    },
    defaults: { tabIndex: props.tabIndex },
  });
});
