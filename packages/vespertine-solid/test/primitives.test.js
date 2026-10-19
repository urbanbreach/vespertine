// APP-010 for Solid: window, selection and text-search primitives.

import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { createRoot as createReactiveRoot } from "solid-js";
import {
  createComponent,
  createRoot,
  createSelectedText,
  createSignal,
  createTextSearch,
  createWindowInsets,
  createWindowSize,
  findRanges,
} from "../src/index.js";
import { h, referenceRenderer } from "./helpers.js";

let intervals;
const real = { setInterval: globalThis.setInterval, clearInterval: globalThis.clearInterval };
beforeEach(() => {
  const timers = new Map();
  let next = 1;
  intervals = {
    timers,
    tick() {
      for (const timer of [...timers.values()]) timer.fn();
    },
  };
  globalThis.setInterval = (fn, ms) => {
    timers.set(next, { fn, ms });
    return next++;
  };
  globalThis.clearInterval = (handle) => timers.delete(handle);
});
afterEach(() => Object.assign(globalThis, real));

test("A5: createWindowSize returns an accessor whose value changes over time", () => {
  const renderer = referenceRenderer();
  renderer.windowSize = { width: 1024, height: 768 };
  const root = createRoot(renderer);
  let size;
  root.render(() => {
    size = createWindowSize();
    return h("div", { testId: () => (size().width > 900 ? "wide" : "narrow") });
  });
  assert.equal(typeof size, "function");
  assert.deepEqual(size(), { width: 1024, height: 768 });
  assert.deepEqual([...intervals.timers.values()].map((t) => t.ms), [100]);
  renderer.windowSize = { width: 600, height: 400 };
  root.flushSync(() => intervals.tick());
  assert.deepEqual(size(), { width: 600, height: 400 });
  assert.equal(renderer.snapshot().children[0].props.testId, "narrow");
});

test("options pass through to the shared observer and cleanup ends the subscription", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  root.render(() => {
    createWindowSize({ intervalMs: 250 });
    createWindowInsets({ intervalMs: 5 });
    return h("div");
  });
  assert.deepEqual([...intervals.timers.values()].map((t) => t.ms).sort((a, b) => a - b), [16, 250]);
  root.unmount();
  assert.equal(intervals.timers.size, 0);
});

test("A3: window and selection primitives outside a root context throw", () => {
  createReactiveRoot((dispose) => {
    assert.throws(() => createWindowSize(), /inside a Vespertine Solid root/);
    assert.throws(() => createWindowInsets(), /inside a Vespertine Solid root/);
    assert.throws(() => createSelectedText(), /inside a Vespertine Solid root/);
    dispose();
  });
});

test("A4: createSelectedText reports the current selection on first read and follows selection changes", () => {
  const renderer = referenceRenderer();
  renderer.selectedText = "initial";
  const root = createRoot(renderer);
  let selected;
  root.render(() => {
    selected = createSelectedText();
    return h("text", {}, () => selected() ?? "none");
  });
  assert.equal(selected(), "initial");
  assert.deepEqual(renderer.selectionNotifications, [true], "native notifications are on while a subscriber exists");
  renderer.emit({ type: "windowSelectionChange", id: root.tree.generation, text: "dragged" });
  assert.equal(selected(), "dragged");
  assert.equal(renderer.snapshot().children[0].children[0].text, "dragged");
  renderer.emit({ type: "windowSelectionChange", id: root.tree.generation - 1, text: "stale" });
  assert.equal(selected(), "dragged", "a stale generation is ignored");
  root.unmount();
  assert.deepEqual(renderer.selectionNotifications, [true, false]);
});

test("A3: native selection notifications are enabled only while a subscriber exists", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  let disposeFirst;
  root.render(() =>
    createComponent(
      () => {
        createReactiveRoot((dispose) => {
          disposeFirst = dispose;
          createSelectedText();
        });
        return h("div");
      },
      {},
    ),
  );
  assert.deepEqual(renderer.selectionNotifications, [true]);
  disposeFirst();
  assert.deepEqual(renderer.selectionNotifications, [true, false]);
  assert.equal(root.context.subscribeSelection(() => {}) !== undefined, true);
});

test("A1: createTextSearch exposes reactive props, total and active and the shared navigation", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const [query, setQuery] = createSignal("red");
  let search;
  root.render(() => {
    search = createTextSearch(() => ({ query: query() }));
    return h("text", { highlight: () => search.props.highlight, onHighlight: (event) => search.props.onHighlight(event), testId: "doc" }, "Find red, red and RED.");
  });
  const textId = renderer.snapshot().children[0].id;
  assert.deepEqual(renderer.elements.get(textId).props.get("highlight"), { query: "red", activeIndex: 0 });
  renderer.emit({ type: "highlight", id: textId, matchCount: 3 });
  assert.equal(search.total, 3);
  root.flushSync(() => search.next());
  assert.equal(search.active, 1);
  assert.equal(renderer.elements.get(textId).props.get("highlight").activeIndex, 1);
  root.flushSync(() => search.goTo(2));
  root.flushSync(() => search.next());
  assert.equal(search.active, 0, "wraps");
  root.flushSync(() => search.previous());
  assert.equal(search.active, 2);
  root.flushSync(() => setQuery("re"));
  assert.equal(search.active, 0);
  assert.equal(renderer.elements.get(textId).props.get("highlight").query, "re");
  assert.equal(search.controller.listenerCount, 1);
  root.unmount();
  assert.equal(search.controller.listenerCount, 0, "disposing the owner unsubscribes from the revision");
});

test("findRanges is shared with the React adapter's helper", async () => {
  assert.deepEqual(findRanges("a b a", "a"), [[0, 1], [4, 5]]);
  const react = await import("../../vespertine-react/src/hooks.js");
  assert.equal(react.findRanges, findRanges);
});
