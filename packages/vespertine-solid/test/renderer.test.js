// APP-009 A2: the Solid universal renderer on the retained host tree.

import assert from "node:assert/strict";
import { test } from "node:test";
import { For, Show, createComponent, createRoot, createSignal, insert, createElement, insertNode, onCleanup, setProp } from "../src/index.js";
import { h, referenceRenderer, settle } from "./helpers.js";

function mount(code, options) {
  const renderer = referenceRenderer();
  const root = createRoot(renderer, options);
  root.render(code);
  return { renderer, root };
}

test("the initial render flushes immediately in one batch", () => {
  const { renderer } = mount(() => h("div", { testId: "a" }, h("text", {}, "hello")));
  assert.equal(renderer.batches.length, 1);
  const snapshot = renderer.snapshot();
  assert.equal(snapshot.children[0].props.testId, "a");
  assert.equal(snapshot.children[0].children[0].children[0].text, "hello");
});

test("signal-driven text and property updates are incremental", () => {
  const [text, setText] = createSignal("one");
  const [color, setColor] = createSignal("red");
  const { renderer, root } = mount(() =>
    h("div", { style: () => ({ color: color(), width: 3 }) }, h("text", {}, text), h("text", {}, "static")),
  );
  renderer.resetCounts();
  root.flushSync(() => setText("two"));
  assert.deepEqual(renderer.batches.at(-1).map((op) => op[0]), ["setText"]);
  renderer.resetCounts();
  root.flushSync(() => setColor("blue"));
  assert.deepEqual(renderer.batches.at(-1).map((op) => op[0]), ["setStyle"]);
  assert.deepEqual(renderer.snapshot().children[0].style, { color: "blue", width: 3 });
  renderer.resetCounts();
  root.flushSync(() => setColor("blue"));
  assert.equal(renderer.batches.length, 0, "an equal value sends nothing");
});

test("a reactive change outside an event dispatch schedules a microtask flush", async () => {
  const [text, setText] = createSignal("a");
  const { renderer } = mount(() => h("text", {}, text));
  const before = renderer.batches.length;
  setText("b");
  assert.equal(renderer.batches.length, before, "not flushed synchronously");
  await Promise.resolve();
  assert.equal(renderer.batches.length, before + 1);
  assert.equal(renderer.snapshot().children[0].children[0].text, "b");
});

test("an external native event runs as one batch, flushing before it returns", () => {
  const [count, setCount] = createSignal(0);
  const { renderer, root } = mount(() =>
    h("div", { onClick: () => setCount(count() + 1), testId: "t" }, h("text", {}, () => `n=${count()}`)),
  );
  const id = renderer.snapshot().children[0].id;
  renderer.emit({ type: "click", id });
  assert.equal(renderer.snapshot().children[0].children[0].children[0].text, "n=1");
  root.dispatch({ type: "click", id });
  assert.equal(renderer.snapshot().children[0].children[0].children[0].text, "n=2");
});

test("dispatch and flushSync flush in a finally path when user code throws", () => {
  const [count, setCount] = createSignal(0);
  const { renderer, root } = mount(() =>
    h("div", { onClick: () => { setCount(1); throw new Error("handler"); } }, h("text", {}, () => `n=${count()}`)),
  );
  const id = renderer.snapshot().children[0].id;
  assert.throws(() => root.dispatch({ type: "click", id }), /handler/);
  assert.equal(renderer.snapshot().children[0].children[0].children[0].text, "n=1");
  assert.throws(() => root.flushSync(() => { setCount(2); throw new Error("sync"); }), /sync/);
  assert.equal(renderer.snapshot().children[0].children[0].children[0].text, "n=2");
});

test("a keyed reorder preserves node identities", () => {
  const [items, setItems] = createSignal(["a", "b", "c"]);
  const { renderer, root } = mount(() =>
    h("div", {}, createComponent(For, { get each() { return items(); }, children: (key) => h("div", { testId: key }) })),
  );
  const ids = () => Object.fromEntries(renderer.snapshot().children[0].children.map((c) => [c.props.testId, c.id]));
  const before = ids();
  root.flushSync(() => setItems(["c", "a", "b"]));
  assert.deepEqual(ids(), before);
  assert.deepEqual(renderer.snapshot().children[0].children.map((c) => c.props.testId), ["c", "a", "b"]);
  assert.equal(renderer.count("destroyElement"), 0);
  root.flushSync(() => setItems(["c", "x"]));
  assert.deepEqual(renderer.snapshot().children[0].children.map((c) => c.props.testId), ["c", "x"]);
  assert.equal(renderer.count("destroyElement"), 2);
});

