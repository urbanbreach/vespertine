//! Scene layering around native child content (PLT-001).
//!
//! A window opts into layered composition with
//! [`Window::enable_scene_overlay`](crate::Window::enable_scene_overlay) (two planes) or
//! [`Window::enable_window_composition`](crate::Window::enable_window_composition) (a
//! general surface tree). Bottom to top the planes are: the base scene, native children, and
//! a transparent overlay holding deferred framework content (menus, popovers, dialogs,
//! prompts, drag visuals, tooltips). Windows that never opt in draw exactly as before.
//!
//! # Frame submission
//!
//! The framework keeps one paint stream and records every switch of the paint target surface
//! ([`SurfaceSwitch`]). At presentation the platform window receives either
//! [`PlatformWindow::draw_layered`](crate::PlatformWindow::draw_layered) (the whole scene, an
//! overlay start index and a capture flag, when only the base and overlay planes exist) or
//! [`PlatformWindow::draw_composed`](crate::PlatformWindow::draw_composed) (one drawable scene
//! per framework surface in tree order, with per-layer hitboxes). Platforms derive the
//! per-plane scenes with [`Scene::split_at_overlay`](crate::Scene::split_at_overlay), which
//! restores the scopes crossing the split so clipping stays balanced.
//!
//! # What a backend implements
//!
//! * `enable_scene_overlay` / `enable_window_composition` (return an error describing why
//!   composition is unavailable; the window then keeps drawing a single plane),
//!   `native_composition` (display-server handles plus the generation, which changes on every
//!   compositor-surface recreation),
//!   `create_native_attachment` and `sync_composition_order` (the layout is authoritative:
//!   surfaces missing from it are destroyed), `draw_layered` / `draw_composed`,
//!   `set_overlay_input_region` and the focus hooks.
//! * Wayland: every non-window surface is a subsurface; overlays are synchronized and drawn
//!   first, natives are desynchronized; stacking follows the flattened layout through
//!   place-above/place-below.
//! * X11 (not implemented by the core): every framework surface is drawn into the window
//!   itself in tree order. Native producers are child windows. When
//!   [`decide_x11_composition_mode`] says [`X11CompositionMode::Composited`], the framework
//!   content above native children also goes into a transparent override-redirect overlay
//!   window with an input shape from [`overlay_input_shape`], created when
//!   [`overlay_window_needed`] and destroyed (keeping its renderer) when it is not, and
//!   [`native_cutout`] with opaque-only rectangles is cut out of the children's shapes.
//!   Otherwise the SHAPE fallback cuts every occluder above a child out of that child's
//!   bounding and input shapes. Enabling fails with [`require_x11_shape_extension`] when SHAPE
//!   is missing. Attachment geometry is applied in the presentation callbacks and flushed in
//!   one batch right before the window's own presentation.
//!
//! # Limitations
//!
//! * X11 without an ARGB compositing manager cannot blend translucent overlay content over
//!   native pixels: the cutout can only replace native pixels, so translucent backdrops and
//!   shadows show the region without the native content.
//! * X11 has no cross-window atomic presentation; independent windows may be composed one
//!   display refresh apart.
//! * Framework backdrop blur cannot sample native pixels on either backend. Inside the overlay
//!   plane it blurs the earlier overlay content and the base plane where the backend can share
//!   the finished base plane (Wayland, opaque window); on X11 and for translucent windows it
//!   blurs only the overlay plane's own earlier content, so the effect region shows the base
//!   plane unblurred (PLT-001 R9, RND-003 R16).
//! * An attachment hosted above the overlay can cover later framework content painted on the
//!   same overlay.

mod composed;
mod controller;
mod input;
mod native;
mod occlusion;
mod tree;

pub use composed::*;
pub use controller::*;
pub use input::*;
pub use native::*;
pub use occlusion::*;
pub use tree::*;
