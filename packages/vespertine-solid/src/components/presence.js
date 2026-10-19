// Exit lifecycle of headless popup content (CMP-002 R17): a closing layer stays mounted, but
// inert, until its native exit animation completes, while its own Escape registration and focus
// handling follow the close from the moment it begins.
//
// Same behavior as the React adapter's `usePresence`, expressed with accessors.

import { createComputed, createEffect, createMemo, createSignal, onCleanup, untrack } from "solid-js";

/** Time added to the exit duration before the fallback unmounts content whose completion never arrived. */
const FALLBACK_SLACK_MS = 20;

/**
 * Tracks the mounted state of popup content.
 *
 * `open` is an accessor of the open state; `options` (an object or an accessor of one) holds
 * `initial`, `animate`, `exit` (the native exit target, for example `{ opacity: 0 }`) and
 * `transition`. Without an exit target the content unmounts as soon as `open` turns false. With
 * one, the `motion` property drives the native track (`present` false makes it an exit track
 * and each open or close carries a new completion generation), `onMotionComplete` unmounts when
 * the current generation's exit completes (a stale generation is ignored), and a delayed unmount
 * at the end of the exit duration (`transition.duration`, seconds, default 0.3, plus the delay)
 * plus 20 ms is the fallback.
 *
 * @returns {{ mounted: () => boolean, closing: () => boolean, motion: () => object | undefined,
 *            onMotionComplete: (event: object) => void }}
 */
export function createPresence(open, options = {}) {
  const read = () => (typeof options === "function" ? options() : options) ?? {};
  const hasExit = () => read().exit !== undefined && read().exit !== null;
  const [mounted, setMounted] = createSignal(untrack(open));
  const [generation, setGeneration] = createSignal(0);

  // Every open or close starts a new generation; opening mounts at once.
  createComputed((previous) => {
    const now = open();
    if (previous !== now) setGeneration((value) => value + 1);
    if (now) setMounted(true);
    return now;
  }, untrack(open));

  const unmountAt = () => {
    const { transition } = read();
    return ((transition?.duration ?? 0.3) + (transition?.delay ?? 0)) * 1000 + FALLBACK_SLACK_MS;
  };
  createEffect(() => {
    if (open() || !mounted() || !hasExit()) return;
    const seen = untrack(generation);
    const timer = setTimeout(() => {
      if (untrack(generation) === seen) setMounted(false);
    }, unmountAt());
    onCleanup(() => clearTimeout(timer));
  });

  const onMotionComplete = (event) => {
    if (event?.exit && event.generation === untrack(generation)) setMounted(false);
  };

  const shown = () => open() || (hasExit() && mounted());
  const motion = createMemo(() => {
    if (!hasExit()) return undefined;
    const { initial, animate, exit, transition } = read();
    return { ...(initial === undefined ? {} : { initial }), animate: animate ?? {}, exit, transition, present: open(), generation: generation() };
  });
  return { mounted: shown, closing: () => !open() && shown(), motion, onMotionComplete };
}
