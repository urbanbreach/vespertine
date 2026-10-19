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
* Host refs expose `id`, `focus()`, `blur()`, `focusNext()`, `focusPrevious()`, `focusNextWithin()`,
  `focusPreviousWithin()`, `scrollIntoView()`, `scrollTo()`, `scrollToItem()`, `getScrollOffset()`,
  `getListScrollTop()`, and for images `setImage(bytes)` and `setImagePixels(width, height, bytes, "bgra")`.
* `createPortal(children, target, key)` mounts into a mounted host element of the same root.
* `render` / `resetRender` is the convenience entry point: one renderer, one window, one frame
  loop. It also installs the runtime error display (DEV-010): an uncaught failure replaces the tree
  with a readable error view and a Reload control that remounts the last submitted tree.
* Hooks: `useWindowSize`, `useWindowInsets`, `useTextSearch` and the shared `findRanges`.
* Headless components (CMP-001, CMP-002, CMP-004, CMP-005): `Button` and `buttonProps`
  (role, tab index and Enter/Space/click activation for any control), `asChild` composition for
  every part (`renderSlot`), `FloatingLayer` and `DismissableLayer` (window-scoped Escape and
  focus ownership), the compound `Tooltip` (Provider, Root, Trigger, Content) and `Select` (Root,
  Trigger, Value, Content, Item, Group, Label, Separator, ScrollUpButton, ScrollDownButton).
  Popup content accepts a native exit target (`exit`, `transition`) and stays mounted, inert,
  until it completes. React-specific behavior: opacity and radii of a floating surface reach its
  outer anchored surface, Ctrl-N/Ctrl-P step a select, a disabled select blocks navigation and
  selection, and tooltip scheduling replaces the earlier timer of the same kind.
* `@vespertine/react/testing` mounts trees in the offscreen native test host.

Tests: `npm test` (Node) or `bun test`; `cargo test -p vespertine_js_node --test adapters` runs both.
