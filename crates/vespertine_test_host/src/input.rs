//! Input simulation and focus control.
//!
//! Every helper flushes first, routes its coordinates through the window's native hit testing,
//! drains the native events the dispatch produced and hands each to the active dispatcher.

use crate::{
    HostError, HostResult, TestHost,
    types::{logical_point, mouse_button, parse_modifiers},
};
use std::path::PathBuf;
use vespertine::{
    ExternalPaths, FileDropEvent, KeyUpEvent, Keystroke, MouseDownEvent, MouseExitEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta, ScrollWheelEvent, Subscription,
    TouchPhase, point, px,
};

fn button(index: u8) -> HostResult<vespertine::MouseButton> {
    mouse_button(index).ok_or(HostError::InvalidButton(index))
}

impl TestHost {
    fn dispatch_input(&mut self, input: PlatformInput) -> HostResult<bool> {
        self.flush();
        let handled = self
            .cx
            .simulate_platform_input(self.window, input)
            .map_err(|error| HostError::Window(format!("{error:#}")))?;
        self.cx.run_until_parked();
        self.dispatch_native_events();
        Ok(handled)
    }

    /// A complete click: mouse down then mouse up at `(x, y)` with `button` (0 left, 1 middle,
    /// 2 right) and `modifiers` (for example `"ctrl-shift"`).
    pub fn click(&mut self, x: f64, y: f64, button_index: u8, modifiers: &str) -> HostResult<()> {
        self.mouse_down(x, y, button_index, modifiers)?;
        self.mouse_up(x, y, button_index, modifiers)
    }

    /// Presses a mouse button at `(x, y)`.
    pub fn mouse_down(
        &mut self,
        x: f64,
        y: f64,
        button_index: u8,
        modifiers: &str,
    ) -> HostResult<()> {
        let button = button(button_index)?;
        self.dispatch_input(PlatformInput::MouseDown(MouseDownEvent {
            position: logical_point(x, y),
            modifiers: parse_modifiers(modifiers),
            button,
            click_count: 1,
            first_mouse: false,
        }))
        .map(drop)
    }

    /// Releases a mouse button at `(x, y)`.
    pub fn mouse_up(
        &mut self,
        x: f64,
        y: f64,
        button_index: u8,
        modifiers: &str,
    ) -> HostResult<()> {
        let button = button(button_index)?;
        self.dispatch_input(PlatformInput::MouseUp(MouseUpEvent {
            position: logical_point(x, y),
            modifiers: parse_modifiers(modifiers),
            button,
            click_count: 1,
        }))
        .map(drop)
    }

    /// Moves the pointer to `(x, y)`, optionally reporting a held button.
    pub fn mouse_move(
        &mut self,
        x: f64,
        y: f64,
        pressed_button: Option<u8>,
        modifiers: &str,
    ) -> HostResult<()> {
        let pressed_button = pressed_button.map(button).transpose()?;
        self.dispatch_input(PlatformInput::MouseMove(MouseMoveEvent {
            position: logical_point(x, y),
            pressed_button,
            modifiers: parse_modifiers(modifiers),
        }))
        .map(drop)
    }

    /// The pointer leaves the window at `(x, y)`.
    pub fn mouse_exit(&mut self, x: f64, y: f64) -> HostResult<()> {
        self.dispatch_input(PlatformInput::MouseExited(MouseExitEvent {
            position: logical_point(x, y),
            pressed_button: None,
            modifiers: vespertine::Modifiers::none(),
        }))
        .map(drop)
    }

