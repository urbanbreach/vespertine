//! Live application automation (DEV-007, APP-007 R8): the automation contract served over a
//! running window.
//!
//! [`LiveAutomation`] is an [`AutomationHost`] that proxies every operation to the UI thread
//! through the renderer's command link and blocks the calling (automation reader) thread for
//! the result, bounded by the link's response timeout. The window the operations run against is
//! the one the JavaScript renderer owns; nothing is simulated beside it.
//!
//! # Capabilities
//!
//! A capability is advertised only when every method under it works on a live window:
//!
//! * `input` — pointer, wheel and keyboard events are built as platform input and delivered with
//!   [`Window::dispatch_event`] / [`Window::dispatch_keystroke`], the path real input takes;
//!   focus, blur and scrolling go through the retained scene.
//! * `tree` — the retained tree (serialized by the same code as the offscreen test host) with
//!   the painted bounds of the last drawn frame, the strings that frame painted, scroll offsets
//!   (positive toward the far side, the convention `scrollTo` takes).
//!   The retained scene has no document selection, so the selected text is honestly `None` and
//!   clearing the selection has nothing to clear.
//! * `clock` — the motion clock is an application global operated directly.
//! * `screenshot` is **not** offered: no live platform window can read its frame back (only the
//!   offscreen test platform can), so the method fails with `Unsupported` instead of pretending.
//!
//! # Frames
//!
//! Painted bounds and text come from the test host's [`PaintLog`], which records one complete
//! frame. Before an operation (and after an input operation) the UI thread draws a complete
//! frame synchronously and then schedules the normal presenting frame, so a following tree,
//! bounds or text query observes the frame the preceding operations produced (DEV-007 R9). The
//! paint log, the text observer and the scene's painted-bounds hook are installed when the first
//! [`LiveAutomation`] attaches and removed when the last one is dropped (its channel ended), so
//! a window nobody automates paints ordinary frames with retained paint reuse.

use std::rc::Rc;

use vespertine::{
    AnyWindowHandle, App, AppContext as _, Global, KeyUpEvent, Keystroke, MotionClock,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintObserver, PlatformInput, ScrollDelta,
    ScrollWheelEvent, TouchPhase, Window, point, px,
};
use vespertine_test_host::automation::{AutomationError, AutomationHost, Capability, ErrorCode};
use vespertine_test_host::{PaintLog, Rect, TreeNode, mouse_button, parse_modifiers};

use crate::error::HostError;
use crate::link::UiLink;
use crate::runtime::{HostRenderer, HostWindow, UiTask, query_on};
use crate::test_host::snapshot_tree;
use crate::view::{HostView, SharedTree};

/// Maps a renderer failure onto the automation error codes: a UI thread that did not answer in
/// time is a `Timeout`, a renderer that is not (or no longer) running is `Closed`; anything else
/// is reported as a `Protocol` failure carrying the renderer's message.
fn automation_error(error: HostError) -> AutomationError {
    let code = match &error {
        HostError::TimedOut { .. } => ErrorCode::Timeout,
        HostError::NotInitialized | HostError::NotRunning | HostError::Stopped(_) => {
            ErrorCode::Closed
        }
        _ => ErrorCode::Protocol,
    };
    AutomationError::new(code, error.to_string())
}

fn window_gone(error: impl std::fmt::Display) -> AutomationError {
    AutomationError::new(ErrorCode::Closed, format!("the window is gone: {error}"))
}

fn unsupported(message: impl Into<String>) -> AutomationError {
    AutomationError::new(ErrorCode::Unsupported, message)
}

fn logical_point(x: f64, y: f64) -> vespertine::Point<vespertine::Pixels> {
    point(px(x as f32), px(y as f32))
}

fn button(index: u8) -> Result<vespertine::MouseButton, AutomationError> {
    mouse_button(index).ok_or_else(|| {
        AutomationError::new(ErrorCode::Protocol, format!("invalid mouse button {index}"))
    })
}

fn keystroke(token: &str) -> Result<Keystroke, AutomationError> {
    Keystroke::parse(token).map_err(|error| {
        AutomationError::new(
            ErrorCode::Protocol,
            format!("invalid keystroke {token:?}: {error}"),
        )
    })
}

fn check_duration(ms: f64) -> Result<f64, AutomationError> {
    if ms.is_finite() && ms >= 0.0 {
        Ok(ms)
    } else {
        Err(AutomationError::new(
            ErrorCode::Protocol,
            format!("{ms} is not a finite, nonnegative duration"),
        ))
    }
}

