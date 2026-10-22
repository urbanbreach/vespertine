# Key Dispatch

VESPERTINE is designed for keyboard-first interactivity.

To expose functionality to the mouse, you render a button with a click handler.

To expose functionality to the keyboard, you bind an _action_ in a _key context_.

Actions are similar to framework-level events like `MouseDown`, `KeyDown`, etc, but you can define them yourself:

```rust
mod menu {
    #[vespertine::action]
    struct MoveUp;

    #[vespertine::action]
    struct MoveDown;
}
```

Actions are frequently unit structs, for which we have a macro. The above could also be written:

```rust
mod menu {
    actions!(vespertine, [MoveUp, MoveDown]);
}
```

Actions can also be more complex types:

```rust
mod menu {
    #[vespertine::action]
    struct Move {
        direction: Direction,
        select: bool,
    }
}
```

To bind actions, chain `on_action` on to your element:

```rust
impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .on_action(|this: &mut Menu, move: &MoveUp, window: &mut Window, cx: &mut Context<Menu>| {
                // ...
            })
            .on_action(|this, move: &MoveDown, cx| {
                // ...
            })
            .children(unimplemented!())
    }
}
```

In order to bind keys to actions, you need to declare a _key context_ for part of the element tree by calling `key_context`.

```rust
impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("menu")
            .on_action(|this: &mut Menu, move: &MoveUp, window: &mut Window, cx: &mut Context<Menu>| {
                // ...
            })
            .on_action(|this, move: &MoveDown, cx| {
                // ...
            })
            .children(unimplemented!())
    }
}
```

Now you can target your context in the keymap. Note how actions are identified in the keymap by their fully-qualified type name.

```json
{
  "context": "menu",
  "bindings": {
    "up": "menu::MoveUp",
    "down": "menu::MoveDown"
  }
}
```

If you had opted for the more complex type definition, you'd provide the serialized representation of the action alongside the name:

```json
{
  "context": "menu",
  "bindings": {
    "up": ["menu::Move", {direction: "up", select: false}]
    "down": ["menu::Move", {direction: "down", select: false}]
    "shift-up": ["menu::Move", {direction: "up", select: true}]
    "shift-down": ["menu::Move", {direction: "down", select: true}]
  }
}
```

## Where dispatch starts, and losing focus

Key events and actions dispatch along the path from the focused element (one that tracks a focus
handle, `track_focus`) up to the root of the window. The key contexts on that path select the
bindings, and the `on_action` and `on_key_down` listeners on it receive the event.

When the focused element leaves the element tree (an inline editor commits and unmounts, a popover
closes), the window's focus keeps pointing at it and dispatch starts at the root element instead:
the key contexts and listeners of every element below the root stop receiving input until an
element is focused again. The framework does not move focus by itself, as in upstream GPUI.
Register a focus-lost listener once, on a view near the root, that focuses the closest focusable
ancestor the element had, or the view itself:

```rust
impl Workspace {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_focus_lost(window, |this, window, cx| {
            let target = window
                .focus_lost_restore_target(cx)
                .unwrap_or_else(|| this.focus_handle.clone());
            window.focus(&target, cx);
        })
        .detach();
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}
```

`Window::focus_lost_restore_target` answers only while focus-lost listeners run. Give every
element whose key context or listeners must keep working (a pane, a list) a focus handle with
`track_focus`, so that it can become that target; the view's own handle must be tracked by its
root element.
