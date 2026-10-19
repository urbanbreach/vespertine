import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { createPortal, createRoot, flushSync, useRendererContext } from "../src/index.js";
import { referenceRenderer, settle } from "./helpers.js";

const h = React.createElement;

function mount(node, options) {
  const renderer = referenceRenderer();
  const root = createRoot(renderer, options);
  flushSync(() => root.render(node));
  return { renderer, root };
}

test("mount places a tree in one batch and unmount destroys it in one batch", () => {
  const { renderer, root } = mount(h("div", { style: { width: 10 } }, h("div", { testId: "a" }, "hello")));
  assert.equal(renderer.batches.length, 1);
  const snapshot = renderer.snapshot();
  assert.equal(snapshot.type, "div");
  assert.equal(snapshot.children[0].style.width, 10);
  assert.equal(snapshot.children[0].children[0].props.testId, "a");
  assert.equal(snapshot.children[0].children[0].children[0].text, "hello");
  root.unmount();
  assert.equal(renderer.elements.size, 0);
  assert.equal(renderer.batches.length, 2);
});

test("rendering an unmounted root throws and unmount is idempotent", () => {
  const { root } = mount(h("div"));
  root.unmount();
  root.unmount();
  assert.throws(() => root.render(h("div")), /unmounted root/);
});

test("an empty commit sends no batch", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  flushSync(() => root.render(null));
  assert.equal(renderer.batches.length, 0);
  flushSync(() => root.render(h("div")));
  assert.equal(renderer.batches.length, 1);
  flushSync(() => root.render(h("div")));
  assert.equal(renderer.batches.length, 1);
});

test("A1: abandoned concurrent work creates no native node, identity or registration", async () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  let setPhase;
  let siblingRenders = 0;
  const never = new Promise(() => {});
  const Suspender = ({ suspend }) => {
    if (suspend) throw never;
    return h("div");
  };
  const Sibling = () => {
    siblingRenders++;
    return h("div", { testId: "late", onClick() {} }, h("text", {}, "late"));
  };
  function App() {
    const [phase, set] = React.useState(0);
    setPhase = set;
    return h(
      "div",
      { testId: "app" },
      h("div", { testId: "stable" }),
      phase > 0 ? h(Sibling) : null,
      h(React.Suspense, { fallback: null }, h(Suspender, { suspend: phase > 0 })),
    );
  }
  flushSync(() => root.render(h(App)));
  const before = { batches: renderer.batches.length, elements: renderer.elements.size, handlers: root.tree.handlerCount() };
  React.startTransition(() => setPhase(1));
  await settle(6);
  assert.ok(siblingRenders > 0, "the transition rendered the new sibling");
  assert.equal(renderer.batches.length, before.batches);
  assert.equal(renderer.elements.size, before.elements);
  assert.equal(root.tree.handlerCount(), before.handlers);
  assert.equal(renderer.count("createElement"), before.elements);
});

test("A1: deleting a subtree destroys it natively, text included, and unregisters every identity", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const App = ({ show }) =>
    h(
      "div",
      {},
      show ? h("div", { onClick() {}, testId: "gone" }, h("div", { onKeyDown() {} }, "text one", h("text", {}, "two"))) : null,
    );
  flushSync(() => root.render(h(App, { show: true })));
  assert.equal(renderer.elements.size, 6);
  const handlersBefore = root.tree.handlerCount();
  assert.equal(handlersBefore, 2);
  flushSync(() => root.render(h(App, { show: false })));
  assert.equal(renderer.destroyedLog.length, 4, "the destroy batch reports the element, its descendants and its text nodes");
  assert.equal(root.tree.handlerCount(), 0);
  assert.equal(renderer.elements.size, 2);
});

