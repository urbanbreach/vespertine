//! Tests of adaptive tiled scroll-content reuse (PRF-004).

use crate::{
    AnyWindowHandle, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, PathBuilder, Pixels, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase, Window, canvas, div,
    point, px,
};
use std::{cell::Cell, cell::RefCell, rc::Rc};

pub(crate) const ROW_HEIGHT: f32 = 40.0;
pub(crate) const VIEWPORT: (f32, f32) = (300.0, 200.0);

pub(crate) struct Model {
    pub value: u32,
}

pub(crate) type Log = Rc<RefCell<Vec<String>>>;

pub(crate) struct Page {
    pub rows: usize,
    pub model: Entity<Model>,
    pub log: Log,
    pub renders: Rc<Cell<usize>>,
    /// Read the exact scroll offset while rendering.
    pub handle: Option<ScrollHandle>,
    pub read_offset: bool,
    /// Rows paint a path (an overlay primitive of the layer).
    pub paths: bool,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        if self.read_offset
            && let Some(handle) = &self.handle
        {
            let _ = handle.offset();
        }
        let value = self.model.read(cx).value;
        let paths = self.paths;
        let log = self.log.clone();
        let mut scroller = div()
            .id("scroller")
            .w(px(VIEWPORT.0))
            .h(px(VIEWPORT.1))
            .bg(crate::white())
            .overflow_y_scroll();
        if let Some(handle) = &self.handle {
            scroller = scroller.track_scroll(handle);
        }
        div()
            .size_full()
            .bg(crate::rgb(0xeeeeee))
            .child(scroller.children((0..self.rows).map(move |ix| {
                let log = log.clone();
                div()
                    .id(("row", ix))
                    .relative()
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .bg(if ix.is_multiple_of(2) {
                        crate::white()
                    } else {
                        crate::rgb(0xf7f7f7).into()
                    })
                    .hover(|style| style.bg(crate::rgb(0xd8e8ff)))
                    .child(format!("Item {ix} v{}", value + ix as u32 % 3))
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |bounds, _, window, _| {
                                if paths {
                                    let mut stroke = PathBuilder::stroke(px(1.));
                                    stroke.move_to(point(
                                        bounds.origin.x + px(150.),
                                        bounds.origin.y + px(20.),
                                    ));
                                    stroke.line_to(point(
                                        bounds.origin.x + px(160.),
                                        bounds.origin.y + px(10.),
                                    ));
                                    stroke.line_to(point(
                                        bounds.origin.x + px(170.),
                                        bounds.origin.y + px(25.),
                                    ));
                                    window.paint_path(
                                        stroke.build().expect("polyline"),
                                        crate::rgb(0x2868dc),
                                    );
                                }
                                // Listeners observe the bounds captured when the content was
                                // painted.
                                window.on_mouse_event(move |event: &MouseDownEvent, _, _, _| {
                                    if bounds.contains(&event.position) {
                                        log.borrow_mut().push(format!(
                                            "row {ix} at {:?} click {:?}",
                                            bounds.origin, event.position
                                        ));
                                    }
                                });
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
            })))
    }
}

pub(crate) struct Fixture {
    pub handle: AnyWindowHandle,
    pub log: Log,
    pub renders: Rc<Cell<usize>>,
    pub model: Entity<Model>,
}

impl Fixture {
    /// A window showing a page of `rows` rows; `layers` selects whether scroll layers are on.
    pub(crate) fn new(
        cx: &mut TestAppContext,
        rows: usize,
        layers: bool,
        paths: bool,
        read_offset: bool,
    ) -> Self {
        let log: Log = Rc::default();
        let renders = Rc::new(Cell::new(0));
        let model = cx.new(|_| Model { value: 0 });
        let scroll = ScrollHandle::new();
        let handle = cx.add_window({
            let log = log.clone();
            let renders = renders.clone();
            let model = model.clone();
            move |_, _| Page {
                rows,
                model,
                log,
                renders,
                handle: read_offset.then_some(scroll),
                read_offset,
                paths,
            }
        });
        let window: AnyWindowHandle = handle.into();
        cx.update_window(window, |_, window, _| {
            window.set_scroll_layers_enabled(layers)
        })
        .unwrap();
        let fixture = Self {
            handle: window,
            log,
            renders,
            model,
        };
        fixture.draw(cx);
        fixture.draw(cx);
        fixture
    }

