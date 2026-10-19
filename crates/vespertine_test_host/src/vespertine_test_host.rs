//! Production-path offscreen visual test host (DEV-006) and unified application automation
//! (DEV-007).
//!
//! # The test host
//!
//! [`TestHost`] drives the production retained tree, layout engine, hit testing, event dispatch
//! and GPU renderer through an offscreen window and lets a test inspect both the retained state
//! and the pixels and geometry of the frame that was actually painted. Nothing is simulated:
//! there is no shadow tree, no independent layout and no reconstructed pixels. The window is a
//! real [`vespertine::Window`] on the deterministic test platform; it renders with the
//! platform's production GPU renderer (wgpu on Linux, on a software adapter when no hardware
//! adapter exists) into an offscreen target and reads the pixels back. It never creates a
//! native surface: the window's logical placement is [`OFFSCREEN_ORIGIN`] on both axes, so no
//! compositor-visible surface exists and focus is never stolen, in Wayland and X11 sessions
//! alike (or with no session at all).
//!
//! ```no_run
//! use vespertine::{AppContext as _, Context, IntoElement, Render, Styled, Window, div, rgb};
//! use vespertine_test_host::{HostOptions, TestHost};
//!
//! struct Swatch;
//! impl Render for Swatch {
//!     fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
//!         div().size_full().bg(rgb(0x336699))
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut host = TestHost::with_view(HostOptions::with_size(400.0, 300.0), |_, cx| cx.new(|_| Swatch))?;
//! host.click(100.0, 100.0, 0, "")?;          // native hit testing and dispatch
//! host.screenshot("swatch.png")?;            // GPU-rendered, 800 x 600 at scale factor 2
//! # Ok(()) }
//! ```
//!
//! Host-dependent tests start with [`require_backend!`], which skips (returns early with a
//! message) when no GPU test backend exists; the host never substitutes a mock renderer.
//!
//! ## Method map
//!
//! | Capability | Methods |
//! |---|---|
//! | Availability | [`is_available`], [`unavailable_reason`] |
//! | Construction | [`TestHost::new`], [`TestHost::with_tree`], [`TestHost::with_view`], [`TestHost::with_element`], [`TestHost::viewport`] |
//! | Batches and frames | [`TestHost::apply_batch`], [`TestHost::flush`], [`TestHost::drain_events`], [`TestHost::dispatch_native_events`] |
//! | Pointer | [`TestHost::click`], [`TestHost::mouse_down`], [`TestHost::mouse_up`], [`TestHost::mouse_move`], [`TestHost::mouse_exit`], [`TestHost::scroll_wheel`], [`TestHost::drag_select`] |
//! | Keyboard and files | [`TestHost::simulate_keystrokes`], [`TestHost::key_down`], [`TestHost::key_up`], [`TestHost::file_drop`] |
//! | Focus | [`TestHost::focus_element`], [`TestHost::focused_element`], [`TestHost::focus_next`], [`TestHost::focus_previous`], [`TestHost::focus_next_within`], [`TestHost::focus_previous_within`], [`TestHost::on_key_event`] |
//! | Retained tree | [`TestHost::serialize_tree`], [`TestHost::element_by_id`], [`TestHost::elements_by_type`], [`TestHost::elements_by_text`], [`TestHost::elements_by_test_id`], [`TestHost::root_element`], [`TestHost::element_count`] |
//! | Painted frame | [`TestHost::painted_text`], [`TestHost::painted_washes`], [`TestHost::element_bounds`], [`TestHost::elements_at`], [`TestHost::accessibility_tree`] |
//! | Scrolling, selection | [`TestHost::scroll_offset`], [`TestHost::list_anchor`], [`TestHost::scroll_to`], [`TestHost::scroll_to_item`], [`TestHost::scroll_into_view`], [`TestHost::selected_text`], [`TestHost::clear_selection`], [`TestHost::on_selection_change`] |
//! | Capture and time | [`TestHost::screenshot`], [`TestHost::screenshot_image`], [`TestHost::motion_pause`], [`TestHost::motion_set`], [`TestHost::motion_fast_forward`], [`TestHost::motion_resume`], [`TestHost::advance_executor_time`] |
//!
//! Every input helper flushes before dispatch, routes its coordinates through the window's
//! native hit testing, drains the native events produced and hands each to the active
//! dispatcher (see [`HostOptions::event_dispatcher`]). Mouse dispatch goes through the window
//! exactly as in a live window, so a handler that re-enters a leased view fails in the test
//! just as it fails in production.
//!
//! ## Hosting content
//!
//! * Native views: [`TestHost::with_view`]. Report painted geometry of identified elements
//!   with [`PaintLog::record_bounds`] from the element's `on_painted` observer (DEV-005; it adds
//!   no wrapper and leaves layout unchanged, including for image and SVG leaves); strings painted through shaped
//!   lines are recorded automatically.
//! * Retained host trees (APP-006): implement [`RetainedHost`]. Capabilities beyond batches,
//!   rendering and serialization (focus by identity, scrolling, selection, syntax-cache
//!   counters, image content) are optional trait methods; an unimplemented capability surfaces
//!   as [`HostError::Unsupported`]. Selection-aware widgets signal changes with
//!   [`HostEvents::notify_selection_changed`]; elements push framework events with
//!   [`HostEvents::emit`].
//!
//! # Automation
//!
//! The [`automation`] module is the one validated contract (protocol version 1) that serves
//! in-process tests and live applications: strict-locator [`automation::Automation`], an
//! in-process backend over a [`TestHost`], the Server-Sent-Events stdio transport and a server
//! loop. See its documentation.

mod error;
mod events;
mod host;
mod input;
mod inspect;
#[cfg(feature = "profiler")]
mod overlay;
mod paint_log;
mod root;
mod tree;
mod types;

pub mod automation;
#[doc(hidden)]
pub mod fixture;

pub use error::{HostError, HostResult};
pub use events::HostEvents;
pub use host::{
    DEFAULT_HEIGHT, DEFAULT_SCALE_FACTOR, DEFAULT_WIDTH, EventDispatcher, HostOptions,
    MAX_FRAMES_PER_FLUSH, OFFSCREEN_ORIGIN, RendererFactory, SceneObserver, TestHost, is_available,
    unavailable_reason,
};
pub use paint_log::PaintLog;
pub use tree::RetainedHost;
pub use types::{
    ListAnchor, NativeEvent, PaintedWash, Rect, SyntaxCacheCounters, TreeNode, Viewport,
    mouse_button, parse_modifiers,
};

/// Skips the enclosing test (returns early with a message) when no GPU test backend is
/// available. Tests that need the host must not fake a result when the backend is missing.
#[macro_export]
macro_rules! require_backend {
    () => {
        if !$crate::is_available() {
            eprintln!(
                "skipping: GPU test backend unavailable: {}",
                $crate::unavailable_reason().unwrap_or_default()
            );
            return;
        }
    };
}
