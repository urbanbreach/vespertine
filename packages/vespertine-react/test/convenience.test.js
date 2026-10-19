// APP-009 R9 / A4: the convenience render function.

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import React from "react";
import { createReferenceNative } from "@vespertine/host/reference";
import { flushSync, render, resetRender } from "../src/index.js";
import { referenceRenderer, settle } from "./helpers.js";

const h = React.createElement;

afterEach(() => resetRender());

/** An addon module whose renderers are collected, with a counted requiresTick (one per frame loop). */
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

test("A4: repeated renders keep one renderer, one window and one frame loop; later window options are ignored", async () => {
  const { native, created } = trackedNative();
  const first = render(h("div", { testId: "one" }), { native, window: { title: "first" }, frameMs: 5 });
  await settle(2);
  const second = render(h("div", { testId: "two" }), { native, window: { title: "second" }, frameMs: 5 });
  await settle(2);
  assert.equal(created.length, 1);
  const [renderer] = created;
  assert.equal(renderer.initialized, 1);
  assert.equal(renderer.initOptions.title, "first");
  assert.equal(renderer.loopsStarted, 1);
  assert.notEqual(first, second);
  assert.equal(renderer.snapshot().children[0].props.testId, "two", "the previous tree was unmounted before the new one mounted");
  assert.equal(renderer.elements.size, 2);
  assert.ok(renderer.ticks > 0);
});

test("A4: an injected renderer is neither initialized nor pumped", async () => {
  const renderer = referenceRenderer();
  render(h("div"), { renderer, frameMs: 5 });
  await settle(5);
  const native = renderer.native ?? renderer;
  assert.equal(native.initialized, 0);
  assert.equal(native.ticks, 0);
  assert.equal(native.elements.size, 2);
});

test("A4: a debug frame-overlay mode is forwarded to the renderer", () => {
  const renderer = referenceRenderer();
  render(h("div"), { renderer, debugOverlay: "full" });
  assert.equal(renderer.debugOverlay, "full");
});

test("A4: resetRender stops the loop and unmounts the current root", async () => {
  const { native, created } = trackedNative();
  render(h("div"), { native, frameMs: 5 });
  await settle(3);
  const [renderer] = created;
  assert.equal(renderer.elements.size, 2);
  resetRender();
  const ticks = renderer.ticks;
  assert.equal(renderer.elements.size, 0);
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(renderer.ticks, ticks, "no tick after the reset");
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
    render(h("div"), { native: { Renderer }, frameMs: 5 });
    await settle(3);
  } finally {
    process.exit = realExit;
  }
  assert.deepEqual(exits, [0]);
});

test("flushSync renders synchronously", () => {
  const renderer = referenceRenderer();
  const root = render(h("div"), { renderer });
  flushSync(() => root.render(h("div", { testId: "now" })));
  assert.equal(renderer.snapshot().children[0].props.testId, "now");
});
