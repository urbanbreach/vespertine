import { spawn as nodeSpawn } from "node:child_process";

/** Signal that terminates the application on a rebuild and on shutdown. */
export const TERMINATION_SIGNAL = "SIGTERM";

function exited(child) {
  return child.exitCode != null || child.signalCode != null;
}

/**
 * Owns at most one application process: `restart` terminates the previous one with SIGTERM,
 * waits for it to exit, then starts the new one.
 *
 * @param {object} options
 * @param {(command: string, args: string[], options: object) => import("node:child_process").ChildProcess} [options.spawn]
 */
export function createAppRunner({ spawn = nodeSpawn } = {}) {
  let current = null;
  let exitPromise = null;

  async function terminate() {
    if (!current) return;
    const child = current;
    const waiting = exitPromise;
    current = null;
    exitPromise = null;
    if (!exited(child)) child.kill(TERMINATION_SIGNAL);
    await waiting;
  }

  return {
    /** Terminates the running application (if any) and starts `command args` in its place. */
    async restart(command, args, options = {}) {
      await terminate();
      const child = spawn(command, args, { stdio: "inherit", ...options });
      current = child;
      exitPromise = new Promise((resolve) => {
        if (exited(child)) resolve();
        child.once("exit", resolve);
        child.once("error", resolve);
      });
      return child;
    },

    /** Terminates the running application and waits until it has exited. */
    stop: terminate,

    get child() {
      return current;
    },
  };
}
