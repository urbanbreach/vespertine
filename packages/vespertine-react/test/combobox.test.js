// The headless Combobox for React (CMP-006 R1-R8, acceptance A1-A6) over the JavaScript reference
// tree. The native-host halves (a press inside the popup never blurs the input, blur by tabbing
// closes) live in combobox-native.test.js.

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { Combobox, DismissableLayer } from "../src/index.js";
import { h, microtasks, mount, textOf } from "./components-helpers.js";

const FRUIT = ["Apple", "Apricot", "Banana", "Pineapple", "Cherry"];
const mark = ({ selected, highlighted, disabled }) => ({ color: `${highlighted ? "H" : "-"}${selected ? "S" : "-"}${disabled ? "D" : "-"}` });

function comboApp({ root = {}, input = {}, content = {}, trigger = {}, value = {}, items = FRUIT, disabled = ["Banana"], render } = {}) {
  return h(
    Combobox.Root,
    { testId: "root", items, ...root },
    h(Combobox.Input, { testId: "input", ...input }),
    h(Combobox.Trigger, { testId: "trigger", ...trigger }, h("text", {}, "v")),
    h(Combobox.Value, { testId: "value", placeholder: "None", ...value }),
    h(
      Combobox.Content,
      { testId: "content", ...content },
      h(
        Combobox.List,
        { testId: "list" },
        render ??
          ((item) =>
            h(Combobox.Item, { testId: `item-${item}`, value: item, disabled: disabled.includes(item), style: mark }, h("text", {}, item))),
      ),
      h(Combobox.Empty, { testId: "empty" }, h("text", {}, "No results")),
    ),
  );
}

const shown = (ui, items = FRUIT) => items.filter((item) => ui.has(`item-${item}`));
const marks = (ui, items = FRUIT) => Object.fromEntries(items.filter((item) => ui.has(`item-${item}`)).map((item) => [item, ui.node(`item-${item}`).style.color]));
const highlighted = (ui) => Object.entries(marks(ui)).filter(([, flags]) => flags[0] === "H").map(([item]) => item);
const type = (ui, value) => ui.emit("input", { type: "change", value });
const click = (ui, testId) => ui.emit(testId, { type: "click", button: 0 });
const down = (ui, key) => ui.keyDown("input", key);

test("CMP-006 R1: defaults are no items shown, a null value, empty input text, closed, single, enabled", () => {
  const ui = mount(h(Combobox.Root, { testId: "root" }, h(Combobox.Input, { testId: "input" }), h(Combobox.Value, { testId: "value", placeholder: "None" }), h(Combobox.Content, { testId: "content" }, h(Combobox.Empty, { testId: "empty" }, "none"))));
  assert.equal(ui.node("input").props.value, "");
  assert.equal(ui.node("input").props.readOnly, false);
  assert.equal(ui.has("content"), false, "closed");
  assert.equal(textOf(ui.node("value")), "None", "a null value shows the placeholder");
  assert.equal(ui.node("root").style.display, "flex", "floating root defaults");
  assert.equal(ui.node("root").style.position, "relative");
  assert.equal(ui.node("root").style.alignItems, "flex-start");
  type(ui, "x");
  assert.equal(ui.has("empty"), true, "with no items the Empty part mounts once open");
});

test("CMP-006 R1: value, input text and open state are independently controlled, with defaults and change callbacks", () => {
  const log = [];
  const ui = mount(
    comboApp({
      root: {
        value: "Apple",
        onValueChange: (next) => log.push(["value", next]),
        defaultInputValue: "ap",
        onInputValueChange: (next) => log.push(["input", next]),
        open: true,
        onOpenChange: (next) => log.push(["open", next]),
      },
    }),
  );
  assert.equal(ui.node("input").props.value, "ap", "the default input text initializes the uncontrolled input");
  assert.deepEqual(shown(ui), ["Apple", "Apricot", "Pineapple"], "the default input text filters");
  assert.equal(textOf(ui.node("value")), "Apple", "the controlled value is displayed");
  click(ui, "item-Apricot");
  assert.deepEqual(log, [["value", "Apricot"], ["input", "Apricot"], ["open", false]]);
  assert.equal(textOf(ui.node("value")), "Apple", "a controlled value is never overwritten");
  assert.equal(ui.node("input").props.value, "Apricot", "the input text is independent: uncontrolled, it follows");
  assert.ok(ui.has("content"), "a controlled open state stays with the owner");
});

