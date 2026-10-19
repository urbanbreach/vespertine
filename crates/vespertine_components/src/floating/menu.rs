//! Anchored menu surfaces (CMP-002 R1, R2, R5, R16, R18-R21).
//!
//! [`AnchoredMenu`] is the one builder behind every native popup list: it composes the base
//! deferred and anchored elements (or, for contained placement, a measuring layer) around a card
//! filled from the surface token. The card occludes the pointer targets beneath it, consumes
//! wheel events so the content behind never scrolls, applies the entrance/exit motion and, while
//! the popup is closing, covers itself with an occluding overlay so its rows take no clicks.
//!
//! ```ignore
//! anchored_menu_below(&trigger)                    // below the trigger, start aligned, 6 px
//!     .motion(SurfaceMotion::now(&self.popup, Side::Bottom, cx))
//!     .outside_press(press)                         // optional dismissal probe
//!     .child(row_a).child(row_b)
//! ```
//!
//! Variants: [`anchored_menu`] (default: below, start aligned), [`anchored_menu_below`],
//! [`anchored_menu_below_with`] (gap and layer), [`anchored_menu_below_end`],
//! [`anchored_menu_above`], [`anchored_menu_beside`], [`anchored_menu_nested`] (priority 2, side
//! flag), [`anchored_menu_at`] (explicit pointer point) and [`contained_menu`] (limits and
//! alignment).

use super::{
    motion::{Side, SurfaceMotion},
    placement::{ContainedAlign, contained_max_size, place_contained, rows_max_height},
    trigger::{OutsidePress, TriggerBounds},
};
use crate::tokens::ComponentTokens;
use std::rc::Rc;
use vespertine::{
    Anchor, AnyElement, App, Bounds, Display, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, LayoutId, ParentElement,
    Pixels, Point, Position, Refineable, RenderOnce, Role, ScrollHandle, Size,
    StatefulInteractiveElement, Style, StyleRefinement, Styled, Window, anchored, canvas, deferred,
    div, point, px,
};

/// Padding of the menu card.
pub const MENU_PADDING: Pixels = px(4.);
/// Default gap between rows of a menu list.
pub const ROW_GAP: Pixels = px(2.);
/// Default clearance between a trigger and a menu below or above it.
pub const TRIGGER_CLEARANCE: Pixels = px(6.);
/// Default offset of a menu beside its trigger, outward.
pub const BESIDE_OFFSET: Pixels = px(6.);
/// Snap margin to the window edges.
pub const WINDOW_MARGIN: Pixels = px(8.);
/// Layer priority of top-level menus.
pub const TOP_PRIORITY: usize = 1;
/// Layer priority of nested menus.
pub const NESTED_PRIORITY: usize = 2;

/// Where a menu goes relative to its trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// Below the trigger, aligned to its start (left) or end (right) edge.
    Below {
        /// Edge alignment.
        align: ContainedAlign,
        /// Clearance to the trigger.
        gap: Pixels,
    },
    /// Above the trigger.
    Above {
        /// Edge alignment.
        align: ContainedAlign,
        /// Clearance to the trigger.
        gap: Pixels,
    },
    /// Beside the trigger, opening to the left (`left`) or right, `offset` px outward.
    Beside {
        /// Opens to the left of the trigger.
        left: bool,
        /// Outward offset.
        offset: Pixels,
    },
    /// At an explicit window point (a context menu at the pointer).
    At(Point<Pixels>),
    /// Contained inside `limits`, placed after layout from its measured size (R18).
    Contained {
        /// Containment limits.
        limits: Bounds<Pixels>,
        /// Which trigger edge the menu aligns to.
        align: ContainedAlign,
    },
}

impl Placement {
    /// The side of the trigger the menu sits on, which fixes its motion travel.
    pub fn side(&self) -> Side {
        match self {
            Placement::Below { .. } | Placement::At(_) | Placement::Contained { .. } => {
                Side::Bottom
            }
            Placement::Above { .. } => Side::Top,
            Placement::Beside { left: false, .. } => Side::Right,
            Placement::Beside { left: true, .. } => Side::Left,
        }
    }
}