    /// Wheel input in pixel deltas at `(x, y)`. Scrolling content down is a negative `delta_y`.
    pub fn scroll_wheel(
        &mut self,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        modifiers: &str,
    ) -> HostResult<()> {
        self.dispatch_input(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: logical_point(x, y),
            delta: ScrollDelta::Pixels(point(px(delta_x as f32), px(delta_y as f32))),
            modifiers: parse_modifiers(modifiers),
            touch_phase: TouchPhase::Moved,
        }))
        .map(drop)
    }

    /// A file drop: a drag-enter at `(x, y)` followed by submission of the dropped `paths`.
    pub fn file_drop(&mut self, x: f64, y: f64, paths: &[PathBuf]) -> HostResult<()> {
        let position = logical_point(x, y);
        self.dispatch_input(PlatformInput::FileDrop(FileDropEvent::Entered {
            position,
            paths: ExternalPaths(paths.iter().cloned().collect()),
        }))?;
        self.dispatch_input(PlatformInput::FileDrop(FileDropEvent::Submit { position }))
            .map(drop)
    }

    /// Types a whole sequence of space-separated keystroke tokens (`"cmd-a b enter"`),
    /// optionally targeted at an element: the element is focused first. Every token is parsed
    /// before anything is dispatched.
    pub fn simulate_keystrokes(&mut self, keys: &str, element: Option<u64>) -> HostResult<()> {
        let keystrokes = keys
            .split_whitespace()
            .map(|token| {
                Keystroke::parse(token).map_err(|error| HostError::InvalidKeystroke {
                    token: token.to_owned(),
                    reason: error.to_string(),
                })
            })
            .collect::<HostResult<Vec<_>>>()?;
        if let Some(id) = element {
            self.focus_element(id)?;
        }
        for keystroke in keystrokes {
            self.flush();
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.dispatch_keystroke(keystroke, cx)
                })
                .map_err(|error| HostError::Window(format!("{error:#}")))?;
            self.cx.run_until_parked();
            self.dispatch_native_events();
        }
        self.flush();
        Ok(())
    }

    /// A single key press with the key-repeat flag `is_held`, optionally targeted at an element.
    pub fn key_down(&mut self, key: &str, is_held: bool, element: Option<u64>) -> HostResult<()> {
        let keystroke = parse_key(key)?;
        if let Some(id) = element {
            self.focus_element(id)?;
        }
        self.flush();
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_key_down(keystroke, is_held, cx)
            })
            .map_err(|error| HostError::Window(format!("{error:#}")))?;
        self.cx.run_until_parked();
        self.dispatch_native_events();
        Ok(())
    }

    /// A single key release, optionally targeted at an element.
    pub fn key_up(&mut self, key: &str, element: Option<u64>) -> HostResult<()> {
        let keystroke = parse_key(key)?;
        if let Some(id) = element {
            self.focus_element(id)?;
        }
        self.dispatch_input(PlatformInput::KeyUp(KeyUpEvent { keystroke }))
            .map(drop)
    }

    /// Presses at `from`, drags with a held left button to `to` and releases, painting a frame
    /// between the pointer-down, the pointer-move and the pointer-up (selection listeners are
    /// installed during paint). Returns the selected text.
    pub fn drag_select(&mut self, from: (f64, f64), to: (f64, f64)) -> HostResult<Option<String>> {
        self.mouse_down(from.0, from.1, 0, "")?;
        self.paint_frame();
        self.mouse_move(to.0, to.1, Some(0), "")?;
        self.paint_frame();
        self.mouse_up(to.0, to.1, 0, "")?;
        self.paint_frame();
        self.selected_text()
    }

    /// Focuses an element by identity.
    pub fn focus_element(&mut self, id: u64) -> HostResult<()> {
        self.flush();
        let focused = self.on_tree(|tree, window, cx| match tree.focus_handle(id, cx) {
            Some(handle) => {
                window.focus(&handle, cx);
                true
            }
            None => false,
        })?;
        if !focused {
            return Err(HostError::NoSuchElement(id));
        }
        self.flush();
        Ok(())
    }

    /// The identity of the focused element, or `None` when focus is not on a retained element
    /// (or the host has no retained tree).
    pub fn focused_element(&mut self) -> Option<u64> {
        self.on_tree(|tree, window, cx| tree.focused_element(window, cx))
            .ok()
            .flatten()
    }

    /// Removes focus from the focused element.
    pub fn blur(&mut self) -> HostResult<()> {
        self.flush();
        self.cx
            .update_window(self.window, |_, window, cx| window.blur(cx))
            .map_err(|error| HostError::Window(format!("{error:#}")))?;
        self.flush();
        Ok(())
    }

    /// Moves focus to the next focusable element (tab order).
    pub fn focus_next(&mut self) -> HostResult<()> {
        self.traverse_focus(true)
    }

    /// Moves focus to the previous focusable element.
    pub fn focus_previous(&mut self) -> HostResult<()> {
        self.traverse_focus(false)
    }

    fn traverse_focus(&mut self, forward: bool) -> HostResult<()> {
        self.flush();
        self.cx
            .update_window(self.window, |_, window, cx| {
                if forward {
                    window.focus_next(cx)
                } else {
                    window.focus_prev(cx)
                }
            })
            .map_err(|error| HostError::Window(format!("{error:#}")))?;
        self.flush();
        Ok(())
    }

    /// Moves focus to the next focusable element within the subtree of element `id`. Returns
    /// whether focus moved.
    pub fn focus_next_within(&mut self, id: u64) -> HostResult<bool> {
        self.focus_within(id, true)
    }

    /// Moves focus to the previous focusable element within the subtree of element `id`.
    pub fn focus_previous_within(&mut self, id: u64) -> HostResult<bool> {
        self.focus_within(id, false)
    }

    fn focus_within(&mut self, id: u64, forward: bool) -> HostResult<bool> {
        self.flush();
        let moved =
            self.on_tree(|tree, window, cx| tree.focus_within(id, forward, window, cx))??;
        self.flush();
        Ok(moved)
    }

    /// Subscribes to window-level key events: the callback sees every keystroke the window
    /// dispatches, after listeners and bindings ran. Dropping the subscription unsubscribes.
    pub fn on_key_event(
        &mut self,
        mut callback: impl FnMut(&vespertine::KeystrokeEvent) + 'static,
    ) -> Subscription {
        self.cx
            .update(|cx| cx.observe_keystrokes(move |event, _window, _cx| callback(event)))
    }

    /// Subscribes to selection changes. Dropping the subscription unsubscribes.
    pub fn on_selection_change(&mut self, callback: impl FnMut() + 'static) -> Subscription {
        self.events.subscribe_selection(callback)
    }
}

fn parse_key(key: &str) -> HostResult<Keystroke> {
    Keystroke::parse(key).map_err(|error| HostError::InvalidKeystroke {
        token: key.to_owned(),
        reason: error.to_string(),
    })
}
