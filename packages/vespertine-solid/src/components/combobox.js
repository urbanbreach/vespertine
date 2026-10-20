// The headless Combobox (CMP-006 R1-R8): Root, Input, Trigger, Value, Content, List, Item, Empty,
// Group, Label and Separator.
//
// Solid differences from the React adapter (documented, not normalized):
//
// - The default filter retains substring matches in the original order without prefix ranking, and
//   a custom filter receives the trimmed lowercase query (React passes the raw one).
// - Ctrl-N and Ctrl-P are NOT bound: only the arrows step the highlight. Arrow stepping does not
//   check the root's disabled state (React blocks it), but selection and submit do: a disabled
//   root never selects.
// - Selecting computes the highlight through the input setter: a single selection sets the input
//   to the selected string and applies the `autoHighlight` policy to that query, a multiple
//   selection clears the input through the same setter (React clears the highlight instead).
// - The value display joins the raw selected strings with ", " and shows the placeholder only for
//   a null value; an empty string or an empty array displays as empty text.
// - The trigger has no internal arrow-key handler: only click and the button keys toggle it.
// - Hovering an item highlights it even when it is not in the filtered list (index -1: a static
//   item outside the filter); React requires a nonnegative filtered index.
// - An item records its value and disabled flag when it is created; later property changes are
//   not tracked.
// - A list render function runs once for every item that enters the filtered list (the nodes of
//   items that stay are kept); React runs it again on every render.

import { For, Show, createContext, createEffect, createMemo, createSignal, mergeProps, onCleanup, splitProps, untrack, useContext } from "solid-js";
import {
  buttonProps as behavior,
  classifyKey,
  composeHandlers,
  createButtonLatch,
  createPressFlag,
  displayText,
  filterItems,
  highlightAfterEdit,
  identityString,
  mergeStyles,
  resolveStyle,
  selectionList,
  stepHighlight,
  toggleSelection,
} from "@vespertine/host";
import { createComponent } from "../renderer.js";
import { createControllableState } from "./controlled.js";
import { DismissableLayer, FloatingLayer } from "./layer.js";
import { renderSlot } from "./slot.js";

const FLAVOR = "solid";

const ComboboxContext = createContext(null);

function useCombobox(part) {
  const context = useContext(ComboboxContext);
  if (context === null) throw new Error(`Combobox.${part} must be used inside Combobox.Root`);
  return context;
}

/**
 * The combobox. Defaults: `items` empty, `value` null, input text empty, closed, `multiple`
 * false, `disabled` false, `autoHighlight` false. Value (`value`, `defaultValue`,
 * `onValueChange`: a string, an array of strings when `multiple`, or null), input text
 * (`inputValue`, `defaultInputValue`, `onInputValueChange`) and open state (`open`,
 * `defaultOpen`, `onOpenChange`) are each controlled when given. `filter` is a function
 * `(item, query, toString)`, `null` (no filtering) or unset (the default substring filter);
 * `itemToStringValue` converts an item to its string (identity by default); `autoHighlight`
 * (true or "always") highlights the first enabled filtered item on every edit. Renders the
 * floating root container (flex, relative, start-aligned).
 */
