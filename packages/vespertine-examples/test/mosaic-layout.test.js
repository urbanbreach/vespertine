import assert from "node:assert/strict";
import { test } from "node:test";
import { DEFAULT_LAYOUT, hitTest, layoutNode, squarify } from "../src/mosaic/layout.js";

const file = (name, size) => ({ name, size, directory: false });
const dir = (name, children) => ({ name, directory: true, children, size: children.reduce((s, c) => s + c.size, 0) });
const metric = (node) => node.size;

const overlap = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

test("squarify: areas are proportional, tile the rectangle exactly and never overlap", () => {
  const rects = squarify([5, 3, 1], 100, 100);
  const areas = rects.map((r) => r.w * r.h);
  const total = areas.reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(total - 10000) < 1e-6, "covers the whole area");
  assert.ok(Math.abs(areas[0] / 10000 - 5 / 9) < 1e-9);
  assert.ok(Math.abs(areas[1] / 10000 - 3 / 9) < 1e-9);
  assert.ok(Math.abs(areas[2] / 10000 - 1 / 9) < 1e-9);
  for (let i = 0; i < rects.length; i++) {
    for (let j = i + 1; j < rects.length; j++) assert.equal(overlap(rects[i], rects[j]), false);
    assert.ok(rects[i].x >= 0 && rects[i].y >= 0 && rects[i].x + rects[i].w <= 100 + 1e-9 && rects[i].y + rects[i].h <= 100 + 1e-9);
  }
});

test("squarify: degenerate inputs give empty rectangles and zero values are skipped", () => {
  assert.deepEqual(squarify([0, 0], 10, 10).map((r) => r.w * r.h), [0, 0]);
  assert.deepEqual(squarify([1], 0, 10).map((r) => r.w * r.h), [0]);
  assert.deepEqual(squarify([], 10, 10), []);
  const rects = squarify([4, 0, 4], 10, 10);
  assert.equal(rects[1].w * rects[1].h, 0);
  assert.ok(Math.abs(rects[0].w * rects[0].h - 50) < 1e-9);
});

test("squarify: a known layout (6, 6, 4, 3, 2, 2, 1 in 600 x 400)", () => {
  const rects = squarify([6, 6, 4, 3, 2, 2, 1], 600, 400);
  const sum = rects.reduce((s, r) => s + r.w * r.h, 0);
  assert.ok(Math.abs(sum - 240000) < 1e-6);
  const aspect = (r) => Math.max(r.w / r.h, r.h / r.w);
  assert.ok(rects.every((r) => aspect(r) < 4), "squarified tiles stay near square");
});

const area = { x: 0, y: 0, w: 600, h: 400 };

test("nonpositive values are omitted, order is descending with ties in original order", () => {
  const root = dir("root", [file("zero", 0), file("neg", -4), file("b", 10), file("a", 10), file("c", 30)]);
  const tiles = layoutNode(root, area, metric);
  assert.deepEqual(tiles.map((t) => t.node.name), ["c", "b", "a"]);
});

test("more than 96 children produce exactly 96 explicit tiles plus one aggregate with the remainder count", () => {
  const children = Array.from({ length: 130 }, (_, i) => file(`f${i}`, 1000 - i));
  const tiles = layoutNode(dir("root", children), { x: 0, y: 0, w: 4000, h: 3000 }, metric);
  assert.equal(tiles.filter((t) => t.aggregate === 0).length, 96);
  const aggregates = tiles.filter((t) => t.aggregate > 0);
  assert.equal(aggregates.length, 1);
  assert.equal(aggregates[0].aggregate, 34);
  assert.equal(aggregates[0].node, null);
  assert.equal(aggregates[0].header, 0);
});

