// The headless components for Solid: Button and child composition (CMP-001), floating surfaces
// and dismissal layers (CMP-002), the compound tooltip (CMP-004) and the compound select (CMP-005).
// Each test notes where Solid differs from React (see the file headers of src/components).

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  Button,
  DismissableLayer,
  FloatingLayer,
  Select,
  Show,
  Tooltip,
  buttonProps,
  createButtonLatch,
  createComponent,
  createControllableState,
  createPresence,
  createSignal,
  createTextNode,
  mergeProps,
  mergeStyles,
  renderSlot,
  resolveStyle,
  composeHandlers,
} from "../src/index.js";
import { c, h, installFakeTime, microtasks, mount, settle, textOf } from "./components-helpers.js";

const click = (ui, testId) => ui.emit(testId, { type: "click", button: 0 });

test("the public API is exported", () => {
  for (const part of [Button, DismissableLayer, FloatingLayer, buttonProps, createButtonLatch, createControllableState, createPresence, renderSlot, mergeStyles, resolveStyle, composeHandlers]) {
    assert.equal(typeof part, "function");
  }
  assert.deepEqual(Object.keys(Tooltip).sort(), ["Content", "Provider", "Root", "Trigger"]);
  assert.deepEqual(
    Object.keys(Select).sort(),
    ["Content", "Group", "Item", "Label", "Root", "ScrollDownButton", "ScrollUpButton", "Separator", "Trigger", "Value", "textOf"].sort(),
  );
});

// ---- Button and child composition (CMP-001) -------------------------------------------------

test("CMP-001 R1: a button is a focusable generic container with role button", () => {
  const ui = mount(() => c(Button, { testId: "b" }, "Go"));
  const node = ui.node("b");
  assert.equal(node.type, "div");
  assert.equal(node.props.role, "button");
  assert.equal(node.props.tabIndex, 0);
  assert.equal(textOf(node), "Go");
  assert.deepEqual(ui.errors, []);
});

test("CMP-001 A3: a disabled button has tab index -1, or 0 with focusableWhenDisabled, and never activates", () => {
  const log = [];
  const ui = mount(() =>
    h(
      "div",
      {},
      c(Button, { testId: "a", disabled: true, onClick: () => log.push("a") }),
      c(Button, { testId: "b", disabled: true, focusableWhenDisabled: true, onClick: () => log.push("b") }),
    ),
  );
  assert.equal(ui.node("a").props.tabIndex, -1);
  assert.equal(ui.node("b").props.tabIndex, 0);
  for (const id of ["a", "b"]) {
    ui.emit(id, { type: "click", button: 0 });
    ui.key(id, "enter");
    ui.key(id, "space");
  }
  assert.deepEqual(log, []);
});

test("CMP-001 R2: click, one Enter activation per press, Space on key-up", () => {
  const log = [];
  const ui = mount(() => c(Button, { testId: "b", onClick: () => log.push("activate") }, "Go"));
  ui.emit("b", { type: "click", button: 0 });
  assert.deepEqual(log, ["activate"]);
  ui.keyDown("b", "enter");
  ui.keyDown("b", "enter", { held: true });
  ui.keyDown("b", "enter"); // a repeat the platform did not flag as held
  assert.equal(log.length, 2, "a single Enter activation for the physical press");
  ui.emit("b", { type: "keyUp", key: "enter", keystroke: 5000 });
  ui.keyDown("b", "enter");
  assert.equal(log.length, 3, "the key-up released the latch");
  ui.keyDown("b", "space");
  assert.equal(log.length, 3, "space does not activate on key-down");
  ui.emit("b", { type: "keyUp", key: "space", keystroke: 5001 });
  assert.equal(log.length, 4);
});

test("CMP-001 R3: style is an object or a function of the disabled state", () => {
  const style = ({ disabled }) => ({ opacity: disabled ? 0.4 : 1 });
  const ui = mount(() =>
    h("div", {}, c(Button, { testId: "a", style }), c(Button, { testId: "b", style, disabled: true }), c(Button, { testId: "c", style: { width: 5 } })),
  );
  assert.equal(ui.node("a").style.opacity, 1);
  assert.equal(ui.node("b").style.opacity, 0.4);
  assert.equal(ui.node("c").style.width, 5);
});

test("CMP-001 R1, R3 (Solid): disabled, focusableWhenDisabled and tabIndex stay reactive", () => {
  const [disabled, setDisabled] = createSignal(false);
  const [focusable, setFocusable] = createSignal(false);
  const log = [];
  const ui = mount(() =>
    c(Button, {
      testId: "b",
      get disabled() {
        return disabled();
      },
      get focusableWhenDisabled() {
        return focusable();
      },
      style: ({ disabled }) => ({ opacity: disabled ? 0.4 : 1 }),
      onClick: () => log.push("activate"),
    }),
  );
  assert.deepEqual([ui.node("b").props.tabIndex, ui.node("b").style.opacity], [0, 1]);
  ui.set(() => setDisabled(true));
  assert.deepEqual([ui.node("b").props.tabIndex, ui.node("b").style.opacity], [-1, 0.4]);
  ui.emit("b", { type: "click", button: 0 });
  assert.deepEqual(log, []);
  ui.set(() => setFocusable(true));
  assert.equal(ui.node("b").props.tabIndex, 0);
  ui.set(() => setDisabled(false));
  ui.emit("b", { type: "click", button: 0 });
  assert.deepEqual(log, ["activate"]);
});

test("CMP-001 R4: buttonProps supplies the same behavior to any control", () => {
  const log = [];
  const ui = mount(() => {
    const props = buttonProps({ latch: createButtonLatch(), onClick: () => log.push("activate") });
    return h("div", { testId: "c", ...props });
  });
  assert.deepEqual([ui.node("c").props.role, ui.node("c").props.tabIndex], ["button", 0]);
  ui.keyDown("c", "enter");
  assert.deepEqual(log, ["activate"]);
  ui.keyDown("c", "enter", { held: true });
  assert.deepEqual(log, ["activate"], "the latch belongs to the control");
});

test("CMP-001 R5, R6, A2: asChild merges onto one host element child (handler order, tab index, replaced style, ref, children)", () => {
  const log = [];
  let childRef = null;
  let partRef = null;
  const ui = mount(() =>
    c(
      Button,
      {
        asChild: true,
        ref: (node) => (partRef = node),
        tabIndex: -1,
        style: { width: 2 },
        "aria-label": "part label",
        onClick: () => log.push("part"),
        onKeyDown: () => log.push("part keydown"),
      },
      () =>
        h(
          "div",
          {
            testId: "child",
            "aria-label": "child label",
            ref: (node) => (childRef = node),
            tabIndex: 7,
            style: { color: "red", width: 1 },
            onClick: () => log.push("child"),
            onKeyDown: (event) => {
              log.push("child keydown");
              event.preventDefault();
            },
            onPointerDown: () => log.push("child pointerDown"),
          },
          h("text", {}, "kept"),
        ),
    ),
  );
  const node = ui.node("child");
  assert.equal(node.props.role, "button");
  assert.equal(node.props["aria-label"], "part label", "a part property replaces the child's");
  assert.equal(node.props.tabIndex, -1, "an explicit part tab index beats the child's");
  assert.deepEqual(node.style, { width: 2 }, "the part's style replaces the child's: no shallow merge in Solid");
  assert.equal(textOf(node), "kept", "the child's own children are preserved");
  assert.ok(childRef && childRef === partRef, "the child's ref ran when it was created and the part's ref receives the same element");
  assert.equal(childRef.id, node.id);
  ui.emit("child", { type: "click", button: 0 });
  assert.deepEqual(log, ["child", "part"]);
  log.length = 0;
  ui.keyDown("child", "enter");
  assert.deepEqual(log.slice(0, 2), ["child keydown", "part keydown"], "the part handler runs although the child prevented the default");
  assert.equal(log.length, 2, "the prevented default stops the activation");
  log.length = 0;
  ui.emit("child", { type: "pointerDown", button: 0 });
  assert.deepEqual(log, ["child pointerDown"], "a child handler the part does not supply is kept");

  const defaults = mount(() => c(Button, { asChild: true }, () => h("div", { testId: "d", tabIndex: 7 })));
  assert.equal(defaults.node("d").props.tabIndex, 7, "the child's tab index beats the part default");
  const plain = mount(() => c(Button, { asChild: true }, () => h("div", { testId: "p" })));
  assert.equal(plain.node("p").props.tabIndex, 0);
  assert.equal(plain.node("p").props.role, "button");
});