export function Root(props) {
  const [local, rest] = splitProps(props, [
    "items",
    "value",
    "defaultValue",
    "onValueChange",
    "inputValue",
    "defaultInputValue",
    "onInputValueChange",
    "open",
    "defaultOpen",
    "onOpenChange",
    "multiple",
    "disabled",
    "autoHighlight",
    "filter",
    "itemToStringValue",
    "children",
    "style",
  ]);
  const [value, requestValue] = createControllableState({
    value: () => local.value,
    defaultValue: local.defaultValue ?? null,
    onChange: (next) => local.onValueChange?.(next),
  });
  const [inputValue, requestInput] = createControllableState({
    value: () => local.inputValue,
    defaultValue: local.defaultInputValue ?? "",
    onChange: (next) => local.onInputValueChange?.(next),
  });
  const [open, requestOpen] = createControllableState({
    value: () => local.open,
    defaultValue: local.defaultOpen ?? false,
    onChange: (next) => local.onOpenChange?.(next),
  });
  const [highlight, setHighlight] = createSignal(null);
  const disabledValues = new Set();
  const state = { input: null };
  const pressFlag = createPressFlag();
  const items = () => local.items ?? [];
  const itemToString = (item) => (local.itemToStringValue ?? identityString)(item);
  const disabled = () => local.disabled ?? false;
  const filtered = createMemo(() => filterItems(items(), inputValue(), { flavor: FLAVOR, filter: local.filter, itemToString }));
  const strings = createMemo(() => filtered().map(itemToString));

  // Opening takes focus into the input, in a microtask (CMP-006 R3).
  createEffect(() => {
    if (!open()) return;
    queueMicrotask(() => {
      if (state.input?.mounted) state.input.focus();
    });
  });

  /** The input setter: the filter follows and the highlight policy applies to the new query. */
  const setInput = (text) => {
    requestInput(text);
    setHighlight(
      highlightAfterEdit({ autoHighlight: local.autoHighlight, items: items(), query: text, filter: local.filter, flavor: FLAVOR, itemToString, disabled: disabledValues }),
    );
  };

  const context = {
    state,
    pressFlag,
    filtered,
    strings,
    itemToString,
    value,
    inputValue,
    open,
    disabled,
    highlight,
    requestOpen,
    setHighlight,
    isSelected: (entry) => selectionList(value()).includes(entry),
    trackDisabled(entry, isDisabled) {
      if (isDisabled) disabledValues.add(entry);
      else disabledValues.delete(entry);
    },
    edit: setInput,
    select(entry) {
      if (disabled() || disabledValues.has(entry)) return;
      if (local.multiple) {
        requestValue(toggleSelection(untrack(value), entry));
        setInput("");
      } else {
        requestValue(entry);
        setInput(entry);
        requestOpen(false);
      }
    },
    step(direction) {
      setHighlight(stepHighlight(untrack(strings), disabledValues, untrack(highlight), direction));
    },
    submit() {
      const current = untrack(highlight);
      if (current === null || !untrack(strings).includes(current)) return;
      context.select(current);
    },
  };

  return createComponent(ComboboxContext.Provider, {
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
 * The text input: shows the input value, is read-only when the root is disabled or `readOnly`,
 * and takes focus while the popup is open. Click and focus request opening (unless disabled), blur
 * closes, an edit (`change`, caller handler first) updates the filter and highlight and requests
 * opening. Arrow Down and Up step the highlight; submit selects the highlighted filtered item
 * (a disabled root selects nothing).
 */
export function Input(props) {
  const combobox = useCombobox("Input");
  const [local, rest] = splitProps(props, ["readOnly", "onChange", "onClick", "onFocus", "onBlur", "onKeyDown", "onSubmit", "onPointerDown", "ref"]);
  const open = () => {
    if (!combobox.disabled()) combobox.requestOpen(true);
  };
  const keyDown = (event) => {
    const kind = classifyKey(event, { chords: false });
    if (kind === "down" || kind === "up") {
      event.preventDefault?.();
      combobox.step(kind === "down" ? 1 : -1);
    }
  };
  return renderSlot({
    type: "input",
    props: mergeProps(rest, {
      role: "combobox",
      get "aria-expanded"() {
        return combobox.open();
      },
      get value() {
        return combobox.inputValue();
      },
      get readOnly() {
        return combobox.disabled() || (local.readOnly ?? false);
      },
      ref(node) {
        combobox.state.input = node;
        local.ref?.(node);
      },
      get onChange() {
        return composeHandlers(local.onChange, (event) => {
          combobox.edit(event?.value ?? "");
          open();
        });
      },
      get onClick() {
        return composeHandlers(local.onClick, open);
      },
      get onFocus() {
        return composeHandlers(local.onFocus, open);
      },
      get onBlur() {
        return composeHandlers(local.onBlur, () => combobox.requestOpen(false));
      },
      get onPointerDown() {
        return composeHandlers(local.onPointerDown, () => combobox.pressFlag.raise());
      },
      get onKeyDown() {
        return composeHandlers(local.onKeyDown, keyDown);
      },
      get onSubmit() {
        return composeHandlers(local.onSubmit, () => combobox.submit());
      },
    }),
  });
}

/**
 * The button that toggles the popup: tab index 0 (-1 when disabled; `disabled` overrides the
 * root's) and click toggles; there is no internal arrow-key handling. `style` is an object or a
 * function of `{ open, disabled }`.
 */
export function Trigger(props) {
  const combobox = useCombobox("Trigger");
  const [local, rest] = splitProps(props, ["disabled", "asChild", "children", "style", "tabIndex", "onClick", "onKeyDown", "onKeyUp", "onPointerDown"]);
  const latch = createButtonLatch();
  const disabled = () => local.disabled ?? combobox.disabled();
  const button = createMemo(() =>
    behavior({
      disabled: disabled(),
      tabIndex: local.tabIndex,
      latch,
      onClick: (event) => {
        local.onClick?.(event);
        combobox.requestOpen(!untrack(combobox.open));
      },
      onKeyDown: (event) => local.onKeyDown?.(event),
      onKeyUp: (event) => local.onKeyUp?.(event),
    }),
  );
  return renderSlot({
    asChild: local.asChild,
    children: () => local.children,
    props: mergeProps(rest, {
      role: "button",
      get "aria-expanded"() {
        return combobox.open();
      },
      get tabIndex() {
        return local.tabIndex;
      },
      get style() {
        return resolveStyle(local.style, { open: combobox.open(), disabled: disabled() });
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
      get onPointerDown() {
        return composeHandlers(local.onPointerDown, () => combobox.pressFlag.raise());
      },
    }),
    defaults: {
      get tabIndex() {
        return button().tabIndex;
      },
    },
  });
}

/**
 * The selection as text: explicit children (or a render function called with the value), else the
 * raw selected strings joined with ", ", else `placeholder` for a null value.
 */
export function Value(props) {
  const combobox = useCombobox("Value");
  const [local, rest] = splitProps(props, ["children", "placeholder"]);
  return renderSlot({
    type: "text",
    props: rest,
    children: () => {
      if (typeof local.children === "function") return local.children(combobox.value());
      if (local.children !== undefined && local.children !== null) return local.children;
      return displayText(combobox.value(), { flavor: FLAVOR }) ?? local.placeholder;
    },
  });
}

/**
 * The popup, mounted only while open, with the floating defaults. It closes on an outside press or
 * on Escape when it is the top layer, and never takes focus from the input.
 */
export function Content(props) {
  const combobox = useCombobox("Content");
  const [local, rest] = splitProps(props, ["side", "align", "sideOffset", "alignOffset", "collisionPadding", "style", "onEscapeKeyDown", "children", "ref"]);
  return createComponent(Show, {
    get when() {
      return combobox.open();
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
          return local.style;
        },
        ref(node) {
          local.ref?.(node);
        },
        get children() {
          return createComponent(
            DismissableLayer,
            mergeProps(rest, {
              open: true,
              initialFocus: false,
              finalFocus: false,
              pressFlag: combobox.pressFlag,
              get onEscapeKeyDown() {
                return local.onEscapeKeyDown;
              },
              onDismiss: () => combobox.requestOpen(false),
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
 * The list: with a function child it renders `children(item, index)` for every filtered item;
 * static children stay under the caller's control.
 */
export function List(props) {
  const combobox = useCombobox("List");
  const [local, rest] = splitProps(props, ["children", "asChild"]);
  return renderSlot({
    asChild: local.asChild,
    children: () =>
      typeof local.children === "function"
        ? createComponent(For, {
            get each() {
              return combobox.filtered();
            },
            children: (item, index) => local.children(item, index()),
          })
        : local.children,
    props: mergeProps({ role: "listbox" }, rest),
  });
}

/**
 * An option. `value` is required. Hover highlights an enabled item; click selects an enabled item
 * (caller handlers first). `style` and function children receive `{ selected, highlighted,
 * disabled }`. The value and the disabled flag are captured when the item is created.
 */
export function Item(props) {
  const combobox = useCombobox("Item");
  const [local, rest] = splitProps(props, ["value", "disabled", "asChild", "style", "children", "onEnter", "onClick"]);
  const value = local.value;
  if (typeof value !== "string") throw new TypeError("Combobox.Item requires a string value");
  const disabled = local.disabled ?? false;
  combobox.trackDisabled(value, disabled);
  onCleanup(() => combobox.trackDisabled(value, false));
  const state = {
    get selected() {
      return combobox.isSelected(value);
    },
    get highlighted() {
      return combobox.highlight() === value;
    },
    disabled,
  };
  const renders = typeof local.children === "function";
  return renderSlot({
    asChild: local.asChild,
    children: renders ? () => local.children(state) : () => local.children,
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
          if (!disabled) combobox.setHighlight(value);
        });
      },
      get onClick() {
        return composeHandlers(local.onClick, () => {
          if (!disabled) combobox.select(value);
        });
      },
    }),
  });
}

/** Mounts only when no item passes the filter. */
export function Empty(props) {
  const combobox = useCombobox("Empty");
  const [local, rest] = splitProps(props, ["children", "asChild"]);
  return createComponent(Show, {
    get when() {
      return combobox.filtered().length === 0;
    },
    get children() {
      return renderSlot({ asChild: local.asChild, children: () => local.children, props: rest });
    },
  });
}

function plainPart(name) {
  const Part = (props) => {
    useCombobox(name);
    const [local, rest] = splitProps(props, ["children", "asChild"]);
    return renderSlot({ asChild: local.asChild, children: () => local.children, props: rest });
  };
  Object.defineProperty(Part, "name", { value: name });
  return Part;
}

/** Plain containers: they add no behavior. */
export const Group = plainPart("Group");
export const Label = plainPart("Label");
export const Separator = plainPart("Separator");
