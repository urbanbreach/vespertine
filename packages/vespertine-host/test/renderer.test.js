import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { createHandlerMap, createNativeRenderer, createRendererState, registerEventHandler } from "../src/index.js";
import { tty } from "../src/stdio.js";

const realTty = { ...tty };
afterEach(() => Object.assign(tty, realTty));

class FakeRenderer {
  constructor(callback) {
    this.callback = callback;
    this.initialized = false;
  }
  init(options) {
    this.initialized = true;
    return options;
  }
}

test("importing the helpers does not load the native addon or open a window", async () => {
  const mod = await import("../src/index.js");
  assert.equal(typeof mod.createNativeRenderer, "function");
});

test("native events reach the active root; only handled events reach onEvent; errors reach onError", () => {
  tty.stdinExists = () => true;
  tty.stdinIsTerminal = () => true;
  const events = [];
  const errors = [];
  const renderer = createNativeRenderer({
    native: { Renderer: FakeRenderer },
    onEvent: (e) => events.push(e.type),
    onError: (e) => errors.push(e.message),
  });
  const handlers = createHandlerMap();
  registerEventHandler(handlers, 1, "click", () => {});
  registerEventHandler(handlers, 2, "click", () => {
    throw new Error("handler failed");
  });
  createRendererState(renderer).attach({ handlers });
  renderer.callback(null, { type: "click", id: 1 });
  renderer.callback(null, { type: "click", id: 9 });
  renderer.callback(null, { type: "click", id: 2 });
  renderer.callback(new Error("native failed"), undefined);
  assert.deepEqual(events, ["click"]);
  assert.deepEqual(errors, ["handler failed", "native failed"]);
});

test("a non-interactive process serves automation after the real init succeeds", () => {
  tty.stdinExists = () => true;
  tty.stdinIsTerminal = () => false;
  const log = [];
  const renderer = createNativeRenderer({
    native: { Renderer: FakeRenderer },
    serveAutomation: () => log.push(["serve", renderer.initialized]),
  });
  assert.deepEqual(renderer.init({ title: "t" }), { title: "t" });
  assert.deepEqual(log, [["serve", true]]);
});

test("a failing init does not start automation; an interactive process never does", () => {
  tty.stdinExists = () => true;
  tty.stdinIsTerminal = () => false;
  const log = [];
  class Failing extends FakeRenderer {
    init() {
      throw new Error("no window");
    }
  }
  const failing = createNativeRenderer({ native: { Renderer: Failing }, serveAutomation: () => log.push("serve") });
  assert.throws(() => failing.init(), /no window/);
  tty.stdinIsTerminal = () => true;
  const interactive = createNativeRenderer({ native: { Renderer: FakeRenderer }, serveAutomation: () => log.push("serve") });
  interactive.init();
  assert.deepEqual(log, []);
});
