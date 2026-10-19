import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";

import { parseArguments } from "../src/cli.js";
import { DEFAULT_SHOTS_PATTERN, findExampleExecutable, startDev } from "../src/dev.js";
import { isRustSource, watchRustSources } from "../src/watch.js";
import { createFakeChild, createFakeClock, settle } from "./helpers.js";

function artifact(name, executable) {
  return JSON.stringify({ reason: "compiler-artifact", target: { name, kind: ["example"] }, executable });
}

/** A workspace root with `crates/`, and fakes for spawn and fs.watch. `script` answers cargo. */
function fixture(script) {
  const root = mkdtempSync(path.join(tmpdir(), "vespertine-dev-"));
  mkdirSync(path.join(root, "crates"));
  const clock = createFakeClock();
  const spawned = [];
  const logs = [];
  const callbacks = [];
  const closed = [];
  const spawn = (command, args, options) => {
    const child = createFakeChild();
    const call = { command, args, options, child };
    spawned.push(call);
    if (command === "cargo") {
      const { code, stdout } = script(args, spawned.filter((entry) => entry.command === "cargo").length);
      queueMicrotask(() => {
        if (stdout) child.stdout.emit("data", stdout);
        child.finish(code);
      });
    }
    return child;
  };
  const watch = (directory, options, callback) => {
    callbacks.push({ directory, options, callback });
    return { close: () => closed.push(directory) };
  };
  const options = { root, spawn, watch, clock, cargo: "cargo", env: {}, log: (message) => logs.push(message) };
  return {
    root,
    clock,
    spawned,
    logs,
    callbacks,
    closed,
    options,
    cargoCalls: () => spawned.filter((entry) => entry.command === "cargo").map((entry) => entry.args),
    apps: () => spawned.filter((entry) => entry.command !== "cargo"),
    change: (name = "lib.rs") => callbacks[0].callback("change", name),
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}

test("A3: application mode builds, starts the example and restarts it after a successful rebuild", async () => {
  const f = fixture((args) => ({ code: 0, stdout: `${artifact("other", "/bin/other")}\n${artifact("demo", "/bin/demo")}\n` }));
  try {
    const handle = await startDev({ ...f.options, mode: "app", app: "demo", packageName: "my_crate" });
    assert.deepEqual(f.cargoCalls()[0], ["build", "-p", "my_crate", "--example", "demo", "--message-format=json-render-diagnostics"]);
    assert.equal(f.apps().length, 1);
    assert.equal(f.apps()[0].command, "/bin/demo");
    assert.equal(f.callbacks[0].directory, path.join(f.root, "crates"));
    assert.equal(f.callbacks[0].options.recursive, true);

    f.change();
    f.clock.advance(120);
    await settle();
    assert.deepEqual(f.apps()[0].child.signals, ["SIGTERM"], "the previous application is terminated with SIGTERM");
    assert.equal(f.apps().length, 1, "the new one starts after the old one exited");
    f.apps()[0].child.finish(0, "SIGTERM");
    await handle.scheduler.whenIdle();
    assert.equal(f.apps().length, 2);

    const stopping = handle.stop();
    await settle();
    assert.deepEqual(f.apps()[1].child.signals, ["SIGTERM"]);
    f.apps()[1].child.finish(0, "SIGTERM");
    await stopping;
    assert.deepEqual(f.closed, [path.join(f.root, "crates")]);
  } finally {
    f.cleanup();
  }
});

test("A3: a failed build leaves the running application untouched and the next change rebuilds", async () => {
  const f = fixture((_args, call) => ({ code: call === 2 ? 101 : 0, stdout: `${artifact("demo", "/bin/demo")}\n` }));
  try {
    const handle = await startDev({ ...f.options, mode: "app", app: "demo" });
    f.change();
    f.clock.advance(120);
    await handle.scheduler.whenIdle();
    assert.equal(f.cargoCalls().length, 2);
    assert.deepEqual(f.apps()[0].child.signals, []);
    assert.equal(f.apps().length, 1);

    f.change();
    f.clock.advance(120);
    await settle();
    assert.deepEqual(f.apps()[0].child.signals, ["SIGTERM"]);
    f.apps()[0].child.finish(0, "SIGTERM");
    await handle.scheduler.whenIdle();
    const stopping = handle.stop();
    await settle();
    f.apps()[1].child.finish(0, "SIGTERM");
    await stopping;
  } finally {
    f.cleanup();
  }
});

test("application mode without an application name fails before building", async () => {
  const f = fixture(() => ({ code: 0, stdout: "" }));
  try {
    await assert.rejects(startDev({ ...f.options, mode: "app" }), /name of the example/);
    await assert.rejects(startDev({ ...f.options, mode: "app", app: "" }), /name of the example/);
    assert.equal(f.spawned.length, 0);
    assert.equal(f.callbacks.length, 0);
  } finally {
    f.cleanup();
  }
});

test("a missing source directory fails before building", async () => {
  const f = fixture(() => ({ code: 0, stdout: "" }));
  try {
    await assert.rejects(startDev({ ...f.options, mode: "shots", watchDirs: ["nope"] }), /not found/);
    assert.equal(f.spawned.length, 0);
  } finally {
    f.cleanup();
  }
});

test("a build without the example's executable is a failed build", async () => {
  const f = fixture(() => ({ code: 0, stdout: `${artifact("other", "/bin/other")}\n` }));
  try {
    const handle = await startDev({ ...f.options, mode: "app", app: "demo" });
    assert.equal(f.apps().length, 0);
    assert.ok(f.logs.some((line) => line.includes("no executable for example demo")));
    await handle.stop();
  } finally {
    f.cleanup();
  }
});

test("screenshot mode runs the tests matching the default pattern after a successful build", async () => {
  const f = fixture(() => ({ code: 0, stdout: "" }));
  try {
    const handle = await startDev({ ...f.options, mode: "shots" });
    assert.equal(DEFAULT_SHOTS_PATTERN, "showcase");
    assert.deepEqual(f.cargoCalls(), [["test", "--no-run"], ["test", "showcase"]]);
    f.change();
    f.clock.advance(120);
    await handle.scheduler.whenIdle();
    assert.deepEqual(f.cargoCalls().slice(2), [["test", "--no-run"], ["test", "showcase"]]);
    await handle.stop();
  } finally {
    f.cleanup();
  }
});

test("screenshot mode with a pattern overrides the default and a failed build runs no tests", async () => {
  const f = fixture((_args, call) => ({ code: call === 3 ? 1 : 0, stdout: "" }));
  try {
    const handle = await startDev({ ...f.options, mode: "shots", pattern: "gallery", packageName: "scenes" });
    assert.deepEqual(f.cargoCalls(), [["test", "-p", "scenes", "--no-run"], ["test", "-p", "scenes", "gallery"]]);
    f.change();
    f.clock.advance(120);
    await handle.scheduler.whenIdle();
    assert.equal(f.cargoCalls().length, 3, "the failed build ran no tests");
    await handle.stop();
  } finally {
    f.cleanup();
  }
});

test("only Rust sources outside target directories trigger rebuilds", () => {
  assert.equal(isRustSource("a/b.rs"), true);
  assert.equal(isRustSource("a/b.toml"), false);
  assert.equal(isRustSource("a/target/b.rs"), false);
  assert.equal(isRustSource(""), false);
  const seen = [];
  const watchers = [];
  const handle = watchRustSources(["/src"], (file) => seen.push(file), {
    watch: (directory, options, callback) => {
      watchers.push(callback);
      return { close: () => seen.push("closed") };
    },
  });
  watchers[0]("change", "x.rs");
  watchers[0]("change", "x.txt");
  watchers[0]("rename", null);
  watchers[0]("change", "target/y.rs");
  handle.close();
  assert.deepEqual(seen, [path.join("/src", "x.rs"), "closed"]);
});

test("the example executable is read from cargo's JSON output", () => {
  const output = ["Compiling...", artifact("a", "/bin/a"), "{not json", artifact("b", null), artifact("b", "/bin/b")].join("\n");
  assert.equal(findExampleExecutable(output, "a"), "/bin/a");
  assert.equal(findExampleExecutable(output, "b"), "/bin/b");
  assert.equal(findExampleExecutable(output, "c"), null);
});

test("launcher arguments: --app names an example, --shots optionally carries a pattern", () => {
  assert.deepEqual(parseArguments(["--app", "demo"]), { mode: "app", app: "demo", pattern: undefined, help: false });
  assert.equal(parseArguments(["--app=demo"]).app, "demo");
  assert.equal(parseArguments(["--app"]).app, undefined);
  assert.equal(parseArguments(["--app", "--package", "x"]).app, undefined);
  assert.equal(parseArguments([]).mode, "shots");
  assert.equal(parseArguments(["--shots"]).pattern, undefined);
  assert.equal(parseArguments(["--shots", "gallery"]).pattern, "gallery");
  assert.equal(parseArguments(["--shots=gallery", "-p", "crate", "--watch", "a", "--watch", "b", "--root", "/r"]).watchDirs.length, 2);
  assert.throws(() => parseArguments(["--app", "x", "--shots"]), /cannot be combined/);
  assert.throws(() => parseArguments(["--bogus"]), /unknown argument/);
  assert.throws(() => parseArguments(["--root"]), /needs a value/);
});
