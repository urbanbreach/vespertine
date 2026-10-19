import assert from "node:assert/strict";
import { test } from "node:test";
import { React, h } from "../src/h.js";
import { Editor, editorStats, layoutMetrics, generateProject } from "../src/editing/index.js";
import { allNodes, findByTestId, mount, textOf } from "./helpers.js";

const W = 1000;
const H = 700;
const project = generateProject({ seed: 1 });
const metrics = layoutMetrics({ viewportWidth: W, viewportHeight: H, tracks: project.tracks, collapsed: new Set(), duration: 230, scale: 24 });

function editor(props = {}) {
  return mount(h(Editor, { viewportWidth: W, viewportHeight: H, ...props }));
}

const readout = (ui, id) => textOf(ui.find(id));

test("defaults: offsets 0, scale 24, playhead 3.5, nothing selected, culling enabled", () => {
  const ui = editor();
  assert.equal(readout(ui, "readout-offset"), "offset 0,0");
  assert.equal(readout(ui, "readout-scale"), "scale 24.00");
  assert.equal(readout(ui, "readout-playhead"), "playhead 3.50");
  assert.equal(readout(ui, "readout-selection"), "selected none");
  const [mounted, total] = readout(ui, "readout-mounted").replace("clips ", "").split("/").map(Number);
  assert.equal(total, project.clips.length);
  assert.ok(mounted < total, "offscreen clips are not mounted");
  const grid = ui.find("grid");
  assert.deepEqual([grid.style.left, grid.style.top, grid.style.width, grid.style.height], [220, metrics.gridTop, 780, metrics.bodyHeight]);
  assert.equal(ui.find("ruler").style.height, 30);
  assert.ok(ui.find("plain-scroller"), "an ordinary native scroller coexists with the editing surface");
  assert.equal(ui.find("plain-scroller").style.overflow, "scroll");
  assert.equal(ui.find("grid-content").style.pointerEvents, "none");
  assert.equal(ui.find("headers-content").style.pointerEvents, "none");
  assert.equal(ui.find("ruler-ticks").style.pointerEvents, "none");
});

test("A1: two-axis and Shift panning move the three panes in the same commit", () => {
  const ui = editor();
  const before = ui.renderer.batches.length;
  ui.emit("grid", { type: "wheel", deltaX: -200, deltaY: -50, x: 500, y: 500 });
  assert.equal(ui.renderer.batches.length, before + 1, "one commit, one batch");
  assert.equal(readout(ui, "readout-offset"), "offset 200,50");
  const [ticks, headers, content] = ["ruler-ticks", "headers-content", "grid-content"].map((id) => ui.find(id).style);
  assert.equal(ticks.left, -200);
  assert.equal(content.left, -200);
  assert.equal(headers.top, -50);
  assert.equal(content.top, -50);
  const styled = ui.renderer.batches.at(-1).filter((op) => op[0] === "setStyle").map((op) => op[1]);
  assert.ok(styled.includes(ui.find("ruler-ticks").id) && styled.includes(ui.find("headers-content").id) && styled.includes(ui.find("grid-content").id));

  ui.emit("grid", { type: "wheel", deltaX: 0, deltaY: -100, x: 500, y: 500, shift: true });
  assert.equal(readout(ui, "readout-offset"), "offset 300,50", "Shift applies the negative vertical delta horizontally and never pans vertically");
  assert.equal(ui.find("headers-content").style.top, -50);
});

test("A1: pointer-centered zoom, exact clamps and the exp(0.5) step", () => {
  const ui = editor();
  ui.emit("grid", { type: "wheel", deltaX: -500, deltaY: 0, x: 600, y: 500 });
  assert.equal(readout(ui, "readout-offset").startsWith("offset 500"), true);
  const t = (600 - 220 + 500) / 24;
  ui.emit("grid", { type: "wheel", deltaX: 0, deltaY: 100, x: 600, y: 500, ctrl: true });
  assert.equal(readout(ui, "readout-scale"), `scale ${(24 * Math.exp(0.5)).toFixed(2)}`);
  const offset = Number(readout(ui, "readout-offset").match(/offset (\d+),/)[1]);
  const after = (600 - 220 + offset) / (24 * Math.exp(0.5));
  assert.ok(Math.abs(after - t) < 0.05, "the time under the pointer is unchanged up to rounding");
  for (let i = 0; i < 40; i++) ui.emit("grid", { type: "wheel", deltaY: 500, x: 600, y: 500, cmd: true });
  assert.equal(readout(ui, "readout-scale"), "scale 400.00");
  for (let i = 0; i < 40; i++) ui.emit("grid", { type: "wheel", deltaY: -500, x: 600, y: 500, ctrl: true });
  assert.equal(readout(ui, "readout-scale"), "scale 2.00");
});

test("A1: a captured drag keeps updating outside the grid and window and commits the move", () => {
  const ui = editor();
  const clip = project.clips.find((c) => c.trackId === "caption");
  const clipId = `clip-${clip.id}`;
  const left = 220 + clip.start * 24;
  const x = left + 20;
  const y = metrics.gridTop + 10;
  ui.emit(clipId, { type: "pointerDown", x, y, button: 0 });
  assert.match(readout(ui, "readout-selection"), new RegExp(`selected ${clip.id} caption`));
  ui.emit(clipId, { type: "pointerMove", x: x + 3, y });
  assert.match(readout(ui, "readout-history"), /dragstart:move/);
  ui.emit(clipId, { type: "pointerMove", x: x + 6000, y: -300 }); // far outside the grid and the window
  ui.emit(clipId, { type: "pointerMove", x: x + 48, y });
  ui.emit(clipId, { type: "pointerUp", x: x + 48, y });
  const selected = readout(ui, "readout-selection");
  const start = Number(selected.split(" ").at(-2));
  assert.ok(Math.abs(start - (clip.start + 2)) < 0.01 || start === 0 || Math.abs(start - clip.start - 2) < 7 / 24, `moved by 2 s (snapping allowed): ${selected}`);
  assert.match(readout(ui, "readout-history"), /dragend:move/);
});

