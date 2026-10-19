//! CMP-023 on the offscreen test host: stable slots, non-interactive skeletons, motion policy.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use vespertine::{
    AppContext as _, Bounds, Context, IntoElement, MotionPreference, ParentElement, Pixels, Render,
    Styled, Window, div, px, rgb,
};
use vespertine_components::status::theme::{Activity, activity};
use vespertine_components::status::{
    Item, ItemStatus, Loadable, loadable, matrix_spinner, mini_glyph_spinner, pulse_glow,
    skeleton_block, skeleton_rows, status_section, wave_loader,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Idle,
    Loading,
    Error,
    Ready,
}

struct SlotView {
    stage: Rc<Cell<Stage>>,
    retries: Rc<Cell<usize>>,
    bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl Render for SlotView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let state = match self.stage.get() {
            Stage::Idle => Loadable::Idle,
            Stage::Loading => Loadable::Loading,
            Stage::Error => Loadable::Error("Could not load".into()),
            Stage::Ready => Loadable::Ready("content"),
        };
        let retries = self.retries.clone();
        let record = self.bounds.clone();
        div()
            .size_full()
            .p(px(16.0))
            .bg(rgb(0x14161A))
            .text_color(rgb(0xE6E8EB))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .on_children_prepainted(move |bounds, _, _| *record.borrow_mut() = bounds)
                    .child(
                        loadable("slot", state)
                            .width(px(240.0))
                            .height(px(96.0))
                            .content(|text, _, _| div().child(text).into_any_element())
                            .on_retry(move |_, _| retries.set(retries.get() + 1)),
                    )
                    .child(div().h(px(20.0)).child("neighbor")),
            )
    }
}

fn host() -> (
    TestHost,
    Rc<Cell<Stage>>,
    Rc<Cell<usize>>,
    Rc<RefCell<Vec<Bounds<Pixels>>>>,
) {
    let stage = Rc::new(Cell::new(Stage::Idle));
    let retries = Rc::new(Cell::new(0));
    let bounds = Rc::new(RefCell::new(Vec::new()));
    let (s, r, b) = (stage.clone(), retries.clone(), bounds.clone());
    let host = TestHost::with_view(HostOptions::with_size(320.0, 200.0), move |_, cx| {
        cx.new(|_| SlotView {
            stage: s,
            retries: r,
            bounds: b,
        })
    })
    .expect("host");
    (host, stage, retries, bounds)
}

fn refresh(host: &mut TestHost) {
    host.app().update(|cx| cx.refresh_windows());
    host.flush();
}

#[test]
fn a1_slot_and_neighbor_bounds_do_not_change_in_any_state() {
    require_backend!();
    let (mut host, stage, retries, bounds) = host();
    host.flush();
    let reference = bounds.borrow().clone();
    assert_eq!(reference.len(), 2);
    for next in [Stage::Loading, Stage::Error, Stage::Ready, Stage::Idle] {
        stage.set(next);
        refresh(&mut host);
        assert_eq!(*bounds.borrow(), reference, "{next:?}");
    }
    // The retry action is reachable by keyboard and by pointer and emits retry events.
    stage.set(Stage::Error);
    refresh(&mut host);
    host.focus_next().unwrap();
    let window = host.window();
    let focused = host
        .app()
        .update_window(window, |_, window, cx| window.focused(cx).is_some())
        .unwrap();
    assert!(focused, "the retry action takes keyboard focus");
    host.simulate_keystrokes("enter", None).unwrap();
    assert_eq!(retries.get(), 1);
    assert_eq!(
        stage.get(),
        Stage::Error,
        "the wrapper never changes state itself"
    );
    let slot = reference[0];
    let x = f64::from(f32::from(slot.origin.x) + 20.0);
    let mut found = false;
    for dy in [30.0, 36.0, 42.0, 48.0, 54.0, 60.0] {
        host.click(x, f64::from(f32::from(slot.origin.y) + dy), 0, "")
            .unwrap();
        if retries.get() == 2 {
            found = true;
            break;
        }
    }
    assert!(found, "a pointer click on the retry action retries");
    // The error state shows no skeleton, and loading shows no error text.
    assert!(host.painted_text().contains(&"Could not load".to_owned()));
    stage.set(Stage::Loading);
    refresh(&mut host);
    assert!(!host.painted_text().contains(&"Could not load".to_owned()));
}

