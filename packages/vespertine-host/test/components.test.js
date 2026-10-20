// The framework-neutral logic of the headless components (CMP-001, CMP-002, CMP-004, CMP-005).

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  backdropStyle,
  buttonProps,
  classifyKey,
  composeHandlers,
  composeRefs,
  createButtonLatch,
  createPressFlag,
  createTooltipProvider,
  createTooltipRoot,
  dismissalCloses,
  displayText,
  edgeIndex,
  filterItems,
  highlightAfterEdit,
  mergeSlotProps,
  mergeStyles,
  popupStyle,
  portalProps,
  rankMatches,
  requestControlledChange,
  resolveStyle,
  selectionList,
  stepHighlight,
  stepIndex,
  toggleSelection,
  trapTab,
} from "../src/index.js";
import { installFakeTime } from "./helpers.js";

const key = (name, over = {}) => {
  const event = { key: name, held: false, defaultPrevented: false, ...over };
  event.preventDefault = () => {
    event.defaultPrevented = true;
  };
  return event;
};

test("styles: shallow merge with the later style winning, and state functions", () => {
  assert.deepEqual(mergeStyles({ a: 1, b: 1 }, undefined, { b: 2 }, null, { c: 3 }), { a: 1, b: 2, c: 3 });
  assert.deepEqual(resolveStyle((state) => ({ opacity: state.disabled ? 0.5 : 1 }), { disabled: true }), { opacity: 0.5 });
  assert.deepEqual(resolveStyle({ width: 1 }, {}), { width: 1 });
  assert.equal(resolveStyle(undefined, {}), undefined);
});

test("CMP-002 A12: controlled-state requests", () => {
  const calls = [];
  const internal = [];
  const request = (controlled, current, next) =>
    requestControlledChange({ controlled, current, next, onChange: (v) => calls.push(v), setInternal: (v) => internal.push(v) });
  assert.equal(request(false, 1, 1), false, "an identical request does nothing");
  assert.deepEqual([calls, internal], [[], []]);
  assert.equal(request(false, 1, 2), true);
  assert.deepEqual([calls, internal], [[2], [2]], "uncontrolled: the callback runs and the internal state follows");
  assert.equal(request(true, 1, 2), true);
  assert.deepEqual([calls, internal], [[2, 2], [2]], "controlled: the callback runs and nothing is overwritten");
});

test("handler composition runs both handlers, the first one first, even when the first prevents the default", () => {
  const log = [];
  const first = (event) => {
    log.push("first");
    event.preventDefault();
  };
  const second = () => log.push("second");
  composeHandlers(first, second)(key("x"));
  assert.deepEqual(log, ["first", "second"]);
  assert.equal(composeHandlers(undefined, second), second);
  assert.equal(composeHandlers(first, undefined), first);
});

test("refs are composed to callbacks and objects", () => {
  const seen = [];
  const object = { current: null };
  composeRefs((value) => seen.push(value), undefined, object)("node");
  assert.deepEqual([seen, object.current], [["node"], "node"]);
  assert.equal(composeRefs(undefined, null), undefined);
});

test("CMP-001 R6: slot props merge child before part, tab index precedence and style order", () => {
  const log = [];
  const child = { tabIndex: 5, role: "link", style: { color: "red", width: 1 }, onClick: () => log.push("child"), testId: "c" };
  const part = { role: "button", onClick: () => log.push("part"), style: { width: 2 } };
  const merged = mergeSlotProps(child, part, { defaults: { tabIndex: 0 }, mergeStyle: mergeStyles });
  merged.onClick();
  assert.deepEqual(log, ["child", "part"]);
  assert.equal(merged.tabIndex, 5, "the child's tab index beats the part default");
  assert.equal(merged.role, "button", "other part properties win");
  assert.equal(merged.testId, "c");
  assert.deepEqual(merged.style, { color: "red", width: 2 });
  assert.equal(mergeSlotProps(child, { tabIndex: -1 }, { defaults: { tabIndex: 0 } }).tabIndex, -1, "an explicit part tab index wins");
  assert.equal(mergeSlotProps({}, {}, { defaults: { tabIndex: 0 } }).tabIndex, 0);
  assert.deepEqual(mergeSlotProps(child, part).style, { width: 2 }, "without a merge function the part style replaces");
});

