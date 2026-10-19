// Controlled and uncontrolled state for compound parts (CMP-002 R12).

import React from "react";
import { isControlled, requestControlledChange } from "@vespertine/host";

/**
 * `[value, request]`: an undefined `value` means uncontrolled, `defaultValue` only initializes
 * the internal state, and `request(next)` calls `onChange` only for a value that is not
 * identical to the current one. A controlled request never overwrites the supplied value.
 */
export function useControllableState({ value, defaultValue, onChange }) {
  const [internal, setInternal] = React.useState(defaultValue);
  const controlled = isControlled(value);
  const current = controlled ? value : internal;
  const latest = React.useRef({ current, controlled, onChange });
  latest.current = { current, controlled, onChange };
  const request = React.useCallback((next) => {
    const state = latest.current;
    return requestControlledChange({
      controlled: state.controlled,
      current: state.current,
      next,
      onChange: state.onChange,
      // Later requests in the same event see the value this one set.
      setInternal: (value) => {
        latest.current = { ...state, current: value };
        setInternal(value);
      },
    });
  }, []);
  return [current, request];
}
