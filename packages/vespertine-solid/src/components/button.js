// The headless Button and the behavior-only buttonProps helper (CMP-001 R1-R6).
//
// Solid differences: the component never destructures its props, so `disabled`,
// `focusableWhenDisabled`, `tabIndex` and `style` stay reactive; `asChild` follows `renderSlot`
// (one intrinsic host element child, properties spread onto it, style replaced, not merged).

import { createMemo, mergeProps, splitProps } from "solid-js";
import { buttonProps as behavior, createButtonLatch, resolveStyle } from "@vespertine/host";
import { renderSlot } from "./slot.js";

export { createButtonLatch };

/**
 * Behavior-only helper: the role, tab index, disabled behavior and events of a button for any
 * other control (see the host package's `buttonProps`; pass a `latch` from `createButtonLatch`
 * that lives as long as the control).
 */
export function buttonProps(options) {
  return behavior(options);
}

/**
 * A style-free button: a focusable generic container with role `button`, tab index 0 (-1 while
 * disabled unless `focusableWhenDisabled`), activation by click, Enter and Space, and no
 * imposed colors, dimensions, borders or typography. `style` is an object or a function of
 * `{ disabled }`. `asChild` merges the part onto its single intrinsic host element child.
 */
export function Button(props) {
  const [local, rest] = splitProps(props, [
    "disabled",
    "focusableWhenDisabled",
    "tabIndex",
    "asChild",
    "children",
    "style",
    "onClick",
    "onKeyDown",
    "onKeyUp",
  ]);
  const latch = createButtonLatch();
  const disabled = () => local.disabled ?? false;
  // Caller handlers are read when the event arrives, so the memo only follows the options.
  const behaviorOf = createMemo(() =>
    behavior({
      disabled: disabled(),
      focusableWhenDisabled: local.focusableWhenDisabled ?? false,
      tabIndex: local.tabIndex,
      onClick: (event) => local.onClick?.(event),
      onKeyDown: (event) => local.onKeyDown?.(event),
      onKeyUp: (event) => local.onKeyUp?.(event),
      latch,
    }),
  );
  return renderSlot({
    asChild: local.asChild,
    children: () => local.children,
    props: mergeProps(rest, {
      get role() {
        return behaviorOf().role;
      },
      get tabIndex() {
        return local.tabIndex;
      },
      get onClick() {
        return behaviorOf().onClick;
      },
      get onKeyDown() {
        return behaviorOf().onKeyDown;
      },
      get onKeyUp() {
        return behaviorOf().onKeyUp;
      },
      get style() {
        return resolveStyle(local.style, { disabled: disabled() });
      },
    }),
    defaults: {
      get tabIndex() {
        return behaviorOf().tabIndex;
      },
    },
  });
}