/// Draws one complete frame now, so the paint log describes exactly the current tree, then
/// schedules the normal frame that presents it. A complete frame bypasses retained paint state,
/// so every painted element reports itself.
fn draw_frame(window: &mut Window, cx: &mut App) {
    window.refresh();
    window.draw(cx).clear(cx);
    window.refresh();
}

/// The retained tree with the painted bounds of the last frame attached to every painted node.
fn painted_tree(cx: &App, tree: &SharedTree) -> Option<TreeNode> {
    let tree = tree.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut root = snapshot_tree(&tree)?;
    let log = cx.try_global::<PaintLog>();
    root.walk_mut(&mut |node| node.bounds = log.and_then(|log| log.bounds_of(node.id)));
    Some(root)
}

/// How many [`LiveAutomation`] hosts are attached to the application; the paint observation is
/// installed by the first and removed by the last.
#[derive(Default)]
struct Attached(usize);

impl Global for Attached {}

/// Installs the paint log, the text and frame observer and the scene's painted-bounds hook for
/// one more attached host.
fn install(cx: &mut App, window: &HostWindow) -> Result<(), AutomationError> {
    let attached = cx.default_global::<Attached>();
    attached.0 += 1;
    if attached.0 > 1 {
        return Ok(());
    }
    let log = cx.try_global::<PaintLog>().cloned().unwrap_or_default();
    cx.set_global(log.clone());
    let text_log = log.clone();
    cx.set_global(PaintObserver {
        on_text: Rc::new(move |painted| text_log.push_text(&painted.text)),
        // The log is cleared exactly once per drawn frame; every frame is a complete one so the
        // log never holds a partial frame.
        on_frame_start: Some(Rc::new(move |window, _| {
            log.begin_frame();
            window.refresh();
        })),
    });
    window
        .update(cx, |view, window, _| {
            view.scene()
                .set_painted_hook(Some(Rc::new(|id, bounds, cx| {
                    PaintLog::record_bounds(cx, id, bounds);
                })));
            window.refresh();
        })
        .map_err(window_gone)
}

/// Detaches one host; the last one restores ordinary frames (retained paint reuse, no
/// observation).
fn uninstall(cx: &mut App, window: &HostWindow) {
    let attached = cx.default_global::<Attached>();
    attached.0 = attached.0.saturating_sub(1);
    if attached.0 > 0 {
        return;
    }
    cx.set_global(PaintObserver {
        on_text: Rc::new(|_| {}),
        on_frame_start: None,
    });
    let _ = window.update(cx, |view, window, _| {
        view.scene().set_painted_hook(None);
        window.refresh();
    });
}

/// Which half (or both) of a button press a pointer operation delivers.
#[derive(Clone, Copy)]
enum Gesture {
    Down,
    Up,
    Click,
}

/// An [`AutomationHost`] over a live renderer window.
pub struct LiveAutomation {
    link: UiLink<UiTask>,
    tree: SharedTree,
}

impl LiveAutomation {
    /// Attaches to a running renderer: installs the paint observation on the UI thread and
    /// fails when the renderer is not initialized, the UI loop is gone or does not answer.
    pub fn new(renderer: &HostRenderer) -> Result<Self, HostError> {
        let host = Self {
            link: renderer.link()?,
            tree: renderer.tree(),
        };
        query_on(&host.link, "attaching automation", |cx, window| {
            install(cx, window)
        })?
        .map_err(|error| HostError::Failed(error.message))?;
        Ok(host)
    }