test("A1: a memoized consumer of the root context is not rerendered by a repeated render", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  let renders = 0;
  const Consumer = React.memo(function Consumer() {
    renders++;
    return h("div", { testId: "c" }, String(useRendererContext().renderer === renderer));
  });
  flushSync(() => root.render(h("div", {}, h(Consumer))));
  flushSync(() => root.render(h("div", {}, h(Consumer))));
  flushSync(() => root.render(h("div", {}, h(Consumer))));
  assert.equal(renders, 1);
  assert.equal(root.context, root.context);
});

test("A1: events carrying an identity of a prior root generation are ignored", () => {
  const renderer = referenceRenderer();
  const calls = [];
  const first = createRoot(renderer, { onWindowKeyDown: () => calls.push("old window") });
  let oldId;
  flushSync(() => first.render(h("div", { onClick: () => calls.push("old click"), ref: (n) => (oldId = n?.id ?? oldId) })));
  assert.equal(typeof oldId, "number");
  const oldGeneration = first.tree.generation;
  first.unmount();
  const second = createRoot(renderer, { onWindowKeyDown: () => calls.push("new window") });
  flushSync(() => second.render(h("div", { onClick: () => calls.push("new click") })));
  renderer.emit({ type: "click", id: oldId });
  renderer.emit({ type: "windowKeyDown", id: oldGeneration, key: "a", keystroke: 1 });
  assert.deepEqual(calls, []);
  renderer.emit({ type: "windowKeyDown", id: second.tree.generation, key: "a", keystroke: 2 });
  assert.deepEqual(calls, ["new window"]);
});

test("A1: replacing a callback keeps the native listener; removing it disables the listener", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const log = [];
  const App = ({ onClick }) => h("div", { onClick, testId: "t" });
  flushSync(() => root.render(h(App, { onClick: () => log.push("a") })));
  const id = renderer.root === null ? null : renderer.snapshot().children[0].id;
  assert.equal(renderer.count("setEventListener"), 1);
  renderer.emit({ type: "click", id });
  flushSync(() => root.render(h(App, { onClick: () => log.push("b") })));
  assert.equal(renderer.count("setEventListener"), 1, "no listener toggle when only the callback changes");
  assert.equal(renderer.batches.length, 1, "an equivalent commit sends nothing");
  renderer.emit({ type: "click", id });
  flushSync(() => root.render(h(App, { onClick: undefined })));
  assert.equal(renderer.count("setEventListener"), 2);
  assert.deepEqual(renderer.batches.at(-1), [["setEventListener", id, "click", false]]);
  renderer.emit({ type: "click", id });
  assert.deepEqual(log, ["a", "b"]);
});

test("A1: a portal keeps the caller's context and appends after the target's existing children", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const Theme = React.createContext("none");
  let target = null;
  const Inner = () => h("text", { testId: "inner" }, React.useContext(Theme));
  function App({ tick }) {
    const [mounted, setMounted] = React.useState(null);
    return h(
      Theme.Provider,
      { value: "dark" },
      h(
        "div",
        {},
        h("div", { testId: "target", ref: (node) => { target = node; setMounted(node); } }, h("div", { testId: "existing" })),
        mounted ? createPortal(h(Inner), mounted, tick % 2 === 0 ? "k" : 7) : null,
      ),
    );
  }
  flushSync(() => root.render(h(App, { tick: 0 })));
  flushSync(() => {});
  const container = renderer.snapshot().children[0].children[0];
  assert.deepEqual(container.children.map((c) => c.props.testId), ["existing", "inner"]);
  assert.equal(container.children[1].text, "dark");
  const targetId = target.id;
  const innerId = container.children[1].id;
  flushSync(() => root.render(h(App, { tick: 1 })));
  const again = renderer.snapshot().children[0].children[0];
  assert.equal(again.id, targetId, "the target container keeps its identity");
  assert.deepEqual(again.children.map((c) => c.props.testId), ["existing", "inner"]);
  assert.equal(typeof innerId, "number");
});

