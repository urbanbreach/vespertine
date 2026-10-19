import { EventEmitter } from "node:events";

/** A manually advanced clock with the `setTimeout`/`clearTimeout` interface the scheduler uses. */
export function createFakeClock() {
  let now = 0;
  let nextId = 1;
  const timers = new Map();
  return {
    setTimeout(callback, ms) {
      const id = nextId++;
      timers.set(id, { at: now + ms, callback });
      return id;
    },
    clearTimeout(id) {
      timers.delete(id);
    },
    /** Advances time, firing due timers in order. */
    advance(ms) {
      const target = now + ms;
      for (;;) {
        const due = [...timers.entries()].filter(([, timer]) => timer.at <= target).sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].callback();
      }
      now = target;
    },
    get pending() {
      return timers.size;
    },
  };
}

/** Lets every pending promise continuation run. */
export function settle() {
  return new Promise((resolve) => setImmediate(resolve));
}

export function deferred() {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** A fake child process that records the signals it receives and exits on request. */
export function createFakeChild() {
  const child = new EventEmitter();
  child.exitCode = null;
  child.signalCode = null;
  child.signals = [];
  child.stdout = new EventEmitter();
  child.kill = (signal = "SIGTERM") => {
    child.signals.push(signal);
    return true;
  };
  child.finish = (code = 0, signal = null) => {
    child.exitCode = signal ? null : code;
    child.signalCode = signal;
    child.emit("exit", child.exitCode, signal);
  };
  return child;
}

export async function waitFor(condition, { timeout = 15000, interval = 25, message = "condition" } = {}) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await condition();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
  throw new Error(`timed out waiting for ${message}`);
}
