// The deterministic culling comparison of the editing surface (APP-012 A2): the same pans run
// with culling enabled and disabled while per-frame work counters are recorded: mounted clip
// nodes, framework commits and native host mutations (counts, not timing, decide the outcome;
// the frame duration is recorded for information only).

import { createNativeRenderer } from "@vespertine/host";
import { createReferenceNative } from "@vespertine/host/reference";
import { createRoot, flushSync } from "@vespertine/react";
import { React, h } from "../h.js";
import { Editor } from "./Editor.js";

const findTestId = (renderer, testId) => {
  for (const [id, element] of renderer.elements) if (element.props.get("testId") === testId) return { id, element };
  return null;
};

const readout = (renderer, testId) => {
  const { element } = findTestId(renderer, testId);
  const textOf = (id) => {
    const node = renderer.elements.get(id);
    return (node.text ?? "") + node.children.map(textOf).join("");
  };
  return textOf(findTestId(renderer, testId).id) || element.text;
};

/**
 * Runs one pan sequence (`horizontalFrames` horizontal pans, then `verticalFrames` vertical pans
 * at a fixed scale) followed by one clip move, and records every frame.
 */
export function runPanSequence({ cull, pressClipId = null, trackCount = 10, durationSeconds = 230, viewportWidth = 1000, viewportHeight = 700, horizontalFrames = 24, verticalFrames = 8 }) {
  const renderer = createNativeRenderer({ native: createReferenceNative(), onError: (error) => { throw error; } });
  const root = createRoot(renderer);
  let commits = 0;
  const profiled = h(
    React.Profiler,
    { id: "editor", onRender: () => commits++ },
    h(Editor, { trackCount, durationSeconds, viewportWidth, viewportHeight, cull }),
  );
  flushSync(() => root.render(profiled));
  const emit = (testId, event) => renderer.emit({ id: findTestId(renderer, testId).id, ...event });
  const frames = [];
  const record = (label, step) => {
    const commitsBefore = commits;
    const batchesBefore = renderer.batches.length;
    const started = performance.now();
    step();
    const duration = performance.now() - started;
    const mutations = renderer.batches.slice(batchesBefore).reduce((sum, batch) => sum + batch.length, 0);
    const [offsetX, offsetY] = readout(renderer, "readout-offset").replace("offset ", "").split(",").map(Number);
    const ticks = renderer.elements.get(findTestId(renderer, "ruler-ticks").id).style;
    const headers = renderer.elements.get(findTestId(renderer, "headers-content").id).style;
    const content = renderer.elements.get(findTestId(renderer, "grid-content").id).style;
    frames.push({
      label,
      mounted: Number(readout(renderer, "readout-mounted").replace("clips ", "").split("/")[0]),
      total: Number(readout(renderer, "readout-mounted").split("/")[1]),
      commits: commits - commitsBefore,
      mutations,
      durationMs: duration,
      offsetX,
      offsetY,
      aligned: ticks.left === content.left && headers.top === content.top && ticks.left === -offsetX && headers.top === -offsetY,
    });
  };
  for (let i = 0; i < horizontalFrames; i++) record(`pan-x-${i}`, () => emit("grid", { type: "wheel", deltaX: -97, deltaY: 0, x: 600, y: 500 }));
  for (let i = 0; i < verticalFrames; i++) record(`pan-y-${i}`, () => emit("grid", { type: "wheel", deltaX: 0, deltaY: -13, x: 600, y: 500 }));
  // A gesture after panning: select the first mounted clip by pressing it.
  const mountedClips = [...renderer.elements]
    .map(([id, element]) => [id, String(element.props.get("testId") ?? "")])
    .filter(([, testId]) => /^clip-clip-\d+$/.test(testId))
    .sort((a, b) => Number(a[1].slice(10)) - Number(b[1].slice(10)));
  const pressed = pressClipId ?? mountedClips[0][1];
  const pressedId = findTestId(renderer, pressed).id;
  record("press-clip", () => renderer.emit({ type: "pointerDown", id: pressedId, x: 400, y: 400, button: 0 }));
  record("release-clip", () => renderer.emit({ type: "pointerUp", id: pressedId, x: 400, y: 400 }));
  const final = { selection: readout(renderer, "readout-selection"), offset: readout(renderer, "readout-offset"), playhead: readout(renderer, "readout-playhead") };
  root.unmount();
  return { cull, frames, final, pressed };
}

/** Runs the sequence with culling on and off and checks the invariants of the comparison. */
export function comparePanModes(options = {}) {
  const culled = runPanSequence({ ...options, cull: true });
  const all = runPanSequence({ ...options, cull: false, pressClipId: culled.pressed });
  const problems = [];
  if (!culled.frames.every((frame) => frame.aligned) || !all.frames.every((frame) => frame.aligned)) problems.push("ruler, headers and grid fell out of step");
  const sameResults = culled.frames.every((frame, index) => frame.offsetX === all.frames[index].offsetX && frame.offsetY === all.frames[index].offsetY);
  if (!sameResults) problems.push("pan results differ between culling modes");
  if (JSON.stringify(culled.final) !== JSON.stringify(all.final)) problems.push("gesture results differ between culling modes");
  const exceeds = culled.frames[0].total > culled.frames[0].mounted || all.frames[0].mounted > culled.frames[0].mounted;
  if (exceeds && !culled.frames.every((frame, index) => frame.mounted < all.frames[index].mounted)) problems.push("culling did not mount strictly fewer clips");
  const sum = (run, key) => run.frames.reduce((total, frame) => total + frame[key], 0);
  return {
    culled,
    all,
    problems,
    summary: {
      culled: { mountedPerFrame: sum(culled, "mounted") / culled.frames.length, commits: sum(culled, "commits"), mutations: sum(culled, "mutations") },
      all: { mountedPerFrame: sum(all, "mounted") / all.frames.length, commits: sum(all, "commits"), mutations: sum(all, "mutations") },
    },
  };
}
