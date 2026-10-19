// Retained host tree shared by the framework adapters (APP-009).
//
// A framework adapter builds host nodes as plain JavaScript descriptions. Nothing reaches the
// native renderer until a node is placed under a mounted parent: placing a subtree creates its
// native elements, applies its properties, registers its event handlers and attaches its
// children, all as operations of one mutation queue that the adapter flushes once per commit.
// Afterwards only changed properties are queued (structural comparison, APP-009 R3).

import { sameMutationValue, serializeCustomProp } from "./compare.js";
import { createHandlerMap, registerEventHandler, unregisterEventHandler, unregisterEventHandlers } from "./handlers.js";
import { createMutationQueue } from "./mutation-queue.js";
import { createRendererState } from "./renderer-state.js";

const NODE_KEY = Symbol.for("vespertine.host.node");
const FOCUS_DIM_KEY = Symbol.for("vespertine.host.focusDim");

/** Property names no adapter forwards as custom properties (APP-009 R4). */
export const RESERVED_PROPS = new Set(["style", "className", "children", "key", "ref"]);

/** The properties the built-in `div` and `text` host types forward (APP-009 R4). */
export const UNIVERSAL_PROPS = new Set([
  "autoFocus",
  "tabIndex",
  "motion",
  "testId",
  "highlight",
  "role",
  "aria-label",
  "aria-description",
  "aria-id",
  "aria-expanded",
  "aria-selected",
  "aria-valuetext",
  "aria-level",
]);

/** The built-in host types that only forward universal properties. */
export const BUILTIN_TYPES = new Set(["div", "text"]);

/** The native element types a framework adapter supports without custom hosts (APP-009 R6). */
export const NATIVE_ELEMENT_TYPES = [
  "div",
  "text",
  "img",
  "svg",
  "canvas",
  "input",
  "textarea",
  "anchored",
  "code",
  "diff",
  "markdown",
  "virtual-list",
];

const EVENT_PROP = /^on[A-Z]/;

/** True for a property name that names an event handler (`onClick`). */
export function isEventProp(name) {
  return EVENT_PROP.test(name);
}

/** The native event type an event-handler property subscribes to (`onKeyDown` is `keyDown`). */
export function eventTypeOf(name) {
  return name.charAt(2).toLowerCase() + name.slice(3);
}

/** True when `value` is a host node of any copy of this module (recognition is structural). */
export function isHostNode(value) {
  return typeof value === "object" && value !== null && value[NODE_KEY] === true;
}

const EMPTY_STYLE = Object.freeze({});

const normalizeStyle = (style) => (style === undefined || style === null ? EMPTY_STYLE : style);

function hiddenStyle(style) {
  const { hover: _hover, active: _active, ...rest } = normalizeStyle(style);
  return { ...rest, visibility: "hidden" };
}

function acceptsCustomProp(type, name) {
  return !RESERVED_PROPS.has(name) && !isEventProp(name) && (!BUILTIN_TYPES.has(type) || UNIVERSAL_PROPS.has(name));
}

/**
 * A host node description. `id` and the native state exist only while `mounted`.
 *
 * @typedef {object} HostNode
 * @property {string} type
 * @property {object} props
 * @property {string | null} text
 * @property {HostNode[]} children
 * @property {HostNode | null} parent
 * @property {number | null} id
 * @property {boolean} mounted
 * @property {boolean} hidden
 * @property {HostTree | null} tree
 */

class Node {
  /** @param {string} type @param {object} props @param {string | null} text */
  constructor(type, props, text = null) {
    this[NODE_KEY] = true;
    this.type = type;
    this.props = props;
    this.text = text;
    this.children = [];
    this.parent = null;
    this.id = null;
    this.mounted = false;
    this.hidden = false;
    this.tree = null;
    /** What the native side currently holds for this node. */
    this.sent = null;
  }

  #renderer(action) {
    if (!this.mounted) throw new Error(`Cannot ${action}: the element is not mounted`);
    return this.tree.renderer;
  }

  #call(action, method, ...args) {
    const renderer = this.#renderer(action);
    if (typeof renderer[method] !== "function") throw new Error(`The renderer does not support ${action}`);
    return renderer[method](...args);
  }

  /** Moves keyboard focus to the element. */
  focus() {
    return this.#call("focus the element", "focusElement", this.id);
  }

  /** Removes keyboard focus from the element when it holds it. */
  blur() {
    return this.#call("blur the element", "blurElement", this.id);
  }

  /** Moves focus to the next focusable element after the focused one. */
  focusNext() {
    return this.#call("move focus", "focusNext");
  }

  /** Moves focus to the previous focusable element. */
  focusPrevious() {
    return this.#call("move focus", "focusPrevious");
  }

  /** Moves focus to the next focusable element inside this element, wrapping inside it. */
  focusNextWithin() {
    return this.#call("move focus within the element", "focusNextWithin", this.id);
  }

  /** Moves focus to the previous focusable element inside this element, wrapping inside it. */
  focusPreviousWithin() {
    return this.#call("move focus within the element", "focusPreviousWithin", this.id);
  }

  /** Scrolls the element into view within its scrolling ancestors. */
  scrollIntoView(options) {
    return this.#call("scroll the element into view", "scrollIntoView", this.id, options ?? null);
  }

  /** Replaces the image of an `img` element with encoded bytes (PNG, JPEG, ...). */
  setImageBytes(bytes) {
    return this.#call("set the image", "setImageBytes", this.id, bytes);
  }

  /** Replaces the image of an `img` element with raw pixels; `format` is `bgra` or `rgba`. */
  setImagePixels(width, height, bytes, format = "bgra") {
    return this.#call("set the image pixels", "setImagePixels", this.id, width, height, bytes, format);
  }
}

