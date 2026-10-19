// The React adapter on the native offscreen test host (DEV-006): the production reconciler, root
// and event path over the real retained tree, layout, hit testing and event dispatch.

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { render, resetRender, useTextSearch } from "../src/index.js";
import { createTestHost, testHostAvailable } from "../src/testing.js";
import { settle } from "./helpers.js";

const h = React.createElement;
const available = testHostAvailable();
const native = (name, body) => test(name, { skip: available ? false : "no GPU test backend" }, body);

function byTestId(host, testId) {
  const [node] = host.renderer.findByTestId(testId);
  assert.ok(node, `no element with testId ${testId}`);
  return node;
}

function centerOf(node) {
  return [node.bounds.x + node.bounds.width / 2, node.bounds.y + node.bounds.height / 2];
}

native("mounted elements are laid out by the native engine", () => {
  const host = createTestHost({ width: 600, height: 400 });
  host.render(
    h(
      "div",
      { testId: "row", style: { display: "flex", width: 300, height: 80, flexDirection: "row", padding: 10, gap: 6 } },
      h("div", { testId: "a", style: { width: 50, height: 40 } }),
      h("div", { testId: "b", style: { flexGrow: 1, height: 40 } }),
    ),
  );
  assert.deepEqual(host.renderer.getWindowSize(), { width: 600, height: 400 });
  assert.deepEqual(byTestId(host, "row").bounds, { x: 0, y: 0, width: 300, height: 80 });
  assert.deepEqual(byTestId(host, "a").bounds, { x: 10, y: 10, width: 50, height: 40 });
  assert.deepEqual(byTestId(host, "b").bounds, { x: 66, y: 10, width: 224, height: 40 });
  host.unmount();
  assert.equal(host.renderer.getRetainedElementCount(), 0);
});

native("clicks reach React handlers with the pointer payload and a replaced callback takes effect", () => {
  const clicks = [];
  const host = createTestHost({ width: 300, height: 200 });
  const tree = (tag) =>
    h("div", {
      testId: "target",
      style: { width: 100, height: 100 },
      onClick: (event) => clicks.push([tag, event.x, event.y, event.button, event.clickCount]),
      onContextMenu: (event) => clicks.push(["menu", event.button]),
    });
  host.render(tree("first"));
  host.renderer.nativeSimulateClick(30, 40);
  host.renderer.nativeSimulateClick(30, 40, 2);
  host.render(tree("second"));
  host.renderer.nativeSimulateClick(31, 41);
  host.renderer.nativeSimulateClick(250, 150);
  assert.deepEqual(clicks, [
    ["first", 30, 40, 0, 1],
    ["menu", 2],
    ["second", 31, 41, 0, 1],
  ]);
  assert.deepEqual(host.errors, []);
});

native("destroyed elements stop receiving events and their identities are reported", () => {
  const clicks = [];
  const host = createTestHost({ width: 300, height: 200 });
  host.render(h("div", { style: { width: 200, height: 200 } }, h("div", { testId: "child", style: { width: 100, height: 100 }, onClick: () => clicks.push("child") })));
  const child = byTestId(host, "child");
  host.renderer.nativeSimulateClick(10, 10);
  host.render(h("div", { style: { width: 200, height: 200 } }));
  host.renderer.nativeSimulateClick(10, 10);
  assert.deepEqual(clicks, ["child"]);
  assert.equal(host.renderer.getElement(child.id), null);
});