    fn query<T: Send + 'static>(
        &self,
        operation: &'static str,
        task: impl FnOnce(&mut App, &HostWindow) -> T + Send + 'static,
    ) -> Result<T, AutomationError> {
        query_on(&self.link, operation, task).map_err(automation_error)
    }

    /// Runs `task` on the UI thread with the window (not leased through its root view, so the
    /// task may draw frames and dispatch input).
    fn on_window<T: Send + 'static>(
        &self,
        operation: &'static str,
        task: impl FnOnce(&mut Window, &mut App) -> Result<T, AutomationError> + Send + 'static,
    ) -> Result<T, AutomationError> {
        self.query(operation, move |cx, window| {
            let handle: AnyWindowHandle = (*window).into();
            cx.update_window(handle, |_, window, cx| task(window, cx))
                .map_err(window_gone)?
        })?
    }

    /// Runs `task` on the UI thread with the window's view (scene operations).
    fn on_scene<T: Send + 'static>(
        &self,
        operation: &'static str,
        task: impl FnOnce(&mut HostView, &mut Window, &mut App) -> T + Send + 'static,
    ) -> Result<T, AutomationError> {
        self.query(operation, move |cx, window| {
            window
                .update(cx, |view, window, cx| task(view, window, cx))
                .map_err(window_gone)
        })?
    }

    /// Draws a complete frame.
    fn frame(&self) -> Result<(), AutomationError> {
        self.on_window("drawing a frame", |window, cx| {
            draw_frame(window, cx);
            Ok(())
        })
    }

    /// Delivers one platform input event between two complete frames: the first so hit testing
    /// sees the current layout, the second so the log describes the result.
    fn input(
        &self,
        operation: &'static str,
        events: impl FnOnce() -> Result<Vec<PlatformInput>, AutomationError> + Send + 'static,
    ) -> Result<(), AutomationError> {
        self.on_window(operation, move |window, cx| {
            let events = events()?;
            for event in events {
                draw_frame(window, cx);
                window.dispatch_event(event, cx);
            }
            draw_frame(window, cx);
            Ok(())
        })
    }

    fn pointer(
        &self,
        operation: &'static str,
        gesture: Gesture,
        (x, y): (f64, f64),
        button_index: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let modifiers = parse_modifiers(modifiers);
        self.input(operation, move || {
            let button = button(button_index)?;
            let position = logical_point(x, y);
            let mut events = Vec::new();
            if matches!(gesture, Gesture::Down | Gesture::Click) {
                events.push(PlatformInput::MouseDown(MouseDownEvent {
                    position,
                    modifiers,
                    button,
                    click_count: 1,
                    first_mouse: false,
                }));
            }
            if matches!(gesture, Gesture::Up | Gesture::Click) {
                events.push(PlatformInput::MouseUp(MouseUpEvent {
                    position,
                    modifiers,
                    button,
                    click_count: 1,
                }));
            }
            Ok(events)
        })
    }

    fn focus_target(&mut self, element: Option<u64>) -> Result<(), AutomationError> {
        match element {
            Some(id) => self.focus(id),
            None => Ok(()),
        }
    }
}

impl Drop for LiveAutomation {
    /// Ends the observation when the channel ends, so a window whose automation client is gone
    /// paints ordinary frames again. Best effort: nothing to restore when the UI loop is gone.
    fn drop(&mut self) {
        let _ = self.link.send(Box::new(|cx, window| uninstall(cx, window)));
    }
}

