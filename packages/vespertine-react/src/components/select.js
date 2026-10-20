// The single-value compound Select (CMP-005 R1-R8): Root, Trigger, Value, Content, Item, Group,
// Label, Separator, ScrollUpButton and ScrollDownButton.
//
// Behavior notes that differ from the Solid adapter (documented, not normalized): React tracks
// item registration in mounted order and initializes the highlight from the registry whenever
// registrations or the value change while open; it maps Ctrl-N and Ctrl-P to Down and Up; a
// disabled root blocks navigation and selection; the trigger press flag keeps an open trigger
// from closing on the outside press and reopening on the click.

import React from "react";
import { classifyKey, composeHandlers, createPressFlag, edgeIndex, mergeStyles, resolveStyle, stepIndex } from "@vespertine/host";
import { useControllableState } from "./controlled.js";
import { DismissableLayer, FloatingLayer } from "./layer.js";
import { usePresence } from "./presence.js";
import { renderSlot } from "./slot.js";

const SelectContext = React.createContext(null);

function useSelect(part) {
  const context = React.useContext(SelectContext);
  if (context === null) throw new Error(`Select.${part} must be used inside Select.Root`);
  return context;
}

/** The text of a React node tree (strings and numbers, through elements and arrays). */
export function textOf(node) {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (React.isValidElement(node)) return textOf(node.props.children);
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
 * `{ value, label }`). Metadata is optional: items register themselves when mounted, including
 * inside wrapper components. Renders the floating root container (flex, relative, start-aligned).
 */
export const Root = React.forwardRef(function Root(
  { value: valueProp, defaultValue, onValueChange, open: openProp, defaultOpen = false, onOpenChange, disabled = false, items, children, style, ...rest },
  ref,
) {
  const [value, requestValue] = useControllableState({ value: valueProp, defaultValue, onChange: onValueChange });
  const [open, requestOpen] = useControllableState({ value: openProp, defaultValue: defaultOpen, onChange: onOpenChange });
  const [highlight, setHighlight] = React.useState(null);
  const [version, setVersion] = React.useState(0);
  const entries = React.useRef([]);
  // Labels of items seen so far, so the closed trigger can show the selected item's text.
  const labels = React.useRef(new Map());
  const trigger = React.useRef(null);
  const closeReason = React.useRef(null);
  const [pressFlag] = React.useState(createPressFlag);

  // Highlight: the enabled selected registered item when the popup opens or the value changes;
  // when only the registrations change, a highlight on an entry that is still registered and
  // enabled (the user may have moved it) stays, otherwise the selected item takes over, or
  // nothing does. An item that unmounted and mounted again is a new entry.
  const initialized = React.useRef({ open: false, value: undefined });
  const highlightedEntry = React.useRef(null);
  React.useEffect(() => {
    if (!open) {
      initialized.current = { open: false, value };
      highlightedEntry.current = null;
      setHighlight(null);
      return;
    }
    const restart = !initialized.current.open || initialized.current.value !== value;
    initialized.current = { open: true, value };
    const kept = highlightedEntry.current;
    const keep = !restart && kept && entries.current.includes(kept) && !kept.disabled;
    const next = keep ? kept : (entries.current.find((entry) => entry.value === value && !entry.disabled) ?? null);
    highlightedEntry.current = next;
    setHighlight(next ? next.value : null);
  }, [open, value, version]);

  const api = React.useMemo(() => {
    const available = () => entries.current.map((entry) => !entry.disabled);
    const move = (index) => {
      if (index !== null) {
        highlightedEntry.current = entries.current[index];
        setHighlight(entries.current[index].value);
      }
    };
    return {
      entries,
      trigger,
      pressFlag,
      closeReason,
      register(entry) {
        entries.current.push(entry);
        labels.current.set(entry.value, entry.label);
        setVersion((v) => v + 1);
        return () => {
          entries.current = entries.current.filter((other) => other !== entry);
          setVersion((v) => v + 1);
        };
      },
      changed() {
        for (const entry of entries.current) labels.current.set(entry.value, entry.label);
        setVersion((v) => v + 1);
      },
      requestOpen,
      select(next) {
        if (disabled || !open) return;
        const entry = entries.current.find((other) => other.value === next);
        if (!entry || entry.disabled) return;
        closeReason.current = "select";
        requestValue(next);
        requestOpen(false);
      },
      step(direction) {
        if (disabled) return;
        const index = entries.current.findIndex((entry) => entry.value === highlight);
        move(stepIndex(available(), index === -1 ? null : index, direction));
      },
      edge(which) {
        if (disabled) return;
        move(edgeIndex(available(), which));
      },
      highlight: (next) => {
        if (disabled) return;
        highlightedEntry.current = entries.current.find((entry) => entry.value === next) ?? null;
        setHighlight(next);
      },
    };
  }, [disabled, highlight, open, pressFlag, requestOpen, requestValue]);

  const labelOf = React.useCallback(
    (selected) =>
      metadataLabel(items, selected) ??
      entries.current.find((entry) => entry.value === selected)?.label ??
      labels.current.get(selected) ??
      String(selected),
    // The registry is read through a ref; `version` makes the label follow registrations.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [items, version],
  );

  const context = { ...api, value, open, disabled, highlighted: highlight, labelOf };
  return React.createElement(
    SelectContext.Provider,
    { value: context },
    React.createElement(
      "div",
      { ...rest, ref, style: mergeStyles({ display: "flex", position: "relative", alignItems: "flex-start" }, style) },
      children,
    ),
  );
});

