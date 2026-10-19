// The Solid adapter on the native offscreen test host (DEV-006): the production universal
// renderer, root and event path over the real retained tree, layout and event dispatch.

import assert from "node:assert/strict";
import { test } from "node:test";
import { createSignal } from "../src/index.js";
import { createTestHost, testHostAvailable } from "../src/testing.js";
import { h } from "./helpers.js";

const available = testHostAvailable();
const native = (name, body) => test(name, { skip: available ? false : "no GPU test backend" }, body);

const byTestId = (host, testId) => {
  const [node] = host.renderer.findByTestId(testId);
  assert.ok(node, `no element with testId ${testId}`);
  return node;
};

native("signal-driven updates repaint through the native engine", () => {
  const [width, setWidth] = createSignal(40);
  const [label, setLabel] = createSignal("one");
  const host = createTestHost({ width: 400, height: 300 });
  host.render(() =>
    h(
      "div",
      { style: () => ({ display: "flex", width: 200, height: 100 }) },
      h("div", { testId: "bar", style: () => ({ width: width(), height: 20, backgroundColor: "#336699" }) }),
      h("text", { testId: "label" }, label),
    ),
  );
  assert.equal(byTestId(host, "bar").bounds.width, 40);
  assert.ok(host.renderer.getPaintedText().includes("one"));
  host.root.flushSync(() => setWidth(90));
  host.root.flushSync(() => setLabel("two"));
  host.renderer.flush();
  assert.equal(byTestId(host, "bar").bounds.width, 90);
  assert.ok(host.renderer.getPaintedText().includes("two"));
});

native("a native click flushes the handler's signal update before the helper returns", () => {
  const [count, setCount] = createSignal(0);
  const host = createTestHost({ width: 300, height: 200 });
  host.render(() =>
    h(
      "div",
      { testId: "button", style: { width: 100, height: 50 }, onClick: () => setCount(count() + 1) },
      h("text", { testId: "count" }, () => `n=${count()}`),
    ),
  );
  host.renderer.nativeSimulateClick(10, 10);
  host.renderer.nativeSimulateClick(20, 20);
  host.renderer.flush();
  assert.equal(byTestId(host, "count").children[0].text, "n=2");
  assert.ok(host.renderer.getPaintedText().includes("n=2"));
});

native("a second render discards the listeners of the previous generation", () => {
  const seen = [];
  const host = createTestHost({ width: 300, height: 200 });
  host.render(() => h("div", { style: { width: 100, height: 100 }, onClick: () => seen.push("first") }));
  host.renderer.nativeSimulateClick(10, 10);
  host.render(() => h("div", { style: { width: 100, height: 100 }, onClick: () => seen.push("second") }));
  host.renderer.nativeSimulateClick(10, 10);
  assert.deepEqual(seen, ["first", "second"]);
  assert.deepEqual(host.errors, []);
});

native("layout notifications are delivered to Solid handlers", () => {
  const sizes = [];
  const [width, setWidth] = createSignal(60);
  const host = createTestHost({ width: 300, height: 200 });
  host.render(() => h("div", { style: () => ({ width: width(), height: 30 }), onLayout: (event) => sizes.push([event.width, event.height]) }));
  host.renderer.flush();
  host.root.flushSync(() => setWidth(80));
  host.renderer.flush();
  assert.deepEqual(sizes, [
    [60, 30],
    [80, 30],
  ]);
});
