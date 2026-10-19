//! CMP-031 A1: draggable overlay rails and classic thumb indicators.
//!
//! The left pane scrolls both ways under overlay rails: scroll (or press the track) to reveal
//! the vertical rail, drag either thumb (also past the end of its track, and release outside it),
//! and watch the rail linger 1400 ms, fade over 260 ms and disappear after 1660 ms. The hover-reveal
//! horizontal rail shows while the pane is hovered. The right pane has classic paint-only thumbs
//! (a style color, no layout space, no hit target).

use std::rc::Rc;

use vespertine::{
    App, AppContext as _, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px,
    rgb, size,
};
use vespertine_components::scroll::{
    HandleRail, RailBar, RailHost, RailOrientation, render_rail, track_hover,
};
use vespertine_platform::application;

struct Scrollbars {
    overlay: ScrollHandle,
    classic: ScrollHandle,
    vertical: RailBar,
    horizontal: RailBar,
}

fn cells(count: usize, columns: usize) -> impl IntoElement {
    let mut grid = div().flex().flex_col().gap_1().p_2();
    for row in 0..count / columns {
        let mut line = div().flex().flex_row().gap_1();
        for column in 0..columns {
            let n = row * columns + column;
            line = line.child(
                div()
                    .flex_none()
                    .w(px(90.0))
                    .h(px(28.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x30343d))
                    .text_color(rgb(0xf0f1f3))
                    .text_size(px(12.0))
                    .child(format!("cell {n}")),
            );
        }
        grid = grid.child(line);
    }
    grid
}

impl Render for Scrollbars {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vertical: Rc<dyn RailHost> =
            Rc::new(HandleRail::new(self.overlay.clone(), self.vertical.clone()));
        let horizontal: Rc<dyn RailHost> = Rc::new(HandleRail::new(
            self.overlay.clone(),
            self.horizontal.clone(),
        ));
        let vertical = render_rail(
            "overlay-vertical",
            vertical,
            RailOrientation::Vertical,
            window,
            cx,
        );
        let horizontal = render_rail(
            "overlay-horizontal",
            horizontal,
            RailOrientation::Horizontal,
            window,
            cx,
        );
        let overlay_pane = track_hover(
            div()
                .id("overlay-pane")
                .relative()
                .w(px(300.0))
                .h(px(340.0))
                .bg(rgb(0x181a1f))
                .child(
                    div()
                        .id("overlay-scroll")
                        .size_full()
                        .overflow_scroll()
                        .track_scroll(&self.overlay)
                        .child(cells(400, 8)),
                )
                .children(vertical)
                .children(horizontal),
            &self.horizontal,
        );
        div()
            .size_full()
            .bg(rgb(0x101010))
            .p_4()
            .flex()
            .flex_row()
            .gap_4()
            .child(overlay_pane)
            .child(
                div()
                    .id("classic-pane")
                    .w(px(300.0))
                    .h(px(340.0))
                    .bg(rgb(0x181a1f))
                    .overflow_scroll()
                    .scrollbar_color(rgb(0xa0a0a0))
                    .track_scroll(&self.classic)
                    .child(cells(400, 8)),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(680.0), px(380.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Scrollbars {
                    overlay: ScrollHandle::new(),
                    classic: ScrollHandle::new(),
                    vertical: RailBar::new(),
                    horizontal: RailBar::hover_reveal(),
                })
            },
        )
        .expect("open the window");
        cx.activate(true);
    });
}