test("CMP-001 R5 (Solid): the part's property getters stay reactive on the child element", () => {
  const [disabled, setDisabled] = createSignal(false);
  const log = [];
  const ui = mount(() =>
    c(
      Button,
      {
        asChild: true,
        get disabled() {
          return disabled();
        },
        style: ({ disabled }) => ({ opacity: disabled ? 0.4 : 1 }),
        onClick: () => log.push("part"),
      },
      () => h("div", { testId: "child", onClick: () => log.push("child") }),
    ),
  );
  assert.deepEqual([ui.node("child").props.tabIndex, ui.node("child").style.opacity], [0, 1]);
  ui.set(() => setDisabled(true));
  assert.deepEqual([ui.node("child").props.tabIndex, ui.node("child").style.opacity], [-1, 0.4]);
  ui.emit("child", { type: "click", button: 0 });
  assert.deepEqual(log, ["child"], "the child's own handler still runs; the disabled part does not activate");
});

test("CMP-001 R5, A2 (Solid): asChild needs exactly one intrinsic host element child", () => {
  const message = /asChild requires exactly one intrinsic host element child/;
  assert.throws(() => mount(() => c(Button, { asChild: true })), message, "zero children");
  assert.throws(() => mount(() => c(Button, { asChild: true }, () => h("div"), () => h("div"))), message, "two children");
  assert.throws(() => mount(() => c(Button, { asChild: true }, "text")), message, "text");
  assert.throws(() => mount(() => c(Button, { asChild: true }, () => createTextNode("raw"))), message, "a raw text node");
  assert.throws(() => mount(() => c(Button, { asChild: true }, () => ({ not: "a host node" }))), message, "a non-host value");
  assert.throws(() => mount(() => c(Button, { asChild: true }, () => null)), message, "a null child");
});

test("CMP-001 R5 (Solid): renderSlot renders a plain part and returns the child of an asChild part", () => {
  const ui = mount(() =>
    h(
      "div",
      {},
      renderSlot({ children: () => "plain", type: "text", props: { testId: "text" } }),
      renderSlot({ children: () => "box", props: { testId: "box", tabIndex: 3 }, defaults: { tabIndex: 9 } }),
      renderSlot({ children: () => "defaulted", props: { testId: "defaulted" }, defaults: { tabIndex: 9 } }),
    ),
  );
  assert.equal(ui.node("text").type, "text");
  assert.equal(textOf(ui.node("text")), "plain");
  assert.equal(ui.node("box").props.tabIndex, 3);
  assert.equal(ui.node("defaulted").props.tabIndex, 9);
  const child = h("div", { testId: "child" });
  assert.equal(renderSlot({ asChild: true, children: () => child, props: {} }), child);
});

// ---- Controlled state and helpers (CMP-002 R12) ---------------------------------------------

test("CMP-002 A12 (Solid): the controlled-state helper returns an accessor", () => {
  const changes = [];
  const [controlledValue, setControlledValue] = createSignal(undefined);
  let shown;
  let request;
  mount(() => {
    [shown, request] = createControllableState({ value: controlledValue, defaultValue: 1, onChange: (next) => changes.push(next) });
    return h("div");
  });
  assert.equal(shown(), 1);
  assert.equal(request(1), false, "an identical request does nothing");
  assert.equal(request(2), true);
  assert.equal(shown(), 2, "an uncontrolled request updates the internal state");
  assert.equal(request(3) && request(3), false, "later requests see the value the earlier one set");
  setControlledValue(5);
  assert.equal(shown(), 5, "a defined value makes the state controlled");
  assert.equal(request(6), true);
  assert.equal(shown(), 5, "a controlled request leaves the supplied value");
  assert.equal(request(5), false);
  assert.deepEqual(changes, [2, 3, 6]);
  setControlledValue(undefined);
  assert.equal(shown(), 3, "undefined is uncontrolled again");
});

test("CMP-002 A12 (Solid): a function value is stored as a value, not as an updater", () => {
  const fn = () => "value";
  let shown;
  let request;
  mount(() => {
    [shown, request] = createControllableState({ defaultValue: undefined });
    return h("div");
  });
  assert.equal(request(fn), true);
  assert.equal(shown(), fn);
});

test("CMP-001 R3, CMP-002 style helpers: mergeStyles is shallow with the later style winning, resolveStyle calls functions", () => {
  assert.deepEqual(mergeStyles({ a: 1, b: 1 }, undefined, { b: 2 }), { a: 1, b: 2 });
  assert.deepEqual(resolveStyle(({ on }) => ({ on }), { on: true }), { on: true });
  assert.equal(resolveStyle(undefined, {}), undefined);
  const log = [];
  composeHandlers(
    (event) => {
      log.push("first");
      event.preventDefault();
    },
    () => log.push("second"),
  )({ preventDefault() {} });
  assert.deepEqual(log, ["first", "second"]);
});

// ---- Floating surfaces (CMP-002) ------------------------------------------------------------

test("CMP-002 A1 (Solid): the caller style applies to the inner container only; the outer surface gets none", () => {
  const style = {
    opacity: 0.5,
    backgroundColor: "#fff",
    borderTopLeftRadius: 4,
    borderBottomRightRadius: 12,
    hover: { opacity: 0.8, borderTopRightRadius: 2, color: "red" },
    active: { opacity: 0.9 },
  };
  const ui = mount(() => c(FloatingLayer, { testId: "content", style }));
  const outer = ui.snapshot().children[0];
  assert.equal(outer.type, "anchored");
  assert.ok(!outer.style || Object.keys(outer.style).length === 0, "no opacity or radii are routed to the outer anchored surface");
  const inner = ui.node("content");
  assert.equal(inner.style.opacity, 0.5, "opacity stays on the inner container, unlike React");
  assert.equal(inner.style.hover.opacity, 0.8);
  assert.equal(inner.style.active.opacity, 0.9);
  assert.equal(inner.style.borderTopLeftRadius, 4);
  assert.equal(inner.style.borderBottomRightRadius, 12);
  assert.equal(inner.style.hover.borderTopRightRadius, 2);
  assert.equal(inner.style.hover.color, "red");
  assert.equal(inner.style.backgroundColor, "#fff", "the caller's fill replaces the default");
  assert.equal(inner.style.display, "flex");
  assert.equal(inner.style.position, "relative");
  assert.equal(inner.style.alignItems, "flex-start");
});

test("CMP-002 R1, R2: floating options map to the anchored host; the default fill is #1A1A1A", () => {
  const ui = mount(() => c(FloatingLayer, { testId: "c", side: "right", align: "end", sideOffset: 6, alignOffset: 3, collisionPadding: 12 }));
  const outer = ui.snapshot().children[0];
  assert.deepEqual(outer.props, { side: "right", align: "end", gap: 6, offset: { x: 0, y: 3 }, fit: "snap", snapMargin: 12, deferred: true, priority: 1, occlude: true });
  assert.equal(ui.node("c").style.backgroundColor, "#1A1A1A");
  const defaults = mount(() => c(FloatingLayer, { testId: "c" }));
  assert.deepEqual(defaults.snapshot().children[0].props, { side: "bottom", align: "start", gap: 0, offset: { x: 0, y: 0 }, fit: "snap", snapMargin: 8, deferred: true, priority: 1, occlude: true });
  const horizontal = mount(() => c(FloatingLayer, { testId: "c", side: "top", alignOffset: 5 }));
  assert.deepEqual(horizontal.snapshot().children[0].props.offset, { x: 5, y: 0 });
  const passive = mount(() => c(FloatingLayer, { testId: "c", style: { pointerEvents: "none" } }));
  assert.equal(passive.snapshot().children[0].props.occlude, false);
});

test("CMP-002 R1 (Solid): floating options stay reactive", () => {
  const [side, setSide] = createSignal("bottom");
  const ui = mount(() => c(FloatingLayer, { testId: "c", get side() { return side(); } }));
  assert.equal(ui.snapshot().children[0].props.side, "bottom");
  ui.set(() => setSide("left"));
  assert.equal(ui.snapshot().children[0].props.side, "left");
  assert.deepEqual(ui.snapshot().children[0].props.offset, { x: 0, y: 0 });
});

test("CMP-002 R4, R7: floating content nests in the root and the layer joins the stack only while open", () => {
  const log = [];
  const [open, setOpen] = createSignal(true);
  const ui = mount(() =>
    c(
      DismissableLayer,
      {
        testId: "layer",
        get open() {
          return open();
        },
        onDismiss: (reason) => log.push(reason),
        onEscapeKeyDown: () => log.push("handler"),
      },
      "body",
    ),
  );
  ui.windowKey("escape");
  assert.deepEqual(log, ["handler", "escape"]);
  ui.set(() => setOpen(false));
  log.length = 0;
  ui.windowKey("escape");
  assert.deepEqual(log, [], "a layer that is not open is not on the stack");
  ui.set(() => setOpen(true));
  ui.windowKey("escape");
  assert.deepEqual(log, ["handler", "escape"]);
});

