// Per-renderer shared state: root binding, element identity allocator, dismissal layers
// and keystroke flags (APP-007 R1, R4, R5).
//
// The state lives in a WeakMap stored under a process-wide registered symbol, so every copy
// of this module loaded into the process sees the same state for the same renderer and a
// discarded renderer is not kept alive.

const REGISTRY_KEY = Symbol.for("vespertine.host.rendererState");

const COMMAND_MODIFIERS = ["ctrl", "alt", "cmd"];

function registry() {
  return (globalThis[REGISTRY_KEY] ??= new WeakMap());
}

function isDescendant(layer, ancestor) {
  for (let node = layer.parent; node; node = node.parent) {
    if (node === ancestor) return true;
  }
  return false;
}

function keyEvent(event, flags, type) {
  return {
    ...event,
    type,
    preventDefault() {
      flags.defaultPrevented = true;
    },
    stopPropagation() {
      flags.stopped = true;
    },
    get defaultPrevented() {
      return flags.defaultPrevented;
    },
  };
}

function createState(renderer) {
  let nextId = 0;
  let generation = 0;
  let active = null;
  /** @type {{ layer: object, previous: number | null }[]} */
  const layers = [];
  let latest = null;

  const focus = (id) => renderer.focusElement?.(id);

  function flagsFor(event) {
    if (event.keystroke === undefined || event.keystroke === null) {
      return { keystroke: null, defaultPrevented: false, stopped: false, defaulted: false };
    }
    if (latest === null || latest.keystroke !== event.keystroke) {
      latest = {
        keystroke: event.keystroke,
        defaultPrevented: false,
        stopped: false,
        defaulted: false,
      };
    }
    return latest;
  }

  function runDefaults(event, flags, run) {
    if (event.type !== "keyDown" || flags.defaulted || flags.defaultPrevented) return false;
    flags.defaulted = true;
    if (COMMAND_MODIFIERS.some((modifier) => event[modifier])) return false;
    if (event.key === "tab") {
      if (active?.tabNavigation === false) return false;
      if (event.shift) renderer.focusPrevious?.();
      else renderer.focusNext?.();
      return true;
    }
    if (event.key === "escape") {
      const top = layers[layers.length - 1];
      if (!top) return false;
      // Like every handler, the layer's Escape callback runs through the root's scheduler.
      run(() => top.layer.onEscape?.());
      return true;
    }
    return false;
  }

  function dispatch(event) {
    const binding = active;
    if (!binding) return false;
    const run = (fn) => (binding.scheduler ? binding.scheduler(fn) : fn());
    let handled = false;
    const invoke = (handler, payload) => {
      handled = true;
      run(() => {
        handler(payload, renderer);
        binding.onEvent?.(event);
      });
    };

    switch (event.type) {
      case "windowKeyDown":
      case "windowKeyUp": {
        if (event.id !== binding.generation) return false;
        const flags = flagsFor(event);
        const type = event.type === "windowKeyDown" ? "keyDown" : "keyUp";
        const handler = type === "keyDown" ? binding.onWindowKeyDown : binding.onWindowKeyUp;
        if (handler && !flags.stopped) {
          invoke(handler, keyEvent({ ...event, id: 0 }, flags, type));
        }
        if (runDefaults({ ...event, type }, flags, run)) handled = true;
        return handled;
      }
      case "windowSelectionChange": {
        if (event.id !== binding.generation || !binding.onSelectionChange) return false;
        invoke(binding.onSelectionChange, event);
        return handled;
      }
      default: {
        const handler = binding.handlers.get(event.id)?.get(event.type);
        if (event.type === "cancel") {
          // A native editor consumed Escape and reports it as a cancel: the host's handler runs,
          // then the top layer sees the Escape like a key that bubbled to the window.
          if (handler) invoke(handler, event);
          const top = layers[layers.length - 1];
          if (top) {
            run(() => top.layer.onEscape?.());
            handled = true;
          }
          return handled;
        }
        if (!handler) return false;
        if (event.type === "keyDown" || event.type === "keyUp") {
          const flags = flagsFor(event);
          if (flags.stopped) return false;
          invoke(handler, keyEvent(event, flags, event.type));
        } else {
          invoke(handler, event);
        }
        return handled;
      }
    }
  }

  function pushLayer(layer, options = {}) {
    const previous =
      options.previousFocus !== undefined ? options.previousFocus : (renderer.getFocusedElement?.() ?? null);
    const entry = { layer, previous, open: true };
    const initial = layer.initialFocus ? layer.initialFocus() : null;
    const index = layers.findIndex((open) => isDescendant(open.layer, layer));
    if (index === -1) {
      layers.push(entry);
      if (initial != null) focus(initial);
    } else {
      layers.splice(index, 0, entry);
      if (initial != null) {
        for (let i = index + 1; i < layers.length; i++) {
          const above = layers[i];
          if (isDescendant(above.layer, layer) && above.previous === previous) {
            above.previous = initial;
          }
        }
      }
    }
    return function close() {
      if (!entry.open) return;
      entry.open = false;
      layers.splice(layers.indexOf(entry), 1);
      if (layer.parent && !layers.some((open) => open.layer === layer.parent)) return;
      const target = layer.finalFocus ? layer.finalFocus(entry.previous) : null;
      if (target != null) focus(target);
    };
  }

  return {
    /** Whether `layer` is the top layer of the stack (the one that owns Escape). */
    isTopLayer(layer) {
      return layers.length > 0 && layers[layers.length - 1].layer === layer;
    },
    ids: {
      next() {
        return nextId++;
      },
    },
    current() {
      return active;
    },
    attach(handlers = {}) {
      if (active) {
        throw new Error(
          "One renderer owns one window, one native root identity and one event map; " +
            "unmount the first root before mounting another.",
        );
      }
      const binding = {
        generation: ++generation,
        handlers: handlers.handlers ?? new Map(),
        onWindowKeyDown: handlers.onWindowKeyDown,
        onWindowKeyUp: handlers.onWindowKeyUp,
        onSelectionChange: handlers.onSelectionChange,
        tabNavigation: handlers.tabNavigation ?? true,
        onEvent: handlers.onEvent,
        scheduler: handlers.scheduler,
        detach() {
          if (active !== binding) return false;
          active = null;
          return true;
        },
      };
      active = binding;
      renderer.setWindowGeneration?.(binding.generation);
      return binding;
    },
    dispatch,
    pushLayer,
  };
}

export function createRendererState(renderer) {
  const states = registry();
  let state = states.get(renderer);
  if (!state) {
    state = createState(renderer);
    states.set(renderer, state);
  }
  return state;
}

export function pushDismissLayer(renderer, layer, options) {
  return createRendererState(renderer).pushLayer(layer, options);
}

/** Whether `layer` (as registered with `pushDismissLayer`) is the top layer of the renderer's stack. */
export function isTopDismissLayer(renderer, layer) {
  return createRendererState(renderer).isTopLayer(layer);
}