test("a portal into a target of another root is rejected", () => {
  const rootB = createRoot(referenceRenderer());
  let targetB = null;
  flushSync(() => rootB.render(h("div", { ref: (node) => (targetB = node) })));
  const errors = [];
  const guarded = createRoot(referenceRenderer(), { onUncaughtError: (error) => errors.push(error) });
  const Portal = () => createPortal(h("div"), targetB);
  flushSync(() => guarded.render(h(Portal)));
  assert.match(String(errors[0]?.message), /portal target must belong to the root/);
  assert.throws(() => createPortal(h("div"), {}), /mounted host element/);
});

test("moving nodes keeps their native identities", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const Items = ({ order }) => h("div", {}, order.map((key) => h("div", { key, testId: key })));
  flushSync(() => root.render(h(Items, { order: ["a", "b", "c"] })));
  const ids = () => Object.fromEntries(renderer.snapshot().children[0].children.map((c) => [c.props.testId, c.id]));
  const before = ids();
  flushSync(() => root.render(h(Items, { order: ["c", "a", "b"] })));
  const after = ids();
  assert.deepEqual(after, before);
  assert.deepEqual(renderer.snapshot().children[0].children.map((c) => c.props.testId), ["c", "a", "b"]);
  assert.equal(renderer.count("createElement"), 5);
  assert.equal(renderer.count("destroyElement"), 0);
});

test("style comparison is structural and values that serialize equally are not resent", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const App = ({ style, custom }) => h("img", { style, tint: custom });
  flushSync(() => root.render(h(App, { style: undefined, custom: undefined })));
  assert.equal(renderer.count("setStyle"), 1, "container only: an absent style is not sent");
  flushSync(() => root.render(h(App, { style: {}, custom: undefined })));
  flushSync(() => root.render(h(App, { style: { color: undefined }, custom: undefined })));
  assert.equal(renderer.count("setStyle"), 1, "absent, empty and undefined-only styles are equal");
  flushSync(() => root.render(h(App, { style: { width: 3, margin: [1, 2] }, custom: { a: [NaN] } })));
  assert.equal(renderer.count("setStyle"), 2);
  assert.equal(renderer.count("setCustomProp"), 1);
  flushSync(() => root.render(h(App, { style: { margin: [1, 2], width: 3 }, custom: { a: [NaN] } })));
  assert.equal(renderer.count("setStyle"), 2, "key order and NaN do not change equality");
  assert.equal(renderer.count("setCustomProp"), 1);
  flushSync(() => root.render(h(App, { style: { width: 3, margin: [1, 2, 3] }, custom: undefined })));
  assert.equal(renderer.count("setStyle"), 3);
  assert.deepEqual(renderer.batches.at(-1).at(-1), ["setCustomProp", renderer.snapshot().children[0].id, "tint", null]);
});

test("custom properties exclude reserved names; div and text forward only universal properties", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const noop = () => {};
  flushSync(() =>
    root.render(
      h(
        "div",
        {},
        h("div", { className: "x", key: "k", testId: "d", "aria-label": "l", tabIndex: 0, whatever: 1, onClick: noop, motion: { opacity: [0, 1] }, role: "button" }),
        h("canvas", { className: "x", whatever: { deep: [1] }, onClick: noop, fn: noop, "data-x": 1 }),
        h("text", { highlight: { query: "a" }, other: 1 }, "t"),
      ),
    ),
  );
  const [div, canvas, text] = renderer.snapshot().children[0].children;
  assert.deepEqual(Object.keys(div.props).sort(), ["aria-label", "motion", "role", "tabIndex", "testId"]);
  assert.deepEqual(div.listeners, ["click"]);
  assert.deepEqual(Object.keys(canvas.props).sort(), ["data-x", "whatever"]);
  assert.deepEqual(Object.keys(text.props), ["highlight"]);
});

