import { createNativeRenderer } from "@vespertine/host";
import { createReferenceNative } from "@vespertine/host/reference";
import { createRoot, flushSync } from "@vespertine/react";

export function mount(element) {
  const renderer = createNativeRenderer({ native: createReferenceNative(), onError: (error) => { throw error; } });
  const root = createRoot(renderer);
  flushSync(() => root.render(element));
  return { renderer, root, find: (testId) => findByTestId(renderer, testId), emit: (testId, event) => emitTo(renderer, testId, event) };
}

/** Finds the element with `testId` in a snapshot or in the reference renderer's element table. */
export function findByTestId(source, testId) {
  if (source && source.elements instanceof Map) {
    for (const [id, element] of source.elements) {
      if (element.props.get("testId") === testId) return source.snapshot(id);
    }
    return null;
  }
  const node = source;
  if (!node) return null;
  if (node.props.testId === testId) return node;
  for (const child of node.children) {
    const found = findByTestId(child, testId);
    if (found) return found;
  }
  return null;
}

export function emitTo(renderer, testId, event) {
  const node = findByTestId(renderer, testId);
  if (!node) throw new Error(`no element with testId ${testId}`);
  renderer.emit({ id: node.id, ...event });
}

export function textOf(node) {
  return node.text ?? node.children.map(textOf).join("");
}

export function allNodes(node, out = []) {
  out.push(node);
  for (const child of node.children) allNodes(child, out);
  return out;
}

/** Lets React finish pending work after an external store update. */
export function flushForTest() {
  flushSync(() => {});
}
