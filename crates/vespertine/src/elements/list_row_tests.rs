//! Acceptance tests of retained virtual-list rows (PRF-005 A1-A8) for the uniform-height list
//! and the variable-height list. Every scenario runs on both lists; the oracle is the same
//! window with row replay disabled (ordinary virtualization).

use crate::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Element, ElementId, Entity,
    FrameStats, GlobalElementId, InputEvent as _, InspectorElementId, InteractiveElement as _,
    IntoElement, LayoutId, ListAlignment, ListState, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, ScrollDelta,
    ScrollWheelEvent, StatefulInteractiveElement as _, Style, Styled as _, TestAppContext,
    TouchPhase, UniformListScrollHandle, Window, div, list, point, px, size, uniform_list,
};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

const ROW_HEIGHT: f32 = 20.0;
const VIEWPORT: f32 = 160.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Uniform,
    Variable,
}

const KINDS: [Kind; 2] = [Kind::Uniform, Kind::Variable];

struct Model {
    value: u32,
}

type Log = Rc<RefCell<Vec<usize>>>;
type Live = Rc<RefCell<BTreeSet<usize>>>;

/// What the rows report to the tests.
#[derive(Clone, Default)]
struct Env {
    /// Row indices the row callback rendered, in order.
    renders: Log,
    /// Rows whose probe holds an element state right now.
    live: Live,
    /// Rows whose probe created an element state, in order.
    created: Log,
    /// Mouse-down listeners that fired: row, position.
    downs: Rc<RefCell<Vec<(usize, i32, i32)>>>,
}

/// An element with an element state of its own, whose lifetime the tests observe.
struct Probe {
    ix: usize,
    env: Env,
}

struct ProbeState {
    ix: usize,
    live: Live,
}

impl Drop for ProbeState {
    fn drop(&mut self) {
        self.live.borrow_mut().remove(&self.ix);
    }
}

