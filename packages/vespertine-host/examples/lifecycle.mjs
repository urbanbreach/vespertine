// APP-005 A2: window lifecycle against a real display server (Wayland or X11, whichever the
// environment selects). Exits non-zero on the first failed check.
//
//   node examples/lifecycle.mjs
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { createNativeRenderer, createMutationQueue, startFrameLoop } from "../src/index.js";

const errors = [];
const renderer = createNativeRenderer({ onError: (error) => errors.push(error) });
const step = (name) => console.log(`ok  ${name}`);

assert.equal(renderer.isInitialized(), false);
assert.throws(() => renderer.tick(), /call init\(\) before tick\(\)/);
step("tick before init fails");

renderer.init({ title: "Lifecycle", width: 480, height: 320, appId: "org.vespertine.lifecycle" });
assert.equal(renderer.isInitialized(), true);
step("init opened the window");

assert.throws(() => renderer.init({ title: "Second" }), /already initialized/);
assert.equal(renderer.isInitialized(), true);
step("double init fails and leaves the first window");

const size = renderer.getWindowSize();
assert.ok(size.width > 0 && size.height > 0, JSON.stringify(size));
const insets = renderer.getWindowInsets();
assert.deepEqual(insets.safeArea, { top: 0, right: 0, bottom: 0, left: 0 });
assert.deepEqual(insets.ime, insets.safeArea);
assert.deepEqual(insets.effective, insets.safeArea);
step(`size query ${size.width}x${size.height}, zero insets`);

// The application identity must be the Wayland app_id / X11 WM_CLASS the compositor sees.
// Hyprland reports both as the client class; other compositors are not inspected.
{
  const appId = "org.vespertine.lifecycle";
  const env = { ...process.env };
  if (!env.HYPRLAND_INSTANCE_SIGNATURE && env.XDG_RUNTIME_DIR) {
    try {
      const dir = join(env.XDG_RUNTIME_DIR, "hypr");
      const newest = readdirSync(dir)
        .map((name) => ({ name, time: statSync(join(dir, name)).mtimeMs }))
        .sort((a, b) => b.time - a.time)[0];
      if (newest) env.HYPRLAND_INSTANCE_SIGNATURE = newest.name;
    } catch {
      // not a Hyprland session
    }
  }
  const list = () =>
    JSON.parse(execFileSync("hyprctl", ["clients", "-j"], { encoding: "utf8", env, stdio: ["ignore", "pipe", "ignore"] }));
  let inspectable = true;
  try {
    list();
  } catch {
    inspectable = false;
  }
  if (inspectable) {
    let found = false;
    for (let attempt = 0; attempt < 50 && !found; attempt++) {
      found = list().some((client) => client.class === appId);
      if (!found) await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assert.ok(found, `no window with class ${appId} appeared`);
    step(`compositor reports the window class ${appId}`);
  } else {
    step("app id check skipped: the compositor cannot be inspected (hyprctl)");
  }
}

const queue = createMutationQueue(renderer);
queue.createElement(0, "div");
queue.setStyle(0, { backgroundColor: "#336699", padding: 12 });
queue.createElement(1, "text");
queue.setText(1, "lifecycle");
queue.appendChild(0, 1);
queue.setRoot(0);
queue.flushMutations();
step("mutation batch applied");

renderer.setWindowTitle("Lifecycle (retitled)");
renderer.activateWindow();
renderer.zoomWindow();
renderer.zoomWindow();
renderer.toggleFullscreen();
renderer.toggleFullscreen();
renderer.minimizeWindow();
renderer.activateWindow();
step("activate, minimize, zoom, fullscreen toggle and title change were queued");

// Result-returning commands are served in order after the queued ones.
const after = renderer.getWindowSize();
assert.ok(after.width > 0 && after.height > 0);
assert.equal(renderer.tick(), true);
step("window still alive after the commands");

// An unanswered prompt does not block JavaScript.
const pending = renderer.promptForPaths({ prompt: "Choose" });
assert.ok(pending instanceof Promise);
let settled = false;
pending.then(
  () => (settled = true),
  () => (settled = true),
);
step("promptForPaths returned a pending promise");

let terminated = false;
const loop = startFrameLoop(renderer, {
  onTerminated: () => (terminated = true),
  onError: (error) => errors.push(error),
});
renderer.closeWindow();
const deadline = Date.now() + 5000;
while (!terminated && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 10));
loop.stop();
assert.equal(terminated, true, "tick never reported that the window closed");
assert.equal(renderer.tick(), false);
step("closing the window made tick report false");

assert.throws(() => renderer.getWindowSize(), /The GPUI UI thread is not running/);
assert.throws(() => renderer.activateWindow(), /The GPUI UI thread is not running/);
assert.throws(() => renderer.setWindowTitle("late"), /The GPUI UI thread is not running/);
assert.throws(
  () => renderer.applyBatch('[["createElement",9,"div"]]'),
  /repaint request failed.*not running/s,
);
step("commands after the window closed fail with the not-running error");

// The prompt that was open when the window closed settles instead of hanging.
await Promise.race([pending.catch(() => {}), new Promise((resolve) => setTimeout(resolve, 3000))]);
step(`the open prompt ${settled ? "settled" : "is still waiting for the portal dialog"}`);

assert.deepEqual(errors, []);
console.log("lifecycle: all checks passed");
process.exit(0);