test("tiles whose inset width or height is below 5 px are dropped", () => {
  // Two files splitting a 14 x 30 area: each ~14 x 15 -> inset by 3 -> 8 x 9 kept; a 10 x 30 area -> 4 wide: dropped.
  const root = dir("root", [file("a", 1), file("b", 1)]);
  assert.equal(layoutNode(root, { x: 0, y: 0, w: 14, h: 30 }, metric).length, 2);
  assert.equal(layoutNode(root, { x: 0, y: 0, w: 10, h: 30 }, metric).length, 0);
  const one = dir("root", [file("a", 1)]);
  assert.equal(layoutNode(one, { x: 0, y: 0, w: 10, h: 10 }, metric, { outerPadding: 3 }).length, 0, "an inset of 4 px is below the minimum");
  assert.equal(layoutNode(one, { x: 0, y: 0, w: 11, h: 11 }, metric, { outerPadding: 3 }).length, 1, "an inset of 5 px survives");
});

test("a directory gets a header only with inset width >= 44 and body height >= 15", () => {
  const inner = dir("inner", [file("x", 1), file("y", 1)]);
  const root = dir("root", [inner]);
  const width = (w) => layoutNode(root, { x: 0, y: 0, w: w + 6, h: 200 }, metric, { maxDepth: 3 });
  assert.equal(width(43)[0].header, 0, "inset width 43: whole tile, no nested tiles");
  assert.equal(width(43).length, 1);
  assert.equal(width(44)[0].header, 22);
  assert.ok(width(44).length > 1);
  const height = (h) => layoutNode(root, { x: 0, y: 0, w: 200, h: h + 6 }, metric, { maxDepth: 3 });
  assert.equal(height(22 + 14)[0].header, 0, "body height 14: no header");
  assert.equal(height(22 + 15)[0].header, 22, "body height 15: header");
});

test("depth setting 1 produces no headers or nested tiles; deeper levels use 1 px padding and 17 px headers", () => {
  const deep = dir("root", [dir("a", [dir("b", [file("c", 5), file("d", 5)]), file("e", 5)])]);
  const flat = layoutNode(deep, { x: 0, y: 0, w: 600, h: 400 }, metric, { maxDepth: 1 });
  assert.equal(flat.length, 1);
  assert.equal(flat[0].header, 0);
  const tiles = layoutNode(deep, { x: 0, y: 0, w: 600, h: 400 }, metric, { maxDepth: 3 });
  assert.deepEqual(tiles.map((t) => [t.node.name, t.depth, t.header]), [["a", 0, 22], ["b", 1, 17], ["c", 2, 0], ["d", 2, 0], ["e", 1, 0]]);
  const [a, b] = tiles;
  assert.equal(a.x, 3, "outer padding at depth 0");
  assert.equal(b.x, a.x + 1, "inner padding at depth 1 inside the parent's body");
  assert.equal(b.y, a.y + 22 + 1, "the body starts below the header band");
  for (const t of tiles) assert.ok(t.w >= 5 && t.h >= 5);
});

test("parents are emitted before their children (paint order) and tiles stay inside their parent", () => {
  const deep = dir("root", [dir("a", [file("b", 3), file("c", 1)]), dir("z", [file("y", 2), file("x", 2)])]);
  const tiles = layoutNode(deep, area, metric);
  const index = (name) => tiles.findIndex((t) => t.node?.name === name);
  assert.ok(index("a") < index("b") && index("a") < index("c"));
  assert.ok(index("z") < index("y"));
  const parent = tiles[index("a")];
  const child = tiles[index("b")];
  assert.ok(child.x >= parent.x && child.y >= parent.y + parent.header && child.x + child.w <= parent.x + parent.w + 1e-9);
});

test("hit testing walks the paint order backwards with half-open rectangles", () => {
  const tiles = [
    { x: 0, y: 0, w: 100, h: 100, name: "parent" },
    { x: 10, y: 10, w: 20, h: 20, name: "child" },
  ];
  assert.equal(hitTest(tiles, 15, 15).tile.name, "child", "the last emitted tile wins");
  assert.equal(hitTest(tiles, 10, 10).tile.name, "child", "the left/top edge is inside");
  assert.equal(hitTest(tiles, 30, 20).tile.name, "parent", "the right edge belongs to the neighbor");
  assert.equal(hitTest(tiles, 20, 30).tile.name, "parent", "the bottom edge is outside");
  assert.equal(hitTest(tiles, 100, 50), null);
  assert.equal(hitTest(tiles, 99.999, 99.999).tile.name, "parent");
  assert.equal(DEFAULT_LAYOUT.maxChildren, 96);
});
