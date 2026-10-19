// Controlled and uncontrolled state for compound parts (CMP-002 R12).

import { createSignal } from "solid-js";
import { isControlled, requestControlledChange } from "@vespertine/host";

/**
 * `[value, request]` where `value` is an accessor. `value` (an accessor, read on every use) is
 * the controlled value: undefined means uncontrolled. `defaultValue` only initializes the
 * internal state. `request(next)` calls `onChange` only for a value that is not identical to
 * the current one and returns whether it did; a controlled request never overwrites the
 * supplied value, an uncontrolled one updates the internal signal at once (later requests in
 * the same event see it).
 *
 * @param {{ value?: () => unknown, defaultValue?: unknown, onChange?: (value: any) => void }} options
 * @returns {[() => any, (next: any) => boolean]}
 */
export function createControllableState({ value = () => undefined, defaultValue, onChange } = {}) {
  const [internal, setInternal] = createSignal(defaultValue);
  const controlled = () => isControlled(value());
  const current = () => (controlled() ? value() : internal());
  const request = (next) =>
    requestControlledChange({
      controlled: controlled(),
      current: current(),
      next,
      onChange,
      // A function value must not be taken for a signal updater.
      setInternal: (stored) => setInternal(() => stored),
    });
  return [current, request];
}
