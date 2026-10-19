//! Pointer capture bound to element identity, pointer presence, keyboard-to-pointer modality
//! slop and hover suppression during stationary-pointer wheel scrolling.

use std::{any::Any, time::Duration};

use smallvec::SmallVec;
use vespertine_scheduler::Instant;

use super::{HitboxId, Window};
use crate::{
    App, GlobalElementId, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, PinchEvent,
    Pixels, Point, ScrollWheelEvent, Task,
};

/// Squared distance, in logical pixels, an idle pointer may travel from the position at which
/// keyboard modality began without switching the window back to mouse modality (8 px,
/// inclusive).
pub(crate) const KEYBOARD_POINTER_SLOP_SQUARED: f32 = 64.0;

/// How long hover-dependent drawing stays frozen to "nothing hovered" after the most recent
/// scroll-wheel event while the pointer has not moved.
pub const HOVER_SUPPRESSION_INTERVAL: Duration = Duration::from_millis(100);

/// Whether `position` lies within the keyboard slop of the position where keyboard modality began.
pub(crate) fn pointer_within_keyboard_slop(position: Point<Pixels>, origin: Point<Pixels>) -> bool {
    let dx = position.x.0 - origin.x.0;
    let dy = position.y.0 - origin.y.0;
    dx * dx + dy * dy <= KEYBOARD_POINTER_SLOP_SQUARED
}

/// Per-window pointer bookkeeping that is not part of the frame.
#[derive(Default)]
pub(super) struct PointerState {
    /// Stable identity of the element that declaratively captured the pointer.
    pub captured_element: Option<GlobalElementId>,
    /// Whether the captured element inserted a hitbox during the frame being drawn.
    pub captured_element_painted: bool,
    /// Set by a platform pointer-exit event until any later pointer input arrives.
    pub absent: bool,
    /// Pointer position at the moment keyboard modality began.
    pub modality_origin: Point<Pixels>,
    /// Position and time of the most recent scroll-wheel event.
    pub wheel: Option<(Point<Pixels>, Instant)>,
    /// Test-support switch turning wheel hover suppression off.
    pub hover_suppression_disabled: bool,
    /// True only while a frame is being drawn with hover suppression in effect.
    pub paint_hover_suppressed: bool,
    /// Redraw scheduled for the end of the suppression interval.
    pub hover_suppression_timer: Option<Task<()>>,
}

impl Window {
    /// Makes `hitbox_id` capture the pointer on behalf of the element identified by
    /// `element_id`. The capture follows the element across frames (see
    /// [`Self::rebind_captured_element`]) instead of a frame-local hitbox number.
    pub(crate) fn capture_pointer_for_element(
        &mut self,
        element_id: &GlobalElementId,
        hitbox_id: HitboxId,
    ) {
        self.captured_hitbox = Some(hitbox_id);
        self.pointer.captured_element = Some(element_id.clone());
        self.pointer.captured_element_painted = true;
    }

    /// Called when an element inserts a hitbox: if that element holds a declarative capture,
    /// the capture rebinds to the new hitbox.
    pub(crate) fn rebind_captured_element(
        &mut self,
        element_id: &GlobalElementId,
        hitbox_id: HitboxId,
    ) {
        if self.pointer.captured_element.as_ref() == Some(element_id) {
            self.captured_hitbox = Some(hitbox_id);
            self.pointer.captured_element_painted = true;
        }
    }

    /// Drops every pointer capture, declarative or by hitbox.
    pub(super) fn clear_pointer_capture(&mut self) {
        self.captured_hitbox = None;
        self.pointer.captured_element = None;
        self.pointer.captured_element_painted = false;
    }

    /// Frame start: computes hover suppression for the frame being drawn.
    pub(super) fn begin_pointer_frame(&mut self, cx: &App) {
        self.pointer.captured_element_painted = false;
        self.pointer.paint_hover_suppressed = self.hover_suppressed(cx);
    }