    pub(crate) fn draw(&self, cx: &mut TestAppContext) {
        cx.update_window(self.handle, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    pub(crate) fn event(&self, cx: &mut TestAppContext, event: impl crate::InputEvent) {
        cx.update_window(self.handle, |_, window, cx| {
            window.dispatch_event(event.to_platform_input(), cx);
        })
        .unwrap();
    }

    pub(crate) fn wheel(&self, cx: &mut TestAppContext, dy: f32) {
        self.event(
            cx,
            ScrollWheelEvent {
                position: point(px(50.), px(50.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            },
        );
    }

    pub(crate) fn move_pointer(&self, cx: &mut TestAppContext, x: f32, y: f32) {
        self.event(
            cx,
            MouseMoveEvent {
                position: point(px(x), px(y)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            },
        );
    }

    pub(crate) fn click(&self, cx: &mut TestAppContext, x: f32, y: f32) {
        self.event(
            cx,
            MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(x), px(y)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            },
        );
        self.event(
            cx,
            MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(x), px(y)),
                modifiers: Modifiers::default(),
                click_count: 1,
            },
        );
    }

    pub(crate) fn key(&self, cx: &mut TestAppContext) {
        self.event(
            cx,
            KeyDownEvent {
                keystroke: Keystroke::parse("a").unwrap(),
                is_held: false,
                prefer_character_input: false,
            },
        );
    }

    pub(crate) fn write(&self, cx: &mut TestAppContext) {
        self.model.update(cx, |model, cx| {
            model.value += 1;
            cx.notify();
        });
    }

    pub(crate) fn content(
        &self,
        cx: &mut TestAppContext,
    ) -> Vec<(String, crate::Bounds<crate::ScaledPixels>)> {
        cx.update_window(self.handle, |_, window, _| window.painted_content())
            .unwrap()
    }

    pub(crate) fn hits(&self, cx: &mut TestAppContext) -> Vec<Vec<crate::Bounds<Pixels>>> {
        cx.update_window(self.handle, |_, window, _| {
            [
                (10., 5.),
                (40., 45.),
                (100., 95.),
                (299., 199.),
                (160., 100.),
            ]
            .into_iter()
            .map(|(x, y)| window.hit_test_bounds(point(px(x), px(y))))
            .collect()
        })
        .unwrap()
    }

    pub(crate) fn stats(&self, cx: &mut TestAppContext) -> crate::FrameStats {
        cx.update_window(self.handle, |_, window, _| window.frame_stats())
            .unwrap()
    }

    pub(crate) fn layers(&self, cx: &mut TestAppContext) -> usize {
        cx.update_window(self.handle, |_, window, _| window.scroll_layer_count())
            .unwrap()
    }
}

/// A1: after random fractional wheel input, hovers, clicks, keys and content writes, the
/// expanded scene, the hit tests and the positions the listeners observe of a window with
/// scroll layers equal those of a window that draws directly.
#[crate::test]
fn a1_composited_scenes_match_direct_scenes_after_random_input(cx: &mut TestAppContext) {
    for paths in [false, true] {
        let layered = Fixture::new(cx, 60, true, paths, false);
        let direct = Fixture::new(cx, 60, false, paths, false);
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        let mut history: Vec<String> = Vec::new();
        for step in 0..110 {
            match next(14) {
                0..=8 => {
                    let dy = [-0.9, -7.3, -2.4, 13.1, -41.7, 0.3, -3.5][next(7) as usize];
                    // A run of consecutive frames of scrolling, as a wheel produces.
                    for _ in 0..2 + next(12) {
                        history.push(format!("{step}: wheel {dy}"));
                        layered.wheel(cx, dy);
                        direct.wheel(cx, dy);
                        assert_agree(cx, &direct, &layered, step, &history);
                    }
                    continue;
                }
                9 => {
                    let (x, y) = (next(300) as f32, next(200) as f32);
                    history.push(format!("{step}: move {x} {y}"));
                    layered.move_pointer(cx, x, y);
                    direct.move_pointer(cx, x, y);
                }
                10 => {
                    let (x, y) = (next(300) as f32, next(200) as f32);
                    history.push(format!("{step}: click {x} {y}"));
                    layered.click(cx, x, y);
                    direct.click(cx, x, y);
                }
                11 => {
                    history.push(format!("{step}: key"));
                    layered.key(cx);
                    direct.key(cx);
                }
                12 => {
                    history.push(format!("{step}: write"));
                    layered.write(cx);
                    direct.write(cx);
                }
                _ => {}
            }
            assert_agree(cx, &direct, &layered, step, &history);
        }
        let stats = layered.stats(cx);
        assert!(
            stats.layer_composites > 10 && stats.layer_repaints > 1,
            "the history exercised the layer: {stats:?}"
        );
    }
}

/// R2: scrolls the owner explains by recorded scrolls are composited without rebuilding the
/// owner or the content.
#[crate::test]
fn composited_wheel_frames_neither_render_the_owner_nor_rebuild_the_content(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(cx, 120, true, false, false);
    for _ in 0..2 {
        fixture.wheel(cx, -3.7);
    }
    let renders = fixture.renders.get();
    let promoted = fixture.stats(cx);
    for _ in 0..12 {
        fixture.wheel(cx, -3.7);
    }
    let stats = fixture.stats(cx);
    assert_eq!(fixture.renders.get(), renders, "the owner stayed clean");
    assert_eq!(stats.layer_repaints, promoted.layer_repaints, "{stats:?}");
    assert_eq!(
        stats.layer_composites,
        promoted.layer_composites + 12,
        "{stats:?}"
    );
    assert_eq!(stats.splice_failures, promoted.splice_failures);
    assert_eq!(stats.rows_built, promoted.rows_built);
}

/// R2: a read of the exact scroll offset while rendering invalidates the optimization.
#[crate::test]
fn a_read_of_the_exact_scroll_offset_repaints_every_frame_the_offset_changes(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(cx, 120, true, false, true);
    for _ in 0..12 {
        fixture.wheel(cx, -3.7);
    }
    let stats = fixture.stats(cx);
    assert_eq!(
        fixture.layers(cx),
        0,
        "no layer for a block whose offset is read"
    );
    assert_eq!(stats.layer_composites, 0, "{stats:?}");
    assert!(
        fixture.renders.get() >= 12,
        "the owner rendered for every offset change"
    );
}

/// Asserts that the window with scroll layers shows what the window drawing directly shows.
fn assert_agree(
    cx: &mut TestAppContext,
    direct: &Fixture,
    layered: &Fixture,
    step: usize,
    history: &[String],
) {
    let recent = || history.iter().rev().take(6).collect::<Vec<_>>();
    let (expected, actual) = (direct.content(cx), layered.content(cx));
    if let Err(difference) = crate::painted_content_agrees(&expected, &actual) {
        panic!(
            "scenes differ at step {step} after {:?}: {difference}",
            recent()
        );
    }
    assert_eq!(
        direct.hits(cx),
        layered.hits(cx),
        "hit tests differ at step {step} after {:?}",
        recent()
    );
    assert_eq!(
        *direct.log.borrow(),
        *layered.log.borrow(),
        "listeners observed different positions at step {step} after {:?}",
        recent()
    );
}
