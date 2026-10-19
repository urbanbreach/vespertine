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
//! ## Custom elements that keep window-level state
//!
//! A replayed view does not run its elements, so an element that updates state outside the
//! frame's own output while it prepaints or paints (a registry of painted runs, a stack of
//! inherited settings, a counter numbered in paint order) would leave that state without
//! the view's contribution. Such updates go through `Window::prepaint_effect` and
//! `Window::paint_effect` (or `Window::record_prepaint_effect` and
//! `Window::record_paint_effect` when the caller performed them already): the frame records
//! the effect, and replaying the view performs it again, in order, with the rest of the
//! replayed output. An effect may depend only on what it captured and on state its
//! predecessors left behind.
//!
//! Two more hooks keep views that inherit something from an element consistent with it.
//! `Window::with_retention_context` mixes a token into the context the views built inside it
//! are replayed under, so nested views are rebuilt when what they inherit changes (a token
//! that is unique per construction rebuilds them whenever the element is rebuilt), and
//! `Window::require_whole_view_rebuild` declares that the view being built computes state
//! over its whole subtree, so it is rebuilt, never replayed around a rebuilt nested view.
//!
//! ## Testing and benchmarking
//!
//! With the `test-support` feature, `Window::set_retention_enabled`
//! turns retention off for one window (every frame constructs every view), and
//! `Window::retention_stats` reports the views rendered,
//! views reused and splice successes and failures since the last reset. The environment
//! variable `VESPERTINE_DISABLE_RETENTION=1` disables retention for the whole process.
