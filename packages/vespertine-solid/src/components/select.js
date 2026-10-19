// The single-value compound Select (CMP-005 R1-R8): Root, Trigger, Value, Content, Item, Group,
// Label, Separator, ScrollUpButton and ScrollDownButton.
//
// Solid differences from the React adapter (documented, not normalized):
//
// - Ctrl-N and Ctrl-P are NOT bound: keys are classified without chords, so a Ctrl-N press does
//   nothing (React maps them to Down and Up). Only the arrows, Home, End, Enter and Space act.
// - A disabled root keeps popup navigation (stepping, Home, End and item enter still change the
//   highlight) but blocks value selection. The trigger of a disabled root is disabled (tab index
//   -1, no keyboard or click toggling) unless the trigger overrides `disabled`.
// - The highlight is initialized on an OPEN REQUEST (a trigger click, Enter, Space, Down or Up
//   that changes the open state) to the enabled recorded selected item, `null` when there is
//   none, and cleared by a close request. It does not follow items that register later, nor the
//   value changing while open, nor an owner flipping a controlled `open` without a request.
//   Items are recorded only while mounted, so a request made while the content is unmounted
//   finds nothing to highlight; it finds the selected item only while the items are still
//   registered (for example while the popup is closing for its exit animation).
// - Item registration is a plain registry recorded when the item is created: its `value`,
//   `disabled` flag and label (`textValue`, else the text of its children, else the value) are
//   captured then and do not track later property changes. Stepping and Home/End use the
//   registry in registration (mounted) order. The registry never has to agree with a selection:
//   selecting rejects a root-disabled select and a recorded-disabled value but does not
//   independently require registry membership. Item enter highlights an enabled item.
// - A select that is not open (including a closing one) takes no selection, as in React.
// - Labels follow the React precedence (root `items` metadata, the registered item's text, the
//   label remembered from an earlier registration, the raw value, then the placeholder); the
//   root `items` are read live, the item labels are the captured ones.
// - Function children of an Item are render functions called with `{ selected, highlighted,
//   disabled }` (getters, so reading them inside the function stays reactive).
// - The trigger press flag and the Escape / outside-press dismissal behave as in React.

import { Show, children as resolveChildren, createContext, createSignal, mergeProps, onCleanup, splitProps, useContext } from "solid-js";
import {
  classifyKey,
  composeHandlers,
  createPressFlag,
  edgeIndex,
  isHostNode,
  mergeStyles,
  resolveStyle,
  stepIndex,
} from "@vespertine/host";
import { createComponent } from "../renderer.js";
import { createControllableState } from "./controlled.js";
import { DismissableLayer, FloatingLayer } from "./layer.js";
import { createPresence } from "./presence.js";
import { renderSlot } from "./slot.js";

const SelectContext = createContext(null);

function useSelect(part) {
  const context = useContext(SelectContext);
  if (context === null) throw new Error(`Select.${part} must be used inside Select.Root`);
  return context;
}

/** The text of resolved children (strings, numbers and host nodes, through arrays). */
export function textOf(node) {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isHostNode(node)) return node.text ?? node.children.map(textOf).join("");
  return "";
}

function metadataLabel(items, value) {
  if (items === undefined || items === null || value === undefined) return undefined;
  if (Array.isArray(items)) return items.find((item) => item.value === value)?.label;
  return Object.hasOwn(items, value) ? items[value] : undefined;
}

/**
 * The select: `value`/`defaultValue`/`onValueChange`, `open`/`defaultOpen`/`onOpenChange`,
 * `disabled` (false), and `items`, the value-to-label metadata (an object or a list of
 * `{ value, label }`). Metadata is optional: items register themselves when created, including
 * inside wrapper components. Renders the floating root container (flex, relative, start-aligned).
 */
