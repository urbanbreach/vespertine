// Style helpers shared by the headless components (CMP-001 R3, CMP-002 public API).

const isStyleObject = (value) => typeof value === "object" && value !== null;

/** Shallow merge of style objects; later styles win. Non-objects are skipped. */
export function mergeStyles(...styles) {
  const merged = {};
  for (const style of styles) {
    if (isStyleObject(style)) Object.assign(merged, style);
  }
  return merged;
}

/** A style given as an object or as a function of the component state, resolved to an object. */
export function resolveStyle(style, state) {
  const resolved = typeof style === "function" ? style(state) : style;
  return isStyleObject(resolved) ? resolved : undefined;
}
