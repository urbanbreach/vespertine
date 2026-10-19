//! # Retained frames
//!
//! Every window reconstructs only the views whose observable inputs changed. A view
//! that is built records what it read; on the next frame, when none of those inputs
//! changed and its bounds, content mask, inherited text style, inherited opacity and
//! interaction state are the same, the view's previous output is replayed and its
//! `render`, layout, prepaint and paint do not run again. When only views nested in a
//! clean view changed, the clean view replays its own output around the rebuilt children.
//!
//! No application code opts in; the explicit `AnyView::cached`
//! entry points keep their rule that an unnotified update does not invalidate them.
//!
//! ## What invalidates a view
//!
//! - An entity the view read, updated, or observed through its `Context`
//!   and then **notified** (outside drawing or during drawing).
//! - A global the view read (`cx.global`, `cx.try_global`); a presence-only read
//!   (`cx.has_global`) is invalidated only when the global is inserted or removed.
//! - The pointer position, the keyboard modifiers or the caps-lock state, when the view
//!   read them (`window.mouse_position()`, `window.modifiers()`, `window.capslock()`).
//! - The versioned scroll and list state the view read (see the virtual-list and scroll
//!   documentation).
//! - A change of the view's bounds, content mask, inherited text style, inherited
//!   opacity or rem size.
//! - A window refresh, a resize, a focus refresh, an active drag, inspector element
//!   picking, and the first frame after accessibility is activated reconstruct every
//!   view for that frame.
//!
//! A view that writes its own construction state while it is drawn does not invalidate
//! itself, but invalidates other views that read what it wrote.
//!
//! ## Observable change for ordinary views
//!
//! Before retention, an ordinary view's `render` ran on every frame the window drew. It
//! now runs only when a tracked input above changed or the view was notified. State that
//! is read **outside** tracked entities, globals and window input therefore requires an
//! explicit `cx.notify()` (or `Window::refresh`) to take effect:
//!
//! - wall-clock time, counters in statics, `Instant::now()`;
//! - interior-mutable data (`RefCell`, `Cell`, `Mutex`) shared with the rest of the app;
//! - hover state read directly from a hitbox in a custom element's paint (the standard
//!   elements notify their view when their hover state changes);
//! - anything a custom element computes from state that no entity owns.
//!
//! Animation callbacks and time-driven views request their next frame with
//! `Window::request_animation_frame` and notify
//! their view from it. Retention runs on the UI drawing thread only: render closures are
//! never run speculatively on worker threads.
//!
//! ## Testing and benchmarking
//!
//! With the `test-support` feature, `Window::set_retention_enabled`
//! turns retention off for one window (every frame constructs every view), and
//! `Window::retention_stats` reports the views rendered,
//! views reused and splice successes and failures since the last reset. The environment
//! variable `VESPERTINE_DISABLE_RETENTION=1` disables retention for the whole process.