/**
 * The control that opens the popup. Tab index 0 (-1 when disabled; `disabled` overrides the
 * root's). Click toggles; Down and Up open a closed popup without advancing and step an open one
 * (Ctrl-N and Ctrl-P likewise); Enter and Space toggle. Caller handlers run first and internal
 * handlers do not honor a prevented default. `style` is an object or a function of
 * `{ open, disabled, placeholder }`.
 */
export const Trigger = React.forwardRef(function Trigger(
  { disabled: disabledProp, asChild = false, children, style, tabIndex, onClick, onKeyDown, onPointerDown, ...rest },
  ref,
) {
  const select = useSelect("Trigger");
  const disabled = disabledProp ?? select.disabled;
  const setRef = React.useCallback(
    (node) => {
      select.trigger.current = node;
      if (typeof ref === "function") ref(node);
      else if (ref) ref.current = node;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ref, select.trigger],
  );
  const toggle = () => {
    if (!disabled) select.requestOpen(!select.open);
  };
  const keyDown = (event) => {
    if (disabled) return;
    switch (classifyKey(event, { chords: true })) {
      case "down":
      case "up": {
        event.preventDefault?.();
        if (!select.open) select.requestOpen(true);
        else select.step(classifyKey(event, { chords: true }) === "down" ? 1 : -1);
        break;
      }
      case "enter":
        event.preventDefault?.();
        toggle();
        break;
      default:
        if (event.key === "space") {
          event.preventDefault?.();
          toggle();
        }
    }
  };
  return renderSlot({
    asChild,
    children,
    ref: setRef,
    props: {
      ...rest,
      role: "button",
      "aria-expanded": select.open,
      tabIndex,
      style: resolveStyle(style, { open: select.open, disabled, placeholder: select.value === undefined }),
      onPointerDown: composeHandlers(onPointerDown, () => select.pressFlag.raise()),
      onClick: composeHandlers(onClick, toggle),
      onKeyDown: composeHandlers(onKeyDown, keyDown),
    },
    defaults: { tabIndex: disabled ? -1 : 0 },
  });
});

/**
 * The selected value as text: explicit children, then the mapped label (root `items`, then the
 * registered item's text), then the raw value, then `placeholder`.
 */
export const Value = React.forwardRef(function Value({ children, placeholder, style, ...rest }, ref) {
  const select = useSelect("Value");
  let content = placeholder;
  if (children !== undefined && children !== null) content = children;
  else if (select.value !== undefined) content = select.labelOf(select.value);
  return React.createElement("text", { ...rest, ref, style }, content);
});

