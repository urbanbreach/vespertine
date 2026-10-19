// One-shot garbage collection after idle bursts (APP-008).

import { mutationBatchAttempts } from "./mutation-queue.js";

function processCpuMs() {
  try {
    const usage = globalThis.process?.cpuUsage?.();
    return usage ? (usage.user + usage.system) / 1000 : 0;
  } catch {
    return 0;
  }
}

/**
 * The runtime's forced full collection: Bun's, else a globally exposed `gc` (for example
 * Node started with `--expose-gc`); undefined when neither exists.
 */
export function defaultCollector(globals = globalThis) {
  const bun = globals.Bun;
  if (bun && typeof bun.gc === "function") return () => bun.gc(true);
  if (typeof globals.gc === "function") return () => globals.gc();
  return undefined;
}

/**
 * Requests one full collection once declarative activity and process CPU use have both
 * settled; returns a stop function.
 */
export function collectGarbageWhenIdle({
  activity = mutationBatchAttempts,
  collect = defaultCollector(),
  checkMs = 5000,
  quietChecks = 2,
  busyCpu = 0.5,
} = {}) {
  if (!collect) return () => {};
  let lastActivity = activity();
  let lastCpu = processCpuMs();
  let lastWall = Date.now();
  let quiet = 0;
  let armed = true;

  const timer = globalThis.setInterval(() => {
    const wall = Date.now();
    const cpu = processCpuMs();
    const elapsed = wall - lastWall;
    const ratio = elapsed > 0 ? (cpu - lastCpu) / elapsed : 0;
    const count = activity();
    const active = count !== lastActivity || ratio >= busyCpu;
    lastWall = wall;
    lastCpu = cpu;
    lastActivity = count;
    if (active) {
      quiet = 0;
      armed = true;
      return;
    }
    quiet++;
    if (quiet >= quietChecks && armed) {
      armed = false;
      collect();
    }
  }, checkMs);
  timer.unref?.();
  return () => globalThis.clearInterval(timer);
}
