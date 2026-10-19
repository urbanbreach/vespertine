// APP-005 A5: opens one layer-shell surface and checks the compositor's placement of it.
// Needs Hyprland (`hyprctl layers`, `hyprctl monitors`); on another compositor it reports that
// it cannot inspect layer surfaces and fails.
//
//   node examples/layer_shell.mjs --list       print, per case, whether it changes the reserved area
//   node examples/layer_shell.mjs <index>      run one case
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { createNativeRenderer } from "../src/index.js";
import { cases } from "./layer_shell_cases.mjs";

if (process.argv[2] === "--list") {
  console.log(JSON.stringify(cases.map((c) => c.reserved !== undefined)));
  process.exit(0);
}

const env = { ...process.env };
if (!env.HYPRLAND_INSTANCE_SIGNATURE && env.XDG_RUNTIME_DIR) {
  const dir = join(env.XDG_RUNTIME_DIR, "hypr");
  const newest = readdirSync(dir)
    .map((name) => ({ name, time: statSync(join(dir, name)).mtimeMs }))
    .sort((a, b) => b.time - a.time)[0];
  if (newest) env.HYPRLAND_INSTANCE_SIGNATURE = newest.name;
}
const hypr = (...args) =>
  JSON.parse(execFileSync("hyprctl", [...args, "-j"], { encoding: "utf8", env }));

const index = Number(process.argv[2]);
const testCase = cases[index];
assert.ok(testCase, `no case ${index}`);

const namespace = `vsp-case-${index}-${process.pid}`;
const monitor = hypr("monitors").find((m) => m.focused) ?? hypr("monitors")[0];
const W = monitor.width / monitor.scale;
const H = monitor.height / monitor.scale;
const reservedBefore = [...monitor.reserved]; // left, top, right, bottom
const area = {
  x: reservedBefore[0],
  y: reservedBefore[1],
  w: W - reservedBefore[0] - reservedBefore[2],
  h: H - reservedBefore[1] - reservedBefore[3],
};

const renderer = createNativeRenderer({ onError: (error) => console.error(error) });
renderer.init({
  ...testCase.config,
  layerShell: { ...testCase.config.layerShell, namespace },
});
renderer.applyBatch(
  JSON.stringify([
    ["createElement", 0, "div"],
    ["setStyle", 0, { backgroundColor: "#cc5522" }],
    ["setRoot", 0],
  ]),
);

const want = testCase.expect(area);
const matches = (got) => ["x", "y", "w", "h"].every((key) => Math.abs(got[key] - want[key]) <= 1.5);
let surface = null;
let level = null;
// The compositor may configure a surface more than once while it maps; wait for it to settle.
for (let attempt = 0; attempt < 80; attempt++) {
  await new Promise((resolve) => setTimeout(resolve, 100));
  surface = null;
  for (const output of Object.values(hypr("layers"))) {
    for (const [lvl, list] of Object.entries(output.levels)) {
      const found = list.find((s) => s.namespace === namespace);
      if (found) [surface, level] = [found, Number(lvl)];
    }
  }
  if (surface && matches(surface)) break;
}
assert.ok(surface, `the layer surface ${namespace} never appeared`);
assert.equal(level, testCase.level, "layer");
const got = { x: surface.x, y: surface.y, w: surface.w, h: surface.h };
assert.ok(matches(got), `got ${JSON.stringify(got)}, wanted ${JSON.stringify(want)}`);
if (testCase.reserved) {
  const now = hypr("monitors").find((m) => m.name === monitor.name).reserved;
  const delta = now.map((value, i) => value - reservedBefore[i]);
  assert.deepEqual(delta, testCase.reserved, "reserved area change");
}
assert.equal(renderer.tick(), true);
console.log(`ok  ${testCase.name}: ${JSON.stringify(got)}`);

renderer.closeWindow();
const deadline = Date.now() + 3000;
while (renderer.tick() && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 20));
process.exit(0);