test("CMP-002 R7, R14 (Solid): an outside press runs the caller handler, then dismisses, with or without a press flag", async () => {
  const log = [];
  const ui = mount(() => c(DismissableLayer, { testId: "layer", onOutsideDown: () => log.push("caller"), onDismiss: (reason) => log.push(reason) }, "body"));
  ui.emit("layer", { type: "outsideDown", x: 1, y: 1 });
  assert.deepEqual(log, ["caller", "outside"], "without a press flag the dismissal is immediate");
  const flag = { defer: (fn) => log.push("deferred") && fn() };
  log.length = 0;
  const flagged = mount(() => c(DismissableLayer, { testId: "layer", pressFlag: flag, onDismiss: (reason) => log.push(reason) }, "body"));
  flagged.emit("layer", { type: "outsideDown", x: 1, y: 1 });
  assert.deepEqual(log, ["deferred", "outside"]);
  const closed = mount(() => c(DismissableLayer, { testId: "layer", open: false, onDismiss: (reason) => log.push(reason) }, "body"));
  log.length = 0;
  closed.emit("layer", { type: "outsideDown", x: 1, y: 1 });
  assert.deepEqual(log, [], "a layer that is not open ignores outside presses");
});

test("CMP-002 R9, R10, R11 (Solid): initial and final focus targets, and the default restoration of the captured focus", () => {
  let target = null;
  const inside = mount(() =>
    h(
      "div",
      {},
      c(DismissableLayer, { testId: "layer", initialFocus: () => target }, () => h("div", { testId: "inner", tabIndex: 0, ref: (node) => (target = node) })),
    ),
  );
  assert.equal(inside.focused(), inside.node("inner").id, "the initial focus target is a function returning a node");
  const suppressed = mount(() => c(DismissableLayer, { testId: "layer", initialFocus: false, defaultFocus: (node) => node.id }, "body"));
  assert.equal(suppressed.focused(), null, "false leaves focus where it is");
  const defaulted = mount(() => c(DismissableLayer, { testId: "layer", defaultFocus: (node) => node.id, tabIndex: -1 }, "body"));
  assert.equal(defaulted.focused(), defaulted.node("layer").id, "unset selects the component default");

  const [open, setOpen] = createSignal(false);
  const ui = mount(() =>
    h(
      "div",
      {},
      h("div", { testId: "other", tabIndex: 0 }),
      h("div", { testId: "elsewhere", tabIndex: 0 }),
      c(
        DismissableLayer,
        {
          testId: "layer",
          tabIndex: -1,
          get open() {
            return open();
          },
          defaultFocus: (node) => node.id,
        },
        "body",
      ),
    ),
  );
  ui.renderer.focusElement(ui.node("other").id);
  ui.set(() => setOpen(true));
  assert.equal(ui.focused(), ui.node("layer").id);
  ui.set(() => setOpen(false));
  assert.equal(ui.focused(), ui.node("other").id, "the captured prior focus returns while focus is still inside the layer");
  ui.set(() => setOpen(true));
  ui.renderer.focusElement(ui.node("elsewhere").id);
  ui.set(() => setOpen(false));
  assert.equal(ui.focused(), ui.node("elsewhere").id, "focus that moved elsewhere is kept");

  const [openFinal, setOpenFinal] = createSignal(true);
  const custom = mount(() =>
    h(
      "div",
      {},
      h("div", { testId: "target", tabIndex: 0, ref: (node) => (target = node) }),
      c(
        DismissableLayer,
        {
          testId: "layer",
          initialFocus: false,
          get open() {
            return openFinal();
          },
          finalFocus: (previous) => (previous === null ? target : false),
        },
        "body",
      ),
    ),
  );
  custom.set(() => setOpenFinal(false));
  assert.equal(custom.focused(), custom.node("target").id, "finalFocus receives the captured owner and returns a node");
});

// ---- Tooltip (CMP-004) ----------------------------------------------------------------------

function tooltipApp({ provider = {}, root = {}, content = {}, trigger = {} } = {}) {
  return c(
    Tooltip.Provider,
    provider,
    () =>
      c(
        Tooltip.Root,
        mergeProps({ testId: "root" }, root),
        () => c(Tooltip.Trigger, mergeProps({ testId: "trigger" }, trigger), () => h("text", {}, "?")),
        () => c(Tooltip.Content, mergeProps({ testId: "content" }, content), () => h("text", {}, "tip")),
      ),
  );
}

async function withTime(body) {
  const time = installFakeTime();
  try {
    await body(time);
  } finally {
    time.restore();
  }
}

/** Advances virtual time and lets the microtask flush of the state the timers requested run. */
async function advance(time, ms) {
  time.advance(ms);
  await settle();
}

const anchoredOf = (ui) => ui.snapshot().children[0].children.find((child) => child.type === "anchored");

test("CMP-004 R5, R10: the content mounts only while open with top, center and no offset by default", async () => {
  await withTime(async () => {
    const ui = mount(() => tooltipApp());
    assert.equal(ui.has("content"), false, "closed by default");
    assert.equal(ui.node("trigger").props.tabIndex, 0, "the trigger defaults to tab index 0");
    assert.equal(ui.node("root").style.display, "flex");
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.ok(ui.has("content"), "a zero delay opens immediately");
    const anchored = anchoredOf(ui);
    assert.equal(anchored.props.side, "top");
    assert.equal(anchored.props.align, "center");
    assert.equal(anchored.props.gap, 0);
  });
});

test("CMP-004 A2(a): delay 700 and skip 300 through real hover events", async () => {
  await withTime(async (time) => {
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 700, skipDelayDuration: 300, disableHoverableContent: true } }));
    ui.emit("trigger", { type: "enter" });
    await advance(time, 699);
    assert.equal(ui.has("content"), false);
    await advance(time, 1);
    assert.equal(ui.has("content"), true, "open at 700 ms");
    ui.emit("trigger", { type: "leave" });
    await settle();
    assert.equal(ui.has("content"), false, "a non-hoverable tooltip closes at once");
    await advance(time, 250);
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.equal(ui.has("content"), true, "re-entering 250 ms after the close opens immediately");
    ui.emit("trigger", { type: "leave" });
    await advance(time, 400);
    ui.emit("trigger", { type: "enter" });
    await advance(time, 699);
    assert.equal(ui.has("content"), false, "re-entering 400 ms after the close waits 700 ms");
    await advance(time, 1);
    assert.equal(ui.has("content"), true);
  });
});

test("CMP-004 A2(b): hoverable content keeps the tooltip open for 80 ms", async () => {
  await withTime(async (time) => {
    const ui = mount(() => tooltipApp());
    ui.emit("trigger", { type: "enter" });
    await settle();
    ui.emit("trigger", { type: "leave" });
    await advance(time, 79);
    assert.ok(ui.has("content"), "still open inside the grace");
    ui.emit("content", { type: "enter" });
    await advance(time, 500);
    assert.ok(ui.has("content"), "entering the content within the grace keeps it open");
    ui.emit("content", { type: "leave" });
    await advance(time, 80);
    assert.equal(ui.has("content"), false, "leaving the content closes after the same grace");
  });
  await withTime(async () => {
    const ui = mount(() => tooltipApp({ root: { disableHoverableContent: true } }));
    ui.emit("trigger", { type: "enter" });
    await settle();
    ui.emit("trigger", { type: "leave" });
    await settle();
    assert.equal(ui.has("content"), false, "disableHoverableContent closes at once");
  });
});

test("CMP-004 R8, A2(c): focus opens, blur, press and click close and update the last close time; caller handlers run first", async () => {
  await withTime(async (time) => {
    const log = [];
    const ui = mount(() =>
      c(
        Tooltip.Provider,
        { delayDuration: 700 },
        () =>
          c(
            Tooltip.Root,
            {},
            () => c(Tooltip.Trigger, { testId: "trigger", onFocus: () => log.push("caller focus"), onClick: () => log.push("caller click") }),
            () => c(Tooltip.Content, { testId: "content" }),
          ),
      ),
    );
    ui.emit("trigger", { type: "focus" });
    await settle();
    assert.ok(ui.has("content"), "focus opens immediately");
    assert.deepEqual(log, ["caller focus"]);
    ui.emit("trigger", { type: "blur" });
    await settle();
    assert.equal(ui.has("content"), false);
    await advance(time, 1000);
    ui.emit("trigger", { type: "pointerDown", button: 0 });
    await advance(time, 100);
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.ok(ui.has("content"), "a press while closed still counts for the skip window");
    ui.emit("trigger", { type: "click", button: 0 });
    await settle();
    assert.equal(ui.has("content"), false, "click closes");
    assert.deepEqual(log, ["caller focus", "caller click"]);
  });
});

