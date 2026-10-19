import assert from "node:assert/strict";
import { test } from "node:test";
import { createMutationQueue, mutationBatchAttempts } from "../src/index.js";

function host(result = []) {
  const calls = [];
  return {
    calls,
    fail: false,
    applyBatch(json) {
      calls.push(JSON.parse(json));
      if (this.fail) throw new Error("rejected");
      return result;
    },
  };
}

test("operations serialize in order as one JSON batch", () => {
  const h = host();
  const q = createMutationQueue(h);
  q.createElement(0, "div");
  q.setStyle(0, { width: 10 });
  q.setText(0, "hi");
  q.appendChild(0, 1);
  q.insertBefore(0, 2, 1);
  q.setEventListener(0, "click", true);
  q.setRoot(0);
  q.setKeyboardFocusDim(false);
  q.setCustomProp(0, "tabIndex", 1);
  q.setCustomProp(0, "motion", undefined);
  assert.equal(q.pending, 10);
  q.flushMutations();
  assert.equal(q.pending, 0);
  assert.deepEqual(h.calls[0], [
    ["createElement", 0, "div"],
    ["setStyle", 0, { width: 10 }],
    ["setText", 0, "hi"],
    ["appendChild", 0, 1],
    ["insertBefore", 0, 2, 1],
    ["setEventListener", 0, "click", true],
    ["setRoot", 0],
    ["setKeyboardFocusDim", false],
    ["setCustomProp", 0, "tabIndex", 1],
    ["setCustomProp", 0, "motion", null],
  ]);
});

test("an empty flush does nothing and does not count as a submission", () => {
  const h = host();
  const q = createMutationQueue(h);
  const before = mutationBatchAttempts();
  q.flushMutations();
  assert.equal(h.calls.length, 0);
  assert.equal(mutationBatchAttempts(), before);
});

test("destroyElement returns an empty list immediately; cleanup receives native-reported ids", () => {
  const h = host([5, 6, 7]);
  const cleaned = [];
  const q = createMutationQueue(h, (ids) => cleaned.push(ids));
  assert.deepEqual(q.destroyElement(5), []);
  q.flushMutations();
  assert.deepEqual(cleaned, [[5, 6, 7]]);
  assert.equal(q.pending, 0);
});

test("a failed batch retains operations and skips cleanup, and still counts as an attempt", () => {
  const h = host();
  const cleaned = [];
  const q = createMutationQueue(h, (ids) => cleaned.push(ids));
  q.createElement(1, "div");
  h.fail = true;
  const before = mutationBatchAttempts();
  assert.throws(() => q.flushMutations(), /rejected/);
  assert.equal(q.pending, 1);
  assert.deepEqual(cleaned, []);
  assert.equal(mutationBatchAttempts(), before + 1);
  h.fail = false;
  q.flushMutations();
  assert.equal(q.pending, 0);
  assert.equal(h.calls.length, 2);
  assert.equal(mutationBatchAttempts(), before + 2);
});

test("a scheduler is asked once for many enqueues", () => {
  const h = host();
  const scheduled = [];
  const q = createMutationQueue(h, undefined, (fn) => scheduled.push(fn));
  for (let i = 0; i < 5; i++) q.createElement(i, "div");
  assert.equal(scheduled.length, 1);
  scheduled[0]();
  assert.equal(h.calls.length, 1);
  q.createElement(9, "div");
  assert.equal(scheduled.length, 2);
});

test("an explicit flush makes the scheduled callback a no-op", () => {
  const h = host();
  const scheduled = [];
  const q = createMutationQueue(h, undefined, (fn) => scheduled.push(fn));
  q.createElement(1, "div");
  q.flushMutations();
  scheduled[0]();
  assert.equal(h.calls.length, 1);
  // A stale callback must not flush a later schedule's operations early.
  q.createElement(2, "div");
  assert.equal(scheduled.length, 2);
  scheduled[0]();
  assert.equal(h.calls.length, 1);
  scheduled[1]();
  assert.equal(h.calls.length, 2);
});

test("operations queued during cleanup stay for the next batch", () => {
  const h = host([1]);
  let q;
  q = createMutationQueue(h, () => q.createElement(9, "div"));
  q.destroyElement(1);
  q.flushMutations();
  assert.equal(q.pending, 1);
});