/** Creates an unplaced element description. */
export function createHostNode(type, props = {}) {
  return new Node(type, props);
}

/** Creates an unplaced text description. */
export function createTextNode(text) {
  return new Node("text", {}, String(text));
}

/**
 * Replaces a node's properties (the caller passes the complete next set) and, when `text` is
 * given, its own text content (`null` clears it).
 */
export function setNodeProps(node, props, text) {
  node.props = props;
  if (text !== undefined) node.text = text === null ? null : String(text);
  if (node.mounted) node.tree.syncNode(node);
}

/** Replaces a node's own text content (`null` clears it). */
export function setNodeText(node, text) {
  node.text = text === null ? null : String(text);
  if (node.mounted) node.tree.syncNode(node);
}

/** Requests (or lifts) hidden visibility for the node (framework suspense). */
export function setNodeHidden(node, hidden) {
  if (node.hidden === hidden) return;
  node.hidden = hidden;
  if (node.mounted) node.tree.syncNode(node);
}

/**
 * Places `child` as the last child of `parent`, or before `anchor`. A child that already has a
 * parent is moved (its native element and identity are kept). Under an unplaced parent only the
 * descriptions change.
 */
export function insertNode(parent, child, anchor = null) {
  if (anchor !== null && anchor.parent !== parent) {
    throw new Error("The insertion anchor is not a child of the target parent");
  }
  if (parent.tree !== null) {
    parent.tree.insert(parent, child, anchor);
    return;
  }
  if (anchor === child) return;
  if (child.parent !== null) removeNode(child.parent, child);
  if (anchor === null) parent.children.push(child);
  else parent.children.splice(parent.children.indexOf(anchor), 0, child);
  child.parent = parent;
}

/** Removes `child` from `parent`, destroying its native subtree when it was placed. */
export function removeNode(parent, child) {
  if (child.parent !== parent) return;
  if (child.tree !== null) {
    child.tree.remove(parent, child);
    return;
  }
  parent.children.splice(parent.children.indexOf(child), 1);
  child.parent = null;
}

function freshSent() {
  return { style: EMPTY_STYLE, text: null, custom: new Map(), handlers: new Map() };
}

function* subtree(node) {
  yield node;
  for (const child of node.children) yield* subtree(child);
}

/**
 * The tree an adapter root builds into.
 *
 * @param {object} renderer the native renderer (or a compatible object)
 * @param {{ scheduleFlush?: (flush: () => void) => void, rootStyle?: object }} [options]
 */
export function createHostTree(renderer, options = {}) {
  return new HostTree(renderer, options);
}

