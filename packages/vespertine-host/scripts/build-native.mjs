// Builds the native addon and installs it as native/vespertine_js.node.
//
//   node scripts/build-native.mjs [--release] [--test-host]
//
// `--test-host` also builds the offscreen test renderer behind `@vespertine/host/testing` (and
// the React/Solid testing helpers) into the addon. Leave it off for addons that ship.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const release = process.argv.includes("--release");
const testHost = process.argv.includes("--test-host");

const build = spawnSync(
  "cargo",
  [
    "build",
    "-p",
    "vespertine_js_node",
    "--message-format=json",
    ...(release ? ["--release"] : []),
    ...(testHost ? ["--features", "offscreen"] : []),
  ],
  { cwd: root, stdio: ["inherit", "pipe", "inherit"], encoding: "utf8", maxBuffer: 1 << 28 },
);
if (build.status !== 0) process.exit(build.status ?? 1);

let artifact = null;
for (const line of build.stdout.split("\n")) {
  if (!line.startsWith("{")) continue;
  const message = JSON.parse(line);
  if (message.reason === "compiler-artifact" && message.target?.name === "vespertine_js_node") {
    artifact = message.filenames.find((name) => /\.(so|dylib|dll)$/.test(name)) ?? artifact;
  }
}
if (!artifact) {
  console.error("cargo did not report a vespertine_js_node library artifact");
  process.exit(1);
}
const destination = join(here, "..", "native", "vespertine_js.node");
mkdirSync(dirname(destination), { recursive: true });
copyFileSync(artifact, destination);
console.log(destination);
