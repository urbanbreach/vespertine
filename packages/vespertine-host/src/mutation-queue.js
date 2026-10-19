// Ordered mutation queue and the attempted-submission counter (APP-007 R2, APP-008 R1).

const COUNTER_KEY = Symbol.for("vespertine.host.mutationAttempts");

function counter() {
  return (globalThis[COUNTER_KEY] ??= { count: 0 });
}

/**
 * Process-wide count of nonempty mutation-batch submissions across all queues. A nonempty
 * flush advances it before the renderer is called, so a failing flush also advances it.
 */
export function mutationBatchAttempts() {
  return counter().count;
}

import { serializeCustomProp } from "./compare.js";

/**
 * @param {{ applyBatch(json: string): number[] }} host
 * @param {(destroyed: number[]) => void} [onDestroyed]
 * @param {(flush: () => void) => void} [scheduleFlush]
 */
export function createMutationQueue(host, onDestroyed, scheduleFlush) {
  let ops = [];
  let scheduled = false;
  let token = 0;

  function enqueue(op) {
    ops.push(op);
    if (scheduleFlush && !scheduled) {
      scheduled = true;
      const mine = ++token;
      scheduleFlush(() => {
        if (scheduled && token === mine) flushMutations();
      });
    }
  }

  function flushMutations() {
    scheduled = false;
    token++;
    if (ops.length === 0) return;
    counter().count++;
    const submitted = ops.length;
    const destroyed = host.applyBatch(JSON.stringify(ops));
    // Operations queued by the cleanup callback itself belong to the next batch.
    ops = ops.slice(submitted);
    onDestroyed?.(destroyed);
  }

  return {
    get pending() {
      return ops.length;
    },
    flushMutations,
    createElement(id, type) {
      enqueue(["createElement", id, type]);
    },
    destroyElement(id) {
      enqueue(["destroyElement", id]);
      return [];
    },
    appendChild(parentId, childId) {
      enqueue(["appendChild", parentId, childId]);
    },
    insertBefore(parentId, childId, beforeId) {
      enqueue(["insertBefore", parentId, childId, beforeId]);
    },
    setStyle(id, style) {
      enqueue(["setStyle", id, style]);
    },
    setText(id, text) {
      enqueue(["setText", id, text]);
    },
    setEventListener(id, eventType, hasHandler) {
      enqueue(["setEventListener", id, eventType, hasHandler]);
    },
    setRoot(id) {
      enqueue(["setRoot", id]);
    },
    setKeyboardFocusDim(enabled) {
      enqueue(["setKeyboardFocusDim", enabled]);
    },
    setCustomProp(id, key, value) {
      enqueue(["setCustomProp", id, key, serializeCustomProp(value)]);
    },
  };
}