/// A menu surface attached to a trigger. See the [module documentation](self).
#[derive(IntoElement)]
pub struct AnchoredMenu {
    id: ElementId,
    trigger: TriggerBounds,
    placement: Placement,
    priority: usize,
    margin: Pixels,
    row_gap: Pixels,
    motion: SurfaceMotion,
    scroll: Option<ScrollHandle>,
    outside: Option<OutsidePress>,
    focus: Option<FocusHandle>,
    key_down: Option<KeyDownListener>,
    observer: Option<Rc<dyn Fn(Bounds<Pixels>, &mut App)>>,
    style: StyleRefinement,
    rows: Vec<AnyElement>,
    heading: Option<(AnyElement, Pixels)>,
}

type KeyDownListener = Box<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;

impl AnchoredMenu {
    /// A menu with an explicit [`Placement`] (the named builders cover the common ones).
    pub fn new(id: impl Into<ElementId>, trigger: &TriggerBounds, placement: Placement) -> Self {
        menu(id, trigger, placement)
    }
}

fn menu(id: impl Into<ElementId>, trigger: &TriggerBounds, placement: Placement) -> AnchoredMenu {
    AnchoredMenu {
        id: id.into(),
        trigger: trigger.clone(),
        placement,
        priority: TOP_PRIORITY,
        margin: WINDOW_MARGIN,
        row_gap: ROW_GAP,
        motion: SurfaceMotion::REST,
        scroll: None,
        outside: None,
        focus: None,
        key_down: None,
        observer: None,
        style: StyleRefinement::default(),
        rows: Vec::new(),
        heading: None,
    }
}

/// The default menu: below the trigger, start aligned, 6 px clearance, priority 1.
pub fn anchored_menu(id: impl Into<ElementId>, trigger: &TriggerBounds) -> AnchoredMenu {
    anchored_menu_below(id, trigger)
}

/// Below the trigger, start aligned, default clearance.
pub fn anchored_menu_below(id: impl Into<ElementId>, trigger: &TriggerBounds) -> AnchoredMenu {
    anchored_menu_below_with(id, trigger, TRIGGER_CLEARANCE, TOP_PRIORITY)
}

/// Below the trigger, start aligned, with a caller clearance and layer priority.
pub fn anchored_menu_below_with(
    id: impl Into<ElementId>,
    trigger: &TriggerBounds,
    gap: Pixels,
    priority: usize,
) -> AnchoredMenu {
    menu(
        id,
        trigger,
        Placement::Below {
            align: ContainedAlign::Start,
            gap,
        },
    )
    .priority(priority)
}

/// Below the trigger, aligned to its right edge.
pub fn anchored_menu_below_end(id: impl Into<ElementId>, trigger: &TriggerBounds) -> AnchoredMenu {
    menu(
        id,
        trigger,
        Placement::Below {
            align: ContainedAlign::End,
            gap: TRIGGER_CLEARANCE,
        },
    )
}

/// Above the trigger, start aligned.
pub fn anchored_menu_above(id: impl Into<ElementId>, trigger: &TriggerBounds) -> AnchoredMenu {
    menu(
        id,
        trigger,
        Placement::Above {
            align: ContainedAlign::Start,
            gap: TRIGGER_CLEARANCE,
        },
    )
}

/// Beside the trigger, 6 px outward; `left` opens to the left.
pub fn anchored_menu_beside(
    id: impl Into<ElementId>,
    trigger: &TriggerBounds,
    left: bool,
) -> AnchoredMenu {
    menu(
        id,
        trigger,
        Placement::Beside {
            left,
            offset: BESIDE_OFFSET,
        },
    )
}

/// A nested menu (submenu): beside its trigger row at layer priority 2; `left` mirrors the
/// horizontal sign.
pub fn anchored_menu_nested(
    id: impl Into<ElementId>,
    trigger: &TriggerBounds,
    left: bool,
) -> AnchoredMenu {
    anchored_menu_beside(id, trigger, left).priority(NESTED_PRIORITY)
}

/// At an explicit window point (the trigger is ignored).
pub fn anchored_menu_at(id: impl Into<ElementId>, position: Point<Pixels>) -> AnchoredMenu {
    menu(id, &TriggerBounds::new(), Placement::At(position))
}