native("keyboard events carry one dispatch identity and tab moves focus through the tab order", () => {
  const log = [];
  const host = createTestHost({
    width: 300,
    height: 200,
    onWindowKeyDown: (event) => log.push(["window", event.key, event.keystroke]),
  });
  host.render(
    h(
      "div",
      { style: { width: 300, height: 200, flexDirection: "row" } },
      ["one", "two", "three"].map((name, index) =>
        h("div", {
          key: name,
          testId: name,
          tabIndex: name === "two" ? -1 : 0,
          style: { width: 50, height: 50 },
          onFocus: () => log.push(["focus", name]),
          onBlur: () => log.push(["blur", name]),
          onKeyDown: (event) => log.push(["key", name, event.key, event.keystroke]),
        }),
      ),
    ),
  );
  const { renderer } = host;
  renderer.focusElement(byTestId(host, "one").id);
  assert.equal(renderer.getFocusedElementId(), byTestId(host, "one").id);
  renderer.simulateKeystrokes("x");
  const key = log.find((entry) => entry[0] === "key");
  const window = log.find((entry) => entry[0] === "window");
  assert.deepEqual(key.slice(1, 3), ["one", "x"]);
  assert.equal(key[3], window[2], "node and window callbacks of one keystroke share the dispatch identity");
  renderer.simulateKeystrokes("tab");
  assert.equal(renderer.getFocusedElementId(), byTestId(host, "three").id, "a negative tab index is skipped");
  renderer.simulateKeystrokes("shift-tab");
  assert.equal(renderer.getFocusedElementId(), byTestId(host, "one").id);
  renderer.focusElement(byTestId(host, "two").id);
  assert.equal(renderer.getFocusedElementId(), byTestId(host, "two").id, "programmatic focus works for negative tab index");
  assert.deepEqual(host.errors, []);
});

native("events of a replaced root generation are ignored", () => {
  const seen = [];
  const host = createTestHost({ width: 200, height: 200, onWindowKeyDown: (event) => seen.push("first") });
  host.render(h("div", { style: { width: 100, height: 100 } }));
  host.renderer.simulateKeystrokes("a");
  host.unmount();
  host.renderer.simulateKeystrokes("b");
  assert.deepEqual(seen, ["first"]);
});

native("scroll containers report their offset and scroll programmatically", () => {
  const host = createTestHost({ width: 300, height: 200 });
  const changes = [];
  host.render(
    h(
      "div",
      { testId: "scroller", style: { width: 100, height: 100, overflowY: "scroll" }, onScrollChange: (event) => changes.push([event.x, event.y]) },
      h("div", { style: { width: 100, height: 400 } }),
    ),
  );
  const scroller = byTestId(host, "scroller");
  assert.deepEqual(host.renderer.getScrollOffset(scroller.id), { x: 0, y: 0 });
  host.renderer.scrollTo(scroller.id, 0, 120);
  assert.deepEqual(host.renderer.getScrollOffset(scroller.id), { x: 0, y: -120 });
  host.renderer.flush();
  assert.deepEqual(changes, [[0, -120]]);
  host.renderer.nativeSimulateScrollWheel(50, 50, 0, -30);
  assert.equal(host.renderer.getScrollOffset(scroller.id).y, -150, "a negative wheel delta scrolls toward the far side");
});

native("layout notifications fire at mount and on size change only", () => {
  const layouts = [];
  const host = createTestHost({ width: 300, height: 200 });
  const tree = (width, left) =>
    h("div", { style: { width: 300, height: 100, paddingLeft: left } }, h("div", { testId: "box", style: { width, height: 40 }, onLayout: (event) => layouts.push([event.width, event.height]) }));
  host.render(tree(100, 0));
  host.renderer.flush();
  host.render(tree(100, 20));
  host.renderer.flush();
  host.render(tree(120, 20));
  host.renderer.flush();
  assert.deepEqual(layouts, [
    [100, 40],
    [120, 40],
  ]);
});

