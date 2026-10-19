import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import {
  observeSelectedText,
  observeWindowInsets,
  observeWindowSize,
  readWindowInsets,
  readWindowSize,
} from "../src/index.js";
import { installFakeTime } from "./helpers.js";

let time;
beforeEach(() => {
  time = installFakeTime();
});
afterEach(() => time.restore());

const edges = (top = 0, right = 0, bottom = 0, left = 0) => ({ top, right, bottom, left });

test("size falls back to 800x600 when unavailable, throwing or non-positive", () => {
  const fallback = { width: 800, height: 600 };
  assert.deepEqual(readWindowSize(undefined), fallback);
  assert.deepEqual(readWindowSize({}), fallback);
  assert.deepEqual(readWindowSize({ getWindowSize: () => { throw new Error("x"); } }), fallback);
  assert.deepEqual(readWindowSize({ getWindowSize: () => ({ width: 0, height: 10 }) }), fallback);
  assert.deepEqual(readWindowSize({ getWindowSize: () => ({ width: 10, height: -1 }) }), fallback);
  assert.deepEqual(readWindowSize({ getWindowSize: () => ({ width: 300, height: 200 }) }), { width: 300, height: 200 });
});

test("insets derive keyboardTop, keyboardVisible and visibleHeight", () => {
  const renderer = {
    getWindowSize: () => ({ width: 400, height: 900 }),
    getWindowInsets: () => ({ safeArea: edges(30, 0, 20, 0), ime: edges(0, 0, 300, 0), effective: edges(30, 0, 300, 0) }),
  };
  const insets = readWindowInsets(renderer);
  assert.equal(insets.keyboardTop, 600);
  assert.equal(insets.keyboardVisible, true);
  assert.equal(insets.visibleHeight, 570);
  assert.deepEqual(insets.effective, edges(30, 0, 300, 0));
});

test("unavailable insets read zero edges against the fallback size; a size read before an insets failure is kept", () => {
  const zero = readWindowInsets(undefined);
  assert.deepEqual(zero.safeArea, edges());
  assert.equal(zero.keyboardTop, 600);
  assert.equal(zero.keyboardVisible, false);
  assert.equal(zero.visibleHeight, 600);
  const partial = readWindowInsets({
    getWindowSize: () => ({ width: 100, height: 500 }),
    getWindowInsets: () => { throw new Error("x"); },
  });
  assert.equal(partial.keyboardTop, 500);
  assert.equal(partial.visibleHeight, 500);
});

test("observing size delivers synchronously, suppresses equal values and reports changes", () => {
  let size = { width: 100, height: 100 };
  const renderer = { getWindowSize: () => size };
  const seen = [];
  const stop = observeWindowSize(renderer, (s) => seen.push(s));
  assert.deepEqual(seen, [{ width: 100, height: 100 }]);
  time.advance(300);
  assert.equal(seen.length, 1);
  size = { width: 120, height: 100 };
  time.advance(100);
  assert.deepEqual(seen[1], { width: 120, height: 100 });
  stop();
  size = { width: 5, height: 5 };
  time.advance(1000);
  assert.equal(seen.length, 2);
  assert.equal(time.pendingTimers, 0);
});

test("intervalMs false reads once with a no-op unsubscribe", () => {
  const seen = [];
  const stop = observeWindowSize({ getWindowSize: () => ({ width: 1, height: 1 }) }, (s) => seen.push(s), { intervalMs: false });
  assert.equal(seen.length, 1);
  assert.equal(time.pendingTimers, 0);
  stop();
});

test("intervals below 16 ms are raised to 16 and the default is 100", () => {
  let size = { width: 1, height: 1 };
  const renderer = { getWindowSize: () => size };
  const seen = [];
  observeWindowSize(renderer, (s) => seen.push(s), { intervalMs: 1 });
  size = { width: 2, height: 1 };
  time.advance(15);
  assert.equal(seen.length, 1);
  time.advance(1);
  assert.equal(seen.length, 2);

  const seen2 = [];
  observeWindowSize(renderer, (s) => seen2.push(s));
  size = { width: 3, height: 1 };
  time.advance(99);
  assert.equal(seen2.length, 1);
  time.advance(1);
  assert.equal(seen2.length, 2);
});

test("observing insets compares all three edge sets and the derived values", () => {
  let ime = edges();
  const renderer = {
    getWindowSize: () => ({ width: 400, height: 800 }),
    getWindowInsets: () => ({ safeArea: edges(), ime, effective: ime }),
  };
  const seen = [];
  observeWindowInsets(renderer, (i) => seen.push(i), { intervalMs: 20 });
  time.advance(100);
  assert.equal(seen.length, 1);
  ime = edges(0, 0, 250, 0);
  time.advance(20);
  assert.equal(seen.length, 2);
  assert.equal(seen[1].keyboardVisible, true);
  assert.equal(seen[1].keyboardTop, 550);
});

test("selected text: current value, then subscription values, then unsubscribe", () => {
  let listener = null;
  const subscribe = (fn) => {
    listener = fn;
    return () => {
      listener = null;
    };
  };
  const seen = [];
  const stop = observeSelectedText({ getSelectedText: () => "abc" }, subscribe, (t) => seen.push(t));
  assert.deepEqual(seen, ["abc"]);
  listener({ text: "def" });
  listener({});
  assert.deepEqual(seen, ["abc", "def", null]);
  stop();
  assert.equal(listener, null);
  const seen2 = [];
  observeSelectedText({}, subscribe, (t) => seen2.push(t));
  assert.deepEqual(seen2, [null]);
});
