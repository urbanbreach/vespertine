//! The host side of the automation contract: what a host application must be able to do, and
//! the one dispatcher that maps validated catalog requests onto it.

use super::protocol::{AutomationError, Capability, ErrorCode, METHODS, Request, validate_request};
use crate::{HostError, Rect, TestHost, TreeNode};
use serde_json::{Value, json};
use std::path::Path;

fn unsupported(what: &str) -> AutomationError {
    AutomationError::new(
        ErrorCode::Unsupported,
        format!("{what} is not supported by this host"),
    )
}

/// A host that automation can drive: the in-process [`TestHost`], or a live application.
///
/// Implementations perform each operation against the real native tree, input path, painted
/// frame and clock. They MUST flush and tick the host around every mutation and every input
/// enough that a following tree, bounds or screenshot query observes the frame those operations
/// produced. Every operation has a default that fails with `Unsupported`, so a host that lacks a
/// capability (or a host mode that cannot perform an operation, such as a live window without
/// frame readback) fails explicitly instead of silently succeeding. [`AutomationHost::capabilities`]
/// and [`AutomationHost::viewport`] are required.
#[allow(unused_variables)]
pub trait AutomationHost {
    /// The capabilities this host offers; a subset of input, screenshot, clock, tree.
    fn capabilities(&self) -> Vec<Capability>;
    /// The viewport size `(width, height)` in logical pixels.
    fn viewport(&mut self) -> (f64, f64);

    /// Click at a window point.
    fn click(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        Err(unsupported("click"))
    }
    /// Press a button.
    fn mouse_down(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        Err(unsupported("mouseDown"))
    }
    /// Release a button.
    fn mouse_up(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        Err(unsupported("mouseUp"))
    }
    /// Move the pointer, with an optional held button.
    fn mouse_move(
        &mut self,
        x: f64,
        y: f64,
        pressed: Option<u8>,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        Err(unsupported("mouseMove"))
    }
    /// Wheel input in pixel deltas.
    fn scroll_wheel(
        &mut self,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        Err(unsupported("scrollWheel"))
    }
    /// Space-separated keystroke tokens, optionally targeted at an element.
    fn keystrokes(&mut self, keys: &str, element: Option<u64>) -> Result<(), AutomationError> {
        Err(unsupported("keystrokes"))
    }
    /// A key press with the held (repeat) flag.
    fn key_down(
        &mut self,
        key: &str,
        held: bool,
        element: Option<u64>,
    ) -> Result<(), AutomationError> {
        Err(unsupported("keyDown"))
    }
    /// A key release.
    fn key_up(&mut self, key: &str, element: Option<u64>) -> Result<(), AutomationError> {
        Err(unsupported("keyUp"))
    }
    /// Focus an element.
    fn focus(&mut self, element: u64) -> Result<(), AutomationError> {
        Err(unsupported("focus"))
    }
    /// Remove focus.
    fn blur(&mut self) -> Result<(), AutomationError> {
        Err(unsupported("blur"))
    }
    /// Scroll a container to an absolute offset.
    fn scroll_to(&mut self, element: u64, x: f64, y: f64) -> Result<(), AutomationError> {
        Err(unsupported("scrollTo"))
    }
    /// The scroll offset of a container, `None` for elements that do not scroll.
    fn scroll_offset(&mut self, element: u64) -> Result<Option<(f64, f64)>, AutomationError> {
        Err(unsupported("getScrollOffset"))
    }
    /// The retained tree with painted bounds attached.
    fn tree(&mut self) -> Result<Option<TreeNode>, AutomationError> {
        Err(unsupported("getTree"))
    }
    /// The strings painted in the last frame, in paint order.
    fn painted_text(&mut self) -> Result<Vec<String>, AutomationError> {
        Err(unsupported("getPaintedText"))
    }
    /// Every retained text, in document order.
    fn all_text(&mut self) -> Result<Vec<String>, AutomationError> {
        Err(unsupported("getAllText"))
    }
    /// Painted bounds of an element.
    fn bounds(&mut self, element: u64) -> Result<Option<Rect>, AutomationError> {
        Err(unsupported("getBounds"))
    }
    /// The selected text.
    fn selected_text(&mut self) -> Result<Option<String>, AutomationError> {
        Err(unsupported("getSelectedText"))
    }
    /// Lets `ms` milliseconds of host time pass while a locator polls: a deterministic host
    /// advances its executor time; the default sleeps.
    fn idle(&mut self, ms: f64) {
        std::thread::sleep(std::time::Duration::from_secs_f64(ms.max(0.0) / 1000.0));
    }
    /// Clear the selection.
    fn clear_selection(&mut self) -> Result<(), AutomationError> {
        Err(unsupported("clearSelection"))
    }
    /// Save a PNG of the painted frame.
    fn screenshot(&mut self, path: &Path) -> Result<(), AutomationError> {
        Err(unsupported("screenshot"))
    }
    /// Freeze the motion clock; returns elapsed milliseconds.
    fn clock_pause(&mut self) -> Result<f64, AutomationError> {
        Err(unsupported("clockPause"))
    }
    /// Resume the motion clock.
    fn clock_resume(&mut self) -> Result<f64, AutomationError> {
        Err(unsupported("clockResume"))
    }
    /// Freeze the motion clock at an absolute time.
    fn clock_set(&mut self, now_ms: f64) -> Result<f64, AutomationError> {
        Err(unsupported("clockSet"))
    }
    /// Advance the motion clock and freeze it.
    fn clock_fast_forward(&mut self, delta_ms: f64) -> Result<f64, AutomationError> {
        Err(unsupported("clockFastForward"))
    }
}