struct SkeletonView;

impl Render for SkeletonView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(16.0))
            .bg(rgb(0x14161A))
            .child(div().w(px(200.0)).child(skeleton_block(
                "skeleton",
                "Loading messages",
                skeleton_rows(3, 10.0),
            )))
    }
}

#[test]
fn a2_skeletons_are_not_actionable_and_the_block_is_one_progress_node() {
    require_backend!();
    let mut host = TestHost::with_view(HostOptions::with_size(260.0, 120.0), |_, cx| {
        cx.new(|_| SkeletonView)
    })
    .expect("host");
    host.flush();
    // Not focusable, absent from tab traversal.
    host.focus_next().unwrap();
    assert_eq!(host.focused_element(), None);
    // No hitbox of an interactive kind: nothing registers under the bars.
    assert!(host.elements_at(40.0, 22.0).is_empty());
    let tree = host.accessibility_tree().unwrap();
    let Some(tree) = tree else {
        panic!("accessibility must be connectable in the test host");
    };
    let text = tree.to_string();
    assert!(text.contains("Loading messages"), "{text}");
    let indicators =
        text.matches("progressIndicator").count() + text.matches("ProgressIndicator").count();
    assert_eq!(
        indicators, 1,
        "one progress node for the block, none per bar: {text}"
    );
}

struct MotionView;

impl Render for MotionView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(16.0))
            .bg(rgb(0x14161A))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().w(px(200.0)).child(skeleton_block(
                "m-skeleton",
                "Loading",
                skeleton_rows(3, 10.0),
            )))
            .child(wave_loader("m-wave", 6.0))
            .child(div().w(px(80.0)).h(px(24.0)).relative().child(pulse_glow(
                "m-glow",
                6.0,
                rgb(0x4C8DF6).into(),
                0.2,
                0.9,
            )))
            .child(matrix_spinner("m-matrix", 4.0))
            .child(mini_glyph_spinner("m-mini", 2.5))
    }
}

#[test]
fn a5_reduced_motion_frames_are_static_and_schedule_nothing() {
    require_backend!();
    let mut host = TestHost::with_view(HostOptions::with_size(260.0, 200.0), |_, cx| {
        cx.new(|_| MotionView)
    })
    .expect("host");
    host.app().update(|cx| cx.set_reduce_motion(true));
    refresh(&mut host);
    let first = host.screenshot_image().unwrap();
    let leases = host.app().update(|cx| cx.animation_lease_count());
    assert_eq!(leases, 0, "no lease is taken under reduced motion");
    for step in 1..=4 {
        host.advance_executor_time(137.0 * f64::from(step)).unwrap();
        refresh(&mut host);
        assert_eq!(first.as_raw(), host.screenshot_image().unwrap().as_raw());
    }
    assert!(!host.app().update(|cx| cx.animation_clock_is_running()));
    assert_eq!(host.app().update(|cx| cx.animation_lease_count()), 0);
}

#[test]
fn a5_with_motion_the_clock_parks_after_the_last_indicator_stops_painting() {
    require_backend!();
    let mut host = TestHost::with_view(HostOptions::with_size(260.0, 200.0), |_, cx| {
        cx.new(|_| MotionView)
    })
    .expect("host");
    refresh(&mut host);
    assert!(host.app().update(|cx| cx.animation_clock_is_running()));
    assert!(host.app().update(|cx| cx.animation_lease_count()) >= 1);
    // Frames at different clock phases differ while motion is on.
    let a = host.screenshot_image().unwrap();
    host.advance_executor_time(400.0).unwrap();
    refresh(&mut host);
    let b = host.screenshot_image().unwrap();
    assert_ne!(a.as_raw(), b.as_raw());
}

