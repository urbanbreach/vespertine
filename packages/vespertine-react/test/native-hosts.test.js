// The native editor and canvas host types on the offscreen test host (ELM-007 R12): the `input`
// field (CMP-007), the `textarea` editor (CMP-008) and the `canvas` drawing host (RND-009) render
// through persistent adapters and take real keyboard and pointer input through the production
// event path.

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { screenshotPixels } from "@vespertine/host/testing";
import { createTestHost, testHostAvailable } from "../src/testing.js";

const h = React.createElement;
const available = testHostAvailable();
const native = (name, body) => test(name, { skip: available ? false : "no GPU test backend" }, body);

function byTestId(host, testId) {
  const [node] = host.renderer.findByTestId(testId);
  assert.ok(node, `no element with testId ${testId}`);
  return node;
}

const centerOf = ({ bounds }) => [bounds.x + bounds.width / 2, bounds.y + bounds.height / 2];

const type = (host, keys) => host.renderer.nativeSimulateKeystrokes(keys);

/** The painted text runs that show something (an empty field paints an empty run). */
const painted = (host) => host.renderer.getPaintedText().filter((text) => text !== "");

// ---- input (CMP-007) -----------------------------------------------------------------------

function inputTree(log, props = {}) {
  return h(
    "div",
    { style: { width: 400, height: 200, display: "flex", flexDirection: "column", gap: 10 } },
    h("input", {
      testId: "field",
      placeholder: "Type here",
      style: { width: 300, height: 30 },
      onChange: (event) => log.push(["change", event.value]),
      onSubmit: (event) => log.push(["submit", event.value]),
      onCancel: () => log.push(["cancel"]),
      onFocus: () => log.push(["focus"]),
      onBlur: () => log.push(["blur"]),
      ...props,
    }),
    h("input", { testId: "other", style: { width: 300, height: 30 } }),
  );
}

native("an input shows its placeholder, takes focus by a press and reports typing, submit and cancel", () => {
  const log = [];
  const host = createTestHost({ width: 400, height: 200 });
  host.render(inputTree(log));
  assert.deepEqual(painted(host), ["Type here"], "the placeholder shows while the value is empty");
  const field = byTestId(host, "field");
  assert.deepEqual(field.bounds, { x: 0, y: 0, width: 300, height: 30 });
  host.renderer.nativeSimulateClick(...centerOf(field));
  assert.equal(host.renderer.getFocusedElementId(), field.id, "the host node holds the editor's focus");
  assert.deepEqual(log, [["focus"]]);
  type(host, "h i");
  assert.deepEqual(log.slice(1), [["change", "h"], ["change", "hi"]]);
  assert.deepEqual(painted(host), ["hi"]);
  type(host, "enter");
  type(host, "escape");
  assert.deepEqual(log.slice(3), [["submit", "hi"], ["cancel"]]);
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "other")));
  assert.deepEqual(log.at(-1), ["blur"]);
  assert.deepEqual(host.errors, []);
});

native("a controlled input keeps its caret on an echo, takes an unrelated value once and edits on from its end", () => {
  const log = [];
  const host = createTestHost({ width: 400, height: 200 });
  host.render(inputTree(log, { value: "" }));
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "field")));
  type(host, "a b c");
  assert.deepEqual(log.filter(([name]) => name === "change").map(([, value]) => value), ["a", "ab", "abc"]);
  host.render(inputTree(log, { value: "ab" }));
  assert.deepEqual(painted(host), ["abc"], "an echo of an older emission changes nothing");
  host.render(inputTree(log, { value: "zzz" }));
  assert.deepEqual(painted(host), ["zzz"], "an unrelated value replaces the text");
  const changes = log.filter(([name]) => name === "change").length;
  host.render(inputTree(log, { value: "zzz" }));
  type(host, "!");
  assert.deepEqual(log.at(-1), ["change", "zzz!"], "the caret was collapsed at the end of the pushed text");
  assert.equal(log.filter(([name]) => name === "change").length, changes + 1, "pushing a value is not an edit");
  assert.deepEqual(host.errors, []);
});

