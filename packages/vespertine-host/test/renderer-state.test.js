import assert from "node:assert/strict";
import { test } from "node:test";
import {
  createHandlerMap,
  createRendererState,
  pushDismissLayer,
  registerEventHandler,
  unregisterEventHandler,
  unregisterEventHandlers,
} from "../src/index.js";

function fakeRenderer(focused = null) {
  const r = {
    focused,
    log: [],
    getFocusedElement: () => r.focused,
    focusElement(id) {
      r.focused = id;
      r.log.push(["focus", id]);
    },
    focusNext: () => r.log.push(["next"]),
    focusPrevious: () => r.log.push(["prev"]),
  };
  return r;
}

const key = (over) => ({ type: "keyDown", id: 1, key: "a", keystroke: 1, ...over });

test("handler map removes an element's entry with its last handler", () => {
  const map = createHandlerMap();
  registerEventHandler(map, 1, "click", () => {});
  registerEventHandler(map, 1, "focus", () => {});
  unregisterEventHandler(map, 1, "click");
  assert.ok(map.has(1));
  unregisterEventHandler(map, 1, "focus");
  assert.ok(!map.has(1));
  registerEventHandler(map, 2, "click", () => {});
  unregisterEventHandlers(map, 2);
  assert.equal(map.size, 0);
});

test("identity allocator starts at zero and never resets across remounts", () => {
  const r = fakeRenderer();
  const state = createRendererState(r);
  assert.equal(state.ids.next(), 0);
  const a = state.attach({});
  a.detach();
  state.attach({});
  assert.equal(state.ids.next(), 1);
  assert.equal(createRendererState(r), state);
});

test("a second root is rejected until the first is detached", () => {
  const r = fakeRenderer();
  const state = createRendererState(r);
  const first = state.attach({});
  assert.throws(() => state.attach({}), /one window.*first root/s);
  assert.equal(first.detach(), true);
  assert.equal(first.detach(), false);
  const second = state.attach({});
  assert.equal(second.generation, 2);
  assert.equal(first.detach(), false, "a replaced binding does not detach the active one");
  assert.equal(state.current(), second);
});

test("generation is announced to the renderer and stale window events are rejected", () => {
  const r = fakeRenderer();
  r.setWindowGeneration = (g) => (r.generation = g);
  const state = createRendererState(r);
  const seen = [];
  const first = state.attach({ onWindowKeyDown: (e) => seen.push(["first", e.id]), onSelectionChange: () => seen.push(["sel1"]) });
  assert.equal(r.generation, 1);
  first.detach();
  state.attach({ onWindowKeyDown: (e) => seen.push(["second", e.id]), onSelectionChange: () => seen.push(["sel2"]) });
  assert.equal(r.generation, 2);
  assert.equal(state.dispatch({ type: "windowKeyDown", id: 1, key: "a", keystroke: 1 }), false);
  assert.equal(state.dispatch({ type: "windowSelectionChange", id: 1, text: "x" }), false);
  assert.deepEqual(seen, []);
  assert.equal(state.dispatch({ type: "windowKeyDown", id: 2, key: "a", keystroke: 2 }), true);
  assert.equal(state.dispatch({ type: "windowSelectionChange", id: 2, text: "x" }), true);
  assert.deepEqual(seen, [["second", 0], ["sel2"]]);
});

test("dispatch without a root, or without a handler, reports not handled", () => {
  const r = fakeRenderer();
  const state = createRendererState(r);
  assert.equal(state.dispatch({ type: "click", id: 1 }), false);
  state.attach({});
  assert.equal(state.dispatch({ type: "click", id: 1 }), false);
});

test("element events reach the handler by id and type, then the generic callback inside the scheduler", () => {
  const r = fakeRenderer();
  const state = createRendererState(r);
  const handlers = createHandlerMap();
  const order = [];
  registerEventHandler(handlers, 4, "click", (e) => order.push(["click", e.id]));
  state.attach({
    handlers,
    onEvent: (e) => order.push(["generic", e.type]),
    scheduler: (fn) => {
      order.push(["begin"]);
      fn();
      order.push(["end"]);
    },
  });
  assert.equal(state.dispatch({ type: "click", id: 4 }), true);
  assert.deepEqual(order, [["begin"], ["click", 4], ["generic", "click"], ["end"]]);
});

function keyHarness(opts = {}) {
  const r = fakeRenderer();
  const state = createRendererState(r);
  const handlers = createHandlerMap();
  const order = [];
  const reg = (id, fn) => registerEventHandler(handlers, id, "keyDown", (e) => (order.push(["el", id]), fn?.(e)));
  state.attach({
    handlers,
    onWindowKeyDown: (e, renderer) => order.push(["win", e.id, e.type, renderer === r]),
    ...opts,
  });
  return { r, state, order, reg };
}

