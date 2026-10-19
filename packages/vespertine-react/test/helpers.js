// Shared fixtures: a renderer over the JavaScript reference tree and React helpers.

import { createNativeRenderer } from "@vespertine/host";
import { createReferenceNative } from "@vespertine/host/reference";

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

/** Lets pending React work and microtasks run. */
export async function settle(times = 3) {
  for (let i = 0; i < times; i++) await new Promise((resolve) => setTimeout(resolve, 0));
}
