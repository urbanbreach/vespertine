// Exit lifecycle of headless popup content (CMP-002 R17): a closing layer stays mounted, but
// inert, until its native exit animation completes, while its own Escape registration and focus
// handling follow the close from the moment it begins.

import React from "react";

/** Time added to the exit duration before the fallback unmounts content whose completion never arrived. */
const FALLBACK_SLACK_MS = 20;

/**
 * Tracks the mounted state of popup content.
 *
 * `exit` is the native exit target (for example `{ opacity: 0 }`); without one the content
 * unmounts as soon as `open` turns false. With one, the returned `motion` property drives the
 * native track (`present` false makes it an exit track and each open or close carries a new
 * completion generation), `onMotionComplete` unmounts when the current generation's exit
 * completes (a stale generation is ignored), and a delayed unmount at the end of the exit
 * duration (`transition.duration`, seconds, default 0.3) is the fallback.
 *
 * @returns {{ mounted: boolean, closing: boolean, motion: object | undefined, onMotionComplete: Function }}
 */
export function usePresence(open, { initial, animate, exit, transition } = {}) {
  const hasExit = exit !== undefined && exit !== null;
  const [mounted, setMounted] = React.useState(open);
  const generation = React.useRef(0);
  const previous = React.useRef(open);
  if (previous.current !== open) {
    previous.current = open;
    generation.current++;
  }
  if (open && !mounted) setMounted(true);

  const unmountAt = hasExit ? ((transition?.duration ?? 0.3) + (transition?.delay ?? 0)) * 1000 + FALLBACK_SLACK_MS : 0;
  React.useEffect(() => {
    if (open || !mounted || !hasExit) return undefined;
    const seen = generation.current;
    const timer = setTimeout(() => {
      if (generation.current === seen) setMounted(false);
    }, unmountAt);
    return () => clearTimeout(timer);
  }, [open, mounted, hasExit, unmountAt]);

  const onMotionComplete = React.useCallback(
    (event) => {
      if (event?.exit && event.generation === generation.current) setMounted(false);
    },
    [],
  );

  const shown = open || (hasExit && mounted);
  return {
    mounted: shown,
    closing: !open && shown,
    motion: hasExit
      ? { ...(initial === undefined ? {} : { initial }), animate: animate ?? {}, exit, transition, present: open, generation: generation.current }
      : undefined,
    onMotionComplete,
  };
}
