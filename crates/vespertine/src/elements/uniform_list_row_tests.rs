//! Tests of retained virtual-list rows for the uniform-height list (PRF-005).

use crate::{
    AnyWindowHandle, AppContext as _, Context, Entity, InputEvent as _, InteractiveElement as _,
    IntoElement, Modifiers, MouseMoveEvent, ParentElement as _, Render, ScrollDelta,
    ScrollWheelEvent, Styled as _, TestAppContext, TouchPhase, UniformListScrollHandle, Window,
    div, point, px, uniform_list,
};
use std::{cell::RefCell, rc::Rc};

const ROW_HEIGHT: f32 = 20.0;
const VIEWPORT: f32 = 100.0;

struct Model {
    value: u32,
}

/// Row indices the row callback rendered, in order.
type Renders = Rc<RefCell<Vec<usize>>>;

struct ListView {
    scroll: UniformListScrollHandle,
    models: Vec<Entity<Model>>,
    renders: Renders,
    count: usize,
}

impl Render for ListView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let renders = self.renders.clone();
        let models = self.models.clone();
        div().size_full().child(
            uniform_list("rows", self.count, move |range, _, cx| {
                range
                    .map(|ix| {
                        renders.borrow_mut().push(ix);
                        let value = models[ix].read(cx).value;
                        div()
                            .id(("row", ix))
                            .h(px(ROW_HEIGHT))
                            .w_full()
                            .bg(if ix % 2 == 0 {
                                crate::white()
                            } else {
                                crate::rgb(0xf0f0f0).into()
                            })
                            .hover(|style| style.bg(crate::rgb(0xd0e0ff)))
                            .child(format!("row {ix} value {value}"))
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .h(px(VIEWPORT)),
        )
    }
}

struct Fixture {
    models: Vec<Entity<Model>>,
}

impl Fixture {
    fn new(cx: &mut TestAppContext, count: usize) -> Self {
        Self {
            models: (0..count).map(|_| cx.new(|_| Model { value: 0 })).collect(),
        }
    }

    fn window(
        &self,
        cx: &mut TestAppContext,
        retained: bool,
    ) -> (AnyWindowHandle, Entity<ListView>, Renders) {
        let renders: Renders = Rc::default();
        let models = self.models.clone();
        let count = models.len();
        let handle = cx.add_window({
            let renders = renders.clone();
            move |_, _| ListView {
                scroll: UniformListScrollHandle::new(),
                models,
                renders,
                count,
            }
        });
        let root = handle.root(cx).unwrap();
        let window: AnyWindowHandle = handle.into();
        cx.update_window(window, |_, window, _| {
            window.set_retention_enabled(retained)
        })
        .unwrap();
        draw(cx, window);
        draw(cx, window);
        (window, root, renders)
    }
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn wheel(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) {
    cx.update_window(window, |_, window, cx| {
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
    draw(cx, window);
}

fn move_pointer(cx: &mut TestAppContext, window: AnyWindowHandle, x: f32, y: f32) {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            MouseMoveEvent {
                modifiers: Modifiers::default(),
                pressed_button: None,
                position: point(px(x), px(y)),
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    draw(cx, window);
}

fn signature(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
) -> (Vec<String>, Vec<Vec<crate::Bounds<crate::Pixels>>>) {
    cx.update_window(window, |_, window, _| {
        let hits = [(10., 5.), (10., 45.), (30., 95.), (10., 130.)]
            .into_iter()
            .map(|(x, y)| window.hit_test_bounds(point(px(x), px(y))))
            .collect();
        (window.painted_primitives(), hits)
    })
    .unwrap()
}

/// A1: every frame of a history of wheel scrolling, reversals, hovers and model changes, the
/// retained window shows the primitives and hit-tests exactly what a window without row
/// replay shows.
#[crate::test]
fn retained_rows_match_ordinary_virtualization_every_frame(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, 400);
    let (retained, _, _) = fixture.window(cx, true);
    let (full, _, _) = fixture.window(cx, false);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    let mut log: Vec<String> = Vec::new();
    for step in 0..200 {
        match next(8) {
            0..=3 => {
                let dy = [-7.5, -23.0, -40.0, 11.0, 3.25, -90.0][next(6) as usize];
                log.push(format!("{step}: wheel {dy}"));
                wheel(cx, retained, dy);
                wheel(cx, full, dy);
            }
            4 => {
                let (x, y) = (next(200) as f32, next(120) as f32);
                log.push(format!("{step}: move {x} {y}"));
                move_pointer(cx, retained, x, y);
                move_pointer(cx, full, x, y);
            }
            5 => {
                let ix = next(400) as usize;
                log.push(format!("{step}: model {ix}"));
                fixture.models[ix].update(cx, |model, cx| {
                    model.value += 1;
                    cx.notify();
                });
            }
            _ => {}
        }
        draw(cx, retained);
        draw(cx, full);
        let (retained_scene, retained_hits) = signature(cx, retained);
        let (full_scene, full_hits) = signature(cx, full);
        if retained_scene != full_scene {
            let at = retained_scene
                .iter()
                .zip(&full_scene)
                .position(|(a, b)| a != b);
            panic!(
                "primitives differ at step {step} after {:?}: lengths {} vs {}, first difference at {at:?}\nRETAINED {:?}\nFULL {:?}",
                log.iter().rev().take(6).collect::<Vec<_>>(),
                retained_scene.len(),
                full_scene.len(),
                at.map(|at| &retained_scene[at]),
                at.map(|at| &full_scene[at]),
            );
        }
        assert_eq!(retained_hits, full_hits, "hit tests differ at step {step}");
    }
}

/// Scrolling by a few pixels per frame builds only the rows that enter the viewport.
#[crate::test]
fn scrolling_builds_only_the_rows_that_enter(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, 400);
    let (window, _, renders) = fixture.window(cx, true);
    // Fill the retained region, then scroll steadily.
    for _ in 0..6 {
        wheel(cx, window, -7.0);
    }
    renders.borrow_mut().clear();
    let mut worst = 0;
    for _ in 0..60 {
        let before = renders.borrow().len();
        wheel(cx, window, -7.0);
        worst = worst.max(renders.borrow().len() - before);
    }
    assert!(
        worst <= 3,
        "a 7 px scroll step built {worst} rows (renders: {:?})",
        renders.borrow()
    );
}
