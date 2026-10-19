import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

import { DEFAULT_CONFIG, MAGICK_OPTIONS, collectScreenshots } from "../src/screenshots.js";

const cliPath = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "src", "screenshots-cli.js");

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** A small RGB PNG with a gradient, so that palette reduction has something to do. */
function png(width = 64, height = 64) {
  const chunk = (type, data) => {
    const head = Buffer.alloc(8);
    head.writeUInt32BE(data.length, 0);
    head.write(type, 4, "ascii");
    const tail = Buffer.alloc(4);
    tail.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
    return Buffer.concat([head, data, tail]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  const rows = [];
  for (let y = 0; y < height; y += 1) {
    const row = Buffer.alloc(1 + width * 3);
    for (let x = 0; x < width; x += 1) row.set([x * 4, y * 4, (x + y) * 2], 1 + x * 3);
    rows.push(row);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(Buffer.concat(rows))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** A workspace whose jobs are `node` scripts that write images (or fail). */
function workspace(jobs) {
  const root = mkdtempSync(path.join(tmpdir(), "vespertine-shots-"));
  mkdirSync(path.join(root, "out"));
  const config = path.join(root, "screenshots.json");
  writeFileSync(config, JSON.stringify({ destination: "docs/shots", jobs: jobs(root) }));
  return { root, config, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

function writingJob(root, name, file, { exit = 0 } = {}) {
  const script = path.join(root, `${name}.mjs`);
  writeFileSync(
    script,
    `import { appendFileSync, writeFileSync } from "node:fs";
appendFileSync(${JSON.stringify(path.join(root, "order.log"))}, ${JSON.stringify(name)} + "\\n");
${file ? `writeFileSync(${JSON.stringify(path.join(root, "out", file))}, Buffer.from(${JSON.stringify(png().toString("base64"))}, "base64"));` : ""}
process.exit(${exit});
`,
  );
  return { name, command: [process.execPath, script], images: file ? [{ source: `out/${file}`, name: file }] : [] };
}

const noLog = () => {};

/** An environment in which neither `magick` nor `convert` can be found. */
function withoutMagick() {
  const empty = mkdtempSync(path.join(tmpdir(), "vespertine-nopath-"));
  return { env: { ...process.env, PATH: empty }, cleanup: () => rmSync(empty, { recursive: true, force: true }) };
}

function magickAvailable() {
  try {
    execFileSync("magick", ["-version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

test("A4: without ImageMagick the original image is copied unchanged and the fact is logged", async () => {
  const w = workspace((root) => [writingJob(root, "one", "one.png")]);
  const bare = withoutMagick();
  try {
    const logs = [];
    const { written } = await collectScreenshots({ config: w.config, root: w.root, env: bare.env, log: (m) => logs.push(m) });
    const destination = path.join(w.root, "docs", "shots", "one.png");
    assert.deepEqual(written, [destination]);
    assert.deepEqual(readFileSync(destination), readFileSync(path.join(w.root, "out", "one.png")));
    assert.ok(logs.some((line) => line.includes("copied the original") && line.includes("not available")), logs.join("\n"));
  } finally {
    bare.cleanup();
    w.cleanup();
  }
});

test("A4: the collector's command line runs the jobs and uses the fallback with magick absent", async () => {
  const w = workspace((root) => [writingJob(root, "one", "one.png")]);
  const bare = withoutMagick();
  try {
    const child = spawn(process.execPath, [cliPath, "--config", w.config, "--root", w.root], {
      env: bare.env,
      stdio: ["ignore", "ignore", "pipe"],
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const code = await new Promise((resolve) => child.once("exit", resolve));
    assert.equal(code, 0, stderr);
    assert.match(stderr, /copied the original/);
    assert.ok(existsSync(path.join(w.root, "docs", "shots", "one.png")));
  } finally {
    bare.cleanup();
    w.cleanup();
  }
});

test("A4: the repository's curated configuration is a valid job list", () => {
  const config = JSON.parse(readFileSync(DEFAULT_CONFIG, "utf8"));
  assert.equal(typeof config.destination, "string");
  assert.ok(config.jobs.length > 0);
  for (const job of config.jobs) {
    assert.equal(typeof job.name, "string");
    assert.ok(Array.isArray(job.command) && job.command.length > 0);
    for (const image of job.images) assert.equal(typeof image.source, "string");
  }
});

test("A4: a failing conversion command also copies the original and logs it", async () => {
  const w = workspace((root) => [writingJob(root, "one", "one.png")]);
  const fakeBin = mkdtempSync(path.join(tmpdir(), "vespertine-fakebin-"));
  try {
    // Leaves a partial output behind and fails: the collector must replace it with the original.
    writeFileSync(path.join(fakeBin, "magick"), '#!/bin/sh\nfor last; do :; done\necho partial > "$last"\nexit 1\n');
    execFileSync("chmod", ["+x", path.join(fakeBin, "magick")]);
    const logs = [];
    await collectScreenshots({ config: w.config, root: w.root, env: { ...process.env, PATH: fakeBin }, log: (m) => logs.push(m) });
    const destination = path.join(w.root, "docs", "shots", "one.png");
    assert.deepEqual(readFileSync(destination), readFileSync(path.join(w.root, "out", "one.png")));
    assert.ok(logs.some((line) => line.includes("magick failed") && line.includes("copied the original")), logs.join("\n"));
  } finally {
    rmSync(fakeBin, { recursive: true, force: true });
    w.cleanup();
  }
});

test("with ImageMagick the image is stripped, palettized and compressed", async () => {
  if (!magickAvailable()) return; // The fallback path is covered above.
  const w = workspace((root) => [writingJob(root, "one", "one.png")]);
  try {
    const logs = [];
    await collectScreenshots({ config: w.config, root: w.root, log: (m) => logs.push(m) });
    const destination = path.join(w.root, "docs", "shots", "one.png");
    assert.ok(statSync(destination).size > 0);
    assert.equal(logs.some((line) => line.includes("copied the original")), false, logs.join("\n"));
    const info = execFileSync("magick", ["identify", "-format", "%[colors] %[type]", destination]).toString();
    assert.ok(Number.parseInt(info, 10) <= 256, info);
    assert.deepEqual(MAGICK_OPTIONS, ["-strip", "-colors", "256", "-define", "png:compression-level=9"]);
  } finally {
    w.cleanup();
  }
});

test("A4: jobs run sequentially and stop at the first failing job", async () => {
  const w = workspace((root) => [
    writingJob(root, "first", "first.png"),
    writingJob(root, "second", "second.png", { exit: 3 }),
    writingJob(root, "third", "third.png"),
  ]);
  const bare = withoutMagick();
  try {
    await assert.rejects(collectScreenshots({ config: w.config, root: w.root, env: bare.env, log: noLog }), /"second" failed \(exit code 3\)/);
    assert.deepEqual(readFileSync(path.join(w.root, "order.log"), "utf8").split("\n").filter(Boolean), ["first", "second"]);
    assert.ok(existsSync(path.join(w.root, "docs", "shots", "first.png")));
    assert.equal(existsSync(path.join(w.root, "docs", "shots", "second.png")), false);
  } finally {
    bare.cleanup();
    w.cleanup();
  }
});

test("A4: a missing expected image stops the collection and reports its path", async () => {
  const w = workspace((root) => [
    { ...writingJob(root, "first", null), images: [{ source: "out/never.png", name: "never.png" }] },
    writingJob(root, "second", "second.png"),
  ]);
  const bare = withoutMagick();
  try {
    await assert.rejects(
      collectScreenshots({ config: w.config, root: w.root, env: bare.env, log: noLog }),
      (error) => {
        assert.ok(error.message.includes(path.join(w.root, "out", "never.png")), error.message);
        return true;
      },
    );
    assert.deepEqual(readFileSync(path.join(w.root, "order.log"), "utf8").split("\n").filter(Boolean), ["first"]);
  } finally {
    bare.cleanup();
    w.cleanup();
  }
});

test("a job command that cannot be started fails the collection", async () => {
  const w = workspace(() => [{ name: "ghost", command: ["/nonexistent/renderer"], images: [] }]);
  try {
    await assert.rejects(collectScreenshots({ config: w.config, root: w.root, log: noLog }), /"ghost" failed/);
  } finally {
    w.cleanup();
  }
});