    /// Frame end, after the new frame became the rendered frame: releases a declarative capture
    /// whose element did not paint in this frame.
    pub(super) fn end_pointer_frame(&mut self) {
        self.pointer.paint_hover_suppressed = false;
        if self.pointer.captured_element.is_none() {
            return;
        }
        let painted = self.pointer.captured_element_painted
            || self.captured_hitbox.is_some_and(|captured| {
                // A cached view replays its hitboxes without painting its elements again.
                self.rendered_frame
                    .hitboxes
                    .iter()
                    .any(|hitbox| hitbox.id == captured)
            });
        if !painted {
            self.clear_pointer_capture();
        }
    }

    /// Whether the platform currently reports the pointer inside the window. A pointer-exit
    /// event marks the pointer absent until the next pointer move, press, release, scroll-wheel
    /// or pinch event.
    pub fn is_pointer_present(&self) -> bool {
        !self.pointer.absent
    }

    /// Tracks pointer presence and the wheel-scroll hover suppression state for an event that is
    /// about to be dispatched through the mouse listeners.
    pub(super) fn note_pointer_event(&mut self, event: &dyn Any, cx: &mut App) {
        if event.is::<MouseExitEvent>() {
            let had_hover = self.mouse_hit_test.hover_hitbox_count > 0;
            self.pointer.absent = true;
            if had_hover {
                self.refresh();
            }
            return;
        }
        if let Some(wheel) = event.downcast_ref::<ScrollWheelEvent>() {
            self.pointer.absent = false;
            self.note_wheel_event(wheel.position, cx);
        } else if let Some(mouse_move) = event.downcast_ref::<MouseMoveEvent>() {
            self.pointer.absent = false;
            if let Some((wheel_position, _)) = self.pointer.wheel
                && wheel_position != mouse_move.position
            {
                self.pointer.wheel = None;
                self.pointer.hover_suppression_timer = None;
                self.refresh();
            }
        } else if event.is::<MouseDownEvent>()
            || event.is::<MouseUpEvent>()
            || event.is::<PinchEvent>()
        {
            self.pointer.absent = false;
        }
    }

    fn note_wheel_event(&mut self, position: Point<Pixels>, cx: &mut App) {
        if self.pointer.hover_suppression_disabled {
            return;
        }
        self.pointer.wheel = Some((position, cx.background_executor().now()));
        // Every wheel event restarts the interval; the previous timer is cancelled by dropping it.
        self.pointer.hover_suppression_timer = Some(self.spawn(cx, async move |cx| {
            cx.background_executor
                .timer(HOVER_SUPPRESSION_INTERVAL)
                .await;
            cx.update(|window, cx| window.hover_suppression_expired(cx))
                .ok();
        }));
    }

    fn hover_suppression_expired(&mut self, cx: &App) {
        self.pointer.hover_suppression_timer = None;
        if let Some((_, at)) = self.pointer.wheel
            && cx.background_executor().now() >= at + HOVER_SUPPRESSION_INTERVAL
        {
            self.pointer.wheel = None;
            // Repaint so the correct hover appears without any further input.
            self.refresh();
        }
    }

    /// Whether hover-dependent drawing is currently frozen to "nothing hovered" because a
    /// scroll-wheel event arrived less than [`HOVER_SUPPRESSION_INTERVAL`] ago and the pointer
    /// has not moved since. Geometric hit testing, cursor selection and event dispatch are not
    /// affected.
    pub fn hover_suppressed(&self, cx: &App) -> bool {
        if self.pointer.hover_suppression_disabled {
            return false;
        }
        let Some((position, at)) = self.pointer.wheel else {
            return false;
        };
        position == self.mouse_position
            && cx.background_executor().now() < at + HOVER_SUPPRESSION_INTERVAL
    }

    /// The hitboxes of the rendered frame that would be reported as hovered if the window were
    /// painted now, used to decide which cached rows must rebuild because their hover state
    /// would change. Empty while hover is suppressed (wheel scroll under a stationary pointer),
    /// in keyboard modality, and while the pointer is absent; a pointer capture makes only the
    /// captured hitbox hovered.
    pub fn predicted_hovered_hitboxes(&self, cx: &App) -> SmallVec<[HitboxId; 4]> {
        if let Some(captured) = self.captured_hitbox {
            let mut hovered = SmallVec::new();
            hovered.push(captured);
            return hovered;
        }
        if self.last_input_was_keyboard() || self.pointer.absent || self.hover_suppressed(cx) {
            return SmallVec::new();
        }
        let hit_test = self.rendered_frame.hit_test(self.mouse_position);
        hit_test
            .ids
            .iter()
            .take(hit_test.hover_hitbox_count)
            .copied()
            .collect()
    }