test("CMP-001 R1, R2, A3: button tab index and disabled activation", () => {
  const log = [];
  const enabled = buttonProps({ onClick: () => log.push("click") });
  assert.deepEqual([enabled.role, enabled.tabIndex], ["button", 0]);
  const disabled = buttonProps({ disabled: true, onClick: () => log.push("disabled") });
  assert.equal(disabled.tabIndex, -1);
  assert.equal(buttonProps({ disabled: true, focusableWhenDisabled: true }).tabIndex, 0);
  assert.equal(buttonProps({ disabled: true, tabIndex: 3 }).tabIndex, 3);
  disabled.onClick({ button: 0 });
  disabled.onKeyDown(key("enter"));
  disabled.onKeyDown(key("space"));
  disabled.onKeyUp(key("space"));
  assert.deepEqual(log, []);
  const focusable = buttonProps({ disabled: true, focusableWhenDisabled: true, onClick: () => log.push("still disabled") });
  focusable.onClick({ button: 0 });
  focusable.onKeyDown(key("enter"));
  assert.deepEqual(log, []);
  enabled.onClick({ button: 0 });
  enabled.onClick({ button: 2 });
  assert.deepEqual(log, ["click"], "only the primary button activates");
});

test("CMP-001 R2: Enter activates once per physical press, Space on key-up, caller handlers first", () => {
  const log = [];
  const latch = createButtonLatch();
  const props = () =>
    buttonProps({
      latch,
      onClick: () => log.push("activate"),
      onKeyDown: (event) => log.push(`caller down ${event.key}`),
      onKeyUp: (event) => log.push(`caller up ${event.key}`),
    });
  const first = key("enter");
  props().onKeyDown(first);
  assert.equal(first.defaultPrevented, true, "the key-down default is consumed");
  props().onKeyDown(key("enter", { held: true }));
  props().onKeyDown(key("enter")); // a repeat the platform did not flag as held
  props().onKeyUp(key("enter"));
  assert.deepEqual(log, ["caller down enter", "activate", "caller down enter", "caller down enter", "caller up enter"]);
  log.length = 0;
  props().onKeyDown(key("enter"));
  assert.deepEqual(log, ["caller down enter", "activate"], "the key-up released the latch");
  log.length = 0;
  const down = key("space");
  props().onKeyDown(down);
  assert.deepEqual([down.defaultPrevented, log], [true, ["caller down space"]], "space never activates on key-down");
  props().onKeyUp(key("space"));
  assert.deepEqual(log, ["caller down space", "caller up space", "activate"]);
});

test("CMP-001 R2: a prevented key event and modified keys do not activate", () => {
  const log = [];
  const props = buttonProps({ latch: createButtonLatch(), onClick: () => log.push("activate"), onKeyDown: (event) => event.preventDefault() });
  props.onKeyDown(key("enter"));
  buttonProps({ onClick: () => log.push("ctrl") }).onKeyDown(key("enter", { ctrl: true }));
  buttonProps({ onClick: () => log.push("up") }).onKeyUp(key("space", { defaultPrevented: true }));
  assert.deepEqual(log, []);
});

test("CMP-002 R22: key classification", () => {
  assert.equal(classifyKey({ key: "down" }), "down");
  assert.equal(classifyKey({ key: "n", ctrl: true }), "down");
  assert.equal(classifyKey({ key: "p", ctrl: true }), "up");
  assert.equal(classifyKey({ key: "n", ctrl: true }, { chords: false }), "other");
  assert.equal(classifyKey({ key: "enter" }), "enter");
  assert.equal(classifyKey({ key: "enter", ctrl: true }), "secondary-accept");
  assert.equal(classifyKey({ key: "home" }), "home");
  assert.equal(classifyKey({ key: "escape" }), "escape");
  assert.equal(classifyKey({ key: "a" }), "other");
});

