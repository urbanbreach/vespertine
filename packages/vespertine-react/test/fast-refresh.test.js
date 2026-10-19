// APP-009 R10 / A1: Fast Refresh keeps component state and updates painted output.

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import RefreshRuntime from "react-refresh/runtime";
import { createRoot, flushSync } from "../src/index.js";
import { referenceRenderer, settle } from "./helpers.js";

const h = React.createElement;

test("Fast Refresh swaps a component implementation and keeps its state", async () => {
  RefreshRuntime.injectIntoGlobalHook(globalThis);
  const renderer = referenceRenderer();
  const root = createRoot(renderer);

  function Counter() {
    const [n, setN] = React.useState(0);
    return h("div", { testId: "c", onClick: () => setN(n + 1) }, h("text", {}, `v1:${n}`));
  }
  RefreshRuntime.register(Counter, "Counter");
  flushSync(() => root.render(h(Counter)));
  const id = renderer.snapshot().children[0].id;
  renderer.emit({ type: "click", id });
  renderer.emit({ type: "click", id });
  assert.equal(renderer.snapshot().children[0].children[0].text, "v1:2");

  function CounterV2() {
    const [n, setN] = React.useState(0);
    return h("div", { testId: "c", onClick: () => setN(n + 1) }, h("text", {}, `v2:${n}`));
  }
  RefreshRuntime.register(CounterV2, "Counter");
  RefreshRuntime.performReactRefresh();
  await settle(4);
  const painted = renderer.snapshot().children[0];
  assert.equal(painted.children[0].text, "v2:2", "painted output changed, state retained");
  assert.equal(painted.id, id, "the native element was kept");
});

test("a missing development-tools hook does not prevent startup", () => {
  const saved = globalThis.__REACT_DEVTOOLS_GLOBAL_HOOK__;
  delete globalThis.__REACT_DEVTOOLS_GLOBAL_HOOK__;
  try {
    const renderer = referenceRenderer();
    const root = createRoot(renderer);
    flushSync(() => root.render(h("div")));
    assert.equal(renderer.elements.size, 2);
    root.unmount();
  } finally {
    globalThis.__REACT_DEVTOOLS_GLOBAL_HOOK__ = saved;
  }
});