test("elements then window share one keystroke; the window event resets id to zero", () => {
  const { r, state, order, reg } = keyHarness();
  reg(5);
  reg(2);
  state.dispatch(key({ id: 5, key: "x" }));
  state.dispatch(key({ id: 2, key: "x" }));
  state.dispatch(key({ type: "windowKeyDown", id: 1, key: "x" }));
  assert.deepEqual(order, [["el", 5], ["el", 2], ["win", 0, "keyDown", true]]);
  assert.deepEqual(r.log, []);
});

test("a prevented Tab does not move focus", () => {
  const { r, state, reg } = keyHarness();
  reg(5, (e) => e.preventDefault());
  state.dispatch(key({ id: 5, key: "tab" }));
  state.dispatch(key({ type: "windowKeyDown", id: 1, key: "tab" }));
  assert.deepEqual(r.log, []);
});

test("window handlers see the defaultPrevented flag set by an element handler", () => {
  const { state, reg } = keyHarness({ onWindowKeyDown: (e) => (seen = e.defaultPrevented) });
  let seen = false;
  reg(5, (e) => e.preventDefault());
  state.dispatch(key({ id: 5, key: "tab" }));
  state.dispatch(key({ type: "windowKeyDown", id: 1, key: "tab" }));
  assert.equal(seen, true);
});

test("stopPropagation skips later element handlers and the window handler, but defaults still run", () => {
  const { r, state, order, reg } = keyHarness();
  reg(5, (e) => e.stopPropagation());
  reg(2);
  state.dispatch(key({ id: 5, key: "tab" }));
  state.dispatch(key({ id: 2, key: "tab" }));
  state.dispatch(key({ type: "windowKeyDown", id: 1, key: "tab" }));
  assert.deepEqual(order, [["el", 5]]);
  assert.deepEqual(r.log, [["next"]]);
});

test("shift reverses Tab; shift alone does not block defaults", () => {
  const { r, state } = keyHarness();
  state.dispatch(key({ type: "windowKeyDown", id: 1, key: "tab", shift: true, keystroke: 1 }));
  state.dispatch(key({ type: "windowKeyDown", id: 1, key: "tab", keystroke: 2 }));
  assert.deepEqual(r.log, [["prev"], ["next"]]);
});

test("command-modified keys do not run defaults", () => {
  const { r, state } = keyHarness();
  for (const [i, m] of ["ctrl", "alt", "cmd"].entries()) {
    state.dispatch(key({ type: "windowKeyDown", id: 1, key: "tab", [m]: true, keystroke: i + 1 }));
  }
  assert.deepEqual(r.log, []);
});

test("tab navigation can be disabled by the root", () => {
  const { r, state } = keyHarness({ tabNavigation: false });
  state.dispatch(key({ type: "windowKeyDown", id: 1, key: "tab" }));
  assert.deepEqual(r.log, []);
});

test("key-up events never run defaults", () => {
  const { r, state } = keyHarness();
  state.dispatch({ type: "windowKeyUp", id: 1, key: "tab", keystroke: 1 });
  assert.deepEqual(r.log, []);
});

test("synthetic events do not share flags", () => {
  const { state, order, reg } = keyHarness();
  reg(5, (e) => e.stopPropagation());
  state.dispatch({ type: "keyDown", id: 5, key: "a" });
  state.dispatch({ type: "windowKeyDown", id: 1, key: "a" });
  assert.deepEqual(order, [["el", 5], ["win", 0, "keyDown", true]]);
});

test("only the latest keystroke keeps flags", () => {
  const { state, order, reg } = keyHarness();
  reg(5, (e) => e.stopPropagation());
  state.dispatch(key({ id: 5, keystroke: 1 }));
  state.dispatch(key({ id: 5, keystroke: 2 }));
  state.dispatch(key({ type: "windowKeyDown", id: 1, keystroke: 2 }));
  assert.deepEqual(order, [["el", 5], ["el", 5]]);
});

test("Escape with no open layer is delivered to none", () => {
  const { state } = keyHarness();
  assert.equal(state.dispatch({ type: "windowKeyDown", id: 1, key: "escape", keystroke: 1 }), true);
});

test("nested Escape dismissal reaches only the top layer", () => {
  const r = fakeRenderer();
  const state = createRendererState(r);
  state.attach({});
  const calls = [];
  const outer = { onEscape: () => calls.push("outer") };
  const inner = { parent: outer, onEscape: () => calls.push("inner") };
  pushDismissLayer(r, outer);
  pushDismissLayer(r, inner);
  state.dispatch({ type: "windowKeyDown", id: 1, key: "escape", keystroke: 1 });
  assert.deepEqual(calls, ["inner"]);
  state.dispatch({ type: "windowKeyDown", id: 1, key: "escape", keystroke: 2, ctrl: true });
  assert.deepEqual(calls, ["inner"]);
});

