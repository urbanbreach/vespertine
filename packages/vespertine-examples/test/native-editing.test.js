// APP-012 A1 and A3 on the native offscreen test host: the multi-pane editing surface through
// real wheel, pointer-capture and keyboard-free input, native layout and the recorded mutation
// batches.

import assert from "node:assert/strict";
import { h } from "../src/h.js";
import { Editor, editorStats, generateProject, layoutMetrics } from "../src/editing/index.js";
import { bounds, centerOf, createTestHost, find, native, need, recordingNative, text } from "./native-helpers.js";

const W = 1000;
const H = 700;
const project = generateProject({ seed: 1 });
const metrics = layoutMetrics({ viewportWidth: W, viewportHeight: H, tracks: project.tracks, collapsed: new Set(), duration: 230, scale: 24 });
const GRID_LEFT = 220;
const GRID_TOP = metrics.gridTop;

function mount(props = {}, options = {}) {
  const host = createTestHost({ width: W, height: H, ...options });
  host.render(h(Editor, { viewportWidth: W, viewportHeight: H, ...props }));
  return host;
}

const readout = (host, id) => text(host, id);
const offsets = (host) => readout(host, "readout-offset").match(/offset (-?\d+),(-?\d+)/).slice(1).map(Number);
const scale = (host) => Number(readout(host, "readout-scale").replace("scale ", ""));
const wheel = (host, x, y, dx, dy, modifiers) => host.renderer.nativeSimulateScrollWheel(x, y, dx, dy, modifiers);
const selected = (host) => {
  const [, id, track, start, duration] = readout(host, "readout-selection").split(" ");
  return { id, track, start: Number(start), duration: Number(duration) };
};

native("A1: two-axis and Shift panning move the ruler, the headers and the grid in the same batch and the same frame", () => {
  const { native: addon, batches } = recordingNative();
  const host = mount({}, { native: addon });
  const ids = ["ruler-ticks", "headers-content", "grid-content"].map((testId) => need(host, testId).id);
  const before = batches.length;
  wheel(host, 500, GRID_TOP + 50, -200, -50);
  assert.deepEqual(offsets(host), [200, 50], "the wheel deltas are subtracted from the offsets");
  assert.equal(batches.length, before + 1, "one commit, one batch");
  const styled = batches.at(-1).filter((op) => op[0] === "setStyle").map((op) => op[1]);
  for (const id of ids) assert.ok(styled.includes(id), `node ${id} moved in that batch`);
  // The native layout of the three panes agrees: the ruler ticks, the headers and the grid content
  // are translated by the same offsets.
  assert.equal(bounds(host, "ruler-ticks").x, GRID_LEFT - 200);
  assert.equal(bounds(host, "grid-content").x, GRID_LEFT - 200);
  assert.equal(bounds(host, "headers-content").y, GRID_TOP - 50);
  assert.equal(bounds(host, "grid-content").y, GRID_TOP - 50);

  wheel(host, 500, GRID_TOP + 50, 0, -100, "shift");
  assert.deepEqual(offsets(host), [300, 50], "Shift applies the negative vertical delta horizontally and never pans vertically");
  assert.equal(bounds(host, "headers-content").y, GRID_TOP - 50);
  assert.deepEqual(host.errors, []);
});

native("A1: the ruler and the header column take the wheel too", () => {
  const host = mount();
  wheel(host, GRID_LEFT + 100, GRID_TOP - 10, -100, 0);
  assert.deepEqual(offsets(host), [100, 0], "the ruler pans horizontally");
  wheel(host, 100, GRID_TOP + 20, 0, -40);
  assert.deepEqual(offsets(host), [100, 40], "the headers pan vertically");
});

native("A1: pointer-centered zoom keeps the time under the pointer, multiplies by exp(0.5) per 100 and clamps at 2 and 400", () => {
  const host = mount();
  wheel(host, 600, 500, -500, 0);
  assert.equal(offsets(host)[0], 500);
  const timeUnder = (offset, factor) => (600 - GRID_LEFT + offset) / factor;
  const before = timeUnder(500, 24);
  wheel(host, 600, 500, 0, 100, "ctrl");
  assert.equal(scale(host), Number((24 * Math.exp(0.5)).toFixed(2)));
  assert.ok(Math.abs(timeUnder(offsets(host)[0], 24 * Math.exp(0.5)) - before) < 0.05, "the time under the pointer is unchanged up to rounding");
  for (let i = 0; i < 40; i++) wheel(host, 600, 500, 0, 500, "ctrl");
  assert.equal(scale(host), 400);
  for (let i = 0; i < 40; i++) wheel(host, 600, 500, 0, -500, "ctrl");
  assert.equal(scale(host), 2);
  assert.deepEqual(host.errors, []);
});

