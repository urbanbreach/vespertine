import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { h } from "../src/h.js";
import { Mosaic, PROGRESS_INTERVAL_MS, createFixtureScanner, createMosaicStore, generateFixtureTree, hitTest, layoutNode, metricFor, freshNav } from "../src/mosaic/index.js";
import { allNodes, flushForTest, mount, textOf } from "./helpers.js";

const WIDTH = 800;
const HEIGHT = 628;

const file = (name, size, mtime = 1_700_000_000_000) => ({ name, directory: false, allocated: size, apparent: size / 2, files: 1, mtime });
const dir = (name, children) => ({
  name,
  directory: true,
  allocated: children.reduce((s, c) => s + c.allocated, 0),
  apparent: children.reduce((s, c) => s + c.apparent, 0),
  files: children.reduce((s, c) => s + c.files, 0),
  mtime: 1_700_000_000_000,
  children,
});
const sample = () => dir("root", [dir("alpha", [file("a1", 4000), file("a2", 3000), dir("deep", [file("d1", 1000), file("d2", 900)])]), dir("beta", [file("b1", 2000), file("b2", 1500)]), file("loose", 800)]);

/** A scanner whose scans resolve when the test says so. */
function controlledSource() {
  const scans = [];
  const scan = ({ signal, counters, includeHidden }) =>
    new Promise((resolve, reject) => {
      const entry = { counters, includeHidden, signal, resolve: (tree) => resolve(tree), reject };
      signal.addEventListener("abort", () => reject(Object.assign(new Error("aborted"), { name: "AbortError" })));
      scans.push(entry);
    });
  return { scan, scans };
}

let intervals;
const real = { setInterval: globalThis.setInterval, clearInterval: globalThis.clearInterval };
beforeEach(() => {
  const timers = new Map();
  let next = 1;
  intervals = { timers, tick: () => [...timers.values()].forEach((t) => t.fn()) };
  globalThis.setInterval = (fn, ms) => (timers.set(next, { fn, ms }), next++);
  globalThis.clearInterval = (id) => timers.delete(id);
});
afterEach(() => Object.assign(globalThis, real));

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

async function ready(tree = sample()) {
  const source = controlledSource();
  const store = createMosaicStore({ scan: source.scan });
  store.startScan();
  source.scans[0].resolve(tree);
  await settle();
  const ui = mount(h(Mosaic, { store, width: WIDTH, height: HEIGHT, now: 1_700_000_000_000 }));
  ui.emit("mosaic-area", { type: "layout", x: 0, y: 28, width: WIDTH, height: HEIGHT - 28 });
  return { ...ui, store, source, tree };
}

const tilesOf = (tree, nav = freshNav(tree), depth = 3) => layoutNode(tree, { x: 0, y: 0, w: WIDTH, h: HEIGHT - 28 }, metricFor(nav), { maxDepth: depth });
const centre = (tile) => ({ x: tile.x + tile.w / 2, y: tile.y + tile.h / 2 + 28 });
const details = (ui, id) => textOf(ui.find(id));

test("A2: scanning publishes progress every 100 ms from the live counters and keeps the previous tree while a rescan runs", async () => {
  const source = controlledSource();
  const store = createMosaicStore({ scan: source.scan });
  store.startScan();
  assert.deepEqual([...intervals.timers.values()].map((t) => t.ms), [PROGRESS_INTERVAL_MS]);
  assert.equal(store.getSnapshot().scanning, true);
  source.scans[0].counters.files = 5;
  assert.equal(store.getSnapshot().progress.files, 0, "nothing is published between ticks");
  intervals.tick();
  assert.equal(store.getSnapshot().progress.files, 5);
  const tree = sample();
  source.scans[0].resolve(tree);
  await settle();
  assert.equal(store.getSnapshot().tree, tree);
  assert.equal(intervals.timers.size, 0, "the timer stops with the scan");
  store.startScan();
  assert.equal(store.getSnapshot().tree, tree, "the previous completed tree stays displayed during a rescan");
  assert.equal(store.getSnapshot().scanning, true);
});

