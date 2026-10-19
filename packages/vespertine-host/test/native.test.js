// Headless checks of the native addon (no window is opened). Skipped by nothing: the
// addon must be built (`cargo build -p vespertine_js_node`) and located through
// VESPERTINE_NATIVE_ADDON or `native/vespertine_js.node`.

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadNative } from "../src/native.js";

const native = loadNative();

const mount = JSON.stringify([
  ["createElement", 1, "div"],
  ["setStyle", 1, { width: 10 }],
  ["setRoot", 1],
]);

test("a new renderer is uninitialized and requires ticking", () => {
  const renderer = new native.Renderer();
  assert.equal(renderer.isInitialized(), false);
  assert.equal(renderer.requiresTick(), true);
  assert.equal(renderer.setFrameWakeCallback(() => {}), false);
  assert.equal(renderer.setFrameWakeCallback(null), false);
});

test("tick before initialization tells the caller to initialize first", () => {
  assert.throws(() => new native.Renderer().tick(), /call init\(\) before tick\(\)/);
});

test("commands before initialization fail with the uninitialized error", () => {
  const renderer = new native.Renderer(() => {});
  for (const call of [
    () => renderer.getWindowSize(),
    () => renderer.getWindowInsets(),
    () => renderer.activateWindow(),
    () => renderer.minimizeWindow(),
    () => renderer.zoomWindow(),
    () => renderer.toggleFullscreen(),
    () => renderer.setWindowTitle("x"),
    () => renderer.closeWindow(),
  ]) {
    assert.throws(call, /not initialized/);
  }
});

test("invalid window options fail init before any window is opened", () => {
  const renderer = new native.Renderer();
  assert.throws(() => renderer.init({ width: -5 }), /width must be a positive/);
  assert.throws(() => renderer.init({ windowBackground: "glass" }), /windowBackground/);
  assert.throws(() => renderer.init({ width: "wide" }), /invalid window options/);
  assert.equal(renderer.isInitialized(), false);
});

test("a prompt before initialization rejects with the uninitialized error", async () => {
  const renderer = new native.Renderer();
  const pending = renderer.promptForPaths({});
  assert.ok(pending instanceof Promise);
  await assert.rejects(pending, /not initialized/);
  await assert.rejects(renderer.promptForPaths({ files: false, directories: false }), /not initialized/);
});

test("an empty batch is valid and returns no destroyed identities", () => {
  assert.deepEqual(new native.Renderer().applyBatch("[]"), []);
});

test("a malformed batch is rejected naming the failing operation", () => {
  const renderer = new native.Renderer();
  assert.throws(() => renderer.applyBatch('[["createElement",1,"div"],["bogus",2]]'), /Batch op 1/);
  assert.throws(() => renderer.applyBatch('{"not":"an array"}'), /failed to parse batch/);
  assert.throws(() => renderer.applyBatch('[["setRoot",-1]]'), /Batch op 0/);
  assert.throws(() => renderer.applyBatch('[["setStyle",1,"nope"]]'), /Batch op 0 setStyle parse error/);
});

test("a batch before initialization is applied, and then the repaint request fails", () => {
  const renderer = new native.Renderer();
  assert.throws(() => renderer.applyBatch(mount), /repaint request failed.*not initialized/s);
  // The tree was changed: an equivalent resend changes nothing, so it needs no repaint.
  assert.deepEqual(
    renderer.applyBatch(
      JSON.stringify([
        ["setStyle", 1, { width: 10 }],
        ["setRoot", 1],
      ]),
    ),
    [],
  );
});

test("registerFont is available and rejects before initialization", () => {
  assert.throws(() => new native.Renderer().registerFont(new Uint8Array([1, 2, 3, 4])), /not initialized/);
});
