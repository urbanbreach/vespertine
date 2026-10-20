// APP-011 A2 on the native offscreen test host: the mosaic through real pointer and keyboard
// input, native layout and hit testing, with scans resolved by the test.

import assert from "node:assert/strict";
import { h } from "../src/h.js";
import { Mosaic, createMosaicStore, freshNav, layoutNode, metricFor } from "../src/mosaic/index.js";
import { bounds, createTestHost, find, native, need, settle, text } from "./native-helpers.js";

const WIDTH = 800;
const HEIGHT = 628;
const DETAILS = 28;

const file = (name, size, mtime = 1_700_000_000_000) => ({ name, directory: false, allocated: size, apparent: size / 2, files: 1, mtime });
const dir = (name, children) => ({
  name,
  directory: true,
  allocated: children.reduce((sum, child) => sum + child.allocated, 0),
  apparent: children.reduce((sum, child) => sum + child.apparent, 0),
  files: children.reduce((sum, child) => sum + child.files, 0),
  mtime: 1_700_000_000_000,
  children,
});
const sample = () => dir("root", [dir("alpha", [file("a1", 4000), file("a2", 3000), dir("deep", [file("d1", 1000), file("d2", 900)])]), dir("beta", [file("b1", 2000), file("b2", 1500)]), file("loose", 800)]);

/** A scanner whose scans resolve when the test says so. */
function controlledSource() {
  const scans = [];
  const scan = ({ signal, counters, includeHidden }) =>
    new Promise((resolve, reject) => {
      signal.addEventListener("abort", () => reject(Object.assign(new Error("aborted"), { name: "AbortError" })));
      scans.push({ counters, includeHidden, signal, resolve: (tree) => resolve(tree), reject });
    });
  return { scan, scans };
}

async function ready(tree = sample()) {
  const source = controlledSource();
  const store = createMosaicStore({ scan: source.scan });
  store.startScan();
  source.scans[0].resolve(tree);
  const host = createTestHost({ width: WIDTH, height: HEIGHT });
  await settle(host);
  host.render(h(Mosaic, { store, width: WIDTH, height: HEIGHT, now: 1_700_000_000_000 }));
  return { host, store, source, tree };
}

const tilesOf = (tree, depth = 3) => layoutNode(tree, { x: 0, y: 0, w: WIDTH, h: HEIGHT - DETAILS }, metricFor(freshNav(tree)), { maxDepth: depth });
const centre = (tile) => [tile.x + tile.w / 2, tile.y + tile.h / 2 + DETAILS];
const details = (host, id) => text(host, id);
const press = (host, x, y) => host.renderer.nativeSimulateClick(x, y);
const keys = (host, sequence) => host.renderer.nativeSimulateKeystrokes(sequence);

native("A2: the native tiles are laid out where the layout puts them, parent before child, and take no pointer events themselves", async () => {
  const { host, tree } = await ready();
  const area = bounds(host, "mosaic-area");
  assert.deepEqual([area.x, area.y, area.width, area.height], [0, DETAILS, WIDTH, HEIGHT - DETAILS]);
  const tiles = tilesOf(tree);
  assert.ok(tiles.length > 3);
  // Native layout snaps edges to the device pixel grid (half a logical pixel at 2x).
  const near = (a, b) => Math.abs(a - b) <= 0.5;
  tiles.forEach((tile, index) => {
    const placed = bounds(host, `tile-${index}`);
    assert.ok(near(placed.x, tile.x) && near(placed.y, tile.y + DETAILS), `tile ${index} position`);
    assert.ok(near(placed.width, tile.w) && near(placed.height, tile.h), `tile ${index} size`);
  });
  // The container receives the pointer for the tiles beneath it: a move over a tile outlines it.
  const [x, y] = centre(tiles[1]);
  assert.ok(host.renderer.getElementsAt(x, y).includes(need(host, "mosaic-area").id));
  host.renderer.nativeSimulateMouseMove(x, y);
  assert.ok(find(host, "tile-hover"), "the container hit-tested the tile under the pointer");
  assert.deepEqual(host.errors, []);
});

native("A2: one primary click on a folder tile enters it; a click on a file selects it in its folder; empty space clears the selection", async () => {
  const { host, tree } = await ready();
  const alpha = tilesOf(tree).find((tile) => tile.node?.name === "alpha");
  host.renderer.nativeSimulateClick(alpha.x + 5, alpha.y + DETAILS + 3, 2);
  assert.equal(details(host, "details-path"), "/", "another button is ignored");
  press(host, alpha.x + 5, alpha.y + DETAILS + 3);
  assert.equal(details(host, "details-path"), "0", "the folder was entered");
  const a1 = tilesOf(tree.children[0]).find((tile) => tile.node.name === "a1");
  press(host, ...centre(a1));
  assert.equal(details(host, "details-path"), "0");
  assert.equal(details(host, "details-name"), "a1", "the file is selected in its folder");
  assert.ok(find(host, "tile-selection"));
  press(host, 1, DETAILS + 1);
  assert.equal(find(host, "tile-selection"), null, "empty space clears the selection");
  assert.deepEqual(host.errors, []);
});

