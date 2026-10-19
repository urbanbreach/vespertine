//! The overlay rail element and its host trait (CMP-031 R8, R11-R15).
//!
//! [`render_rail`] is the one-call helper: it observes the host's scroll position, decides
//! whether the rail is visible, arms the single hide wake-up and returns the positioned strip
//! element with its pointer listeners (strip hover, press, drag capture, drag move and both
//! release ends). The strip exists, and is hit-testable, only while the rail is visible.

use std::{cell::RefCell, rc::Rc, time::Duration};

use vespertine::{
    AnyElement, App, DispatchPhase, ElementId, InteractiveElement as _, IntoElement as _,
    ListState, MotionClock, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    ScrollHandle, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, hsla, point,
    px,
};

use super::{
    metrics::{RAIL_HIT_THICKNESS, RAIL_THUMB_EDGE_OFFSET, RailMetrics, RailOrientation},
    state::RailState,
};

/// The shared state object of one rail. Cloning shares the state.
#[derive(Clone, Debug, Default)]
pub struct RailBar(Rc<RefCell<RailState>>);

impl RailBar {
    /// A rail with the activity policy (vertical rails).
    pub fn new() -> Self {
        Self::default()
    }

    /// A rail with the hover-reveal policy of nested horizontal planes (R14).
    pub fn hover_reveal() -> Self {
        Self(Rc::new(RefCell::new(RailState::hover_reveal())))
    }

    /// Runs `f` with the state.
    pub fn with<R>(&self, f: impl FnOnce(&mut RailState) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }

    /// Whether two handles share one state.
    pub fn same_state(&self, other: &RailBar) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Feeds the pointer's presence over a list (or plane) into `bar`: the hover-reveal policy shows
/// the rail while the element is hovered, and leaving the list straight off the strip restarts
/// the linger (R12, R14).
pub fn track_hover<E: vespertine::StatefulInteractiveElement>(element: E, bar: &RailBar) -> E {
    let bar = bar.clone();
    element.on_hover(move |hovered, window, cx| {
        let now = MotionClock::now_ms(cx);
        bar.with(|state| state.set_list_hovered(*hovered, now));
        window.refresh();
    })
}

/// What a rail needs from the container it scrolls (R15). Hosts with a scroll handle implement
/// [`RailHost::rail_scroll`] only; hosts without one (virtualized lists, emulator-style
/// scrollbacks) supply the geometry, a position value that changes exactly when the visible
/// position does, and apply the drag target fraction in their own units.
pub trait RailHost {
    /// The state object of this rail.
    fn rail_bar(&self) -> &RailBar;

    /// The scroll handle of the container, when it has one.
    fn rail_scroll(&self) -> Option<ScrollHandle> {
        None
    }

    /// The metrics of the thumb along `orientation`; `None` hides the rail. The default derives
    /// them from [`RailHost::rail_scroll`].
    fn rail_metrics(&self, orientation: RailOrientation) -> Option<RailMetrics> {
        let handle = self.rail_scroll()?;
        let bounds = handle.bounds();
        let (viewport, max, offset) = match orientation {
            RailOrientation::Vertical => {
                (bounds.size.height, handle.max_offset().y, handle.offset().y)
            }
            RailOrientation::Horizontal => {
                (bounds.size.width, handle.max_offset().x, handle.offset().x)
            }
        };
        RailMetrics::new(f32::from(viewport), f32::from(max), f32::from(-offset))
    }

    /// A value that changes exactly when the visible position does; observed every render to
    /// detect scroll activity. The default is the scroll distance of the handle.
    fn rail_position(&self, orientation: RailOrientation) -> f32 {
        match self.rail_scroll() {
            Some(handle) => match orientation {
                RailOrientation::Vertical => f32::from(-handle.offset().y),
                RailOrientation::Horizontal => f32::from(-handle.offset().x),
            },
            None => 0.0,
        }
    }

    /// A press on the strip begins a drag (a virtual list freezes its reported content height
    /// here).
    fn rail_press(&self) {}

    /// Applies the drag target: the scroll fraction `0..=1` along `orientation`. The default
    /// sets the handle's offset to `-fraction x max scroll` and keeps the other axis.
    fn rail_drag_to(&self, orientation: RailOrientation, fraction: f32) {
        if let Some(handle) = self.rail_scroll() {
            let max = handle.max_offset();
            let mut offset = handle.offset();
            match orientation {
                RailOrientation::Vertical => offset.y = -max.y * fraction,
                RailOrientation::Horizontal => offset.x = -max.x * fraction,
            }
            handle.set_offset(offset);
        }
    }