/// A contained menu: placed after layout inside `limits` from its measured size (R18), with
/// the maximum size of R19 and an inner scroll region.
pub fn contained_menu(
    id: impl Into<ElementId>,
    trigger: &TriggerBounds,
    limits: Bounds<Pixels>,
    align: ContainedAlign,
) -> AnchoredMenu {
    menu(id, trigger, Placement::Contained { limits, align })
}

impl AnchoredMenu {
    /// Layer priority (a menu opened inside a priority-2 palette passes a higher one).
    pub fn priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }

    /// Snap margin to the window edges (default 8 px).
    pub fn margin(mut self, margin: Pixels) -> Self {
        self.margin = margin;
        self
    }

    /// Gap between rows (default 2 px).
    pub fn row_gap(mut self, gap: Pixels) -> Self {
        self.row_gap = gap;
        self
    }

    /// The motion of this frame ([`SurfaceMotion::now`]).
    pub fn motion(mut self, motion: SurfaceMotion) -> Self {
        self.motion = motion;
        self
    }

    /// The scroll handle of the inner scroll region of a contained menu; the owner calls
    /// [`ScrollHandle::scroll_to_item`] on it to bring the selected row into view (nearest edge).
    pub fn scroll_handle(mut self, handle: &ScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// Adds the dismissal probe (outside press, bounds tracking) to the card.
    pub fn outside_press(mut self, press: OutsidePress) -> Self {
        self.outside = Some(press);
        self
    }

    /// Makes the card the focus target of the layer: its handle receives the opening focus and
    /// key events reach [`AnchoredMenu::on_key_down`] while focus is inside it.
    pub fn track_focus(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }

    /// A key-down listener on the card.
    pub fn on_key_down(
        mut self,
        listener: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.key_down = Some(Box::new(listener));
        self
    }

    /// Reports the painted bounds of the whole card (inspection, test hosts).
    pub fn observe_bounds(mut self, observer: Rc<dyn Fn(Bounds<Pixels>, &mut App)>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Fixed heading chrome above the rows, `height` tall. Only the rows scroll: in a contained
    /// menu their available height is the card's maximum height minus the chrome (R19).
    pub fn heading(mut self, heading: impl IntoElement, height: Pixels) -> Self {
        self.heading = Some((heading.into_any_element(), height));
        self
    }

    /// The placement.
    pub fn placement(&self) -> Placement {
        self.placement
    }
}

impl ParentElement for AnchoredMenu {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.rows.extend(elements);
    }
}

impl Styled for AnchoredMenu {
    /// Styles the card; caller properties override the token defaults.
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AnchoredMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = ComponentTokens::current(cx);
        if self.motion.animating {
            window.request_animation_frame();
        }
        let trigger = self.trigger.get();
        if trigger.is_none() && !matches!(self.placement, Placement::At(_)) {
            // The trigger has not painted yet: nothing to attach to. Try again next frame.
            window.request_animation_frame();
            return div().into_any_element();
        }
        let motion = self.motion;
        let contained = match self.placement {
            Placement::Contained { limits, align } => Some((limits, align)),
            _ => None,
        };

        let mut card = div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .p(MENU_PADDING)
            .bg(tokens.surface)
            .rounded(tokens.surface_radius)
            .opacity(motion.opacity)
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        card.style().refine(&self.style);
        card = card.role(Role::Menu);
        if let Some(focus) = &self.focus {
            card = card.track_focus(focus);
        }
        if let Some(listener) = self.key_down {
            card = card.on_key_down(listener);
        }