test("CMP-006 R1: string conversion defaults to identity and itemToStringValue converts items", () => {
  const ui = mount(
    comboApp({
      items: [{ id: 1, name: "Alpha" }, { id: 2, name: "Beta" }],
      root: { itemToStringValue: (item) => item.name, open: true },
      render: (item) => h(Combobox.Item, { key: item.id, testId: `item-${item.name}`, value: item.name }, h("text", {}, item.name)),
    }),
  );
  type(ui, "be");
  assert.ok(ui.has("item-Beta"));
  assert.equal(ui.has("item-Alpha"), false, "the filter reads the converted string");
});

test("CMP-006 A3: React ranks prefix matches before other substring matches, preserves order inside a rank, trims and lowercases", () => {
  const ui = mount(comboApp({ items: ["xab", "ab", "zab", "abz", "qq", "abc"], disabled: [], root: { open: true } }));
  const order = () => ui.node("list").children.map((child) => child.props.testId.slice(5));
  type(ui, "  AB ");
  assert.deepEqual(order(), ["ab", "abz", "abc", "xab", "zab"], "prefix matches first, original order within each rank");
  type(ui, "");
  assert.deepEqual(order(), ["xab", "ab", "zab", "abz", "qq", "abc"], "an empty query keeps every item in order");
});

test("CMP-006 A3: a null filter shows every item and custom filters receive the item, the raw query and the string callback", () => {
  const unfiltered = mount(comboApp({ root: { filter: null, open: true } }));
  type(unfiltered, "zzz");
  assert.deepEqual(shown(unfiltered), FRUIT);
  assert.equal(unfiltered.has("empty"), false);

  const seen = [];
  const custom = mount(
    comboApp({
      root: {
        open: true,
        filter: (item, query, toString) => {
          seen.push([item, query, toString(item)]);
          return item.length > 5;
        },
      },
    }),
  );
  type(custom, "  Mixed Case ");
  assert.deepEqual(shown(custom), ["Apricot", "Banana", "Pineapple", "Cherry"]);
  assert.deepEqual(seen.at(-1), ["Cherry", "  Mixed Case ", "Cherry"], "React passes the raw query");
});

test("CMP-006 R3: the input is read-only while the root is disabled or when the caller says so", () => {
  assert.equal(mount(comboApp({ root: { disabled: true } })).node("input").props.readOnly, true);
  assert.equal(mount(comboApp({ input: { readOnly: true } })).node("input").props.readOnly, true);
  assert.equal(mount(comboApp()).node("input").props.readOnly, false);
});

test("CMP-006 R3: click and focus request opening, blur closes, opening takes focus in a microtask", async () => {
  const log = [];
  const ui = mount(comboApp({ root: { onOpenChange: (open) => log.push(open) }, input: { onClick: () => log.push("caller click") } }));
  ui.emit("input", { type: "click", button: 0 });
  assert.deepEqual(log, ["caller click", true], "the caller's handler runs first");
  assert.ok(ui.has("content"));
  assert.notEqual(ui.focused(), ui.node("input").id, "focus is scheduled, not immediate");
  await microtasks();
  assert.equal(ui.focused(), ui.node("input").id, "the input takes focus in a microtask");
  ui.emit("input", { type: "blur" });
  assert.equal(ui.has("content"), false, "blur closes");
  ui.emit("input", { type: "focus" });
  assert.ok(ui.has("content"), "focus requests opening");
  const disabled = mount(comboApp({ root: { disabled: true } }));
  disabled.emit("input", { type: "click", button: 0 });
  disabled.emit("input", { type: "focus" });
  assert.equal(disabled.has("content"), false, "a disabled combobox never opens from the input");
});

test("CMP-006 R3: a change runs the caller's handler first, filters, requests opening and falls back to an empty string", () => {
  const log = [];
  const ui = mount(comboApp({ root: { onInputValueChange: (value) => log.push(["input", value]), onOpenChange: (open) => log.push(["open", open]) }, input: { onChange: (event) => log.push(["caller", event.value]) } }));
  type(ui, "ap");
  assert.deepEqual(log, [["caller", "ap"], ["input", "ap"], ["open", true]]);
  assert.deepEqual(shown(ui), ["Apple", "Apricot", "Pineapple"]);
  ui.emit("input", { type: "change" });
  assert.equal(ui.node("input").props.value, "", "an event without a value means an empty string");
  assert.deepEqual(shown(ui), FRUIT);
});