impl AutomationHost for LiveAutomation {
    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::Input, Capability::Tree, Capability::Clock]
    }

    fn viewport(&mut self) -> (f64, f64) {
        self.on_window("reading the viewport", |window, _| {
            let size = window.viewport_size();
            Ok((
                f64::from(f32::from(size.width)),
                f64::from(f32::from(size.height)),
            ))
        })
        .unwrap_or((0.0, 0.0))
    }

    fn click(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.pointer("click", Gesture::Click, (x, y), button, modifiers)
    }

    fn mouse_down(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.pointer("mouse down", Gesture::Down, (x, y), button, modifiers)
    }

    fn mouse_up(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        self.pointer("mouse up", Gesture::Up, (x, y), button, modifiers)
    }

    fn mouse_move(
        &mut self,
        x: f64,
        y: f64,
        pressed: Option<u8>,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let modifiers = parse_modifiers(modifiers);
        self.input("mouse move", move || {
            let pressed_button = pressed.map(button).transpose()?;
            Ok(vec![PlatformInput::MouseMove(MouseMoveEvent {
                position: logical_point(x, y),
                pressed_button,
                modifiers,
            })])
        })
    }

    fn scroll_wheel(
        &mut self,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let modifiers = parse_modifiers(modifiers);
        self.input("wheel", move || {
            Ok(vec![PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: logical_point(x, y),
                delta: ScrollDelta::Pixels(point(px(dx as f32), px(dy as f32))),
                modifiers,
                touch_phase: TouchPhase::Moved,
            })])
        })
    }

    fn keystrokes(&mut self, keys: &str, element: Option<u64>) -> Result<(), AutomationError> {
        // Every token is parsed before anything is dispatched.
        let keystrokes = keys
            .split_whitespace()
            .map(keystroke)
            .collect::<Result<Vec<_>, _>>()?;
        self.focus_target(element)?;
        self.on_window("keystrokes", move |window, cx| {
            for keystroke in keystrokes {
                draw_frame(window, cx);
                window.dispatch_keystroke(keystroke, cx);
            }
            draw_frame(window, cx);
            Ok(())
        })
    }

    fn key_down(
        &mut self,
        key: &str,
        held: bool,
        element: Option<u64>,
    ) -> Result<(), AutomationError> {
        let keystroke = keystroke(key)?;
        self.focus_target(element)?;
        self.on_window("key down", move |window, cx| {
            draw_frame(window, cx);
            window.dispatch_key_down(keystroke, held, cx);
            draw_frame(window, cx);
            Ok(())
        })
    }

    fn key_up(&mut self, key: &str, element: Option<u64>) -> Result<(), AutomationError> {
        let keystroke = keystroke(key)?;
        self.focus_target(element)?;
        self.input("key up", move || {
            Ok(vec![PlatformInput::KeyUp(KeyUpEvent { keystroke })])
        })
    }

    fn focus(&mut self, element: u64) -> Result<(), AutomationError> {
        self.frame()?;
        let focused = self.on_scene("focus", move |view, window, cx| {
            view.scene().focus(element, window, cx)
        })?;
        if !focused {
            return Err(AutomationError::new(
                ErrorCode::NotFound,
                format!("no focusable element with id {element}"),
            ));
        }
        self.frame()
    }

    fn blur(&mut self) -> Result<(), AutomationError> {
        self.on_window("blur", |window, cx| {
            draw_frame(window, cx);
            window.blur(cx);
            draw_frame(window, cx);
            Ok(())
        })
    }

    fn scroll_to(&mut self, element: u64, x: f64, y: f64) -> Result<(), AutomationError> {
        self.frame()?;
        let scrolled = self.on_scene("scrolling", move |view, window, _| {
            view.scene().scroll_to(element, x as f32, y as f32, window)
        })?;
        if !scrolled {
            return Err(unsupported(format!("element {element} does not scroll")));
        }
        self.frame()
    }

    fn scroll_offset(&mut self, element: u64) -> Result<Option<(f64, f64)>, AutomationError> {
        self.frame()?;
        let offset = self.on_scene("reading the scroll offset", move |view, _, _| {
            view.scene().scroll_offset(element)
        })?;
        // The scene reports a scroll position as a negative offset toward the far side; the
        // automation contract reads back what `scrollTo` takes (positive toward the far side).
        // `+ 0.0` turns a negated zero into a plain zero.
        Ok(offset.map(|(x, y)| (f64::from(-x) + 0.0, f64::from(-y) + 0.0)))
    }

    fn tree(&mut self) -> Result<Option<TreeNode>, AutomationError> {
        let tree = self.tree.clone();
        self.on_window("reading the tree", move |window, cx| {
            draw_frame(window, cx);
            Ok(painted_tree(cx, &tree))
        })
    }

    fn painted_text(&mut self) -> Result<Vec<String>, AutomationError> {
        self.on_window("reading painted text", |window, cx| {
            draw_frame(window, cx);
            Ok(cx
                .try_global::<PaintLog>()
                .map(PaintLog::painted_text)
                .unwrap_or_default())
        })
    }

    fn all_text(&mut self) -> Result<Vec<String>, AutomationError> {
        let tree = self.tree.clone();
        self.on_window("reading all text", move |window, cx| {
            draw_frame(window, cx);
            let mut texts = Vec::new();
            if let Some(root) = painted_tree(cx, &tree) {
                root.walk(&mut |node| {
                    if let Some(text) = &node.text {
                        texts.push(text.clone());
                    }
                });
            }
            Ok(texts)
        })
    }

    fn bounds(&mut self, element: u64) -> Result<Option<Rect>, AutomationError> {
        self.on_window("reading bounds", move |window, cx| {
            draw_frame(window, cx);
            Ok(cx
                .try_global::<PaintLog>()
                .and_then(|log| log.bounds_of(element)))
        })
    }

    fn selected_text(&mut self) -> Result<Option<String>, AutomationError> {
        // The retained scene has no document selection, so there is never selected text.
        Ok(None)
    }

    fn clear_selection(&mut self) -> Result<(), AutomationError> {
        // Nothing can be selected, so there is nothing to clear.
        Ok(())
    }

    fn clock_pause(&mut self) -> Result<f64, AutomationError> {
        self.query("pausing the clock", |cx, _| MotionClock::pause(cx))
    }

    fn clock_resume(&mut self) -> Result<f64, AutomationError> {
        self.query("resuming the clock", |cx, _| MotionClock::resume(cx))
    }

    fn clock_set(&mut self, now_ms: f64) -> Result<f64, AutomationError> {
        let now_ms = check_duration(now_ms)?;
        self.query("setting the clock", move |cx, _| {
            MotionClock::set(cx, now_ms)
        })
    }

    fn clock_fast_forward(&mut self, delta_ms: f64) -> Result<f64, AutomationError> {
        let delta_ms = check_duration(delta_ms)?;
        self.query("advancing the clock", move |cx, _| {
            MotionClock::fast_forward(cx, delta_ms)
        })
    }
}