test("a prevented Escape is not delivered", () => {
  const r = fakeRenderer();
  const state = createRendererState(r);
  const handlers = createHandlerMap();
  registerEventHandler(handlers, 3, "keyDown", (e) => e.preventDefault());
  state.attach({ handlers });
  const calls = [];
  pushDismissLayer(r, { onEscape: () => calls.push("x") });
  state.dispatch({ type: "keyDown", id: 3, key: "escape", keystroke: 1 });
  state.dispatch({ type: "windowKeyDown", id: 1, key: "escape", keystroke: 1 });
  assert.deepEqual(calls, []);
});

function layerRenderer(focused) {
  const r = fakeRenderer(focused);
  createRendererState(r).attach({});
  return r;
}

test("a layer pushed on top focuses its initial target and restores the previous focus on close", () => {
  const r = layerRenderer(10);
  const close = pushDismissLayer(r, { initialFocus: () => 20, finalFocus: (prev) => prev });
  assert.equal(r.focused, 20);
  close();
  assert.equal(r.focused, 10);
});

test("a null initial target leaves focus alone", () => {
  const r = layerRenderer(10);
  pushDismissLayer(r, { initialFocus: () => null });
  assert.deepEqual(r.log, []);
});

test("closing is idempotent", () => {
  const r = layerRenderer(10);
  const close = pushDismissLayer(r, { finalFocus: (prev) => prev });
  close();
  r.focused = 99;
  r.log.length = 0;
  close();
  assert.equal(r.focused, 99);
  assert.deepEqual(r.log, []);
});

test("an explicitly supplied previous focus wins over the live focus", () => {
  const r = layerRenderer(55);
  const close = pushDismissLayer(r, { finalFocus: (prev) => prev }, { previousFocus: 10 });
  close();
  assert.equal(r.focused, 10);
});

test("child-before-parent registration: only the top layer is focused initially", () => {
  const r = layerRenderer(1);
  const parent = { initialFocus: () => 100, finalFocus: (prev) => prev };
  const child = { parent, initialFocus: () => 200, finalFocus: (prev) => prev };
  pushDismissLayer(r, child);
  assert.equal(r.focused, 200);
  r.log.length = 0;
  pushDismissLayer(r, parent);
  assert.deepEqual(r.log, [], "inserting a parent below the child moves no focus");
  assert.equal(r.focused, 200);
});

test("child-before-parent: closing the child alone returns focus inside the parent", () => {
  const r = layerRenderer(1);
  const parent = { initialFocus: () => 100, finalFocus: (prev) => prev };
  const child = { parent, initialFocus: () => 200, finalFocus: (prev) => prev };
  const closeChild = pushDismissLayer(r, child, { previousFocus: 1 });
  pushDismissLayer(r, parent, { previousFocus: 1 });
  closeChild();
  assert.equal(r.focused, 100);
});

test("child-before-parent: closing the child after its parent moves no focus", () => {
  const r = layerRenderer(1);
  const parent = { initialFocus: () => 100, finalFocus: (prev) => prev };
  const child = { parent, initialFocus: () => 200, finalFocus: (prev) => prev };
  const closeChild = pushDismissLayer(r, child, { previousFocus: 1 });
  const closeParent = pushDismissLayer(r, parent, { previousFocus: 1 });
  closeParent();
  assert.equal(r.focused, 1);
  r.log.length = 0;
  closeChild();
  assert.deepEqual(r.log, []);
  closeChild();
  assert.deepEqual(r.log, []);
});

test("parent-before-child registration orders the stack naturally", () => {
  const r = layerRenderer(1);
  const calls = [];
  const parent = { onEscape: () => calls.push("parent"), initialFocus: () => 100, finalFocus: (p) => p };
  const child = { parent, onEscape: () => calls.push("child"), initialFocus: () => 200, finalFocus: (p) => p };
  pushDismissLayer(r, parent);
  const closeChild = pushDismissLayer(r, child);
  assert.equal(r.focused, 200);
  closeChild();
  assert.equal(r.focused, 100);
});

test("escape goes to the topmost layer after child-before-parent registration", () => {
  const r = layerRenderer(1);
  const state = createRendererState(r);
  const calls = [];
  const parent = { onEscape: () => calls.push("parent") };
  const child = { parent, onEscape: () => calls.push("child") };
  pushDismissLayer(r, child);
  pushDismissLayer(r, parent);
  state.dispatch({ type: "windowKeyDown", id: 1, key: "escape", keystroke: 1 });
  assert.deepEqual(calls, ["child"]);
});
