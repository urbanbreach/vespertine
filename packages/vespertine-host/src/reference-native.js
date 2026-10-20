// A JavaScript model of the native retained host tree (APP-006) for adapter tests that only need
// to observe what an adapter sends: structure, styles, text, listeners and properties, with the
// native destroy semantics (a destroy reports every destroyed identity, descendants included) and
// per-operation counters. It implements the same `Renderer` constructor shape as the native
// addon, so `createNativeRenderer({ native })` accepts it. Pixel and interaction tests use the
// real native test host instead.

const OPERATIONS = new Set([
  "createElement",
  "destroyElement",
  "appendChild",
  "insertBefore",
  "setStyle",
  "setText",
  "setEventListener",
  "setRoot",
  "setKeyboardFocusDim",
  "setCustomProp",
]);

/**
 * @returns {{ Renderer: new (callback: Function) => object }} an addon-shaped module; the
 * constructed renderers expose the recorded state under `model`.
 */
export function createReferenceNative() {
  class Renderer {
    constructor(callback) {
      this.callback = callback;
      this.elements = new Map();
      this.root = null;
      this.focusDim = true;
      /** Every batch received (parsed). */
      this.batches = [];
      /** Operation counters by name. */
      this.counts = {};
      this.focused = null;
      this.generation = 0;
      this.calls = [];
      this.initialized = 0;
      this.ticks = 0;
      this.debugOverlay = null;
      this.windowSize = { width: 800, height: 600 };
      this.windowInsets = null;
      this.selectedText = null;
      this.selectionNotifications = [];
      this.destroyedLog = [];
    }

    init(options) {
      this.initialized++;
      this.initOptions = options;
    }

    tick() {
      this.ticks++;
      return true;
    }

    requiresTick() {
      return true;
    }

    setWindowGeneration(generation) {
      this.generation = generation;
    }

    setDebugOverlay(mode) {
      this.debugOverlay = mode;
    }

    getWindowSize() {
      return this.windowSize;
    }

    getWindowInsets() {
      return this.windowInsets;
    }

    getSelectedText() {
      return this.selectedText;
    }

    setSelectionNotifications(enabled) {
      this.selectionNotifications.push(enabled);
    }

    getFocusedElement() {
      return this.focused;
    }

    focusElement(id) {
      this.calls.push(["focusElement", id]);
      this.focused = id;
    }

    focusNext() {
      this.calls.push(["focusNext"]);
    }

    focusPrevious() {
      this.calls.push(["focusPrevious"]);
    }

    focusNextWithin(id) {
      this.calls.push(["focusNextWithin", id]);
    }

    focusPreviousWithin(id) {
      this.calls.push(["focusPreviousWithin", id]);
    }

    scrollIntoView(id, options) {
      this.calls.push(["scrollIntoView", id, options]);
    }

    setImage(id, bytes) {
      this.calls.push(["setImage", id, bytes]);
    }

    setImagePixels(id, width, height, bytes, format) {
      this.calls.push(["setImagePixels", id, width, height, bytes, format]);
    }

    /** Delivers a native event to the callback the way the UI thread does. */
    emit(event) {
      this.callback(null, event);
    }

    /** The number of operations of `name` received so far. */
    count(name) {
      return this.counts[name] ?? 0;
    }

    /** Clears the counters and the batch log (the tree is untouched). */
    resetCounts() {
      this.counts = {};
      this.batches = [];
    }

    /** The element ids in the subtree of `id`, pre-order. */
    subtreeIds(id) {
      const out = [];
      const visit = (current) => {
        const element = this.elements.get(current);
        if (!element) return;
        out.push(current);
        for (const child of element.children) visit(child);
      };
      visit(id);
      return out;
    }

    /** A plain-object snapshot of the tree from the root. */
    snapshot(id = this.root) {
      const element = this.elements.get(id);
      if (!element) return null;
      return {
        id,
        type: element.type,
        text: element.text,
        style: element.style,
        listeners: [...element.listeners].sort(),
        props: Object.fromEntries(element.props),
        children: element.children.map((child) => this.snapshot(child)),
      };
    }

    applyBatch(json) {
      const ops = JSON.parse(json);
      if (!Array.isArray(ops)) throw new Error("failed to parse batch: not an array");
      ops.forEach((op, index) => {
        if (!Array.isArray(op) || !OPERATIONS.has(op[0])) {
          throw new Error(`failed to parse batch: Batch op ${index} is not a known operation`);
        }
      });
      this.batches.push(ops);
      const destroyed = [];
      for (const [name, ...args] of ops) {
        this.counts[name] = (this.counts[name] ?? 0) + 1;
        this.#apply(name, args, destroyed);
      }
      this.destroyedLog.push(...destroyed);
      return destroyed;
    }

    #unlink(id) {
      const element = this.elements.get(id);
      if (!element || element.parent === null) return;
      const parent = this.elements.get(element.parent);
      if (parent) parent.children.splice(parent.children.indexOf(id), 1);
      element.parent = null;
    }

    #apply(name, args, destroyed) {
      switch (name) {
        case "createElement": {
          const [id, type] = args;
          this.elements.set(id, {
            type,
            parent: null,
            children: [],
            style: null,
            text: null,
            listeners: new Set(),
            props: new Map(),
          });
          break;
        }
        case "destroyElement": {
          const [id] = args;
          if (!this.elements.has(id)) break;
          this.#unlink(id);
          for (const gone of this.subtreeIds(id)) {
            this.elements.delete(gone);
            destroyed.push(gone);
            if (this.root === gone) this.root = null;
            if (this.focused === gone) this.focused = null;
          }
          break;
        }
        case "appendChild": {
          const [parentId, childId] = args;
          const child = this.elements.get(childId);
          if (!child) break;
          this.#unlink(childId);
          const parent = this.elements.get(parentId);
          if (!parent) break;
          parent.children.push(childId);
          child.parent = parentId;
          break;
        }
        case "insertBefore": {
          const [parentId, childId, beforeId] = args;
          const child = this.elements.get(childId);
          if (!child) break;
          this.#unlink(childId);
          const parent = this.elements.get(parentId);
          if (!parent) break;
          const at = parent.children.indexOf(beforeId);
          if (at === -1) parent.children.push(childId);
          else parent.children.splice(at, 0, childId);
          child.parent = parentId;
          break;
        }
        case "setStyle": {
          const element = this.elements.get(args[0]);
          if (element) element.style = args[1];
          break;
        }
        case "setText": {
          const element = this.elements.get(args[0]);
          if (element) element.text = args[1];
          break;
        }
        case "setEventListener": {
          const element = this.elements.get(args[0]);
          if (!element) break;
          if (args[2]) element.listeners.add(args[1]);
          else element.listeners.delete(args[1]);
          break;
        }
        case "setRoot":
          this.root = args[0];
          break;
        case "setKeyboardFocusDim":
          this.focusDim = args[0];
          break;
        case "setCustomProp": {
          const element = this.elements.get(args[0]);
          if (!element) break;
          if (args[2] === null) element.props.delete(args[1]);
          else element.props.set(args[1], args[2]);
          break;
        }
        default:
          break;
      }
    }
  }
  return { Renderer };
}
