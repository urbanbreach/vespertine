// The React reconciler configured for the retained host tree (APP-009 R1-R5).
//
// Mutation-mode concurrent reconciliation only. During the render phase host nodes are plain
// descriptions (`createInstance`, `createTextInstance`, `appendInitialChild` touch no native
// state, no identity and no handler registry); placing a node under a mounted parent, which only
// a commit does, creates its native subtree. The end of every commit flushes the mutation queue
// once.

import React from "react";
import Reconciler from "react-reconciler";
import { DefaultEventPriority, NoEventPriority } from "react-reconciler/constants.js";
import {
  createHostNode,
  createTextNode,
  insertNode,
  isHostNode,
  removeNode,
  setNodeHidden,
  setNodeProps,
} from "@vespertine/host";

let currentPriority = NoEventPriority;

/** The text a `text` host owns directly when its only child is a string or number. */
function textContentOf(type, props) {
  if (type !== "text") return null;
  const { children } = props;
  return typeof children === "string" || typeof children === "number" ? String(children) : null;
}

const hostConfig = {
  supportsMutation: true,
  supportsPersistence: false,
  supportsHydration: false,
  isPrimaryRenderer: true,
  supportsMicrotasks: true,
  noTimeout: -1,
  rendererPackageName: "@vespertine/react",
  rendererVersion: "0.1.0",
  extraDevToolsConfig: null,

  scheduleTimeout: (fn, delay) => setTimeout(fn, delay),
  cancelTimeout: (handle) => clearTimeout(handle),
  scheduleMicrotask: (fn) => queueMicrotask(fn),

  getRootHostContext: () => ({}),
  getChildHostContext: (parent) => parent,
  getPublicInstance: (node) => node,
  prepareForCommit: () => null,
  resetAfterCommit(container) {
    container.tree?.flushAfterCommit();
  },
  preparePortalMount() {},

  shouldSetTextContent: (type, props) => textContentOf(type, props) !== null,

  createInstance(type, props) {
    const node = createHostNode(type, props);
    const text = textContentOf(type, props);
    if (text !== null) node.text = text;
    return node;
  },
  createTextInstance: (text) => createTextNode(text),
  appendInitialChild: (parent, child) => insertNode(parent, child),
  finalizeInitialChildren: () => false,

  appendChild: (parent, child) => insertNode(parent, child),
  appendChildToContainer: (container, child) => insertNode(container, child),
  insertBefore: (parent, child, before) => insertNode(parent, child, before),
  insertInContainerBefore: (container, child, before) => insertNode(container, child, before),
  removeChild: (parent, child) => removeNode(parent, child),
  removeChildFromContainer: (container, child) => removeNode(container, child),
  clearContainer(container) {
    for (const child of [...container.children]) removeNode(container, child);
  },

  commitUpdate(node, type, _previous, next) {
    setNodeProps(node, next, textContentOf(type, next));
  },
  commitTextUpdate: (node, _old, text) => setNodeProps(node, node.props, text),
  resetTextContent: (node) => setNodeProps(node, node.props, null),
  hideInstance: (node) => setNodeHidden(node, true),
  unhideInstance: (node) => setNodeHidden(node, false),
  // A raw text node has no style of its own to hide.
  hideTextInstance() {},
  unhideTextInstance() {},
  detachDeletedInstance() {},

  getCurrentUpdatePriority: () => currentPriority,
  setCurrentUpdatePriority(priority) {
    currentPriority = priority;
  },
  resolveUpdatePriority: () => (currentPriority === NoEventPriority ? DefaultEventPriority : currentPriority),
  shouldAttemptEagerTransition: () => false,
  trackSchedulerEvent() {},
  resolveEventType: () => null,
  resolveEventTimeStamp: () => -1.1,
  maySuspendCommit: () => false,
  preloadInstance: () => true,
  startSuspendingCommit() {},
  suspendInstance() {},
  waitForCommitToBeReady: () => null,
  NotPendingTransition: null,
  HostTransitionContext: React.createContext(null),
  resetFormInstance() {},
  requestPostPaintCallback() {},
  prepareScopeUpdate() {},
  getInstanceFromNode: () => null,
  getInstanceFromScope: () => null,
  beforeActiveInstanceBlur() {},
  afterActiveInstanceBlur() {},
};

export const reconciler = Reconciler(hostConfig);

/** Creates the portal object React expects; `containerInfo` is the target host node. */
export function createPortalObject(children, target, key) {
  if (!isHostNode(target) || !target.mounted) {
    throw new Error("createPortal needs a mounted host element of the same root as its target");
  }
  return reconciler.createPortal(children, target, null, key);
}
