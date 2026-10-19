// APP-005 A1: the window keeps painting while JavaScript computes synchronously, and the
// process exits cleanly when the window closes (tick reports false).
//
//   node examples/busy_window.mjs [seconds] [--hold]
//
// With --hold the window stays open until the user closes it.
import assert from "node:assert/strict";
import { createNativeRenderer, startFrameLoop } from "../src/index.js";

const seconds = Number(process.argv.find((arg) => /^\d+(\.\d+)?$/.test(arg)) ?? 1.5);
const hold = process.argv.includes("--hold");

const renderer = createNativeRenderer({ onError: (error) => console.error(error) });
renderer.init({ title: "Busy JavaScript", width: 520, height: 260, appId: "org.vespertine.busy" });
const size = renderer.getWindowSize();
console.log(`window opened: ${size.width}x${size.height}`);
renderer.applyBatch(
  JSON.stringify([
    ["createElement", 0, "div"],
    ["setStyle", 0, { backgroundColor: "#202830", padding: 16 }],
    ["setRoot", 0],
  ]),
);

let terminated = false;
const loop = startFrameLoop(renderer, { onTerminated: () => (terminated = true) });

if (!hold) {
  // Queue repaint requests, then block this thread. Only the UI thread can run them.
  const before = renderer.getRenderCount();
  for (let i = 0; i < 20; i++) renderer.requestRepaint();
  const until = Date.now() + seconds * 1000;
  let sink = 0;
  while (Date.now() < until) sink += Math.sqrt(sink + 1);
  const during = renderer.getRenderCount() - before;
  console.log(`frames built while JavaScript was blocked for ${seconds}s: ${during}`);
  assert.ok(during >= 1, "the window did not paint while JavaScript was busy");
  renderer.closeWindow();
}

while (!terminated) await new Promise((resolve) => setTimeout(resolve, 10));
loop.stop();
assert.equal(renderer.tick(), false);
console.log("window closed; tick reported false");