test("CMP-002 R23 / A9: stepping wraps, enters, skips disabled rows and handles empty lists", () => {
  const rows = [true, false, true, true];
  assert.equal(stepIndex(rows, 0, 1), 2, "a disabled row is skipped");
  assert.equal(stepIndex(rows, 3, 1), 0, "forward wraps");
  assert.equal(stepIndex(rows, 0, -1), 3, "backward wraps");
  assert.equal(stepIndex(rows, 2, -1), 0);
  assert.equal(stepIndex(rows, null, 1), 0, "forward from none enters at the first");
  assert.equal(stepIndex(rows, null, -1), 3, "backward from none enters at the last");
  assert.equal(stepIndex([], null, 1), null);
  assert.equal(stepIndex([false, false], 0, 1), null, "all disabled");
  assert.equal(edgeIndex(rows, "first"), 0);
  assert.equal(edgeIndex([false, true, true, false], "last"), 2);
  assert.equal(edgeIndex([false], "first"), null);
});

test("CMP-002 R25 / A9: search ranks prefix before substring and keeps the order stable", () => {
  const labels = ["Banana", "Cabin", "Abacus", "bandana", "nothing", "Cab"];
  assert.deepEqual(rankMatches(labels, "  BAN "), [0, 3], "trimmed and case-insensitive");
  assert.deepEqual(rankMatches(labels, "ab"), [2, 1, 5], "a prefix first, then substrings in input order");
  assert.deepEqual(rankMatches(labels, ""), [0, 1, 2, 3, 4, 5], "an empty query keeps the input order");
  assert.deepEqual(rankMatches(labels, "zzz"), []);
});

test("CMP-002 R13: the press flag lasts a microtask and defers outside presses in either order", async () => {
  const flag = createPressFlag();
  const log = [];
  flag.raise();
  assert.equal(flag.consumes(), true);
  flag.defer(() => log.push("outside after press"));
  await Promise.resolve();
  assert.equal(flag.consumes(), false);
  assert.deepEqual(log, [], "an outside press delivered after the trigger press is ignored");

  flag.defer(() => log.push("outside before press"));
  flag.raise();
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(log, [], "an outside press delivered before the trigger press is ignored too");

  flag.defer(() => log.push("plain outside press"));
  await Promise.resolve();
  assert.deepEqual(log, ["plain outside press"]);
});

function tooltip({ settings = {}, overrides = {}, replaceTimers = true } = {}) {
  const time = installFakeTime();
  const start = time.now;
  const provider = createTooltipProvider(settings);
  const log = [];
  const root = createTooltipRoot({
    provider,
    overrides: () => overrides,
    replaceTimers,
    requestOpen: (open) => log.push([time.now - start, open]),
  });
  return { time, provider, root, log };
}

function withTooltip(options, body) {
  const t = tooltip(options);
  try {
    body(t);
  } finally {
    t.time.restore();
  }
}

test("CMP-004 A2(a): show delay 700 and skip window 300", () => {
  withTooltip({ settings: { delayDuration: 700, skipDelayDuration: 300, disableHoverableContent: true } }, (t) => {
    t.root.triggerEnter();
    t.time.advance(699);
    assert.deepEqual(t.log, []);
    t.time.advance(1);
    assert.deepEqual(t.log, [[700, true]], "opens after the delay");
    t.root.triggerLeave();
    assert.deepEqual(t.log.at(-1), [700, false], "a non-hoverable tooltip closes at once");
    t.time.advance(250);
    t.root.triggerEnter();
    assert.deepEqual(t.log.at(-1), [950, true], "re-entering 250 ms after the close opens immediately");
    t.root.triggerLeave();
    t.time.advance(400);
    t.root.triggerEnter();
    assert.equal(t.log.length, 4, "re-entering 400 ms after the close waits");
    t.time.advance(699);
    assert.equal(t.log.length, 4);
    t.time.advance(1);
    assert.deepEqual(t.log.at(-1), [2050, true]);
  });
});

