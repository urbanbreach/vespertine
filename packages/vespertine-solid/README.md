# @vespertine/solid

Solid adapter for the Vespertine native retained renderer (APP-009, APP-010).

```tsx
import { createSignal, render } from "@vespertine/solid";

function Counter() {
  const [n, setN] = createSignal(0);
  return (
    <div onClick={() => setN(n() + 1)} style={{ padding: 16 }}>
      <text>{`clicked ${n()} times`}</text>
    </div>
  );
}

render(() => <Counter />, { window: { title: "Counter" } });
```

* Run TSX under Bun with the preload: `bun --preload @vespertine/solid/preload app.tsx` (or
  `preload = ["@vespertine/solid/preload"]` in `bunfig.toml`), or build with the exported plugin:
  `import plugin from "@vespertine/solid/bun-plugin"; Bun.build({ plugins: [plugin], ... })`.
  The integration compiles JSX with Babel's Solid universal preset, strips TypeScript, ignores
  repository Babel configuration, strips query strings from specifiers and resolves `solid-js` to its
  client runtime. Under Node, run with `--conditions=browser` so `solid-js` resolves to its client build.
* `createRoot(renderer, options)` returns `{ render, flush, flushSync, dispatch, unmount }`.
  Reactive changes outside event dispatch flush in a microtask; event dispatch and `flushSync` flush in
  a `finally` path; a second `render` disposes the first tree and starts a fresh event generation.
* Intrinsic types: `div`, `text`, `img`, `svg`, `canvas`, `input`, `textarea`, `anchored`, `code`, `diff`,
  `markdown`, `virtual-list`; anything else throws.
* Primitives: `createWindowSize`, `createWindowInsets`, `createSelectedText`, `createTextSearch` and the
  shared `findRanges`. Context access: `useOptionalRendererContext` (undefined outside a root) and
  `useRendererContext` (throws).
* Headless components (CMP-001 to CMP-006): `Button` and `buttonProps` with
  `createButtonLatch`, `renderSlot` (`asChild` composition), `createControllableState` (returns
  `[accessor, request]`), `FloatingLayer`, `DismissableLayer`, `createPresence`, the compound
  `Tooltip` (Provider, Root, Trigger, Content), `Select` (Root, Trigger, Value, Content, Item,
  Group, Label, Separator, ScrollUpButton, ScrollDownButton), `Dialog` (Root, Trigger, Portal,
  Backdrop, Popup, Title, Description, Close) and `Combobox` (Root, Input, Trigger, Value, Content,
  List, Item, Empty, Group, Label, Separator). Components never destructure props, so reactive
  getters stay reactive. Solid-specific behavior: `asChild` needs exactly one intrinsic host
  element child and its properties (style included) replace the child's, only handlers composing;
  the caller style of a floating layer applies to the inner container only; tooltip provider
  settings and root hoverability are captured at construction and a newer timer overwrites the
  tracked handle without cancelling the earlier one; a select binds no Ctrl-N/Ctrl-P, keeps popup
  navigation while disabled (selection stays blocked) and initializes its highlight on an open
  request; the combobox filters by substring in the original order (custom filters get the trimmed
  lowercase query), binds no Ctrl-N/Ctrl-P or trigger arrow keys, shows the raw selected strings
  (placeholder only for null), applies the highlight policy through its input setter and records
  an item's disabled flag at construction.
* `Dialog` renders no element of its own. The portal (a deferred full-window `anchored` layer at
  priority 0) and the popup mount only while open; the popup is a dismissal layer (a select opened
  from inside closes before the dialog), a modal popup traps Tab with `focusNextWithin` /
  `focusPreviousWithin` and a nonmodal one does not, `initialFocus` / `finalFocus` follow the
  focus-target contract (default: the popup, then the trigger or the previously focused element;
  a `ref` host element is a valid target), and `disablePointerDismissal` /
  `disableEscapeDismissal` on the root switch the two dismissals (Escape is consumed either way).
  The backdrop's press handler is `onPointerDown`.
* `@vespertine/solid/testing` mounts trees in the offscreen native test host.

Tests: `npm test` (Node) or `bun test`; `cargo test -p vespertine_js_node --test adapters` runs both.
