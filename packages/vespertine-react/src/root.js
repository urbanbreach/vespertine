// React roots on a native renderer (APP-009 R1, R5, R8, R10).

import React from "react";
import { createHostTree, createNativeRenderer } from "@vespertine/host";
import { createPortalObject, reconciler } from "./reconciler.js";

const ConcurrentRoot = 1;

/** The context every root provides. */
export const RendererContext = React.createContext(null);

/**
 * The renderer context of the nearest root, or null outside any root.
 * @returns {{ renderer: object, root: object } | null}
 */
export function useOptionalRendererContext() {
  return React.useContext(RendererContext);
}

/**
 * The renderer context of the nearest root; throws outside any root.
 * @returns {{ renderer: object, root: object }}
 */
export function useRendererContext() {
  const context = React.useContext(RendererContext);
  if (context === null) throw new Error("This hook must be used inside a Vespertine React root");
  return context;
}

/** The native renderer of the nearest root; throws outside any root. */
export function useRenderer() {
  return useRendererContext().renderer;
}

/**
 * Creates the native renderer for React roots: native events are routed to the active root and
 * every handled event also reaches `onEvent`.
 *
 * @param {Function} [onEvent]
 * @param {{ onError?: Function, native?: object }} [options]
 */
export function createRenderer(onEvent, options = {}) {
  return createNativeRenderer({ ...options, onEvent });
}

/** The tree a root context belongs to (portal targets must belong to the rendering root). */
const contextTrees = new WeakMap();

const hooksSeen = new WeakSet();

/** Registers with the React development-tools hook when one exists (Fast Refresh). */
function registerWithDevTools() {
  const hook = globalThis.__REACT_DEVTOOLS_GLOBAL_HOOK__;
  if (hook === undefined || hook === null || hooksSeen.has(hook)) return;
  hooksSeen.add(hook);
  try {
    reconciler.injectIntoDevTools();
  } catch {
    // A missing or incompatible hook never prevents startup.
  }
}

/** Runs `fn` as a discrete input and flushes the React work it caused synchronously. */
function dispatchDiscrete(fn) {
  try {
    reconciler.discreteUpdates(fn);
  } finally {
    reconciler.flushSyncWork();
  }
}

/** Runs `fn` and flushes React work synchronously (`ReactDOM.flushSync`). */
export function flushSync(fn) {
  return reconciler.flushSyncFromReconciler(fn);
}

/**
 * Mounts React trees into the native renderer.
 *
 * Options: `onEvent`, `onWindowKeyDown`, `onWindowKeyUp`, `onSelectionChange`, `tabNavigation`
 * (default true), `keyboardFocusDim` (false disables the keyboard focus dimming for this root),
 * `onUncaughtError(error, info)` with `info.componentStack`.
 *
 * @param {object} renderer
 * @param {object} [options]
 */
export function createRoot(renderer, options = {}) {
  registerWithDevTools();
  const tree = createHostTree(renderer);
  tree.attach({
    onEvent: options.onEvent,
    onWindowKeyDown: options.onWindowKeyDown,
    onWindowKeyUp: options.onWindowKeyUp,
    onSelectionChange: options.onSelectionChange,
    tabNavigation: options.tabNavigation,
    keyboardFocusDim: options.keyboardFocusDim,
    scheduler: dispatchDiscrete,
  });
  let unmounted = false;
  // Native selection notifications run only while a callback wants them.
  if (options.onSelectionChange) renderer.setSelectionNotifications?.(true);
  if (options.onUncaughtError) tree.reportError = (error) => options.onUncaughtError(error, undefined);
  const onUncaughtError = options.onUncaughtError
    ? (error, info) => options.onUncaughtError(error, info?.componentStack)
    : undefined;
  const container = reconciler.createContainer(
    tree.container,
    ConcurrentRoot,
    null,
    false,
    null,
    "",
    onUncaughtError ?? reconciler.defaultOnUncaughtError,
    reconciler.defaultOnCaughtError,
    reconciler.defaultOnRecoverableError,
    null,
  );
  const context = Object.freeze({ renderer });
  contextTrees.set(context, tree);
  const root = {
    /** The renderer context value this root provides (stable for the root's lifetime). */
    context,
    /** The native renderer. */
    renderer,
    /** The tree the root builds (for tests and tooling). */
    tree,
    /**
     * Renders `node` and flushes React and the native tree synchronously; renderer failures
     * propagate to the caller instead of being reported asynchronously.
     */
    renderSync(node) {
      tree.deferFlush = true;
      try {
        flushSync(() => root.render(node));
      } finally {
        tree.deferFlush = false;
      }
      tree.flush();
    },
    render(node) {
      if (unmounted) throw new Error("Cannot render an unmounted root");
      reconciler.updateContainer(
        React.createElement(RendererContext.Provider, { value: context }, node),
        container,
        null,
        null,
      );
    },
    unmount() {
      if (unmounted) return;
      unmounted = true;
      if (options.onSelectionChange) renderer.setSelectionNotifications?.(false);
      tree.deferFlush = true;
      try {
        reconciler.updateContainerSync(null, container, null, null);
        reconciler.flushSyncWork();
      } finally {
        tree.deferFlush = false;
      }
      tree.unmount();
    },
  };
  return root;
}

/**
 * Renders `children` into the mounted host element `target` of the same root. The optional key
 * is stringified.
 */
export function createPortal(children, target, key) {
  const portal = createPortalObject(children, target, key);
  let context = null;
  try {
    context = React.use(RendererContext);
  } catch {
    // Called outside a component render: there is no rendering root to compare with.
  }
  if (context !== null && contextTrees.get(context) !== target.tree) {
    throw new Error("A portal target must belong to the root that renders the portal");
  }
  return portal;
}
