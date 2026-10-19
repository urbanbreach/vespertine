// Renders the curated showcase screenshot (DEV-009 R5): the task-list reference surface on the
// native offscreen test host, captured as a PNG at the path given as the first argument.
//
//   VESPERTINE_NATIVE_ADDON=<addon> node scripts/showcase.mjs <output.png>
//
// Exits 0 after writing the file, 2 when the machine has no GPU or software adapter for the
// offscreen host (nothing is written), 1 on any other failure.

import { mkdir } from "node:fs/promises";
import path from "node:path";
import { createTestHost, testHostAvailable } from "@vespertine/react/testing";
import { h } from "../src/h.js";
import { TaskList } from "../src/tasks/index.js";

const output = process.argv[2];
if (!output) {
  process.stderr.write("usage: showcase.mjs <output.png>\n");
  process.exit(1);
}
if (!testHostAvailable()) {
  process.stderr.write("showcase: the offscreen test host has no rendering adapter on this machine\n");
  process.exit(2);
}

const host = createTestHost({ width: 940, height: 660, scaleFactor: 1 });
try {
  host.render(h(TaskList, { width: 940, height: 660 }));
  await mkdir(path.dirname(path.resolve(output)), { recursive: true });
  host.renderer.captureScreenshot(path.resolve(output));
  if (host.errors.length > 0) throw host.errors[0];
} finally {
  host.unmount();
}
