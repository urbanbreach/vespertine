// Fixtures for the headless component tests: a Solid root over the JavaScript reference tree,
// lookup by test id, native-style event emission and virtual time.

import { createComponent, createRoot, mergeProps } from "../src/index.js";
import { h, referenceRenderer } from "./helpers.js";

export { installFakeTime } from "./fake-time.js";
export { h };

const realSetTimeout = globalThis.setTimeout;

/** Lets the microtask flush of work started outside an event run, even while time is faked. */
export async function settle(times = 8) {
  for (let i = 0; i < times; i++) await new Promise((resolve) => realSetTimeout(resolve, 0));
}

/** Lets pending microtasks run. */
export async function microtasks(times = 3) {
  for (let i = 0; i < times; i++) await Promise.resolve();
}

/**
 * Creates a component without JSX. `kids` are strings, nodes or thunks that build children
 * lazily (components that read a context must be built inside their parent, so pass a thunk);
 * `props` may carry getters, which are preserved.
 */
export function c(Component, props = {}, ...kids) {
  if (kids.length === 0) return createComponent(Component, props);
  return createComponent(
    Component,
    mergeProps(props, {
      get children() {
        const built = kids.map((kid) => (typeof kid === "function" ? kid() : kid));
        return built.length === 1 ? built[0] : built;
      },
    }),
  );
}

let keystroke = 100;

export function findByTestId(node, testId) {
  if (!node) return null;
  if (node.props.testId === testId) return node;
  for (const child of node.children) {
    const found = findByTestId(child, testId);
    if (found) return found;
  }
  return null;
}

export function textOf(node) {
  return node.text ?? node.children.map(textOf).join("");
}

/** Mounts `code` (a function returning the UI) into a fresh reference renderer. */
export function mount(code, options = {}) {
  const renderer = referenceRenderer();
  const root = createRoot(renderer, options);
  root.render(code);
  const ui = {
    renderer,
    root,
    errors: renderer.errors,
    node: (testId) => findByTestId(ui.snapshot(), testId),
    snapshot: () => renderer.snapshot(),
    has: (testId) => ui.node(testId) !== null,
    /** Runs `fn` as one reactive batch and flushes the native queue (a state change from the owner). */
    set: (fn) => root.flushSync(fn),
    /** Emits a native event to the element with `testId` (a missing element is an error). */
    emit(testId, event) {
      const node = ui.node(testId);
      if (!node) throw new Error(`no element with testId ${testId}`);
      renderer.emit({ id: node.id, ...event });
    },
    /** One physical key press delivered to the element: key down then key up. */
    key(testId, name, over = {}) {
      const id = keystroke++;
      ui.emit(testId, { type: "keyDown", key: name, keystroke: id, ...over });
      // The key-down may have removed the element (a popup closing on Enter).
      if (ui.has(testId)) ui.emit(testId, { type: "keyUp", key: name, keystroke: id + 1000, ...over });
    },
    keyDown(testId, name, over = {}) {
      ui.emit(testId, { type: "keyDown", key: name, keystroke: keystroke++, ...over });
    },
    /** A window-level key event of the active root, with the generation its binding expects. */
    windowKey(name, over = {}) {
      renderer.emit({ type: "windowKeyDown", id: root.tree.binding.generation, key: name, keystroke: keystroke++, ...over });
    },
    focused: () => renderer.getFocusedElement(),
  };
  return ui;
}
