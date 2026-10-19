// The headless components for React: Button and child composition (CMP-001), floating surfaces
// and dismissal layers (CMP-002), the compound tooltip (CMP-004) and the compound select (CMP-005).

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { Button, DismissableLayer, FloatingLayer, Select, Tooltip, buttonProps, useControllableState } from "../src/index.js";
import { h, installFakeTime, microtasks, mount, settle, textOf } from "./components-helpers.js";

test("CMP-001 R1: a button is a focusable generic container with role button", () => {
  const ui = mount(h(Button, { testId: "b" }, "Go"));
  const node = ui.node("b");
  assert.equal(node.type, "div");
  assert.equal(node.props.role, "button");
  assert.equal(node.props.tabIndex, 0);
  assert.deepEqual(ui.errors, []);
});

test("CMP-001 A3: a disabled button has tab index -1, or 0 with focusableWhenDisabled, and never activates", () => {
  const log = [];
  const ui = mount(
    h(
      "div",
      {},
      h(Button, { testId: "a", disabled: true, onClick: () => log.push("a") }),
      h(Button, { testId: "b", disabled: true, focusableWhenDisabled: true, onClick: () => log.push("b") }),
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
  const ui = mount(h(Button, { testId: "b", onClick: () => log.push("activate") }, "Go"));
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
  const ui = mount(h("div", {}, h(Button, { testId: "a", style }), h(Button, { testId: "b", style, disabled: true }), h(Button, { testId: "c", style: { width: 5 } })));
  assert.equal(ui.node("a").style.opacity, 1);
  assert.equal(ui.node("b").style.opacity, 0.4);
  assert.equal(ui.node("c").style.width, 5);
});

test("CMP-001 R4: buttonProps supplies the same behavior to any control", () => {
  const log = [];
  function Control() {
    const latch = React.useRef({ enter: false }).current;
    const props = buttonProps({ latch, onClick: () => log.push("activate") });
    return h("div", { testId: "c", ...props });
  }
  const ui = mount(h(Control));
  assert.deepEqual([ui.node("c").props.role, ui.node("c").props.tabIndex], ["button", 0]);
  ui.keyDown("c", "enter");
  assert.deepEqual(log, ["activate"]);
});

test("CMP-001 A2: asChild merges onto one element child (handler order, tab index, styles, refs)", () => {
  const log = [];
  const childRef = { current: null };
  const forwardedRef = { current: null };
  const ui = mount(
    h(
      Button,
      {
        asChild: true,
        ref: forwardedRef,
        tabIndex: -1,
        style: { width: 2 },
        onClick: () => log.push("part"),
        onKeyDown: () => log.push("part keydown"),
      },
      h("div", {
        testId: "child",
        ref: childRef,
        tabIndex: 7,
        style: { color: "red", width: 1 },
        onClick: () => log.push("child"),
        onKeyDown: (event) => {
          log.push("child keydown");
          event.preventDefault();
        },
      }),
    ),
  );
  const node = ui.node("child");
  assert.equal(node.props.role, "button");
  assert.equal(node.props.tabIndex, -1, "an explicit part tab index beats the child's");
  assert.deepEqual(node.style, { color: "red", width: 2 }, "child styles first, the part wins");
  assert.ok(childRef.current && childRef.current === forwardedRef.current, "both refs are delivered");
  assert.equal(childRef.current.id, node.id);
  ui.emit("child", { type: "click", button: 0 });
  assert.deepEqual(log, ["child", "part"]);
  log.length = 0;
  ui.keyDown("child", "enter");
  assert.deepEqual(log.slice(0, 2), ["child keydown", "part keydown"], "the part handler runs although the child prevented the default");
  assert.equal(log.length, 2, "the prevented default stops the activation");

  const defaults = mount(h(Button, { asChild: true }, h("div", { testId: "d", tabIndex: 7 })));
  assert.equal(defaults.node("d").props.tabIndex, 7, "the child's tab index beats the part default");
  const plain = mount(h(Button, { asChild: true }, h("div", { testId: "p" })));
  assert.equal(plain.node("p").props.tabIndex, 0);
});

test("CMP-001 A2: asChild with zero or two children throws", () => {
  const caught = [];
  const onUncaughtError = (error) => caught.push(error.message);
  mount(h(Button, { asChild: true }), { onUncaughtError });
  mount(h(Button, { asChild: true }, h("div"), h("div")), { onUncaughtError });
  mount(h(Button, { asChild: true }, "text"), { onUncaughtError });
  assert.equal(caught.length, 3);
  for (const message of caught) assert.match(message, /asChild requires exactly one React element child/);
});

test("CMP-002 A12 (hook): the controlled-state helper", () => {
  const changes = [];
  let request;
  let shown;
  function Probe({ value, defaultValue }) {
    [shown, request] = useControllableState({ value, defaultValue, onChange: (next) => changes.push(next) });
    return null;
  }
  const ui = mount(h(Probe, { defaultValue: 1 }));
  assert.equal(shown, 1);
  assert.equal(request(1), false);
  assert.equal(request(2), true);
  ui.render(h(Probe, { defaultValue: 99 }));
  assert.equal(shown, 2, "the default only initializes");
  ui.render(h(Probe, { value: 5 }));
  request(6);
  assert.equal(shown, 5, "a controlled request leaves the supplied value");
  assert.deepEqual(changes, [2, 6]);
});

test("CMP-002 A1: floating style routing puts opacity and radii on the outer surface only once", () => {
  const ui = mount(
    h(FloatingLayer, {
      testId: "content",
      style: {
        opacity: 0.5,
        backgroundColor: "#fff",
        borderTopLeftRadius: 4,
        borderBottomRightRadius: 12,
        hover: { opacity: 0.8, borderTopRightRadius: 2, color: "red" },
        active: { opacity: 0.9 },
      },
    }),
  );
  const outer = ui.snapshot().children[0];
  assert.equal(outer.type, "anchored");
  assert.deepEqual(outer.style, {
    opacity: 0.5,
    borderTopLeftRadius: 4,
    borderBottomRightRadius: 12,
    hover: { opacity: 0.8, borderTopRightRadius: 2 },
    active: { opacity: 0.9 },
  });
  const inner = ui.node("content");
  assert.equal(inner.style.opacity, undefined);
  assert.equal(inner.style.hover.opacity, undefined);
  assert.equal(inner.style.active.opacity, undefined);
  assert.equal(inner.style.backgroundColor, "#fff", "the caller's fill replaces the default");
  assert.equal(inner.style.hover.color, "red", "all other content style stays on the inner container");
  assert.equal(inner.style.borderTopLeftRadius, 4);
  assert.equal(inner.style.display, "flex");
  assert.equal(inner.style.position, "relative");
  assert.equal(inner.style.alignItems, "flex-start");
});

test("CMP-002 R1, R2: floating options map to the anchored host; the default fill is #1A1A1A", () => {
  const ui = mount(h(FloatingLayer, { testId: "c", side: "right", align: "end", sideOffset: 6, alignOffset: 3, collisionPadding: 12 }));
  const outer = ui.snapshot().children[0];
  assert.deepEqual(outer.props, { side: "right", align: "end", gap: 6, offset: { x: 0, y: 3 }, fit: "snap", snapMargin: 12, deferred: true, priority: 1, occlude: true });
  assert.equal(ui.node("c").style.backgroundColor, "#1A1A1A");
  const defaults = mount(h(FloatingLayer, { testId: "c" }));
  assert.deepEqual(defaults.snapshot().children[0].props, { side: "bottom", align: "start", gap: 0, offset: { x: 0, y: 0 }, fit: "snap", snapMargin: 8, deferred: true, priority: 1, occlude: true });
  const passive = mount(h(FloatingLayer, { testId: "c", style: { pointerEvents: "none" } }));
  assert.equal(passive.snapshot().children[0].props.occlude, false);
});

// ---- Tooltip (CMP-004) ----------------------------------------------------------------------

function tooltipApp({ provider = {}, root = {}, content = {} } = {}) {
  return h(
    Tooltip.Provider,
    provider,
    h(
      Tooltip.Root,
      { testId: "root", ...root },
      h(Tooltip.Trigger, { testId: "trigger" }, h("text", {}, "?")),
      h(Tooltip.Content, { testId: "content", ...content }, h("text", {}, "tip")),
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

/** Advances virtual time and lets React apply the state the timers requested. */
async function advance(time, ms) {
  time.advance(ms);
  await settle();
}

test("CMP-004 R5, R10: the content mounts only while open with top, center and no offset by default", async () => {
  await withTime(async () => {
    const ui = mount(tooltipApp());
    assert.equal(ui.has("content"), false, "closed by default");
    assert.equal(ui.node("trigger").props.tabIndex, 0, "the trigger defaults to tab index 0");
    ui.emit("trigger", { type: "enter" });
    await settle();
    assert.ok(ui.has("content"), "a zero delay opens immediately");
    const anchored = ui.snapshot().children[0].children.find((child) => child.type === "anchored");
    assert.equal(anchored.props.side, "top");
    assert.equal(anchored.props.align, "center");
    assert.equal(anchored.props.gap, 0);
  });
});

test("CMP-004 A2(a): delay 700 and skip 300 through real hover events", async () => {
  await withTime(async (time) => {
    const ui = mount(tooltipApp({ provider: { delayDuration: 700, skipDelayDuration: 300, disableHoverableContent: true } }));
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
    const ui = mount(tooltipApp());
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
    const ui = mount(tooltipApp({ root: { disableHoverableContent: true } }));
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
    const ui = mount(
      h(
        Tooltip.Provider,
        { delayDuration: 700 },
        h(
          Tooltip.Root,
          {},
          h(Tooltip.Trigger, { testId: "trigger", onFocus: () => log.push("caller focus"), onClick: () => log.push("caller click") }),
          h(Tooltip.Content, { testId: "content" }),
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

test("CMP-004 A2(d), R9: unmount cancels pending timers", async () => {
  await withTime(async (time) => {
    const ui = mount(tooltipApp({ provider: { delayDuration: 700 } }));
    ui.emit("trigger", { type: "enter" });
    assert.equal(time.pendingTimers >= 1, true);
    ui.root.unmount();
    await settle();
    time.advance(1000);
    assert.equal(ui.renderer.elements.size, 0);
    assert.deepEqual(ui.errors, []);
  });
});

test("CMP-004 A3: React cancels the prior open timer, so repeated hover scheduling leaves one", async () => {
  await withTime(async (time) => {
    const ui = mount(tooltipApp({ provider: { delayDuration: 700 } }));
    const before = time.pendingTimers;
    ui.emit("trigger", { type: "enter" });
    ui.emit("trigger", { type: "enter" });
    ui.emit("trigger", { type: "enter" });
    assert.equal(time.pendingTimers - before, 1);
  });
});

test("CMP-004 R5: a controlled root reports requests and does not overwrite the value; a root override beats the provider", async () => {
  await withTime(async (time) => {
    const requests = [];
    const ui = mount(tooltipApp({ provider: { delayDuration: 700 }, root: { open: false, delayDuration: 100, onOpenChange: (open) => requests.push(open) } }));
    ui.emit("trigger", { type: "enter" });
    await advance(time, 100);
    assert.deepEqual(requests, [true]);
    assert.equal(ui.has("content"), false, "the owner decides");
  });
});

test("CMP-004 R5: provider changes propagate to existing roots", async () => {
  await withTime(async (time) => {
    const ui = mount(tooltipApp({ provider: { delayDuration: 700 } }));
    ui.render(tooltipApp({ provider: { delayDuration: 50 } }));
    ui.emit("trigger", { type: "enter" });
    await advance(time, 50);
    assert.ok(ui.has("content"));
  });
});

test("CMP-004 R10, CMP-002 R6, R7: Escape closes the tooltip first and goes on to the layer below it", async () => {
  await withTime(async () => {
    const ui = mount(
      h(
        DismissableLayer,
        { testId: "outer", onDismiss: () => ui.render(h("div", { testId: "gone" })) },
        tooltipApp({ root: { defaultOpen: true } }),
      ),
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
    const ui = mount(tooltipApp({ root: { defaultOpen: true, disableHoverableContent: true } }));
    const anchored = ui.snapshot().children[0].children.find((child) => child.type === "anchored");
    assert.equal(anchored.props.occlude, false);
    assert.equal(ui.node("content").style.pointerEvents, "none");
    assert.equal(ui.focused(), null, "opening never moves focus");
  });
});

// ---- Select (CMP-005) -----------------------------------------------------------------------

const itemStyle = ({ selected, highlighted, disabled }) => ({ color: `${highlighted ? "H" : "-"}${selected ? "S" : "-"}${disabled ? "D" : "-"}` });

function selectApp({ root = {}, trigger = {}, content = {}, items } = {}) {
  const defaults = [
    h(Select.Item, { key: "a", testId: "item-a", value: "a", style: itemStyle }, "Apple"),
    h(Select.Item, { key: "b", testId: "item-b", value: "b", disabled: true, style: itemStyle }, "Banana"),
    h(Select.Item, { key: "c", testId: "item-c", value: "c", style: itemStyle }, "Cherry"),
    h(Select.Item, { key: "e", testId: "item-e", value: "", style: itemStyle }, "Nothing"),
  ];
  return h(
    "div",
    { style: { width: 300, height: 300 } },
    h("div", { testId: "other", tabIndex: 0 }),
    h(
      Select.Root,
      { testId: "root", ...root },
      h(Select.Trigger, { testId: "trigger", ...trigger }, h(Select.Value, { testId: "value", placeholder: "Pick one" })),
      h(Select.Content, { testId: "content", ...content }, items ?? defaults),
    ),
  );
}

/** The marks of every item: H highlighted, S selected, D disabled. */
function marks(ui, ids = ["a", "b", "c", "e"]) {
  return Object.fromEntries(ids.filter((id) => ui.has(`item-${id}`)).map((id) => [id, ui.node(`item-${id}`).style.color]));
}

const highlighted = (ui) => Object.entries(marks(ui)).filter(([, mark]) => mark[0] === "H").map(([id]) => id);

test("CMP-005 R1, R3: defaults, trigger tab index and the placeholder", () => {
  const ui = mount(selectApp());
  assert.equal(ui.has("content"), false, "closed by default");
  assert.equal(textOf(ui.node("value")), "Pick one");
  assert.equal(ui.node("trigger").props.tabIndex, 0);
  assert.equal(ui.node("root").style.display, "flex");
  assert.equal(ui.node("root").style.position, "relative");
  const disabled = mount(selectApp({ root: { disabled: true } }));
  assert.equal(disabled.node("trigger").props.tabIndex, -1);
  const override = mount(selectApp({ root: { disabled: true }, trigger: { disabled: false } }));
  assert.equal(override.node("trigger").props.tabIndex, 0, "a trigger may override disabled");
});

test("CMP-005 R3: trigger style functions receive open, disabled and placeholder", () => {
  const seen = [];
  const ui = mount(selectApp({ trigger: { style: (state) => (seen.push(state), {}) } }));
  assert.deepEqual(seen.at(-1), { open: false, disabled: false, placeholder: true });
  ui.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(seen.at(-1), { open: true, disabled: false, placeholder: true });
});

test("CMP-005 A3: Down on a closed trigger opens without moving the highlight; Enter and Space toggle", () => {
  const ui = mount(selectApp());
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

test("CMP-005 A3: Down, Up, Home and End move the highlight, wrapping and skipping disabled items; Ctrl-N and Ctrl-P work", () => {
  const ui = mount(selectApp({ root: { defaultOpen: true } }));
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
  assert.deepEqual(highlighted(ui), ["a"], "Ctrl-N is Down in React");
  press("p", { ctrl: true });
  assert.deepEqual(highlighted(ui), ["e"], "Ctrl-P is Up in React");
});

test("CMP-005 A3, R6: Enter selects a truthy highlighted value and ignores an empty-string highlight; a click selects it", () => {
  const changes = [];
  const ui = mount(selectApp({ root: { defaultOpen: true, onValueChange: (value) => changes.push(value) } }));
  ui.key("content", "end");
  assert.deepEqual(highlighted(ui), ["e"]);
  ui.key("content", "enter");
  ui.key("content", "space");
  assert.deepEqual(changes, [], "an empty-string highlight is not keyboard-submittable");
  assert.ok(ui.has("content"));
  ui.emit("item-e", { type: "click", button: 0 });
  assert.deepEqual(changes, [""], "clicking that item selects it");
  assert.equal(ui.has("content"), false);
  const second = mount(selectApp({ root: { defaultOpen: true, onValueChange: (value) => changes.push(value) } }));
  second.key("content", "down");
  second.key("content", "enter");
  assert.deepEqual(changes, ["", "a"]);
  assert.equal(textOf(second.node("value")), "Apple", "the value shows the item's text without root metadata");
});

test("CMP-005 R6: the popup takes focus, has tab index -1 and traps Tab", () => {
  const ui = mount(selectApp({ root: { defaultOpen: true } }));
  assert.equal(ui.node("content").props.tabIndex, -1);
  assert.equal(ui.focused(), ui.node("content").id, "the popup takes focus once mounted");
  const event = { defaultPrevented: false };
  const down = ui.renderer.emit.bind(ui.renderer);
  down({ id: ui.node("content").id, type: "keyDown", key: "tab", keystroke: 9000 });
  assert.equal(ui.has("content"), true);
  assert.deepEqual(ui.renderer.calls.filter((call) => call[0] === "focusNext"), [], "the default Tab navigation did not run");
  void event;
});

test("CMP-005 R7, A4: item hover highlights, click selects, closes and restores focus to the trigger; caller handlers run first", () => {
  const log = [];
  const ui = mount(
    selectApp({
      root: { defaultOpen: true, onValueChange: (value) => log.push(`value ${value}`) },
      items: [
        h(Select.Item, { key: "a", testId: "item-a", value: "a", style: itemStyle, onEnter: () => log.push("caller enter"), onClick: () => log.push("caller click") }, "Apple"),
        h(Select.Item, { key: "b", testId: "item-b", value: "b", disabled: true, style: itemStyle }, "Banana"),
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
});

test("CMP-005 R2, A2: labels come from the root metadata, then the item's text, then the value", () => {
  const mapped = mount(selectApp({ root: { defaultValue: "a", items: { a: "Mapped apple" } } }));
  assert.equal(textOf(mapped.node("value")), "Mapped apple");
  const listed = mount(selectApp({ root: { defaultValue: "c", items: [{ value: "c", label: "Cherry!" }] } }));
  assert.equal(textOf(listed.node("value")), "Cherry!");
  const raw = mount(selectApp({ root: { defaultValue: "zzz" } }));
  assert.equal(textOf(raw.node("value")), "zzz", "the raw value when nothing maps it");
  const explicit = mount(
    h(
      Select.Root,
      { defaultValue: "a" },
      h(Select.Trigger, {}, h(Select.Value, { testId: "value", placeholder: "x" }, "Explicit children")),
    ),
  );
  assert.equal(textOf(explicit.node("value")), "Explicit children");
});

test("CMP-005 A2: items inside wrapper components register and their text becomes the label", () => {
  function Wrapped({ value, children }) {
    return h("div", {}, h(Select.Item, { value, testId: `item-${value}` }, h("span", {}, children)));
  }
  const ui = mount(
    h(
      Select.Root,
      { defaultOpen: true },
      h(Select.Trigger, { testId: "trigger" }, h(Select.Value, { testId: "value", placeholder: "Pick" })),
      h(Select.Content, { testId: "content" }, h(Wrapped, { value: "w" }, "Wrapped label")),
    ),
  );
  ui.emit("item-w", { type: "click", button: 0 });
  assert.equal(textOf(ui.node("value")), "Wrapped label");
});

test("CMP-005 A2: the highlight starts on the selected item, also when it mounts late, and is re-initialized when the highlighted item is removed", async () => {
  const items = (withC) => [
    h(Select.Item, { key: "a", testId: "item-a", value: "a", style: itemStyle }, "Apple"),
    withC ? h(Select.Item, { key: "c", testId: "item-c", value: "c", style: itemStyle }, "Cherry") : null,
  ];
  const ui = mount(selectApp({ root: { defaultOpen: true, defaultValue: "c" }, items: items(false) }));
  assert.deepEqual(highlighted(ui, ["a", "c"]), [], "no selected item is registered yet");
  ui.render(selectApp({ root: { defaultOpen: true, defaultValue: "c" }, items: items(true) }));
  await settle();
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["c"], "the selected item that mounts late takes the highlight");
  ui.key("content", "up");
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["a"]);
  ui.render(selectApp({ root: { defaultOpen: true, defaultValue: "c" }, items: [] }));
  await settle();
  ui.render(selectApp({ root: { defaultOpen: true, defaultValue: "c" }, items: items(true) }));
  await settle();
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["c"]);
  ui.render(selectApp({ root: { defaultOpen: true, defaultValue: "c" }, items: items(false) }));
  await settle();
  assert.deepEqual(highlighted(ui, ["a", "c"]), [], "removing the selected, highlighted item leaves no highlight");
  ui.key("content", "down");
  assert.deepEqual(highlighted(ui, ["a", "c"]), ["a"]);
  ui.render(selectApp({ root: { defaultOpen: true, defaultValue: "a" }, items: [] }));
  await settle();
  assert.deepEqual(highlighted(ui, ["a", "c"]), []);
});

test("CMP-005 A2, CMP-002 A4: pressing the open trigger leaves it closed (outside press then click), in either delivery order", async () => {
  for (const order of ["outside first", "trigger first"]) {
    const ui = mount(selectApp({ root: { defaultOpen: true } }));
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
  const ui = mount(
    h(
      "div",
      {},
      h(
        Select.Root,
        { defaultOpen: true },
        h(Select.Trigger, { testId: "t1" }, h(Select.Value, { placeholder: "one" })),
        h(Select.Content, { testId: "c1" }, h(Select.Item, { value: "a" }, "A")),
      ),
      h(
        Select.Root,
        {},
        h(Select.Trigger, { testId: "t2" }, h(Select.Value, { placeholder: "two" })),
        h(Select.Content, { testId: "c2" }, h(Select.Item, { value: "b" }, "B")),
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
  const ui = mount(selectApp({ root: { defaultOpen: true } }));
  ui.renderer.focusElement(ui.node("other").id);
  ui.emit("content", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.equal(ui.has("content"), false);
  assert.equal(ui.focused(), ui.node("other").id, "focus stays where the press put it");
});

test("CMP-005 A5, CMP-002 R7, R10: Escape calls the caller handler first, then closes and focuses the trigger", () => {
  const log = [];
  const ui = mount(selectApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(`open ${open}`) }, content: { onEscapeKeyDown: () => log.push("escape handler") } }));
  assert.equal(ui.focused(), ui.node("content").id);
  ui.windowKey("escape");
  assert.deepEqual(log, ["escape handler", "open false"]);
  assert.equal(ui.has("content"), false);
  assert.equal(ui.focused(), ui.node("trigger").id);
});

test("CMP-005 A5, CMP-002 R7: onEscapeKeyDown may prevent the dismissal", () => {
  const ui = mount(selectApp({ root: { defaultOpen: true }, content: { onEscapeKeyDown: (event) => event.preventDefault() } }));
  ui.windowKey("escape");
  assert.ok(ui.has("content"));
});

test("CMP-002 A2 (headless): with a select open inside another layer one Escape closes the select only", () => {
  let outerOpen = true;
  const tree = () =>
    h(
      "div",
      {},
      outerOpen ? h(DismissableLayer, { testId: "outer", onDismiss: () => { outerOpen = false; ui.render(tree()); } }, selectApp({ root: { defaultOpen: true } })) : null,
    );
  const ui = mount(tree());
  ui.windowKey("escape");
  assert.equal(ui.has("content"), false, "the select, opened above, closed first");
  assert.ok(ui.has("outer"));
  ui.windowKey("escape");
  assert.equal(ui.has("outer"), false, "the second Escape closes the outer layer");
});

test("CMP-005 A4: a disabled root blocks navigation and selection in React; disabled items cannot be selected", () => {
  const changes = [];
  const ui = mount(selectApp({ root: { defaultOpen: true, disabled: true, onValueChange: (value) => changes.push(value) } }));
  ui.key("content", "down");
  assert.deepEqual(highlighted(ui), [], "navigation is blocked");
  ui.emit("item-a", { type: "enter" });
  assert.deepEqual(highlighted(ui), []);
  ui.emit("item-a", { type: "click", button: 0 });
  assert.deepEqual(changes, [], "selection is blocked");
  ui.key("trigger", "down");
  ui.emit("trigger", { type: "click", button: 0 });
  assert.ok(ui.has("content"), "a disabled trigger does not toggle");
});

test("CMP-005 R8: group, label, separator and scroll buttons are plain containers", () => {
  const ui = mount(
    h(
      Select.Root,
      { defaultOpen: true },
      h(Select.Trigger, {}),
      h(
        Select.Content,
        {},
        h(Select.Group, { testId: "group" }, h(Select.Label, { testId: "label" }, "Fruit"), h(Select.Item, { value: "a" }, "A"), h(Select.Separator, { testId: "sep" })),
        h(Select.ScrollUpButton, { testId: "up" }),
        h(Select.ScrollDownButton, { testId: "down" }),
      ),
    ),
  );
  for (const id of ["group", "label", "sep", "up", "down"]) {
    const node = ui.node(id);
    assert.equal(node.type, "div");
    assert.deepEqual(Object.keys(node.props).sort(), ["testId"], `${id} adds nothing`);
  }
  ui.key("up", "down");
});

test("CMP-005 R3: a controlled value and open state stay with the owner", () => {
  const requests = [];
  const ui = mount(selectApp({ root: { value: "a", open: false, onValueChange: (v) => requests.push(["value", v]), onOpenChange: (o) => requests.push(["open", o]) } }));
  assert.equal(textOf(ui.node("value")), "a");
  ui.emit("trigger", { type: "click", button: 0 });
  assert.deepEqual(requests, [["open", true]]);
  assert.equal(ui.has("content"), false, "the owner did not open it");
});

// ---- Presence of headless parts (CMP-002 R17) -----------------------------------------------

const exiting = { exit: { opacity: 0 }, transition: { duration: 0.1 } };

test("CMP-002 R17: a closing select stays mounted but inert until its exit completes, while Escape and focus follow the close", async () => {
  const ui = mount(selectApp({ root: { defaultOpen: true }, content: exiting }));
  assert.equal(ui.node("content").props.motion.present, true);
  assert.equal(ui.focused(), ui.node("content").id);
  ui.windowKey("escape");
  assert.ok(ui.has("content"), "the closing popup stays mounted");
  const motion = ui.node("content").props.motion;
  assert.deepEqual([motion.present, motion.exit], [false, { opacity: 0 }]);
  assert.equal(ui.focused(), ui.node("trigger").id, "focus handling follows the close from its start");
  assert.equal(ui.windowKey("escape"), undefined);
  assert.ok(ui.has("content"), "its own Escape registration is gone: a second Escape does not touch it");
  const changes = [];
  ui.keyDown("content", "down");
  ui.emit("item-a", { type: "click", button: 0 });
  assert.equal(ui.node("item-a").style.color[1], "-", "a closing popup takes no selection");
  assert.deepEqual(changes, []);
  ui.emit("content", { type: "motionComplete", generation: motion.generation + 7, exit: true });
  assert.ok(ui.has("content"), "a stale generation does not release it");
  ui.emit("content", { type: "motionComplete", generation: motion.generation, exit: true });
  await settle();
  assert.equal(ui.has("content"), false, "the current exit completion unmounts it");
});

test("CMP-002 R17: a delayed unmount at the end of the exit duration is the fallback; reopening cancels the exit", async () => {
  const time = installFakeTime();
  try {
    const ui = mount(selectApp({ root: { defaultOpen: true }, content: exiting }));
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

test("CMP-002 R17: content without an exit target unmounts as soon as it closes", () => {
  const ui = mount(selectApp({ root: { defaultOpen: true } }));
  assert.equal(ui.node("content").props.motion, undefined);
  ui.windowKey("escape");
  assert.equal(ui.has("content"), false);
});

test("CMP-002 R17: a closing tooltip is inert and its Escape registration is gone", async () => {
  const ui = mount(
    h(
      DismissableLayer,
      { testId: "outer", onDismiss: () => ui.render(h("div", { testId: "gone" })) },
      tooltipApp({ root: { defaultOpen: true }, content: exiting }),
    ),
  );
  ui.emit("trigger", { type: "blur" });
  await settle();
  assert.ok(ui.has("content"), "still mounted for its exit");
  assert.equal(ui.node("content").style.pointerEvents, "none", "inert while closing");
  ui.windowKey("escape");
  await settle();
  assert.ok(ui.has("gone"), "Escape belongs to the layer below once the tooltip began closing");
});