native("A1: a captured drag keeps updating outside the grid and the window and commits the move", () => {
  const host = mount();
  const clip = project.clips.find((candidate) => candidate.trackId === "caption");
  const clipNode = need(host, `clip-${clip.id}`);
  const x = clipNode.bounds.x + 10;
  const y = clipNode.bounds.y + 10;
  const down = (px, py) => host.renderer.nativeSimulateMouseDown(px, py);
  down(x, y);
  assert.match(readout(host, "readout-selection"), new RegExp(`selected ${clip.id} caption`));
  host.renderer.nativeSimulateMouseMove(x + 3, y, 0);
  assert.match(readout(host, "readout-history"), /dragstart:move/);
  host.renderer.nativeSimulateMouseMove(x + 6000, -300, 0);
  host.renderer.nativeSimulateMouseUp(x + 6000, -300);
  assert.match(readout(host, "readout-history"), /dragend:move/);
  const moved = selected(host);
  assert.equal(moved.track, "caption", "above the first row selects the first track");
  assert.ok(Math.abs(moved.start - (230 - clip.duration)) < 0.01, `the clip ended flush with the project end: ${moved.start}`);
  assert.deepEqual(host.errors, []);
});

native("A1: a 3 px displacement does not commit a move and 4 px does", () => {
  const host = mount();
  const clip = project.clips.filter((candidate) => candidate.trackId === "caption").find((candidate) => candidate.start > 8 && candidate.start + candidate.duration < 30);
  const node = need(host, `clip-${clip.id}`);
  const x = node.bounds.x + 20;
  const y = node.bounds.y + 10;
  host.renderer.nativeSimulateMouseDown(x, y);
  host.renderer.nativeSimulateMouseMove(x + 3, y, 0);
  host.renderer.nativeSimulateMouseUp(x + 3, y);
  assert.equal(selected(host).start, Number(clip.start.toFixed(2)), "3 px is not a move");
  host.renderer.nativeSimulateMouseDown(x, y);
  host.renderer.nativeSimulateMouseMove(x + 4, y, 0);
  host.renderer.nativeSimulateMouseUp(x + 4, y);
  assert.notEqual(selected(host).start, Number(clip.start.toFixed(2)), "4 px commits the move");
  assert.deepEqual(host.errors, []);
});

native("A1: trimming snaps within 6 px inclusive and not at 6.01 px, never goes below 0.4 s and hides the guide when clamped", () => {
  const host = mount();
  const decorative = project.clips.find((candidate) => candidate.trackId === "track-10");
  // Zoom out to 2 px per second: the end handle of the full-length clip is then in view.
  for (let i = 0; i < 40; i++) wheel(host, 600, 500, 0, -500, "ctrl");
  assert.equal(scale(host), 2);
  const handle = need(host, `trim-end-${decorative.id}`);
  const pressX = handle.bounds.x + 3;
  const pressY = handle.bounds.y + 10;
  const drag = (toX) => {
    host.renderer.nativeSimulateMouseDown(pressX, pressY);
    host.renderer.nativeSimulateMouseMove(toX, pressY, 0);
    return () => host.renderer.nativeSimulateMouseUp(toX, pressY);
  };
  assert.equal(pressX, 677);
  // The playhead (3.5 s) is a snap candidate: the end edge 6.5 s is 6 px away at 2 px/s.
  let release = drag(230);
  assert.ok(find(host, "snap-guide"), "the guide is shown while a candidate is snapped to");
  release();
  assert.equal(selected(host).duration, 3.5, "6 px snaps (inclusive)");

  // Undo the trim by dragging the end back out, then 6.01 px away: no snap.
  const again = need(host, `trim-end-${decorative.id}`);
  // The 7 px clip is clipped to its 3 px content box by its border: press on its left edge.
  host.renderer.nativeSimulateMouseDown(again.bounds.x + 1, again.bounds.y + 10);
  host.renderer.nativeSimulateMouseMove(677, again.bounds.y + 10, 0);
  host.renderer.nativeSimulateMouseUp(677, again.bounds.y + 10);
  assert.equal(selected(host).duration, 230);
  release = drag(230.01);
  release();
  assert.ok(Math.abs(selected(host).duration - 6.505) <= 0.005, `6.01 px does not snap: ${selected(host).duration}`);

  // The minimum duration clamps the trim and the guide stays hidden.
  const last = need(host, `trim-end-${decorative.id}`);
  host.renderer.nativeSimulateMouseDown(last.bounds.x + 1, last.bounds.y + 10);
  host.renderer.nativeSimulateMouseMove(100, last.bounds.y + 10, 0);
  assert.equal(find(host, "snap-guide"), null, "no guide while the trim is clamped");
  host.renderer.nativeSimulateMouseUp(100, last.bounds.y + 10);
  assert.equal(selected(host).duration, 0.4, "a clip never gets shorter than 0.4 s");
  assert.deepEqual(host.errors, []);
});

