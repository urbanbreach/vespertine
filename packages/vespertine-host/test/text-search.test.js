import assert from "node:assert/strict";
import { test } from "node:test";
import { createSearchCursor, createTextSearchController, findRanges } from "../src/index.js";

test("matching is case-insensitive by default, case-sensitive on request, and never overlaps", () => {
  assert.deepEqual(findRanges("Find red, red and RED.", "red"), [[5, 8], [10, 13], [18, 21]]);
  assert.deepEqual(findRanges("Find red, red and RED.", "red", { caseSensitive: true }), [[5, 8], [10, 13]]);
  assert.deepEqual(findRanges("aaaa", "aa"), [[0, 2], [2, 4]]);
  assert.deepEqual(findRanges("abc", ""), []);
});

test("whole-word matching uses Unicode alphanumerics and underscore as word characters", () => {
  assert.deepEqual(findRanges("foo_bar foo", "foo", { wholeWord: true }), [[8, 11]]);
  assert.deepEqual(findRanges("foo-bar (foo) café", "foo", { wholeWord: true }), [[0, 3], [9, 12]]);
  assert.deepEqual(findRanges("naïve naïvety", "naïve", { wholeWord: true }), [[0, 5]]);
});

test("lowercase expansion maps offsets back to the original text", () => {
  const dotted = "\u0130";
  assert.deepEqual(findRanges(`a${dotted}b`, "i"), [], "a query i does not match inside U+0130");
  assert.deepEqual(findRanges(`a${dotted}b`, "i\u0307"), [[1, 2]], "i followed by U+0307 matches the whole character");
  assert.deepEqual(findRanges("\ufb00 ff", "ff"), [[2, 4]], "the ligature does not match ff (no case folding)");
});

test("astral characters count as two UTF-16 units", () => {
  assert.deepEqual(findRanges("a\u{1F600}b", "b"), [[3, 4]]);
  assert.deepEqual(findRanges("\u{1F600}x\u{1F600}x", "x", { wholeWord: true }), [[2, 3], [5, 6]]);
});

test("groups are joined by one newline and matches never cross a group boundary", () => {
  assert.deepEqual(findRanges(["abc", "bcd"], "bc"), [[1, 3], [4, 6]]);
  assert.deepEqual(findRanges(["ab", "cd"], "bc"), []);
  assert.deepEqual(findRanges(["Hello ", "x"], "lo"), [[3, 5]]);
});

test("the cursor wraps, clamps when the count shrinks and resets", () => {
  const cursor = createSearchCursor(3);
  assert.deepEqual([cursor.next(), cursor.next(), cursor.next(), cursor.next()], [1, 2, 0, 1]);
  assert.equal(cursor.previous(), 0);
  assert.equal(cursor.previous(), 2);
  cursor.setTotal(2);
  assert.equal(cursor.index, 1, "clamped to the last match");
  cursor.setTotal(0);
  assert.equal(cursor.next(), 0);
  cursor.setTotal(5);
  assert.equal(cursor.goTo(9), 4);
  assert.equal(cursor.goTo(-3), 0);
  cursor.reset();
  assert.equal(cursor.index, 0);
});

test("the controller reports totals from the native count, wraps navigation and resets on a new query", () => {
  const controller = createTextSearchController();
  let revisions = 0;
  const unsubscribe = controller.subscribe(() => revisions++);
  let result = controller.snapshot({ query: "red" });
  assert.deepEqual(result.props.highlight, { query: "red", activeIndex: 0 });
  assert.equal(result.total, 0);
  result.props.onHighlight({ matchCount: 3 });
  assert.equal(revisions, 1);
  result = controller.snapshot({ query: "red" });
  assert.equal(result.total, 3);
  result.next();
  result.next();
  result.next();
  assert.equal(controller.snapshot({ query: "red" }).active, 0, "wrapped");
  result.previous();
  assert.equal(controller.snapshot({ query: "red" }).active, 2);
  result.goTo(1);
  const same = controller.snapshot({ query: "red" });
  assert.equal(same.active, 1);
  assert.equal(same.props, controller.snapshot({ query: "red" }).props, "props are stable while nothing changes");
  assert.equal(controller.snapshot({ query: "re" }).active, 0, "a new query resets the cursor");
  const count = revisions;
  result.props.onHighlight({ matchCount: 3 });
  assert.equal(revisions, count, "an unchanged count does not notify");
  unsubscribe();
  assert.equal(controller.listenerCount, 0);
});