test("CMP-004 R1: the skip comparison is inclusive", () => {
  withTooltip({ settings: { delayDuration: 700, skipDelayDuration: 300 } }, (t) => {
    t.root.setOpen(false);
    t.time.advance(300);
    t.root.triggerEnter();
    assert.deepEqual(t.log.at(-1), [300, true], "at exactly the skip window the delay is zero");
  });
  withTooltip({ settings: { delayDuration: 700, skipDelayDuration: 300 } }, (t) => {
    t.root.setOpen(false);
    t.time.advance(301);
    t.root.triggerEnter();
    assert.equal(t.log.length, 1, "one millisecond later it waits");
  });
});

test("CMP-004 R5: a root's own delay overrides the provider and the default delay opens synchronously", () => {
  withTooltip({ settings: { delayDuration: 700 }, overrides: { delayDuration: 100 } }, (t) => {
    t.root.triggerEnter();
    t.time.advance(100);
    assert.deepEqual(t.log, [[100, true]]);
  });
  withTooltip({}, (t) => {
    t.root.triggerEnter();
    assert.deepEqual(t.log, [[0, true]], "a delay of zero opens immediately");
  });
});

test("CMP-004 A2(b): hoverable content keeps the tooltip open for 80 ms; disableHoverableContent closes at once", () => {
  withTooltip({}, (t) => {
    t.root.triggerEnter();
    t.root.triggerLeave();
    t.time.advance(79);
    assert.deepEqual(t.log, [[0, true]], "still open inside the grace");
    t.time.advance(1);
    assert.deepEqual(t.log.at(-1), [80, false]);
  });
  withTooltip({}, (t) => {
    t.root.triggerEnter();
    t.root.triggerLeave();
    t.time.advance(40);
    t.root.contentEnter();
    t.time.advance(200);
    assert.deepEqual(t.log, [[0, true]], "entering the content within the grace keeps it open");
    t.root.contentLeave();
    t.time.advance(80);
    assert.deepEqual(t.log.at(-1), [320, false], "leaving the content uses the same policy");
  });
  withTooltip({ overrides: { disableHoverableContent: true } }, (t) => {
    t.root.triggerEnter();
    t.root.triggerLeave();
    assert.deepEqual(t.log.at(-1), [0, false]);
    t.root.contentEnter();
    assert.equal(t.root.hasPendingClose, false);
  });
  withTooltip({ settings: { disableHoverableContent: true }, overrides: { disableHoverableContent: false } }, (t) => {
    t.root.triggerEnter();
    t.root.triggerLeave();
    assert.equal(t.log.length, 1, "a root override of hoverability beats the provider");
    assert.equal(t.root.hasPendingClose, true);
  });
});

test("CMP-004 A2(c): focus opens, blur, pointer-down and click close and update the last close time", () => {
  withTooltip({ settings: { delayDuration: 700 } }, (t) => {
    t.root.focus();
    assert.deepEqual(t.log.at(-1), [0, true]);
    t.root.blur();
    assert.deepEqual(t.log.at(-1), [0, false]);
    t.time.advance(1000);
    t.root.pointerDown(); // the tooltip was already closed
    t.time.advance(100);
    t.root.triggerEnter();
    assert.deepEqual(t.log.at(-1), [1100, true], "a close request while closed still counts for the skip window");
    t.time.advance(500);
    t.root.click();
    t.time.advance(299);
    t.root.triggerEnter();
    assert.deepEqual(t.log.at(-1), [1899, true]);
  });
});