test("A2: a restarted scan aborts the previous one and ignores its late completion and error", async () => {
  const source = controlledSource();
  const store = createMosaicStore({ scan: source.scan });
  store.startScan();
  store.startScan();
  assert.equal(source.scans[0].signal.aborted, true);
  const newer = sample();
  source.scans[1].resolve(newer);
  await settle();
  source.scans[0].resolve(sample());
  source.scans[0].reject(new Error("late"));
  await settle();
  assert.equal(store.getSnapshot().tree, newer);
  assert.equal(store.getSnapshot().error, null);
  store.startScan();
  source.scans[2].reject(new Error("failed"));
  await settle();
  assert.equal(store.getSnapshot().error.message, "failed");
  assert.equal(store.getSnapshot().tree, newer, "an error keeps the last tree");
});

test("A2: the generated fixture scanner counts and yields a fresh tree each scan", async () => {
  const tree = generateFixtureTree({ seed: 3, depth: 3, breadth: 3 });
  const scan = createFixtureScanner({ tree, chunk: 4 });
  const counters = { files: 0, directories: 0, bytes: 0, errors: 0 };
  const first = await scan({ signal: new AbortController().signal, counters });
  assert.ok(counters.files > 0 && counters.directories > 0);
  assert.equal(counters.files, tree.files);
  const second = await scan({ signal: new AbortController().signal, counters: { files: 0, directories: 0, bytes: 0, errors: 0 } });
  assert.notEqual(first, second);
  assert.deepEqual(first, second);
  const controller = new AbortController();
  const pending = scan({ signal: controller.signal, counters });
  controller.abort();
  await assert.rejects(pending, /aborted/);
});

test("A2: one primary click on a folder tile enters it; a file tile selects the file in its folder", async () => {
  const ui = await ready();
  let tiles = tilesOf(ui.tree);
  const alpha = tiles.find((t) => t.node?.name === "alpha");
  ui.emit("mosaic-area", { type: "pointerDown", ...centre({ ...alpha, x: alpha.x + 1, y: alpha.y + alpha.header + 1, w: 1, h: 1 }), button: 1 });
  assert.equal(details(ui, "details-path"), "/", "other buttons are ignored");
  // Press on the alpha folder's header band: the folder tile itself is topmost there.
  ui.emit("mosaic-area", { type: "pointerDown", x: alpha.x + 5, y: alpha.y + 28 + 3, button: 0 });
  assert.equal(details(ui, "details-path"), "0");
  tiles = tilesOf(ui.tree.children[0], freshNav(ui.tree), 3);
  const a1 = tiles.find((t) => t.node.name === "a1");
  ui.emit("mosaic-area", { type: "pointerDown", ...centre(a1), button: 0 });
  assert.equal(details(ui, "details-path"), "0", "a file press navigates to its containing folder");
  assert.equal(details(ui, "details-name"), "a1", "and selects the file");
  assert.ok(ui.find("tile-selection"));
  ui.emit("mosaic-area", { type: "pointerDown", x: 799, y: 28 + 1, button: 0 });
  assert.equal(ui.find("tile-selection"), null, "empty space clears the selection");
});

test("A2: Backspace and Escape return up with the folder just left selected; Enter opens the selection", async () => {
  const ui = await ready();
  const alpha = tilesOf(ui.tree).find((t) => t.node?.name === "alpha");
  ui.emit("mosaic-area", { type: "pointerDown", x: alpha.x + 5, y: alpha.y + 28 + 3, button: 0 });
  assert.equal(details(ui, "details-path"), "0");
  ui.emit("mosaic", { type: "keyDown", key: "backspace" });
  assert.equal(details(ui, "details-path"), "/");
  assert.equal(details(ui, "details-name"), "alpha", "the folder just left is selected");
  ui.emit("mosaic", { type: "keyDown", key: "escape" });
  assert.equal(details(ui, "details-path"), "/", "no effect at the root");
  ui.emit("mosaic", { type: "keyDown", key: "enter" });
  assert.equal(details(ui, "details-path"), "0", "Enter enters the selected folder");
  ui.emit("mosaic", { type: "keyDown", key: "escape" });
  assert.equal(details(ui, "details-path"), "/");
});

