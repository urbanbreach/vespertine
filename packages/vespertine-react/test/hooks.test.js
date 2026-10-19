// APP-010 for React: window, insets and text-search hooks.

import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import React from "react";
import { createRoot, findRanges, flushSync, useTextSearch, useWindowInsets, useWindowSize } from "../src/index.js";
import { reconciler } from "../src/reconciler.js";
import { referenceRenderer } from "./helpers.js";

const h = React.createElement;

/** Replaces setInterval/clearInterval with a manual clock (React's scheduler uses other timers). */
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

test("A2: useWindowSize's first render has the real value and follows a resize at the default 100 ms interval", () => {
  const renderer = referenceRenderer();
  renderer.windowSize = { width: 1024, height: 768 };
  const root = createRoot(renderer);
  const seen = [];
  const Layout = () => {
    const size = useWindowSize();
    seen.push(size);
    return h("div", { testId: size.width > 900 ? "wide" : "narrow" });
  };
  flushSync(() => root.render(h(Layout)));
  assert.deepEqual(seen[0], { width: 1024, height: 768 });
  assert.equal(seen.length, 1, "the synchronous first read causes no extra render");
  assert.deepEqual([...intervals.timers.values()].map((t) => t.ms), [100]);
  assert.equal(renderer.snapshot().children[0].props.testId, "wide");

  renderer.windowSize = { width: 600, height: 400 };
  flushSync(() => intervals.tick());
  assert.deepEqual(seen.at(-1), { width: 600, height: 400 });
  assert.equal(renderer.snapshot().children[0].props.testId, "narrow");
  const renders = seen.length;
  flushSync(() => intervals.tick());
  assert.equal(seen.length, renders, "an unchanged size does not render again");
});

test("A2: replacing the interval unsubscribes the old observer; unmounting unsubscribes", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const Layout = ({ intervalMs }) => {
    useWindowSize({ intervalMs });
    return null;
  };
  flushSync(() => root.render(h(Layout, { intervalMs: undefined })));
  assert.deepEqual([...intervals.timers.values()].map((t) => t.ms), [100]);
  flushSync(() => root.render(h(Layout, { intervalMs: 250 })));
  assert.deepEqual([...intervals.timers.values()].map((t) => t.ms), [250], "the old subscription was removed");
  flushSync(() => root.render(h(Layout, { intervalMs: 5 })));
  assert.deepEqual([...intervals.timers.values()].map((t) => t.ms), [16], "intervals below 16 ms are raised");
  flushSync(() => root.render(null));
  assert.equal(intervals.timers.size, 0);
});

test("useWindowInsets reports the shared derived values and follows changes", () => {
  const renderer = referenceRenderer();
  renderer.windowInsets = { safeArea: { top: 4, right: 0, bottom: 0, left: 0 }, ime: { top: 0, right: 0, bottom: 100, left: 0 }, effective: { top: 4, right: 0, bottom: 100, left: 0 } };
  const root = createRoot(renderer);
  const seen = [];
  const Probe = () => {
    seen.push(useWindowInsets());
    return null;
  };
  flushSync(() => root.render(h(Probe)));
  assert.equal(seen[0].keyboardTop, 500);
  assert.equal(seen[0].keyboardVisible, true);
  assert.equal(seen[0].visibleHeight, 496);
  renderer.windowInsets = { safeArea: { top: 0, right: 0, bottom: 0, left: 0 }, ime: { top: 0, right: 0, bottom: 0, left: 0 }, effective: { top: 0, right: 0, bottom: 0, left: 0 } };
  flushSync(() => intervals.tick());
  assert.equal(seen.at(-1).keyboardVisible, false);
  flushSync(() => root.render(null));
  assert.equal(intervals.timers.size, 0);
});

test("A3: window hooks outside a root throw", () => {
  const errors = [];
  const container = reconciler.createContainer({ tree: null, children: [] }, 1, null, false, null, "", (error) => errors.push(error), () => {}, () => {}, null);
  const Probe = () => {
    useWindowSize();
    return null;
  };
  try {
    reconciler.updateContainerSync(h(Probe), container, null, null);
    reconciler.flushSyncWork();
  } catch {
    // The failure is reported through the uncaught-error callback.
  }
  assert.match(String(errors[0]?.message), /inside a Vespertine React root/);
});

test("A1: the search props spread onto a text element; navigation moves the active match and total follows the native count", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  let result;
  const Finder = ({ query }) => {
    result = useTextSearch({ query });
    return h("text", { ...result.props, testId: "doc" }, "Find red, red and RED.");
  };
  flushSync(() => root.render(h(Finder, { query: "red" })));
  const textId = renderer.snapshot().children[0].id;
  assert.equal(renderer.elements.get(textId).listeners.has("highlight"), true);
  assert.deepEqual(renderer.elements.get(textId).props.get("highlight"), { query: "red", activeIndex: 0 });
  assert.equal(result.total, 0);

  renderer.emit({ type: "highlight", id: textId, matchCount: 3 });
  assert.equal(result.total, 3);
  assert.equal(result.active, 0);
  flushSync(() => result.next());
  assert.equal(result.active, 1);
  assert.equal(renderer.elements.get(textId).props.get("highlight").activeIndex, 1, "the wash follows the active match");
  flushSync(() => result.next());
  flushSync(() => result.next());
  assert.equal(result.active, 0, "next wraps");
  flushSync(() => result.previous());
  assert.equal(result.active, 2, "previous wraps");
  flushSync(() => result.goTo(1));
  assert.equal(result.active, 1);
  flushSync(() => root.render(h(Finder, { query: "re" })));
  assert.equal(result.active, 0, "a new query resets the cursor");
  assert.equal(renderer.elements.get(textId).props.get("highlight").query, "re");

  assert.equal(result.controller.listenerCount, 1);
  flushSync(() => root.render(null));
  assert.equal(result.controller.listenerCount, 0, "unmounting unsubscribes from the revision");
});

test("findRanges is the shared helper", () => {
  assert.deepEqual(findRanges("a b a", "a"), [[0, 1], [4, 5]]);
});
