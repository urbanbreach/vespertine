//! The disabled context of control groups (CMP-001 R11).
//!
//! A control is effectively disabled when it, or any enclosing control group that declares
//! disabled, is disabled. Wrap a subtree in [`disabled_group`] and every control rendered
//! inside it resolves [`effective_disabled`] to `true`, whatever its own flag says:
//!
//! ```ignore
//! disabled_group(form_locked, div().child(Button::new("save")).child(Switch::new("auto")))
//! ```
//!
//! The context exists while the group lays out and prepaints its subtree, which is when
//! component children and nested views render, so it applies to every control built lazily
//! inside the group (including controls inside nested components and views). A control reads it
//! once, at render time, and keeps the result for the frame; nothing is read during paint.

use std::cell::Cell;

use vespertine::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

thread_local! {
    /// Number of enclosing disabled groups of the subtree being laid out or prepainted.
    ///
    /// The count is scoped to a group's own layout and prepaint calls, so it is not state that
    /// changes between frames, and it is not an app global: a global write would mark every
    /// view nested in the group as reading changed state, so they would be built again
    /// whenever the group is instead of replayed. The views' dependency on the group is carried
    /// by the group's retention context and whole-view rebuild instead (see [`DisabledGroup`]).
    static DISABLED_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// The retention context token of the subtree of a disabled group.
const DISABLED_CONTEXT: u64 = 0x6469_7361_626c_6564;

/// Whether the subtree being rendered sits inside a disabled control group. `cx` is the context
/// the control renders with.
pub fn group_disabled(_cx: &App) -> bool {
    DISABLED_DEPTH.get() > 0
}

/// Runs `f` inside one more disabled group, restoring the depth even if `f` unwinds.
fn in_disabled_group<R>(f: impl FnOnce() -> R) -> R {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            DISABLED_DEPTH.set(DISABLED_DEPTH.get() - 1);
        }
    }
    DISABLED_DEPTH.set(DISABLED_DEPTH.get() + 1);
    let _restore = Restore;
    f()
}

/// Whether a control whose own disabled flag is `own` is effectively disabled: it or an
/// enclosing control group is disabled. Call it while rendering the control.
pub fn effective_disabled(own: bool, cx: &App) -> bool {
    own || group_disabled(cx)
}

/// An element that declares a control group's disabled state for its subtree.
pub struct DisabledGroup {
    disabled: bool,
    child: AnyElement,
}

/// Declares `disabled` for every control rendered inside `child`. An enabled group changes
/// nothing; a disabled group disables all of its controls, even those that were enabled on
/// their own and even inside enabled groups nested in it.
pub fn disabled_group(disabled: bool, child: impl IntoElement) -> DisabledGroup {
    DisabledGroup {
        disabled,
        child: child.into_any_element(),
    }
}

impl IntoElement for DisabledGroup {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for DisabledGroup {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        if !self.disabled {
            return (self.child.request_layout(window, cx), ());
        }
        // Views nested in the group read the context while they render. They are built together
        // with the group, never on their own around it, and replayed only inside a disabled
        // group: the retention context is compared when a view is prepainted, and a view whose
        // context changed is rendered again there, so prepaint runs inside the group too.
        window.require_whole_view_rebuild();
        let layout_id = in_disabled_group(|| {
            window.with_retention_context(DISABLED_CONTEXT, |window| {
                self.child.request_layout(window, cx)
            })
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _element_id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout_state: &mut (),
        window: &mut Window,
        app: &mut App,
    ) {
        if !self.disabled {
            self.child.prepaint(window, app);
            return;
        }
        in_disabled_group(|| {
            window
                .with_retention_context(DISABLED_CONTEXT, |window| self.child.prepaint(window, app))
        });
    }

    fn paint(
        &mut self,
        _element_id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout_state: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        app: &mut App,
    ) {
        self.child.paint(window, app);
    }
}
