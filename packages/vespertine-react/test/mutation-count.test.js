// APP-009 A3: mutation counts (not timing) for a parent that rerenders with fresh equal styles.

import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import { createRoot, flushSync } from "../src/index.js";
import { referenceRenderer } from "./helpers.js";

const h = React.createElement;
const LEAVES = 1000;
const FRAMES = 120;

const freshStyle = (index) => ({ width: 8, height: 4 + (index % 3), backgroundColor: "#336699", margin: [1, 2] });

/** Leaf 7 grows by one pixel every 40 frames; every other style only gets fresh objects. */
function Parent({ frame }) {
  const leaves = [];
  for (let i = 0; i < LEAVES; i++) {
    const style = freshStyle(i);
    if (i === 7) style.width = 8 + Math.floor(frame / 40);
    leaves.push(h("div", { key: i, style }));
  }
  return h("div", { style: { display: "flex" } }, leaves);
}

test("A3: 1000 equal-styled leaves under a parent that rerenders for 120 frames send no style mutation after the mount", () => {
  const renderer = referenceRenderer();
  const root = createRoot(renderer);
  flushSync(() => root.render(h(Parent, { frame: 0 })));
  assert.equal(renderer.batches.length, 1);
  const stylesAfterMount = renderer.count("setStyle");
  assert.equal(renderer.count("createElement"), LEAVES + 2);

  let batchesAfter = 0;
  let naiveStyleUpdates = 0;
  let previous = Array.from({ length: LEAVES }, (_, i) => freshStyle(i));
  for (let frame = 1; frame <= FRAMES; frame++) {
    const changes = frame % 40 === 0;
    const before = renderer.batches.length;
    flushSync(() => root.render(h(Parent, { frame })));
    const sent = renderer.batches.length - before;
    assert.ok(sent <= 1, `frame ${frame} sent ${sent} batches`);
    if (!changes) assert.equal(sent, 0, `frame ${frame} changed nothing and must send no batch`);
    else assert.deepEqual(renderer.batches.at(-1).map((op) => op[0]), ["setStyle"]);
    batchesAfter += sent;

    // A naive full-tree bridge compares styles by identity: every fresh object looks changed.
    const next = Array.from({ length: LEAVES }, (_, i) => freshStyle(i));
    naiveStyleUpdates += next.filter((style, i) => style !== previous[i]).length;
    previous = next;
  }
  assert.equal(batchesAfter, 3, "only the three frames that changed a style sent a batch");
  assert.equal(renderer.count("setStyle") - stylesAfterMount, 3, "no style mutation beyond the three real changes");
  assert.equal(naiveStyleUpdates, LEAVES * FRAMES, "the naive bridge would have sent 1,000 style updates per frame");
});
