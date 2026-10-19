import assert from "node:assert/strict";
import { test } from "node:test";
import { comparePanModes } from "../src/editing/index.js";

test("A2: culling mounts strictly fewer clips and every other result is identical (ten tracks, then a stress run)", () => {
  for (const options of [{}, { trackCount: 40, horizontalFrames: 12, verticalFrames: 6 }]) {
    const { culled, all, problems, summary } = comparePanModes(options);
    assert.deepEqual(problems, []);
    assert.ok(culled.frames.length === all.frames.length && culled.frames.length > 0);
    for (let i = 0; i < culled.frames.length; i++) {
      assert.ok(culled.frames[i].mounted < all.frames[i].mounted, `frame ${i}: ${culled.frames[i].mounted} < ${all.frames[i].mounted}`);
      assert.equal(culled.frames[i].commits >= 0, true);
    }
    assert.ok(summary.culled.mountedPerFrame < summary.all.mountedPerFrame);
    // Counts are recorded per frame; no timing claim is made.
    assert.ok(culled.frames.every((frame) => Number.isFinite(frame.durationMs) && frame.mutations >= 0));
    console.log(`  mounted/frame culled ${summary.culled.mountedPerFrame.toFixed(1)} vs ${summary.all.mountedPerFrame.toFixed(1)}; mutations ${summary.culled.mutations} vs ${summary.all.mutations}`);
  }
});
