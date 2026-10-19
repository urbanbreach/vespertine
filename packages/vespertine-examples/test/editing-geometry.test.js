import assert from "node:assert/strict";
import { test } from "node:test";
import {
  applyWheel,
  generateProject,
  initialState,
  layoutMetrics,
  marqueeSelection,
  metricsOf,
  previewClip,
  reduce,
  scaleToSlider,
  sliderToScale,
  snapTime,
  tickStep,
  visibleClips,
} from "../src/editing/index.js";

const project = {
  duration: 100,
  tracks: [
    { id: "t1", kind: "numbered" },
    { id: "t2", kind: "numbered" },
    { id: "t3", kind: "audio" },
  ],
  clips: [
    { id: "a", trackId: "t1", start: 10, duration: 5 },
    { id: "b", trackId: "t1", start: 30, duration: 10 },
    { id: "c", trackId: "t2", start: 20, duration: 4 },
  ],
};

const metrics = (extra = {}) =>
  layoutMetrics({ viewportWidth: 1000, viewportHeight: 700, tracks: project.tracks, collapsed: new Set(), duration: 100, scale: 24, ...extra });

test("R9: the project is deterministic and follows the track and clip rules", () => {
  const a = generateProject({ seed: 7 });
  const b = generateProject({ seed: 7 });
  assert.deepEqual(a, b);
  assert.notDeepEqual(a, generateProject({ seed: 8 }));
  assert.equal(a.duration, 230);
  assert.deepEqual(a.tracks.map((t) => t.id), ["caption", ..."10,9,8,7,6,5,4,3,2,1".split(",").map((n) => `track-${n}`), "audio"]);
  const decorative = a.tracks.filter((t) => t.decorative).map((t) => t.number);
  assert.deepEqual(decorative, [10, 9]);
  for (const id of ["track-10", "track-9"]) {
    const clips = a.clips.filter((c) => c.trackId === id);
    assert.deepEqual(clips.map((c) => [c.start, c.duration]), [[0, 230]]);
  }
  assert.equal(a.tracks.find((t) => t.number === 6).clipKind, "first");
  assert.equal(a.tracks.find((t) => t.number === 5).clipKind, "second");
  for (const clip of a.clips) {
    assert.ok(clip.start >= 0 && clip.start + clip.duration <= 230 + 1e-9);
    assert.equal(clip.start, Math.round(clip.start * 1000) / 1000);
    assert.equal(clip.duration, Math.round(clip.duration * 1000) / 1000);
  }
  const audio = a.clips.filter((c) => c.trackId === "audio");
  assert.ok(audio.every((c) => c.duration >= 24 && c.duration < 64.001));
  const captions = a.clips.filter((c) => c.trackId === "caption");
  assert.ok(captions.every((c) => c.duration >= 1.2 && c.duration < 3.801));
  assert.ok(captions[0].start < 6);
  assert.ok(generateProject({ trackCount: 30 }).tracks.length === 32);
});

test("R2: layout metrics", () => {
  const m = layoutMetrics({ viewportWidth: 1000, viewportHeight: 700, tracks: generateProject().tracks, collapsed: new Set(), duration: 230, scale: 24 });
  assert.equal(m.bodyHeight, Math.round(0.42 * 700));
  assert.equal(m.gridLeft, 220);
  assert.equal(m.gridTop, 700 - (30 + m.bodyHeight + 34) + 30);
  assert.equal(m.gridWidth, 780);
  assert.equal(m.contentWidth, 230 * 24);
  assert.equal(m.contentHeight, 30 + 340 + 56);
  assert.equal(m.maxOffsetX, 230 * 24 - 780);
  assert.equal(m.maxOffsetY, 426 - m.bodyHeight);
  const small = metrics({ viewportHeight: 200 });
  assert.equal(small.bodyHeight, 120, "the body is at least 120 px");
  const collapsed = metrics({ collapsed: new Set(["t3"]) });
  assert.equal(collapsed.contentHeight, 34 + 34 + 18);
  assert.equal(metrics({ viewportWidth: 100 }).gridWidth, 0);
  assert.equal(tickStep(24), 5);
  assert.equal(tickStep(400), 0.5);
  assert.equal(tickStep(2), 60);
  assert.equal(tickStep(0.1), 300);
});