native("read-only, disabled and default-value inputs behave like the editor's modes", () => {
  const log = [];
  const host = createTestHost({ width: 400, height: 200 });
  host.render(inputTree(log, { defaultValue: "kept", readOnly: true }));
  assert.deepEqual(painted(host), ["kept"], "the default value seeds the editor");
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "field")));
  type(host, "x backspace");
  assert.equal(log.some(([name]) => name === "change"), false, "a read-only field rejects edits");
  host.render(inputTree(log, { defaultValue: "kept", readOnly: false, disabled: true }));
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "other")));
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "field")));
  type(host, "y");
  assert.equal(log.some(([name]) => name === "change"), false, "a disabled field takes no input");
  host.render(inputTree(log, { defaultValue: "kept" }));
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "field")));
  type(host, "y");
  assert.deepEqual(log.filter(([name]) => name === "change"), [["change", "kepty"]]);
  assert.deepEqual(host.errors, []);
});

native("an input is reached by a host reference, autofocus and the tab order", () => {
  const log = [];
  const ref = React.createRef();
  const host = createTestHost({ width: 400, height: 200 });
  host.render(inputTree(log, { ref }));
  assert.equal(host.renderer.getFocusedElementId(), null);
  ref.current.focus();
  assert.equal(host.renderer.getFocusedElementId(), byTestId(host, "field").id);
  host.renderer.focusNext();
  assert.equal(host.renderer.getFocusedElementId(), byTestId(host, "other").id, "Tab leaves a plain field (CMP-007 R8)");
  host.renderer.focusPrevious();
  assert.equal(host.renderer.getFocusedElementId(), byTestId(host, "field").id);
  host.unmount();
  const auto = createTestHost({ width: 400, height: 200 });
  auto.render(inputTree([], { autoFocus: true }));
  auto.renderer.advanceTime(50);
  assert.equal(auto.renderer.getFocusedElementId(), byTestId(auto, "field").id, "autoFocus focuses the editor");
});

native("an input's style frames the editor: surface, border and text color follow the style", () => {
  const host = createTestHost({ width: 400, height: 200, scaleFactor: 1 });
  host.render(
    h(
      "div",
      { style: { width: 400, height: 200, backgroundColor: "#101010", display: "flex" } },
      h("input", {
        testId: "field",
        defaultValue: "",
        style: { width: 200, height: 40, marginLeft: 20, marginTop: 20, backgroundColor: "#204060", borderRadius: 0, borderWidth: 2, borderColor: "#ff8800" },
      }),
    ),
  );
  const { x, y } = byTestId(host, "field").bounds;
  const png = screenshotPixels(host.renderer);
  assert.deepEqual(png.at(x + 100, y + 20), [0x20, 0x40, 0x60, 255], "the style's background fills the field");
  assert.deepEqual(png.at(x + 100, y + 1), [0xff, 0x88, 0x00, 255], "the style's border surrounds it");
  assert.deepEqual(png.at(x + 100, y - 5), [0x10, 0x10, 0x10, 255]);
});

// ---- textarea (CMP-008) --------------------------------------------------------------------

function areaTree(log, props = {}) {
  return h(
    "div",
    { style: { width: 400, height: 300, display: "flex", flexDirection: "column" } },
    h("textarea", {
      testId: "area",
      placeholder: "Write",
      style: { width: 300 },
      onChange: (event) => log.push(["change", event.value]),
      onSubmit: (event) => log.push(["submit", event.value]),
      ...props,
    }),
  );
}

native("a textarea inserts newlines, grows with its lines up to the row bound and scrolls beyond it", () => {
  const log = [];
  const host = createTestHost({ width: 400, height: 300 });
  host.render(areaTree(log, { maxRows: 3 }));
  assert.deepEqual(painted(host), ["Write"]);
  const one = byTestId(host, "area").bounds.height;
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "area")));
  type(host, "a enter b");
  assert.deepEqual(log.at(-1), ["change", "a\nb"]);
  host.renderer.flush();
  const two = byTestId(host, "area").bounds.height;
  assert.ok(two > one, `two rows are taller than one (${one} -> ${two})`);
  type(host, "enter c enter d enter e");
  host.renderer.flush();
  const capped = byTestId(host, "area").bounds.height;
  assert.ok(capped > two, "three rows are taller than two");
  type(host, "enter f");
  host.renderer.flush();
  assert.equal(byTestId(host, "area").bounds.height, capped, "growth stops at the maximum row count");
  assert.equal(log.at(-1)[1], "a\nb\nc\nd\ne\nf");
  assert.deepEqual(host.errors, []);
});

