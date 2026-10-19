//! A 100000-row windowed virtual list (ELM-001 A1).
//!
//! Only a window of 40 rows is mounted at any time; the list reports the visible logical range
//! after each layout and the handler slides the window to follow it. The header shows how many
//! rows were rendered in the last frame, which stays bounded by the viewport plus the overdraw
//! band no matter where the list is scrolled. Use the buttons to jump to the first or last row.

use std::{cell::Cell, rc::Rc};

use vespertine::{
    App, Bounds, Context, ListAlignment, ListOffset, ListState, MouseButton, Render, Window,
    WindowBounds, WindowOptions, div, list, prelude::*, px, rgb, size,
};
use vespertine_platform::application;

const ROW_COUNT: usize = 100_000;
const ESTIMATE: f32 = 24.;
const WINDOW_ROWS: usize = 40;
const SLIDE_MARGIN: usize = 10;

struct WindowedList {
    state: ListState,
    rendered: Rc<Cell<usize>>,
}

impl WindowedList {
    fn new() -> Self {
        let state = ListState::new(WINDOW_ROWS, ListAlignment::Top, px(512.));
        state.set_estimated_item_height(Some(px(ESTIMATE)));
        state.set_logical_item_count(Some(ROW_COUNT as f64));
        state.set_window_start(0);

        let slider = state.clone();
        state.set_scroll_handler(move |event, window, _| {
            let start = slider.window_start();
            let mounted = start..start + slider.mounted_item_count();
            let range = &event.visible_range;
            let near_edge = (range.start < mounted.start + SLIDE_MARGIN && mounted.start > 0)
                || (range.end + SLIDE_MARGIN > mounted.end && mounted.end < ROW_COUNT);
            if near_edge {
                let start = range
                    .start
                    .saturating_sub(SLIDE_MARGIN * 2)
                    .min(ROW_COUNT - WINDOW_ROWS);
                if start != mounted.start {
                    slider.slide_window(start, WINDOW_ROWS);
                    window.refresh();
                }
            }
        });

        Self {
            state,
            rendered: Rc::new(Cell::new(0)),
        }
    }
}

impl Render for WindowedList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let top = self.state.logical_scroll_top();
        let button = |label: &'static str| div().px_2().py_1().bg(rgb(0x303030)).child(label);
        let first = button("First").on_mouse_down(MouseButton::Left, {
            let state = self.state.clone();
            cx.listener(move |_, _, _, cx| {
                state.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                });
                cx.notify();
            })
        });
        let last = button("Last").on_mouse_down(MouseButton::Left, {
            let state = self.state.clone();
            cx.listener(move |_, _, _, cx| {
                state.scroll_to_end();
                cx.notify();
            })
        });

        let rendered = self.rendered.clone();
        let frame_rows = Rc::new(Cell::new(0usize));
        div()
            .size_full()
            .bg(rgb(0x181818))
            .text_color(rgb(0xEEEEEE))
            .flex()
            .flex_col()
            .p_4()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(first)
                    .child(last)
                    .child(format!(
                        "top row {} (+{:.0}px) | rendered last frame: {} | mounted rows {}..{}",
                        top.item_ix,
                        top.offset_in_item.as_f32(),
                        self.rendered.get(),
                        self.state.window_start(),
                        self.state.window_start() + self.state.mounted_item_count(),
                    )),
            )
            .child(
                list(self.state.clone(), move |ix, _, _| {
                    frame_rows.set(frame_rows.get() + 1);
                    rendered.set(frame_rows.get());
                    div()
                        .h(px(ESTIMATE))
                        .w_full()
                        .flex()
                        .items_center()
                        .bg(rgb(if ix % 2 == 0 { 0x202020 } else { 0x282828 }))
                        .child(format!("Row {ix}"))
                        .into_any()
                })
                .px_3()
                .flex_1()
                .bg(rgb(0x202020)),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(600.), px(420.)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| WindowedList::new()),
        )
        .unwrap();
        cx.activate(true);
    });
}
