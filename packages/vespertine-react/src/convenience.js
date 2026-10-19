// The convenience render entry point (APP-009 R9) with the runtime error display and remount
// recovery of DEV-010.

import React from "react";
import { startFrameLoop } from "@vespertine/host";
import { ErrorView, normalizeFailure } from "./error-view.js";
import { createRenderer, createRoot } from "./root.js";

/** The single retained renderer, window, frame loop and root of the convenience renderer. */
let session = null;
/** The process-level error handlers, installed once per process. */
let processHandlers = null;

const logError = (...args) => console.error(...args);

/** Routes a failure into the error display (DEV-010 R1, R2). */
function routeError(value, componentStack) {
  const { message, stack } = normalizeFailure(value, componentStack);
  logError(stack);
  const current = session;
  const captured = current?.root;
  if (!captured) return;
  queueMicrotask(() => {
    if (session !== current || current.root !== captured || current.errorDisplayed) return;
    showErrorView(current, message, stack);
  });
}

function showErrorView(current, message, stack) {
  current.errorDisplayed = true;
  try {
    current.root.unmount();
    const root = createRoot(current.renderer, current.rootOptions);
    current.root = root;
    root.renderSync(
      React.createElement(ErrorView, {
        message,
        stack,
        colors: current.errorColors,
        onReload: () => reload(current),
      }),
    );
  } catch (error) {
    current.errorDisplayed = false;
    logError("Failed to display the runtime error view:", error);
  }
}

/** Remounts the last submitted application tree in place of the error view (DEV-010 R5). */
function reload(current) {
  if (session !== current || !current.errorDisplayed) return;
  current.errorDisplayed = false;
  mount(current);
}

function mount(current) {
  current.root.unmount();
  current.root = createRoot(current.renderer, current.rootOptions);
  current.root.renderSync(current.node);
}

function installProcessHandlers() {
  const process = globalThis.process;
  if (processHandlers !== null || typeof process?.on !== "function") return;
  processHandlers = {
    uncaughtException: (error) => routeError(error),
    unhandledRejection: (reason) => routeError(reason),
  };
  process.on("uncaughtException", processHandlers.uncaughtException);
  process.on("unhandledRejection", processHandlers.unhandledRejection);
}

function removeProcessHandlers() {
  const process = globalThis.process;
  if (processHandlers === null) return;
  process.off("uncaughtException", processHandlers.uncaughtException);
  process.off("unhandledRejection", processHandlers.unhandledRejection);
  processHandlers = null;
}

function rootOptionsFor(options) {
  return {
    onEvent: options.onEvent,
    onWindowKeyDown: options.onWindowKeyDown,
    onWindowKeyUp: options.onWindowKeyUp,
    onSelectionChange: options.onSelectionChange,
    tabNavigation: options.tabNavigation,
    keyboardFocusDim: options.keyboardFocusDim,
    onUncaughtError(error, componentStack) {
      options.onUncaughtError?.(error, componentStack);
      routeError(error, componentStack);
    },
  };
}

/**
 * Renders `node` into the single window of this process, creating the native renderer, opening
 * the window and starting the frame loop on the first call. Later calls unmount the previous tree
 * first and ignore window options.
 *
 * Options: `renderer` (an externally owned renderer: never initialized nor pumped by this
 * function), `native` (the addon module for an owned renderer), `window` (renderer init options),
 * `debugOverlay` (`"hidden" | "minimal" | "full"`), `errorColors`, `frameMs`, and every root
 * option of `createRoot`.
 *
 * @returns the React root
 */
export function render(node, options = {}) {
  if (session === null) {
    const injected = options.renderer ?? null;
    const owned = injected === null;
    const renderer = owned
      ? createRenderer(undefined, { native: options.native, onError: (error) => routeError(error) })
      : injected;
    session = {
      renderer,
      owned,
      loop: null,
      root: null,
      node: null,
      rootOptions: null,
      errorColors: undefined,
      errorDisplayed: false,
    };
    if (owned) {
      installProcessHandlers();
      renderer.init(options.window ?? {});
      session.loop = startFrameLoop(renderer, {
        frameMs: options.frameMs,
        onError: (error) => routeError(error),
        onTerminated: () => globalThis.process?.exit?.(0),
      });
    }
  }
  const current = session;
  if (options.debugOverlay !== undefined) current.renderer.setDebugOverlay?.(options.debugOverlay);
  current.root?.unmount();
  current.errorDisplayed = false;
  current.node = node;
  current.errorColors = options.errorColors ?? current.errorColors;
  current.rootOptions = rootOptionsFor(options);
  current.root = createRoot(current.renderer, current.rootOptions);
  current.root.render(node);
  return current.root;
}

/** Stops the frame loop, unmounts the current root and removes the runtime error handlers. */
export function resetRender() {
  const current = session;
  session = null;
  removeProcessHandlers();
  if (current === null) return;
  current.loop?.stop();
  current.root?.unmount();
}
