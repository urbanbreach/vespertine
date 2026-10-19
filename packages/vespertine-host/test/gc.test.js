import assert from "node:assert/strict";
import { afterEach, beforeEach, mock, test } from "node:test";
import { collectGarbageWhenIdle, createMutationQueue, mutationBatchAttempts } from "../src/index.js";
import { defaultCollector } from "../src/gc.js";
import { installFakeTime } from "./helpers.js";

let time;
let cpuMs;
const realCpuUsage = process.cpuUsage;
beforeEach(() => {
  time = installFakeTime();
  cpuMs = 0;
  process.cpuUsage = () => ({ user: cpuMs * 1000, system: 0 });
});
afterEach(() => {
  time.restore();
  process.cpuUsage = realCpuUsage;
  mock.restoreAll();
});

function setup(options = {}) {
  let activity = 0;
  let collections = 0;
  const stop = collectGarbageWhenIdle({
    activity: () => activity,
    collect: () => collections++,
    ...options,
  });
  return { stop, bump: () => activity++, count: () => collections };
}

test("collects after exactly two quiet checks, once per idle interval, and rearms after activity", () => {
  const g = setup();
  time.advance(5000);
  assert.equal(g.count(), 0);
  time.advance(5000);
  assert.equal(g.count(), 1);
  time.advance(15000);
  assert.equal(g.count(), 1);
  g.bump();
  time.advance(5000);
  assert.equal(g.count(), 1);
  time.advance(5000);
  assert.equal(g.count(), 1);
  time.advance(5000);
  assert.equal(g.count(), 2);
});

test("new activity during an incomplete quiet interval resets the count", () => {
  const g = setup();
  time.advance(5000);
  g.bump();
  time.advance(5000);
  assert.equal(g.count(), 0);
  time.advance(5000);
  assert.equal(g.count(), 0);
  time.advance(5000);
  assert.equal(g.count(), 1);
});

test("CPU at or above the busy fraction (including exactly equal) counts as active", () => {
  const g = setup();
  time.advance(5000);
  cpuMs += 2500; // exactly 0.5 of one core
  time.advance(5000);
  assert.equal(g.count(), 0);
  time.advance(5000);
  assert.equal(g.count(), 0);
  cpuMs += 2499; // just below
  time.advance(5000);
  assert.equal(g.count(), 1);
});

test("stopping prevents later collection and clears the timer", () => {
  const g = setup();
  time.advance(5000);
  g.stop();
  time.advance(60000);
  assert.equal(g.count(), 0);
  assert.equal(time.pendingTimers, 0);
});

test("a quiet count of 1 and a different period change the schedule", () => {
  const g = setup({ quietChecks: 1, checkMs: 1000 });
  time.advance(999);
  assert.equal(g.count(), 0);
  time.advance(1);
  assert.equal(g.count(), 1);
});

test("the default activity reader counts failed and successful submissions but not empty flushes", () => {
  const host = { ok: true, applyBatch() { if (!this.ok) throw new Error("x"); return []; } };
  const q = createMutationQueue(host);
  const base = mutationBatchAttempts();
  q.flushMutations();
  assert.equal(mutationBatchAttempts(), base);
  q.createElement(1, "div");
  q.flushMutations();
  assert.equal(mutationBatchAttempts(), base + 1);
  host.ok = false;
  q.createElement(2, "div");
  assert.throws(() => q.flushMutations());
  assert.equal(mutationBatchAttempts(), base + 2);

  let collections = 0;
  const stop = collectGarbageWhenIdle({ collect: () => collections++ });
  time.advance(5000);
  host.ok = true;
  q.flushMutations();
  time.advance(5000);
  time.advance(5000);
  assert.equal(collections, 0, "default reader saw the commit between checks");
  time.advance(5000);
  assert.equal(collections, 1);
  stop();
});

test("the check timer is unreferenced", () => {
  let unref = 0;
  const original = globalThis.setInterval;
  globalThis.setInterval = (fn, ms) => {
    const handle = original(fn, ms);
    handle.unref = () => {
      unref++;
      return handle;
    };
    return handle;
  };
  setup().stop();
  assert.equal(unref, 1);
});

test("with no collector available no timer is installed and the stopper is harmless", () => {
  const stop = collectGarbageWhenIdle({ activity: () => 0, collect: null });
  assert.equal(time.pendingTimers, 0);
  assert.equal(typeof stop, "function");
  stop();
  stop();
  assert.equal(defaultCollector({}), undefined);
});

test("Bun.gc(true) is preferred over a global gc function", () => {
  const calls = [];
  const globals = { gc: () => calls.push("gc"), Bun: { gc: (force) => calls.push(`bun:${force}`) } };
  defaultCollector(globals)();
  assert.deepEqual(calls, ["bun:true"]);
  defaultCollector({ gc: globals.gc })();
  assert.deepEqual(calls, ["bun:true", "gc"]);
});

test("absent process CPU reporting yields a ratio of zero without error", () => {
  process.cpuUsage = undefined;
  const g = setup({ quietChecks: 1 });
  time.advance(5000);
  assert.equal(g.count(), 1);
});