test("CMP-006 A4: autoHighlight false clears the highlight on edits; true and always highlight the first enabled filtered item", () => {
  for (const [autoHighlight, expected] of [[false, []], [true, ["Apple"]], ["always", ["Apple"]]]) {
    const ui = mount(comboApp({ root: { autoHighlight, open: true } }));
    down(ui, "down");
    down(ui, "down");
    assert.deepEqual(highlighted(ui), ["Apricot"], `${autoHighlight}: stepping highlights`);
    type(ui, "a");
    assert.deepEqual(highlighted(ui), expected, `autoHighlight ${autoHighlight}`);
  }
  const skipped = mount(comboApp({ root: { autoHighlight: true, open: true } }));
  type(skipped, "ban");
  assert.deepEqual(highlighted(skipped), [], "the only match is disabled, so nothing is highlighted");
  type(skipped, "pi");
  assert.deepEqual(highlighted(skipped), ["Pineapple"], "the first enabled item of the new filtered list");
});

test("CMP-006 R5: Down and Up wrap and skip disabled items; Ctrl-N and Ctrl-P also step in React", () => {
  const ui = mount(comboApp({ root: { open: true } }));
  down(ui, "down");
  assert.deepEqual(highlighted(ui), ["Apple"], "with no highlight forward starts at the first");
  down(ui, "down");
  down(ui, "down");
  assert.deepEqual(highlighted(ui), ["Pineapple"], "Banana is disabled and skipped");
  down(ui, "down");
  down(ui, "down");
  assert.deepEqual(highlighted(ui), ["Apple"], "forward wraps from the last to the first");
  down(ui, "up");
  assert.deepEqual(highlighted(ui), ["Cherry"], "reverse wraps to the last");
  ui.keyDown("input", "n", { ctrl: true });
  assert.deepEqual(highlighted(ui), ["Apple"], "Ctrl-N is Down");
  ui.keyDown("input", "p", { ctrl: true });
  assert.deepEqual(highlighted(ui), ["Cherry"], "Ctrl-P is Up");
  const fresh = mount(comboApp({ root: { open: true } }));
  down(fresh, "up");
  assert.deepEqual(highlighted(fresh), ["Cherry"], "with no highlight reverse starts at the last");
});

test("CMP-006 A1: typed filtering, submission, disabled-item skipping and the empty state", () => {
  const changes = [];
  const ui = mount(comboApp({ root: { onValueChange: (value) => changes.push(value) } }));
  type(ui, "a");
  assert.ok(ui.has("content"), "typing opens the popup");
  assert.deepEqual(shown(ui), ["Apple", "Apricot", "Banana", "Pineapple"]);
  down(ui, "down");
  down(ui, "down");
  down(ui, "down");
  assert.deepEqual(highlighted(ui), ["Pineapple"], "Banana is skipped");
  ui.emit("input", { type: "submit" });
  assert.deepEqual(changes, ["Pineapple"], "submit selects the highlighted filtered item");
  assert.equal(ui.node("input").props.value, "Pineapple", "single selection fills the input with the label");
  assert.equal(ui.has("content"), false, "and closes the popup");
  assert.equal(textOf(ui.node("value")), "Pineapple");
  assert.deepEqual(highlighted(ui), [], "React clears the highlight after selecting");
  type(ui, "zzz");
  assert.ok(ui.has("empty"), "the empty state mounts when nothing passes the filter");
  assert.deepEqual(shown(ui), []);
  type(ui, "ch");
  assert.equal(ui.has("empty"), false);
  ui.emit("input", { type: "submit" });
  assert.deepEqual(changes, ["Pineapple"], "submit without a highlight does nothing");
});

test("CMP-006 A1: a filled asChild item takes the click and hover of the part", () => {
  const changes = [];
  const ui = mount(
    comboApp({
      root: { open: true, onValueChange: (value) => changes.push(value) },
      render: (item) => h(Combobox.Item, { key: item, value: item, asChild: true }, h("div", { testId: `item-${item}`, style: { backgroundColor: "#ff0000" } })),
    }),
  );
  assert.equal(ui.node("item-Apple").props.role, "option");
  ui.emit("item-Cherry", { type: "click", button: 0 });
  assert.deepEqual(changes, ["Cherry"]);
});

