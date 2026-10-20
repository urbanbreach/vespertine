// The headless Combobox (CMP-006 R1-R8): Root, Input, Trigger, Value, Content, List, Item, Empty,
// Group, Label and Separator.
//
// Behavior notes that differ from the Solid adapter (documented, not normalized): the default
// filter trims and lowercases the query and ranks prefix matches before other substring matches,
// custom filters receive the raw query, Ctrl-N and Ctrl-P step the highlight, a disabled input
// blocks navigation and submit, selecting clears the highlight, the value display converts every
// selected string with `itemToStringValue` and shows the placeholder for an empty selection,
// the trigger's arrow keys request opening, and hovering an item highlights it only while the item
// is in the filtered list.

import React from "react";
import {
  buttonProps as behavior,
  classifyKey,
  composeHandlers,
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
import { useButtonLatch } from "./button.js";
import { useControllableState } from "./controlled.js";
import { DismissableLayer, FloatingLayer } from "./layer.js";
import { renderSlot } from "./slot.js";

const FLAVOR = "react";

const ComboboxContext = React.createContext(null);

function useCombobox(part) {
  const context = React.useContext(ComboboxContext);
  if (context === null) throw new Error(`Combobox.${part} must be used inside Combobox.Root`);
  return context;
}

function assign(ref, node) {
  if (typeof ref === "function") ref(node);
  else if (ref) ref.current = node;
}

/**
 * The combobox. Defaults: `items` empty, `value` null, input text empty, closed, `multiple`
 * false, `disabled` false, `autoHighlight` false. Value (`value`, `defaultValue`,
 * `onValueChange`: a string, an array of strings when `multiple`, or null), input text
 * (`inputValue`, `defaultInputValue`, `onInputValueChange`) and open state (`open`,
 * `defaultOpen`, `onOpenChange`) are each controlled when given. `filter` is a function
 * `(item, query, toString)`, `null` (no filtering) or unset (the default ranking filter);
 * `itemToStringValue` converts an item to its string (identity by default); `autoHighlight`
 * (true or "always") highlights the first enabled filtered item on every edit. Renders the
 * floating root container (flex, relative, start-aligned).
 */
export const Root = React.forwardRef(function Root(
  {
    items = [],
    filter,
    itemToStringValue = identityString,
    multiple = false,
    disabled = false,
    autoHighlight = false,
    open: openProp,
    defaultOpen = false,
    onOpenChange,
    inputValue: inputProp,
    defaultInputValue = "",
    onInputValueChange,
    value: valueProp,
    defaultValue = null,
    onValueChange,
    children,
    style,
    ...rest
  },
  ref,
) {
  const [value, requestValue] = useControllableState({ value: valueProp, defaultValue, onChange: onValueChange });
  const [inputValue, requestInput] = useControllableState({ value: inputProp, defaultValue: defaultInputValue, onChange: onInputValueChange });
  const [open, requestOpen] = useControllableState({ value: openProp, defaultValue: defaultOpen, onChange: onOpenChange });
  const [highlight, setHighlight] = React.useState(null);
  const disabledValues = React.useRef(new Set());
  const inputNode = React.useRef(null);
  const [pressFlag] = React.useState(createPressFlag);

  // Disabled-item tracking is removed again when the item unmounts or stops being disabled.
  const trackDisabled = React.useCallback((entry, isDisabled) => {
    if (!isDisabled) return undefined;
    disabledValues.current.add(entry);
    return () => disabledValues.current.delete(entry);
  }, []);

  const filtered = filterItems(items, inputValue, { flavor: FLAVOR, filter, itemToString: itemToStringValue });
  const strings = filtered.map(itemToStringValue);

  // Opening takes focus into the input, in a microtask (CMP-006 R3).
  React.useEffect(() => {
    if (!open) return;
    queueMicrotask(() => {
      if (inputNode.current?.mounted) inputNode.current.focus();
    });
  }, [open]);

  const api = {
    items,
    filtered,
    strings,
    itemToString: itemToStringValue,
    value,
    inputValue,
    open,
    multiple,
    disabled,
    highlight,
    inputNode,
    pressFlag,
    requestOpen,
    setHighlight,
    isSelected: (entry) => selectionList(value).includes(entry),
    trackDisabled,
    /** An edit of the query: the filter follows and the highlight is recomputed. */
    edit(text) {
      requestInput(text);
      setHighlight(
        highlightAfterEdit({ autoHighlight, items, query: text, filter, flavor: FLAVOR, itemToString: itemToStringValue, disabled: disabledValues.current }),
      );
    },
    select(entry) {
      if (disabled || disabledValues.current.has(entry)) return;
      if (multiple) {
        requestValue(toggleSelection(value, entry));
        requestInput("");
        setHighlight(null);
      } else {
        requestValue(entry);
        requestInput(entry);
        setHighlight(null);
        requestOpen(false);
      }
    },
    step(direction) {
      if (disabled) return;
      setHighlight(stepHighlight(strings, disabledValues.current, highlight, direction));
    },
    submit() {
      if (disabled || highlight === null || !strings.includes(highlight)) return;
      api.select(highlight);
    },
  };

  return React.createElement(
    ComboboxContext.Provider,
    { value: api },
    React.createElement(
      "div",
      { ...rest, ref, style: mergeStyles({ display: "flex", position: "relative", alignItems: "flex-start" }, style) },
      children,
    ),
  );
});

/**
 * The text input: shows the input value, is read-only when the root is disabled or `readOnly`,
 * and takes focus while the popup is open. Click and focus request opening (unless disabled), blur
 * closes, an edit (`change`, caller handler first) updates the filter and highlight and requests
 * opening. Arrow Down and Up (Ctrl-N and Ctrl-P) step the highlight; submit selects the
 * highlighted filtered item. A disabled root blocks stepping and submit.
 */
export const Input = React.forwardRef(function Input(
  { readOnly = false, onChange, onClick, onFocus, onBlur, onKeyDown, onSubmit, onPointerDown, ...rest },
  ref,
) {
  const combobox = useCombobox("Input");
  const setRef = React.useCallback(
    (node) => {
      combobox.inputNode.current = node;
      assign(ref, node);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ref, combobox.inputNode],
  );
  const open = () => {
    if (!combobox.disabled) combobox.requestOpen(true);
  };
  const keyDown = (event) => {
    if (combobox.disabled) return;
    const kind = classifyKey(event, { chords: true });
    if (kind === "down" || kind === "up") {
      event.preventDefault?.();
      combobox.step(kind === "down" ? 1 : -1);
    }
  };
  return React.createElement("input", {
    ...rest,
    ref: setRef,
    role: "combobox",
    "aria-expanded": combobox.open,
    value: combobox.inputValue,
    readOnly: combobox.disabled || readOnly,
    onChange: composeHandlers(onChange, (event) => {
      combobox.edit(event?.value ?? "");
      open();
    }),
    onClick: composeHandlers(onClick, open),
    onFocus: composeHandlers(onFocus, open),
    onBlur: composeHandlers(onBlur, () => combobox.requestOpen(false)),
    onPointerDown: composeHandlers(onPointerDown, () => combobox.pressFlag.raise()),
    onKeyDown: composeHandlers(onKeyDown, keyDown),
    onSubmit: composeHandlers(onSubmit, () => combobox.submit()),
  });
});

/**
 * The button that toggles the popup: tab index 0 (-1 when disabled; `disabled` overrides the
 * root's), click toggles, and Down and Up request opening. `style` is an object or a function of
 * `{ open, disabled }`.
 */
export const Trigger = React.forwardRef(function Trigger(
  { disabled: disabledProp, asChild = false, children, style, tabIndex, onClick, onKeyDown, onKeyUp, onPointerDown, ...rest },
  ref,
) {
  const combobox = useCombobox("Trigger");
  const disabled = disabledProp ?? combobox.disabled;
  const latch = useButtonLatch();
  const button = behavior({
    disabled,
    tabIndex,
    latch,
    onClick: (event) => {
      onClick?.(event);
      combobox.requestOpen(!combobox.open);
    },
    onKeyDown: (event) => {
      onKeyDown?.(event);
      if (disabled) return;
      const kind = classifyKey(event, { chords: true });
      if (kind === "down" || kind === "up") {
        event.preventDefault?.();
        combobox.requestOpen(true);
      }
    },
    onKeyUp,
  });
  return renderSlot({
    asChild,
    children,
    ref,
    props: {
      ...rest,
      role: "button",
      "aria-expanded": combobox.open,
      tabIndex,
      style: resolveStyle(style, { open: combobox.open, disabled }),
      onClick: button.onClick,
      onKeyDown: button.onKeyDown,
      onKeyUp: button.onKeyUp,
      onPointerDown: composeHandlers(onPointerDown, () => combobox.pressFlag.raise()),
    },
    defaults: { tabIndex: button.tabIndex },
  });
});

/**
 * The selection as text: explicit children (or a render function called with the value), else the
 * labels joined with ", ", else `placeholder` for a null or empty selection.
 */
export const Value = React.forwardRef(function Value({ children, placeholder, style, ...rest }, ref) {
  const combobox = useCombobox("Value");
  let content;
  if (typeof children === "function") content = children(combobox.value);
  else if (children !== undefined && children !== null) content = children;
  else content = displayText(combobox.value, { flavor: FLAVOR, itemToString: combobox.itemToString }) ?? placeholder;
  return React.createElement("text", { ...rest, ref, style }, content);
});

/**
 * The popup, mounted only while open, with the floating defaults. It closes on an outside press or
 * on Escape when it is the top layer, and never takes focus from the input.
 */
export const Content = React.forwardRef(function Content(
  { side, align, sideOffset, alignOffset, collisionPadding, style, onEscapeKeyDown, children, ...rest },
  ref,
) {
  const combobox = useCombobox("Content");
  if (!combobox.open) return null;
  return React.createElement(
    FloatingLayer,
    { side, align, sideOffset, alignOffset, collisionPadding, style, ref },
    React.createElement(
      DismissableLayer,
      {
        ...rest,
        open: true,
        initialFocus: false,
        finalFocus: false,
        pressFlag: combobox.pressFlag,
        onEscapeKeyDown,
        onDismiss: () => combobox.requestOpen(false),
      },
      children,
    ),
  );
});

/**
 * The list: with a function child it renders `children(item, index)` for every filtered item;
 * static children stay under the caller's control.
 */
export const List = React.forwardRef(function List({ children, asChild = false, ...rest }, ref) {
  const combobox = useCombobox("List");
  const content =
    typeof children === "function"
      ? combobox.filtered.map((item, index) => React.createElement(React.Fragment, { key: combobox.strings[index] }, children(item, index)))
      : children;
  return renderSlot({ asChild, children: content, ref, props: { role: "listbox", ...rest } });
});

/**
 * An option. `value` is required. Hover highlights an enabled item that is in the filtered list;
 * click selects an enabled item (caller handlers first). `style` and function children receive
 * `{ selected, highlighted, disabled }`.
 */
export const Item = React.forwardRef(function Item(
  { value, disabled = false, asChild = false, style, children, onEnter, onClick, ...rest },
  ref,
) {
  if (typeof value !== "string") throw new TypeError("Combobox.Item requires a string value");
  const combobox = useCombobox("Item");
  const { trackDisabled } = combobox;
  React.useEffect(() => trackDisabled(value, disabled), [trackDisabled, value, disabled]);
  const state = { selected: combobox.isSelected(value), highlighted: combobox.highlight === value, disabled };
  const inFilter = combobox.strings.includes(value);
  return renderSlot({
    asChild,
    children: typeof children === "function" ? children(state) : children,
    ref,
    props: {
      ...rest,
      role: "option",
      "aria-selected": state.selected,
      style: resolveStyle(style, state),
      onEnter: composeHandlers(onEnter, () => {
        if (!disabled && inFilter) combobox.setHighlight(value);
      }),
      onClick: composeHandlers(onClick, () => {
        if (!disabled) combobox.select(value);
      }),
    },
  });
});

/** Mounts only when no item passes the filter. */
export const Empty = React.forwardRef(function Empty({ children, asChild = false, ...rest }, ref) {
  const combobox = useCombobox("Empty");
  if (combobox.filtered.length > 0) return null;
  return renderSlot({ asChild, children, ref, props: rest });
});

function plainPart(name) {
  const Part = React.forwardRef(function Part({ children, asChild = false, ...rest }, ref) {
    useCombobox(name);
    return renderSlot({ asChild, children, ref, props: rest });
  });
  Part.displayName = `Combobox.${name}`;
  return Part;
}

/** Plain containers: they add no behavior. */
export const Group = plainPart("Group");
export const Label = plainPart("Label");
export const Separator = plainPart("Separator");