        let rows = self.rows;
        let (heading, chrome) = match self.heading {
            Some((element, height)) => (Some(element), height),
            None => (None, px(0.)),
        };
        let body = if let Some((limits, _)) = contained {
            let trigger_height = trigger.map_or(px(0.), |bounds| bounds.size.height);
            let max = contained_max_size(limits, trigger_height);
            card = card.max_h(max.height).max_w(max.width).overflow_hidden();
            let mut inner = div()
                .id((self.id.clone(), "scroll"))
                .flex()
                .flex_col()
                .gap(self.row_gap)
                .max_h(rows_max_height(
                    max.height,
                    chrome + MENU_PADDING * 2.,
                ))
                .overflow_y_scroll();
            if let Some(handle) = &self.scroll {
                inner = inner.track_scroll(handle);
            }
            card = card.children(heading).child(inner.children(rows));
            card
        } else {
            card.gap(self.row_gap).children(heading).children(rows)
        };
        let mut card = body;
        if let Some(press) = self.outside {
            card = card.child(press.probe());
        }
        if let Some(observer) = self.observer {
            card = card.child(
                canvas(move |bounds, _, cx| observer(bounds, cx), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            );
        }
        if motion.dying {
            // The exit phase: an occluding overlay over the whole dying surface swallows
            // clicks, so its rows take none and nothing underneath receives them.
            card = card.child(div().absolute().top_0().left_0().size_full().occlude());
        }

        if let Some((limits, align)) = contained {
            let layer = ContainedLayer {
                child: Some(card.into_any_element()),
                trigger: self.trigger,
                limits,
                align,
                offset: motion.offset,
            };
            return deferred(layer)
                .with_priority(self.priority)
                .into_any_element();
        }

        let (attach, anchor, offset) = attach_geometry(self.placement, trigger);
        deferred(
            anchored()
                .anchor(anchor)
                .position(attach)
                .offset(offset + motion.offset)
                .snap_to_window_with_margin(self.margin)
                .child(card),
        )
        .with_priority(self.priority)
        .into_any_element()
    }
}

/// The attach point, content anchor and outward offset of a non-contained placement.
fn attach_geometry(
    placement: Placement,
    trigger: Option<Bounds<Pixels>>,
) -> (Point<Pixels>, Anchor, Point<Pixels>) {
    let t = trigger.unwrap_or_default();
    match placement {
        Placement::Below {
            align: ContainedAlign::Start,
            gap,
        } => (t.bottom_left(), Anchor::TopLeft, point(px(0.), gap)),
        Placement::Below {
            align: ContainedAlign::End,
            gap,
        } => (t.bottom_right(), Anchor::TopRight, point(px(0.), gap)),
        Placement::Above {
            align: ContainedAlign::Start,
            gap,
        } => (t.origin, Anchor::BottomLeft, point(px(0.), -gap)),
        Placement::Above {
            align: ContainedAlign::End,
            gap,
        } => (t.top_right(), Anchor::BottomRight, point(px(0.), -gap)),
        Placement::Beside {
            left: false,
            offset,
        } => (t.top_right(), Anchor::TopLeft, point(offset, px(0.))),
        Placement::Beside { left: true, offset } => {
            (t.origin, Anchor::TopRight, point(-offset, px(0.)))
        }
        Placement::At(position) => (position, Anchor::TopLeft, point(px(0.), px(0.))),
        Placement::Contained { .. } => unreachable!("contained menus use ContainedLayer"),
    }
}

/// Positions its child after layout from the measured size (R18), like the base anchored
/// element but with [`place_contained`] instead of corner fitting.
struct ContainedLayer {
    child: Option<AnyElement>,
    trigger: TriggerBounds,
    limits: Bounds<Pixels>,
    align: ContainedAlign,
    offset: Point<Pixels>,
}

impl Element for ContainedLayer {
    type RequestLayoutState = Option<LayoutId>;
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout = self
            .child
            .as_mut()
            .map(|child| child.request_layout(window, cx));
        let style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            ..Style::default()
        };
        let layout_id = window.request_layout(style, child_layout, cx);
        (layout_id, child_layout)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (Some(child), Some(layout)) = (self.child.as_mut(), *child_layout) else {
            return;
        };
        let size: Size<Pixels> = window.layout_bounds(layout).size;
        let trigger = self.trigger.get().unwrap_or_default();
        let anchor = match self.align {
            ContainedAlign::Start => trigger.bottom_left(),
            ContainedAlign::End => trigger.bottom_right(),
        };
        let placed = place_contained(anchor, trigger.size.height, self.limits, size, self.align);
        let target = placed.origin + self.offset;
        let delta = target - bounds.origin;
        let delta = point(delta.x.round(), delta.y.round());
        window.with_element_offset(delta, |window| child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = self.child.as_mut() {
            child.paint(window, cx);
        }
    }
}

impl IntoElement for ContainedLayer {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
