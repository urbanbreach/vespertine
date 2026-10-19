//! ANM-003 A2: a virtualized list whose removed rows keep their place while they play an exit
//! animation, and which finishes exits for rows that scrolled offscreen instead of retaining them
//! forever.
//!
//! Rows are tracked by a `PresenceManager`; each exiting row is a `with_motion` exit track whose
//! completion report (carrying the track generation) releases it. A row that is not painted never
//! gets a native frame, so it can never report completion: every removal also arms the delayed
//! fallback (exit duration plus a grace period) that delivers the owed completion. The completion
//! is generation checked, so whichever path arrives second is rejected harmlessly.
//!
//! The example drives itself: it scrolls to the bottom, removes three offscreen rows and one
//! visible row, waits, asserts that no exiting row is retained, prints `PASS` and exits 0 (or
//! prints `FAIL` and exits 1) within about eight seconds.

use std::time::Duration;

use vespertine::{
    App, Bounds, Context, ScrollStrategy, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions, div,
    motion::{
        MotionDescription, MotionExt as _, MotionProps, PaintScope, PresenceChild, PresenceFlavor,
        PresenceManager, PresenceOrder, RemovalMode, presets, spawn_fallback_timer,
    },
    prelude::*,
    px, rgb, size, uniform_list,
};
use vespertine_platform::application;

const ROWS: u32 = 40;
const ROW_HEIGHT: f32 = 28.0;
/// Grace added to the exit duration before the fallback fires, so painted rows finish through
/// their native completion first.
const FALLBACK_GRACE: Duration = Duration::from_millis(150);

struct PresenceList {
    manager: PresenceManager<u32>,
    rows: Vec<u32>,
    scroll: UniformListScrollHandle,
    native_completions: usize,
    fallback_completions: usize,
}

impl PresenceList {
    fn new() -> Self {
        let mut this = Self {
            manager: PresenceManager::new(
                true,
                PresenceFlavor {
                    order: PresenceOrder::Positional,
                    removal: RemovalMode::Individual,
                },
            ),
            rows: (0..ROWS).collect(),
            scroll: UniformListScrollHandle::new(),
            native_completions: 0,
            fallback_completions: 0,
        };
        this.reconcile();
        this
    }

    fn reconcile(&mut self) {
        self.manager.reconcile(
            self.rows
                .iter()
                .map(|&key| PresenceChild {
                    key,
                    has_exit: true,
                    signature: 1,
                    payload: (),
                })
                .collect(),
        );
        // Rows without an exit animation (none here) would be released right after effects.
        self.manager.after_effects();
    }

    fn retained_exits(&self) -> usize {
        self.manager
            .items()
            .iter()
            .filter(|item| !item.present)
            .count()
    }

    /// Removes rows: they start their exit, and the fallback is armed for the ones that start now.
    fn remove(&mut self, keys: &[u32], cx: &mut Context<Self>) {
        self.rows.retain(|key| !keys.contains(key));
        self.reconcile();
        let exiting: Vec<(u32, u64)> = self
            .manager
            .items()
            .into_iter()
            .filter(|item| !item.present && keys.contains(&item.key))
            .map(|item| (item.key, item.generation))
            .collect();
        spawn_fallback_timer(
            presets::MENU_EXIT.duration() + FALLBACK_GRACE,
            cx,
            move |this: &mut Self, cx| {
                for (key, generation) in exiting {
                    let outcome = this.manager.motion_complete(&key, generation);
                    this.fallback_completions += outcome.removed.len();
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn on_native_completion(&mut self, key: u32, generation: u64, cx: &mut Context<Self>) {
        let outcome = self.manager.motion_complete(&key, generation);
        self.native_completions += outcome.removed.len();
        cx.notify();
    }
}

impl Render for PresenceList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.manager.items().len();
        div().size_full().bg(rgb(0x181a1f)).child(
            uniform_list(
                "rows",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    let items = this.manager.items();
                    let view = cx.weak_entity();
                    range
                        .map(|ix| {
                            let item = &items[ix];
                            let key = item.key;
                            let description =
                                MotionDescription::new(MotionProps::new().opacity(1.0))
                                    .exit(MotionProps::new().opacity(0.0))
                                    .present(item.present)
                                    .generation(item.generation)
                                    .transition(presets::MENU_EXIT.transition());
                            let view = view.clone();
                            // A closing row stays painted but inert while it fades out.
                            PaintScope::new(
                                div()
                                    .h(px(ROW_HEIGHT))
                                    .px_2()
                                    .text_color(rgb(0xf0f1f3))
                                    .child(format!("Row {key}"))
                                    .with_motion(("row", key as usize), description)
                                    .on_motion_complete(move |completion, _, cx| {
                                        view.update(cx, |this, cx| {
                                            this.on_native_completion(
                                                key,
                                                completion.generation,
                                                cx,
                                            )
                                        })
                                        .ok();
                                    }),
                            )
                            .inert(!item.present)
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .h_full(),
        )
    }
}

fn fail(message: &str) -> ! {
    println!("FAIL: {message}");
    std::process::exit(1);
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(300.0), px(200.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| PresenceList::new()),
            )
            .unwrap();
        let view = window.entity(cx).unwrap();

        // Hard deadline for the scripted run.
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(8)).await;
            fail("scripted run did not finish within 8 seconds");
        })
        .detach();

        cx.spawn(async move |cx| {
            let executor = cx.background_executor().clone();
            let wait = |millis| executor.timer(Duration::from_millis(millis));
            wait(400).await;
            // Scroll to the bottom so rows 10..=12 are offscreen (the list lays out row 0 to
            // measure item height, so the rows under test avoid it).
            view.update(cx, |this, cx| {
                this.scroll
                    .scroll_to_item(ROWS as usize - 1, ScrollStrategy::Top);
                cx.notify();
            });
            wait(400).await;
            // Three offscreen rows and one visible row start their exit.
            view.update(cx, |this, cx| this.remove(&[10, 11, 12, ROWS - 1], cx));
            let retained = view.update(cx, |this, _| this.retained_exits());
            if retained != 4 {
                fail(&format!(
                    "expected 4 exiting rows right after removal, got {retained}"
                ));
            }
            wait(1000).await;
            let (retained, native, fallback) = view.update(cx, |this, _| {
                (
                    this.retained_exits(),
                    this.native_completions,
                    this.fallback_completions,
                )
            });
            if retained != 0 {
                fail(&format!("{retained} exiting rows are still retained"));
            }
            println!("exits finished: {native} natively, {fallback} by the fallback");
            println!("PASS");
            std::process::exit(0);
        })
        .detach();
        cx.activate(true);
    });
}
