// The headless Combobox on the native offscreen test host (CMP-006 A1, A6) for Solid: real hit
// testing, focus and keyboard input. Text editing itself is exercised over the reference tree in
// combobox.test.js (the native input host reports no edits to this adapter yet).

import assert from "node:assert/strict";
import { test } from "node:test";
import { Combobox } from "../src/index.js";
import { createTestHost, testHostAvailable } from "../src/testing.js";
import { c, h, settle } from "./components-helpers.js";

const available = testHostAvailable();
const native = (name, body) => test(name, { skip: available ? false : "no GPU test backend" }, body);

const find = (host, testId) => host.renderer.findByTestId(testId)[0] ?? null;
const bounds = (host, testId) => find(host, testId)?.bounds;
const center = ({ x, y, width, height }) => [x + width / 2, y + height / 2];
const focusedId = (host, testId) => host.renderer.getFocusedElementId() === find(host, testId)?.id;
const FRUIT = ["Apple", "Apricot", "Banana", "Cherry"];

async function flush(host) {
  await settle();
  host.renderer.flush();
}

async function click(host, testId) {
  host.renderer.nativeSimulateClick(...center(bounds(host, testId)));
  await flush(host);
}

async function press(host, keys) {
  host.renderer.simulateKeystrokes(keys);
  await flush(host);
}

const mark = ({ selected, highlighted, disabled }) => ({ height: 28, width: 196, color: `${highlighted ? "H" : "-"}${selected ? "S" : "-"}${disabled ? "D" : "-"}` });
const marks = (host) => Object.fromEntries(FRUIT.filter((item) => find(host, `item-${item}`)).map((item) => [item, find(host, `item-${item}`).style.color]));

function comboApp({ log = [] } = {}) {
  return h(
    "div",
    { style: { width: 400, height: 300, padding: 20 } },
    c(
      Combobox.Root,
      { items: FRUIT, onValueChange: (value) => log.push(["value", value]) },
      () =>
        h(
          "div",
          { style: { flexDirection: "row", gap: 8 } },
          c(Combobox.Input, {
            testId: "input",
            style: { width: 200, height: 30 },
            onFocus: () => log.push("input focus"),
            onBlur: () => log.push("input blur"),
          }),
          c(Combobox.Trigger, { testId: "trigger", style: { width: 30, height: 30, backgroundColor: "#303030" } }, () => h("text", {}, "v")),
        ),
      () =>
        c(
          Combobox.Content,
          { testId: "content", sideOffset: 4, style: { width: 200, flexDirection: "column", padding: 2 } },
          () => c(Combobox.Label, { testId: "heading", style: { height: 20 } }, () => h("text", { style: { color: "#fff" } }, "Fruit")),
          () =>
            c(Combobox.List, {
              testId: "list",
              children: (item) =>
                c(Combobox.Item, { testId: `item-${item}`, value: item, disabled: item === "Banana", style: mark }, () => h("text", { style: { color: "#fff" } }, item)),
            }),
        ),
    ),
  );
}

native("CMP-006 A1: a click on the input opens the popup below it and focuses the input; arrow keys step over the disabled item and Ctrl-P is not bound", async () => {
  const host = createTestHost({ width: 400, height: 300 });
  host.render(() => comboApp());
  assert.equal(find(host, "content"), null);
  await click(host, "input");
  assert.ok(find(host, "content"), "a click requests opening");
  assert.ok(focusedId(host, "input"), "the input keeps the focus");
  const input = bounds(host, "input");
  assert.ok(bounds(host, "content").y >= input.y + input.height, "the popup sits below the input");
  await press(host, "down");
  assert.deepEqual(marks(host), { Apple: "H--", Apricot: "---", Banana: "--D", Cherry: "---" }, "forward from no highlight enters at the first item");
  await press(host, "down");
  await press(host, "down");
  assert.equal(marks(host).Cherry, "H--", "the disabled Banana is skipped");
  await press(host, "down");
  assert.equal(marks(host).Apple, "H--", "wraparound");
  await press(host, "ctrl-p");
  assert.equal(marks(host).Apple, "H--", "Ctrl-P is not bound in Solid");
  await press(host, "up");
  assert.equal(marks(host).Cherry, "H--");
  assert.ok(focusedId(host, "input"), "navigation never moves focus out of the input");
  assert.deepEqual(host.errors, []);
});

native("CMP-006 A6: a press on an item selects it and the input never blurs first", async () => {
  const log = [];
  const host = createTestHost({ width: 400, height: 300 });
  host.render(() => comboApp({ log }));
  await click(host, "input");
  assert.ok(find(host, "content"));
  log.length = 0;
  await click(host, "item-Cherry");
  assert.deepEqual(log, [["value", "Cherry"]], "the item was selected without a blur in between");
  assert.equal(find(host, "content"), null, "selecting closed the popup");
  assert.ok(focusedId(host, "input"), "the input still holds the focus");
  assert.deepEqual(host.errors, []);
});

native("CMP-006 A6: a press on the popup's own surface leaves the input focused and the popup open", async () => {
  const host = createTestHost({ width: 400, height: 300 });
  host.render(() => comboApp());
  await click(host, "input");
  host.renderer.nativeSimulateClick(...center(bounds(host, "heading")));
  await flush(host);
  assert.ok(find(host, "content"), "a press inside the popup is not an outside press");
  assert.ok(focusedId(host, "input"), "the content takes no focus on press");
  assert.deepEqual(host.errors, []);
});

native("CMP-006 A6: blur by tabbing closes the popup", async () => {
  const host = createTestHost({ width: 400, height: 300 });
  host.render(() => comboApp());
  await click(host, "input");
  assert.ok(find(host, "content"));
  await press(host, "tab");
  assert.equal(find(host, "content"), null, "tabbing away blurs the input, which closes the popup");
  assert.ok(focusedId(host, "trigger"), "focus moved to the next control");
  assert.deepEqual(host.errors, []);
});

native("CMP-006 R7: the trigger toggles the popup and Escape closes it", async () => {
  const host = createTestHost({ width: 400, height: 300 });
  host.render(() => comboApp());
  await click(host, "trigger");
  assert.ok(find(host, "content"), "a trigger click opens");
  assert.ok(focusedId(host, "input"), "opening moves focus to the input");
  await press(host, "escape");
  assert.equal(find(host, "content"), null, "Escape closes the top layer");
  assert.deepEqual(host.errors, []);
});
