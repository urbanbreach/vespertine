import assert from "node:assert/strict";
import { test } from "node:test";
import { sameMutationValue, serializeCustomProp } from "../src/index.js";

test("undefined-valued properties are absent", () => {
  assert.ok(sameMutationValue({ a: 1, b: undefined }, { a: 1 }));
  assert.ok(sameMutationValue({ a: 1 }, { a: 1, b: undefined }));
  assert.ok(!sameMutationValue({ a: 1, b: null }, { a: 1 }));
  assert.ok(!sameMutationValue({ a: undefined, b: 1 }, { a: 1, c: undefined }));
});

test("NaN equals NaN and negative zero equals zero", () => {
  assert.ok(sameMutationValue(NaN, NaN));
  assert.ok(sameMutationValue(-0, 0));
  assert.ok(sameMutationValue({ x: NaN }, { x: NaN }));
  assert.ok(!sameMutationValue(NaN, 0));
  assert.ok(!sameMutationValue(1, "1"));
});

test("arrays compare positionally with equal length", () => {
  assert.ok(sameMutationValue([1, [2, 3]], [1, [2, 3]]));
  assert.ok(!sameMutationValue([1, 2], [2, 1]));
  assert.ok(!sameMutationValue([1], [1, 2]));
});

test("plain objects compare structurally regardless of key order", () => {
  assert.ok(sameMutationValue({ a: 1, b: { c: 2 } }, { b: { c: 2 }, a: 1 }));
  assert.ok(sameMutationValue(Object.create(null), Object.create(null)));
  assert.ok(!sameMutationValue(Object.assign(Object.create(null), { a: 1 }), { a: 1 }));
  assert.ok(!sameMutationValue({ a: 1 }, { a: 2 }));
  assert.ok(!sameMutationValue({}, []));
  assert.ok(!sameMutationValue([], {}));
});

test("functions and class instances compare by identity only", () => {
  const f = () => 1;
  assert.ok(sameMutationValue(f, f));
  assert.ok(!sameMutationValue(f, () => 1));
  class Point {
    constructor(x) {
      this.x = x;
    }
  }
  const p = new Point(1);
  assert.ok(sameMutationValue(p, p));
  assert.ok(!sameMutationValue(new Point(1), new Point(1)));
  assert.ok(!sameMutationValue(new Date(1), new Date(1)));
});

test("custom property serialization maps undefined and functions to null", () => {
  assert.equal(serializeCustomProp(undefined), null);
  assert.equal(serializeCustomProp(() => 1), null);
  assert.equal(serializeCustomProp(0), 0);
  assert.deepEqual(serializeCustomProp({ a: 1 }), { a: 1 });
  assert.equal(serializeCustomProp(null), null);
});
