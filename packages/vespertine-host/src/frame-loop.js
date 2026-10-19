// Frame loop (APP-007 R8).

const DEFAULT_FRAME_MS = 8;
const WAKE_SAFETY_MS = 100;

/**
 * Drives `renderer.tick()`. Returns `{ stop }`; inert when the renderer needs no ticking.
 */
export function startFrameLoop(renderer, { frameMs, onTerminated, onError } = {}) {
  if (typeof renderer.requiresTick === "function" && !renderer.requiresTick()) {
    return { stop() {} };
  }
  let stopped = false;
  let timer = null;
  let wakeRegistered = false;

  const clearTimer = () => {
    if (timer !== null) {
      globalThis.clearTimeout(timer);
      timer = null;
    }
  };

  function stop() {
    if (stopped) return;
    stopped = true;
    clearTimer();
    if (wakeRegistered) {
      wakeRegistered = false;
      renderer.setFrameWakeCallback(null);
    }
  }

  function tick() {
    clearTimer();
    if (stopped) return;
    const started = Date.now();
    let alive = true;
    try {
      alive = renderer.tick();
    } catch (error) {
      onError?.(error);
    }
    if (alive === false) {
      stop();
      onTerminated?.();
      return;
    }
    if (stopped) return;
    const delay = wakeRegistered
      ? WAKE_SAFETY_MS
      : Math.max(0, (frameMs ?? DEFAULT_FRAME_MS) - (Date.now() - started));
    timer = globalThis.setTimeout(tick, delay);
  }

  if (frameMs === undefined && typeof renderer.setFrameWakeCallback === "function") {
    wakeRegistered = renderer.setFrameWakeCallback(() => {
      if (!stopped) tick();
    }) === true;
  }
  tick();
  return { stop };
}