test("CMP-004 R8 (Solid): the trigger merges onto a host element child with asChild", async () => {
  await withTime(async () => {
    const log = [];
    const ui = mount(() =>
      c(
        Tooltip.Root,
        {},
        () => c(Tooltip.Trigger, { asChild: true }, () => h("div", { testId: "trigger", onEnter: () => log.push("child enter") })),
        () => c(Tooltip.Content, { testId: "content" }),
      ),
    );
    assert.equal(ui.node("trigger").props.tabIndex, 0);
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.deepEqual(log, ["child enter"]);
    assert.ok(ui.has("content"));
  });
});

test("CMP-004 A2(d), R9: unmount cancels the tracked timers", async () => {
  await withTime(async (time) => {
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 700 } }));
    const before = time.pendingTimers;
    ui.emit("trigger", { type: "enter" });
    assert.equal(time.pendingTimers - before, 1);
    ui.root.unmount();
    await settle();
    assert.equal(time.pendingTimers - before, 0, "the tracked open timer was cancelled");
    time.advance(1000);
    assert.equal(ui.renderer.elements.size, 0);
    assert.deepEqual(ui.errors, []);
  });
});

test("CMP-004 A2(c): a pointer-down on an open tooltip closes it and updates the last close time", async () => {
  await withTime(async (time) => {
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 700, skipDelayDuration: 300 }, root: { defaultOpen: true } }));
    assert.ok(ui.has("content"));
    ui.emit("trigger", { type: "pointerDown", button: 0 });
    await settle();
    assert.equal(ui.has("content"), false, "the press closes it");
    await advance(time, 300);
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.ok(ui.has("content"), "re-entering exactly 300 ms after the pointer-down close opens immediately (the skip window is inclusive)");
    ui.emit("trigger", { type: "pointerDown", button: 0 });
    await advance(time, 301);
    ui.emit("trigger", { type: "enter" });
    await advance(time, 699);
    assert.equal(ui.has("content"), false, "301 ms after the close the full delay applies");
    await advance(time, 1);
    assert.ok(ui.has("content"));
  });
});

test("CMP-004 A2(c): a click on an open tooltip closes it and updates the last close time", async () => {
  await withTime(async (time) => {
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 700, skipDelayDuration: 300 }, root: { defaultOpen: true } }));
    click(ui, "trigger");
    await settle();
    assert.equal(ui.has("content"), false, "the click closes it");
    await advance(time, 250);
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.ok(ui.has("content"), "re-entering 250 ms after the click close opens immediately");
  });
});

test("CMP-004 A2(a): roots without a provider share the default provider's last close time", async () => {
  await withTime(async (time) => {
    const bare = (root) =>
      c(
        Tooltip.Root,
        mergeProps({ testId: "root", delayDuration: 700 }, root),
        () => c(Tooltip.Trigger, { testId: "trigger" }, () => h("text", {}, "?")),
        () => c(Tooltip.Content, { testId: "content" }, () => h("text", {}, "tip")),
      );
    const first = mount(() => bare({ defaultOpen: true }));
    const second = mount(() => bare({}));
    first.emit("trigger", { type: "blur" });
    await settle();
    assert.equal(first.has("content"), false);
    await advance(time, 200);
    second.emit("trigger", { type: "enter" });
    await settle();
    assert.ok(second.has("content"), "another root without a provider opens at once inside the shared skip window");
    second.emit("trigger", { type: "blur" });
    await advance(time, 400);
    second.emit("trigger", { type: "enter" });
    await advance(time, 699);
    assert.equal(second.has("content"), false, "outside the skip window the root's own 700 ms delay applies");
    await advance(time, 1);
    assert.ok(second.has("content"));
  });
});

test("CMP-004 A2(b): a hoverable tooltip closes 80 ms after leaving the trigger, and a re-entry of the trigger cancels the close", async () => {
  await withTime(async (time) => {
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 0 } }));
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.ok(ui.has("content"));
    ui.emit("trigger", { type: "leave" });
    await advance(time, 79);
    assert.ok(ui.has("content"));
    ui.emit("trigger", { type: "enter" });
    await advance(time, 200);
    assert.ok(ui.has("content"), "re-entering the trigger within the grace cancels the pending close");
    ui.emit("trigger", { type: "leave" });
    await advance(time, 80);
    assert.equal(ui.has("content"), false, "the close fires exactly at 80 ms");
  });
});

test("CMP-004 A2(d): unmounting cancels a pending close as well as a pending open", async () => {
  await withTime(async (time) => {
    const requests = [];
    const ui = mount(() => tooltipApp({ root: { defaultOpen: true, onOpenChange: (open) => requests.push(open) } }));
    const before = time.pendingTimers;
    ui.emit("trigger", { type: "leave" });
    assert.equal(time.pendingTimers - before, 1, "the close grace is pending");
    ui.root.unmount();
    await settle();
    assert.equal(time.pendingTimers - before, 0, "unmount cancelled the tracked close timer");
    time.advance(1000);
    assert.deepEqual(requests, [], "no close request fired after the unmount");
    assert.deepEqual(ui.errors, []);
  });
});

test("CMP-004 A3 (Solid): the overwrite is observable: repeated hover scheduling leaves one timer per enter, not one", async () => {
  await withTime(async (time) => {
    const requests = [];
    // A controlled root reports every request, so each timer that fires shows as a request.
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 700 }, root: { open: false, onOpenChange: (open) => requests.push(open) } }));
    const before = time.pendingTimers;
    ui.emit("trigger", { type: "enter" });
    ui.emit("trigger", { type: "enter" });
    assert.equal(time.pendingTimers - before, 2, "the earlier open timer was not cancelled when its handle was overwritten");
    await advance(time, 700);
    assert.deepEqual(requests, [true, true], "both timers fire");
    assert.equal(time.pendingTimers - before, 0);
  });
});

test("CMP-004 A3, R9 (Solid): a leave or an unmount cancels only the tracked (latest) timer", async () => {
  await withTime(async (time) => {
    const requests = [];
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 700 }, root: { open: false, onOpenChange: (open) => requests.push(open) } }));
    const before = time.pendingTimers;
    ui.emit("trigger", { type: "enter" });
    ui.emit("trigger", { type: "enter" });
    ui.emit("trigger", { type: "leave" });
    assert.equal(time.pendingTimers - before, 2, "the leave cancelled the tracked open timer and scheduled the close grace; the first open timer is still pending");
    await advance(time, 700);
    assert.ok(requests.includes(true), "the untracked earlier open timer still fired");

    const second = mount(() => tooltipApp({ provider: { delayDuration: 700 } }));
    const base = time.pendingTimers;
    second.emit("trigger", { type: "enter" });
    second.emit("trigger", { type: "enter" });
    second.root.unmount();
    assert.equal(time.pendingTimers - base, 1, "unmount cancelled the tracked timer only");
    time.advance(1000);
    await settle();
    assert.deepEqual(second.errors, []);
  });
});

test("CMP-004 R5: a controlled root reports requests and does not overwrite the value; a root override beats the provider", async () => {
  await withTime(async (time) => {
    const requests = [];
    const ui = mount(() => tooltipApp({ provider: { delayDuration: 700 }, root: { open: false, delayDuration: 100, onOpenChange: (open) => requests.push(open) } }));
    ui.emit("trigger", { type: "enter" });
    await advance(time, 100);
    assert.deepEqual(requests, [true]);
    assert.equal(ui.has("content"), false, "the owner decides");
  });
});

test("CMP-004 R5 (Solid): a controlled open state follows the owner", async () => {
  await withTime(async () => {
    const [open, setOpen] = createSignal(false);
    const ui = mount(() =>
      tooltipApp({
        root: {
          get open() {
            return open();
          },
        },
      }),
    );
    assert.equal(ui.has("content"), false);
    ui.set(() => setOpen(true));
    assert.ok(ui.has("content"));
    ui.set(() => setOpen(false));
    assert.equal(ui.has("content"), false);
  });
});

test("CMP-004 R5 (Solid): provider settings and the root's hoverability are captured at construction", async () => {
  await withTime(async (time) => {
    const [delay, setDelay] = createSignal(700);
    const ui = mount(() =>
      tooltipApp({
        provider: {
          get delayDuration() {
            return delay();
          },
        },
      }),
    );
    ui.set(() => setDelay(50));
    ui.emit("trigger", { type: "enter" });
    await advance(time, 50);
    assert.equal(ui.has("content"), false, "the changed provider setting did not reach the existing root (React propagates it)");
    await advance(time, 650);
    assert.ok(ui.has("content"), "the captured 700 ms delay applies");
  });
  await withTime(async (time) => {
    const [off, setOff] = createSignal(false);
    const ui = mount(() =>
      tooltipApp({
        root: {
          defaultOpen: true,
          get disableHoverableContent() {
            return off();
          },
        },
      }),
    );
    ui.set(() => setOff(true));
    ui.emit("trigger", { type: "leave" });
    await advance(time, 79);
    assert.ok(ui.has("content"), "the captured hoverability still grants the grace");
    assert.equal(ui.node("content").style.pointerEvents, undefined);
    await advance(time, 1);
    assert.equal(ui.has("content"), false);
  });
});