/**
 * The popup, mounted only while open. It takes focus (tab index -1), traps Tab, joins the
 * dismissal stack (Escape calls `onEscapeKeyDown`, then closes and focuses the trigger; an
 * outside press closes without moving focus) and uses the floating defaults. Down, Up, Ctrl-N,
 * Ctrl-P, Home and End move the highlight; Enter and Space select a truthy highlighted value.
 */
export const Content = React.forwardRef(function Content(
  { side, align, sideOffset, alignOffset, collisionPadding, style, onEscapeKeyDown, onKeyDown, initial, animate, exit, transition, children, ...rest },
  ref,
) {
  const select = useSelect("Content");
  const presence = usePresence(select.open, { initial, animate, exit, transition });
  if (!presence.mounted) return null;
  const keyDown = (event) => {
    // A closing popup reads as closed: its key handlers fall through.
    if (!select.open) return;
    switch (classifyKey(event, { chords: true })) {
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
        if (select.highlighted) select.select(select.highlighted);
        break;
      default:
        if (event.key === "space") {
          event.preventDefault?.();
          if (select.highlighted) select.select(select.highlighted);
        } else if (event.key === "tab") {
          event.preventDefault?.();
        }
    }
  };
  return React.createElement(
    FloatingLayer,
    { side, align, sideOffset, alignOffset, collisionPadding, style: mergeStyles(presence.closing ? { pointerEvents: "none" } : undefined, style), ref },
    React.createElement(
      DismissableLayer,
      {
        ...rest,
        open: select.open,
        motion: presence.motion,
        onMotionComplete: presence.motion ? presence.onMotionComplete : undefined,
        tabIndex: -1,
        pressFlag: select.pressFlag,
        trigger: () => select.trigger.current,
        defaultFocus: (node) => node?.id ?? null,
        finalFocus: () => {
          const reason = select.closeReason.current;
          select.closeReason.current = null;
          if (reason === "outside") return null;
          if (reason !== null) return select.trigger.current ?? null;
          return true;
        },
        onEscapeKeyDown,
        onDismiss: (reason) => {
          select.closeReason.current = reason;
          select.requestOpen(false);
        },
        onKeyDown: composeHandlers(onKeyDown, keyDown),
      },
      children,
    ),
  );
});

/**
 * An option. `value` is required; `disabled` items are skipped by stepping and cannot be
 * selected. Hover highlights an enabled item and click selects it (caller handlers run first).
 * `style` and function children receive `{ selected, highlighted, disabled }`. `textValue` is the
 * item's label metadata (default: the text of its children).
 */
export const Item = React.forwardRef(function Item(
  { value, disabled = false, asChild = false, style, children, textValue, onEnter, onClick, ...rest },
  ref,
) {
  if (typeof value !== "string") throw new TypeError("Select.Item requires a string value");
  const select = useSelect("Item");
  const label = textValue ?? (typeof children === "function" ? value : textOf(children) || value);
  const entry = React.useRef(null);
  if (entry.current === null) entry.current = { value, disabled, label };
  React.useEffect(() => select.register(entry.current), []); // eslint-disable-line react-hooks/exhaustive-deps
  React.useEffect(() => {
    const own = entry.current;
    if (own.value === value && own.disabled === disabled && own.label === label) return;
    Object.assign(own, { value, disabled, label });
    select.changed();
  }, [value, disabled, label]); // eslint-disable-line react-hooks/exhaustive-deps
  const state = { selected: select.value === value, highlighted: select.highlighted === value, disabled };
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
        if (!disabled && entry.current && select.entries.current.includes(entry.current)) select.highlight(value);
      }),
      onClick: composeHandlers(onClick, () => select.select(value)),
    },
  });
});

function plainPart(name) {
  const Part = React.forwardRef(function Part({ children, asChild = false, ...rest }, ref) {
    useSelect(name);
    return renderSlot({ asChild, children, ref, props: rest });
  });
  Part.displayName = `Select.${name}`;
  return Part;
}

/** Plain containers: they imply no automatic scrolling and no extra keyboard behavior. */
export const Group = plainPart("Group");
export const Label = plainPart("Label");
export const Separator = plainPart("Separator");
export const ScrollUpButton = plainPart("ScrollUpButton");
export const ScrollDownButton = plainPart("ScrollDownButton");
