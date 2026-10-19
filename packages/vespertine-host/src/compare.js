// Structural comparison of mutation values (APP-007 R3).

const objectProto = Object.prototype;

function isPlain(value) {
  const proto = Object.getPrototypeOf(value);
  return proto === objectProto || proto === null;
}

function definedKeys(value) {
  const keys = [];
  for (const key of Object.keys(value)) {
    if (value[key] !== undefined) keys.push(key);
  }
  return keys;
}

/**
 * Structural equality for styles and custom property values.
 *
 * `undefined` object properties are absent, NaN equals NaN, -0 equals 0, arrays compare
 * positionally, plain objects structurally; class instances and functions only by identity.
 */
export function sameMutationValue(a, b) {
  if (a === b) return true;
  if (typeof a === "number" && typeof b === "number") return a !== a && b !== b;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  const aArray = Array.isArray(a);
  const bArray = Array.isArray(b);
  if (aArray || bArray) {
    if (!aArray || !bArray || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      if (!sameMutationValue(a[i], b[i])) return false;
    }
    return true;
  }
  if (!isPlain(a) || !isPlain(b) || Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) {
    return false;
  }
  const aKeys = definedKeys(a);
  if (aKeys.length !== definedKeys(b).length) return false;
  for (const key of aKeys) {
    if (b[key] === undefined || !sameMutationValue(a[key], b[key])) return false;
  }
  return true;
}

/** Custom property serialization: undefined and functions become null (removal). */
export function serializeCustomProp(value) {
  return value === undefined || typeof value === "function" ? null : value;
}