    /// The drag ended (a virtual list releases its frozen content height here).
    fn rail_release(&self) {}
}

/// A rail over a plain scroll container tracked by a [`ScrollHandle`].
#[derive(Clone, Debug)]
pub struct HandleRail {
    handle: ScrollHandle,
    bar: RailBar,
}

impl HandleRail {
    /// A rail for `handle` with the state `bar`.
    pub fn new(handle: ScrollHandle, bar: RailBar) -> Self {
        Self { handle, bar }
    }
}

impl RailHost for HandleRail {
    fn rail_bar(&self) -> &RailBar {
        &self.bar
    }

    fn rail_scroll(&self) -> Option<ScrollHandle> {
        Some(self.handle.clone())
    }
}

/// A vertical rail over a virtual list (R2, R15): its extent counts unseen rows at the average
/// measured height and stays constant while the thumb is dragged.
#[derive(Clone, Debug)]
pub struct ListRail {
    list: ListState,
    bar: RailBar,
}

impl ListRail {
    /// A rail for `list` with the state `bar`.
    pub fn new(list: ListState, bar: RailBar) -> Self {
        Self { list, bar }
    }
}

impl RailHost for ListRail {
    fn rail_bar(&self) -> &RailBar {
        &self.bar
    }

    fn rail_metrics(&self, orientation: RailOrientation) -> Option<RailMetrics> {
        if orientation != RailOrientation::Vertical {
            return None;
        }
        let viewport = f32::from(self.list.viewport_bounds().size.height);
        let (max, position) = self.list.scrollbar_extents();
        RailMetrics::new(viewport, f32::from(max), f32::from(position))
    }

    fn rail_position(&self, _orientation: RailOrientation) -> f32 {
        f32::from(self.list.scrollbar_extents().1)
    }

    fn rail_press(&self) {
        self.list.scrollbar_drag_started();
    }

    fn rail_drag_to(&self, _orientation: RailOrientation, fraction: f32) {
        let (max, _) = self.list.scrollbar_extents();
        self.list
            .set_offset_from_scrollbar(point(px(0.), -max * fraction));
    }

