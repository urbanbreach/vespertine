// APP-009 R10 / A2: the Bun integration compiles TSX consumers, from sources and from an
// installed package, through the preload and through the exported build plugin.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "fixtures");
const bun = spawnSync("bun", ["--version"], { encoding: "utf8" });
const skip = bun.status === 0 ? false : "bun is not installed";

function run(args, cwd) {
  const result = spawnSync("bun", args, { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, `bun ${args.join(" ")} failed:\n${result.stderr}`);
  return result.stdout.trim();
}

const texts = (node) => (node.text ? [node.text] : []).concat(node.children.flatMap(texts));

test("the preload compiles TSX, strips query strings and types, and runs Solid's client runtime", { skip }, () => {
  const out = run(["--preload", "@vespertine/solid/preload", join(fixtures, "consumer.tsx")], fixtures);
  const snapshot = JSON.parse(out);
  const app = snapshot.children[0];
  assert.equal(app.props.testId, "app");
  assert.deepEqual(app.style, { width: 10 });
  assert.deepEqual(texts(app), ["count: 1"], "the click updated the signal through the client runtime");
});

test("the exported build plugin compiles the same consumer", { skip }, () => {
  const temp = mkdtempSync(join(fixtures, ".build-"));
  try {
    const script = join(temp, "build.mjs");
    writeFileSync(
      script,
      `import plugin from "@vespertine/solid/bun-plugin";
const result = await Bun.build({ entrypoints: [${JSON.stringify(join(fixtures, "consumer.tsx"))}], plugins: [plugin], target: "bun", outdir: ${JSON.stringify(temp)} });
if (!result.success) { console.error(result.logs.join("\\n")); process.exit(1); }
`,
    );
    run([script], temp);
    const snapshot = JSON.parse(run([join(temp, "consumer.js")], temp));
    assert.deepEqual(texts(snapshot.children[0]), ["count: 1"]);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test("TSX from an installed package is compiled through the preload and through the plugin", { skip }, () => {
  const temp = mkdtempSync(join(fixtures, ".installed-"));
  try {
    mkdirSync(join(temp, "node_modules"), { recursive: true });
    cpSync(join(fixtures, "installed-ui"), join(temp, "node_modules", "installed-ui"), { recursive: true });
    cpSync(join(fixtures, "installed-consumer.tsx"), join(temp, "main.tsx"));
    const viaPreload = JSON.parse(run(["--preload", "@vespertine/solid/preload", join(temp, "main.tsx")], temp));
    assert.deepEqual(texts(viaPreload), ["installed 7"]);

    writeFileSync(
      join(temp, "build.mjs"),
      `import plugin from "@vespertine/solid/bun-plugin";
const result = await Bun.build({ entrypoints: [${JSON.stringify(join(temp, "main.tsx"))}], plugins: [plugin], target: "bun", outdir: ${JSON.stringify(join(temp, "out"))} });
if (!result.success) { console.error(result.logs.join("\\n")); process.exit(1); }
`,
    );
    run([join(temp, "build.mjs")], temp);
    const viaPlugin = JSON.parse(run([join(temp, "out", "main.js")], temp));
    assert.deepEqual(texts(viaPlugin), ["installed 7"]);
  } finally {
    rmSync(temp, { recursive: true, force: true });
  }
});

test("registering the preload twice is a no-op", { skip }, async () => {
  const { registerPreload } = await import("../src/preload.js");
  const calls = [];
  const fake = { plugin: (plugin) => calls.push(plugin.name) };
  const KEY = Symbol.for("vespertine.solid.preloadRegistered");
  const saved = globalThis[KEY];
  delete globalThis[KEY];
  try {
    assert.equal(registerPreload(fake), true);
    assert.equal(registerPreload(fake), false);
    assert.deepEqual(calls, ["vespertine-solid"]);
  } finally {
    globalThis[KEY] = saved;
  }
});

test("compileSolid generates universal output imported from the adapter and reads no repository Babel config", async () => {
  const { compileSolid } = await import("../src/bun-plugin.js");
  const output = await compileSolid("const a: number = 1; export const el = <div testId=\"x\">{a}</div>;", join(fixtures, "inline.tsx"));
  assert.match(output, /from "@vespertine\/solid"/);
  assert.doesNotMatch(output, /: number/);
  assert.doesNotMatch(output, /solid-js\/web/);
});
