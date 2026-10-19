import { createNativeRenderer } from "@vespertine/host";
import { createReferenceNative } from "@vespertine/host/reference";
import { createElement, effect, insert, setProp } from "../src/index.js";

export function referenceRenderer(options = {}) {
  const errors = [];
  const renderer = createNativeRenderer({
    native: createReferenceNative(),
    onError: (error) => errors.push(error),
    ...options,
  });
  renderer.errors = errors;
  return renderer;
}

/**
 * Builds an element without JSX: static props, accessor props (`() => value` for a name that is
 * not an event handler) and children (strings, nodes or accessors).
 */
export function h(type, props = {}, ...children) {
  const element = createElement(type);
  for (const [name, value] of Object.entries(props)) {
    if (typeof value === "function" && !/^on[A-Z]/.test(name) && name !== "ref") {
      effect(() => setProp(element, name, value()));
    } else if (name === "ref") {
      value(element);
    } else {
      setProp(element, name, value);
    }
  }
  for (const child of children) insert(element, child);
  return element;
}

export async function settle(times = 3) {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0));
}