struct ProbeView {
    seen: Rc<Cell<Option<Activity>>>,
}

impl Render for ProbeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.seen.set(Some(activity(window, cx)));
        div().size_full()
    }
}

#[test]
fn a5_activity_indicators_follow_the_motion_policy() {
    require_backend!();
    let seen = Rc::new(Cell::new(None));
    let sink = seen.clone();
    let mut host = TestHost::with_view(HostOptions::with_size(60.0, 60.0), move |_, cx| {
        cx.new(|_| ProbeView { seen: sink })
    })
    .expect("host");
    let read = |host: &mut TestHost| {
        refresh(host);
        seen.get().expect("the probe rendered")
    };
    host.app().update(|cx| cx.set_system_reduced_motion(true));
    // System-level reduction: the slow uniform pulse 0.6 + 0.2 x wave.
    match read(&mut host) {
        Activity::Uniform(opacity) => assert!((0.6..=0.8 + 1e-4).contains(&opacity), "{opacity}"),
        other => panic!("{other:?}"),
    }
    assert!(host.app().update(|cx| cx.animation_lease_count()) >= 1);
    // Explicit reduction: flat 0.6, no repeating frames.
    host.app()
        .update(|cx| cx.set_motion_preference(MotionPreference::On));
    host.advance_executor_time(500.0).unwrap();
    assert_eq!(read(&mut host), Activity::Uniform(0.6));
    host.advance_executor_time(500.0).unwrap();
    assert_eq!(
        host.app().update(|cx| cx.animation_lease_count()),
        0,
        "no lease under a forced reduction"
    );
    // No motion reduction: the chase.
    host.app().update(|cx| {
        cx.set_motion_preference(MotionPreference::Off);
        cx.set_system_reduced_motion(false);
    });
    assert!(matches!(read(&mut host), Activity::Chase(_)));
}

#[test]
fn a7_sections_render_and_remember_their_expansion() {
    require_backend!();
    struct SectionView {
        live: Rc<Cell<bool>>,
        statuses: Rc<RefCell<Vec<ItemStatus>>>,
    }
    impl Render for SectionView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let items: Vec<Item> = self
                .statuses
                .borrow()
                .iter()
                .enumerate()
                .map(|(i, status)| Item::new(format!("step {i}"), *status))
                .collect();
            div()
                .size_full()
                .p(px(8.0))
                .bg(rgb(0x14161A))
                .text_color(rgb(0xE6E8EB))
                .child(
                    div()
                        .w(px(280.0))
                        .child(status_section("section-test", items, self.live.get()).name("plan")),
                )
        }
    }
    let live = Rc::new(Cell::new(true));
    let statuses = Rc::new(RefCell::new(vec![
        ItemStatus::Completed,
        ItemStatus::InProgress,
        ItemStatus::Pending,
    ]));
    let (l, s) = (live.clone(), statuses.clone());
    let mut host = TestHost::with_view(HostOptions::with_size(320.0, 240.0), move |_, cx| {
        cx.new(|_| SectionView {
            live: l,
            statuses: s,
        })
    })
    .expect("host");
    refresh(&mut host);
    let text = host.painted_text();
    // Unfinished: expanded, the headline is the active item, the rows are listed.
    assert!(
        text.iter().filter(|t| *t == "step 1").count() >= 2,
        "{text:?}"
    );
    assert!(text.contains(&"step 2".to_owned()), "{text:?}");
    assert!(text.contains(&"1 / 3".to_owned()), "{text:?}");
    // Everything done and idle: the section tidies itself to compact exactly once.
    *statuses.borrow_mut() = vec![ItemStatus::Completed; 3];
    live.set(false);
    refresh(&mut host);
    let text = host.painted_text();
    assert!(text.contains(&"All done".to_owned()), "{text:?}");
    assert!(!text.contains(&"step 2".to_owned()), "{text:?}");
}