native("A2: a tile inside a folder tile wins the press (reverse paint order); edges are half-open", async () => {
  const { host, tree } = await ready();
  const tiles = tilesOf(tree);
  const alpha = tiles.find((tile) => tile.node?.name === "alpha");
  const nested = tiles.find((tile) => tile.node?.name === "a1");
  assert.ok(nested.x >= alpha.x && nested.x + nested.w <= alpha.x + alpha.w, "the file tile lies inside the folder tile");
  press(host, ...centre(nested));
  assert.equal(details(host, "details-name"), "a1", "the later-painted tile under the pointer is hit, not its folder");

  const edge = await ready();
  const top = tilesOf(edge.tree).find((tile) => tile.node?.name === "alpha");
  const hover = (x, y) => edge.host.renderer.nativeSimulateMouseMove(x, y);
  hover(top.x, top.y + DETAILS);
  assert.equal(details(edge.host, "details-name"), "alpha", "the left and top edges belong to the tile");
  hover(top.x - 0.01, top.y + DETAILS + 10);
  assert.notEqual(details(edge.host, "details-name"), "alpha", "just outside the left edge is another tile or none");
  hover(top.x + top.w - 0.01, top.y + DETAILS + 3);
  assert.equal(details(edge.host, "details-name"), "alpha", "just inside the right edge belongs to the tile");
  hover(top.x + top.w + 0.01, top.y + DETAILS + 3);
  assert.notEqual(details(edge.host, "details-name"), "alpha", "the right edge is exclusive");
  assert.deepEqual(host.errors, []);
});

native("A2: Backspace and Escape return up with the folder just left selected; Enter opens the selection", async () => {
  const { host, tree } = await ready();
  const alpha = tilesOf(tree).find((tile) => tile.node?.name === "alpha");
  press(host, alpha.x + 5, alpha.y + DETAILS + 3);
  assert.equal(details(host, "details-path"), "0");
  keys(host, "backspace");
  assert.equal(details(host, "details-path"), "/");
  assert.equal(details(host, "details-name"), "alpha", "the folder just left is selected");
  keys(host, "escape");
  assert.equal(details(host, "details-path"), "/", "no effect at the root");
  keys(host, "enter");
  assert.equal(details(host, "details-path"), "0", "Enter enters the selected folder");
  keys(host, "escape");
  assert.equal(details(host, "details-path"), "/");
  assert.deepEqual(host.errors, []);
});

native("A2: bracket keys clamp the depth to 1 and 6 and t cycles size, files, age", async () => {
  const { host } = await ready();
  const mode = () => details(host, "details-mode");
  assert.equal(mode(), "size depth 3");
  keys(host, Array(10).fill("[").join(" "));
  assert.equal(mode(), "size depth 1");
  keys(host, Array(10).fill("]").join(" "));
  assert.equal(mode(), "size depth 6");
  keys(host, "t");
  assert.equal(mode(), "files depth 6");
  keys(host, "t");
  assert.equal(mode(), "age depth 6");
  keys(host, "t");
  assert.equal(mode(), "size depth 6");
  assert.deepEqual(host.errors, []);
});

native("A2: progress reaches the store on a 100 ms timer; r restarts a scan and the earlier scan's late completion is ignored; a new tree resets navigation", async () => {
  const { host, store, source, tree } = await ready();
  const alpha = tilesOf(tree).find((tile) => tile.node?.name === "alpha");
  press(host, alpha.x + 5, alpha.y + DETAILS + 3);
  assert.equal(details(host, "details-path"), "0");
  keys(host, "r r");
  assert.equal(source.scans.length, 3);
  assert.equal(details(host, "details-path"), "0", "the old tree and navigation stay during a rescan");
  assert.match(details(host, "details-progress"), /scanning/);
  source.scans[2].counters.files = 7;
  assert.match(details(host, "details-progress"), /0 files/, "nothing is published between ticks");
  await settle(host, 160);
  assert.match(details(host, "details-progress"), /scanning 7 files/, "the interval timer copied the live counters");
  const fresh = sample();
  source.scans[2].resolve(fresh);
  await settle(host);
  assert.equal(details(host, "details-path"), "/", "navigation resets when a new tree arrives");
  source.scans[1].resolve(sample());
  await settle(host);
  assert.equal(store.getSnapshot().tree, fresh, "the earlier scan's late completion is ignored");
  assert.equal(details(host, "details-progress"), "idle");
  keys(host, "h");
  assert.equal(source.scans.at(-1).includeHidden, false, "toggling hidden entries rescans");
  assert.deepEqual(host.errors, []);
});

native("A2: hovering ignores the aggregate tail tile and updates only the outline", async () => {
  const children = Array.from({ length: 100 }, (_, index) => file(`f${index}`, 1000 - index));
  const { host, tree } = await ready(dir("root", children));
  const tiles = layoutNode(tree, { x: 0, y: 0, w: WIDTH, h: HEIGHT - DETAILS }, metricFor(freshNav(tree)), { maxDepth: 3 });
  const tail = tiles.find((tile) => tile.aggregate > 0);
  const regular = tiles.find((tile) => tile.aggregate === 0);
  assert.equal(tiles.filter((tile) => tile.aggregate > 0).length, 1, "96 explicit tiles plus one aggregate tile");
  const before = host.renderer.getRetainedElementCount();
  host.renderer.nativeSimulateMouseMove(...centre(regular));
  assert.ok(find(host, "tile-hover"));
  assert.equal(host.renderer.getRetainedElementCount() - before, 1, "only the outline was created");
  host.renderer.nativeSimulateMouseMove(...centre(tail));
  assert.equal(find(host, "tile-hover"), null, "hover clears on the aggregate tail");
  assert.deepEqual(host.errors, []);
});