test("R3: wheel panning subtracts deltas, Shift pans horizontally, Ctrl zooms about the pointer", () => {
  const m = metrics();
  const view = { scale: 24, offsetX: 100, offsetY: 0 };
  assert.deepEqual(applyWheel(view, m, { deltaX: -30, deltaY: 0 }, 100), { ...view, offsetX: 130 });
  assert.equal(applyWheel(view, m, { deltaX: 500, deltaY: 0 }, 100).offsetX, 0, "clamped at 0");
  assert.equal(applyWheel(view, m, { deltaX: -1e6, deltaY: 0 }, 100).offsetX, m.maxOffsetX);
  const shifted = applyWheel(view, m, { deltaX: 40, deltaY: -50, shift: true }, 100);
  assert.deepEqual(shifted, { ...view, offsetX: 150 });
  assert.equal(applyWheel({ ...view, offsetY: 0 }, m, { deltaY: -5, shift: true }, 100).offsetY, 0, "no vertical pan with Shift");

  const zoom = applyWheel(view, m, { deltaY: 100, x: 520, ctrl: true }, 100);
  assert.ok(Math.abs(zoom.scale - 24 * Math.exp(0.5)) < 1e-9);
  const before = (520 - m.gridLeft + view.offsetX) / view.scale;
  const after = (520 - m.gridLeft + zoom.offsetX) / zoom.scale;
  assert.ok(Math.abs(before - after) < 1e-9, "the time under the pointer stays fixed");
  assert.equal(zoom.offsetY, view.offsetY);
  assert.equal(applyWheel(view, m, { deltaY: 1e6, ctrl: true }, 100).scale, 400);
  assert.equal(applyWheel(view, m, { deltaY: -1e6, cmd: true }, 100).scale, 2);
  const noPointer = applyWheel({ scale: 24, offsetX: 0, offsetY: 0 }, m, { deltaY: 100, ctrl: true }, 100);
  assert.ok(Math.abs(noPointer.scale - 24 * Math.exp(0.5)) < 1e-9, "the pointer defaults to the grid left");
});

test("R7: the zoom slider maps exponentially", () => {
  assert.equal(sliderToScale(0), 2);
  assert.ok(Math.abs(sliderToScale(1) - 400) < 1e-9);
  assert.ok(Math.abs(sliderToScale(0.5) - 28.2843) < 1e-3);
  assert.equal(sliderToScale(-3), 2);
  assert.ok(Math.abs(scaleToSlider(sliderToScale(0.3)) - 0.3) < 1e-12);
});

test("R6: snapping is inclusive at 6 px, not at 6.01 px, and the later equidistant candidate wins", () => {
  assert.deepEqual(snapTime(10 + 6 / 24, [10], 24), { time: 10, guide: 10, snapped: true });
  assert.equal(snapTime(10 + 6.01 / 24, [10], 24).snapped, false);
  assert.equal(snapTime(10 + 6.01 / 24, [10], 24).guide, null);
  assert.equal(snapTime(5, [4, 6], 1).time, 6, "equidistant: the later candidate in order wins");
  assert.equal(snapTime(5, [6, 4], 1).time, 4);
});

const gestureOf = (kind, clip, dx, dy = 0, scale = 24) => ({
  kind,
  clip,
  scale,
  offsetX: 0,
  offsetY: 0,
  origin: { x: 500, y: 500 },
  current: { x: 500 + dx, y: 500 + dy },
});