test("CMP-004 R10, CMP-002 R6, R7: Escape closes the tooltip first and goes on to the layer below it", async () => {
  await withTime(async () => {
    const [gone, setGone] = createSignal(false);
    const ui = mount(() =>
      c(Show, {
        get when() {
          return !gone();
        },
        get fallback() {
          return h("div", { testId: "gone" });
        },
        get children() {
          return c(DismissableLayer, { testId: "outer", onDismiss: () => setGone(true) }, () => tooltipApp({ root: { defaultOpen: true } }));
        },
      }),
    );
    assert.ok(ui.has("content"));
    ui.windowKey("escape");
    await settle();
    assert.equal(ui.has("content"), false, "the tooltip, the top layer, takes the first Escape");
    assert.ok(ui.has("outer"));
    ui.windowKey("escape");
    await settle();
    assert.ok(ui.has("gone"), "the second Escape reaches the layer below");
  });
});

test("CMP-004 R10: a non-hoverable tooltip is pointer-transparent and never takes focus", async () => {
  await withTime(async () => {
    const ui = mount(() => tooltipApp({ root: { defaultOpen: true, disableHoverableContent: true } }));
    assert.equal(anchoredOf(ui).props.occlude, false);
    assert.equal(ui.node("content").style.pointerEvents, "none");
    assert.equal(ui.focused(), null, "opening never moves focus");
    const hoverable = mount(() => tooltipApp({ root: { defaultOpen: true } }));
    assert.equal(hoverable.focused(), null, "a hoverable tooltip never takes focus either");
    assert.equal(anchoredOf(hoverable).props.occlude, true);
  });
});

test("CMP-004 R10 (Solid): a part outside its root throws", () => {
  assert.throws(() => mount(() => c(Tooltip.Trigger, {})), /Tooltip\.Trigger must be used inside Tooltip\.Root/);
  assert.throws(() => mount(() => c(Tooltip.Content, {})), /Tooltip\.Content must be used inside Tooltip\.Root/);
});

// ---- Select (CMP-005) -----------------------------------------------------------------------

const itemStyle = ({ selected, highlighted, disabled }) => ({ color: `${highlighted ? "H" : "-"}${selected ? "S" : "-"}${disabled ? "D" : "-"}` });

const item = (id, value, label, over = {}) => () => c(Select.Item, mergeProps({ testId: `item-${id}`, value, style: itemStyle }, over), label);

const defaultItems = () => [
  item("a", "a", "Apple"),
  item("b", "b", "Banana", { disabled: true }),
  item("c", "c", "Cherry"),
  item("e", "", "Nothing"),
];

function selectApp({ root = {}, trigger = {}, content = {}, items = defaultItems() } = {}) {
  return h(
    "div",
    { style: { width: 300, height: 300 } },
    h("div", { testId: "other", tabIndex: 0 }),
    c(
      Select.Root,
      mergeProps({ testId: "root" }, root),
      () => c(Select.Trigger, mergeProps({ testId: "trigger" }, trigger), () => c(Select.Value, { testId: "value", placeholder: "Pick one" })),
      () => c(Select.Content, mergeProps({ testId: "content" }, content), ...items),
    ),
  );
}

/** The marks of every item: H highlighted, S selected, D disabled. */
function marks(ui, ids = ["a", "b", "c", "e"]) {
  return Object.fromEntries(ids.filter((id) => ui.has(`item-${id}`)).map((id) => [id, ui.node(`item-${id}`).style.color]));
}

const highlighted = (ui, ids) => Object.entries(marks(ui, ids)).filter(([, mark]) => mark[0] === "H").map(([id]) => id);

test("CMP-005 R1, R3: defaults, trigger tab index and the placeholder", () => {
  const ui = mount(() => selectApp());
  assert.equal(ui.has("content"), false, "closed by default");
  assert.equal(textOf(ui.node("value")), "Pick one");
  assert.equal(ui.node("trigger").props.tabIndex, 0);
  assert.equal(ui.node("trigger").props.role, "button");
  assert.equal(ui.node("root").style.display, "flex");
  assert.equal(ui.node("root").style.position, "relative");
  assert.equal(ui.node("root").style.alignItems, "flex-start");
  const disabled = mount(() => selectApp({ root: { disabled: true } }));
  assert.equal(disabled.node("trigger").props.tabIndex, -1);
  const override = mount(() => selectApp({ root: { disabled: true }, trigger: { disabled: false } }));
  assert.equal(override.node("trigger").props.tabIndex, 0, "a trigger may override disabled");
});

test("CMP-005 R3 (Solid): the root's disabled state reaches the trigger reactively", () => {
  const [off, setOff] = createSignal(false);
  const ui = mount(() =>
    selectApp({
      root: {
        get disabled() {
          return off();
        },
      },
    }),
  );
  assert.equal(ui.node("trigger").props.tabIndex, 0);
  ui.set(() => setOff(true));
  assert.equal(ui.node("trigger").props.tabIndex, -1);
  ui.emit("trigger", { type: "click", button: 0 });
  assert.equal(ui.has("content"), false);
});

test("CMP-005 R3: trigger style functions receive open, disabled and placeholder; aria-expanded follows the state", () => {
  const seen = [];
  const ui = mount(() => selectApp({ trigger: { style: (state) => (seen.push(state), {}) } }));
  assert.deepEqual(seen.at(-1), { open: false, disabled: false, placeholder: true });
  assert.equal(ui.node("trigger").props["aria-expanded"], false);
  ui.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(seen.at(-1), { open: true, disabled: false, placeholder: true });
  assert.equal(ui.node("trigger").props["aria-expanded"], true);
});

test("CMP-005 A3: Down on a closed trigger opens without moving the highlight; Enter and Space toggle", () => {
  const ui = mount(() => selectApp());
  ui.key("trigger", "down");
  assert.ok(ui.has("content"));
  assert.deepEqual(highlighted(ui), [], "opening does not advance");
  ui.key("trigger", "enter");
  assert.equal(ui.has("content"), false, "Enter toggles");
  ui.key("trigger", "space");
  assert.ok(ui.has("content"));
  ui.key("trigger", "space");
  assert.equal(ui.has("content"), false);
  ui.key("trigger", "up");
  assert.ok(ui.has("content"), "Up opens a closed popup too");
});

test("CMP-005 R4: Down and Up on an open trigger step the highlight; caller handlers run first and a prevented default is not honored", () => {
  const log = [];
  const ui = mount(() =>
    selectApp({
      trigger: {
        onKeyDown: (event) => {
          log.push("caller");
          event.preventDefault();
        },
        onClick: () => log.push("caller click"),
      },
    }),
  );
  ui.key("trigger", "down");
  assert.ok(ui.has("content"), "the internal handler ran although the caller prevented the default");
  ui.key("trigger", "down");
  assert.deepEqual(highlighted(ui), ["a"]);
  ui.key("trigger", "down");
  assert.deepEqual(highlighted(ui), ["c"]);
  ui.key("trigger", "up");
  assert.deepEqual(highlighted(ui), ["a"]);
  ui.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(log.at(-1), "caller click");
});

test("CMP-005 A3, R4, R6: Down, Up, Home and End move the highlight, wrapping and skipping disabled items; Ctrl-N and Ctrl-P are not bound", () => {
  const ui = mount(() => selectApp({ root: { defaultOpen: true } }));
  const press = (name, over) => ui.key("content", name, over);
  press("down");
  assert.deepEqual(highlighted(ui), ["a"], "forward from none selects the first");
  press("down");
  assert.deepEqual(highlighted(ui), ["c"], "the disabled item is skipped");
  press("down");
  press("down");
  assert.deepEqual(highlighted(ui), ["a"], "wraps to the first");
  press("up");
  assert.deepEqual(highlighted(ui), ["e"], "backward wraps to the last");
  press("home");
  assert.deepEqual(highlighted(ui), ["a"]);
  press("end");
  assert.deepEqual(highlighted(ui), ["e"]);
  press("n", { ctrl: true });
  assert.deepEqual(highlighted(ui), ["e"], "Ctrl-N does nothing in Solid");
  press("p", { ctrl: true });
  assert.deepEqual(highlighted(ui), ["e"], "Ctrl-P does nothing in Solid");
  press("home");
  press("n", { ctrl: true });
  assert.deepEqual(highlighted(ui), ["a"]);
  const closed = mount(() => selectApp());
  closed.key("trigger", "n", { ctrl: true });
  assert.equal(closed.has("content"), false, "Ctrl-N on a closed trigger does not open it either");
  closed.key("trigger", "p", { ctrl: true });
  assert.equal(closed.has("content"), false);
});