impl IntoElement for Probe {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Probe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = px(4.).into();
        style.size.height = px(4.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let (ix, env) = (self.ix, self.env.clone());
        window.with_element_state::<ProbeState, _>(id.unwrap(), |state, _| {
            let state = state.unwrap_or_else(|| {
                env.live.borrow_mut().insert(ix);
                env.created.borrow_mut().push(ix);
                ProbeState {
                    ix,
                    live: env.live.clone(),
                }
            });
            ((), state)
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
    fn id(&self) -> Option<ElementId> {
        Some(ElementId::named_usize("probe", self.ix))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
}

/// What a row contains beyond its label.
#[derive(Clone, Default)]
struct RowSpec {
    /// Rows hold a probe element (a row with element state).
    probe: bool,
    /// The row that writes `shared` while it is rendered.
    writer: Option<usize>,
    /// The rows that read `shared` while they are rendered.
    readers: Vec<usize>,
}

struct ListView {
    kind: Kind,
    scroll: UniformListScrollHandle,
    state: ListState,
    models: Vec<Entity<Model>>,
    shared: Entity<Model>,
    env: Env,
    spec: RowSpec,
    /// Part of every row's text: a different value is a different render callback.
    variant: u32,
    viewport: f32,
    /// Variable rows are taller than uniform rows when set.
    variable_heights: bool,
}

fn row_element(
    kind: Kind,
    ix: usize,
    models: &[Entity<Model>],
    shared: &Entity<Model>,
    env: &Env,
    spec: &RowSpec,
    variant: u32,
    variable_heights: bool,
    cx: &mut App,
) -> AnyElement {
    env.renders.borrow_mut().push(ix);
    let value = models[ix].read(cx).value;
    if spec.writer == Some(ix) {
        shared.update(cx, |shared, _| shared.value += 1);
    }
    let seen = if spec.readers.contains(&ix) {
        shared.read(cx).value
    } else {
        0
    };
    let height = if kind == Kind::Variable && variable_heights {
        ROW_HEIGHT + 8.0 * (ix % 3) as f32
    } else {
        ROW_HEIGHT
    };
    let downs = env.downs.clone();
    let mut row = div()
        .id(("row", ix))
        .role(accesskit::Role::ListItem)
        .aria_label(format!("row {ix}"))
        .h(px(height))
        .w_full()
        .bg(if ix % 2 == 0 {
            crate::white()
        } else {
            crate::rgb(0xf0f0f0).into()
        })
        .hover(|style| style.bg(crate::rgb(0xd0e0ff)))
        .on_mouse_down(MouseButton::Left, move |event, _, _| {
            downs.borrow_mut().push((
                ix,
                f32::from(event.position.x) as i32,
                f32::from(event.position.y) as i32,
            ));
        })
        .child(format!(
            "row {ix} value {value} seen {seen} variant {variant}"
        ));
    if spec.probe {
        row = row.child(Probe {
            ix,
            env: env.clone(),
        });
    }
    row.into_any_element()
}

impl Render for ListView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (kind, models, shared, env, spec, variant, variable_heights) = (
            self.kind,
            self.models.clone(),
            self.shared.clone(),
            self.env.clone(),
            self.spec.clone(),
            self.variant,
            self.variable_heights,
        );
        let viewport = self.viewport;
        let count = models.len();
        let element = match kind {
            Kind::Uniform => uniform_list("rows", count, move |range, _, cx| {
                range
                    .map(|ix| {
                        row_element(
                            kind,
                            ix,
                            &models,
                            &shared,
                            &env,
                            &spec,
                            variant,
                            variable_heights,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.scroll)
            .h(px(viewport))
            .into_any_element(),
            Kind::Variable => list(self.state.clone(), move |ix, _, cx| {
                row_element(
                    kind,
                    ix,
                    &models,
                    &shared,
                    &env,
                    &spec,
                    variant,
                    variable_heights,
                    cx,
                )
            })
            .id("rows")
            .w_full()
            .h(px(viewport))
            .into_any_element(),
        };
        div().size_full().child(element)
    }
}

struct Fixture {
    kind: Kind,
    models: Vec<Entity<Model>>,
    shared: Entity<Model>,
    spec: RowSpec,
    variable_heights: bool,
    viewport: f32,
}

struct Win {
    handle: AnyWindowHandle,
    root: Option<Entity<ListView>>,
    env: Env,
}

impl Fixture {
    fn new(cx: &mut TestAppContext, kind: Kind, count: usize) -> Self {
        Self {
            kind,
            models: (0..count).map(|_| cx.new(|_| Model { value: 0 })).collect(),
            shared: cx.new(|_| Model { value: 0 }),
            spec: RowSpec::default(),
            variable_heights: true,
            viewport: VIEWPORT,
        }
    }

    fn window(&self, cx: &mut TestAppContext, retained: bool) -> Win {
        let env = Env::default();
        let (kind, models, shared, spec, variable_heights, viewport) = (
            self.kind,
            self.models.clone(),
            self.shared.clone(),
            self.spec.clone(),
            self.variable_heights,
            self.viewport,
        );
        let count = models.len();
        let handle = cx.add_window({
            let env = env.clone();
            move |_, _| ListView {
                kind,
                scroll: UniformListScrollHandle::new(),
                state: ListState::new(count, ListAlignment::Top, px(0.)).measure_all(),
                models,
                shared,
                env,
                spec,
                variant: 0,
                viewport,
                variable_heights,
            }
        });
        let root = handle.root(cx).unwrap();
        let handle: AnyWindowHandle = handle.into();
        cx.update_window(handle, |_, window, _| {
            window.set_retention_enabled(retained)
        })
        .unwrap();
        let win = Win {
            handle,
            root: Some(root),
            env,
        };
        draw(cx, &win);
        draw(cx, &win);
        win
    }
}

fn draw(cx: &mut TestAppContext, win: &Win) {
    cx.update_window(win.handle, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn stats(cx: &mut TestAppContext, win: &Win) -> FrameStats {
    cx.update_window(win.handle, |_, window, _| window.frame_stats())
        .unwrap()
}

fn reset_stats(cx: &mut TestAppContext, win: &Win) {
    cx.update_window(win.handle, |_, window, _| window.reset_frame_stats())
        .unwrap();
}

fn wheel(cx: &mut TestAppContext, win: &Win, dy: f32) {
    cx.update_window(win.handle, |_, window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position: point(px(10.), px(10.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    draw(cx, win);
}

/// Delivers the events to the window through its input dispatch, then draws a frame.
fn send(cx: &mut TestAppContext, win: &Win, events: Vec<crate::PlatformInput>) {
    cx.update_window(win.handle, |_, window, cx| {
        for event in events {
            window.dispatch_event(event, cx);
        }
    })
    .unwrap();
    draw(cx, win);
}

fn move_pointer(cx: &mut TestAppContext, win: &Win, x: f32, y: f32) {
    let moved = MouseMoveEvent {
        modifiers: Modifiers::default(),
        position: point(px(x), px(y)),
        pressed_button: None,
    };
    send(cx, win, vec![moved.to_platform_input()]);
}

fn click(cx: &mut TestAppContext, win: &Win, x: f32, y: f32) {
    let position = point(px(x), px(y));
    let down = MouseDownEvent {
        click_count: 1,
        first_mouse: false,
        modifiers: Modifiers::default(),
        position,
        button: MouseButton::Left,
    };
    let up = MouseUpEvent {
        click_count: 1,
        modifiers: Modifiers::default(),
        position,
        button: MouseButton::Left,
    };
    send(
        cx,
        win,
        vec![down.to_platform_input(), up.to_platform_input()],
    );
}

/// Sets the scroll offset the way a scrollbar does (the same path a thumb drag takes) and
/// notifies the owner, which is what the scrollbar's own handler does.
fn set_offset(cx: &mut TestAppContext, win: &Win, y: f32) {
    win.root.as_ref().unwrap().update(cx, |view, cx| {
        match view.kind {
            Kind::Uniform => view
                .scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(-y))),
            Kind::Variable => view.state.set_offset_from_scrollbar(point(px(0.), px(-y))),
        }
        cx.notify();
    });
    draw(cx, win);
}

fn notify_model(cx: &mut TestAppContext, model: &Entity<Model>) {
    model.update(cx, |model, cx| {
        model.value += 1;
        cx.notify();
    });
}

fn take_renders(win: &Win) -> Vec<usize> {
    std::mem::take(&mut *win.env.renders.borrow_mut())
}

const PROBE_POINTS: [(f32, f32); 6] = [
    (10., 5.),
    (10., 45.),
    (30., 95.),
    (10., 130.),
    (40., 155.),
    (10., 190.),
];

/// Everything that is observable of a frame: primitives, hit tests, element states, listeners.
#[derive(PartialEq, Debug)]
struct Observed {
    primitives: Vec<String>,
    hits: Vec<Vec<Bounds<Pixels>>>,
    live: BTreeSet<usize>,
    downs: Vec<(usize, i32, i32)>,
}

fn observe(cx: &mut TestAppContext, win: &Win) -> Observed {
    let (primitives, hits) = cx
        .update_window(win.handle, |_, window, _| {
            let hits = PROBE_POINTS
                .into_iter()
                .map(|(x, y)| window.hit_test_bounds(point(px(x), px(y))))
                .collect();
            (window.painted_primitives(), hits)
        })
        .unwrap();
    Observed {
        primitives,
        hits,
        live: win.env.live.borrow().clone(),
        downs: win.env.downs.borrow().clone(),
    }
}

/// The y origin and the lightness of the background of a quad, to show what differs.
fn brief(primitive: &str) -> String {
    let find = |key: &str| {
        primitive
            .split_once(key)
            .map(|(_, rest)| {
                rest.chars()
                    .take_while(|c| *c != ',' && *c != ' ' && *c != '}')
                    .collect::<String>()
            })
            .unwrap_or_default()
    };
    format!(
        "{} y={} l={}",
        primitive.split_whitespace().next().unwrap_or(""),
        find("y: "),
        find("l: ")
    )
}

fn assert_same(cx: &mut TestAppContext, retained: &Win, full: &Win, what: &str) {
    let a = observe(cx, retained);
    let b = observe(cx, full);
    if a != b {
        let differing: Vec<String> = a
            .primitives
            .iter()
            .zip(&b.primitives)
            .enumerate()
            .filter(|(_, (x, y))| x != y)
            .map(|(at, (x, y))| format!("#{at}: {} vs {}", brief(x), brief(y)))
            .collect();
        panic!(
            "{what}: the windows differ\nprimitives: {} vs {}, differing: {differing:?}\nhits equal: {}\nlive: {:?} vs {:?}\ndowns equal: {}",
            a.primitives.len(),
            b.primitives.len(),
            a.hits == b.hits,
            a.live,
            b.live,
            a.downs == b.downs,
        );
    }
}

fn both_scroll(cx: &mut TestAppContext, a: &Win, b: &Win, dy: f32) {
    wheel(cx, a, dy);
    wheel(cx, b, dy);
}

/// A1: every frame of a history of wheel scrolling, scrollbar offsets, reversals, resizes,
/// hovers, clicks and model changes, the retained window shows exactly what a window without
/// row replay shows: primitives, hit tests, the coordinates its listeners observe and the
/// lifetime of the element states of its rows.
#[crate::test]
fn a1_oracle_every_frame_matches_ordinary_virtualization(cx: &mut TestAppContext) {
    for kind in KINDS {
        let mut fixture = Fixture::new(cx, kind, 300);
        fixture.spec.probe = true;
        let retained = fixture.window(cx, true);
        let full = fixture.window(cx, false);
        let mut seed = 0x2545_F491_4F6C_DD1Du64 ^ (kind as u64 + 1);
        let mut next = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        let mut log: Vec<String> = Vec::new();
        for step in 0..260 {
            match next(11) {
                0..=3 => {
                    let dy = [-7.5, -23.0, -40.0, 11.0, 3.25, -90.0, 61.0][next(7) as usize];
                    log.push(format!("{step}: wheel {dy}"));
                    both_scroll(cx, &retained, &full, dy);
                }
                4 => {
                    let (x, y) = (next(200) as f32, next(200) as f32);
                    log.push(format!("{step}: move {x} {y}"));
                    move_pointer(cx, &retained, x, y);
                    move_pointer(cx, &full, x, y);
                }
                5 => {
                    let ix = next(300) as usize;
                    log.push(format!("{step}: model {ix}"));
                    notify_model(cx, &fixture.models[ix]);
                }
                6 => {
                    let y = next(5000) as f32;
                    log.push(format!("{step}: scrollbar {y}"));
                    set_offset(cx, &retained, y);
                    set_offset(cx, &full, y);
                }
                7 => {
                    let (x, y) = (next(200) as f32, next(160) as f32);
                    log.push(format!("{step}: click {x} {y}"));
                    click(cx, &retained, x, y);
                    click(cx, &full, x, y);
                }
                8 => {
                    let width = [240.0, 360.0, 480.0, 360.0][next(4) as usize];
                    log.push(format!("{step}: resize {width}"));
                    cx.simulate_window_resize(retained.handle, size(px(width), px(300.)));
                    cx.simulate_window_resize(full.handle, size(px(width), px(300.)));
                }
                _ => {}
            }
            draw(cx, &retained);
            draw(cx, &full);
            assert_same(
                cx,
                &retained,
                &full,
                &format!(
                    "{kind:?} step {step} after {:?}",
                    log.iter().rev().take(6).collect::<Vec<_>>()
                ),
            );
        }
    }
}

/// Scrolling by a few pixels per frame builds only the rows that enter the viewport (three
/// at the most), and replays the rest.
#[crate::test]
fn a1_scrolling_builds_only_the_rows_that_enter(cx: &mut TestAppContext) {
    for kind in KINDS {
        let fixture = Fixture::new(cx, kind, 400);
        let win = fixture.window(cx, true);
        for _ in 0..8 {
            wheel(cx, &win, -7.0);
        }
        reset_stats(cx, &win);
        take_renders(&win);
        let mut worst = 0;
        for _ in 0..60 {
            let before = stats(cx, &win).rows_built;
            wheel(cx, &win, -7.0);
            worst = worst.max(stats(cx, &win).rows_built - before);
        }
        let totals = stats(cx, &win);
        assert!(
            worst <= 3,
            "{kind:?}: a 7 px scroll step built {worst} rows ({:?})",
            take_renders(&win)
        );
        assert!(
            totals.rows_replayed > totals.rows_built * 4,
            "{kind:?}: {} rows replayed against {} built",
            totals.rows_replayed,
            totals.rows_built
        );
    }
}

/// A3: one row's model changes (exactly that row rebuilds); the row callback is replaced
/// without a change of the item count (the visible rows rebuild, the other retained rows are
/// suspect and rebuild when shown); a row is inserted (the content rebuilds).
#[crate::test]
fn a3_exactly_the_required_rows_invalidate(cx: &mut TestAppContext) {
    for kind in KINDS {
        let mut fixture = Fixture::new(cx, kind, 200);
        fixture.variable_heights = false;
        let retained = fixture.window(cx, true);
        let full = fixture.window(cx, false);
        // Scroll into the middle so rows are retained on both sides.
        for _ in 0..30 {
            both_scroll(cx, &retained, &full, -40.0);
        }
        for _ in 0..8 {
            draw(cx, &retained);
        }
        take_renders(&retained);
        assert_same(cx, &retained, &full, "after scrolling");

        // 1. One model changes.
        reset_stats(cx, &retained);
        notify_model(cx, &fixture.models[63]);
        draw(cx, &retained);
        draw(cx, &full);
        assert_eq!(
            take_renders(&retained),
            vec![63],
            "{kind:?}: a model change rebuilds exactly its row"
        );
        assert_eq!(stats(cx, &retained).rows_built, 1, "{kind:?}");
        assert_same(cx, &retained, &full, "after the model change");

        // 2. The row callback is replaced; the item count stays.
        let visible = {
            take_renders(&retained);
            take_renders(&full);
            retained.root.as_ref().unwrap().update(cx, |view, cx| {
                view.variant += 1;
                cx.notify();
            });
            full.root.as_ref().unwrap().update(cx, |view, cx| {
                view.variant += 1;
                cx.notify();
            });
            draw(cx, &retained);
            draw(cx, &full);
            let mut built = take_renders(&retained);
            built.retain(|ix| *ix != 0);
            built.sort_unstable();
            built.dedup();
            let mut ordinary = take_renders(&full);
            ordinary.retain(|ix| *ix != 0);
            ordinary.sort_unstable();
            ordinary.dedup();
            assert_eq!(
                built, ordinary,
                "{kind:?}: a replaced callback rebuilds the visible rows and only those"
            );
            built
        };
        assert_same(cx, &retained, &full, "after the callback was replaced");
        // Scroll by half a viewport: rows that were only retained are suspect and rebuild when
        // they are shown, so nothing shows the old callback.
        for _ in 0..4 {
            both_scroll(cx, &retained, &full, -40.0);
            assert_same(cx, &retained, &full, "scrolling to suspect rows");
        }
        let rebuilt: BTreeSet<usize> = take_renders(&retained).into_iter().collect();
        assert!(
            rebuilt.iter().all(|ix| !visible.contains(ix) || *ix == 0),
            "{kind:?}: rows that were rebuilt already are not rebuilt again ({rebuilt:?})"
        );
        assert!(!rebuilt.is_empty(), "{kind:?}: suspect rows were rebuilt");

        // 3. A row is inserted. The notification makes the test app draw the dirty windows at
        // once, so the logs are emptied before it.
        take_renders(&retained);
        for win in [&retained, &full] {
            win.root.as_ref().unwrap().update(cx, |view, cx| {
                let model = cx.new(|_| Model { value: 0 });
                view.models.insert(3, model);
                if view.kind == Kind::Variable {
                    view.state.splice(3..3, 1);
                }
                cx.notify();
            });
        }
        draw(cx, &retained);
        draw(cx, &full);
        let mut built = take_renders(&retained);
        built.retain(|ix| *ix != 0);
        built.sort_unstable();
        built.dedup();
        assert!(
            built.len() >= 7,
            "{kind:?}: an item-count change rebuilds the visible content ({built:?})"
        );
        assert_same(cx, &retained, &full, "after the row was inserted");
    }
}

/// A3 (nested views): a view nested in a row that is notified rebuilds its row.
#[crate::test]
fn a3_a_nested_view_notification_rebuilds_its_row(cx: &mut TestAppContext) {
    struct Nested {
        value: u32,
    }
    impl Render for Nested {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .h(px(10.))
                .bg(crate::rgb(0x102030 + 0x101010 * self.value))
                .child(format!("nested {}", self.value))
        }
    }
    struct Host {
        nested: Vec<Entity<Nested>>,
        renders: Log,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let nested = self.nested.clone();
            let renders = self.renders.clone();
            div().size_full().child(
                uniform_list("rows", nested.len(), move |range, _, _| {
                    range
                        .map(|ix| {
                            renders.borrow_mut().push(ix);
                            div()
                                .id(("row", ix))
                                .h(px(ROW_HEIGHT))
                                .child(nested[ix].clone())
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                })
                .h(px(VIEWPORT)),
            )
        }
    }
    let nested: Vec<Entity<Nested>> = (0..50).map(|_| cx.new(|_| Nested { value: 0 })).collect();
    let open = |cx: &mut TestAppContext, retained: bool| {
        let renders: Log = Rc::default();
        let handle = cx.add_window({
            let nested = nested.clone();
            let renders = renders.clone();
            move |_, _| Host { nested, renders }
        });
        let window: AnyWindowHandle = handle.into();
        cx.update_window(window, |_, window, _| {
            window.set_retention_enabled(retained)
        })
        .unwrap();
        let win = Win {
            handle: window,
            root: None,
            env: Env::default(),
        };
        draw(cx, &win);
        draw(cx, &win);
        (win, renders)
    };
    let (retained, renders) = open(cx, true);
    let (full, _) = open(cx, false);
    renders.borrow_mut().clear();
    nested[3].update(cx, |nested, cx| {
        nested.value += 1;
        cx.notify();
    });
    draw(cx, &retained);
    draw(cx, &full);
    let built: Vec<usize> = renders
        .borrow()
        .iter()
        .copied()
        .filter(|ix| *ix != 0)
        .collect();
    assert_eq!(
        built,
        vec![3],
        "the row of the notified nested view rebuilds"
    );
    assert_same(cx, &retained, &full, "after the nested view was notified");
}

/// A4: a scrollbar round trip within 240 frames adopts the layout nodes the rows kept (no node
/// is created and no line is shaped again), and the hit tests after the trip match ordinary
/// virtualization.
#[crate::test]
fn a4_scrollbar_round_trip_reuses_layout_nodes_and_keeps_hit_tests(cx: &mut TestAppContext) {
    for kind in KINDS {
        let mut fixture = Fixture::new(cx, kind, 300);
        fixture.spec.probe = true;
        let retained = fixture.window(cx, true);
        let full = fixture.window(cx, false);
        set_offset(cx, &retained, 300.0);
        set_offset(cx, &full, 300.0);
        // Far away, then a few frames later back to the rows that were left.
        set_offset(cx, &retained, 3000.0);
        set_offset(cx, &full, 3000.0);
        for _ in 0..10 {
            draw(cx, &retained);
        }
        assert_same(cx, &retained, &full, "far away");
        reset_stats(cx, &retained);
        reset_stats(cx, &full);
        set_offset(cx, &retained, 300.0);
        set_offset(cx, &full, 300.0);
        let back = stats(cx, &retained);
        let ordinary = stats(cx, &full);
        assert_eq!(
            back.nodes_created, 0,
            "{kind:?}: the rows that return adopt their old layout nodes ({back:?})"
        );
        assert_eq!(back.lines_shaped, 0, "{kind:?}: no text is shaped again");
        assert!(
            ordinary.nodes_created > 0,
            "{kind:?}: ordinary virtualization creates the nodes again ({ordinary:?})"
        );
        assert_same(cx, &retained, &full, "after the round trip");
        for (x, y) in [(10., 15.), (20., 75.), (30., 140.)] {
            click(cx, &retained, x, y);
            click(cx, &full, x, y);
        }
        assert_same(cx, &retained, &full, "clicks after the round trip");
        assert_eq!(
            retained.env.downs.borrow().len(),
            3,
            "{kind:?}: every click reached a row"
        );
    }
}

/// A5: a row that leaves the viewport drops its element state and reconstructs it before it
/// is shown again; a row's own render-time write does not invalidate the row, a write read by
/// another row does.
#[crate::test]
fn a5_element_state_lifetime_and_render_time_writes(cx: &mut TestAppContext) {
    for kind in KINDS {
        let mut fixture = Fixture::new(cx, kind, 120);
        fixture.spec.probe = true;
        fixture.variable_heights = false;
        let retained = fixture.window(cx, true);
        let full = fixture.window(cx, false);
        assert!(retained.env.live.borrow().contains(&0), "row 0 has state");
        assert_eq!(
            retained
                .env
                .created
                .borrow()
                .iter()
                .filter(|ix| **ix == 0)
                .count(),
            1
        );
        // Row 0 leaves the viewport; its state goes with it although the row stays retained.
        for _ in 0..4 {
            both_scroll(cx, &retained, &full, -60.0);
        }
        draw(cx, &retained);
        draw(cx, &retained);
        assert!(
            !retained.env.live.borrow().contains(&0),
            "{kind:?}: row 0 dropped its state"
        );
        assert_same(cx, &retained, &full, "after leaving");
        // And back: the state is constructed again before the row is shown.
        for _ in 0..4 {
            both_scroll(cx, &retained, &full, 60.0);
        }
        assert!(retained.env.live.borrow().contains(&0));
        assert!(
            retained
                .env
                .created
                .borrow()
                .iter()
                .filter(|ix| **ix == 0)
                .count()
                >= 2,
            "{kind:?}: the state of row 0 was constructed again before the row was shown again"
        );
        assert_same(cx, &retained, &full, "after returning");

        // Render-time writes: row 2 writes `shared` and reads it; row 4 reads it.
        let mut fixture = Fixture::new(cx, kind, 120);
        fixture.variable_heights = false;
        fixture.spec.writer = Some(2);
        fixture.spec.readers = vec![2, 4];
        let win = fixture.window(cx, true);
        for _ in 0..4 {
            draw(cx, &win);
        }
        take_renders(&win);
        // The notification makes the test app draw the dirty window at once: that frame
        // rebuilds the notified row.
        notify_model(cx, &fixture.models[2]);
        let notified_frame = take_renders(&win);
        assert!(
            notified_frame.contains(&2),
            "{kind:?}: the notified row rebuilds ({notified_frame:?})"
        );
        // Row 2 wrote `shared` while it was built: the row that reads it is invalidated, in
        // that frame (it comes after row 2) or in the next one, and rebuilds exactly once.
        draw(cx, &win);
        let next_frame = take_renders(&win);
        let row_4 = notified_frame
            .iter()
            .chain(&next_frame)
            .filter(|ix| **ix == 4)
            .count();
        assert_eq!(
            row_4, 1,
            "{kind:?}: the write of row 2 invalidates the row that reads it ({notified_frame:?} {next_frame:?})"
        );
        assert!(
            !next_frame.contains(&2),
            "{kind:?}: a row is not invalidated by its own render-time write ({next_frame:?})"
        );
        draw(cx, &win);
        draw(cx, &win);
        assert!(
            take_renders(&win).iter().all(|ix| *ix == 0),
            "{kind:?}: nothing keeps invalidating"
        );
    }
}

/// A6: visible missing rows are filled at once, beyond the budget; overscan filling stops at
/// 0.75 viewport heights per frame and splits the remainder three to one toward the scroll
/// direction.
#[crate::test]
fn a6_extension_budget_fills_visible_rows_first_and_splits_the_remainder(cx: &mut TestAppContext) {
    for kind in KINDS {
        let mut fixture = Fixture::new(cx, kind, 400);
        fixture.variable_heights = false;
        let win = fixture.window(cx, true);
        // The first frames build the whole retained region.
        let initial = take_renders(&win);
        assert!(
            initial.len() >= 20,
            "{kind:?}: the initial construction fills the overscan ({} rows)",
            initial.len()
        );
        // The pointer rests where the wheel events are dispatched (and hover is not suppressed
        // by them), so that the arrival of the pointer does not change a row's hover state in
        // the middle of the measurement.
        cx.update_window(win.handle, |_, window, _| {
            window.set_hover_suppression_enabled(false)
        })
        .unwrap();
        move_pointer(cx, &win, 10.0, 10.0);
        take_renders(&win);
        // Jump far away: every visible row is missing and is built in the same frame, more
        // than the budget of 6 rows; nothing else is built in that frame.
        set_offset(cx, &win, 2000.0);
        let mut jump = take_renders(&win);
        jump.retain(|ix| *ix != 0);
        jump.sort_unstable();
        jump.dedup();
        assert_eq!(
            jump,
            (100..108).collect::<Vec<_>>(),
            "{kind:?}: the visible rows are built at once, the overscan waits"
        );
        // The frames after fill the overscan within the budget: the row that enters at the
        // bottom, 4 more toward the direction of the scroll (below) and 1 against it.
        wheel(cx, &win, -1.0);
        let mut filled = take_renders(&win);
        filled.retain(|ix| *ix != 0);
        filled.sort_unstable();
        assert_eq!(
            filled,
            vec![99, 108, 109, 110, 111, 112],
            "{kind:?}: a frame spends 0.75 viewport heights, three quarters toward the scroll"
        );
        // And it keeps going until the region is complete, never more than the budget.
        for _ in 0..12 {
            wheel(cx, &win, -1.0);
            let mut more = take_renders(&win);
            more.retain(|ix| *ix != 0);
            assert!(
                more.len() <= 6,
                "{kind:?}: at most the budget per frame ({more:?})"
            );
        }
    }
}

/// A7: row replay is bypassed for a flipped uniform list and while accessibility is active;
/// the accessibility tree then holds the visible rows only.
#[crate::test]
fn a7_flipped_lists_and_accessibility_bypass_row_replay(cx: &mut TestAppContext) {
    use accesskit::Role;
    // A flipped uniform list builds every frame and retains nothing.
    struct Flipped {
        scroll: UniformListScrollHandle,
    }
    impl Render for Flipped {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                uniform_list("rows", 100, |range, _, _| {
                    range
                        .map(|ix| {
                            div()
                                .id(("row", ix))
                                .h(px(ROW_HEIGHT))
                                .child(format!("row {ix}"))
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(&self.scroll)
                .y_flipped(true)
                .h(px(VIEWPORT)),
            )
        }
    }
    let handle = cx.add_window(|_, _| Flipped {
        scroll: UniformListScrollHandle::new(),
    });
    let window: AnyWindowHandle = handle.into();
    let flipped = Win {
        handle: window,
        root: None,
        env: Env::default(),
    };
    draw(cx, &flipped);
    reset_stats(cx, &flipped);
    for _ in 0..4 {
        handle
            .update(cx, |view, _, cx| {
                let _ = &view.scroll;
                cx.notify();
            })
            .unwrap();
        draw(cx, &flipped);
    }
    let totals = stats(cx, &flipped);
    assert_eq!(
        (totals.rows_built, totals.rows_replayed),
        (0, 0),
        "a flipped uniform list bypasses row replay"
    );

    // Accessibility: the rows carry labels; with accessibility active only the visible ones
    // reach the tree, and rows are neither built nor replayed.
    for kind in KINDS {
        let mut fixture = Fixture::new(cx, kind, 100);
        fixture.variable_heights = false;
        let win = fixture.window(cx, true);
        draw(cx, &win);
        reset_stats(cx, &win);
        cx.update_window(win.handle, |_, window, _| {
            window.set_a11y_active_for_tests(true)
        })
        .unwrap();
        for _ in 0..4 {
            wheel(cx, &win, -20.0);
        }
        let totals = stats(cx, &win);
        assert_eq!(
            (totals.rows_built, totals.rows_replayed),
            (0, 0),
            "{kind:?}: no row replay while accessibility is active"
        );
        let labels = cx
            .update_window(win.handle, |_, window, _| {
                let update = window
                    .platform_window
                    .as_test()
                    .unwrap()
                    .last_accessibility_update()
                    .unwrap();
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| node.label().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap();
        let _ = Role::Group;
        assert!(
            labels
                .iter()
                .all(|label| !label.contains("row ") || visible_label(label)),
            "{kind:?}: only visible rows are in the tree: {labels:?}"
        );
    }

    /// Rows the viewport shows at offset 80 are rows 4..=12 (the tree labels rows by value).
    fn visible_label(label: &str) -> bool {
        let Some(ix) = label
            .strip_prefix("row ")
            .and_then(|rest| rest.split(' ').next())
            .and_then(|ix| ix.parse::<usize>().ok())
        else {
            return true;
        };
        (4..=12).contains(&ix)
    }
}

/// A8: moving content under a stationary pointer rebuilds the row that loses hover and the row
/// that gains it, and the rows that enter the viewport; the others replay.
#[crate::test]
fn a8_hover_is_predicted_so_only_the_changing_rows_rebuild(cx: &mut TestAppContext) {
    for kind in KINDS {
        let mut fixture = Fixture::new(cx, kind, 200);
        fixture.variable_heights = false;
        let retained = fixture.window(cx, true);
        let full = fixture.window(cx, false);
        // The pointer rests on row 3 (y 60..80).
        move_pointer(cx, &retained, 20.0, 70.0);
        move_pointer(cx, &full, 20.0, 70.0);
        draw(cx, &retained);
        take_renders(&retained);
        reset_stats(cx, &retained);
        // The content moves up by one row through the scrollbar path; the pointer stays.
        set_offset(cx, &retained, 20.0);
        set_offset(cx, &full, 20.0);
        let mut built = take_renders(&retained);
        built.retain(|ix| *ix != 0 || kind == Kind::Variable);
        built.sort_unstable();
        built.dedup();
        // Row 3 loses hover, row 4 gains it; the row that enters at the bottom (row 8 or 9)
        // is built because it comes from the overscan.
        assert!(
            built.contains(&3) && built.contains(&4),
            "{kind:?}: {built:?}"
        );
        assert!(
            built.iter().all(|ix| (3..=4).contains(ix) || *ix >= 8),
            "{kind:?}: no other visible row rebuilds ({built:?})"
        );
        assert_same(
            cx,
            &retained,
            &full,
            "after the content moved under the pointer",
        );
    }
}

/// Rows keep their output through frames the pointer and the scroll position do not touch.
#[crate::test]
fn unchanged_frames_rebuild_nothing(cx: &mut TestAppContext) {
    for kind in KINDS {
        let fixture = Fixture::new(cx, kind, 100);
        let win = fixture.window(cx, true);
        draw(cx, &win);
        take_renders(&win);
        reset_stats(cx, &win);
        for _ in 0..5 {
            draw(cx, &win);
        }
        assert_eq!(stats(cx, &win).rows_built, 0, "{kind:?}");
        assert!(take_renders(&win).is_empty(), "{kind:?}");
    }
}
