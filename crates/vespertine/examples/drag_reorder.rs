//! Reordering a scrolled, padded list by dragging, built on `vespertine::drag_drop`.
//!
//! * The slot under the pointer is `floor((pointer - list origin - scroll offset - padding) /
//!   row height)`, clamped: the pointer in the leading padding targets slot 0, in the trailing
//!   padding the last slot, and over rows the row slot adjusted by the scroll offset.
//! * While dragging, the dragged row travels to the hovered slot and the rows in between slide one
//!   slot toward the source (the gap preview); only rows whose destination changed restart their
//!   slide.
//! * Dropping applies the insertion rule once. Releasing outside the list (or pressing Escape)
//!   clears every drag visual and leaves the order unchanged.
//! * Dragging near the top or bottom edge of the list autoscrolls it (profile (a): up to 540 px/s,
//!   linear in depth into a 28 px band).

use std::time::Duration;

use vespertine::{
    Animation, AnimationExt as _, App, Bounds, Context, DragMoveEvent, FocusHandle, KeyDownEvent,
    ScrollHandle, SharedString, Window, WindowBounds, WindowOptions, div,
    drag_drop::{
        AutoscrollLoop, EdgeAutoscroll, ReorderPreview, content_coordinate, set_drag_cursor,
        uniform_slot,
    },
    prelude::*,
    px, rgb, size, white,
};
use vespertine_platform::application;

const ROW_HEIGHT: f32 = 40.;
const LIST_PADDING: f32 = 12.;
const LIST_WIDTH: f32 = 380.;
const LIST_HEIGHT: f32 = 280.;
const SLIDE: Duration = Duration::from_millis(120);

/// The typed payload of a row drag: the index the row was dragged from.
#[derive(Clone)]
struct RowDrag {
    from: usize,
    label: SharedString,
}

/// The preview that follows the pointer.
struct RowPreview {
    label: SharedString,
}

impl Render for RowPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(LIST_WIDTH))
            .h(px(ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .rounded_md()
            .bg(rgb(0x2563eb))
            .opacity(0.85)
            .text_color(white())
            .child(self.label.clone())
    }
}

struct DragReorder {
    items: Vec<u32>,
    preview: ReorderPreview,
    scroll: ScrollHandle,
    autoscroll: AutoscrollLoop,
    focus: FocusHandle,
    status: SharedString,
}

impl DragReorder {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            items: (1..=12).collect(),
            preview: ReorderPreview::new(),
            scroll: ScrollHandle::new(),
            autoscroll: AutoscrollLoop::new(),
            focus: cx.focus_handle(),
            status: "Drag a row to reorder; Escape cancels".into(),
        }
    }

    /// The slot under the pointer, or `None` when the pointer is outside the list.
    fn slot_under_pointer(&self, window: &Window) -> Option<usize> {
        let viewport = self.scroll.bounds();
        let pointer = window.mouse_position();
        if !viewport.contains(&pointer) {
            return None;
        }
        let coordinate = content_coordinate(
            pointer.y,
            viewport.top(),
            self.scroll.offset().y,
            px(LIST_PADDING),
        );
        uniform_slot(coordinate, px(ROW_HEIGHT), self.items.len())
    }

    fn refresh_preview(&mut self, window: &Window) {
        match self.slot_under_pointer(window) {
            Some(over) => {
                self.preview.hover(over);
            }
            None => {
                self.preview.leave();
            }
        }
    }

    fn row_label(item: u32) -> SharedString {
        format!("Item {item}").into()
    }
}

impl Render for DragReorder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A mouse-up outside the list ends the drag without any drop callback, so every
        // drag-derived visual is reset in any frame without an active drag.
        if !cx.has_active_drag() {
            self.preview.clear();
        } else if self.preview.from().is_some() {
            // Autoscroll moves content under a still pointer: keep the hovered slot current.
            self.refresh_preview(window);
        }

        let rows = self.items.iter().enumerate().map(|(ix, item)| {
            let item = *item;
            let label = Self::row_label(item);
            let transition = self.preview.transition(ix);
            let dragged = self.preview.from() == Some(ix);
            let hovered = self.preview.over() == Some(ix);
            let row = div()
                .id(("row", item as usize))
                .relative()
                .h(px(ROW_HEIGHT))
                .w_full()
                .flex_none()
                .px_3()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(0xd1d5db))
                .bg(if hovered {
                    rgb(0xdbeafe)
                } else {
                    rgb(0xffffff)
                })
                .when(dragged, |row| row.opacity(0.4))
                .child(label.clone())
                .on_drag(RowDrag { from: ix, label }, |drag, _, _, cx| {
                    let label = drag.label.clone();
                    cx.new(|_| RowPreview { label })
                });
            match transition {
                // Only rows whose destination changed restart their slide.
                Some(transition) if transition.restarts() => row
                    .with_animation(
                        format!("slide-{item}-{}", transition.to),
                        Animation::new(SLIDE),
                        move |row, progress| row.top(transition.offset(progress, px(ROW_HEIGHT))),
                    )
                    .into_any_element(),
                Some(transition) => row
                    .top(transition.offset(1., px(ROW_HEIGHT)))
                    .into_any_element(),
                None => row.into_any_element(),
            }
        });

        div()
            .id("root")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && cx.stop_active_drag(window) {
                    this.preview.clear();
                    this.autoscroll.cancel();
                    this.status = "Cancelled".into();
                    cx.notify();
                }
            }))
            .size_full()
            .bg(rgb(0xf3f4f6))
            .text_color(rgb(0x111827))
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(16.))
                    .text_sm()
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .id("list")
                    .absolute()
                    .left(px(20.))
                    .top(px(70.))
                    .w(px(LIST_WIDTH))
                    .h(px(LIST_HEIGHT))
                    .py(px(LIST_PADDING))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x9ca3af))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<RowDrag>, window, cx| {
                            if this.preview.from().is_none() {
                                this.preview.begin(event.drag(cx).from);
                            }
                            this.refresh_preview(window);
                            set_drag_cursor(window, cx, this.preview.over().is_some());
                            this.autoscroll.start_vertical(
                                window,
                                cx,
                                &this.scroll,
                                EdgeAutoscroll::TimeLinear,
                            );
                            cx.notify();
                        },
                    ))
                    .on_drop(cx.listener(|this, _: &RowDrag, _, cx| {
                        // The pointer is over the list, padding included: apply the insertion
                        // rule once.
                        if let Some(reorder) = this.preview.pending_move(this.items.len()) {
                            reorder.apply(&mut this.items);
                            this.status = format!(
                                "Moved item {} to position {}",
                                reorder.from + 1,
                                reorder.to + 1
                            )
                            .into();
                        } else {
                            this.status = "Dropped on itself; order unchanged".into();
                        }
                        this.preview.clear();
                        this.autoscroll.cancel();
                        cx.notify();
                    }))
                    .children(rows),
            )
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(400.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(DragReorder::new);
                    window.focus(&view.read(cx).focus.clone(), cx);
                    view
                },
            )
            .unwrap();
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.activate(true);
    });
}

fn main() {
    run_example();
}