test("CMP-005 A3, R6: Enter selects a truthy highlighted value and ignores an empty-string highlight; a click selects it", () => {
  const changes = [];
  const ui = mount(() => selectApp({ root: { defaultOpen: true, onValueChange: (value) => changes.push(value) } }));
  ui.key("content", "end");
  assert.deepEqual(highlighted(ui), ["e"]);
  ui.key("content", "enter");
  ui.key("content", "space");
  assert.deepEqual(changes, [], "an empty-string highlight is not keyboard-submittable");
  assert.ok(ui.has("content"));
  ui.emit("item-e", { type: "click", button: 0 });
  assert.deepEqual(changes, [""], "clicking that item selects it");
  assert.equal(ui.has("content"), false);
  const second = mount(() => selectApp({ root: { defaultOpen: true, onValueChange: (value) => changes.push(value) } }));
  second.key("content", "down");
  second.key("content", "enter");
  assert.deepEqual(changes, ["", "a"]);
  assert.equal(textOf(second.node("value")), "Apple", "the value shows the item's text without root metadata");
  const space = mount(() => selectApp({ root: { defaultOpen: true, onValueChange: (value) => changes.push(value) } }));
  space.key("content", "end");
  space.key("content", "up");
  space.key("content", "space");
  assert.deepEqual(changes, ["", "a", "c"]);
});

test("CMP-005 R6: the popup takes focus, has tab index -1 and traps Tab", () => {
  const ui = mount(() => selectApp({ root: { defaultOpen: true } }));
  assert.equal(ui.node("content").props.tabIndex, -1);
  assert.equal(ui.focused(), ui.node("content").id, "the popup takes focus once mounted");
  ui.renderer.emit({ id: ui.node("content").id, type: "keyDown", key: "tab", keystroke: 9000 });
  assert.equal(ui.has("content"), true);
  assert.deepEqual(ui.renderer.calls.filter((call) => call[0] === "focusNext"), [], "the default Tab navigation did not run");
});

test("CMP-005 R6: opening takes focus after a trigger request too", () => {
  const ui = mount(() => selectApp());
  assert.equal(ui.focused(), null);
  ui.emit("trigger", { type: "click", button: 0 });
  assert.equal(ui.focused(), ui.node("content").id);
});

test("CMP-005 R7, A4: item hover highlights, click selects, closes and restores focus to the trigger; caller handlers run first", () => {
  const log = [];
  const ui = mount(() =>
    selectApp({
      root: { defaultOpen: true, onValueChange: (value) => log.push(`value ${value}`) },
      items: [
        item("a", "a", "Apple", { onEnter: () => log.push("caller enter"), onClick: () => log.push("caller click") }),
        item("b", "b", "Banana", { disabled: true }),
      ],
    }),
  );
  ui.emit("item-a", { type: "enter" });
  assert.deepEqual(highlighted(ui, ["a", "b"]), ["a"]);
  ui.emit("item-b", { type: "enter" });
  assert.deepEqual(highlighted(ui, ["a", "b"]), ["a"], "a disabled item is not highlighted");
  ui.emit("item-b", { type: "click", button: 0 });
  assert.ok(ui.has("content"), "a disabled item cannot be selected");
  ui.emit("item-a", { type: "click", button: 0 });
  assert.deepEqual(log, ["caller enter", "caller click", "value a"]);
  assert.equal(ui.has("content"), false);
  assert.equal(ui.focused(), ui.node("trigger").id, "focus returns to the trigger");
  assert.equal(textOf(ui.node("value")), "Apple");
});

test("CMP-005 R7 (Solid): item roles, selected state and render-function children", () => {
  const ui = mount(() =>
    selectApp({
      root: { defaultOpen: true, defaultValue: "a" },
      items: [
        () =>
          c(Select.Item, {
            testId: "item-a",
            value: "a",
            children: (state) => h("text", { testId: "label-a" }, `${state.selected ? "on" : "off"}:${state.highlighted ? "hot" : "cold"}:${state.disabled}`),
          }),
        item("c", "c", "Cherry"),
      ],
    }),
  );
  assert.equal(ui.node("item-a").props.role, "option");
  assert.equal(ui.node("item-a").props["aria-selected"], true);
  assert.equal(ui.node("item-c").props["aria-selected"], false);
  assert.equal(textOf(ui.node("label-a")), "on:cold:false");
  ui.emit("item-a", { type: "enter" });
  assert.equal(textOf(ui.node("label-a")), "on:hot:false", "function children follow the state");
  ui.emit("item-c", { type: "click", button: 0 });
  assert.equal(textOf(ui.node("value")), "Cherry");
  const second = mount(() => selectApp({ root: { defaultOpen: true }, items: [() => c(Select.Item, { testId: "item-a", value: "a", children: () => h("text", {}, "rendered") })] }));
  second.emit("item-a", { type: "click", button: 0 });
  assert.equal(textOf(second.node("value")), "a", "a render-function item has its value as the label");
});

test("CMP-005 R7 (Solid): an item needs a string value", () => {
  assert.throws(() => mount(() => selectApp({ root: { defaultOpen: true }, items: [() => c(Select.Item, { value: 5 }, "five")] })), /Select\.Item requires a string value/);
});

test("CMP-005 R8 (Solid): a part outside its root throws", () => {
  assert.throws(() => mount(() => c(Select.Trigger, {})), /Select\.Trigger must be used inside Select\.Root/);
  assert.throws(() => mount(() => c(Select.Group, {})), /Select\.Group must be used inside Select\.Root/);
});

test("CMP-005 R2, A2: labels come from the root metadata, then the item's text, then the value", () => {
  const mapped = mount(() => selectApp({ root: { defaultValue: "a", items: { a: "Mapped apple" } } }));
  assert.equal(textOf(mapped.node("value")), "Mapped apple");
  const listed = mount(() => selectApp({ root: { defaultValue: "c", items: [{ value: "c", label: "Cherry!" }] } }));
  assert.equal(textOf(listed.node("value")), "Cherry!");
  const raw = mount(() => selectApp({ root: { defaultValue: "zzz" } }));
  assert.equal(textOf(raw.node("value")), "zzz", "the raw value when nothing maps it");
  const explicit = mount(() =>
    c(Select.Root, { defaultValue: "a" }, () => c(Select.Trigger, {}, () => c(Select.Value, { testId: "value", placeholder: "x" }, "Explicit children"))),
  );
  assert.equal(textOf(explicit.node("value")), "Explicit children");
  const textValue = mount(() =>
    selectApp({ root: { defaultOpen: true }, items: [item("a", "a", "Apple", { textValue: "Pomme" })] }),
  );
  textValue.emit("item-a", { type: "click", button: 0 });
  assert.equal(textOf(textValue.node("value")), "Pomme", "textValue is the label metadata");
});

test("CMP-005 R2 (Solid): the root metadata is read live, the item labels are captured when the item is created", () => {
  const [label, setLabel] = createSignal("Apple");
  const [items, setItems] = createSignal({ a: "Mapped" });
  const ui = mount(() =>
    selectApp({
      root: {
        defaultOpen: true,
        get items() {
          return items();
        },
      },
      items: [() => c(Select.Item, { testId: "item-a", value: "a", get textValue() { return label(); } }, "Apple")],
    }),
  );
  ui.emit("item-a", { type: "click", button: 0 });
  assert.equal(textOf(ui.node("value")), "Mapped");
  ui.set(() => setItems({ a: "Remapped" }));
  assert.equal(textOf(ui.node("value")), "Remapped");
  ui.set(() => {
    setItems(undefined);
    setLabel("Changed later");
  });
  assert.equal(textOf(ui.node("value")), "Apple", "the label recorded at creation, not the changed textValue");
});

test("CMP-005 A2: items inside wrapper components register and their text becomes the label", () => {
  const Wrapped = (props) => h("div", {}, c(Select.Item, { value: props.value, testId: `item-${props.value}` }, () => h("text", {}, props.children)));
  const ui = mount(() =>
    c(
      Select.Root,
      { defaultOpen: true },
      () => c(Select.Trigger, { testId: "trigger" }, () => c(Select.Value, { testId: "value", placeholder: "Pick" })),
      () => c(Select.Content, { testId: "content" }, () => c(Wrapped, { value: "w" }, "Wrapped label")),
    ),
  );
  ui.emit("item-w", { type: "click", button: 0 });
  assert.equal(textOf(ui.node("value")), "Wrapped label");
});