test("A2: bracket keys clamp the depth to 1 and 6; t cycles size, files, age", async () => {
  const ui = await ready();
  const mode = () => details(ui, "details-mode");
  assert.equal(mode(), "size depth 3");
  for (let i = 0; i < 10; i++) ui.emit("mosaic", { type: "keyDown", key: "[" });
  assert.equal(mode(), "size depth 1");
  for (let i = 0; i < 10; i++) ui.emit("mosaic", { type: "keyDown", key: "]" });
  assert.equal(mode(), "size depth 6");
  ui.emit("mosaic", { type: "keyDown", key: "t" });
  assert.equal(mode(), "files depth 6");
  ui.emit("mosaic", { type: "keyDown", key: "t" });
  assert.equal(mode(), "age depth 6");
  ui.emit("mosaic", { type: "keyDown", key: "t" });
  assert.equal(mode(), "size depth 6");
});

test("A2: r restarts a scan, ignoring the earlier scan's late completion; a new tree resets navigation", async () => {
  const ui = await ready();
  const alpha = tilesOf(ui.tree).find((t) => t.node?.name === "alpha");
  ui.emit("mosaic-area", { type: "pointerDown", x: alpha.x + 5, y: alpha.y + 28 + 3, button: 0 });
  assert.equal(details(ui, "details-path"), "0");
  ui.emit("mosaic", { type: "keyDown", key: "r" });
  ui.emit("mosaic", { type: "keyDown", key: "r" });
  assert.equal(ui.source.scans.length, 3);
  assert.equal(details(ui, "details-path"), "0", "the old tree and navigation stay during a rescan");
  assert.match(details(ui, "details-progress"), /scanning/);
  const fresh = sample();
  ui.source.scans[2].resolve(fresh);
  await settle();
  flushForTest();
  assert.equal(details(ui, "details-path"), "/", "navigation resets when a new tree arrives");
  ui.source.scans[1].resolve(sample());
  await settle();
  assert.equal(ui.store.getSnapshot().tree, fresh, "the earlier scan's late completion is ignored");
  assert.equal(details(ui, "details-progress"), "idle");
  ui.emit("mosaic", { type: "keyDown", key: "h" });
  assert.equal(ui.source.scans.at(-1).includeHidden, false, "toggling hidden entries rescans");
});

test("A2: hover ignores the aggregate tail tile and updates the outline without regenerating the tiles", async () => {
  const children = Array.from({ length: 100 }, (_, i) => file(`f${i}`, 1000 - i));
  const ui = await ready(dir("root", children));
  const area = { x: 0, y: 0, w: WIDTH, h: HEIGHT - 28 };
  const tiles = layoutNode(ui.tree, area, metricFor(freshNav(ui.tree)), { maxDepth: 3 });
  assert.equal(tiles.filter((t) => t.aggregate > 0).length, 1);
  const tail = tiles.find((t) => t.aggregate > 0);
  const regular = tiles.find((t) => t.aggregate === 0);
  const renderersBefore = ui.renderer.count("createElement");
  ui.emit("mosaic-area", { type: "pointerMove", ...centre(regular) });
  assert.ok(ui.find("tile-hover"));
  assert.equal(ui.renderer.count("createElement") - renderersBefore, 1, "only the outline was created; no tile was regenerated");
  ui.emit("mosaic-area", { type: "pointerMove", ...centre(tail) });
  assert.equal(ui.find("tile-hover"), null, "hover clears on the aggregate tail");
});

test("A2: tiles are absolutely positioned with pointer events disabled and in paint order", async () => {
  const ui = await ready();
  const tiles = allNodes(ui.renderer.snapshot()).filter((n) => /^tile-\d+$/.test(n.props.testId ?? ""));
  assert.ok(tiles.length > 3);
  assert.ok(tiles.every((t) => t.style.position === "absolute" && t.style.pointerEvents === "none"));
  assert.equal(ui.find("mosaic-area").listeners.includes("pointerDown"), true);
  assert.equal(hitTest(tilesOf(ui.tree), -1, 0), null);
});