test("a suspense-hidden instance keeps its layout style but is forced hidden and stays so across style updates", async () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  let suspend = false;
  const gate = { promise: null, resolve: null };
  gate.promise = new Promise((resolve) => (gate.resolve = resolve));
  const Maybe = () => {
    if (suspend) throw gate.promise;
    return null;
  };
  const App = ({ width }) =>
    h(
      React.Suspense,
      { fallback: h("div", { testId: "fallback" }) },
      h("div", { testId: "content", style: { width, hover: { color: "red" }, active: { color: "blue" } } }),
      h(Maybe),
    );
  flushSync(() => root.render(h(App, { width: 5 })));
  const content = () => renderer.snapshot().children.find((c) => c.props.testId === "content");
  assert.deepEqual(content().style, { width: 5, hover: { color: "red" }, active: { color: "blue" } });
  suspend = true;
  flushSync(() => root.render(h(App, { width: 5, tick: 1 })));
  assert.deepEqual(content().style, { width: 5, visibility: "hidden" });
  suspend = false;
  gate.resolve();
  await settle(6);
  flushSync(() => root.render(h(App, { width: 5, tick: 2 })));
  assert.deepEqual(content().style, { width: 5, hover: { color: "red" }, active: { color: "blue" } });
});

test("host refs expose the element id, focus and scroll controls; image refs expose pixel setters", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  let div = null;
  let img = null;
  flushSync(() => root.render(h("div", { ref: (n) => (div = n) }, h("img", { ref: (n) => (img = n) }))));
  assert.equal(div.id, renderer.snapshot().children[0].id);
  div.scrollIntoView({ block: "end" });
  div.focus();
  const pixels = new Uint8Array(16);
  img.setImagePixels(2, 2, pixels, "bgra");
  img.setImage(new Uint8Array([1, 2, 3]));
  assert.deepEqual(renderer.calls.map((c) => c[0]), ["scrollIntoView", "focusElement", "setImagePixels", "setImage"]);
  assert.deepEqual(renderer.calls[2].slice(2), [2, 2, pixels, "bgra"]);
  const held = div;
  flushSync(() => root.render(null));
  assert.throws(() => held.focus(), /not mounted/);
});

test("A5: identities allocated after an unmount and remount exceed every earlier identity", () => {
  const renderer = referenceRenderer();
  const first = createRoot(renderer);
  flushSync(() => first.render(h("div", {}, h("div"), h("div"))));
  const used = [...renderer.elements.keys()];
  first.unmount();
  const second = createRoot(renderer);
  flushSync(() => second.render(h("div", {}, h("div"))));
  const later = [...renderer.elements.keys()];
  assert.ok(Math.min(...later) > Math.max(...used));
});

test("a root that omits a previously supplied false focus-dim override restores the default", () => {
  const renderer = referenceRenderer();
  const first = createRoot(renderer, { keyboardFocusDim: false });
  flushSync(() => first.render(h("div")));
  assert.equal(renderer.focusDim, false);
  first.unmount();
  const second = createRoot(renderer, {});
  flushSync(() => second.render(h("div")));
  assert.equal(renderer.focusDim, true);
});

test("one renderer owns one root: a second concurrent root is rejected", () => {
  const renderer = referenceRenderer();
  const first = createRoot(renderer);
  assert.throws(() => createRoot(renderer), /one renderer owns one window/i);
  first.unmount();
  createRoot(renderer).unmount();
});

test("input dispatch flushes React work synchronously and window key defaults run without a window handler", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  const App = () => {
    const [n, setN] = React.useState(0);
    return h("div", { onClick: () => setN(n + 1), testId: "d" }, h("text", {}, `n=${n}`));
  };
  flushSync(() => root.render(h(App)));
  const id = renderer.snapshot().children[0].id;
  renderer.emit({ type: "click", id });
  assert.equal(renderer.snapshot().children[0].children[0].text, "n=1");
  renderer.emit({ type: "windowKeyDown", id: root.tree.generation, key: "tab", keystroke: 5 });
  assert.deepEqual(renderer.calls, [["focusNext"]]);
});
