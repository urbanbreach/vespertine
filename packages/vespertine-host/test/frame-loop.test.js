import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { startFrameLoop } from "../src/index.js";
import { installFakeTime } from "./helpers.js";

let time;
beforeEach(() => {
  time = installFakeTime();
});
afterEach(() => time.restore());

function renderer(over = {}) {
  const r = {
    ticks: [],
    wake: undefined,
    wakeRegistrations: [],
    cost: 0,
    result: true,
    requiresTick: () => true,
    setFrameWakeCallback(cb) {
      r.wakeRegistrations.push(cb);
      return r.wakeSupported === true && cb !== null;
    },
    tick() {
      r.ticks.push(time.now);
      time.spend(r.cost);
      return r.result;
    },
    ...over,
  };
  return r;
}

test("a renderer that needs no ticking gets an inert loop", () => {
  const r = renderer({ requiresTick: () => false });
  const loop = startFrameLoop(r);
  assert.equal(r.ticks.length, 0);
  assert.equal(r.wakeRegistrations.length, 0);
  loop.stop();
});

test("ticks immediately, then pumps on the 8 ms cadence", () => {
  const r = renderer();
  startFrameLoop(r);
  assert.equal(r.ticks.length, 1);
  time.advance(8);
  assert.equal(r.ticks.length, 2);
  time.advance(16);
  assert.equal(r.ticks.length, 4);
});

test("cadence subtracts time spent ticking and clamps to zero", () => {
  const r = renderer({ cost: 5 });
  startFrameLoop(r, { frameMs: 10 });
  const t0 = r.ticks[0];
  time.advance(5);
  assert.equal(r.ticks.length, 2);
  assert.equal(r.ticks[1] - t0, 10);
  r.cost = 30;
  time.advance(5);
  assert.equal(r.ticks.length, 3);
  // cost >= cadence: the next tick is due immediately (delay clamped to zero).
  time.advance(0);
  assert.equal(r.ticks.length, 4);
});

test("an explicit cadence skips wake registration", () => {
  const r = renderer();
  r.wakeSupported = true;
  startFrameLoop(r, { frameMs: 16 });
  assert.equal(r.wakeRegistrations.length, 0);
});

test("when wakes are supported the wake callback drives ticks with a 100 ms safety timer", () => {
  const r = renderer();
  r.wakeSupported = true;
  startFrameLoop(r);
  assert.equal(r.wakeRegistrations.length, 1);
  assert.equal(r.ticks.length, 1);
  time.advance(99);
  assert.equal(r.ticks.length, 1);
  time.advance(1);
  assert.equal(r.ticks.length, 2);
  r.wakeRegistrations[0]();
  assert.equal(r.ticks.length, 3);
  // The tick cancelled the pending safety timer and rearmed one.
  assert.equal(time.pendingTimers, 1);
  time.advance(99);
  assert.equal(r.ticks.length, 3);
});

test("a wake registration that reports false falls back to timed pumping", () => {
  const r = renderer();
  startFrameLoop(r);
  assert.equal(r.wakeRegistrations.length, 1);
  time.advance(8);
  assert.equal(r.ticks.length, 2);
});

test("a thrown tick reports to onError and the loop continues", () => {
  const errors = [];
  let n = 0;
  const r = renderer({
    tick() {
      if (n++ === 0) throw new Error("boom");
      return true;
    },
  });
  startFrameLoop(r, { onError: (e) => errors.push(e.message) });
  assert.deepEqual(errors, ["boom"]);
  time.advance(8);
  assert.equal(n, 2);
});

test("tick returning false terminates once and cleans up", () => {
  const r = renderer();
  r.wakeSupported = true;
  let terminated = 0;
  startFrameLoop(r, { onTerminated: () => terminated++ });
  r.result = false;
  time.advance(100);
  assert.equal(terminated, 1);
  assert.equal(time.pendingTimers, 0);
  assert.equal(r.wakeRegistrations.at(-1), null);
  const ticks = r.ticks.length;
  r.wakeRegistrations[0]();
  time.advance(1000);
  assert.equal(r.ticks.length, ticks);
  assert.equal(terminated, 1);
});

test("stop is idempotent, clears the timer and removes the wake callback", () => {
  const r = renderer();
  r.wakeSupported = true;
  const loop = startFrameLoop(r);
  loop.stop();
  loop.stop();
  assert.equal(time.pendingTimers, 0);
  assert.deepEqual(r.wakeRegistrations.filter((cb) => cb === null).length, 1);
  const ticks = r.ticks.length;
  r.wakeRegistrations[0]();
  time.advance(500);
  assert.equal(r.ticks.length, ticks);
});
