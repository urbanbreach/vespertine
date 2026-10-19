//! CMP-023 A10: idle, loading, error/retry and ready states of one slot, a determinate bar and
//! ring, every indeterminate indicator and a collapsible checklist with a 10-item list and
//! live/idle switching. Click "next state" to cycle the slot, "advance" to move the checklist.

use vespertine::{
    App, AppContext as _, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px,
    rgb, size,
};
use vespertine_components::status::{
    Item, ItemStatus, Loadable, ProgressRing, loadable, matrix_spinner, mini_glyph_spinner,
    progress_bar, pulse_glow, status_section, wave_loader,
};
use vespertine_platform::application;

struct Kit {
    stage: usize,
    progress: f64,
    done: usize,
    live: bool,
}

impl Kit {
    fn state(&self) -> Loadable<SharedString> {
        match self.stage % 4 {
            0 => Loadable::Idle,
            1 => Loadable::Loading,
            2 => Loadable::Error("Could not load the data".into()),
            _ => Loadable::Ready("Loaded content".into()),
        }
    }

    fn items(&self) -> Vec<Item> {
        (0..10)
            .map(|i| {
                let status = if i < self.done {
                    ItemStatus::Completed
                } else if i == self.done {
                    ItemStatus::InProgress
                } else {
                    ItemStatus::Pending
                };
                Item::new(format!("Step {} of the plan", i + 1), status)
            })
            .collect()
    }
}

fn button(label: &'static str) -> vespertine::Stateful<vespertine::Div> {
    div()
        .id(label)
        .px(px(10.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .bg(rgb(0x30343D))
        .cursor_pointer()
        .child(label)
}

impl Render for Kit {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = rgb(0x4C8DF6).into();
        div()
            .size_full()
            .bg(rgb(0x14161A))
            .text_color(rgb(0xE6E8EB))
            .text_size(px(13.0))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(12.0))
                    .child(button("next state").on_click(cx.listener(|this, _, _, cx| {
                        this.stage += 1;
                        cx.notify();
                    })))
                    .child(button("advance").on_click(cx.listener(|this, _, _, cx| {
                        this.progress = (this.progress + 13.0) % 113.0;
                        this.done = (this.done + 1) % 11;
                        cx.notify();
                    })))
                    .child(
                        button("live / idle").on_click(cx.listener(|this, _, _, cx| {
                            this.live = !this.live;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div().flex().flex_row().gap(px(24.0)).items_start().child(
                    loadable("slot", self.state())
                        .width(px(280.0))
                        .height(px(96.0))
                        .content(|text, _, _| div().child(text).into_any_element())
                        .loading_label("Loading data")
                        .on_retry({
                            let kit = cx.entity();
                            move |_, cx| {
                                kit.update(cx, |kit, cx| {
                                    kit.stage += 1;
                                    cx.notify();
                                })
                            }
                        }),
                ),
            )
            .child(
                div()
                    .w(px(280.0))
                    .child(progress_bar(self.progress, 100.0).label("Upload")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(16.0))
                    .child(ProgressRing::compact(
                        (self.progress / 100.0) as f32,
                        accent,
                    ))
                    .child(
                        div()
                            .bg(rgb(0x20242C))
                            .child(ProgressRing::labeled(self.progress as u32, 44.0)),
                    )
                    .child(wave_loader("wave", 6.0))
                    .child(matrix_spinner("matrix", 4.0))
                    .child(mini_glyph_spinner("mini", 2.5))
                    .child(
                        div()
                            .relative()
                            .w(px(80.0))
                            .h(px(24.0))
                            .child(pulse_glow("glow", 6.0, accent, 0.15, 0.7)),
                    ),
            )
            .child(
                div()
                    .w(px(320.0))
                    .child(status_section("kit-plan", self.items(), self.live).name("plan")),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(520.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Kit {
                    stage: 0,
                    progress: 40.0,
                    done: 3,
                    live: true,
                })
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