export function Root(props) {
  const [local, rest] = splitProps(props, [
    "value",
    "defaultValue",
    "onValueChange",
    "open",
    "defaultOpen",
    "onOpenChange",
    "disabled",
    "items",
    "children",
    "style",
  ]);
  const [value, requestValue] = createControllableState({
    value: () => local.value,
    defaultValue: local.defaultValue,
    onChange: (next) => local.onValueChange?.(next),
  });
  const [open, requestOpenRaw] = createControllableState({
    value: () => local.open,
    defaultValue: local.defaultOpen ?? false,
    onChange: (next) => local.onOpenChange?.(next),
  });
  const [highlight, setHighlight] = createSignal(null);
  const [version, setVersion] = createSignal(0);
  const disabled = () => local.disabled ?? false;
  /** Items in registration order, recorded when created. */
  const entries = [];
  /** Labels of items seen so far, so the closed trigger can show the selected item's text. */
  const labels = new Map();
  const pressFlag = createPressFlag();
  const state = { trigger: null, closeReason: null };

  const move = (index) => {
    if (index !== null) setHighlight(entries[index].value);
  };

  const requestOpen = (next) => {
    const changed = requestOpenRaw(next);
    if (changed) {
      const selected = entries.find((entry) => entry.value === value() && !entry.disabled);
      setHighlight(next && selected ? selected.value : null);
    }
    return changed;
  };

  const context = {
    state,
    pressFlag,
    value,
    open,
    disabled,
    highlighted: highlight,
    requestOpen,
    register(entry) {
      entries.push(entry);
      labels.set(entry.value, entry.label);
      setVersion((count) => count + 1);
    },
    unregister(entry) {
      const index = entries.indexOf(entry);
      if (index !== -1) entries.splice(index, 1);
    },
    labelOf(selected) {
      version();
      return (
        metadataLabel(local.items, selected) ??
        entries.find((entry) => entry.value === selected)?.label ??
        labels.get(selected) ??
        String(selected)
      );
    },
    highlight: (next) => setHighlight(next),
    select(next) {
      if (disabled() || !open()) return;
      const entry = entries.find((other) => other.value === next);
      if (entry?.disabled) return;
      state.closeReason = "select";
      requestValue(next);
      requestOpen(false);
    },
    step(direction) {
      const index = entries.findIndex((entry) => entry.value === highlight());
      move(stepIndex(entries.map((entry) => !entry.disabled), index === -1 ? null : index, direction));
    },
    edge(which) {
      move(edgeIndex(entries.map((entry) => !entry.disabled), which));
    },
  };

  return createComponent(SelectContext.Provider, {
    value: context,
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
 * The control that opens the popup. Tab index 0 (-1 when disabled; `disabled` overrides the
 * root's). Click toggles; Down and Up open a closed popup without advancing and step an open one
 * (Ctrl-N and Ctrl-P are not bound); Enter and Space toggle. Caller handlers run first and
 * internal handlers do not honor a prevented default. `style` is an object or a function of
 * `{ open, disabled, placeholder }`.
 */
export function Trigger(props) {
  const select = useSelect("Trigger");
  const [local, rest] = splitProps(props, [
    "disabled",
    "asChild",
    "children",
    "style",
    "tabIndex",
    "onClick",
    "onKeyDown",
    "onPointerDown",
    "ref",
  ]);
  const disabled = () => local.disabled ?? select.disabled();
  const toggle = () => {
    if (!disabled()) select.requestOpen(!select.open());
  };
  const keyDown = (event) => {
    if (disabled()) return;
    const kind = classifyKey(event, { chords: false });
    if (kind === "down" || kind === "up") {
      event.preventDefault?.();
      if (!select.open()) select.requestOpen(true);
      else select.step(kind === "down" ? 1 : -1);
    } else if (kind === "enter" || event.key === "space") {
      event.preventDefault?.();
      toggle();
    }
  };
  return renderSlot({
    asChild: local.asChild,
    children: () => local.children,
    props: mergeProps(rest, {
      role: "button",
      get "aria-expanded"() {
        return select.open();
      },
      get tabIndex() {
        return local.tabIndex;
      },
      get style() {
        return resolveStyle(local.style, { open: select.open(), disabled: disabled(), placeholder: select.value() === undefined });
      },
      ref(node) {
        select.state.trigger = node;
        local.ref?.(node);
      },
      get onPointerDown() {
        return composeHandlers(local.onPointerDown, () => select.pressFlag.raise());
      },
      get onClick() {
        return composeHandlers(local.onClick, toggle);
      },
      get onKeyDown() {
        return composeHandlers(local.onKeyDown, keyDown);
      },
    }),
    defaults: {
      get tabIndex() {
        return disabled() ? -1 : 0;
      },
    },
  });
}

/**
 * The selected value as text: explicit children, then the mapped label (root `items`, then the
 * registered item's text), then the raw value, then `placeholder`.
 */
export function Value(props) {
  const select = useSelect("Value");
  const [local, rest] = splitProps(props, ["children", "placeholder"]);
  return renderSlot({
    type: "text",
    props: rest,
    children: () => {
      if (local.children !== undefined && local.children !== null) return local.children;
      const selected = select.value();
      return selected !== undefined ? select.labelOf(selected) : local.placeholder;
    },
  });
}

/**
 * The popup, mounted only while open. It takes focus (tab index -1), traps Tab, joins the
 * dismissal stack (Escape calls `onEscapeKeyDown`, then closes and focuses the trigger; an
 * outside press closes without moving focus) and uses the floating defaults. Down, Up, Home and
 * End move the highlight (Ctrl-N and Ctrl-P are not bound); Enter and Space select a truthy
 * highlighted value.
 */
export function Content(props) {
  const select = useSelect("Content");
  const [local, rest] = splitProps(props, [
    "side",
    "align",
    "sideOffset",
    "alignOffset",
    "collisionPadding",
    "style",
    "onEscapeKeyDown",
    "onKeyDown",
    "initial",
    "animate",
    "exit",
    "transition",
    "children",
    "ref",
  ]);
  const presence = createPresence(select.open, () => ({
    initial: local.initial,
    animate: local.animate,
    exit: local.exit,
    transition: local.transition,
  }));
  const keyDown = (event) => {
    // A closing popup reads as closed: its key handlers fall through.
    if (!select.open()) return;
    switch (classifyKey(event, { chords: false })) {
      case "down":
        event.preventDefault?.();
        select.step(1);
        break;
      case "up":
        event.preventDefault?.();
        select.step(-1);
        break;
      case "home":
        event.preventDefault?.();
        select.edge("first");
        break;
      case "end":
        event.preventDefault?.();
        select.edge("last");
        break;
      case "enter":
        event.preventDefault?.();
        if (select.highlighted()) select.select(select.highlighted());
        break;
      default:
        if (event.key === "space") {
          event.preventDefault?.();
          if (select.highlighted()) select.select(select.highlighted());
        } else if (event.key === "tab") {
          event.preventDefault?.();
        }
    }
  };
  return createComponent(Show, {
    get when() {
      return presence.mounted();
    },
    get children() {
      return createComponent(FloatingLayer, {
        get side() {
          return local.side;
        },
        get align() {
          return local.align;
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
          return mergeStyles(presence.closing() ? { pointerEvents: "none" } : undefined, local.style);
        },
        ref(node) {
          local.ref?.(node);
        },
        get children() {
          return createComponent(
            DismissableLayer,
            mergeProps(rest, {
              get open() {
                return select.open();
              },
              get motion() {
                return presence.motion();
              },
              get onMotionComplete() {
                return presence.motion() ? presence.onMotionComplete : undefined;
              },
              tabIndex: -1,
              pressFlag: select.pressFlag,
              trigger: () => select.state.trigger,
              defaultFocus: (node) => node?.id ?? null,
              finalFocus: () => {
                const reason = select.state.closeReason;
                select.state.closeReason = null;
                if (reason === "outside") return null;
                if (reason !== null) return select.state.trigger ?? null;
                return true;
              },
              get onEscapeKeyDown() {
                return local.onEscapeKeyDown;
              },
              onDismiss: (reason) => {
                select.state.closeReason = reason;
                select.requestOpen(false);
              },
              get onKeyDown() {
                return composeHandlers(local.onKeyDown, keyDown);
              },
              get children() {
                return local.children;
              },
            }),
          );
        },
      });
    },
  });
}

/**
 * An option. `value` is required; `disabled` items are skipped by stepping and cannot be
 * selected. Hover highlights an enabled item and click selects it (caller handlers run first).
 * `style` and function children receive `{ selected, highlighted, disabled }`. `textValue` is the
 * item's label metadata (default: the text of its children). The value, the disabled flag and
 * the label are captured when the item is created.
 */
export function Item(props) {
  const select = useSelect("Item");
  const [local, rest] = splitProps(props, ["value", "disabled", "asChild", "style", "children", "textValue", "onEnter", "onClick"]);
  const value = local.value;
  if (typeof value !== "string") throw new TypeError("Select.Item requires a string value");
  const disabled = local.disabled ?? false;
  const state = {
    get selected() {
      return select.value() === value;
    },
    get highlighted() {
      return select.highlighted() === value;
    },
    disabled,
  };
  const renders = typeof local.children === "function";
  const content = renders ? undefined : resolveChildren(() => local.children);
  const label = local.textValue ?? (renders ? value : textOf(content()) || value);
  const entry = { value, disabled, label };
  select.register(entry);
  onCleanup(() => select.unregister(entry));
  return renderSlot({
    asChild: local.asChild,
    children: renders ? () => local.children(state) : content,
    props: mergeProps(rest, {
      role: "option",
      get "aria-selected"() {
        return state.selected;
      },
      get style() {
        return resolveStyle(local.style, state);
      },
      get onEnter() {
        return composeHandlers(local.onEnter, () => {
          if (!disabled) select.highlight(value);
        });
      },
      get onClick() {
        return composeHandlers(local.onClick, () => select.select(value));
      },
    }),
  });
}

function plainPart(name) {
  const Part = (props) => {
    useSelect(name);
    const [local, rest] = splitProps(props, ["children", "asChild"]);
    return renderSlot({ asChild: local.asChild, children: () => local.children, props: rest });
  };
  Object.defineProperty(Part, "name", { value: name });
  return Part;
}

/** Plain containers: they imply no automatic scrolling and no extra keyboard behavior. */
export const Group = plainPart("Group");
export const Label = plainPart("Label");
export const Separator = plainPart("Separator");
export const ScrollUpButton = plainPart("ScrollUpButton");
export const ScrollDownButton = plainPart("ScrollDownButton");
