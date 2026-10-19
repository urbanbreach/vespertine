import assert from "node:assert/strict";
import { test } from "node:test";
import { resolveFocusTarget } from "../src/index.js";

test("unset, null and true select the fallback", () => {
  assert.equal(resolveFocusTarget(undefined, 7), 7);
  assert.equal(resolveFocusTarget(null, 7), 7);
  assert.equal(resolveFocusTarget(true, 7), 7);
  assert.equal(resolveFocusTarget(true, null), null);
});

test("false preserves focus", () => {
  assert.equal(resolveFocusTarget(false, 7), null);
});

test("a node yields its identity", () => {
  assert.equal(resolveFocusTarget({ id: 3 }, 7), 3);
  assert.equal(resolveFocusTarget({ id: 0 }, 7), 0);
});

test("a reference yields its node, or the fallback while empty", () => {
  assert.equal(resolveFocusTarget({ current: null }, 7), 7);
  assert.equal(resolveFocusTarget({ current: { id: 4 } }, 7), 4);
});

test("a function is called first", () => {
  assert.equal(resolveFocusTarget(() => undefined, 7), null);
  assert.equal(resolveFocusTarget(() => null, 7), 7);
  assert.equal(resolveFocusTarget(() => ({ id: 9 }), 7), 9);
  assert.equal(resolveFocusTarget(() => false, 7), null);
  assert.equal(resolveFocusTarget(() => ({ current: { id: 2 } }), 7), 2);
});

test("anything else is rejected", () => {
  assert.throws(() => resolveFocusTarget("x", 7), TypeError);
});
