/** Quiet period after the last change event before a rebuild starts. */
export const DEBOUNCE_MS = 120;

const realClock = {
  setTimeout: (callback, ms) => setTimeout(callback, ms),
  clearTimeout: (handle) => clearTimeout(handle),
};

/**
 * Pure rebuild scheduling: debounces bursts of change events into one build, never runs two
 * builds at once, and keeps at most one rebuild queued while a build runs.
 *
 * - `clock` has `setTimeout(callback, ms)` and `clearTimeout(handle)`; tests inject a fake.
 * - `build()` resolves `true` for a successful build and `false` for a failed one (a rejection
 *   counts as a failure and is reported through `onError`).
 * - `onBuilt()` runs after each successful build and is awaited before the next build starts;
 *   it is never called for a failed build, so whatever it replaces keeps running.
 *
 * @param {object} options
 * @param {{ setTimeout: Function, clearTimeout: Function }} [options.clock]
 * @param {() => Promise<boolean> | boolean} options.build
 * @param {() => Promise<void> | void} [options.onBuilt]
 * @param {(error: unknown) => void} [options.onError]
 * @param {number} [options.debounceMs]
 */
export function createRebuildScheduler({ clock = realClock, build, onBuilt, onError, debounceMs = DEBOUNCE_MS }) {
  let timer = null;
  let building = false;
  let queued = false;
  let stopped = false;
  let waiters = [];

  const report = (error) => {
    if (onError) onError(error);
  };

  function clearTimer() {
    if (timer !== null) {
      clock.clearTimeout(timer);
      timer = null;
    }
  }

  function settle() {
    if (building || timer !== null) return;
    const done = waiters;
    waiters = [];
    for (const resolve of done) resolve();
  }

  async function cycle() {
    building = true;
    try {
      do {
        queued = false;
        let ok = false;
        try {
          ok = (await build()) !== false;
        } catch (error) {
          report(error);
        }
        if (ok && !stopped) {
          try {
            await onBuilt?.();
          } catch (error) {
            report(error);
          }
        }
      } while (queued && !stopped);
    } finally {
      building = false;
      settle();
    }
  }

  function requestBuild() {
    clearTimer();
    if (building) {
      queued = true;
      return;
    }
    void cycle();
  }

  return {
    /** Runs the initial build. */
    start() {
      requestBuild();
      return this.whenIdle();
    },

    /** A source change: (re)starts the quiet period, or queues the single follow-up build. */
    notifyChange() {
      if (stopped) return;
      if (building) {
        queued = true;
        clearTimer();
        return;
      }
      clearTimer();
      timer = clock.setTimeout(() => {
        timer = null;
        requestBuild();
      }, debounceMs);
    },

    /** Stops scheduling; resolves once a running build (and its follow-up) has finished. */
    stop() {
      stopped = true;
      queued = false;
      clearTimer();
      settle();
      return this.whenIdle();
    },

    /** Resolves when no build is running and none is waiting on the quiet period. */
    whenIdle() {
      if (!building && timer === null) return Promise.resolve();
      return new Promise((resolve) => waiters.push(resolve));
    },

    get building() {
      return building;
    },

    get queued() {
      return queued;
    },
  };
}
