// Focus-target configuration (APP-007 R6).

function isNode(value) {
  return typeof value === "object" && value !== null && typeof value.id === "number";
}

function isRef(value) {
  return typeof value === "object" && value !== null && "current" in value && !isNode(value);
}

/**
 * Resolves an initial/final focus option to an element identity, or null when focus must
 * not move or the fallback is itself null.
 *
 * Unset, null and true select the fallback (a value, or a function producing it); false
 * preserves focus; a node yields its identity; a ref yields its current node (the fallback
 * while empty); a function is called first and a function returning undefined preserves focus.
 */
export function resolveFocusTarget(target, fallback) {
  const useFallback = () => {
    const value = typeof fallback === "function" ? fallback() : fallback;
    return value === undefined ? null : value;
  };
  let value = target;
  if (typeof value === "function") {
    value = value();
    if (value === undefined) return null;
  } else if (value === undefined) {
    return useFallback();
  }
  if (value === null || value === true) return useFallback();
  if (value === false) return null;
  if (isRef(value)) return value.current == null ? useFallback() : resolveNode(value.current);
  return resolveNode(value);
}

function resolveNode(value) {
  if (isNode(value)) return value.id;
  throw new TypeError("a focus target must be a boolean, a node, a node reference or a function returning one");
}
