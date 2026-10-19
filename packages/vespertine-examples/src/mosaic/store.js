// The external store the mosaic subscribes to (`subscribe` and `getSnapshot`) and the scan
// controller behind it (APP-011 R4): a new scan aborts the previous one, the previous completed
// tree stays displayed while a rescan runs, scan progress is published every 100 ms from the live
// counters, and a completion or error from a superseded scan never replaces newer state.

export const PROGRESS_INTERVAL_MS = 100;

const freshCounters = () => ({ files: 0, directories: 0, bytes: 0, errors: 0 });

/**
 * @param {{ scan: (request: { signal: AbortSignal, counters: object, includeHidden: boolean }) => Promise<object> }} source
 */
export function createMosaicStore({ scan }) {
  const listeners = new Set();
  let snapshot = { tree: null, scanning: false, progress: freshCounters(), error: null };
  let generation = 0;
  let controller = null;
  let timer = null;

  const publish = (next) => {
    snapshot = { ...snapshot, ...next };
    for (const listener of [...listeners]) listener();
  };

  function stopTimer() {
    if (timer !== null) {
      globalThis.clearInterval(timer);
      timer = null;
    }
  }

  return {
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getSnapshot: () => snapshot,
    /** Starts a scan; the previous one is aborted and its late result ignored. */
    startScan({ includeHidden = true } = {}) {
      controller?.abort();
      stopTimer();
      const mine = ++generation;
      controller = new AbortController();
      const counters = freshCounters();
      publish({ scanning: true, error: null, progress: { ...counters } });
      timer = globalThis.setInterval(() => {
        if (mine === generation) publish({ progress: { ...counters } });
      }, PROGRESS_INTERVAL_MS);
      timer?.unref?.();
      scan({ signal: controller.signal, counters, includeHidden }).then(
        (tree) => {
          if (mine !== generation) return;
          stopTimer();
          publish({ tree, scanning: false, progress: { ...counters } });
        },
        (error) => {
          if (mine !== generation) return;
          stopTimer();
          publish({ scanning: false, error, progress: { ...counters } });
        },
      );
      return mine;
    },
    dispose() {
      generation++;
      controller?.abort();
      stopTimer();
      listeners.clear();
    },
  };
}
