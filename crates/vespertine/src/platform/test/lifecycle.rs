//! The test platform's side of the application lifecycle (PLT-008): the callbacks the
//! platform's activation mechanism would invoke, a quit counter, and the guard that fails a
//! test when displays are enumerated while a close request is being handled.

use std::cell::{Cell, RefCell};

use crate::{ActivationRequest, Bounds, Pixels, WindowBoundsTracker, WindowMode};

#[derive(Default)]
pub(crate) struct TestLifecycle {
    pub(crate) open_urls: RefCell<Option<Box<dyn FnMut(Vec<String>)>>>,
    pub(crate) reopen: RefCell<Option<Box<dyn FnMut()>>>,
    pub(crate) activation_request: RefCell<Option<Box<dyn FnMut(ActivationRequest)>>>,
    pub(crate) activate_action: RefCell<Option<Box<dyn FnMut(String)>>>,
    pub(crate) quit_requested: RefCell<Option<Box<dyn FnMut()>>>,
    /// How many times the platform was asked to stop the event loop.
    pub(crate) quit_count: Cell<usize>,
    /// While set, enumerating displays fails the test.
    pub(crate) forbid_display_enumeration: Cell<bool>,
}

impl TestLifecycle {
    /// Runs the callback in `slot` without keeping `slot` borrowed, so that the callback can
    /// call back into the platform.
    pub(crate) fn invoke<T>(slot: &RefCell<Option<Box<dyn FnMut(T)>>>, argument: T) {
        let callback = slot.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback(argument);
            slot.borrow_mut().get_or_insert(callback);
        }
    }

    #[track_caller]
    pub(crate) fn assert_display_enumeration_allowed(&self) {
        assert!(
            !self.forbid_display_enumeration.get(),
            "displays were enumerated while a window close request was being handled; use the \
             window's cached display information instead"
        );
    }
}

/// How the test platform was asked to bring a window forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestActivation {
    /// Plain activation without a token; never taken from a lifecycle request.
    Activate,
    /// Raised with a platform activation token (xdg-activation or startup id).
    WithToken(String),
    /// Attention was requested and focus was not forced.
    Attention,
}

/// A test window's mode, ordinary-bounds tracking, close callback and activation log.
pub(crate) struct TestWindowLifecycle {
    pub(crate) mode: WindowMode,
    pub(crate) tracker: WindowBoundsTracker,
    pub(crate) minimized: bool,
    pub(crate) close_callback: Option<Box<dyn FnOnce()>>,
    pub(crate) activations: Vec<TestActivation>,
}

impl TestWindowLifecycle {
    pub(crate) fn new(ordinary: Bounds<Pixels>) -> Self {
        Self {
            mode: WindowMode::Windowed,
            tracker: WindowBoundsTracker::new(ordinary),
            minimized: false,
            close_callback: None,
            activations: Vec::new(),
        }
    }
}

/// While alive, enumerating the test platform's displays fails the test.
pub(crate) struct DisplayEnumerationForbidden(Option<std::rc::Rc<super::TestPlatform>>);

impl DisplayEnumerationForbidden {
    pub(crate) fn new(platform: Option<std::rc::Rc<super::TestPlatform>>) -> Self {
        if let Some(platform) = &platform {
            platform.lifecycle.forbid_display_enumeration.set(true);
        }
        Self(platform)
    }
}

impl Drop for DisplayEnumerationForbidden {
    fn drop(&mut self) {
        if let Some(platform) = &self.0 {
            platform.lifecycle.forbid_display_enumeration.set(false);
        }
    }
}
