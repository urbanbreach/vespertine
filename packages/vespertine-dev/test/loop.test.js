import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, appendFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { waitFor } from "./helpers.js";

const cliPath = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "src", "cli.js");

function read(file) {
  return existsSync(file) ? readFileSync(file, "utf8") : "";
}

function lines(file) {
  return read(file).split("\n").filter(Boolean);
}

/** A workspace with a fake `cargo` (logs its arguments, fails while `fail` exists) and a fake app. */
function fakeWorkspace() {
  const root = mkdtempSync(path.join(tmpdir(), "vespertine-dev-loop-"));
  mkdirSync(path.join(root, "crates", "demo", "src"), { recursive: true });
  writeFileSync(path.join(root, "crates", "demo", "src", "lib.rs"), "// v0\n");
  const cargo = path.join(root, "fake-cargo");
  const app = path.join(root, "fake-app");
  writeFileSync(
    cargo,
    `#!/bin/sh
echo "$@" >> "${root}/cargo.log"
if [ -e "${root}/fail" ]; then echo "error: could not compile" >&2; exit 101; fi
printf '{"reason":"compiler-artifact","target":{"name":"demo","kind":["example"]},"executable":"${app}"}\\n'
`,
  );
  writeFileSync(
    app,
    `#!/bin/sh
echo "start" >> "${root}/app.log"
trap 'echo "term" >> "${root}/app.log"; exit 0' TERM
while :; do sleep 0.05; done
`,
  );
  chmodSync(cargo, 0o755);
  chmodSync(app, 0o755);
  return {
    root,
    cargo,
    appLog: path.join(root, "app.log"),
    cargoLog: path.join(root, "cargo.log"),
    source: path.join(root, "crates", "demo", "src", "lib.rs"),
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

function launch(args, env) {
  const child = spawn(process.execPath, [cliPath, ...args], { env: { ...process.env, ...env }, stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => resolve({ code, signal })));
  return { child, exited, stderr: () => stderr };
}

test("A4: the watcher loop builds, restarts the example on changes, survives failed builds and terminates the child on SIGTERM", async () => {
  const w = fakeWorkspace();
  const watcher = launch(["--app", "demo", "--root", w.root], { CARGO: w.cargo });
  try {
    await waitFor(() => lines(w.appLog).length === 1, { message: "the initial start" });
    assert.match(lines(w.cargoLog)[0], /^build --example demo --message-format=json-render-diagnostics$/);

    // A change in an existing file restarts the application: SIGTERM first, then a new start.
    appendFileSync(w.source, "// v1\n");
    await waitFor(() => lines(w.appLog).join(",") === "start,term,start", { message: "the first restart" });

    // A file in a directory created after the watcher started is noticed too.
    mkdirSync(path.join(w.root, "crates", "demo", "src", "deep"));
    await new Promise((resolve) => setTimeout(resolve, 100));
    writeFileSync(path.join(w.root, "crates", "demo", "src", "deep", "new.rs"), "// new\n");
    await waitFor(() => lines(w.appLog).join(",") === "start,term,start,term,start", { message: "the restart for a new directory" });

    // Non-Rust changes are ignored.
    const builds = lines(w.cargoLog).length;
    writeFileSync(path.join(w.root, "crates", "demo", "notes.txt"), "x");
    await new Promise((resolve) => setTimeout(resolve, 500));
    assert.equal(lines(w.cargoLog).length, builds);

    // A failed build leaves the running application alone.
    writeFileSync(path.join(w.root, "fail"), "");
    appendFileSync(w.source, "// broken\n");
    await waitFor(() => lines(w.cargoLog).length === builds + 1, { message: "the failing build" });
    await new Promise((resolve) => setTimeout(resolve, 500));
    assert.equal(lines(w.appLog).join(","), "start,term,start,term,start");

    // The next change after a failure still rebuilds and restarts.
    rmSync(path.join(w.root, "fail"));
    appendFileSync(w.source, "// fixed\n");
    await waitFor(() => lines(w.appLog).length === 7, { message: "the restart after the failure" });

    // SIGTERM to the watcher terminates the application before the watcher exits.
    watcher.child.kill("SIGTERM");
    const exit = await watcher.exited;
    assert.equal(exit.code, 143, watcher.stderr());
    assert.equal(lines(w.appLog).at(-1), "term", "the application was terminated before the watcher exited");
    assert.equal(lines(w.appLog).length, 8);
  } finally {
    watcher.child.kill("SIGKILL");
    w.cleanup();
  }
});

test("A4: the screenshot mode runs the default test pattern after each build and exits on SIGINT", async () => {
  const w = fakeWorkspace();
  const watcher = launch(["--root", w.root], { CARGO: w.cargo });
  try {
    await waitFor(() => lines(w.cargoLog).length === 2, { message: "the initial build and tests" });
    assert.deepEqual(lines(w.cargoLog), ["test --no-run", "test showcase"]);
    appendFileSync(w.source, "// v1\n");
    await waitFor(() => lines(w.cargoLog).length === 4, { message: "the rebuild and tests" });
    assert.deepEqual(lines(w.cargoLog).slice(2), ["test --no-run", "test showcase"]);
    watcher.child.kill("SIGINT");
    assert.equal((await watcher.exited).code, 130, watcher.stderr());
  } finally {
    watcher.child.kill("SIGKILL");
    w.cleanup();
  }
});

test("A4: a missing application name fails before building", async () => {
  const w = fakeWorkspace();
  try {
    const watcher = launch(["--app", "--root", w.root], { CARGO: w.cargo });
    const exit = await watcher.exited;
    assert.equal(exit.code, 1);
    assert.match(watcher.stderr(), /name of the example/);
    assert.equal(existsSync(w.cargoLog), false);
  } finally {
    w.cleanup();
  }
});

test("A4: the loop works with real cargo on a tiny temporary project", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "vespertine-dev-cargo-"));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("CARGO_") || key === "CARGO_HOME"));
  env.CARGO = "cargo";
  env.CARGO_TARGET_DIR = path.join(root, "target");
  env.TICKER_LOG = path.join(root, "ticker.log");
  mkdirSync(path.join(root, "src"));
  mkdirSync(path.join(root, "examples"));
  writeFileSync(path.join(root, "Cargo.toml"), '[package]\nname = "tiny"\nversion = "0.0.0"\nedition = "2021"\n\n[workspace]\n');
  writeFileSync(path.join(root, "src", "lib.rs"), "pub fn value() -> u32 { 1 }\n");
  writeFileSync(
    path.join(root, "examples", "ticker.rs"),
    `fn main() {
    use std::io::Write;
    let log = std::env::var("TICKER_LOG").unwrap();
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(log).unwrap();
    writeln!(file, "start {}", tiny::value()).unwrap();
    loop { std::thread::sleep(std::time::Duration::from_millis(50)); }
}
`,
  );
  const watcher = launch(["--app", "ticker", "--root", root, "--watch", "src"], env);
  try {
    await waitFor(() => lines(env.TICKER_LOG).length === 1, { timeout: 120000, message: `the first build (${watcher.stderr()})` });
    assert.equal(lines(env.TICKER_LOG)[0], "start 1");
    writeFileSync(path.join(root, "src", "lib.rs"), "pub fn value() -> u32 { 2 }\n");
    await waitFor(() => lines(env.TICKER_LOG).length === 2, { timeout: 120000, message: `the rebuild (${watcher.stderr()})` });
    assert.equal(lines(env.TICKER_LOG)[1], "start 2");
    watcher.child.kill("SIGTERM");
    assert.equal((await watcher.exited).code, 143);
  } finally {
    watcher.child.kill("SIGKILL");
    rmSync(root, { recursive: true, force: true });
  }
});
