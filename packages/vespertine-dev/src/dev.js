import { spawn as nodeSpawn } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";

import { createAppRunner } from "./runner.js";
import { createRebuildScheduler } from "./scheduler.js";
import { watchRustSources } from "./watch.js";

/** Test name pattern the screenshot mode runs unless one is supplied. */
export const DEFAULT_SHOTS_PATTERN = "showcase";

/** Finds the executable of the example `name` in `cargo build --message-format=json` output. */
export function findExampleExecutable(stdout, name) {
  let executable = null;
  for (const line of stdout.split("\n")) {
    if (!line.startsWith("{")) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (
      message.reason === "compiler-artifact" &&
      message.executable &&
      message.target?.name === name &&
      message.target.kind?.includes("example")
    ) {
      executable = message.executable;
    }
  }
  return executable;
}

/**
 * Starts the native development loop: builds once, then rebuilds whenever Rust sources change.
 *
 * Application mode (`mode: "app"`) restarts the example `app` after each successful build;
 * screenshot mode (`mode: "shots"`) runs `cargo test <pattern>` after each successful build.
 * Resolves once the initial build (and its follow-up action) has finished.
 *
 * @param {object} options
 * @param {"app" | "shots"} options.mode
 * @param {string} [options.app] example name, required in application mode
 * @param {string} [options.pattern] test-name pattern of screenshot mode
 * @param {string} [options.root] workspace root (default: the current directory)
 * @param {string} [options.packageName] cargo package (`-p`) to build and test
 * @param {string[]} [options.watchDirs] directories to watch (default: `<root>/crates`)
 * @param {string} [options.cargo] cargo program (default: `$CARGO` or `cargo`)
 * @param {NodeJS.ProcessEnv} [options.env]
 * @param {boolean} [options.handleSignals] terminate on SIGINT/SIGTERM (child first)
 * @param {(code: number) => void} [options.exit] called after a handled signal's shutdown
 * @param {(message: string) => void} [options.log]
 * @param {Function} [options.spawn] `child_process.spawn` replacement
 * @param {object} [options.clock] scheduler clock
 * @param {typeof import("node:fs").watch} [options.watch] `fs.watch` replacement
 */
export async function startDev({
  mode,
  app,
  pattern,
  root = process.cwd(),
  packageName,
  watchDirs,
  env = process.env,
  cargo = env.CARGO || "cargo",
  handleSignals = false,
  exit = (code) => process.exit(code),
  log = (message) => process.stderr.write(`[vespertine-dev] ${message}\n`),
  spawn = nodeSpawn,
  clock,
  watch,
}) {
  if (mode !== "app" && mode !== "shots") {
    throw new Error(`Unknown mode "${mode}": expected "app" or "shots"`);
  }
  if (mode === "app" && (typeof app !== "string" || app === "")) {
    throw new Error("Application mode needs the name of the example to run (--app <example>)");
  }
  const directories = (watchDirs ?? [path.join(root, "crates")]).map((dir) => path.resolve(root, dir));
  for (const directory of directories) {
    if (!existsSync(directory)) {
      throw new Error(`Rust source directory not found: ${directory} (choose another with --watch)`);
    }
  }
  const testPattern = pattern || DEFAULT_SHOTS_PATTERN;
  const packageArgs = packageName ? ["-p", packageName] : [];
  const active = new Set();
  const runner = createAppRunner({ spawn });
  let executable = null;
  let stopping = false;

  /** Runs a command to completion, optionally capturing stdout; its child can be terminated on stop. */
  function execute(command, args, { capture = false } = {}) {
    return new Promise((resolve) => {
      let child;
      try {
        child = spawn(command, args, {
          cwd: root,
          env,
          stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit",
        });
      } catch (error) {
        log(`could not run ${command}: ${error.message}`);
        resolve({ code: null, stdout: "" });
        return;
      }
      active.add(child);
      let stdout = "";
      child.stdout?.on("data", (chunk) => {
        stdout += chunk;
      });
      child.once("error", (error) => {
        active.delete(child);
        log(`could not run ${command}: ${error.message}`);
        resolve({ code: null, stdout });
      });
      child.once("exit", (code) => {
        active.delete(child);
        resolve({ code, stdout });
      });
    });
  }

  async function build() {
    log(mode === "app" ? `building example ${app}` : "building tests");
    const args =
      mode === "app"
        ? ["build", ...packageArgs, "--example", app, "--message-format=json-render-diagnostics"]
        : ["test", ...packageArgs, "--no-run"];
    const result = await execute(cargo, args, { capture: mode === "app" });
    if (result.code !== 0) {
      log("build failed; keeping the previous result and waiting for changes");
      return false;
    }
    if (mode === "app") {
      executable = findExampleExecutable(result.stdout, app);
      if (!executable) {
        log(`cargo reported no executable for example ${app}`);
        return false;
      }
    }
    return true;
  }

  async function onBuilt() {
    if (mode === "app") {
      log(`restarting ${app}`);
      await runner.restart(executable, [], { cwd: root, env });
      return;
    }
    log(`running tests matching "${testPattern}"`);
    const result = await execute(cargo, ["test", ...packageArgs, testPattern]);
    if (result.code !== 0) log("the screenshot tests failed");
  }

  const scheduler = createRebuildScheduler({
    clock,
    build,
    onBuilt,
    onError: (error) => log(`error: ${error?.message ?? error}`),
  });
  const watcher = watchRustSources(directories, () => scheduler.notifyChange(), { watch });

  let stopped = null;
  function stop() {
    stopped ??= (async () => {
      stopping = true;
      watcher.close();
      const finished = scheduler.stop();
      for (const child of active) child.kill("SIGTERM");
      await finished;
      await runner.stop();
    })();
    return stopped;
  }

  const handlers = new Map();
  if (handleSignals) {
    for (const [signal, code] of [["SIGINT", 130], ["SIGTERM", 143]]) {
      const handler = () => {
        if (stopping) return;
        log(`${signal} received; shutting down`);
        stop().then(() => exit(code), (error) => {
          log(`error while shutting down: ${error?.message ?? error}`);
          exit(code);
        });
      };
      handlers.set(signal, handler);
      process.on(signal, handler);
    }
  }

  const handle = {
    scheduler,
    runner,
    /** Stops watching, terminates every child process and waits for them. */
    async stop() {
      for (const [signal, handler] of handlers) process.off(signal, handler);
      await stop();
    },
  };
  await scheduler.start();
  return handle;
}