test("removing a node cleans the listeners of the whole removed subtree", () => {
  const [show, setShow] = createSignal(true);
  const { renderer, root } = mount(() =>
    h("div", {}, createComponent(Show, { get when() { return show(); }, get children() { return h("div", { onClick() {} }, h("div", { onKeyDown() {} })); } })),
  );
  assert.equal(root.tree.handlerCount(), 2);
  root.flushSync(() => setShow(false));
  assert.equal(root.tree.handlerCount(), 0);
  assert.equal(renderer.elements.size, 2);
});

test("layout events reach the registered handler", () => {
  const seen = [];
  const { renderer } = mount(() => h("div", { onLayout: (event) => seen.push([event.width, event.height]) }));
  const id = renderer.snapshot().children[0].id;
  assert.deepEqual(renderer.snapshot().children[0].listeners, ["layout"]);
  renderer.emit({ type: "layout", id, x: 0, y: 0, width: 30, height: 12 });
  assert.deepEqual(seen, [[30, 12]]);
});

test("an unsupported intrinsic type throws", () => {
  assert.throws(() => createElement("marquee"), /Unsupported intrinsic element "marquee"/);
  for (const type of ["div", "text", "img", "svg", "canvas", "input", "textarea", "anchored", "code", "diff", "markdown", "virtual-list"]) {
    assert.doesNotThrow(() => createElement(type));
  }
});

test("an insertion anchor that is not a child of the target parent throws", () => {
  const parent = createElement("div");
  const stranger = createElement("div");
  assert.throws(() => insertNode(parent, createElement("div"), stranger), /anchor is not a child/);
});

test("moving a node between different roots throws", () => {
  const first = mount(() => h("div", { testId: "one" }));
  const second = mount(() => h("div", { testId: "two" }));
  const nodeOne = first.root.tree.container.children[0];
  const nodeTwo = second.root.tree.container.children[0];
  assert.throws(() => insertNode(nodeTwo, nodeOne), /between roots/);
});

test("a second render discards the prior generation, its listeners and its native content", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const calls = [];
  root.render(() => h("div", { onClick: () => calls.push("old") }));
  const oldId = renderer.snapshot().children[0].id;
  const oldGeneration = root.tree.generation;
  root.render(() => h("div", { onClick: () => calls.push("new") }));
  const newId = renderer.snapshot().children[0].id;
  assert.ok(newId > oldId);
  assert.ok(root.tree.generation > oldGeneration);
  assert.equal(renderer.elements.has(oldId), false);
  renderer.emit({ type: "click", id: oldId });
  assert.deepEqual(calls, []);
  renderer.emit({ type: "click", id: newId });
  assert.deepEqual(calls, ["new"]);
});

test("a render disposes the previous reactive owner", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const cleanups = [];
  root.render(() => {
    onCleanup(() => cleanups.push("first"));
    return h("div");
  });
  root.render(() => {
    onCleanup(() => cleanups.push("second"));
    return h("div");
  });
  assert.deepEqual(cleanups, ["first"]);
  root.unmount();
  assert.deepEqual(cleanups, ["first", "second"]);
});

test("identities allocated after unmount and remount exceed every earlier identity", () => {
  const renderer = referenceRenderer();
  const first = createRoot(renderer);
  first.render(() => h("div", {}, h("div"), h("div")));
  const used = [...renderer.elements.keys()];
  first.unmount();
  assert.equal(renderer.elements.size, 0);
  const second = createRoot(renderer);
  second.render(() => h("div"));
  assert.ok(Math.min(...renderer.elements.keys()) > Math.max(...used));
});

test("one renderer owns one root; rendering an unmounted root throws", () => {
  const renderer = referenceRenderer();
  const first = createRoot(renderer);
  assert.throws(() => createRoot(renderer), /one renderer owns one window/i);
  first.unmount();
  assert.throws(() => first.render(() => h("div")), /unmounted root/);
});

test("insert and setProp work on placed nodes only through the host tree", async () => {
  const { renderer } = mount(() => {
    const el = createElement("div");
    insert(el, "text");
    setProp(el, "testId", "x");
    return el;
  });
  assert.equal(renderer.snapshot().children[0].props.testId, "x");
  await settle(1);
});