test("CMP-004 R9: every state request clears the tracked timers and dispose cancels them", () => {
  withTooltip({ settings: { delayDuration: 700 } }, (t) => {
    t.root.triggerEnter();
    assert.equal(t.root.hasPendingOpen, true);
    t.root.setOpen(true);
    assert.equal(t.root.hasPendingOpen, false);
    t.time.advance(1000);
    assert.equal(t.log.length, 1);
  });
  withTooltip({ settings: { delayDuration: 700 } }, (t) => {
    t.root.triggerEnter();
    t.root.dispose();
    assert.equal(t.time.pendingTimers, 0);
    t.time.advance(1000);
    assert.deepEqual(t.log, [], "unmount cancels the pending open");
  });
});

test("CMP-004 A3 (core): replacing timers leaves one open timer; overwriting leaves the earlier one running", () => {
  withTooltip({ settings: { delayDuration: 700 }, replaceTimers: true }, (t) => {
    t.root.triggerEnter();
    t.time.advance(100);
    t.root.triggerEnter();
    assert.equal(t.time.pendingTimers, 1);
    t.time.advance(700);
    assert.equal(t.log.length, 1);
  });
  withTooltip({ settings: { delayDuration: 700 }, replaceTimers: false }, (t) => {
    t.root.triggerEnter();
    t.time.advance(100);
    t.root.triggerEnter();
    assert.equal(t.time.pendingTimers, 2, "the earlier handle was overwritten, not cancelled");
    t.time.advance(600);
    assert.deepEqual(t.log, [[700, true]], "the earlier timer still fired");
  });
});

test("CMP-003 R3, R4, R5: portal, backdrop and popup defaults", () => {
  const modal = portalProps();
  assert.deepEqual([modal.fill, modal.deferred, modal.priority, modal.occlude], ["window", true, 0, true]);
  assert.deepEqual(modal.style, { display: "flex", alignItems: "center", justifyContent: "center", backgroundColor: "transparent" });
  const nonmodal = portalProps({ modal: false });
  assert.equal(nonmodal.occlude, false);
  assert.equal(nonmodal.style.pointerEvents, "none");
  assert.deepEqual(backdropStyle(), { position: "absolute", top: 0, left: 0, right: 0, bottom: 0 });
  assert.equal(backdropStyle({ modal: false }).pointerEvents, "none");
  assert.deepEqual(popupStyle(), { pointerEvents: "auto", keyboardFocusDim: false });
});

test("CMP-003 R7: the Tab trap moves within the popup, Shift reverses, and nothing else is handled", () => {
  const moves = [];
  const popup = { focusNextWithin: () => moves.push("next"), focusPreviousWithin: () => moves.push("previous") };
  assert.equal(trapTab(key("tab"), popup), true);
  assert.equal(trapTab(key("tab", { shift: true }), popup), true);
  assert.deepEqual(moves, ["next", "previous"]);
  const prevented = key("tab");
  prevented.preventDefault();
  assert.equal(trapTab(prevented, popup), false, "a prevented key is left alone");
  assert.equal(trapTab(key("tab", { ctrl: true }), popup), false);
  assert.equal(trapTab(key("a"), popup), false);
  const handled = key("tab");
  trapTab(handled, popup);
  assert.equal(handled.defaultPrevented, true, "the default is prevented");
  assert.deepEqual(moves, ["next", "previous", "next"]);
});

test("CMP-003 R9, R10: dismissal policy", () => {
  const policy = (over = {}) => ({ modal: true, disablePointerDismissal: false, disableEscapeDismissal: false, ...over });
  assert.equal(dismissalCloses("escape", policy()), true);
  assert.equal(dismissalCloses("escape", policy({ disableEscapeDismissal: true })), false);
  assert.equal(dismissalCloses("outside", policy()), false, "a modal dialog is dismissed through its backdrop");
  assert.equal(dismissalCloses("outside", policy({ modal: false })), true);
  assert.equal(dismissalCloses("outside", policy({ modal: false, disablePointerDismissal: true })), false);
  assert.equal(dismissalCloses("outside", policy({ modal: false }), { top: false }), false, "a nested layer owns the press");
});