test("CMP-005 R5 (Solid): the highlight initializes on an open request from the registry and does not track later registrations", () => {
  const time = installFakeTime();
  try {
    const exiting = { exit: { opacity: 0 }, transition: { duration: 0.1 } };
    const ui = mount(() => selectApp({ root: { defaultValue: "c" }, content: exiting }));
    ui.key("trigger", "enter");
    assert.deepEqual(highlighted(ui), [], "the items were not registered when the request was made");
    ui.key("trigger", "enter");
    assert.ok(ui.has("content"), "closing for its exit: the items are still registered");
    assert.deepEqual(highlighted(ui), [], "a close request clears the highlight");
    ui.key("trigger", "enter");
    assert.deepEqual(highlighted(ui), ["c"], "an open request finds the recorded selected item");
    ui.key("trigger", "enter");
    ui.key("trigger", "enter");
    ui.key("content", "up");
    assert.deepEqual(highlighted(ui), ["a"]);
  } finally {
    time.restore();
  }
});

test("CMP-005 R5 (Solid): the recorded selected item must be enabled; with no selected item the highlight starts empty", () => {
  const time = installFakeTime();
  try {
    const exiting = { exit: { opacity: 0 }, transition: { duration: 0.1 } };
    for (const value of ["b", "zzz", undefined]) {
      const ui = mount(() => selectApp({ root: { defaultOpen: true, defaultValue: value }, content: exiting }));
      ui.key("trigger", "enter");
      ui.key("trigger", "enter");
      assert.deepEqual(highlighted(ui), [], `value ${value}`);
    }
  } finally {
    time.restore();
  }
});

test("CMP-005 R5 (Solid): an open popup does not initialize from a late item or a changed value, and stepping continues from a removed highlight", () => {
  const [withC, setWithC] = createSignal(false);
  const [value, setValue] = createSignal("c");
  const ui = mount(() =>
    selectApp({
      root: {
        defaultOpen: true,
        get value() {
          return value();
        },
      },
      items: [
        item("a", "a", "Apple"),
        () =>
          c(Show, {
            get when() {
              return withC();
            },
            get children() {
              return item("c", "c", "Cherry")();
            },
          }),
      ],
    }),
  );
  assert.deepEqual(highlighted(ui, ["a", "c"]), []);
  ui.set(() => setWithC(true));
  assert.deepEqual(highlighted(ui, ["a", "c"]), [], "the selected item mounting late does not take the highlight (React does)");
  ui.key("content", "down");
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["a"]);
  ui.key("content", "down");
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["c"], "navigation follows the mounted registration order");
  ui.set(() => setWithC(false));
  assert.deepEqual(highlighted(ui, ["a", "c"]), [], "the removed item is gone");
  ui.key("content", "down");
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["a"], "stepping from a highlight that is not registered enters at the first item");
  ui.set(() => setValue("a"));
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["a"], "a changed value does not reinitialize the highlight");
});

test("CMP-005 A2, CMP-002 A4: pressing the open trigger leaves it closed (outside press then click), in either delivery order", async () => {
  for (const order of ["outside first", "trigger first"]) {
    const ui = mount(() => selectApp({ root: { defaultOpen: true } }));
    const press = () => ui.emit("trigger", { type: "pointerDown", button: 0 });
    const outside = () => ui.emit("content", { type: "outsideDown", x: 1, y: 1 });
    if (order === "outside first") (outside(), press());
    else (press(), outside());
    await microtasks();
    ui.emit("trigger", { type: "click", button: 0 });
    assert.equal(ui.has("content"), false, `${order}: closed exactly once, not closed and reopened`);
    await microtasks();
    assert.equal(ui.has("content"), false);
  }
});

test("CMP-002 A4: pressing a second trigger while the first popup is open switches in one click", async () => {
  const ui = mount(() =>
    h(
      "div",
      {},
      c(
        Select.Root,
        { defaultOpen: true },
        () => c(Select.Trigger, { testId: "t1" }, () => c(Select.Value, { placeholder: "one" })),
        () => c(Select.Content, { testId: "c1" }, () => c(Select.Item, { value: "a" }, "A")),
      ),
      c(
        Select.Root,
        {},
        () => c(Select.Trigger, { testId: "t2" }, () => c(Select.Value, { placeholder: "two" })),
        () => c(Select.Content, { testId: "c2" }, () => c(Select.Item, { value: "b" }, "B")),
      ),
    ),
  );
  ui.emit("c1", { type: "outsideDown", x: 1, y: 1 });
  ui.emit("t2", { type: "pointerDown", button: 0 });
  await microtasks();
  ui.emit("t2", { type: "click", button: 0 });
  assert.equal(ui.has("c1"), false, "the first popup closed on the outside press");
  assert.equal(ui.has("c2"), true, "the second opened on the click");
});

test("CMP-005 A5: an outside press closes without moving focus away from the pressed control", async () => {
  const ui = mount(() => selectApp({ root: { defaultOpen: true } }));
  ui.renderer.focusElement(ui.node("other").id);
  ui.emit("content", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.equal(ui.has("content"), false);
  assert.equal(ui.focused(), ui.node("other").id, "focus stays where the press put it");
});

test("CMP-005 A5, CMP-002 R7, R10: Escape calls the caller handler first, then closes and focuses the trigger", () => {
  const log = [];
  const ui = mount(() =>
    selectApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, content: { onEscapeKeyDown: () => log.push("escape handler") } }),
  );
  assert.equal(ui.focused(), ui.node("content").id);
  ui.windowKey("escape");
  assert.deepEqual(log, ["escape handler", "open false"]);
  assert.equal(ui.has("content"), false);
  assert.equal(ui.focused(), ui.node("trigger").id);
});

test("CMP-005 A5, CMP-002 R7: onEscapeKeyDown may prevent the dismissal", () => {
  const ui = mount(() => selectApp({ root: { defaultOpen: true }, content: { onEscapeKeyDown: (event) => event.preventDefault() } }));
  ui.windowKey("escape");
  assert.ok(ui.has("content"));
});

test("CMP-002 A2 (headless): with a select open inside another layer one Escape closes the select only", () => {
  const [outerOpen, setOuterOpen] = createSignal(true);
  const ui = mount(() =>
    h(
      "div",
      {},
      c(Show, {
        get when() {
          return outerOpen();
        },
        get children() {
          return c(DismissableLayer, { testId: "outer", onDismiss: () => setOuterOpen(false) }, () => selectApp({ root: { defaultOpen: true } }));
        },
      }),
    ),
  );
  ui.windowKey("escape");
  assert.equal(ui.has("content"), false, "the select, opened above, closed first");
  assert.ok(ui.has("outer"));
  ui.windowKey("escape");
  assert.equal(ui.has("outer"), false, "the second Escape closes the outer layer");
});

test("CMP-005 A4, R7: a disabled root keeps popup navigation but blocks value selection; disabled items cannot be selected", () => {
  const changes = [];
  const ui = mount(() => selectApp({ root: { defaultOpen: true, disabled: true, onValueChange: (value) => changes.push(value) } }));
  ui.key("content", "down");
  assert.deepEqual(highlighted(ui), ["a"], "navigation still moves the highlight in Solid");
  ui.key("content", "down");
  assert.deepEqual(highlighted(ui), ["c"]);
  ui.key("content", "home");
  assert.deepEqual(highlighted(ui), ["a"]);
  ui.emit("item-c", { type: "enter" });
  assert.deepEqual(highlighted(ui), ["c"], "item enter highlights an enabled item");
  ui.key("content", "enter");
  ui.emit("item-c", { type: "click", button: 0 });
  assert.deepEqual(changes, [], "selection is blocked");
  assert.ok(ui.has("content"));
  ui.key("trigger", "down");
  ui.emit("trigger", { type: "click", button: 0 });
  assert.ok(ui.has("content"), "a disabled trigger does not toggle");

  const enabled = mount(() => selectApp({ root: { defaultOpen: true, onValueChange: (value) => changes.push(value) } }));
  enabled.emit("item-b", { type: "click", button: 0 });
  assert.deepEqual(changes, [], "a recorded-disabled item cannot be selected");
  enabled.emit("item-c", { type: "click", button: 0 });
  assert.deepEqual(changes, ["c"]);
});

test("CMP-005 R8: group, label, separator and scroll buttons are plain containers", () => {
  const ui = mount(() =>
    c(
      Select.Root,
      { defaultOpen: true },
      () => c(Select.Trigger, {}),
      () =>
        c(
          Select.Content,
          {},
          () =>
            c(
              Select.Group,
              { testId: "group" },
              () => c(Select.Label, { testId: "label" }, "Fruit"),
              () => c(Select.Item, { value: "a" }, "A"),
              () => c(Select.Separator, { testId: "sep" }),
            ),
          () => c(Select.ScrollUpButton, { testId: "up" }),
          () => c(Select.ScrollDownButton, { testId: "down" }),
        ),
    ),
  );
  for (const id of ["group", "label", "sep", "up", "down"]) {
    const node = ui.node(id);
    assert.equal(node.type, "div");
    assert.deepEqual(Object.keys(node.props).sort(), ["testId"], `${id} adds nothing`);
  }
  assert.equal(textOf(ui.node("label")), "Fruit");
  ui.key("up", "down");
  assert.ok(ui.has("group"));
});