fn f(params: &Value, key: &str) -> f64 {
    params.get(key).and_then(Value::as_f64).unwrap_or_default()
}

fn button(params: &Value, key: &str) -> Option<u8> {
    params
        .get(key)
        .and_then(Value::as_f64)
        .map(|button| button as u8)
}

fn modifiers(params: &Value) -> &str {
    params
        .get("modifiers")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn element(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(Value::as_f64).map(|id| id as u64)
}

fn ok() -> Value {
    json!({ "ok": true })
}

/// Dispatches one validated request onto a host and returns the validated result.
///
/// A method whose capability the host lacks fails with `Unsupported` before the host is asked.
/// `initialize` is answered here (version negotiation, process identity, capabilities, viewport).
pub fn dispatch(
    host: &mut dyn AutomationHost,
    request: &Request,
) -> Result<Value, AutomationError> {
    let params = &request.params;
    let method = request.method;
    if let Some(capability) = method.capability
        && !host.capabilities().contains(&capability)
    {
        return Err(AutomationError::new(
            ErrorCode::Unsupported,
            format!(
                "{} needs the {} capability, which this host does not offer",
                method.name,
                capability.as_str()
            ),
        ));
    }
    let result = match method.name {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(Value::as_i64);
            if version != Some(super::protocol::PROTOCOL_VERSION) {
                return Err(AutomationError::protocol_issues(
                    format!(
                        "unsupported protocol version {}; this host speaks version {}",
                        version.map_or("null".to_owned(), |version| version.to_string()),
                        super::protocol::PROTOCOL_VERSION
                    ),
                    vec!["protocolVersion: unsupported".to_owned()],
                ));
            }
            let (width, height) = host.viewport();
            json!({
                "protocolVersion": super::protocol::PROTOCOL_VERSION,
                "pid": std::process::id(),
                "capabilities": host.capabilities().iter().map(|cap| cap.as_str()).collect::<Vec<_>>(),
                "width": width,
                "height": height,
            })
        }
        // Requests are handled strictly in order, so by the time `cancel` runs the request it
        // names has already completed; acknowledging is the only correct answer.
        "cancel" => ok(),
        "click" => {
            host.click(
                f(params, "x"),
                f(params, "y"),
                button(params, "button").unwrap_or(0),
                modifiers(params),
            )?;
            ok()
        }
        "mouseDown" => {
            host.mouse_down(
                f(params, "x"),
                f(params, "y"),
                button(params, "button").unwrap_or(0),
                modifiers(params),
            )?;
            ok()
        }
        "mouseUp" => {
            host.mouse_up(
                f(params, "x"),
                f(params, "y"),
                button(params, "button").unwrap_or(0),
                modifiers(params),
            )?;
            ok()
        }
        "mouseMove" => {
            host.mouse_move(
                f(params, "x"),
                f(params, "y"),
                button(params, "pressedButton"),
                modifiers(params),
            )?;
            ok()
        }
        "scrollWheel" => {
            host.scroll_wheel(
                f(params, "x"),
                f(params, "y"),
                f(params, "deltaX"),
                f(params, "deltaY"),
                modifiers(params),
            )?;
            ok()
        }
        "keystrokes" => {
            let keys = params.get("keys").and_then(Value::as_str).unwrap_or("");
            host.keystrokes(keys, element(params, "elementId"))?;
            ok()
        }
        "keyDown" => {
            let key = params.get("key").and_then(Value::as_str).unwrap_or("");
            let held = params
                .get("isHeld")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            host.key_down(key, held, element(params, "elementId"))?;
            ok()
        }
        "keyUp" => {
            let key = params.get("key").and_then(Value::as_str).unwrap_or("");
            host.key_up(key, element(params, "elementId"))?;
            ok()
        }
        "focus" => {
            host.focus(element(params, "elementId").unwrap_or_default())?;
            ok()
        }
        "blur" => {
            host.blur()?;
            ok()
        }
        "scrollTo" => {
            host.scroll_to(
                element(params, "elementId").unwrap_or_default(),
                f(params, "x"),
                f(params, "y"),
            )?;
            ok()
        }
        "getScrollOffset" => {
            match host.scroll_offset(element(params, "elementId").unwrap_or_default())? {
                Some((x, y)) => json!([x, y]),
                None => Value::Null,
            }
        }
        "getTree" => host.tree()?.map_or(Value::Null, |tree| tree.to_json()),
        "getPaintedText" => json!(host.painted_text()?),
        "getAllText" => json!(host.all_text()?),
        "getBounds" => match host.bounds(element(params, "elementId").unwrap_or_default())? {
            Some(rect) => serde_json::to_value(rect).unwrap_or(Value::Null),
            None => Value::Null,
        },
        "getSelectedText" => host.selected_text()?.map_or(Value::Null, Value::String),
        "clearSelection" => {
            host.clear_selection()?;
            ok()
        }
        "screenshot" => {
            let path = params.get("path").and_then(Value::as_str).unwrap_or("");
            host.screenshot(Path::new(path))?;
            json!({ "path": path })
        }
        "clockPause" => json!({ "nowMs": host.clock_pause()? }),
        "clockResume" => json!({ "nowMs": host.clock_resume()? }),
        "clockSet" => json!({ "nowMs": host.clock_set(f(params, "nowMs"))? }),
        "clockFastForward" => json!({ "nowMs": host.clock_fast_forward(f(params, "deltaMs"))? }),
        other => {
            return Err(AutomationError::new(
                ErrorCode::Protocol,
                format!("method {other:?} has no dispatcher"),
            ));
        }
    };
    method.validate_result(&result)?;
    Ok(result)
}