test("CMP-006 R4: editing sets the highlight only through the autoHighlight option, never on open", () => {
  const ui = mount(comboApp({ root: { autoHighlight: true } }));
  click(ui, "trigger");
  assert.ok(ui.has("content"));
  assert.deepEqual(highlighted(ui), [], "opening alone highlights nothing");
});

test("CMP-006 R5: submit selects only a highlighted item that is in the filtered list", () => {
  const changes = [];
  const ui = mount(comboApp({ root: { open: true, onValueChange: (value) => changes.push(value) } }));
  down(ui, "down");
  type(ui, "ch");
  assert.deepEqual(highlighted(ui), [], "the edit cleared the highlight");
  ui.emit("input", { type: "submit" });
  assert.deepEqual(changes, []);
  down(ui, "down");
  ui.emit("input", { type: "submit" });
  assert.deepEqual(changes, ["Cherry"]);
});

test("CMP-006 A4: with the root disabled React blocks navigation and submit", () => {
  const changes = [];
  const ui = mount(comboApp({ root: { open: true, disabled: true, onValueChange: (value) => changes.push(value) } }));
  down(ui, "down");
  assert.deepEqual(highlighted(ui), [], "navigation is blocked");
  ui.emit("item-Apple", { type: "enter" });
  ui.emit("input", { type: "submit" });
  ui.emit("item-Cherry", { type: "click", button: 0 });
  assert.deepEqual(changes, [], "selection is blocked");
});

test("CMP-006 R6: single selection sets the string, fills the input with it, closes and clears the highlight", () => {
  const log = [];
  const ui = mount(comboApp({ root: { defaultOpen: true, onValueChange: (value) => log.push(["value", value]), onInputValueChange: (value) => log.push(["input", value]), onOpenChange: (open) => log.push(["open", open]) } }));
  click(ui, "item-Cherry");
  assert.deepEqual(log, [["value", "Cherry"], ["input", "Cherry"], ["open", false]]);
  assert.equal(ui.has("content"), false);
  ui.emit("input", { type: "click", button: 0 });
  assert.deepEqual(shown(ui), ["Cherry"], "the filter follows the filled input");
  assert.equal(ui.node("item-Cherry").style.color, "-S-", "the selected item reports selected");
});

test("CMP-006 A2: multiple selection keeps the popup open, toggles membership in array order and clears the input", () => {
  const changes = [];
  const ui = mount(comboApp({ root: { multiple: true, onValueChange: (value) => changes.push(value) } }));
  type(ui, "a");
  click(ui, "item-Pineapple");
  assert.deepEqual(changes, [["Pineapple"]]);
  assert.ok(ui.has("content"), "the popup stays open");
  assert.equal(ui.node("input").props.value, "", "the input is cleared");
  assert.deepEqual(shown(ui), FRUIT, "so every item shows again");
  click(ui, "item-Apple");
  click(ui, "item-Cherry");
  assert.deepEqual(changes.at(-1), ["Pineapple", "Apple", "Cherry"], "membership is appended in order");
  click(ui, "item-Apple");
  assert.deepEqual(changes.at(-1), ["Pineapple", "Cherry"], "toggling removes it and keeps the rest in order");
  assert.equal(ui.node("item-Pineapple").style.color, "-S-");
  assert.equal(ui.node("item-Apple").style.color, "---");
  assert.equal(textOf(ui.node("value")), "Pineapple, Cherry");
  assert.deepEqual(highlighted(ui), [], "React clears the highlight");
});

test("CMP-006 A5: React joins converted labels with a comma and space and shows the placeholder for null or an empty selection", () => {
  const show = (root) => textOf(mount(comboApp({ root })).node("value"));
  assert.equal(show({ value: null }), "None");
  assert.equal(show({}), "None");
  assert.equal(show({ multiple: true, value: [] }), "None", "an empty array shows the placeholder");
  assert.equal(show({ value: "" }), "None", "so does an empty string");
  assert.equal(show({ value: "Apple" }), "Apple");
  assert.equal(show({ multiple: true, value: ["Apple", "Cherry"] }), "Apple, Cherry");
  assert.equal(show({ multiple: true, value: ["a", "b"], itemToStringValue: (item) => item.toUpperCase() }), "A, B", "labels pass through the string conversion");
});

