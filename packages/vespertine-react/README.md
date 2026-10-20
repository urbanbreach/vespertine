# @vespertine/react

React adapter for the Vespertine native retained renderer (APP-009, APP-010, DEV-010).

```js
import React from "react";
import { render, useWindowSize } from "@vespertine/react";

function App() {
  const { width } = useWindowSize();
  return React.createElement("div", { style: { padding: 16 } }, React.createElement("text", {}, `width ${width}`));
}

render(React.createElement(App), { window: { title: "Hello", width: 640, height: 360 } });
```

* `createRoot(renderer, options)` mounts concurrent trees (mutation mode). Host nodes stay JavaScript
  descriptions during render; a commit creates native elements, applies properties, attaches
  children and registers handlers, then flushes exactly one mutation batch. Style and custom
  properties compare structurally, so a rerender with equal values sends nothing.
* Host types: `div` and `text` forward only the universal properties (`autoFocus`, `tabIndex`,
  `motion`, `testId`, `highlight`, `role`, `aria-*`); `img`, `svg`, `canvas`, `input`, `textarea`,
  `anchored`, `code`, `diff`, `markdown`, `virtual-list` and custom hosts accept any serializable
  property. `onX` props subscribe to the native event `x` (`onClick` is `click`).
* Native adapters behind those types: `input` and `textarea` are the CMP-007/CMP-008 editors
  (`value`, `defaultValue`, `placeholder`, `readOnly`, `disabled`; `onChange`, `onSubmit`, `onCancel`;
  Escape reaches an open layer after the editor's `onCancel`); `canvas` draws a `commands` list;
  `code` is the CMP-014 block (`code`, `language`, `path`, `showLineNumbers`, `theme`); `markdown`
  is the CMP-011 document (`markdown` source, `theme`, `onLinkClick` with the destination in
  `event.value`); `diff` is the CMP-015 viewer.
* Host refs expose `id`, `focus()`, `blur()`, `focusNext()`, `focusPrevious()`, `focusNextWithin()`,
  `focusPreviousWithin()`, `scrollIntoView()`, `scrollTo()`, `scrollToItem()`, `getScrollOffset()`,
  `getListScrollTop()`, and for images `setImage(bytes)` and `setImagePixels(width, height, bytes, "bgra")`.
* `createPortal(children, target, key)` mounts into a mounted host element of the same root.
* `render` / `resetRender` is the convenience entry point: one renderer, one window, one frame
  loop. It also installs the runtime error display (DEV-010): an uncaught failure replaces the tree
  with a readable error view and a Reload control that remounts the last submitted tree.
* Hooks: `useWindowSize`, `useWindowInsets`, `useTextSearch` and the shared `findRanges`.
* Headless components (CMP-001 to CMP-006): `Button` and `buttonProps`
  (role, tab index and Enter/Space/click activation for any control), `asChild` composition for
  every part (`renderSlot`), `FloatingLayer` and `DismissableLayer` (window-scoped Escape and
  focus ownership), the compound `Tooltip` (Provider, Root, Trigger, Content), `Select` (Root,
  Trigger, Value, Content, Item, Group, Label, Separator, ScrollUpButton, ScrollDownButton),
  `Dialog` (Root, Trigger, Portal, Backdrop, Popup, Title, Description, Close) and `Combobox`
  (Root, Input, Trigger, Value, Content, List, Item, Empty, Group, Label, Separator).
  Popup content accepts a native exit target (`exit`, `transition`) and stays mounted, inert,
  until it completes. React-specific behavior: opacity and radii of a floating surface reach its
  outer anchored surface, Ctrl-N/Ctrl-P step a select or combobox, a disabled select blocks
  navigation and selection, tooltip scheduling replaces the earlier timer of the same kind, the
  default combobox filter ranks prefix matches first and hands custom filters the raw query, a
  disabled combobox blocks navigation and submit, and the combobox value display converts and
  joins labels (placeholder for an empty selection).
* `Dialog` renders no element of its own. The portal (a deferred full-window `anchored` layer at
  priority 0) and the popup mount only while open; the popup is a dismissal layer (a select opened
  from inside closes before the dialog), a modal popup traps Tab with `focusNextWithin` /
  `focusPreviousWithin` and a nonmodal one does not, `initialFocus` / `finalFocus` follow the
  focus-target contract (default: the popup, then the trigger or the previously focused element),
  and `disablePointerDismissal` / `disableEscapeDismissal` on the root switch the two dismissals
  (Escape is consumed either way). The backdrop's press handler is `onPointerDown`.
* `@vespertine/react/testing` mounts trees in the offscreen native test host.

Tests: `npm test` (Node) or `bun test`; `cargo test -p vespertine_js_node --test adapters` runs both.
