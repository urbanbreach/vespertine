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
//! The context exists while the group lays out its subtree, which is when component children
//! render, so it applies to every control built lazily inside the group (including controls
//! inside nested components and views). A control reads it once, at render time, and keeps the
//! result for the frame; nothing is read during paint.

use vespertine::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

/// Number of enclosing disabled groups of the subtree currently being laid out.
#[derive(Default)]
struct DisabledDepth(usize);

impl vespertine::Global for DisabledDepth {}

/// Whether the subtree being laid out sits inside a disabled control group.
pub fn group_disabled(cx: &App) -> bool {
    cx.try_global::<DisabledDepth>()
        .is_some_and(|depth| depth.0 > 0)
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
        if self.disabled {
            cx.default_global::<DisabledDepth>().0 += 1;
        }
        let layout_id = self.child.request_layout(window, cx);
        if self.disabled {
            cx.default_global::<DisabledDepth>().0 -= 1;
        }
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