native("a textarea in enter-submits mode submits on Enter and breaks the line on Shift+Enter", () => {
  const log = [];
  const host = createTestHost({ width: 400, height: 300 });
  host.render(areaTree(log, { submitMode: "enterSubmits", value: "" }));
  host.renderer.nativeSimulateClick(...centerOf(byTestId(host, "area")));
  type(host, "o k shift-enter z");
  assert.deepEqual(log.at(-1), ["change", "ok\nz"]);
  type(host, "enter");
  assert.deepEqual(log.at(-1), ["submit", "ok\nz"]);
  host.render(areaTree(log, { submitMode: "enterSubmits", value: "reset" }));
  assert.deepEqual(painted(host), ["reset"], "a controlled value replaces the document");
  assert.deepEqual(host.errors, []);
});

// ---- canvas (RND-009) ----------------------------------------------------------------------

const canvasTree = (commands, props = {}) =>
  h(
    "div",
    { style: { width: 200, height: 100, backgroundColor: "#181a1f" } },
    h("canvas", { testId: "canvas", style: { position: "absolute", left: 20, top: 10, width: 100, height: 60 }, commands, ...props }),
  );

native("a canvas paints its drawing list inside its own box with the host's coordinates", () => {
  const host = createTestHost({ width: 200, height: 100, scaleFactor: 1 });
  host.render(
    canvasTree([
      { op: "rect", x: 0, y: 0, width: 10, height: 10, color: "#ff0000" },
      { op: "circle", cx: 50, cy: 30, radius: 10, color: "#00ff00" },
      { op: "fill", points: [[70, 10], [90, 10], [80, 30]], color: "#0000ff" },
      { op: "stroke", points: [[0, 50], [100, 50]], width: 2, color: "#ffffff" },
      { op: "path", segments: [["M", 10, 20], ["L", 30, 20], ["L", 30, 40], ["Z"]], fill: "#ffff00" },
      { op: "text", x: 2, y: 36, text: "Hi", size: 14, color: "#ffffff" },
      { op: "rect", x: 90, y: 40, width: 200, height: 100, color: "#ff00ff" },
      { op: "nothing" },
    ]),
  );
  assert.deepEqual(byTestId(host, "canvas").bounds, { x: 20, y: 10, width: 100, height: 60 });
  const png = screenshotPixels(host.renderer);
  const at = (x, y) => png.at(20 + x, 10 + y);
  assert.deepEqual(at(5, 5), [255, 0, 0, 255], "a rectangle");
  assert.deepEqual(at(50, 30), [0, 255, 0, 255], "a circle");
  assert.deepEqual(at(80, 15), [0, 0, 255, 255], "a filled polygon");
  assert.deepEqual(at(50, 49), [255, 255, 255, 255], "a stroked line");
  assert.deepEqual(at(25, 25), [255, 255, 0, 255], "a closed path");
  assert.deepEqual(at(40, 5), [24, 26, 31, 255], "nothing else is painted");
  assert.deepEqual(at(95, 45), [255, 0, 255, 255], "a rectangle past the box is cut at the box edge");
  assert.deepEqual(png.at(130, 50), [24, 26, 31, 255], "drawing never leaves the canvas box");
  assert.deepEqual(painted(host), ["Hi"], "canvas text is painted text");
  assert.deepEqual(host.errors, []);
});

native("a canvas repaints when its commands change and clears when they are removed", () => {
  const host = createTestHost({ width: 200, height: 100, scaleFactor: 1 });
  host.render(canvasTree([{ op: "rect", x: 0, y: 0, width: 20, height: 20, color: "#ff0000" }]));
  assert.deepEqual(screenshotPixels(host.renderer).at(30, 20), [255, 0, 0, 255]);
  host.render(canvasTree([{ op: "rect", x: 0, y: 0, width: 20, height: 20, color: "#0000ff" }]));
  assert.deepEqual(screenshotPixels(host.renderer).at(30, 20), [0, 0, 255, 255]);
  host.render(canvasTree(undefined));
  assert.deepEqual(screenshotPixels(host.renderer).at(30, 20), [24, 26, 31, 255]);
});

native("a canvas reports the standard click event and has no hit area beyond its box", () => {
  const clicks = [];
  const host = createTestHost({ width: 200, height: 100, scaleFactor: 1 });
  host.render(canvasTree([{ op: "rect", x: 0, y: 0, width: 10, height: 10, color: "#ff0000" }], { onClick: (event) => clicks.push([event.x, event.y]) }));
  host.renderer.nativeSimulateClick(60, 40);
  host.renderer.nativeSimulateClick(150, 90);
  assert.deepEqual(clicks, [[60, 40]]);
});
