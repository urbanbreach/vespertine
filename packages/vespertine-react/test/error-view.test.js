// DEV-010: runtime error display and remount recovery.

import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import React from "react";
import { createReferenceNative } from "@vespertine/host/reference";
import { flushSync, normalizeFailure, render, resetRender, stackRows } from "../src/index.js";
import { referenceRenderer, settle } from "./helpers.js";

const h = React.createElement;

let logged;
const realConsoleError = console.error;
beforeEach(() => {
  logged = [];
  console.error = (...args) => logged.push(args);
});
afterEach(() => {
  console.error = realConsoleError;
  resetRender();
});

const textOf = (node) => node.text ?? node.children.map(textOf).join("");

function find(node, testId) {
  if (node.props.testId === testId) return node;
  for (const child of node.children) {
    const found = find(child, testId);
    if (found) return found;
  }
  return null;
}

function failingError(message, stack) {
  const error = new Error(message);
  error.stack = stack;
  return error;
}

test("A2: an Error with a component stack shows the message once and indents frame rows", () => {
  const error = failingError(
    "Boom",
    "Error: Boom\nat explode (file.js:1:1)\n    at nested (file.js:2:2)\nTypeError: other",
  );
  const { message, stack } = normalizeFailure(error, "\n    at Widget (widget.js:3:3)");
  assert.equal(message, "Boom");
  assert.equal(stack.split("at Widget").length, 2, "the component stack is appended once");
  assert.equal(normalizeFailure(error, "\n    at Widget (widget.js:3:3)").stack, stack);
  const again = normalizeFailure(failingError("Boom", stack), "\n    at Widget (widget.js:3:3)");
  assert.equal(again.stack, stack, "an already contained component stack is not appended twice");
  assert.deepEqual(stackRows(message, stack), [
    "    at explode (file.js:1:1)",
    "    at nested (file.js:2:2)",
    "TypeError: other",
    "    at Widget (widget.js:3:3)",
  ]);
});

test("A2: strings, unconvertible values and empty messages", () => {
  assert.deepEqual(normalizeFailure("plain", ""), { message: "plain", stack: "plain" });
  const hostile = { toString() { throw new Error("no"); } };
  assert.equal(normalizeFailure(hostile).message, "Unknown error");
  assert.equal(normalizeFailure(new Error("")).message, "Error", "an empty message falls back to the name");
  const nameless = new Error("");
  nameless.name = "";
  assert.equal(normalizeFailure(nameless).message, "Unknown error");
  assert.equal(normalizeFailure("").message, "Unknown error");
  assert.equal(normalizeFailure(42).message, "42");
  assert.equal(normalizeFailure(new Error("m")).stack.includes("m"), true);
  const noStack = new Error("m");
  noStack.stack = undefined;
  assert.equal(normalizeFailure(noStack).stack.includes("Error: m"), true);
});

test("A2: a stack consisting only of the message shows it as the single fallback row", () => {
  assert.deepEqual(stackRows("Example failure", "Example failure"), ["Example failure"]);
  assert.deepEqual(stackRows("Example failure", "Error: Example failure\n   \n"), ["Example failure"]);
  assert.deepEqual(stackRows("Example failure", ""), ["Example failure"]);
});

function Boom() {
  throw new Error("Example failure");
}

test("A1: a component that throws is replaced by the error view in the same window; Reload remounts the application", async () => {
  const renderer = referenceRenderer();
  let broken = true;
  const App = () => {
    if (broken) throw new Error("Example failure");
    return h("div", { testId: "app" }, "all good");
  };
  render(h(App), { renderer });
  await settle(4);
  let view = renderer.snapshot().children[0];
  assert.equal(view.props.testId, "error-view");
  assert.equal(textOf(find(view, "error-heading")), "Uncaught runtime errors:");
  assert.equal(textOf(find(view, "error-message")), "Example failure");
  const reload = find(view, "error-reload");
  assert.equal(textOf(reload), "Reload");
  assert.ok(logged.some((args) => String(args[0]).includes("Example failure")), "the error is logged first");
  const initialized = renderer.initialized;

  broken = false;
  renderer.emit({ type: "click", id: reload.id });
  await settle(2);
  view = renderer.snapshot().children[0];
  assert.equal(view.props.testId, "app");
  assert.equal(renderer.initialized, initialized, "the window (renderer) identity is unchanged");

  broken = true;
  render(h(App), { renderer });
  await settle(4);
  assert.equal(renderer.snapshot().children[0].props.testId, "error-view", "a new error shows the view again");
});

