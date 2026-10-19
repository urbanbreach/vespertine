// Solid roots on a native renderer (APP-009 R7, R8; APP-010 R3).

import { batch, createContext, createComponent, createRoot as createReactiveRoot, useContext } from "solid-js";
import { createHostTree, createNativeRenderer, createRendererState, observeSelectedText } from "@vespertine/host";
import { universal } from "./renderer.js";

/** The context every root provides. */
export const RendererContext = createContext();

/** The renderer context of the nearest root, or undefined outside any root. */
export function useOptionalRendererContext() {
  return useContext(RendererContext);
}

/** The renderer context of the nearest root; throws outside any root. */
export function useRendererContext() {
  const context = useContext(RendererContext);
  if (context === undefined) throw new Error("This primitive must be used inside a Vespertine Solid root");
  return context;
}

/** The native renderer of the nearest root; throws outside any root. */
export function useRenderer() {
  return useRendererContext().renderer;
}

/**
 * Creates the native renderer for Solid roots: native events are routed to the active root and
 * every handled event also reaches `onEvent`.
 */
export function createRenderer(onEvent, options = {}) {
  return createNativeRenderer({ ...options, onEvent });
}

/**
 * Mounts Solid reactive trees into the native renderer.
 *
 * Options: `onEvent`, `onWindowKeyDown`, `onWindowKeyUp`, `onSelectionChange`, `tabNavigation`
 * (default true), `keyboardFocusDim` (false disables the keyboard focus dimming for this root).
 *
 * @param {object} renderer
 * @param {object} [options]
 */
export function createRoot(renderer, options = {}) {
  const tree = createHostTree(renderer, { scheduleFlush: (flush) => queueMicrotask(flush) });
  const selectionListeners = new Set();
  let disposeReactive = null;
  let unmounted = false;

  // Runs `fn` as one reactive batch and flushes the native queue before returning, even when
  // user code throws: the updates made before the throw are kept (Solid would drop them with the
  // aborted batch) and the failure is rethrown afterwards.
  const runFlushed = (fn) => {
    let failure = null;
    try {
      const value = batch(() => {
        try {
          return fn();
        } catch (error) {
          failure = { error };
          return undefined;
        }
      });
      return failure === null ? value : undefined;
    } finally {
      tree.flush();
      if (failure !== null) throw failure.error;
    }
  };
  const scheduler = runFlushed;
  const bindingOptions = () => ({
    onEvent: options.onEvent,
    onWindowKeyDown: options.onWindowKeyDown,
    onWindowKeyUp: options.onWindowKeyUp,
    tabNavigation: options.tabNavigation,
    keyboardFocusDim: options.keyboardFocusDim,
    scheduler,
    onSelectionChange(event) {
      options.onSelectionChange?.(event);
      for (const listener of [...selectionListeners]) listener(event);
    },
  });
  tree.attach(bindingOptions());

  /** Native selection notifications run only while a subscriber exists. */
  function subscribeSelection(callback) {
    const unsubscribeNative = () => {
      if (selectionListeners.size === 0) renderer.setSelectionNotifications?.(false);
    };
    const subscribe = (listener) => {
      selectionListeners.add(listener);
      if (selectionListeners.size === 1) renderer.setSelectionNotifications?.(true);
      return () => {
        if (!selectionListeners.delete(listener)) return;
        unsubscribeNative();
      };
    };
    return observeSelectedText(renderer, subscribe, callback);
  }

  const context = Object.freeze({ renderer, subscribeSelection });

  function disposeContent() {
    if (disposeReactive !== null) {
      const dispose = disposeReactive;
      disposeReactive = null;
      dispose();
    }
    tree.clear();
  }

  const root = {
    /** The renderer context value this root provides. */
    context,
    renderer,
    /** The tree the root builds (for tests and tooling). */
    tree,
    /**
     * Renders `code` (a function returning the UI) as the root's content. A previous render is
     * disposed first: its reactive owner, native content and event callbacks are discarded and
     * the new content gets a fresh event generation. Flushes immediately.
     */
    render(code) {
      if (unmounted) throw new Error("Cannot render an unmounted root");
      const rendered = disposeReactive !== null || tree.container.children.length > 0;
      disposeContent();
      if (rendered) {
        tree.flush();
        tree.rebind(bindingOptions());
      }
      createReactiveRoot((dispose) => {
        disposeReactive = dispose;
        universal.insert(
          tree.container,
          createComponent(RendererContext.Provider, {
            value: context,
            get children() {
              return code();
            },
          }),
        );
      });
      tree.flush();
    },
    /** Flushes the queued native mutations as one batch. */
    flush() {
      tree.flush();
    },
    /** Runs `fn` as one reactive batch and flushes the native queue afterwards, even on a throw. */
    flushSync(fn) {
      return runFlushed(fn);
    },
    /** Delivers a native event to the root's handlers, exactly as the native side does. */
    dispatch(event) {
      return createRendererState(renderer).dispatch(event);
    },
    /** Disposes the content and detaches the root. */
    unmount() {
      if (unmounted) return;
      unmounted = true;
      selectionListeners.clear();
      renderer.setSelectionNotifications?.(false);
      disposeContent();
      tree.unmount();
    },
  };
  return root;
}
