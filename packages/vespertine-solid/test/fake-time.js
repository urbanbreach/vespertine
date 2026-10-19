// Virtual time for runtime-neutral (node and bun) tests.

export function installFakeTime() {
  const real = {
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
    setInterval: globalThis.setInterval,
    clearInterval: globalThis.clearInterval,
    now: Date.now,
  };
  let now = 1_000_000;
  let nextId = 1;
  const timers = new Map();

  const add = (fn, delay, repeat) => {
    const handle = { id: nextId++, unref() { return handle; }, ref() { return handle; } };
    timers.set(handle.id, { handle, fn, delay: Math.max(0, delay ?? 0), at: now + Math.max(0, delay ?? 0), repeat });
    return handle;
  };
  globalThis.setTimeout = (fn, delay) => add(fn, delay, false);
  globalThis.setInterval = (fn, delay) => add(fn, delay, true);
  const clear = (handle) => {
    if (handle) timers.delete(handle.id);
  };
  globalThis.clearTimeout = clear;
  globalThis.clearInterval = clear;
  Date.now = () => now;

  return {
    get now() {
      return now;
    },
    get pendingTimers() {
      return timers.size;
    },
    /** Advances virtual time, running due timers in order. */
    advance(ms) {
      const end = now + ms;
      for (;;) {
        let due = null;
        for (const timer of timers.values()) {
          if (timer.at <= end && (due === null || timer.at < due.at || (timer.at === due.at && timer.handle.id < due.handle.id))) {
            due = timer;
          }
        }
        if (!due) break;
        now = due.at;
        if (due.repeat) due.at += due.delay || 1;
        else timers.delete(due.handle.id);
        due.fn();
      }
      now = Math.max(now, end);
    },
    /** Advances `now` without running timers (simulates time spent working). */
    spend(ms) {
      now += ms;
    },
    restore() {
      Object.assign(globalThis, {
        setTimeout: real.setTimeout,
        clearTimeout: real.clearTimeout,
        setInterval: real.setInterval,
        clearInterval: real.clearInterval,
      });
      Date.now = real.now;
    },
  };
}