test("R5: Reload always remounts the most recent submission", async () => {
  const renderer = referenceRenderer();
  let broken = true;
  const App = ({ label }) => {
    if (broken) throw new Error("later");
    return h("div", { testId: label });
  };
  render(h(App, { label: "first" }), { renderer });
  await settle(4);
  render(h(App, { label: "second" }), { renderer });
  await settle(4);
  const reload = find(renderer.snapshot().children[0], "error-reload");
  assert.ok(reload);
  broken = false;
  renderer.emit({ type: "click", id: reload.id });
  await settle(2);
  assert.equal(renderer.snapshot().children[0].props.testId, "second");
  // Once the application was replaced, a stale Reload control no longer exists and does nothing.
  assert.equal(renderer.elements.has(reload.id), false);
  renderer.emit({ type: "click", id: reload.id });
  assert.equal(renderer.snapshot().children[0].props.testId, "second");
});

test("A3: an error is ignored when the root was replaced before the microtask; a second error does not replace the view; a failing display is logged without recursion", async () => {
  const { Renderer } = createReferenceNative();
  const created = [];
  const native = {
    Renderer: class extends Renderer {
      constructor(callback) {
        super(callback);
        created.push(this);
      }
    },
  };
  render(h("div", { testId: "app" }), { native });
  await settle(2);
  const renderer = created[0];

  // Superseded root: the error is raised, the root is replaced before the microtask runs.
  renderer.callback(new Error("superseded"), undefined);
  render(h("div", { testId: "next" }), { native });
  await settle(3);
  assert.equal(renderer.snapshot().children[0].props.testId, "next");

  // A second error while the view is displayed does not replace it.
  renderer.callback(new Error("first"), undefined);
  await settle(3);
  const displayed = renderer.snapshot().children[0];
  assert.equal(displayed.props.testId, "error-view");
  const displayedId = displayed.id;
  renderer.callback(new Error("second"), undefined);
  await settle(3);
  assert.equal(renderer.snapshot().children[0].id, displayedId);
  assert.equal(textOf(find(renderer.snapshot().children[0], "error-message")), "first");

  // A failure while displaying is logged and does not recurse.
  render(h("div", { testId: "again" }), { native });
  await settle(2);
  const applyBatch = renderer.applyBatch.bind(renderer);
  renderer.applyBatch = (json) => {
    if (json.includes("Uncaught runtime errors")) throw new Error("display failed");
    return applyBatch(json);
  };
  logged.length = 0;
  renderer.callback(new Error("trigger"), undefined);
  await settle(3);
  assert.ok(logged.some((args) => String(args[0]).includes("Failed to display")));
  renderer.applyBatch = applyBatch;
});

test("A5: process handlers are installed once across repeated renders and removed by the reset", () => {
  const { Renderer } = createReferenceNative();
  const counts = () => [process.listenerCount("uncaughtException"), process.listenerCount("unhandledRejection")];
  const baseline = counts();
  render(h("div"), { native: { Renderer } });
  const installed = counts();
  assert.deepEqual(installed, [baseline[0] + 1, baseline[1] + 1]);
  render(h("div"), { native: { Renderer } });
  render(h("div"), { native: { Renderer } });
  assert.deepEqual(counts(), installed);
  resetRender();
  assert.deepEqual(counts(), baseline);
});

test("R1: an injected renderer installs no process handlers", () => {
  const baseline = process.listenerCount("uncaughtException");
  render(h("div"), { renderer: referenceRenderer() });
  assert.equal(process.listenerCount("uncaughtException"), baseline);
});

test("R1: an unhandled rejection is routed into the error view", async () => {
  const { Renderer } = createReferenceNative();
  const created = [];
  const before = new Set(process.listeners("unhandledRejection"));
  render(h("div", { testId: "app" }), {
    native: { Renderer: class extends Renderer { constructor(c) { super(c); created.push(this); } } },
  });
  await settle(2);
  // Simulate the process-level event without raising a real unhandled rejection.
  const handler = process.listeners("unhandledRejection").find((candidate) => !before.has(candidate));
  handler("rejected with a string");
  await settle(3);
  const view = created[0].snapshot().children[0];
  assert.equal(view.props.testId, "error-view");
  assert.equal(textOf(find(view, "error-message")), "rejected with a string");
});

test("no current root: an error schedules nothing", async () => {
  const { Renderer } = createReferenceNative();
  const before = new Set(process.listeners("uncaughtException"));
  render(h("div"), { native: { Renderer } });
  const handler = process.listeners("uncaughtException").find((candidate) => !before.has(candidate));
  resetRender();
  logged.length = 0;
  handler(new Error("late"));
  await settle(2);
  assert.ok(logged.length === 1, "logged once");
  flushSync(() => {});
});