test("CMP-005 R8 (Solid): select parts merge onto a host element child with asChild", () => {
  const ui = mount(() =>
    c(
      Select.Root,
      { defaultOpen: true },
      () => c(Select.Trigger, { asChild: true }, () => h("div", { testId: "trigger", onClick: () => {} })),
      () => c(Select.Content, {}, () => c(Select.Label, { asChild: true }, () => h("text", { testId: "label" }, "Fruit"))),
    ),
  );
  assert.equal(ui.node("trigger").props.role, "button");
  assert.equal(ui.node("trigger").props.tabIndex, 0);
  assert.equal(ui.node("label").type, "text");
});

test("CMP-005 R3: a controlled value and open state stay with the owner", () => {
  const requests = [];
  const ui = mount(() =>
    selectApp({ root: { value: "a", open: false, onValueChange: (v) => requests.push(["value", v]), onOpenChange: (o) => requests.push(["open", o]) } }),
  );
  assert.equal(textOf(ui.node("value")), "a");
  ui.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(requests, [["open", true]]);
  assert.equal(ui.has("content"), false, "the owner did not open it");
});

test("CMP-005 R1 (Solid): controlled value and open state follow the owner reactively", () => {
  const [value, setValue] = createSignal("a");
  const [open, setOpen] = createSignal(false);
  const ui = mount(() =>
    selectApp({
      root: {
        get value() {
          return value();
        },
        get open() {
          return open();
        },
      },
    }),
  );
  ui.set(() => setOpen(true));
  assert.ok(ui.has("content"));
  assert.equal(marks(ui).a[1], "S");
  ui.set(() => setValue("c"));
  assert.equal(marks(ui).a[1], "-");
  assert.equal(marks(ui).c[1], "S");
  assert.equal(textOf(ui.node("value")), "Cherry", "the label recorded for the mounted item");
  ui.set(() => setOpen(false));
  assert.equal(ui.has("content"), false);
});

// ---- Presence of headless parts (CMP-002 R17) -----------------------------------------------

const exiting = { exit: { opacity: 0 }, transition: { duration: 0.1 } };

test("CMP-002 R17: a closing select stays mounted but inert until its exit completes, while Escape and focus follow the close", async () => {
  const ui = mount(() => selectApp({ root: { defaultOpen: true }, content: exiting }));
  assert.equal(ui.node("content").props.motion.present, true);
  assert.equal(ui.focused(), ui.node("content").id);
  ui.windowKey("escape");
  assert.ok(ui.has("content"), "the closing popup stays mounted");
  const motion = ui.node("content").props.motion;
  assert.deepEqual([motion.present, motion.exit], [false, { opacity: 0 }]);
  assert.equal(ui.focused(), ui.node("trigger").id, "focus handling follows the close from its start");
  assert.equal(ui.windowKey("escape"), undefined);
  assert.ok(ui.has("content"), "its own Escape registration is gone: a second Escape does not touch it");
  assert.equal(anchoredOfSelect(ui).children[0].style.pointerEvents, "none", "the closing surface is inert");
  ui.keyDown("content", "down");
  ui.emit("item-a", { type: "click", button: 0 });
  assert.equal(ui.node("item-a").style.color[1], "-", "a closing popup takes no selection");
  ui.emit("content", { type: "motionComplete", generation: motion.generation + 7, exit: true });
  assert.ok(ui.has("content"), "a stale generation does not release it");
  ui.emit("content", { type: "motionComplete", generation: motion.generation, exit: true });
  await settle();
  assert.equal(ui.has("content"), false, "the current exit completion unmounts it");
});

function anchoredOfSelect(ui) {
  return ui.snapshot().children[0].children[1].children.find((child) => child.type === "anchored");
}

test("CMP-002 R17: the closing popup's surface is pointer-transparent", () => {
  const ui = mount(() => selectApp({ root: { defaultOpen: true }, content: { ...exiting, style: { color: "red" } } }));
  ui.windowKey("escape");
  const inner = anchoredOfSelect(ui).children[0];
  assert.equal(inner.style.pointerEvents, "none", "inert while closing");
  assert.equal(inner.style.color, "red", "the caller's style is kept");
  assert.equal(anchoredOfSelect(ui).props.occlude, false);
});

test("CMP-002 R17: a delayed unmount at the end of the exit duration is the fallback; reopening cancels the exit", async () => {
  const time = installFakeTime();
  try {
    const ui = mount(() => selectApp({ root: { defaultOpen: true }, content: exiting }));
    ui.key("trigger", "enter");
    assert.ok(ui.has("content"));
    time.advance(119);
    await settle();
    assert.ok(ui.has("content"));
    time.advance(1);
    await settle();
    assert.equal(ui.has("content"), false, "unmounted after the exit duration plus the slack");
    ui.key("trigger", "enter");
    assert.equal(ui.node("content").props.motion.present, true);
    ui.key("trigger", "enter");
    ui.key("trigger", "enter");
    assert.equal(ui.node("content").props.motion.present, true, "reopening mid-exit leaves it open");
    time.advance(500);
    await settle();
    assert.ok(ui.has("content"));
  } finally {
    time.restore();
  }
});

test("CMP-002 R17: the fallback waits for the transition delay too", async () => {
  const time = installFakeTime();
  try {
    const ui = mount(() => selectApp({ root: { defaultOpen: true }, content: { exit: { opacity: 0 }, transition: { duration: 0.1, delay: 0.05 } } }));
    ui.key("trigger", "enter");
    time.advance(169);
    await settle();
    assert.ok(ui.has("content"));
    time.advance(1);
    await settle();
    assert.equal(ui.has("content"), false);
    const defaults = mount(() => selectApp({ root: { defaultOpen: true }, content: { exit: { opacity: 0 } } }));
    defaults.key("trigger", "enter");
    time.advance(319);
    await settle();
    assert.ok(defaults.has("content"), "the default exit duration is 0.3 s");
    time.advance(1);
    await settle();
    assert.equal(defaults.has("content"), false);
  } finally {
    time.restore();
  }
});

test("CMP-002 R17: content without an exit target unmounts as soon as it closes", () => {
  const ui = mount(() => selectApp({ root: { defaultOpen: true } }));
  assert.equal(ui.node("content").props.motion, undefined);
  ui.windowKey("escape");
  assert.equal(ui.has("content"), false);
});

test("CMP-002 R17: a closing tooltip is inert and its Escape registration is gone", async () => {
  const [gone, setGone] = createSignal(false);
  const ui = mount(() =>
    c(Show, {
      get when() {
        return !gone();
      },
      get fallback() {
        return h("div", { testId: "gone" });
      },
      get children() {
        return c(DismissableLayer, { testId: "outer", onDismiss: () => setGone(true) }, () => tooltipApp({ root: { defaultOpen: true }, content: exiting }));
      },
    }),
  );
  ui.emit("trigger", { type: "blur" });
  await settle();
  assert.ok(ui.has("content"), "still mounted for its exit");
  assert.equal(ui.node("content").style.pointerEvents, "none", "inert while closing");
  ui.windowKey("escape");
  await settle();
  assert.ok(ui.has("gone"), "Escape belongs to the layer below once the tooltip began closing");
});

test("CMP-002 R17 (Solid): createPresence follows an accessor and restarts the generation on every open change", () => {
  const [open, setOpen] = createSignal(true);
  let presence;
  mount(() => {
    presence = createPresence(open, { exit: { opacity: 0 }, animate: { opacity: 1 }, transition: { duration: 0.2 } });
    return h("div");
  });
  assert.deepEqual([presence.mounted(), presence.closing()], [true, false]);
  const first = presence.motion();
  assert.deepEqual([first.present, first.animate, first.exit], [true, { opacity: 1 }, { opacity: 0 }]);
  setOpen(false);
  assert.deepEqual([presence.mounted(), presence.closing()], [true, true]);
  assert.equal(presence.motion().generation, first.generation + 1);
  presence.onMotionComplete({ exit: true, generation: first.generation });
  assert.equal(presence.mounted(), true, "a stale generation is ignored");
  presence.onMotionComplete({ exit: false, generation: first.generation + 1 });
  assert.equal(presence.mounted(), true, "an entrance completion is ignored");
  presence.onMotionComplete({ exit: true, generation: first.generation + 1 });
  assert.deepEqual([presence.mounted(), presence.closing()], [false, false]);
  setOpen(true);
  assert.equal(presence.mounted(), true);
  const plain = [];
  mount(() => {
    const none = createPresence(() => false, {});
    plain.push(none.mounted(), none.motion());
    return h("div");
  });
  assert.deepEqual(plain, [false, undefined]);
});