native("A1: the ruler scrubs the playhead and keeps following outside the window", () => {
  const host = mount();
  const rulerY = GRID_TOP - 10;
  host.renderer.nativeSimulateMouseDown(GRID_LEFT + 240, rulerY);
  assert.equal(readout(host, "readout-playhead"), "playhead 10.00");
  host.renderer.nativeSimulateMouseMove(GRID_LEFT + 480, 3000, 0);
  assert.equal(readout(host, "readout-playhead"), "playhead 20.00");
  host.renderer.nativeSimulateMouseUp(GRID_LEFT + 480, 3000);
  host.renderer.nativeSimulateMouseMove(GRID_LEFT + 720, rulerY);
  assert.equal(readout(host, "readout-playhead"), "playhead 20.00", "the playhead stays after release");
  assert.deepEqual(host.errors, []);
});

native("A1: marquee selection replaces the selection, and Shift keeps it on an empty press", () => {
  const host = mount();
  const clip = project.clips.find((candidate) => candidate.trackId === "caption");
  const node = need(host, `clip-${clip.id}`);
  host.renderer.nativeSimulateClick(...centerOf(node.bounds));
  assert.match(readout(host, "readout-selection"), new RegExp(`selected ${clip.id}`));
  // An empty press without Shift clears the selection; with Shift it keeps it.
  host.renderer.nativeSimulateClick(GRID_LEFT + 2, GRID_TOP + 1);
  assert.equal(readout(host, "readout-selection"), "selected none");
  host.renderer.nativeSimulateClick(...centerOf(node.bounds));
  host.renderer.nativeSimulateClick(GRID_LEFT + 2, GRID_TOP + 1, 0, "shift");
  assert.match(readout(host, "readout-selection"), new RegExp(`selected ${clip.id}`));
  // A dragged marquee shows while dragging and selects the clips it overlaps.
  host.renderer.nativeSimulateMouseDown(GRID_LEFT + 2, GRID_TOP + 1);
  host.renderer.nativeSimulateMouseMove(900, GRID_TOP + 120, 0);
  assert.ok(find(host, "marquee"), "the marquee is shown while dragging");
  host.renderer.nativeSimulateMouseUp(900, GRID_TOP + 120);
  assert.equal(find(host, "marquee"), null);
  assert.notEqual(readout(host, "readout-selection"), "selected none", "the clips under the marquee are selected");
  assert.deepEqual(host.errors, []);
});

native("A1: collapsing a track keeps its compact header row and hides its clips", () => {
  const host = mount();
  const first = project.clips.find((candidate) => candidate.trackId === "caption" && candidate.start < 780 / 24);
  assert.ok(find(host, `clip-${first.id}`));
  host.renderer.nativeSimulateClick(...centerOf(need(host, "collapse-caption").bounds));
  assert.equal(bounds(host, "header-caption").height, 18);
  assert.equal(find(host, `clip-${first.id}`), null);
  host.renderer.nativeSimulateClick(...centerOf(need(host, "collapse-caption").bounds));
  assert.equal(bounds(host, "header-caption").height, 30);
  assert.ok(find(host, `clip-${first.id}`), "the clips return with the row");
});

native("A3: a pure pan with unchanged membership re-renders no clip layer and mutates no clip natively", () => {
  const { native: addon, batches } = recordingNative();
  const host = mount({ trackCount: 2, durationSeconds: 230 }, { native: addon });
  const clipIds = new Set(
    ["div", "text"].flatMap((type) => host.renderer.findByType(type)).filter((node) => /^(clip|trim)-/.test(node.testId ?? "")).map((node) => node.id),
  );
  assert.ok(clipIds.size > 0);
  const before = host.renderer.findByType("div").filter((node) => /^clip-/.test(node.testId ?? "")).map((node) => [node.testId, node.id]);
  const gridId = need(host, "grid-content").id;
  const layerRenders = editorStats.clipLayerRenders;
  const clipRenders = editorStats.clipRenders;
  const batchCount = batches.length;
  const gridY = bounds(host, "grid-content").y + 20;
  wheel(host, 500, gridY, -3, 0);
  wheel(host, 500, gridY, -3, 0);
  assert.equal(editorStats.clipLayerRenders, layerRenders, "no clip-layer re-render for a pure pan");
  assert.equal(editorStats.clipRenders, clipRenders);
  const operations = batches.slice(batchCount).flat();
  assert.deepEqual(operations.filter((op) => clipIds.has(op[1])), [], "no native mutation reaches a stable clip");
  assert.equal(operations.filter((op) => op[0] === "setStyle" && op[1] === gridId).length, 2, "only the translating wrapper's offset changed");
  const after = host.renderer.findByType("div").filter((node) => /^clip-/.test(node.testId ?? "")).map((node) => [node.testId, node.id]);
  assert.deepEqual(after, before, "the same native elements stay mounted");
  assert.equal(bounds(host, "grid-content").x, GRID_LEFT - 6, "the pan moved the wrapper natively");
  assert.deepEqual(host.errors, []);
});
