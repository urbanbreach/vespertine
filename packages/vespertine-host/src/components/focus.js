// Focus restoration for dismissal layers (CMP-002 R10).

/** The host node with identity `id` under `node`'s tree, or null. */
function findById(root, id) {
  if (root.id === id) return root;
  for (const child of root.children) {
    const found = findById(child, id);
    if (found) return found;
  }
  return null;
}

/** Whether the node with identity `id` is `ancestor` or one of its descendants. */
export function isWithin(ancestor, id) {
  return ancestor != null && findById(ancestor, id) !== null;
}

/**
 * The default final focus of a layer: the captured prior focus, but only if focus is still inside
 * the layer or nowhere; focus that an outside press moved elsewhere is kept. A prior target that
 * is no longer mounted falls back to the owning trigger when it is mounted. With `triggerFirst`
 * (dialogs, CMP-003 R6) the mounted trigger wins over the captured target.
 *
 * @param {object} renderer
 * @param {{ layer: () => object | null, trigger?: () => object | null, triggerFirst?: boolean }} nodes
 *   the layer's node and its trigger's node (read when the layer closes)
 */
export function defaultFinalFocus(renderer, { layer, trigger, triggerFirst = false }) {
  return (previous) => {
    const focused = renderer.getFocusedElement?.() ?? null;
    const layerNode = layer();
    const inside = focused === null || focused === undefined || isWithin(layerNode, focused);
    if (!inside) return null;
    const triggerNode = trigger?.() ?? null;
    if (triggerFirst && triggerNode?.mounted) return triggerNode.id;
    const root = layerNode?.tree?.container ?? triggerNode?.tree?.container ?? null;
    const previousMounted = previous !== null && previous !== undefined && (root === null || findById(root, previous) !== null);
    if (previousMounted) return previous;
    return triggerNode?.mounted ? triggerNode.id : null;
  };
}
