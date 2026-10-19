// The headless trigger press flag (CMP-002 R13): a trigger press raises a flag that makes the
// popup's outside-press handling ignore that press; the flag lasts only through the current
// microtask, and the click that follows then toggles the popup.

/**
 * Creates the press flag of one popup root (its trigger raises it; its dismissable layer asks
 * `consumes` before dismissing on an outside press).
 *
 * `consumes()` is true while the flag is up. `defer(fn)` runs `fn` once the current task's
 * handlers have run (a microtask) unless the flag is raised meanwhile or before: the outside
 * press may be delivered before or after the trigger's own press, and both orders leave the
 * popup to the click.
 */
export function createPressFlag(schedule = queueMicrotask) {
  let raised = false;
  let generation = 0;
  return {
    raise() {
      raised = true;
      generation++;
      schedule(() => {
        raised = false;
      });
    },
    consumes() {
      return raised;
    },
    defer(fn) {
      const seen = generation;
      if (raised) return;
      schedule(() => {
        if (!raised && seen === generation) fn();
      });
    },
  };
}
