//! Tests for LAY-004. Time comes from the executor's fake clock
//! (`advance_clock`), pointer input from simulated platform events and
//! every "frame" is an explicit window draw following the event.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use super::*;
use crate::{
    AppContext as _, Bounds, Context, CursorStyle, Entity, FocusHandle, Hitbox, HitboxBehavior,
    InteractiveElement as _, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, Size, Styled as _, TestAppContext,
    VisualTestContext, Window, canvas, div, point, px, size,
};

fn near(actual: Pixels, expected: f32) -> bool {
    (actual.0 - expected).abs() < 0.01
}

// ---------------------------------------------------------------------------
// Pure fitting (R1, R3, A2, A6)
// ---------------------------------------------------------------------------

fn entry(descriptor: PaneDescriptor, order: u64) -> PaneEntry {
    PaneEntry {
        state: PaneState {
            size: descriptor.preferred_size,
            collapsed: descriptor.collapsed,
            opened_order: order,
            adjustment: 0,
        },
        descriptor,
    }
}

#[test]
fn presets_match_the_specification() {
    let leading = PaneDescriptor::leading("l");
    assert_eq!(
        (leading.min_size, leading.max_size, leading.default_size),
        (px(224.), px(400.), px(256.))
    );
    let trailing = PaneDescriptor::trailing("t");
    assert_eq!(
        (trailing.min_size, trailing.default_size),
        (px(360.), px(520.))
    );
    let bottom = PaneDescriptor::bottom("b");
    assert_eq!(
        (bottom.min_size, bottom.default_size, bottom.max_size),
        (px(160.), px(280.), px(2000.))
    );
    assert_eq!(MAIN_MIN_WIDTH, px(300.));
    assert_eq!(HORIZONTAL_SEAM_WIDTH, px(20.));
    assert_eq!(BOTTOM_SEAM_HEIGHT, px(10.));
}

