// Controlled and uncontrolled state requests (CMP-002 R12).

/**
 * Applies a requested value. Returns whether a change was requested: an identical request does
 * nothing, otherwise the change callback runs with the requested value and, only when the state
 * is uncontrolled (`controlled` is false), the internal state is updated through `setInternal`.
 * A controlled request never overwrites the supplied value; the owner decides.
 *
 * @param {{ controlled: boolean, current: unknown, next: unknown,
 *           onChange?: (value: any) => void, setInternal?: (value: any) => void }} request
 */
export function requestControlledChange({ controlled, current, next, onChange, setInternal }) {
  if (Object.is(current, next)) return false;
  onChange?.(next);
  if (!controlled) setInternal?.(next);
  return true;
}

/** Whether a value prop makes the state controlled: an undefined value means uncontrolled. */
export function isControlled(value) {
  return value !== undefined;
}
