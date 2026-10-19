// APP-009 R9 / A4 for the Solid adapter.

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { createReferenceNative } from "@vespertine/host/reference";
import { render, resetRender } from "../src/index.js";
import { h, referenceRenderer, settle } from "./helpers.js";

afterEach(() => resetRender());

function trackedNative() {
  const base = createReferenceNative();
  const created = [];
  class Renderer extends base.Renderer {
    constructor(callback) {
      super(callback);
      this.loopsStarted = 0;
      created.push(this);
    }
    requiresTick() {
      this.loopsStarted++;
      return true;
    }
  }
  return { native: { Renderer }, created };
}

test("A4: repeated renders keep one renderer, window and frame loop; later window options are ignored", async () => {
  const { native, created } = trackedNative();
  render(() => h("div", { testId: "one" }), { native, window: { title: "first" }, frameMs: 5 });
  render(() => h("div", { testId: "two" }), { native, window: { title: "second" }, frameMs: 5 });
  await settle(2);
  assert.equal(created.length, 1);
  const [renderer] = created;
  assert.equal(renderer.initialized, 1);
  assert.equal(renderer.initOptions.title, "first");
  assert.equal(renderer.loopsStarted, 1);
  assert.equal(renderer.snapshot().children[0].props.testId, "two");
  assert.equal(renderer.elements.size, 2);
});

test("A4: an injected renderer is neither initialized nor pumped", async () => {
  const renderer = referenceRenderer();
  render(() => h("div"), { renderer, frameMs: 5 });
  await settle(5);
  assert.equal(renderer.initialized, 0);
  assert.equal(renderer.ticks, 0);
  assert.equal(renderer.elements.size, 2);
});

test("A4: a debug overlay mode is forwarded and resetRender stops the loop and unmounts", async () => {
  const { native, created } = trackedNative();
  render(() => h("div"), { native, frameMs: 5, debugOverlay: "minimal" });
  await settle(3);
  const [renderer] = created;
  assert.equal(renderer.debugOverlay, "minimal");
  resetRender();
  const ticks = renderer.ticks;
  assert.equal(renderer.elements.size, 0);
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(renderer.ticks, ticks);
});

test("R9: the Solid adapter installs no process-level error handlers", () => {
  const before = [process.listenerCount("uncaughtException"), process.listenerCount("unhandledRejection")];
  const { native } = trackedNative();
  render(() => h("div"), { native });
  assert.deepEqual([process.listenerCount("uncaughtException"), process.listenerCount("unhandledRejection")], before);
});

test("R9: the process exits when the native side reports termination", async () => {
  const base = createReferenceNative();
  class Renderer extends base.Renderer {
    tick() {
      return false;
    }
  }
  const exits = [];
  const realExit = process.exit;
  process.exit = (code) => exits.push(code);
  try {
    render(() => h("div"), { native: { Renderer }, frameMs: 5 });
    await settle(3);
  } finally {
    process.exit = realExit;
  }
  assert.deepEqual(exits, [0]);
});
