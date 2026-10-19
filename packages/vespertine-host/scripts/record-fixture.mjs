// Records the mutation batches a real React reconciler (react-reconciler) produces when driven
// through the shared mutation queue (createMutationQueue), one serialized batch per line.
//
//   node scripts/record-fixture.mjs <output.ndjson>
//
// The package's own tests do not need React; this script is the only user of the `react` and
// `react-reconciler` development dependencies.
import { writeFileSync } from "node:fs";
import React from "react";
import Reconciler from "react-reconciler";
import { DefaultEventPriority, NoEventPriority } from "react-reconciler/constants.js";
import { createMutationQueue, createRendererState, registerEventHandler, unregisterEventHandlers, createHandlerMap } from "../src/index.js";

const batches = [];
const host = {
  applyBatch(json) {
    batches.push(json);
    return [];
  },
};
const renderer = {};
const state = createRendererState(renderer);
const handlers = createHandlerMap();
const queue = createMutationQueue(host, (destroyed) => destroyed.forEach((id) => unregisterEventHandlers(handlers, id)));

const EVENT_PROPS = { onClick: "click", onKeyDown: "keyDown", onFocus: "focus", onBlur: "blur", onPointerEnter: "enter" };
let currentPriority = NoEventPriority;

function applyProps(id, type, props, previous = {}) {
  for (const key of Object.keys({ ...previous, ...props })) {
    if (key === "children" && typeof props.children !== "string" && typeof props.children !== "number") continue;
    if (key === "children") {
      if (previous.children !== props.children) queue.setText(id, String(props.children));
    } else if (key === "style") {
      if (JSON.stringify(previous.style) !== JSON.stringify(props.style)) queue.setStyle(id, props.style ?? {});
    } else if (key in EVENT_PROPS) {
      const had = typeof previous[key] === "function";
      const has = typeof props[key] === "function";
      if (has) registerEventHandler(handlers, id, EVENT_PROPS[key], props[key]);
      if (had !== has) queue.setEventListener(id, EVENT_PROPS[key], has);
    } else if (key !== "key" && previous[key] !== props[key]) {
      queue.setCustomProp(id, key, props[key]);
    }
  }
}

const hostConfig = {
  supportsMutation: true,
  supportsPersistence: false,
  supportsHydration: false,
  isPrimaryRenderer: true,
  noTimeout: -1,
  scheduleTimeout: setTimeout,
  cancelTimeout: clearTimeout,
  scheduleMicrotask: queueMicrotask,
  getRootHostContext: () => ({}),
  getChildHostContext: (parent) => parent,
  getPublicInstance: (instance) => instance,
  prepareForCommit: () => null,
  resetAfterCommit: () => queue.flushMutations(),
  shouldSetTextContent: (_type, props) => typeof props.children === "string" || typeof props.children === "number",
  createInstance(type, props) {
    const id = state.ids.next();
    queue.createElement(id, type);
    applyProps(id, type, props);
    return { id, type, props };
  },
  createTextInstance(text) {
    const id = state.ids.next();
    queue.createElement(id, "text");
    queue.setText(id, text);
    return { id, type: "text", props: {} };
  },
  appendInitialChild: (parent, child) => queue.appendChild(parent.id, child.id),
  finalizeInitialChildren: () => false,
  appendChild: (parent, child) => queue.appendChild(parent.id, child.id),
  appendChildToContainer: (container, child) => {
    queue.appendChild(container.id, child.id);
  },
  insertBefore: (parent, child, before) => queue.insertBefore(parent.id, child.id, before.id),
  insertInContainerBefore: (container, child, before) => queue.insertBefore(container.id, child.id, before.id),
  removeChild: (_parent, child) => queue.destroyElement(child.id),
  removeChildFromContainer: (_container, child) => queue.destroyElement(child.id),
  commitUpdate(instance, type, previous, next) {
    applyProps(instance.id, type, next, previous);
    instance.props = next;
  },
  commitTextUpdate: (instance, _old, text) => queue.setText(instance.id, text),
  clearContainer: () => {},
  detachDeletedInstance: () => {},
  getCurrentUpdatePriority: () => currentPriority,
  setCurrentUpdatePriority: (priority) => (currentPriority = priority),
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
  preparePortalMount() {},
};

