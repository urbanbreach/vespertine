import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync, existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { isEntryPoint, main } from "../src/cli.js";
import { createTarGz, starterEntries, stubFetch } from "./helpers.js";

const run = promisify(execFile);
const cliPath = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "src", "cli.js");

function temporary() {
  return mkdtempSync(path.join(tmpdir(), "create-vespertine-"));
}

function capture() {
  const chunks = { out: "", err: "" };
  return {
    chunks,
    stdout: { write: (text) => void (chunks.out += text) },
    stderr: { write: (text) => void (chunks.err += text) },
  };
}

test("A1: entry detection succeeds when the entry file is reached through a symbolic link", async () => {
  const directory = temporary();
  try {
    const link = path.join(directory, "create-vespertine-link");
    symlinkSync(cliPath, link);
    assert.equal(isEntryPoint(pathToFileURL(cliPath).href, link), true);
    assert.equal(isEntryPoint(pathToFileURL(cliPath).href, cliPath), true);
    assert.equal(isEntryPoint(pathToFileURL(cliPath).href, path.join(directory, "other.js")), false);
    assert.equal(isEntryPoint(pathToFileURL(cliPath).href, undefined), false);

    // A path that cannot be resolved falls back to the path as given.
    const missing = path.join(directory, "missing", "cli.js");
    assert.equal(isEntryPoint(pathToFileURL(missing).href, missing), true);

    // Launching the real command through the link runs the CLI.
    const { stdout } = await run(process.execPath, [link, "--version"]);
    assert.match(stdout.trim(), /^\d+\.\d+\.\d+/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("A1: importing the module does not parse arguments", async () => {
  // The process arguments say --version; if importing parsed them, the version would print.
  const script = `await import(${JSON.stringify(pathToFileURL(cliPath).href)}); console.log("imported");`;
  const { stdout, stderr } = await run(process.execPath, ["--input-type=module", "-e", script, "--", "--version"]);
  assert.equal(stdout.trim(), "imported");
  assert.equal(stderr, "");
});

test("help, version and completion output are available", async () => {
  for (const argv of [[], ["help"], ["--help"], ["-h"]]) {
    const io = capture();
    assert.equal(await main(argv, io), 0);
    assert.match(io.chunks.out, /Usage: create-vespertine/);
    assert.match(io.chunks.out, /new <target>/);
  }
  const version = capture();
  assert.equal(await main(["--version"], version), 0);
  assert.match(version.chunks.out, /^\d+\.\d+\.\d+/);
  for (const shell of ["bash", "zsh", "fish"]) {
    const io = capture();
    assert.equal(await main(["completion", shell], io), 0);
    assert.match(io.chunks.out, /--no-install|no-install/);
    assert.match(io.chunks.out, /create-vespertine/);
  }
  const unknown = capture();
  assert.equal(await main(["completion", "powershell"], unknown), 2);
  assert.match(unknown.chunks.err, /bash, zsh and fish/);
  const bad = capture();
  assert.equal(await main(["frobnicate"], bad), 2);
  const noTarget = capture();
  assert.equal(await main(["new"], noTarget), 2);
  const badOption = capture();
  assert.equal(await main(["new", "x", "--force"], badOption), 2);
});

test("new resolves the target against the working directory, installs and prints next steps", async () => {
  const cwd = temporary();
  try {
    const io = capture();
    const installed = [];
    const fetch = stubFetch();
    const code = await main(["new", "my-app"], {
      ...io,
      cwd,
      env: { GITHUB_TOKEN: "secret" },
      fetch,
      install: async (directory) => void installed.push(directory),
    });
    assert.equal(code, 0, io.chunks.err);
    assert.deepEqual(installed, [path.join(cwd, "my-app")]);
    assert.ok(existsSync(path.join(cwd, "my-app", "src", "main.tsx")));
    assert.match(io.chunks.out, /Created my-app/);
    assert.match(io.chunks.out, /cd my-app/);
    assert.match(io.chunks.out, /bun run start/);
    assert.equal(fetch.calls[0].headers.Authorization, "Bearer secret");
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("--no-install skips installation; an installation failure keeps the project and fails the command", async () => {
  const cwd = temporary();
  try {
    let installs = 0;
    const io = capture();
    const skipped = await main(["new", "quiet", "--no-install"], {
      ...io,
      cwd,
      env: {},
      fetch: stubFetch(),
      install: async () => void (installs += 1),
    });
    assert.equal(skipped, 0, io.chunks.err);
    assert.equal(installs, 0);

    const failing = capture();
    const code = await main(["new", "broken"], {
      ...failing,
      cwd,
      env: {},
      fetch: stubFetch(),
      install: async () => {
        throw new Error("bun exploded");
      },
    });
    assert.equal(code, 1);
    assert.match(failing.chunks.err, /bun exploded/);
    assert.ok(existsSync(path.join(cwd, "broken", "package.json")));
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("a failing creation exits non-zero and reports the error", async () => {
  const cwd = temporary();
  try {
    mkdirSync(path.join(cwd, "taken"));
    writeFileSync(path.join(cwd, "taken", "keep"), "x");
    const io = capture();
    const code = await main(["new", "taken"], { ...io, cwd, env: {}, fetch: stubFetch() });
    assert.equal(code, 1);
    assert.match(io.chunks.err, /already exists/);
    assert.equal(readFileSync(path.join(cwd, "taken", "keep"), "utf8"), "x");

    const empty = capture();
    const noStarter = await main(["new", "nothing"], {
      ...empty,
      cwd,
      env: {},
      fetch: stubFetch({ archive: createTarGz([{ path: "root/other.txt", data: "x" }]) }),
    });
    assert.equal(noStarter, 1);
    assert.match(empty.chunks.err, /no starter project/);
    assert.equal(existsSync(path.join(cwd, "nothing")), false);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
