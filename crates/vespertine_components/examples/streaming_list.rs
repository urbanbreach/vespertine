//! CMP-029 A1: tokens stream into a variable-height list that follows the bottom, detaches when
//! the user scrolls up, keeps its anchor when tall media lands below, and re-pins when the user
//! returns toward the bottom.
//!
//! Interactively the list streams tokens and rows forever; wheel up to detach, wheel down to
//! re-engage; the jump-to-latest button appears beyond 320 px. With `STREAMING_SCRIPT=1` the
//! example drives the acceptance script with synthetic wheel input and exits 0 when every check
//! passes: an upward notch detaches, tall media below does not move the detached anchor by more
//! than 1 px, scrolling to within 70 px while moving toward the bottom re-pins, and a wheel-up
//! notch inside 70 px does not.

use std::{cell::RefCell, rc::Rc, time::Duration};

use vespertine::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, InteractiveElement,
    IntoElement, ListAlignment, ListState, MouseButton, ParentElement, PlatformInput, Render,
    ScrollDelta, ScrollWheelEvent, Styled, Window, WindowBounds, WindowOptions, div, list, point,
    px, rgb, size,
};
use vespertine_components::streaming::{Commit, FollowController};
use vespertine_platform::application;

struct Example {
    list: ListState,
    heights: Rc<RefCell<Vec<f32>>>,
    controller: FollowController,
}

impl Render for Example {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let heights = self.heights.clone();
        let controller = self.controller.clone();
        let jump = self.controller.show_jump_button();
        div().size_full().bg(rgb(0x101216)).p_4().child(
            div()
                .relative()
                .w(px(400.0))
                .h(px(320.0))
                .bg(rgb(0x181a1f))
                .child(
                    list(self.list.clone(), move |ix, _, _| {
                        let h = heights.borrow().get(ix).copied().unwrap_or(10.0);
                        div()
                            .h(px(h))
                            .w_full()
                            .px_2()
                            .bg(rgb(if ix % 2 == 0 { 0x252a33 } else { 0x2d333d }))
                            .text_color(rgb(0xf0f1f3))
                            .child(format!("message {ix} ({h} px)"))
                            .into_any_element()
                    })
                    .size_full(),
                )
                .children(jump.then(|| {
                    div()
                        .id("jump")
                        .absolute()
                        .right_2()
                        .bottom_2()
                        .px_2()
                        .py_1()
                        .rounded(px(6.0))
                        .bg(rgb(0x405470))
                        .text_color(rgb(0xf0f1f3))
                        .child("Jump to latest")
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            controller.jump_to_bottom(window, cx)
                        })
                })),
        )
    }
}

struct Script {
    window: AnyWindowHandle,
    heights: Rc<RefCell<Vec<f32>>>,
    controller: FollowController,
    list: ListState,
}

impl Script {
    async fn sleep(&self, cx: &AsyncApp, ms: u64) {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
    }

    fn commit(&self, cx: &mut AsyncApp, commit: Commit) {
        let controller = self.controller.clone();
        cx.update_window(self.window, move |_, window, cx| {
            controller.commit(commit, window, cx)
        })
        .ok();
    }

    async fn append(&self, cx: &mut AsyncApp, height: f32) {
        let n = self.heights.borrow().len();
        self.heights.borrow_mut().push(height);
        self.commit(
            cx,
            Commit::Splice {
                old_range: n..n,
                new_count: 1,
            },
        );
        self.sleep(cx, 30).await;
    }

    async fn token(&self, cx: &mut AsyncApp) {
        let n = self.heights.borrow().len();
        *self.heights.borrow_mut().last_mut().unwrap() += 6.0;
        self.commit(cx, Commit::ReplaceInPlace(n - 1..n));
        self.sleep(cx, 30).await;
    }

    async fn wheel(&self, cx: &mut AsyncApp, dy: f32) {
        cx.update_window(self.window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(px(200.0), px(150.0)),
                    delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
                    ..Default::default()
                }),
                cx,
            );
        })
        .ok();
        self.sleep(cx, 80).await;
    }

    fn top_of(&self, row: usize) -> f32 {
        self.list
            .bounds_for_item(row)
            .map_or(f32::NAN, |bounds| f32::from(bounds.top()))
    }
}

fn main() {
    let script = std::env::var_os("STREAMING_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(440.0), px(360.0)), cx);
        let heights = Rc::new(RefCell::new(vec![40.0f32; 20]));
        let list = ListState::new(20, ListAlignment::Top, px(100.0));
        let controller = FollowController::attach(list.clone());
        controller.set_tail_streaming(true);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                {
                    let (list, heights, controller) =
                        (list.clone(), heights.clone(), controller.clone());
                    move |_, cx| {
                        cx.new(|_| Example {
                            list,
                            heights,
                            controller,
                        })
                    }
                },
            )
            .expect("open the window");
        cx.activate(true);
        let driver = Script {
            window: window.into(),
            heights,
            controller,
            list,
        };
        cx.spawn(async move |cx| {
            driver.sleep(cx, 800).await;
            let mut failures = Vec::new();
            let mut check = |name: &str, ok: bool| {
                println!("{}: {name}", if ok { "ok" } else { "FAILED" });
                if !ok {
                    failures.push(name.to_owned());
                }
            };
            for _ in 0..30 {
                driver.token(cx).await;
            }
            check(
                "pinned while streaming stays at the end",
                driver.controller.is_pinned() && driver.controller.distance_from_bottom() <= 2.0,
            );
            driver.wheel(cx, 30.0).await;
            check(
                "an upward wheel notch detaches",
                !driver.controller.is_pinned(),
            );
            let anchor = driver.list.logical_scroll_top().item_ix;
            let before = driver.top_of(anchor);
            driver.append(cx, 400.0).await;
            driver.append(cx, 400.0).await;
            for _ in 0..10 {
                driver.token(cx).await;
            }
            driver.sleep(cx, 200).await;
            let after = driver.top_of(anchor);
            check(
                "tall media below does not move the detached anchor by more than 1 px",
                (after - before).abs() <= 1.0,
            );
            driver.wheel(cx, 120.0).await;
            let far = driver.controller.distance_from_bottom();
            driver.wheel(cx, -(far - 60.0)).await;
            check(
                "scrolling to within 70 px moving toward the bottom re-pins",
                driver.controller.is_pinned(),
            );
            driver.sleep(cx, 1500).await;
            driver.wheel(cx, 30.0).await;
            driver.wheel(cx, 20.0).await;
            check(
                "a wheel-up notch inside 70 px does not re-pin",
                !driver.controller.is_pinned(),
            );
            if script {
                std::process::exit(if failures.is_empty() { 0 } else { 1 });
            }
            // Interactive: keep streaming.
            loop {
                for _ in 0..12 {
                    driver.token(cx).await;
                }
                driver.append(cx, 60.0).await;
            }
        })
        .detach();
    });
}