test("CMP-006 R2: default filtering ranks prefix matches in React and keeps the order in Solid", () => {
  const items = ["xab", "ab", "zab", "abz", "qq"];
  assert.deepEqual(filterItems(items, " AB ", { flavor: "react" }), ["ab", "abz", "xab", "zab"]);
  assert.deepEqual(filterItems(items, " AB ", { flavor: "solid" }), ["xab", "ab", "zab", "abz"]);
  assert.deepEqual(filterItems(items, "  ", { flavor: "react" }), items);
  assert.deepEqual(filterItems(items, "  ", { flavor: "solid" }), items);
});

test("CMP-006 R2: a null filter keeps everything and custom filters get the flavor's query", () => {
  const items = ["a", "b"];
  assert.deepEqual(filterItems(items, "zzz", { flavor: "react", filter: null }), items);
  const seen = [];
  const filter = (item, query, toString) => {
    seen.push([item, query, toString(item)]);
    return item === "b";
  };
  assert.deepEqual(filterItems(items, " Q ", { flavor: "react", filter }), ["b"]);
  assert.deepEqual(seen[0], ["a", " Q ", "a"], "React passes the raw query");
  seen.length = 0;
  filterItems(items, " Q ", { flavor: "solid", filter });
  assert.deepEqual(seen[0], ["a", "q", "a"], "Solid passes the trimmed lowercase query");
  assert.deepEqual(filterItems([{ n: "Kiwi" }], "ki", { flavor: "solid", itemToString: (item) => item.n }), [{ n: "Kiwi" }]);
});

test("CMP-006 R6: selection toggling keeps the array order and the display differs by flavor", () => {
  assert.deepEqual(selectionList(null), []);
  assert.deepEqual(selectionList("a"), ["a"]);
  assert.deepEqual(toggleSelection(null, "a"), ["a"]);
  assert.deepEqual(toggleSelection(["b", "a"], "c"), ["b", "a", "c"]);
  assert.deepEqual(toggleSelection(["b", "a", "c"], "a"), ["b", "c"]);
  assert.equal(displayText(null, { flavor: "react" }), null);
  assert.equal(displayText("", { flavor: "react" }), null);
  assert.equal(displayText([], { flavor: "react" }), null);
  assert.equal(displayText(["a", "b"], { flavor: "react", itemToString: (v) => v.toUpperCase() }), "A, B");
  assert.equal(displayText(null, { flavor: "solid" }), null);
  assert.equal(displayText("", { flavor: "solid" }), "");
  assert.equal(displayText([], { flavor: "solid" }), "");
  assert.equal(displayText(["a", "b"], { flavor: "solid", itemToString: (v) => v.toUpperCase() }), "a, b");
});

test("CMP-006 R4, R5: the highlight after an edit and stepping with wraparound over disabled entries", () => {
  const disabled = new Set(["b"]);
  const options = { items: ["a", "b", "c"], query: "", flavor: "react", disabled };
  assert.equal(highlightAfterEdit({ ...options, autoHighlight: false }), null);
  assert.equal(highlightAfterEdit({ ...options, autoHighlight: true }), "a");
  assert.equal(highlightAfterEdit({ ...options, autoHighlight: "always" }), "a");
  assert.equal(highlightAfterEdit({ ...options, autoHighlight: true, query: "b" }), null, "only a disabled match");
  const strings = ["a", "b", "c"];
  assert.equal(stepHighlight(strings, disabled, null, 1), "a");
  assert.equal(stepHighlight(strings, disabled, null, -1), "c");
  assert.equal(stepHighlight(strings, disabled, "a", 1), "c", "the disabled entry is skipped");
  assert.equal(stepHighlight(strings, disabled, "c", 1), "a", "wraparound");
  assert.equal(stepHighlight(strings, disabled, "gone", 1), "a", "a highlight outside the list starts over");
  assert.equal(stepHighlight([], disabled, null, 1), null);
  assert.equal(stepHighlight(strings, new Set(strings), "a", 1), "a", "nothing available keeps the highlight");
});