test("R5/R6: moving clamps, picks the destination row and snaps either edge, preferring the start edge on a tie", () => {
  const rows = metrics().rows;
  const clipA = project.clips[0];
  // Plain move: 10 s + 48 px / 24 = 12 s (no candidate within 6 px).
  const plain = previewClip(gestureOf("move", clipA, 48), project, 80, rows);
  assert.deepEqual([plain.trackId, plain.start, plain.guide], ["t1", 12, null]);
  // Clamp to the project start and end.
  assert.equal(previewClip(gestureOf("move", clipA, -5000), project, 80, rows).start, 0);
  assert.equal(previewClip(gestureOf("move", clipA, 5000), project, 80, rows).start, 95);
  // Destination row: center of t1 + 40 falls in t2 (34..68).
  assert.equal(previewClip(gestureOf("move", clipA, 0, 40), project, 80, rows).trackId, "t2");
  assert.equal(previewClip(gestureOf("move", clipA, 0, -900), project, 80, rows).trackId, "t1", "above the first row selects the first");
  assert.equal(previewClip(gestureOf("move", clipA, 0, 9000), project, 80, rows).trackId, "t3", "below the last row selects the last");
  // Start snaps to the playhead (20 s) within 6 px: wanted start 20.2 s -> 20.
  const snapped = previewClip(gestureOf("move", clipA, (10.2 * 24)), project, 20, rows);
  assert.deepEqual([snapped.start, snapped.guide], [20, 20]);
  // End edge snaps: end at 29.8 -> candidate 30 (clip b start), so start = 30 - 5.
  const endSnap = previewClip(gestureOf("move", clipA, (14.8 * 24)), project, 80, rows);
  assert.deepEqual([endSnap.start, endSnap.guide], [25, 30]);
  // Equal correction prefers the start edge: start 24.9 -> 25? candidates: playhead 25 and clip b start 30.
  const tie = previewClip(gestureOf("move", { ...clipA, duration: 5 }, 14.9 * 24), project, 25, rows);
  assert.equal(tie.start, 25);
  // The guide is hidden when the clamp overrides the snapped position (end snap beyond the end).
  const clampedGuide = previewClip(gestureOf("move", { ...clipA, duration: 20 }, 5000), { ...project, duration: 100 }, 1, rows);
  assert.equal(clampedGuide.start, 80);
  assert.equal(clampedGuide.guide, 100, "the end edge snapped to the project duration and survived the clamp");
  // Only moved clip changes: other clips on the destination track are candidates, the dragged one is not.
  const onlyOthers = previewClip(gestureOf("move", clipA, 0), project, 80, rows);
  assert.equal(onlyOthers.guide, null, "the clip's own edges never attract it");
});

test("R6: trimming snaps, keeps the other edge fixed and never goes below 0.4 s", () => {
  const rows = metrics().rows;
  const clipB = project.clips[1]; // 30..40 on t1
  const startTrim = previewClip(gestureOf("trim-start", clipB, -6 * 24), project, 80, rows);
  assert.deepEqual([startTrim.start, startTrim.duration, startTrim.guide], [24, 16, null]);
  const nearA = previewClip(gestureOf("trim-start", clipB, -(30 - 15.2) * 24), project, 80, rows);
  assert.deepEqual([nearA.start, nearA.guide], [15, 15], "snaps to the end of clip a");
  const tooShort = previewClip(gestureOf("trim-start", clipB, 9.9 * 24), project, 80, rows);
  assert.ok(Math.abs(tooShort.duration - 0.4) < 1e-9);
  assert.equal(tooShort.guide, null, "the clamp overrode the snapped position");
  const endTrim = previewClip(gestureOf("trim-end", clipB, 5 * 24), project, 80, rows);
  assert.deepEqual([endTrim.start, endTrim.duration], [30, 15]);
  const endShort = previewClip(gestureOf("trim-end", clipB, -9.9 * 24), project, 80, rows);
  assert.ok(Math.abs(endShort.duration - 0.4) < 1e-9);
  assert.equal(previewClip(gestureOf("trim-end", clipB, 1e6), project, 80, rows).duration, 70, "bounded by the project end");
});

test("R7/R8: marquee overlap is inclusive and culling intersects inclusively", () => {
  const m = metrics();
  const view = { scale: 24, offsetX: 0, offsetY: 0 };
  const left = m.gridLeft + 10 * 24;
  const top = m.gridTop + 3;
  assert.deepEqual(
    marqueeSelection(project, new Set(), m.rows, view, m, { x1: left - 50, y1: top, x2: left, y2: top + 1 }),
    ["a"],
    "a marquee whose right edge touches the clip's left edge selects it",
  );
  assert.deepEqual(marqueeSelection(project, new Set(), m.rows, view, m, { x1: left - 50, y1: top, x2: left - 0.5, y2: top + 1 }), []);
  assert.deepEqual(marqueeSelection(project, new Set(["t1"]), m.rows, view, m, { x1: 0, y1: 0, x2: 2000, y2: 2000 }), ["c"], "collapsed tracks are excluded");
  const narrow = { ...m, gridWidth: 240 }; // visible 0..10 s
  assert.deepEqual(visibleClips(project, new Set(), m.rows, view, narrow).map((c) => c.id), ["a"], "start == visible end is inclusive");
  assert.equal(visibleClips(project, new Set(), m.rows, view, narrow, false).length, 3, "culling off mounts every clip");
  assert.deepEqual(visibleClips(project, new Set(["t1"]), m.rows, view, m, false).map((c) => c.id), ["c"]);
});