const reconciler = Reconciler(hostConfig);
const rootId = state.ids.next();
queue.createElement(rootId, "div");
queue.setStyle(rootId, { flexDirection: "column", width: 900, height: 700, backgroundColor: "#1e1e2e", color: "#cdd6f4" });
queue.setRoot(rootId);
const container = reconciler.createContainer({ id: rootId }, 1, null, false, null, "", console.error, console.error, console.error, () => {}, null);

const PALETTE = ["#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#cba6f7", "#94e2d5"];

function Row({ index, selected, label }) {
  return React.createElement(
    "div",
    {
      tabIndex: 0,
      testId: `row-${index}`,
      onClick: () => {},
      onKeyDown: () => {},
      style: {
        flexDirection: "row",
        height: 28,
        padding: 6,
        gap: 8,
        borderRadius: 4,
        backgroundColor: selected ? "#45475a" : index % 2 ? "#181825" : "#1e1e2e",
        borderColor: PALETTE[index % PALETTE.length],
      },
    },
    React.createElement("div", { style: { width: 10, height: 10, borderRadius: 5, backgroundColor: PALETTE[index % PALETTE.length] } }),
    React.createElement("div", { style: { flexGrow: 1, fontSize: 13, color: "#cdd6f4" } }, label),
    React.createElement("div", { style: { width: 64, fontSize: 12, color: "#a6adc8" }, motion: index % 5 === 0 ? { opacity: [0.6, 1] } : undefined }, `#${index}`),
  );
}

function App({ rows, selected, title }) {
  return React.createElement(
    React.Fragment,
    null,
    React.createElement("div", { style: { height: 40, padding: 10, fontSize: 18, fontWeight: 700 } }, title),
    React.createElement(
      "div",
      { style: { flexDirection: "column", flexGrow: 1, overflow: "scroll", padding: 8 }, highlight: { query: "row" } },
      rows.map((label, index) => React.createElement(Row, { key: label, index, selected: index === selected, label })),
    ),
  );
}

const make = (n, prefix = "item") => Array.from({ length: n }, (_, i) => `${prefix} ${i}: lorem ipsum dolor sit amet`);
const commit = (props) => {
  reconciler.updateContainerSync(React.createElement(App, props), container, null, null);
  reconciler.flushSyncWork();
};

const rows = make(400);
commit({ rows, selected: -1, title: "Mutation fixture" }); // first mount
commit({ rows, selected: -1, title: "Mutation fixture" }); // unchanged re-render
for (const selected of [3, 4, 5]) commit({ rows, selected, title: "Mutation fixture" }); // selection moves
commit({ rows: [...make(40, "new"), ...rows], selected: 5, title: "Mutation fixture (40 prepended)" }); // prepend
commit({ rows: rows.slice(0, 250), selected: 5, title: "Mutation fixture (trimmed)" }); // remove
commit({ rows: rows.slice(0, 250).reverse(), selected: 0, title: "Mutation fixture (reversed)" }); // reorder
commit({ rows: make(400, "remount"), selected: 7, title: "Mutation fixture (replaced)" }); // replace all
queue.flushMutations();

const out = process.argv[2];
if (!out) throw new Error("usage: record-fixture.mjs <output.ndjson>");
writeFileSync(out, batches.map((batch) => `${batch}\n`).join(""));
const ops = batches.reduce((sum, batch) => sum + JSON.parse(batch).length, 0);
console.error(`recorded ${batches.length} batches, ${ops} operations, ${batches.reduce((s, b) => s + b.length, 0)} bytes -> ${out}`);