test("CMP-006 A5: explicit children and render functions override the automatic display", () => {
  const explicit = mount(comboApp({ root: { value: "Apple" }, value: { children: "Custom" } }));
  assert.equal(textOf(explicit.node("value")), "Custom");
  const render = mount(comboApp({ root: { multiple: true, value: ["Apple", "Cherry"] }, value: { children: (value) => `${value.length} chosen` } }));
  assert.equal(textOf(render.node("value")), "2 chosen");
  const empty = mount(comboApp({ value: { children: (value) => String(value) } }));
  assert.equal(textOf(empty.node("value")), "null", "the render function runs for a null value too");
});

test("CMP-006 R7: the trigger has tab index 0 (-1 when disabled), click toggles and the arrow keys request opening", () => {
  const ui = mount(comboApp());
  assert.equal(ui.node("trigger").props.tabIndex, 0);
  assert.equal(ui.node("trigger").props.role, "button");
  click(ui, "trigger");
  assert.ok(ui.has("content"));
  assert.equal(ui.node("trigger").props["aria-expanded"], true);
  click(ui, "trigger");
  assert.equal(ui.has("content"), false, "click toggles");
  ui.keyDown("trigger", "down");
  assert.ok(ui.has("content"), "Down requests opening");
  ui.emit("content", { type: "keyDown", key: "escape", keystroke: 1 });
  const up = mount(comboApp());
  up.keyDown("trigger", "up");
  assert.ok(up.has("content"), "Up requests opening");
  const disabled = mount(comboApp({ root: { disabled: true } }));
  assert.equal(disabled.node("trigger").props.tabIndex, -1);
  click(disabled, "trigger");
  disabled.keyDown("trigger", "down");
  assert.equal(disabled.has("content"), false);
  const own = mount(comboApp({ root: { disabled: true }, trigger: { disabled: false } }));
  click(own, "trigger");
  assert.ok(own.has("content"), "the trigger's own disabled flag overrides the root's");
});

test("CMP-006 R7: the content mounts only while open, never takes focus, and closes on an outside press or a top-layer Escape", async () => {
  const ui = mount(comboApp({ root: { defaultOpen: true } }));
  assert.ok(ui.has("content"));
  assert.equal(ui.node("content").props.tabIndex, undefined, "the content takes no focus");
  const focusCalls = ui.renderer.calls.filter((call) => call[0] === "focusElement");
  assert.deepEqual(focusCalls, [], "opening moved no focus into the content");
  ui.emit("content", { type: "outsideDown", x: 1, y: 1 });
  await microtasks();
  assert.equal(ui.has("content"), false, "an outside press closes it");

  const escape = mount(comboApp({ root: { defaultOpen: true } }));
  escape.windowKey("escape");
  assert.equal(escape.has("content"), false, "Escape closes the top layer");

  const layered = mount(h(DismissableLayer, { testId: "outer" }, comboApp({ root: { defaultOpen: true } })));
  layered.windowKey("escape");
  assert.equal(layered.has("content"), false, "the popup, opened above the outer layer, closed first");
  assert.ok(layered.has("outer"));
});

test("CMP-006 R7: a press on the input is no outside press for the popup", async () => {
  const log = [];
  const ui = mount(comboApp({ root: { defaultOpen: true, onOpenChange: (open) => log.push(open) } }));
  ui.emit("content", { type: "outsideDown", x: 1, y: 1 });
  ui.emit("input", { type: "pointerDown", button: 0 });
  await microtasks();
  assert.ok(ui.has("content"), "the input's press flag keeps the outside press from closing the popup");
  assert.deepEqual(log, []);
});

test("CMP-006 R8: list render functions run for every filtered item, static children stay with the caller", () => {
  const ran = [];
  const ui = mount(comboApp({ root: { open: true }, render: (item, index) => { ran.push([item, index]); return h(Combobox.Item, { key: item, testId: `item-${item}`, value: item }, item); } }));
  assert.deepEqual(ran.slice(0, FRUIT.length), FRUIT.map((item, index) => [item, index]));
  ran.length = 0;
  type(ui, "ch");
  assert.deepEqual(ran, [["Cherry", 0]], "the function runs for the filtered items only");

  const statics = mount(
    h(
      Combobox.Root,
      { items: FRUIT, defaultOpen: true, defaultInputValue: "ch" },
      h(Combobox.Content, {}, h(Combobox.List, { testId: "list" }, h(Combobox.Item, { testId: "static-apple", value: "Apple" }, "Apple"), h(Combobox.Item, { testId: "static-cherry", value: "Cherry" }, "Cherry"))),
    ),
  );
  assert.ok(statics.has("static-apple"), "static children are not filtered away");
  assert.ok(statics.has("static-cherry"));
});