    fn rail_release(&self) {
        self.list.scrollbar_drag_ended();
    }
}

/// The handle-based press of R11 for a [`RailHost`]: `pointer` is the window coordinate along the
/// axis. Records the grab offset and scrolls to the pointer immediately.
pub fn begin_press(host: &dyn RailHost, orientation: RailOrientation, pointer: f32) -> Option<f32> {
    let metrics = host.rail_metrics(orientation)?;
    let bar = host.rail_bar();
    let fraction = bar.with(|state| {
        let in_track = state.pointer_in_track(pointer);
        state.begin_press_in(&metrics, in_track)
    });
    host.rail_press();
    host.rail_drag_to(orientation, fraction);
    Some(fraction)
}

/// The handle-based drag of R11: scrolls to `pointer` (a window coordinate along the axis) with
/// the recorded grab offset. Returns the fraction, `None` when no drag is in progress.
pub fn drag_to(host: &dyn RailHost, orientation: RailOrientation, pointer: f32) -> Option<f32> {
    let metrics = host.rail_metrics(orientation)?;
    let fraction = host.rail_bar().with(|state| {
        let in_track = state.pointer_in_track(pointer);
        state.drag_target_in(&metrics, in_track)
    })?;
    host.rail_drag_to(orientation, fraction);
    Some(fraction)
}

/// The handle-based release of R11.
pub fn end_press(host: &dyn RailHost, now_ms: f64) {
    let was_dragging = host.rail_bar().with(|state| {
        let dragging = state.dragging();
        state.end_press(now_ms);
        dragging
    });
    if was_dragging {
        host.rail_release();
    }
}

/// The faint foreground the thumb is drawn in: the theme's faint text role (neutral, no fixed
/// palette), falling back to a neutral grey before a theme is installed.
fn faint_foreground(cx: &App) -> vespertine::Hsla {
    vespertine_theme::appearance::try_theme_of(cx)
        .map(|theme| theme.colors.text_faint.to_hsla())
        .unwrap_or_else(|| hsla(0.0, 0.0, 0.62, 1.0))
}

fn axis_coordinate(
    orientation: RailOrientation,
    position: vespertine::Point<vespertine::Pixels>,
) -> f32 {
    match orientation {
        RailOrientation::Vertical => f32::from(position.y),
        RailOrientation::Horizontal => f32::from(position.x),
    }
}

/// Renders the rail of `host` along `orientation`, or `None` while it is hidden (the strip then
/// does not exist, so a hidden rail cannot be hovered or pressed).
///
/// Call it every render of the view that owns the list: it observes the scroll position (the
/// first observation is only a baseline) and arms the one hide wake-up. Place the returned element
/// in a `relative()` container next to the scrollable content, outside the content clip.
pub fn render_rail(
    id: impl Into<ElementId>,
    host: Rc<dyn RailHost>,
    orientation: RailOrientation,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let id: ElementId = id.into();
    let now = MotionClock::now_ms(cx);
    let bar = host.rail_bar().clone();
    let position = host.rail_position(orientation);
    bar.with(|state| state.note_scroll(position, now));
    let metrics = host.rail_metrics(orientation);
    let Some(metrics) = metrics else {
        // The content fits (or the viewport vanished): a drag cannot continue.
        end_press(host.as_ref(), now);
        return None;
    };
    let (visible, look, wake) = bar.with(|state| {
        let visible = state.visible(now);
        let wake = if visible {
            state.arm_hide_timer(now)
        } else {
            None
        };
        (visible, state.look(now), wake)
    });
    if !visible {
        return None;
    }
    if let Some(delay) = wake {
        let wake_bar = bar.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(delay / 1000.0))
                    .await;
                cx.update(|window, _| {
                    wake_bar.with(|state| state.wake_fired());
                    window.refresh();
                })
                .ok();
            })
            .detach();
    }

    let vertical = orientation == RailOrientation::Vertical;
    let mut color = faint_foreground(cx);
    color.a *= look.opacity;

    let thumb_id = (id.clone(), "thumb");
    let thumb = div()
        .id(thumb_id)
        .absolute()
        .rounded(px(look.thickness / 2.0))
        .bg(color)
        .on_hover({
            let bar = bar.clone();
            move |hovered, window, _| {
                bar.with(|state| state.set_thumb_hovered(*hovered));
                window.refresh();
            }
        });
    let thumb = if vertical {
        thumb
            .right(px(RAIL_THUMB_EDGE_OFFSET))
            .top(px(metrics.thumb_distance()))
            .w(px(look.thickness))
            .h(px(metrics.thumb_len))
    } else {
        thumb
            .bottom(px(RAIL_THUMB_EDGE_OFFSET))
            .left(px(metrics.thumb_distance()))
            .h(px(look.thickness))
            .w(px(metrics.thumb_len))
    };

    let capture = {
        let host = host.clone();
        let bar = bar.clone();
        canvas(
            {
                let bar = bar.clone();
                move |bounds, _, _| {
                    bar.with(|state| {
                        state.set_strip_origin(if vertical {
                            f32::from(bounds.origin.y)
                        } else {
                            f32::from(bounds.origin.x)
                        });
                    });
                }
            },
            move |_, _, window, _| {
                if !bar.with(|state| state.dragging()) {
                    return;
                }
                let move_host = host.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || !move_host.rail_bar().with(|s| s.dragging())
                    {
                        return;
                    }
                    if event.pressed_button != Some(MouseButton::Left) {
                        // The release happened where no event reached us (outside the window).
                        end_press(move_host.as_ref(), MotionClock::now_ms(cx));
                    } else {
                        drag_to(
                            move_host.as_ref(),
                            orientation,
                            axis_coordinate(orientation, event.position),
                        );
                    }
                    window.refresh();
                    cx.stop_propagation();
                });
                let up_host = host.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || event.button != MouseButton::Left
                        || !up_host.rail_bar().with(|s| s.dragging())
                    {
                        return;
                    }
                    end_press(up_host.as_ref(), MotionClock::now_ms(cx));
                    window.refresh();
                    cx.stop_propagation();
                });
            },
        )
        .absolute()
        .size_full()
    };

    let strip = div()
        .id(id)
        .absolute()
        .block_mouse_except_scroll()
        .cursor_default()
        .on_hover(move |hovered, window, cx| {
            let now = MotionClock::now_ms(cx);
            bar.with(|state| state.set_bar_hovered(*hovered, now));
            window.refresh();
        })
        .on_mouse_down(MouseButton::Left, {
            let host = host.clone();
            move |event, window, cx| {
                cx.stop_propagation();
                begin_press(
                    host.as_ref(),
                    orientation,
                    axis_coordinate(orientation, event.position),
                );
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let host = host.clone();
            move |_, window, cx| {
                // A release on the strip itself ends the drag even before a frame installed the
                // window-level release listener.
                end_press(host.as_ref(), MotionClock::now_ms(cx));
                window.refresh();
            }
        });
    let strip = if vertical {
        strip.top_0().bottom_0().right_0().w(px(RAIL_HIT_THICKNESS))
    } else {
        strip
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(RAIL_HIT_THICKNESS))
    };
    Some(strip.child(capture).child(thumb).into_any_element())
}