native("the runtime error view shows in the same window and Reload remounts the application (DEV-010 A1, A4)", async () => {
  const host = createTestHost({ width: 600, height: 400 });
  const { renderer } = host;
  host.root.unmount();
  let broken = true;
  function App() {
    if (broken) throw new Error("Example failure");
    return h("div", { testId: "app", style: { width: 100, height: 100 } });
  }
  const originalError = console.error;
  const logged = [];
  console.error = (...args) => logged.push(args.join(" "));
  try {
    render(h(App), { renderer, onUncaughtError: () => {} });
    await settle(5);
    renderer.flush();
    const view = renderer.findByTestId("error-view");
    assert.equal(view.length, 1);
    assert.equal(renderer.getFocusedElementId(), null);
    assert.ok(renderer.getAllText().includes("Uncaught runtime errors:"));
    assert.deepEqual(byTestId(host, "error-heading").bounds, { x: 32, y: 32, width: 536, height: 44 });
    const panel = byTestId(host, "error-panel");
    assert.equal(panel.bounds.x, 32);
    assert.equal(panel.bounds.y, 96);
    assert.equal(panel.bounds.width, 536);
    const message = byTestId(host, "error-message");
    assert.deepEqual([message.bounds.x, message.bounds.y, message.bounds.width, message.bounds.height], [48, 112, 504, 28]);
    const reload = renderer.findByText("Reload")[0] ?? null;
    assert.ok(reload);
    const button = byTestId(host, "error-reload");
    assert.equal(button.bounds.height, 40);
    assert.equal(button.bounds.x, 32);
    broken = false;
    renderer.nativeSimulateClick(...centerOf(button));
    await settle(5);
    renderer.flush();
    assert.equal(renderer.findByTestId("error-view").length, 0);
    assert.equal(renderer.findByTestId("app").length, 1);
  } finally {
    console.error = originalError;
    resetRender();
  }
});

native("a drag selects painted text, reports it to the root and the selection can be cleared (APP-010 A4)", () => {
  const changes = [];
  const host = createTestHost({ width: 400, height: 200, onSelectionChange: (event) => changes.push(event.text) });
  host.render(h("div", { style: { width: 400, height: 100, padding: 10 } }, h("text", { testId: "line", style: { fontSize: 16 } }, "hello selectable world")));
  const line = byTestId(host, "line");
  assert.equal(host.renderer.getSelectedText(), null);
  const y = line.bounds.y + line.bounds.height / 2;
  const selected = host.renderer.dragSelect(line.bounds.x + 1, y, line.bounds.x + line.bounds.width, y);
  assert.equal(selected, "hello selectable world");
  assert.equal(host.renderer.getSelectedText(), "hello selectable world");
  assert.deepEqual(changes, ["hello selectable world"]);
  host.renderer.clearSelection();
  assert.equal(host.renderer.getSelectedText(), null);
  assert.deepEqual(changes, ["hello selectable world", null]);
});

native("userSelect none text paints but cannot be selected", () => {
  const host = createTestHost({ width: 400, height: 200 });
  host.render(h("div", { style: { width: 400, height: 100, padding: 10, userSelect: "none" } }, h("text", { testId: "line", style: { fontSize: 16 } }, "locked text")));
  const line = byTestId(host, "line");
  const y = line.bounds.y + line.bounds.height / 2;
  assert.equal(host.renderer.dragSelect(line.bounds.x + 1, y, line.bounds.x + line.bounds.width, y), null);
  assert.ok(host.renderer.getPaintedText().includes("locked text"));
});

native("text search washes follow the match count and the active index (APP-010 A1)", () => {
  let search;
  function Searchable({ query }) {
    search = useTextSearch({ query });
    return h("text", { testId: "doc", style: { fontSize: 16 }, ...search.props }, "Red red and RED, not blue");
  }
  const host = createTestHost({ width: 500, height: 100 });
  host.render(h(Searchable, { query: "red" }));
  const washes = () => host.renderer.getPaintedHighlights();
  assert.equal(washes().length, 3);
  assert.deepEqual(washes().map((wash) => [wash.startUtf16, wash.endUtf16, wash.active]), [
    [0, 3, true],
    [4, 7, false],
    [12, 15, false],
  ]);
  host.render(h(Searchable, { query: "red" }));
  assert.equal(search.total, 3, "the native match count reaches the controller");
  search.next();
  host.render(h(Searchable, { query: "red" }));
  assert.equal(search.active, 1);
  assert.deepEqual(washes().map((wash) => wash.active), [false, true, false]);
  search.goTo(2);
  host.render(h(Searchable, { query: "red" }));
  assert.deepEqual(washes().map((wash) => wash.active), [false, false, true]);
  host.render(h(Searchable, { query: "blue" }));
  assert.equal(washes().length, 1);
  assert.equal(host.renderer.findByTestId("doc")[0].id, washes()[0].elementId);
});