function stateOf() {
  return initialState({ project, viewportWidth: 1000, viewportHeight: 700 });
}

test("R4/R5: the 3 px commit threshold, captured moves, selection and history", () => {
  let state = stateOf();
  const m = metricsOf(state);
  const x = m.gridLeft + 10 * 24 + 10;
  const y = m.gridTop + 10;
  state = reduce(state, { type: "pressClip", clipId: "a", part: "body", x, y });
  assert.deepEqual(state.selection, ["a"]);
  assert.equal(state.gesture.kind, "move");
  state = reduce(state, { type: "move", x: x + 3, y });
  assert.equal(state.gesture.moved, false, "3 px does not commit");
  state = reduce(state, { type: "release", x: x + 3, y });
  assert.equal(state.project.clips[0].start, 10, "a release without having moved changes nothing");
  assert.deepEqual(state.history, ["dragstart:move:a", "dragend:move"]);

  state = reduce(state, { type: "pressClip", clipId: "a", part: "body", x, y });
  state = reduce(state, { type: "move", x: x + 4, y });
  assert.equal(state.gesture.moved, true, "4 px commits");
  // The pointer leaves the grid and the window; moves and the release still commit.
  state = reduce(state, { type: "move", x: x + 5000, y: -400 });
  state = reduce(state, { type: "release", x: x + 48, y });
  assert.equal(state.project.clips[0].start, 12);
  assert.equal(state.gesture, null);

  // Press modifiers toggle membership; history keeps the latest 8.
  state = reduce(state, { type: "pressClip", clipId: "b", part: "body", x, y, shift: true });
  state = reduce(state, { type: "release", x, y });
  assert.deepEqual(state.selection, ["a", "b"]);
  state = reduce(state, { type: "pressClip", clipId: "b", part: "body", x, y, cmd: true });
  state = reduce(state, { type: "release", x, y });
  assert.deepEqual(state.selection, ["a"]);
  assert.equal(state.history.length, 8);
  assert.equal(state.history.at(-1), "dragend:move");
});

test("R7: scrubbing, marquee selection with and without Shift, collapse and the zoom slider", () => {
  let state = stateOf();
  const m = metricsOf(state);
  state = reduce(state, { type: "pressRuler", x: m.gridLeft + 240, y: 100 });
  assert.equal(state.playhead, 10);
  state = reduce(state, { type: "move", x: m.gridLeft + 480, y: 5000 });
  assert.equal(state.playhead, 20);
  state = reduce(state, { type: "move", x: -9000, y: 0 });
  assert.equal(state.playhead, 0, "clamped at the start");
  state = reduce(state, { type: "release", x: 0, y: 0 });
  assert.equal(state.gesture, null);

  state = { ...state, selection: ["c"] };
  state = reduce(state, { type: "pressGrid", x: m.gridLeft + 2, y: m.gridTop + 90, shift: true });
  assert.deepEqual(state.selection, ["c"], "Shift keeps the selection");
  state = reduce(state, { type: "move", x: m.gridLeft + 400, y: m.gridTop + 2 });
  state = reduce(state, { type: "release", x: m.gridLeft + 400, y: m.gridTop + 2 });
  assert.deepEqual(state.selection, ["a"], "the marquee replaces the selection with the overlapping clips");
  state = reduce(state, { type: "pressGrid", x: m.gridLeft + 2, y: m.gridTop + 100 });
  assert.deepEqual(state.selection, [], "without Shift the selection is cleared");
  state = reduce(state, { type: "release", x: m.gridLeft + 2, y: m.gridTop + 100 });

  state = reduce(state, { type: "toggleCollapse", trackId: "t1" });
  assert.equal(metricsOf(state).contentHeight, 18 + 34 + 56);
  state = reduce(state, { type: "toggleCollapse", trackId: "t1" });
  assert.equal(metricsOf(state).contentHeight, 34 + 34 + 56);

  state = reduce(state, { type: "pressSlider", x: 8 + 60, y: 0 });
  assert.ok(Math.abs(state.view.scale - 28.2843) < 1e-3);
  state = reduce(state, { type: "move", x: 8 + 120, y: 0 });
  assert.ok(Math.abs(state.view.scale - 400) < 1e-9);
  state = reduce(state, { type: "move", x: 8 + 500, y: 0 });
  assert.ok(Math.abs(state.view.scale - 400) < 1e-9, "clamped at the slider end");
  assert.equal(state.view.offsetX, 0);
});