    /// Per-window test and measurement switch for wheel hover suppression (enabled by default),
    /// independent of scroll-content caching, so runs with and without suppression can be
    /// compared with everything else equal.
    pub fn set_hover_suppression_enabled(&mut self, enabled: bool) {
        self.pointer.hover_suppression_disabled = !enabled;
        if !enabled {
            self.pointer.wheel = None;
            self.pointer.hover_suppression_timer = None;
        }
    }
}

impl Window {
    /// The cursor style the rendered frame requests for the current pointer position, as
    /// resolved from the geometric hit test (not from suppressed hover state).
    #[cfg(any(test, feature = "test-support"))]
    pub fn requested_cursor_style(&self) -> crate::CursorStyle {
        self.rendered_frame
            .cursor_style(self)
            .unwrap_or(crate::CursorStyle::Arrow)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, time::Duration};

    use super::*;
    use crate::{
        Context, CursorStyle, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent,
        Keystroke, MouseButton, ParentElement as _, PinchEvent, Render, ScrollHandle,
        StatefulInteractiveElement as _, Styled as _, TestApp, canvas, div, point, px,
    };

    type HoverLog = Rc<RefCell<Vec<bool>>>;

    thread_local! {
        static PROBE_PAINTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn probe_paints() -> usize {
        PROBE_PAINTS.with(|paints| paints.get())
    }

    fn probe(log: HoverLog, slot: usize) -> crate::Canvas<crate::Hitbox> {
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                PROBE_PAINTS.with(|paints| paints.set(paints.get() + 1));
                let mut log = log.borrow_mut();
                if log.len() <= slot {
                    log.resize(slot + 1, false);
                }
                log[slot] = hitbox.is_hovered(window);
            },
        )
    }

    // ---- INP-001 -------------------------------------------------------------------------

    #[derive(Default)]
    struct CaptureLog {
        captured_moves: usize,
        captured_ups: usize,
    }

    struct CaptureView {
        unrelated_before: usize,
        show_captured: bool,
        log: Rc<RefCell<CaptureLog>>,
        probe: HoverLog,
    }

    impl Render for CaptureView {
        fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
            let log = self.log.clone();
            let up_log = self.log.clone();
            div()
                .size_full()
                .children((0..self.unrelated_before).map(|ix| {
                    div()
                        .id(("unrelated", ix))
                        .absolute()
                        .left(px(400.))
                        .top(px(ix as f32 * 10.))
                        .size(px(8.))
                        .hover(|style| style.opacity(0.5))
                }))
                .children(self.show_captured.then(|| {
                    div()
                        .id("captured")
                        .absolute()
                        .left(px(0.))
                        .top(px(0.))
                        .w(px(80.))
                        .h(px(40.))
                        .capture_pointer()
                        .on_mouse_move(move |_, _, _| log.borrow_mut().captured_moves += 1)
                        .on_mouse_up(MouseButton::Left, move |_, _, _| {
                            up_log.borrow_mut().captured_ups += 1
                        })
                }))
                .child(
                    probe(self.probe.clone(), 0)
                        .absolute()
                        .left(px(150.))
                        .top(px(0.))
                        .w(px(100.))
                        .h(px(40.)),
                )
        }
    }

    fn capture_window(
        app: &mut TestApp,
    ) -> (
        crate::TestAppWindow<CaptureView>,
        Rc<RefCell<CaptureLog>>,
        HoverLog,
    ) {
        let log = Rc::new(RefCell::new(CaptureLog::default()));
        let probe = HoverLog::default();
        let mut window = app.open_window({
            let log = log.clone();
            let probe = probe.clone();
            move |_, _| CaptureView {
                unrelated_before: 0,
                show_captured: true,
                log,
                probe,
            }
        });
        window.draw();
        (window, log, probe)
    }

    fn captured(window: &mut crate::TestAppWindow<CaptureView>) -> Option<crate::HitboxId> {
        window.update(|_, window, _| window.captured_hitbox())
    }

    #[test]
    fn declarative_capture_follows_element_identity_across_frames() {
        let mut app = TestApp::new();
        let (mut window, log, probe) = capture_window(&mut app);

        window.simulate_mouse_move(point(px(10.), px(10.)));
        window.simulate_mouse_down(point(px(10.), px(10.)), MouseButton::Left);
        assert!(captured(&mut window).is_some());

        window.simulate_mouse_move(point(px(200.), px(10.)));
        let moves_after_first = log.borrow().captured_moves;
        assert!(moves_after_first >= 1, "captured element receives moves");

        // Unrelated hitboxes inserted before the captured element shift numeric hitbox ids.
        window.update(|view, _, cx| {
            view.unrelated_before = 3;
            cx.notify();
        });
        window.draw();
        assert!(captured(&mut window).is_some(), "capture rebinds");
        assert_eq!(
            probe.borrow()[0],
            false,
            "probe under the pointer is not hovered"
        );

        window.simulate_mouse_move(point(px(200.), px(10.)));
        assert!(log.borrow().captured_moves > moves_after_first);
        window.draw();
        assert_eq!(probe.borrow()[0], false);

        window.simulate_mouse_up(point(px(200.), px(10.)), MouseButton::Left);
        assert_eq!(log.borrow().captured_ups, 1);
        assert!(captured(&mut window).is_none(), "released after mouse up");

        window.simulate_mouse_move(point(px(200.), px(10.)));
        window.draw();
        assert_eq!(probe.borrow()[0], true, "normal hover resumes");
    }

    #[test]
    fn capture_is_released_when_the_captured_element_stops_painting() {
        let mut app = TestApp::new();
        let (mut window, log, probe) = capture_window(&mut app);
        window.simulate_mouse_move(point(px(10.), px(10.)));
        window.simulate_mouse_down(point(px(10.), px(10.)), MouseButton::Left);
        assert!(captured(&mut window).is_some());

        window.update(|view, _, cx| {
            view.show_captured = false;
            cx.notify();
        });
        window.draw();
        assert!(
            captured(&mut window).is_none(),
            "capture ends with the frame"
        );

        // Ordinary input works again and the probe only hovers when the pointer is over it.
        window.simulate_mouse_move(point(px(200.), px(10.)));
        window.draw();
        assert_eq!(probe.borrow()[0], true);
        window.simulate_mouse_move(point(px(300.), px(10.)));
        window.draw();
        assert_eq!(probe.borrow()[0], false);
        assert_eq!(log.borrow().captured_ups, 0);
    }

    #[test]
    fn low_level_capture_is_exclusive_and_released_after_mouse_up() {
        let mut app = TestApp::new();
        let (mut window, _, probe) = capture_window(&mut app);
        window.simulate_mouse_move(point(px(200.), px(10.)));
        window.draw();
        assert_eq!(probe.borrow()[0], true);

        window.update(|_, window, _| {
            window.capture_pointer(crate::HitboxId::placeholder());
            assert_eq!(
                window.captured_hitbox(),
                Some(crate::HitboxId::placeholder())
            );
            window.refresh();
        });
        window.draw();
        assert_eq!(
            probe.borrow()[0],
            false,
            "every other hitbox reports not hovered"
        );

        window.simulate_mouse_up(point(px(200.), px(10.)), MouseButton::Left);
        assert!(captured(&mut window).is_none());

        window.update(|_, window, _| {
            window.capture_pointer(crate::HitboxId::placeholder());
            window.release_pointer();
            assert_eq!(window.captured_hitbox(), None);
        });
    }

    #[test]
    fn pointer_exit_removes_hover_until_pointer_input_resumes() {
        let mut app = TestApp::new();
        let (mut window, _, probe) = capture_window(&mut app);
        let at = point(px(200.), px(10.));

        let reestablish: [Box<dyn Fn(&mut crate::TestAppWindow<CaptureView>)>; 5] = [
            Box::new(move |w| w.simulate_mouse_move(at)),
            Box::new(move |w| {
                w.simulate_mouse_down(at, MouseButton::Right);
            }),
            Box::new(move |w| {
                w.simulate_mouse_up(at, MouseButton::Right);
            }),
            Box::new(move |w| w.simulate_scroll(at, point(px(0.), px(0.)))),
            Box::new(move |w| {
                w.simulate_event(PinchEvent {
                    position: at,
                    ..Default::default()
                })
            }),
        ];

        for resume in reestablish {
            window.simulate_mouse_move(at);
            window.draw();
            assert_eq!(probe.borrow()[0], true);

            let paints_before_exit = probe_paints();
            window.simulate_event(crate::MouseExitEvent {
                position: at,
                ..Default::default()
            });
            assert!(!window.update(|_, window, _| window.is_pointer_present()));
            assert!(
                probe_paints() > paints_before_exit,
                "the exit scheduled a repaint"
            );
            assert_eq!(probe.borrow()[0], false, "no hitbox hovered while absent");

            resume(&mut window);
            assert!(window.update(|_, window, _| window.is_pointer_present()));
            // A wheel event legitimately freezes painted hover for 100 ms; let it lapse.
            app.advance_clock(Duration::from_millis(101));
            app.run_until_parked();
            window.draw();
            assert_eq!(probe.borrow()[0], true, "hit testing resumes");
        }
    }

    #[test]
    fn captured_hitbox_keeps_hovering_across_pointer_exit() {
        let mut app = TestApp::new();
        let (mut window, _, _) = capture_window(&mut app);
        window.simulate_mouse_move(point(px(10.), px(10.)));
        window.simulate_mouse_down(point(px(10.), px(10.)), MouseButton::Left);
        window.simulate_event(crate::MouseExitEvent {
            position: point(px(10.), px(10.)),
            ..Default::default()
        });
        assert!(captured(&mut window).is_some());
        let hovered = window.update(|_, window, _| {
            let id = window.captured_hitbox().unwrap();
            id.is_hovered(window) && id.is_hovered_ignoring_last_input(window)
        });
        assert!(hovered);
    }

    fn key_down(window: &mut crate::TestAppWindow<CaptureView>, key: &str) {
        window.simulate_event(KeyDownEvent {
            keystroke: Keystroke::parse(key).unwrap(),
            is_held: false,
            prefer_character_input: false,
        });
    }

    fn move_with_button(
        window: &mut crate::TestAppWindow<CaptureView>,
        position: Point<Pixels>,
        button: Option<MouseButton>,
    ) {
        window.simulate_event(MouseMoveEvent {
            position,
            pressed_button: button,
            modifiers: Default::default(),
        });
    }

    fn keyboard_modality(window: &mut crate::TestAppWindow<CaptureView>) -> bool {
        window.update(|_, window, _| window.last_input_was_keyboard())
    }

    #[test]
    fn keyboard_modality_survives_small_idle_pointer_jitter() {
        let mut app = TestApp::new();
        let (mut window, _, _) = capture_window(&mut app);
        let origin = point(px(100.), px(100.));
        let enter_keyboard = |window: &mut crate::TestAppWindow<CaptureView>| {
            window.simulate_mouse_move(origin);
            key_down(window, "a");
            assert!(keyboard_modality(window));
        };

        enter_keyboard(&mut window);
        window.simulate_mouse_move(point(px(105.), px(105.)));
        assert!(keyboard_modality(&mut window), "distance ~7.07 preserved");
        window.simulate_mouse_move(point(px(108.), px(100.)));
        assert!(keyboard_modality(&mut window), "exactly 8 preserved");
        window.simulate_mouse_move(point(px(109.), px(100.)));
        assert!(!keyboard_modality(&mut window), "9 switches to mouse");

        enter_keyboard(&mut window);
        move_with_button(
            &mut window,
            point(px(101.), px(100.)),
            Some(MouseButton::Left),
        );
        assert!(!keyboard_modality(&mut window), "move with button switches");

        enter_keyboard(&mut window);
        window.simulate_mouse_down(origin, MouseButton::Left);
        assert!(!keyboard_modality(&mut window), "press switches");
        window.simulate_mouse_up(origin, MouseButton::Left);

        enter_keyboard(&mut window);
        window.simulate_mouse_move(point(px(105.), px(100.)));
        assert!(keyboard_modality(&mut window));
        window.simulate_mouse_move(point(px(110.), px(100.)));
        assert!(
            !keyboard_modality(&mut window),
            "cumulative 10 from the original origin switches"
        );

        // A second key press while already in keyboard modality does not reset the origin.
        window.simulate_mouse_move(origin);
        key_down(&mut window, "a");
        window.simulate_mouse_move(point(px(105.), px(100.)));
        key_down(&mut window, "b");
        assert!(keyboard_modality(&mut window));
        window.simulate_mouse_move(point(px(110.), px(100.)));
        assert!(!keyboard_modality(&mut window));
    }

    // ---- INP-001 R6: pressed state --------------------------------------------------------

    struct PressedView {
        group_members: bool,
    }

    impl Render for PressedView {
        fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
            if self.group_members {
                return div()
                    .size_full()
                    .child(
                        div()
                            .id("group")
                            .group("g")
                            .capture_pointer()
                            .absolute()
                            .left(px(0.))
                            .top(px(0.))
                            .size(px(60.))
                            .child(
                                div()
                                    .id("member-a")
                                    .debug_selector(|| "member-a".to_string())
                                    .size(px(10.))
                                    .group_active("g", |style| style.size(px(30.))),
                            )
                            .child(
                                div()
                                    .id("member-b")
                                    .debug_selector(|| "member-b".to_string())
                                    .size(px(10.))
                                    .group_active("g", |style| style.size(px(20.))),
                            ),
                    )
                    .into_any_element();
            }
            div()
                .size_full()
                .child(
                    div()
                        .id("pressable")
                        .debug_selector(|| "pressable".to_string())
                        .size(px(10.))
                        .active(|style| style.size(px(20.))),
                )
                .into_any_element()
        }
    }

    fn selector_size(
        window: &mut crate::TestAppWindow<PressedView>,
        selector: &'static str,
    ) -> crate::Size<Pixels> {
        window.update(|_, window, _| {
            window
                .rendered_frame
                .debug_bounds
                .get(selector)
                .unwrap()
                .size
        })
    }

    #[test]
    fn active_style_gets_a_hitbox_and_clears_on_release_without_a_frame() {
        let mut app = TestApp::new();
        let mut window = app.open_window(|_, _| PressedView {
            group_members: false,
        });
        window.draw();
        let inside = point(px(5.), px(5.));
        assert_eq!(selector_size(&mut window, "pressable").width, px(10.));

        window.simulate_mouse_move(inside);
        window.simulate_mouse_down(inside, MouseButton::Left);
        window.draw();
        assert_eq!(selector_size(&mut window, "pressable").width, px(20.));
        assert_eq!(selector_size(&mut window, "pressable").height, px(20.));

        // Release without drawing in between, then draw once.
        window.simulate_mouse_up(inside, MouseButton::Left);
        window.draw();
        assert_eq!(selector_size(&mut window, "pressable").width, px(10.));
        assert_eq!(selector_size(&mut window, "pressable").height, px(10.));

        // Press and release with no frame between them never leaves the pressed style behind.
        window.simulate_mouse_down(inside, MouseButton::Left);
        window.simulate_mouse_up(inside, MouseButton::Left);
        window.draw();
        assert_eq!(selector_size(&mut window, "pressable").width, px(10.));
    }

    #[test]
    fn group_active_applies_to_every_member_and_clears_on_release() {
        let mut app = TestApp::new();
        let mut window = app.open_window(|_, _| PressedView {
            group_members: true,
        });
        window.draw();
        let inside = point(px(5.), px(5.));
        window.simulate_mouse_move(inside);
        window.simulate_mouse_down(inside, MouseButton::Left);
        window.draw();
        assert_eq!(selector_size(&mut window, "member-a").width, px(30.));
        assert_eq!(selector_size(&mut window, "member-b").width, px(20.));
        // The group element also captured the pointer, so it keeps the pressed state while
        // the pointer is elsewhere, and both end on release.
        window.simulate_mouse_move(point(px(300.), px(300.)));
        window.draw();
        assert_eq!(selector_size(&mut window, "member-a").width, px(30.));
        window.simulate_mouse_up(point(px(300.), px(300.)), MouseButton::Left);
        window.draw();
        assert_eq!(selector_size(&mut window, "member-a").width, px(10.));
        assert_eq!(selector_size(&mut window, "member-b").width, px(10.));
    }

    // ---- INP-002 -------------------------------------------------------------------------

    struct ScrollRowsView {
        handle: ScrollHandle,
        hover: HoverLog,
        clicked: Rc<RefCell<Vec<usize>>>,
    }

    impl Render for ScrollRowsView {
        fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .id("viewport")
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .w(px(460.))
                    .h(px(240.))
                    .overflow_y_scroll()
                    .track_scroll(&self.handle)
                    .children((0..12).map(|ix| {
                        let clicked = self.clicked.clone();
                        let row = div()
                            .id(("row", ix))
                            .relative()
                            .w(px(460.))
                            .h(px(40.))
                            .flex_none()
                            .on_click(move |_, _, _| clicked.borrow_mut().push(ix))
                            .child(
                                probe(self.hover.clone(), ix)
                                    .absolute()
                                    .left(px(0.))
                                    .top(px(0.))
                                    .w(px(460.))
                                    .h(px(40.)),
                            );
                        if ix % 2 == 0 {
                            row.cursor_pointer()
                        } else {
                            row.cursor_text()
                        }
                    })),
            )
        }
    }

    fn scroll_window(
        app: &mut TestApp,
    ) -> (
        crate::TestAppWindow<ScrollRowsView>,
        HoverLog,
        Rc<RefCell<Vec<usize>>>,
    ) {
        let hover = HoverLog::default();
        let clicked = Rc::new(RefCell::new(Vec::new()));
        let mut window = app.open_window({
            let hover = hover.clone();
            let clicked = clicked.clone();
            move |_, _| ScrollRowsView {
                handle: ScrollHandle::new(),
                hover,
                clicked,
            }
        });
        window.draw();
        (window, hover, clicked)
    }

    const POINTER: crate::Point<Pixels> = crate::Point {
        x: px(150.),
        y: px(110.),
    };

    fn wheel_one_row(window: &mut crate::TestAppWindow<ScrollRowsView>) {
        window.simulate_scroll(POINTER, point(px(0.), px(-40.)));
        window.draw();
    }

    fn hovered_rows(hover: &HoverLog) -> Vec<usize> {
        hover
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(ix, hovered)| hovered.then_some(ix))
            .collect()
    }

    fn cursor(window: &mut crate::TestAppWindow<ScrollRowsView>) -> CursorStyle {
        window.update(|_, window, _| window.requested_cursor_style())
    }

    #[test]
    fn wheel_scroll_freezes_hover_while_cursor_and_clicks_stay_live() {
        let mut app = TestApp::new();
        let (mut window, hover, clicked) = scroll_window(&mut app);
        window.simulate_mouse_move(POINTER);
        window.draw();
        assert_eq!(
            hovered_rows(&hover),
            vec![2],
            "row 3 hovered before scrolling"
        );
        assert_eq!(cursor(&mut window), CursorStyle::PointingHand);

        for step in 0..4 {
            wheel_one_row(&mut window);
            assert!(
                hovered_rows(&hover).is_empty(),
                "no hover painted after wheel {step}"
            );
            let expected = if (2 + step + 1) % 2 == 0 {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            };
            assert_eq!(
                cursor(&mut window),
                expected,
                "cursor follows row under pointer"
            );
            app.advance_clock(Duration::from_millis(10));
        }

        // A click without pointer motion activates the row now under the pointer (index 6).
        window.simulate_click(POINTER, MouseButton::Left);
        assert_eq!(*clicked.borrow(), vec![6]);
    }

    #[test]
    fn hover_returns_after_the_interval_with_a_scheduled_redraw() {
        let mut app = TestApp::new();
        let (mut window, hover, _) = scroll_window(&mut app);
        window.simulate_mouse_move(POINTER);
        wheel_one_row(&mut window);
        assert!(hovered_rows(&hover).is_empty());

        app.advance_clock(Duration::from_millis(50));
        app.run_until_parked();
        window.draw();
        assert!(hovered_rows(&hover).is_empty(), "still frozen at 50 ms");

        window.draw();
        let paints_before_expiry = probe_paints();
        app.advance_clock(Duration::from_millis(51));
        app.run_until_parked();
        assert!(
            probe_paints() > paints_before_expiry,
            "expiry requests a redraw without further input"
        );
        assert_eq!(hovered_rows(&hover), vec![3], "row 4 hovered at 101 ms");
    }

    #[test]
    fn pointer_movement_ends_suppression_immediately() {
        let mut app = TestApp::new();
        let (mut window, hover, _) = scroll_window(&mut app);
        window.simulate_mouse_move(POINTER);
        wheel_one_row(&mut window);
        app.advance_clock(Duration::from_millis(30));

        // An identical-position move does not terminate it.
        window.simulate_mouse_move(POINTER);
        window.draw();
        assert!(hovered_rows(&hover).is_empty());

        window.simulate_mouse_move(point(px(151.), px(110.)));
        window.draw();
        assert_eq!(hovered_rows(&hover), vec![3]);
    }

    #[test]
    fn repeated_wheel_events_extend_suppression() {
        let mut app = TestApp::new();
        let (mut window, hover, _) = scroll_window(&mut app);
        window.simulate_mouse_move(POINTER);
        for _ in 0..6 {
            window.simulate_scroll(POINTER, point(px(0.), px(0.)));
            window.draw();
            assert!(hovered_rows(&hover).is_empty());
            app.advance_clock(Duration::from_millis(60));
            app.run_until_parked();
            window.draw();
            assert!(
                hovered_rows(&hover).is_empty(),
                "still suppressed between events"
            );
        }
        app.advance_clock(Duration::from_millis(39));
        app.run_until_parked();
        window.draw();
        assert!(
            hovered_rows(&hover).is_empty(),
            "99 ms after the last event"
        );
        app.advance_clock(Duration::from_millis(2));
        app.run_until_parked();
        window.draw();
        assert_eq!(hovered_rows(&hover), vec![2]);
    }

    #[test]
    fn disabling_suppression_restores_upstream_hover() {
        let mut app = TestApp::new();
        let (mut window, hover, _) = scroll_window(&mut app);
        window.update(|_, window, _| window.set_hover_suppression_enabled(false));
        window.simulate_mouse_move(POINTER);
        wheel_one_row(&mut window);
        assert_eq!(hovered_rows(&hover), vec![3]);
        wheel_one_row(&mut window);
        assert_eq!(hovered_rows(&hover), vec![4]);
    }

    #[test]
    fn predicted_hover_is_empty_during_suppression_and_exact_after_expiry() {
        let mut app = TestApp::new();
        let (mut window, _, _) = scroll_window(&mut app);
        window.simulate_mouse_move(POINTER);
        window.simulate_scroll(POINTER, point(px(0.), px(-40.)));
        window.draw();
        app.read(|cx| {
            let _ = cx;
        });
        let suppressed = window.update(|_, window, cx| window.predicted_hovered_hitboxes(cx));
        assert!(suppressed.is_empty());

        app.advance_clock(Duration::from_millis(101));
        app.run_until_parked();
        let after = window.update(|_, window, cx| window.predicted_hovered_hitboxes(cx));
        let expected = window.update(|_, window, _| {
            let hit_test = window.rendered_frame.hit_test(window.mouse_position());
            hit_test.ids.iter().copied().collect::<Vec<_>>()
        });
        assert!(!after.is_empty());
        assert_eq!(after.to_vec(), expected);
    }
}