export class HostTree {
  constructor(renderer, { scheduleFlush, rootStyle } = {}) {
    this.renderer = renderer;
    this.state = createRendererState(renderer);
    this.handlers = createHandlerMap();
    this.queue = createMutationQueue(renderer, (ids) => this.#destroyed(ids), scheduleFlush);
    this.binding = null;
    /** While set, `flushAfterCommit` does nothing (an unmount flushes once at its end). */
    this.deferFlush = false;
    /** Receives renderer failures raised by an end-of-commit flush. */
    this.reportError = (error) => {
      queueMicrotask(() => {
        throw error;
      });
    };
    this.container = new Node("div", { style: rootStyle ?? { width: "100%", height: "100%" } });
    this.container.tree = this;
    this.container.isContainer = true;
  }

  /** Attaches the root binding for window-level events (APP-007 R1) and applies the focus-dim override. */
  attach(handlers = {}) {
    this.binding = this.state.attach({ ...handlers, handlers: this.handlers });
    const wanted = handlers.keyboardFocusDim !== false;
    const registry = (globalThis[FOCUS_DIM_KEY] ??= new WeakMap());
    if ((registry.get(this.renderer) ?? true) !== wanted) {
      this.queue.setKeyboardFocusDim(wanted);
      registry.set(this.renderer, wanted);
    }
    return this.binding;
  }

  get generation() {
    return this.binding?.generation ?? 0;
  }

  /**
   * Places `child` as the last child of `parent`, or before `anchor`. A child that already has a
   * parent is moved (its native element and identity are kept).
   */
  insert(parent, child, anchor = null) {
    if (anchor === child) return;
    if (parent === this.container && !parent.mounted) this.#mountContainer();
    if (child.tree !== null && child.tree !== this) throw new Error("A node cannot be moved between roots");
    this.#adopt(child);
    if (child.parent !== null) {
      const siblings = child.parent.children;
      siblings.splice(siblings.indexOf(child), 1);
    }
    const siblings = parent.children;
    if (anchor === null) siblings.push(child);
    else siblings.splice(siblings.indexOf(anchor), 0, child);
    child.parent = parent;
    if (!child.mounted) this.#materialize(child);
    if (anchor !== null && anchor.mounted) this.queue.insertBefore(parent.id, child.id, anchor.id);
    else this.queue.appendChild(parent.id, child.id);
  }

  /** Removes `child` from `parent`, destroying its native subtree. */
  remove(parent, child) {
    if (child.parent !== parent) return;
    parent.children.splice(parent.children.indexOf(child), 1);
    child.parent = null;
    if (child.mounted) this.#detach(child);
    for (const node of subtree(child)) node.tree = null;
  }

  /** Flushes the queued operations as one batch (nothing when there are none). */
  flush() {
    this.queue.flushMutations();
  }

  /**
   * The end-of-commit flush: exactly one batch per nonempty commit. A renderer failure must not
   * unwind the framework's commit, so it is handed to `reportError` (the operations stay queued).
   */
  flushAfterCommit() {
    if (this.deferFlush) return;
    try {
      this.flush();
    } catch (error) {
      this.reportError(error);
    }
  }

  /** Destroys the root container and detaches the binding; flushes one batch. */
  unmount() {
    const container = this.container;
    for (const child of [...container.children]) this.remove(container, child);
    if (container.mounted) this.#detach(container);
    container.id = null;
    container.mounted = false;
    this.flush();
    this.binding?.detach();
    this.binding = null;
  }

  /** The number of event handlers currently registered for native elements. */
  handlerCount() {
    let count = 0;
    for (const byType of this.handlers.values()) count += byType.size;
    return count;
  }

  #adopt(node) {
    for (const member of subtree(node)) {
      if (member.tree !== null && member.tree !== this) throw new Error("A node cannot be moved between roots");
    }
    for (const member of subtree(node)) member.tree = this;
  }

  #mountContainer() {
    const container = this.container;
    this.#materialize(container);
    this.queue.setRoot(container.id);
  }

  #materialize(node) {
    node.id = this.state.ids.next();
    node.mounted = true;
    node.tree = this;
    node.sent = freshSent();
    this.queue.createElement(node.id, node.type);
    this.syncNode(node);
    for (const child of node.children) {
      this.#materialize(child);
      this.queue.appendChild(node.id, child.id);
    }
  }

  /** Destroys the native subtree of a mounted node and returns its descriptions to unplaced state. */
  #detach(node) {
    this.queue.destroyElement(node.id);
    for (const member of subtree(node)) {
      if (member.id !== null) unregisterEventHandlers(this.handlers, member.id);
      member.mounted = false;
      member.sent = null;
    }
  }

  #destroyed(ids) {
    for (const id of ids) unregisterEventHandlers(this.handlers, id);
  }

  /** Queues the operations that bring the native node in line with the description. */
  syncNode(node) {
    const { id, sent, props } = node;
    const style = node.hidden ? hiddenStyle(props.style) : normalizeStyle(props.style);
    if (!sameMutationValue(sent.style, style)) {
      this.queue.setStyle(id, style);
      sent.style = style;
    }
    const text = node.text;
    if (text !== sent.text) {
      this.queue.setText(id, text ?? "");
      sent.text = text;
    }

    const customs = new Set();
    const handlers = new Map();
    for (const name of Object.keys(props)) {
      if (RESERVED_PROPS.has(name)) continue;
      const value = props[name];
      if (isEventProp(name)) {
        if (typeof value === "function") handlers.set(eventTypeOf(name), value);
      } else if (acceptsCustomProp(node.type, name)) {
        customs.add(name);
        this.#syncCustom(node, name, serializeCustomProp(value));
      }
    }
    for (const name of [...sent.custom.keys()]) {
      if (!customs.has(name)) this.#syncCustom(node, name, null);
    }
    for (const type of [...sent.handlers.keys()]) {
      if (!handlers.has(type)) this.#syncHandler(node, type, null);
    }
    for (const [type, handler] of handlers) this.#syncHandler(node, type, handler);
  }

  #syncCustom(node, name, value) {
    const { sent, id } = node;
    const had = sent.custom.has(name);
    if (value === null) {
      if (!had) return;
      sent.custom.delete(name);
    } else {
      if (had && sameMutationValue(sent.custom.get(name), value)) return;
      sent.custom.set(name, value);
    }
    this.queue.setCustomProp(id, name, value);
  }

  #syncHandler(node, type, handler) {
    const { sent, id } = node;
    const had = sent.handlers.has(type);
    if (handler === null) {
      if (!had) return;
      sent.handlers.delete(type);
      unregisterEventHandler(this.handlers, id, type);
      this.queue.setEventListener(id, type, false);
      return;
    }
    sent.handlers.set(type, handler);
    // Replacing the callback only swaps the JavaScript function; the native listener stays on.
    registerEventHandler(this.handlers, id, type, handler);
    if (!had) this.queue.setEventListener(id, type, true);
  }
}
