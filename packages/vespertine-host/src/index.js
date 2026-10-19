export { sameMutationValue, serializeCustomProp } from "./compare.js";
export { createMutationQueue, mutationBatchAttempts } from "./mutation-queue.js";
export { resolveFocusTarget } from "./focus-target.js";
export {
  createHandlerMap,
  registerEventHandler,
  unregisterEventHandler,
  unregisterEventHandlers,
} from "./handlers.js";
export { createRendererState, pushDismissLayer } from "./renderer-state.js";
export {
  observeSelectedText,
  observeWindowInsets,
  observeWindowSize,
  readWindowInsets,
  readWindowSize,
} from "./observers.js";
export { startFrameLoop } from "./frame-loop.js";
export { collectGarbageWhenIdle } from "./gc.js";
export { createNativeRenderer, isNonInteractive } from "./renderer.js";
export { loadNative } from "./native.js";
export { checkUpdate } from "./updater.js";
export {
  BUILTIN_TYPES,
  HostTree,
  NATIVE_ELEMENT_TYPES,
  RESERVED_PROPS,
  UNIVERSAL_PROPS,
  createHostNode,
  createHostTree,
  createTextNode,
  insertNode,
  removeNode,
  setNodeHidden,
  setNodeProp,
  setNodeProps,
  setNodeText,
  eventTypeOf,
  isEventProp,
  isHostNode,
} from "./host-tree.js";