test("CMP-006 R8: Empty mounts only when nothing passes the filter", () => {
  const ui = mount(comboApp({ root: { open: true } }));
  assert.equal(ui.has("empty"), false);
  type(ui, "zzz");
  assert.equal(ui.has("empty"), true);
});

test("CMP-006 R8: item hover highlights an enabled item in the filtered list; click selects unless disabled; caller handlers run first", () => {
  const log = [];
  const ui = mount(
    comboApp({
      root: { open: true, onValueChange: (value) => log.push(`value ${value}`) },
      render: (item) =>
        h(Combobox.Item, { key: item, testId: `item-${item}`, value: item, disabled: item === "Banana", style: mark, onEnter: () => log.push(`enter ${item}`), onClick: () => log.push(`click ${item}`) }, item),
    }),
  );
  ui.emit("item-Apple", { type: "enter" });
  assert.deepEqual(highlighted(ui), ["Apple"]);
  ui.emit("item-Banana", { type: "enter" });
  assert.deepEqual(highlighted(ui), ["Apple"], "a disabled item is never highlighted");
  ui.emit("item-Banana", { type: "click", button: 0 });
  assert.equal(log.includes("value Banana"), false, "a disabled item cannot be selected");
  ui.emit("item-Cherry", { type: "click", button: 0 });
  assert.deepEqual(log.slice(-2), ["click Cherry", "value Cherry"], "the caller's handler runs first");
  assert.equal(ui.node("item-Apple") === null, true, "selecting closed the popup");
});

test("CMP-006 R8 (React): hovering a static item outside the filtered list does not highlight it", () => {
  const ui = mount(
    h(
      Combobox.Root,
      { items: FRUIT, defaultOpen: true, defaultInputValue: "ch" },
      h(Combobox.Content, {}, h(Combobox.List, {}, h(Combobox.Item, { testId: "apple", value: "Apple", style: mark }, "Apple"), h(Combobox.Item, { testId: "cherry", value: "Cherry", style: mark }, "Cherry"))),
    ),
  );
  ui.emit("apple", { type: "enter" });
  assert.equal(ui.node("apple").style.color, "---", "index -1 is not highlightable in React");
  ui.emit("cherry", { type: "enter" });
  assert.equal(ui.node("cherry").style.color, "H--");
});

test("CMP-006 R8: disabled-item tracking is removed when the item unmounts", () => {
  const changes = [];
  let showBanana = true;
  const tree = () =>
    comboApp({
      root: { open: true, onValueChange: (value) => changes.push(value) },
      render: (item) => (item === "Banana" && !showBanana ? null : h(Combobox.Item, { key: item, testId: `item-${item}`, value: item, disabled: item === "Banana", style: mark }, item)),
    });
  const ui = mount(tree());
  down(ui, "down");
  down(ui, "down");
  down(ui, "down");
  assert.deepEqual(highlighted(ui), ["Pineapple"], "while mounted the disabled Banana is skipped");
  showBanana = false;
  ui.render(tree());
  assert.equal(ui.has("item-Banana"), false);
  down(ui, "up");
  ui.emit("input", { type: "submit" });
  assert.deepEqual(changes, ["Banana"], "once the disabled item unmounted, stepping no longer skips its value");
});

test("CMP-006 R8: group, label and separator are ordinary containers", () => {
  const ui = mount(
    h(
      Combobox.Root,
      {},
      h(Combobox.Group, { testId: "group" }, h(Combobox.Label, { testId: "label" }, "Fruit"), h(Combobox.Separator, { testId: "separator" })),
    ),
  );
  for (const id of ["group", "label", "separator"]) {
    assert.equal(ui.node(id).type, "div");
    assert.equal(ui.node(id).props.role, undefined);
  }
});

test("CMP-006: a part outside its root is reported", () => {
  const ui = mount(h(Combobox.Input, {}));
  assert.match(String(ui.errors[0]), /Combobox\.Input must be used inside Combobox\.Root/);
});

test("CMP-006 R8: an item requires a string value", () => {
  const ui = mount(h(Combobox.Root, {}, h(Combobox.Item, { value: 3 })));
  assert.match(String(ui.errors[0]), /Combobox\.Item requires a string value/);
});

void React;