/// Validates a raw `(method, params)` call against the catalog and dispatches it, as the
/// in-process backend does. The request id is not meaningful in-process.
pub fn call(
    host: &mut dyn AutomationHost,
    method_name: &str,
    params: Value,
) -> Result<Value, AutomationError> {
    let request = validate_request(&Value::from(0), method_name, &params)?;
    dispatch(host, &request)
}

/// Every catalog method name, for documentation and exhaustiveness tests.
pub fn method_names() -> Vec<&'static str> {
    METHODS.iter().map(|method| method.name).collect()
}

fn error(error: HostError) -> AutomationError {
    match error {
        HostError::NoSuchElement(id) => {
            AutomationError::new(ErrorCode::NotFound, format!("no element with id {id}"))
        }
        HostError::Unsupported(message) => AutomationError::new(ErrorCode::Unsupported, message),
        HostError::Screenshot(message) => AutomationError::new(
            ErrorCode::Unsupported,
            format!("frame capture is not available: {message}"),
        ),
        other => AutomationError::new(ErrorCode::Protocol, other.to_string()),
    }
}

impl AutomationHost for TestHost {
    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::Input,
            Capability::Screenshot,
            Capability::Clock,
            Capability::Tree,
        ]
    }

    fn viewport(&mut self) -> (f64, f64) {
        let viewport = TestHost::viewport(self);
        (viewport.width, viewport.height)
    }

    fn click(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        TestHost::click(self, x, y, button, modifiers).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn mouse_down(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        TestHost::mouse_down(self, x, y, button, modifiers).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn mouse_up(
        &mut self,
        x: f64,
        y: f64,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        TestHost::mouse_up(self, x, y, button, modifiers).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn mouse_move(
        &mut self,
        x: f64,
        y: f64,
        pressed: Option<u8>,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        TestHost::mouse_move(self, x, y, pressed, modifiers).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn scroll_wheel(
        &mut self,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        TestHost::scroll_wheel(self, x, y, dx, dy, modifiers).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn keystrokes(&mut self, keys: &str, element: Option<u64>) -> Result<(), AutomationError> {
        self.simulate_keystrokes(keys, element).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn key_down(
        &mut self,
        key: &str,
        held: bool,
        element: Option<u64>,
    ) -> Result<(), AutomationError> {
        TestHost::key_down(self, key, held, element).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn key_up(&mut self, key: &str, element: Option<u64>) -> Result<(), AutomationError> {
        TestHost::key_up(self, key, element).map_err(error)?;
        self.flush();
        Ok(())
    }

    fn focus(&mut self, element: u64) -> Result<(), AutomationError> {
        self.focus_element(element).map_err(error)
    }

    fn blur(&mut self) -> Result<(), AutomationError> {
        TestHost::blur(self).map_err(error)
    }

    fn scroll_to(&mut self, element: u64, x: f64, y: f64) -> Result<(), AutomationError> {
        TestHost::scroll_to(self, element, x, y).map_err(error)
    }

    fn scroll_offset(&mut self, element: u64) -> Result<Option<(f64, f64)>, AutomationError> {
        self.flush();
        TestHost::scroll_offset(self, element).map_err(error)
    }

    fn tree(&mut self) -> Result<Option<TreeNode>, AutomationError> {
        self.flush();
        self.serialize_tree().map_err(error)
    }

    fn painted_text(&mut self) -> Result<Vec<String>, AutomationError> {
        self.flush();
        Ok(TestHost::painted_text(self))
    }

    fn all_text(&mut self) -> Result<Vec<String>, AutomationError> {
        self.flush();
        let mut texts = Vec::new();
        if let Some(root) = self.serialize_tree().map_err(error)? {
            root.walk(&mut |node| {
                if let Some(text) = &node.text {
                    texts.push(text.clone());
                }
            });
        }
        Ok(texts)
    }

    fn bounds(&mut self, element: u64) -> Result<Option<Rect>, AutomationError> {
        self.flush();
        Ok(self.element_bounds(element))
    }

    fn selected_text(&mut self) -> Result<Option<String>, AutomationError> {
        TestHost::selected_text(self).map_err(error)
    }

    fn clear_selection(&mut self) -> Result<(), AutomationError> {
        TestHost::clear_selection(self).map_err(error)
    }

    fn screenshot(&mut self, path: &Path) -> Result<(), AutomationError> {
        TestHost::screenshot(self, path).map_err(error)
    }

    fn idle(&mut self, ms: f64) {
        if self.advance_executor_time(ms).is_err() {
            std::thread::sleep(std::time::Duration::from_secs_f64(ms.max(0.0) / 1000.0));
        }
    }

    fn clock_pause(&mut self) -> Result<f64, AutomationError> {
        Ok(self.motion_pause())
    }

    fn clock_resume(&mut self) -> Result<f64, AutomationError> {
        Ok(self.motion_resume())
    }

    fn clock_set(&mut self, now_ms: f64) -> Result<f64, AutomationError> {
        self.motion_set(now_ms).map_err(error)
    }

    fn clock_fast_forward(&mut self, delta_ms: f64) -> Result<f64, AutomationError> {
        self.motion_fast_forward(delta_ms).map_err(error)
    }
}
