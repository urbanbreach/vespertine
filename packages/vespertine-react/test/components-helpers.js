// Fixtures for the headless component tests: a React root over the JavaScript reference tree,
// lookup by test id, native-style event emission and virtual time.

import { createNativeRenderer } from "@vespertine/host";
import { createReferenceNative } from "@vespertine/host/reference";
import React from "react";
import { createRoot, flushSync } from "../src/index.js";

export { installFakeTime } from "./fake-time.js";

export const h = React.createElement;

const realSetTimeout = globalThis.setTimeout;

/** Lets React run work it scheduled outside an event (timers), even while time is faked. */
export async function settle(times = 8) {
  for (let i = 0; i < times; i++) await new Promise((resolve) => realSetTimeout(resolve, 0));
}

/** Waits (on real timers) until `condition()` holds, failing after `timeoutMs` instead of guessing a tick count. */
export async function until(condition, timeoutMs = 3000) {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`condition not met within ${timeoutMs} ms: ${condition}`);
    await new Promise((resolve) => realSetTimeout(resolve, 1));
  }
}

let keystroke = 100;

export function mount(element, options = {}) {
  const errors = [];
  const renderer = createNativeRenderer({ native: createReferenceNative(), onError: (error) => errors.push(error) });
  const root = createRoot(renderer, { onUncaughtError: (error) => errors.push(error), ...options });
  flushSync(() => root.render(element));
  const ui = {
    renderer,
    root,
    errors,
    render(next) {
      flushSync(() => root.render(next));
    },
    node: (testId) => findByTestId(ui.snapshot(), testId),
    snapshot: () => renderer.snapshot(),
    has: (testId) => ui.node(testId) !== null,
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

/** Lets pending microtasks run. */
export async function microtasks(times = 3) {
  for (let i = 0; i < times; i++) await Promise.resolve();
}