test("A1: the ruler scrubs the playhead; empty grid starts a marquee", () => {
  const ui = editor();
  ui.emit("ruler", { type: "pointerDown", x: 220 + 240, y: metrics.gridTop - 10, button: 0 });
  assert.equal(readout(ui, "readout-playhead"), "playhead 10.00");
  ui.emit("ruler", { type: "pointerMove", x: 220 + 480, y: 3000 });
  assert.equal(readout(ui, "readout-playhead"), "playhead 20.00");
  ui.emit("ruler", { type: "pointerUp", x: 220 + 480, y: 3000 });
  ui.emit("grid", { type: "pointerDown", x: 225, y: metrics.gridTop + 2, button: 0 });
  ui.emit("grid", { type: "pointerMove", x: 900, y: metrics.gridTop + 80 });
  assert.ok(ui.find("marquee"), "the marquee overlay is shown while dragging");
  ui.emit("grid", { type: "pointerUp", x: 900, y: metrics.gridTop + 80 });
  assert.equal(ui.find("marquee"), null);
});

test("A1: collapsing a track keeps its compact header row and hides its clips", () => {
  const ui = editor();
  const captionClips = project.clips.filter((c) => c.trackId === "caption" && c.start < 780 / 24);
  assert.ok(captionClips.length > 0);
  assert.ok(ui.find(`clip-${captionClips[0].id}`), "the clip is mounted before the collapse");
  ui.emit("collapse-caption", { type: "click", x: 0, y: 0 });
  assert.equal(ui.find("header-caption").style.height, 18);
  assert.equal(ui.find(`clip-${captionClips[0].id}`), null);
  for (const clip of captionClips) assert.equal(ui.find(`clip-${clip.id}`), null);
});

test("A3: a pure pan with unchanged membership re-renders no clip layer and mutates no clip", () => {
  // A single track with long clips keeps the visible membership stable under a small pan.
  const long = mount(h(Editor, { viewportWidth: W, viewportHeight: H, trackCount: 2, durationSeconds: 230 }));
  const clipIds = new Set(allNodes(long.renderer.snapshot()).filter((n) => /^(clip|trim)-/.test(n.props.testId ?? "")).flatMap((n) => [n.id, ...allNodes(n).map((d) => d.id)]));
  assert.ok(clipIds.size > 0);
  const handlerOf = (id) => long.root.tree.handlers.get(id)?.get("pointerDown");
  const decorative = findByTestId(long.renderer, "clip-clip-0");
  const handler = handlerOf(decorative.id);
  assert.equal(typeof handler, "function");
  const layerRenders = editorStats.clipLayerRenders;
  const clipRenders = editorStats.clipRenders;
  const batches = long.renderer.batches.length;
  long.emit("grid", { type: "wheel", deltaX: -3, deltaY: 0, x: 500, y: 500 });
  long.emit("grid", { type: "wheel", deltaX: -3, deltaY: 0, x: 500, y: 500 });
  assert.equal(editorStats.clipLayerRenders, layerRenders, "no clip-layer re-render for a pure pan");
  assert.equal(editorStats.clipRenders, clipRenders);
  const touched = long.renderer.batches.slice(batches).flat().filter((op) => ["setStyle", "setText", "setCustomProp", "appendChild", "insertBefore", "destroyElement", "createElement"].includes(op[0]));
  const clipOps = touched.filter((op) => clipIds.has(op[1]));
  assert.deepEqual(clipOps, [], "no native mutation reaches a stable clip");
  assert.equal(touched.filter((op) => op[0] === "setStyle" && op[1] === long.find("grid-content").id).length, 2, "only the translating wrapper's offset changed");
  assert.equal(handlerOf(decorative.id), handler, "gesture handlers keep stable identities");
});

test("A1: gesture handlers are registered on every draggable surface itself", () => {
  const ui = editor();
  const clip = findByTestId(ui.renderer.snapshot(), `clip-${project.clips[0].id}`) ?? allNodes(ui.renderer.snapshot()).find((n) => /^clip-clip/.test(n.props.testId ?? ""));
  for (const id of [clip.props.testId, `trim-start-${clip.props.testId.slice(5)}`, `trim-end-${clip.props.testId.slice(5)}`, "ruler", "grid", "zoom-slider"]) {
    const node = ui.find(id);
    assert.deepEqual(node.listeners.filter((l) => l.startsWith("pointer")).sort(), ["pointerDown", "pointerMove", "pointerUp"], id);
  }
});

test("A2-style: culling off mounts every non-collapsed clip", () => {
  const culled = editor();
  const all = editor({ cull: false });
  const mounted = (ui) => Number(readout(ui, "readout-mounted").split("/")[0].replace("clips ", ""));
  assert.ok(mounted(culled) < mounted(all));
  assert.equal(mounted(all), project.clips.length);
});

test("the window-size hook drives the layout when no explicit size is given", () => {
  const ui = mount(h(Editor, {}));
  assert.equal(ui.find("editor").style.width, 800);
  assert.equal(ui.find("editor").style.height, 600);
  ui.renderer.windowSize = { width: 1000, height: 700 };
});