#[test]
fn small_viewport_hides_earliest_opened_pane() {
    // 300 + 224 + 360 = 884 does not fit in 800: the earliest opened pane
    // is hidden and the remaining panes satisfy their minimums.
    let panes = [
        entry(PaneDescriptor::leading("sidebar"), 0),
        entry(PaneDescriptor::trailing("detail"), 1),
    ];
    let result = fit(size(px(800.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert!(!result.panes[0].visible);
    assert!(result.panes[0].open, "fitting never changes the open flag");
    assert!(result.panes[1].visible);
    assert!(result.panes[1].size >= px(360.));
    assert!(result.main_width >= px(300.));
    assert_eq!(result.panes[1].size + result.main_width, px(800.));

    // The opening order decides, not the declaration order.
    let panes = [
        entry(PaneDescriptor::leading("sidebar"), 5),
        entry(PaneDescriptor::trailing("detail"), 1),
    ];
    let result = fit(size(px(800.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert!(result.panes[0].visible);
    assert!(!result.panes[1].visible);
}

#[test]
fn lowest_priority_is_hidden_before_earlier_opened() {
    let panes = [
        entry(PaneDescriptor::leading("sidebar").priority(5), 0),
        entry(PaneDescriptor::trailing("detail").priority(1), 1),
    ];
    let result = fit(size(px(800.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert!(result.panes[0].visible);
    assert!(!result.panes[1].visible);
}

#[test]
fn fitting_never_produces_negative_or_overlapping_widths() {
    let panes = [
        entry(PaneDescriptor::leading("a"), 0),
        entry(PaneDescriptor::leading("b"), 1),
        entry(PaneDescriptor::trailing("c"), 2),
        entry(PaneDescriptor::bottom("d"), 3),
    ];
    for width in (0..1600).step_by(7) {
        for height in [0., 100., 300., 900.] {
            let result = fit(size(px(width as f32), px(height)), MAIN_MIN_WIDTH, &panes);
            let used: f32 = result.panes[..3].iter().map(|p| p.size.0).sum();
            assert!(used + result.main_width.0 <= width as f32 + 0.001);
            assert!(result.main_width.0 >= 0.0);
            for pane in &result.panes {
                assert!(pane.size.0 >= 0.0);
                if pane.visible {
                    assert!(pane.size >= pane.limits.min);
                    assert!(pane.size <= pane.limits.max + px(0.001));
                }
            }
            let visible = result.panes[..3].iter().filter(|p| p.visible).count();
            if visible > 0 {
                assert!(result.main_width >= px(300.) - px(0.001));
            }
            assert!(result.bottom_height.0 <= height);
        }
    }
}

#[test]
fn takeover_pane_is_hidden_last_and_may_use_main_space() {
    // The takeover pane was opened first, yet the others go first.
    let panes = [
        entry(PaneDescriptor::trailing("expanded").takeover(true), 0),
        entry(PaneDescriptor::leading("sidebar"), 1),
    ];
    let result = fit(size(px(700.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert!(result.panes[0].visible);
    assert!(!result.panes[1].visible);
    assert_eq!(
        result.main_min,
        px(0.),
        "a lone takeover pane lifts the main minimum"
    );
    assert_eq!(result.panes[0].size, px(520.));
    assert_eq!(result.main_width, px(180.));

    // Alone it may use the main region's space down to zero main minimum.
    let result = fit(size(px(500.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert!(result.panes[0].visible && !result.panes[1].visible);
    assert_eq!(result.main_min, px(0.));
    assert_eq!(result.panes[0].size, px(500.));
    assert_eq!(result.main_width, px(0.));

    // Below its own minimum even it is hidden and main takes everything.
    let result = fit(size(px(300.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert!(result.panes.iter().all(|p| !p.visible));
    assert_eq!(result.main_width, px(300.));
}

#[test]
fn leading_width_limit_formula() {
    // min(preferred, viewport - main min - minimums of the other open panes)
    let panes = [
        entry(PaneDescriptor::leading("a").max_size(px(900.)), 0),
        entry(PaneDescriptor::trailing("b"), 1),
    ];
    let result = fit(size(px(1000.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert_eq!(result.panes[0].limits.max, px(1000. - 300. - 360.));
    assert_eq!(result.panes[0].limits.min, px(224.));
    assert_eq!(result.panes[1].limits.max, px(1000. - 300. - 224.));
    // Alone, a preferred width above the available space is limited.
    let panes = [entry(
        PaneDescriptor::leading("a")
            .max_size(px(900.))
            .preferred_size(px(800.)),
        0,
    )];
    let result = fit(size(px(1000.), px(600.)), MAIN_MIN_WIDTH, &panes);
    assert_eq!(result.panes[0].size, px(700.));
}

#[test]
fn bottom_height_limits() {
    let mut bottom = entry(PaneDescriptor::bottom("b"), 0);
    // 55 percent of a 600 pixel viewport is 330.
    bottom.state.size = px(500.);
    let result = fit(size(px(900.), px(600.)), MAIN_MIN_WIDTH, &[bottom.clone()]);
    assert_eq!(result.bottom_height, px(330.));
    assert_eq!(result.panes[0].limits.max, px(330.));
    assert_eq!(result.panes[0].limits.min, px(160.));
    // The minimum wins over a small preferred height.
    bottom.state.size = px(10.);
    let result = fit(size(px(900.), px(600.)), MAIN_MIN_WIDTH, &[bottom.clone()]);
    assert_eq!(result.bottom_height, px(160.));
    // The absolute maximum bounds tall viewports.
    bottom.state.size = px(5000.);
    let result = fit(size(px(900.), px(6000.)), MAIN_MIN_WIDTH, &[bottom.clone()]);
    assert_eq!(result.bottom_height, px(2000.));
    // Never taller than the viewport.
    let result = fit(size(px(900.), px(100.)), MAIN_MIN_WIDTH, &[bottom]);
    assert_eq!(result.bottom_height, px(100.));
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Probe {
    hitbox: RefCell<Option<Hitbox>>,
    downs: Cell<usize>,
    prepaint: Cell<Bounds<Pixels>>,
    paint: Cell<Bounds<Pixels>>,
}

impl Probe {
    fn hitbox(&self) -> Hitbox {
        self.hitbox.borrow().clone().expect("probe was not drawn")
    }
}

fn probe_element(probe: &Rc<Probe>) -> impl IntoElement + use<> {
    let (down, pre, paint) = (probe.clone(), probe.clone(), probe.clone());
    div()
        .size_full()
        .on_mouse_down(MouseButton::Left, move |_, _, _| {
            down.downs.set(down.downs.get() + 1)
        })
        .child(
            canvas(
                move |bounds, window, _| {
                    pre.prepaint.set(bounds);
                    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    *pre.hitbox.borrow_mut() = Some(hitbox.clone());
                    hitbox
                },
                move |bounds, _, _, _| paint.paint.set(bounds),
            )
            .size_full(),
        )
}

struct SurfaceSetup {
    state: MovingSurfaceState,
    route: Rc<Cell<Point<Pixels>>>,
    clear_bottom: bool,
    probe: Rc<Probe>,
    focus: FocusHandle,
}

struct Root {
    state: Entity<PaneGroupState>,
    main: Rc<Probe>,
    leading: Rc<Probe>,
    trailing: Rc<Probe>,
    bottom: Rc<Probe>,
    surface: Option<SurfaceSetup>,
    seam_top_inset: Pixels,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match &self.surface {
            None => probe_element(&self.main).into_any_element(),
            Some(surface) => {
                let mut moving = MovingSurface::new(
                    &surface.state,
                    div()
                        .w(px(200.))
                        .h(px(40.))
                        .track_focus(&surface.focus)
                        .child(probe_element(&surface.probe)),
                )
                .route(surface.route.get());
                if surface.clear_bottom {
                    moving = moving.clear_bottom(self.state.read(cx).metrics());
                }
                div().size_full().child(moving).into_any_element()
            }
        };
        let mut group = pane_group("group", &self.state)
            .seam_top_inset(self.seam_top_inset)
            .main(main);
        for entry in self.state.read(cx).entries() {
            let probe = match entry.descriptor.id.0.as_ref() {
                "leading" => &self.leading,
                "trailing" => &self.trailing,
                "bottom" => &self.bottom,
                other => panic!("unexpected pane {other}"),
            };
            group = group.pane(entry.descriptor.id.clone(), probe_element(probe));
        }
        group
    }
}

struct Harness {
    // Entity handles first: they must drop before the app does.
    root: Entity<Root>,
    state: Entity<PaneGroupState>,
    metrics: PaneMetrics,
    events: Rc<RefCell<Vec<PaneEvent>>>,
    vcx: VisualTestContext,
    cx: TestAppContext,
}

impl Harness {
    fn new(window: Size<Pixels>, descriptors: Vec<PaneDescriptor>) -> Self {
        Self::build(window, descriptors, None, px(0.))
    }

    fn build(
        window_size: Size<Pixels>,
        descriptors: Vec<PaneDescriptor>,
        surface: Option<(bool, Point<Pixels>)>,
        seam_top_inset: Pixels,
    ) -> Self {
        let mut cx = TestAppContext::single();
        let state = cx.new(|_| PaneGroupState::new(descriptors));
        let events = Rc::new(RefCell::new(Vec::new()));
        let log = events.clone();
        cx.update(|cx| {
            cx.subscribe(&state, move |_, event: &PaneEvent, _| {
                log.borrow_mut().push(event.clone())
            })
            .detach();
        });
        let metrics = cx.read(|cx| state.read(cx).metrics());
        let window = cx.open_window(window_size, {
            let state = state.clone();
            move |_, cx| Root {
                state,
                main: Rc::default(),
                leading: Rc::default(),
                trailing: Rc::default(),
                bottom: Rc::default(),
                surface: surface.map(|(clear_bottom, route)| SurfaceSetup {
                    state: MovingSurfaceState::default(),
                    route: Rc::new(Cell::new(route)),
                    clear_bottom,
                    probe: Rc::default(),
                    focus: cx.focus_handle(),
                }),
                seam_top_inset,
            }
        });
        let root = window.root(&mut cx).unwrap();
        let vcx = VisualTestContext::from_window(window.into(), &cx);
        let mut harness = Self {
            cx,
            vcx,
            root,
            state,
            metrics,
            events,
        };
        harness.draw();
        harness
    }

    /// Delivers the next-frame callbacks the platform frame loop would deliver (animating
    /// views notify themselves from them, and an unchanged view is replayed, PRF-001) and
    /// draws one frame.
    fn draw(&mut self) {
        self.vcx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx)
        });
    }

    fn advance(&mut self, millis: u64) {
        self.cx
            .executor()
            .advance_clock(Duration::from_millis(millis));
    }

    fn resize(&mut self, width: f32, height: f32) {
        self.vcx.simulate_resize(size(px(width), px(height)));
        self.draw();
    }

    fn update_state(&mut self, f: impl FnOnce(&mut PaneGroupState, &mut Context<PaneGroupState>)) {
        let state = self.state.clone();
        self.cx.update(|cx| state.update(cx, f));
    }

    fn set_collapsed(&mut self, id: &'static str, collapsed: bool) {
        self.update_state(|state, cx| state.set_collapsed(&id.into(), collapsed, cx));
    }

    fn placement(&self, id: &'static str) -> PanePlacement {
        self.metrics.placement(&id.into()).expect("pane placement")
    }

    fn slot(&self, id: &'static str) -> Bounds<Pixels> {
        self.placement(id).slot
    }

    fn probe<R>(&self, f: impl FnOnce(&Root) -> R) -> R {
        let root = self.root.clone();
        self.cx.read(|cx| f(root.read(cx)))
    }

    fn down(&mut self, x: f32, y: f32, click_count: usize) {
        self.vcx.simulate_event(MouseDownEvent {
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        self.draw();
    }

    fn up(&mut self, x: f32, y: f32) {
        self.vcx.simulate_event(MouseUpEvent {
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
    }

    fn mov(&mut self, x: f32, y: f32, pressed: bool) {
        self.vcx.simulate_event(MouseMoveEvent {
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            pressed_button: pressed.then_some(MouseButton::Left),
        });
    }

    fn hovered(&mut self, hitbox: &Hitbox) -> bool {
        let hitbox = hitbox.clone();
        self.vcx.update(|window, _| hitbox.is_hovered(window))
    }

    fn size_of(&self, id: &'static str) -> Pixels {
        self.cx
            .read(|cx| self.state.read(cx).size(&id.into()).unwrap())
    }
}

fn leading_trailing() -> Vec<PaneDescriptor> {
    vec![
        PaneDescriptor::leading("leading"),
        PaneDescriptor::trailing("trailing").collapsed(true),
    ]
}

// ---------------------------------------------------------------------------
// Group layout, retained state (R1, R7, A6)
// ---------------------------------------------------------------------------

#[test]
fn group_arranges_panes_and_main() {
    let mut h = Harness::new(
        size(px(1200.), px(700.)),
        vec![
            PaneDescriptor::leading("leading"),
            PaneDescriptor::trailing("trailing"),
            PaneDescriptor::bottom("bottom"),
        ],
    );
    h.draw();
    let frame = h.metrics.frame();
    assert_eq!(
        h.slot("leading"),
        Bounds::new(point(px(0.), px(0.)), size(px(256.), px(420.)))
    );
    assert_eq!(
        h.slot("trailing"),
        Bounds::new(point(px(680.), px(0.)), size(px(520.), px(420.)))
    );
    assert_eq!(
        frame.main,
        Bounds::new(point(px(256.), px(0.)), size(px(424.), px(420.)))
    );
    assert_eq!(frame.bottom_reservation, px(280.));
    assert_eq!(
        frame.bottom,
        Bounds::new(point(px(0.), px(420.)), size(px(1200.), px(280.)))
    );
    // Children are laid out at the slot size and painted there.
    assert_eq!(h.probe(|r| r.leading.paint.get()), h.slot("leading"));
    assert_eq!(h.probe(|r| r.main.paint.get()), frame.main);
    assert_eq!(h.probe(|r| r.bottom.paint.get()), frame.bottom);
}

#[test]
fn pane_hidden_by_fitting_keeps_preferences_and_returns() {
    let mut h = Harness::new(
        size(px(1200.), px(700.)),
        vec![
            PaneDescriptor::leading("leading"),
            PaneDescriptor::trailing("trailing"),
        ],
    );
    h.update_state(|state, cx| state.set_size(&"leading".into(), px(300.), cx));
    h.draw();
    assert!(h.placement("leading").visible);
    assert!(h.placement("trailing").visible);

    h.resize(800., 700.);
    let leading = h.placement("leading");
    assert!(!leading.visible && leading.open);
    assert_eq!(leading.slot.size.width, px(0.));
    assert!(h.placement("trailing").visible);
    // Preferences, open flag and order survive.
    assert_eq!(h.size_of("leading"), px(300.));
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        assert_eq!(state.is_collapsed(&"leading".into()), Some(false));
        assert_eq!(state.opened_order(&"leading".into()), Some(0));
    });

    h.resize(1200., 700.);
    let leading = h.placement("leading");
    assert!(leading.visible);
    assert_eq!(leading.slot.size.width, px(300.));
    assert_eq!(leading.slot.origin.x, px(0.));
}

#[test]
fn window_resize_discards_departing_allocation() {
    let mut h = Harness::new(size(px(1000.), px(700.)), leading_trailing());
    h.set_collapsed("trailing", false);
    h.draw();
    h.advance(50);
    h.draw();
    let width = h.slot("trailing").size.width;
    assert!(
        width > px(0.) && width < px(520.),
        "mid-transition: {width:?}"
    );

    // A window-driven change runs no transition: the destination applies at once.
    h.resize(1100., 700.);
    assert_eq!(h.slot("trailing").size.width, px(520.));
    assert!(!h.placement("trailing").closing);

    // Closing mid-flight, then resizing the window, drops the departing slot.
    h.set_collapsed("trailing", true);
    h.draw();
    h.advance(50);
    h.draw();
    assert!(h.placement("trailing").closing);
    h.resize(1000., 700.);
    assert_eq!(h.slot("trailing").size.width, px(0.));
}

#[test]
fn retained_state_is_observable_and_settable() {
    let mut h = Harness::new(size(px(1200.), px(700.)), leading_trailing());
    h.update_state(|state, cx| state.set_size(&"leading".into(), px(5000.), cx));
    assert_eq!(
        h.size_of("leading"),
        px(400.),
        "restored sizes clamp to the pane maximum"
    );
    h.update_state(|state, cx| state.set_size(&"leading".into(), px(300.), cx));
    h.set_collapsed("trailing", false);
    h.set_collapsed("trailing", false);
    h.set_collapsed("trailing", true);
    h.update_state(|state, cx| state.toggle_collapsed(&"trailing".into(), cx));
    h.cx.read(|cx| {
        let state = h.state.read(cx);
        // Opened again last: its order is after the initial opening.
        assert!(
            state.opened_order(&"trailing".into()).unwrap()
                > state.opened_order(&"leading".into()).unwrap()
        );
    });
    assert_eq!(
        *h.events.borrow(),
        vec![
            PaneEvent::Resized {
                pane: "leading".into(),
                size: px(400.)
            },
            PaneEvent::Resized {
                pane: "leading".into(),
                size: px(300.)
            },
            PaneEvent::CollapsedChanged {
                pane: "trailing".into(),
                collapsed: false
            },
            PaneEvent::CollapsedChanged {
                pane: "trailing".into(),
                collapsed: true
            },
            PaneEvent::CollapsedChanged {
                pane: "trailing".into(),
                collapsed: false
            },
        ]
    );
}

// ---------------------------------------------------------------------------
// Slot transitions (R4, A2)
// ---------------------------------------------------------------------------

#[test]
fn rapid_reversal_retargets_from_current_value_without_a_jump() {
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![PaneDescriptor::leading("leading").collapsed(true)],
    );
    assert_eq!(h.slot("leading").size.width, px(0.));

    h.set_collapsed("leading", false);
    h.draw();
    assert_eq!(h.slot("leading").size.width, px(0.), "starts from zero");
    h.advance(50);
    h.draw();
    let opening = h.slot("leading").size.width;
    assert!(opening > px(0.) && opening < px(256.));

    // Reverse mid-flight: the displayed value is the starting point.
    h.set_collapsed("leading", true);
    h.draw();
    assert_eq!(h.slot("leading").size.width, opening, "no jump on close");
    assert!(h.placement("leading").closing);
    h.advance(40);
    h.draw();
    let closing = h.slot("leading").size.width;
    assert!(closing < opening && closing > px(0.));

    // And again.
    h.set_collapsed("leading", false);
    h.draw();
    assert_eq!(h.slot("leading").size.width, closing, "no jump on reopen");
    assert!(!h.placement("leading").closing);
    h.advance(30);
    h.draw();
    assert!(h.slot("leading").size.width > closing);

    // Settles at the destination and then closes completely.
    h.advance(400);
    h.draw();
    assert_eq!(h.slot("leading").size.width, px(256.));
    h.set_collapsed("leading", true);
    h.draw();
    h.advance(400);
    h.draw();
    assert_eq!(h.slot("leading").size.width, px(0.));
    assert!(!h.placement("leading").closing);
}

#[test]
fn reduced_motion_snaps_slot_transitions() {
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![PaneDescriptor::leading("leading").collapsed(true)],
    );
    h.cx.update(|cx| cx.set_reduce_motion(true));
    h.set_collapsed("leading", false);
    h.draw();
    assert_eq!(h.slot("leading").size.width, px(256.));
    h.set_collapsed("leading", true);
    h.draw();
    assert_eq!(h.slot("leading").size.width, px(0.));
    assert!(!h.placement("leading").closing);
}

#[test]
fn slot_transition_uses_the_size_curve() {
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![PaneDescriptor::leading("leading").collapsed(true)],
    );
    h.set_collapsed("leading", false);
    h.draw();
    h.advance(100);
    h.draw();
    let expected = 256. * CubicBezier::new(0., 0., 0.58, 1.).ease(0.5);
    assert!(near(h.slot("leading").size.width, expected));
    h.advance(100);
    h.draw();
    assert_eq!(h.slot("leading").size.width, px(256.));
}

#[test]
fn content_does_not_reflow_during_slot_transitions() {
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![PaneDescriptor::leading("leading").collapsed(true)],
    );
    // Opening from a closed slot: destination width from the first frame.
    h.set_collapsed("leading", false);
    h.draw();
    h.advance(10);
    h.draw();
    assert!(h.slot("leading").size.width < px(256.));
    assert_eq!(h.placement("leading").content_extent, px(256.));
    assert_eq!(h.probe(|r| r.leading.prepaint.get().size.width), px(256.));
    h.advance(60);
    h.draw();
    assert_eq!(h.probe(|r| r.leading.prepaint.get().size.width), px(256.));
    h.advance(300);
    h.draw();

    // Closing keeps the width the content had when closing began.
    h.set_collapsed("leading", true);
    h.draw();
    for _ in 0..4 {
        h.advance(30);
        h.draw();
        assert!(h.placement("leading").closing);
        assert!(h.slot("leading").size.width < px(256.));
        assert_eq!(h.probe(|r| r.leading.prepaint.get().size.width), px(256.));
        // Anchored at the outer edge and clipped by the slot.
        assert_eq!(h.probe(|r| r.leading.prepaint.get().origin.x), px(0.));
    }

    // Reversing between two open sizes eases the layout width.
    h.update_state(|state, cx| state.set_size(&"leading".into(), px(300.), cx));
    h.draw();
    assert_eq!(h.placement("leading").content_extent, px(256.));
    h.set_collapsed("leading", false);
    h.draw();
    assert_eq!(h.placement("leading").content_extent, px(256.));
    h.advance(100);
    h.draw();
    let extent = h.placement("leading").content_extent;
    assert!(extent > px(256.) && extent < px(300.), "eased: {extent:?}");
    h.advance(200);
    h.draw();
    assert_eq!(h.placement("leading").content_extent, px(300.));
    assert_eq!(h.slot("leading").size.width, px(300.));
}

#[test]
fn pane_hidden_by_fitting_closes_from_zero_and_claims_no_space() {
    let mut h = Harness::new(
        size(px(800.), px(700.)),
        vec![
            PaneDescriptor::leading("leading"),
            PaneDescriptor::trailing("trailing"),
        ],
    );
    // The leading pane is hidden by fitting; collapsing it must not run a
    // closing transition or claim any space.
    assert!(!h.placement("leading").visible);
    h.set_collapsed("leading", true);
    h.draw();
    h.advance(50);
    h.draw();
    assert!(!h.placement("leading").closing);
    assert_eq!(h.slot("leading").size.width, px(0.));
    assert_eq!(
        h.metrics.frame().main.size.width,
        px(800. - 520.).max(px(300.))
    );
}

#[test]
fn closing_pane_stays_painted_but_inert() {
    let mut h = Harness::new(size(px(1000.), px(700.)), leading_trailing());
    h.set_collapsed("leading", true);
    h.draw();
    h.advance(50);
    h.draw();
    assert!(h.placement("leading").closing);
    h.mov(20., 100., false);
    h.draw();
    h.mov(20., 100., false);
    let hitbox = h.probe(|r| r.leading.hitbox());
    assert!(!h.hovered(&hitbox), "a closing pane must not hit-test");
    assert_eq!(h.probe(|r| r.leading.paint.get().size.width), px(256.));
    h.advance(300);
    h.draw();
    assert_eq!(h.slot("leading").size.width, px(0.));
}

// ---------------------------------------------------------------------------
// Seams: hitboxes (A3), drag (A2, A4), reset (A2)
// ---------------------------------------------------------------------------

fn side_pane_harness() -> Harness {
    Harness::new(
        size(px(1000.), px(700.)),
        vec![PaneDescriptor::leading("leading")],
    )
}

#[test]
fn seam_consumes_pointer_events_only_within_declared_area() {
    // Horizontal seam at x = 256: 20 pixels wide, 10 on each side.
    for (x, consumed) in [
        (246.5, true),
        (255.9, true),
        (265.5, true),
        (245.5, false),
        (266.5, false),
    ] {
        let mut h = side_pane_harness();
        h.mov(x, 100., false);
        h.down(x, 100., 1);
        h.draw();
        let main_downs = h.probe(|r| r.main.downs.get());
        let leading_downs = h.probe(|r| r.leading.downs.get());
        assert_eq!(
            main_downs + leading_downs == 0,
            consumed,
            "press at x = {x}: main {main_downs}, leading {leading_downs}"
        );
        h.mov(x + 40., 100., true);
        h.draw();
        let moved = h.size_of("leading") != px(256.);
        assert_eq!(moved, consumed, "drag from x = {x}");
        h.up(x + 40., 100.);
    }

    // Bottom seam at y = 420: 10 pixels tall.
    for (y, consumed) in [(415.5, true), (424.5, true), (414.5, false), (425.5, false)] {
        let mut h = Harness::new(
            size(px(1000.), px(700.)),
            vec![PaneDescriptor::bottom("bottom")],
        );
        h.mov(500., y, false);
        h.down(500., y, 1);
        h.draw();
        let downs = h.probe(|r| r.main.downs.get() + r.bottom.downs.get());
        assert_eq!(downs == 0, consumed, "press at y = {y}");
        h.mov(500., y - 40., true);
        h.draw();
        assert_eq!(
            h.size_of("bottom") != px(280.),
            consumed,
            "drag from y = {y}"
        );
        h.up(500., y - 40.);
    }

    // Outside the seam's vertical extent the seam does not exist.
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![
            PaneDescriptor::leading("leading"),
            PaneDescriptor::bottom("bottom"),
        ],
    );
    h.mov(256., 430., false);
    h.down(256., 430., 1);
    assert_eq!(h.probe(|r| r.bottom.downs.get()), 1);
}

#[test]
fn seam_can_be_inset_from_the_top() {
    let mut h = Harness::build(
        size(px(1000.), px(700.)),
        vec![
            PaneDescriptor::leading("leading"),
            PaneDescriptor::trailing("trailing").preferred_size(px(400.)),
        ],
        None,
        px(40.),
    );
    // Press above and below the inset on the leading seam (x = 256).

    let reset = |h: &mut Harness| h.probe(|r| r.main.downs.get() + r.leading.downs.get());
    let base = reset(&mut h);
    h.down(256., 20., 1);
    assert_eq!(reset(&mut h), base + 1, "inside the inset the seam yields");
    h.up(256., 20.);
    let base = reset(&mut h);
    h.down(256., 60., 1);
    assert_eq!(reset(&mut h), base, "below the inset the seam consumes");
    h.up(256., 60.);
    // Mirrored for the trailing seam at x = 600.
    let trailing_base = h.probe(|r| r.main.downs.get() + r.trailing.downs.get());
    h.down(600., 20., 1);
    assert_eq!(
        h.probe(|r| r.main.downs.get() + r.trailing.downs.get()),
        trailing_base + 1
    );
    h.up(600., 20.);
    h.down(600., 60., 1);
    assert_eq!(
        h.probe(|r| r.main.downs.get() + r.trailing.downs.get()),
        trailing_base + 1
    );
    h.up(600., 60.);
}

#[test]
fn seam_drag_updates_layout_painting_and_hit_testing_in_the_same_frame() {
    let mut h = side_pane_harness();
    h.mov(256., 100., false);
    h.down(256., 100., 1);
    h.draw();
    // Pointer-derived size: distance from the outer edge to the pointer.
    for pointer in [300., 333.5, 225., 399.] {
        h.mov(pointer, 100., true);
        // One frame after the event, with no time having passed.
        h.draw();
        assert_eq!(h.size_of("leading"), px(pointer));
        assert_eq!(h.slot("leading").size.width, px(pointer), "layout");
        assert_eq!(
            h.probe(|r| r.leading.prepaint.get().size.width),
            px(pointer)
        );
        assert_eq!(
            h.probe(|r| r.leading.paint.get().size.width),
            px(pointer),
            "painting"
        );
        let main = h.metrics.frame().main;
        assert_eq!(main.origin.x, px(pointer));
        assert_eq!(h.probe(|r| r.main.paint.get().origin.x), px(pointer));
        // Hit testing: the content hitbox and the seam hitbox moved with it.
        let hitbox = h.probe(|r| r.leading.hitbox());
        assert_eq!(hitbox.bounds, h.slot("leading"));
        let seam = h.metrics.frame().seams[0].bounds;
        assert_eq!(seam.origin.x, px(pointer - 10.));
        assert_eq!(seam.size.width, px(20.));
        let captured = h.vcx.update(|window, _| window.captured_hitbox());
        assert!(captured.is_some());
    }
    h.up(399., 100.);
    h.draw();
    // With the drag over, hit testing follows the new slot.
    let hitbox = h.probe(|r| r.leading.hitbox());
    let (inside, outside) = h.vcx.update(|window, _| {
        (
            hitbox.is_hovered_at(point(px(300.), px(200.)), window),
            hitbox.is_hovered_at(point(px(420.), px(200.)), window),
        )
    });
    assert!(inside && !outside);
}

#[test]
fn seam_drag_clamps_pointer_derived_size() {
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![
            PaneDescriptor::leading("leading"),
            PaneDescriptor::trailing("trailing"),
        ],
    );
    let leading_seam = h.slot("leading").right();
    h.mov(leading_seam.0, 100., false);
    h.down(leading_seam.0, 100., 1);
    h.draw();
    // The pane's own maximum is 400, but the trailing pane's minimum leaves
    // only 1000 - 300 - 360 = 340: the trailing pane is squeezed to its
    // minimum instead of overlapping.
    h.mov(900., 100., true);
    h.draw();
    assert_eq!(h.size_of("leading"), px(340.));
    let frame = h.metrics.frame();
    assert_eq!(
        frame.main.size.width + px(340.) + h.slot("trailing").size.width,
        px(1000.)
    );
    assert_eq!(frame.main.size.width, px(300.));
    assert_eq!(h.slot("trailing").size.width, px(360.));
    assert_eq!(
        h.size_of("trailing"),
        px(520.),
        "the squeezed pane keeps its preference"
    );
    // Minimum.
    h.mov(10., 100., true);
    h.draw();
    assert_eq!(h.size_of("leading"), px(224.));
    h.up(10., 100.);
}

#[test]
fn seam_drag_keeps_tracking_across_other_content() {
    let mut h = side_pane_harness();
    assert!(h.probe(|r| r.main.hitbox.borrow().is_some()));
    h.mov(256., 100., false);
    h.down(256., 100., 1);
    h.draw();
    assert!(h.vcx.update(|w, _| w.captured_hitbox().is_some()));

    // Far outside the seam hitbox and over the main content.
    for (x, y) in [(500., 300.), (900., 650.), (300., 20.)] {
        h.mov(x, y, true);
        h.draw();
        h.mov(x, y, true);
        assert!(
            h.vcx.update(|w, _| w.captured_hitbox().is_some()),
            "capture continues"
        );
        let main_hitbox = h.probe(|r| r.main.hitbox());
        assert!(
            !h.hovered(&main_hitbox),
            "other content stays inert at ({x}, {y})"
        );
        assert_eq!(h.probe(|r| r.main.downs.get()), 0);
    }
    assert_eq!(h.size_of("leading"), px(300.));
    h.mov(330., 650., true);
    h.draw();
    assert_eq!(
        h.size_of("leading"),
        px(330.),
        "tracks while far from the seam"
    );

    // Releasing ends the drag and ordinary hit testing resumes.
    h.up(500., 300.);
    h.draw();
    assert!(!h.vcx.update(|w, _| w.captured_hitbox().is_some()));
    h.mov(700., 300., false);
    let main_hitbox = h.probe(|r| r.main.hitbox());
    assert!(h.hovered(&main_hitbox));
    let size_after = h.size_of("leading");
    h.mov(500., 300., true);
    h.draw();
    assert_eq!(
        h.size_of("leading"),
        size_after,
        "no tracking after release"
    );
    // A click after the drag reaches the content again.
    h.down(700., 300., 1);
    assert_eq!(h.probe(|r| r.main.downs.get()), 1);
}

#[test]
fn seam_double_click_resets_to_default_clamped_to_current_limits() {
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![
            PaneDescriptor::leading("leading")
                .default_size(px(380.))
                .preferred_size(px(300.)),
        ],
    );
    let seam = h.slot("leading").right().0;
    assert_eq!(seam, 300.);
    h.mov(seam, 100., false);
    h.down(seam, 100., 1);
    h.up(seam, 100.);
    h.down(seam, 100., 2);
    h.up(seam, 100.);
    h.draw();
    assert_eq!(h.size_of("leading"), px(380.));
    assert_eq!(h.slot("leading").size.width, px(380.));
    assert_eq!(
        h.events.borrow().last(),
        Some(&PaneEvent::Reset {
            pane: "leading".into(),
            default: px(380.),
            size: px(380.)
        })
    );

    // A narrower window limits the pane to viewport - main minimum = 300.
    h.update_state(|state, cx| state.set_size(&"leading".into(), px(250.), cx));
    h.resize(600., 700.);
    let seam = h.slot("leading").right().0;
    assert_eq!(seam, 250.);
    h.mov(seam, 100., false);
    h.down(seam, 100., 2);
    h.up(seam, 100.);
    h.draw();
    assert_eq!(
        h.size_of("leading"),
        px(300.),
        "re-clamped to the current limit"
    );
    assert_eq!(
        h.events.borrow().last(),
        Some(&PaneEvent::Reset {
            pane: "leading".into(),
            default: px(380.),
            size: px(300.)
        })
    );
    // The caller-facing reset applies the same clamp.
    h.update_state(|state, cx| state.set_size(&"leading".into(), px(250.), cx));
    h.draw();
    h.update_state(|state, cx| state.reset_size(&"leading".into(), cx));
    assert_eq!(h.size_of("leading"), px(300.));
}

#[test]
fn bottom_seam_drag_uses_distance_from_bottom_edge() {
    let mut h = Harness::new(
        size(px(1000.), px(700.)),
        vec![PaneDescriptor::bottom("bottom")],
    );
    h.mov(500., 420., false);
    h.down(500., 420., 1);
    h.draw();
    h.mov(500., 400., true);
    h.draw();
    assert_eq!(h.size_of("bottom"), px(300.));
    assert_eq!(h.metrics.bottom_reservation(), px(300.));
    h.mov(500., 100., true);
    h.draw();
    assert_eq!(h.size_of("bottom"), px(385.), "55 percent of 700");
    h.mov(500., 690., true);
    h.draw();
    assert_eq!(h.size_of("bottom"), px(160.));
    h.up(500., 690.);
}

#[test]
fn seam_sets_resize_cursor() {
    let mut h = side_pane_harness();
    h.mov(256., 100., false);
    h.draw();
    let cursor = h
        .vcx
        .update(|window, _| window.rendered_frame.cursor_style(window));
    assert_eq!(cursor, Some(CursorStyle::ResizeLeftRight));
}

// ---------------------------------------------------------------------------
// Moving surface and bottom clearance (R5, R6, A5)
// ---------------------------------------------------------------------------

fn surface_harness(
    clear_bottom: bool,
    route: Point<Pixels>,
    panes: Vec<PaneDescriptor>,
) -> Harness {
    Harness::build(
        size(px(1000.), px(700.)),
        panes,
        Some((clear_bottom, route)),
        px(0.),
    )
}

impl Harness {
    fn surface<R>(&self, f: impl FnOnce(&SurfaceSetup) -> R) -> R {
        self.probe(|root| f(root.surface.as_ref().unwrap()))
    }

    fn set_route(&mut self, route: Point<Pixels>) {
        self.surface(|s| s.route.set(route));
        // The route is read by the view without notifying it; an unchanged view replays its
        // retained output (PRF-001), so the next frame must construct it again.
        self.vcx.update(|window, _| window.refresh());
    }
}

/// Closed form of the critically damped glide (ANM-002 R9) starting at rest.
fn glide_from_rest(target: f32, seconds: f32, duration: f32) -> f32 {
    let w = 12. / duration;
    let d = -target;
    let c = w * d;
    target + (d + c * seconds) * (-w * seconds).exp()
}

#[test]
fn reflow_does_not_restart_route_motion() {
    let mut h = surface_harness(
        false,
        point(px(0.), px(0.)),
        vec![PaneDescriptor::leading("leading").collapsed(true)],
    );
    let state = h.surface(|s| s.state.clone());
    assert_eq!(state.route_retargets(), 0);

    h.set_route(point(px(300.), px(120.)));
    h.draw();
    assert_eq!(state.route_retargets(), 1);
    h.advance(100);
    h.draw();
    let expected = glide_from_rest(300., 0.1, 0.42);
    assert!((state.route_offset().x.0 - expected).abs() < 0.3);

    // Another pane opens: the surface's slot moves right while it travels.
    let origin_before = state.layout_origin().unwrap();
    h.set_collapsed("leading", false);
    h.draw();
    h.advance(40);
    h.draw();
    let origin_after = state.layout_origin().unwrap();
    assert!(origin_after.x > origin_before.x, "the slot was reflowed");
    assert_eq!(
        state.route_retargets(),
        1,
        "reflow never retargets the route"
    );
    let expected = glide_from_rest(300., 0.14, 0.42);
    assert!(
        (state.route_offset().x.0 - expected).abs() < 0.05,
        "route motion unperturbed: {} vs {expected}",
        state.route_offset().x.0
    );
    assert!(
        state.reflow_offset().x < px(0.),
        "reflow compensation is separate"
    );
    assert!(state.is_moving());

    // The displayed position is the sum and the hitbox travels with it.
    let displayed = state.displayed_offset();
    assert!(near(
        displayed.x,
        state.route_offset().x.0 + state.reflow_offset().x.0
    ));
    let probe_bounds = h.surface(|s| s.probe.prepaint.get());
    // Layout rounds element origins to whole device pixels (half a logical
    // pixel at scale 2).
    let layout_tolerance = 0.51;
    assert!(
        (probe_bounds.origin.x.0 - (origin_after.x.0 + displayed.x.0)).abs() < layout_tolerance,
        "probe {probe_bounds:?} layout origin {origin_after:?} displayed {displayed:?}"
    );
    assert!(
        (probe_bounds.origin.y.0 - (origin_after.y.0 + displayed.y.0)).abs() < layout_tolerance
    );
    let hitbox = h.surface(|s| s.probe.hitbox());
    assert_eq!(hitbox.bounds, probe_bounds);

    // Everything settles: the reflow glide decays to nothing, the route lands.
    for _ in 0..40 {
        h.advance(50);
        h.draw();
    }
    assert!(!state.is_moving());
    assert_eq!(state.reflow_offset(), point(px(0.), px(0.)));
    assert_eq!(state.route_offset(), point(px(300.), px(120.)));
    let origin = state.layout_origin().unwrap();
    let settled = h.surface(|s| s.probe.prepaint.get().origin);
    assert!(near(settled.x, origin.x.0 + 300.) && near(settled.y, origin.y.0 + 120.));
}

#[test]
fn route_reversal_keeps_entity_text_and_focus() {
    let mut h = surface_harness(false, point(px(0.), px(0.)), vec![]);
    let focus = h.surface(|s| s.focus.clone());
    h.vcx.update(|window, cx| window.focus(&focus, cx));
    h.draw();
    assert!(h.vcx.update(|window, _| focus.is_focused(window)));

    let state = h.surface(|s| s.state.clone());
    h.set_route(point(px(200.), px(100.)));
    h.draw();
    h.advance(100);
    h.draw();
    let midway = state.route_offset();
    h.set_route(point(px(0.), px(0.)));
    h.draw();
    assert_eq!(
        state.route_offset(),
        midway,
        "retargeted from the displayed value"
    );
    assert_eq!(state.route_retargets(), 2);
    h.advance(60);
    h.draw();
    assert!(state.route_offset().x < midway.x, "reversed");
    assert!(
        h.vcx.update(|window, _| focus.is_focused(window)),
        "focus survives travel"
    );
    for _ in 0..40 {
        h.advance(50);
        h.draw();
    }
    assert_eq!(state.displayed_offset(), point(px(0.), px(0.)));
    assert!(h.vcx.update(|window, _| focus.is_focused(window)));
}

#[test]
fn bottom_clearance_used_by_input_equals_hit_testing_reservation() {
    let mut h = surface_harness(
        true,
        point(px(0.), px(500.)),
        vec![PaneDescriptor::bottom("bottom")],
    );
    let state = h.surface(|s| s.state.clone());
    let check = |h: &Harness, expected_reservation: f32| {
        let reservation = h.metrics.bottom_reservation();
        assert_eq!(reservation, px(expected_reservation));
        assert_eq!(
            state.bottom_clearance(),
            reservation,
            "input uses the measured reservation"
        );
        let group = h.metrics.bounds();
        let bottom_hitbox = h.probe(|r| r.bottom.hitbox());
        assert_eq!(
            bottom_hitbox.bounds.top(),
            group.bottom() - reservation,
            "hit testing uses the same reservation"
        );
        assert_eq!(bottom_hitbox.bounds.size.height, reservation);
        let input = h.surface(|s| s.probe.prepaint.get());
        assert!(
            input.bottom() <= bottom_hitbox.bounds.top() + px(0.01),
            "clears the bottom surface"
        );
    };
    h.draw();
    check(&h, 280.);
    assert!(state.clearance_shift() < px(0.));
    let settled = h.surface(|s| s.probe.prepaint.get());
    assert!(
        near(settled.bottom(), 420.),
        "the input rests on the reservation"
    );

    // Dragging the bottom seam applies in the same frame, with no glide.
    h.mov(500., 420., false);
    h.down(500., 420., 1);
    h.draw();
    h.mov(500., 500., true);
    h.draw();
    check(&h, 200.);
    assert!(near(h.surface(|s| s.probe.prepaint.get()).bottom(), 500.));
    h.mov(500., 300., true);
    h.draw();
    check(&h, 385.);
    assert!(near(
        h.surface(|s| s.probe.prepaint.get()).bottom(),
        700. - 385.
    ));
    h.up(500., 300.);

    // Collapsing the bottom pane releases the clearance as it animates away.
    h.set_collapsed("bottom", true);
    h.draw();
    h.advance(100);
    h.draw();
    assert_eq!(state.bottom_clearance(), h.metrics.bottom_reservation());
    assert!(h.metrics.bottom_reservation() > px(0.));
    h.advance(300);
    h.draw();
    assert_eq!(h.metrics.bottom_reservation(), px(0.));
    assert_eq!(state.bottom_clearance(), px(0.));
}
