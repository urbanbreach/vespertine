//! Wheel scrolling and scrollbar dragging (R4). Both deliver platform input events to the
//! window, so hit testing, hover updates and listener dispatch are part of every frame.

use super::{BORDER, INK, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, EventLog, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use vespertine::{
    AnyView, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels,
    PlatformInput, Point, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, TouchPhase, Window, div, point, px,
};

const ROWS: usize = 400;
const ROW_HEIGHT: f32 = 24.0;
const WINDOW_HEIGHT: f32 = 480.0;
/// Where the pointer rests while wheeling.
const WHEEL_POINT: (f32, f32) = (300.0, 240.0);
const WHEEL_STEP: f32 = 37.0;
/// Frames between direction reversals of the wheel.
const WHEEL_SWEEP: u64 = 60;

fn rows() -> impl Iterator<Item = impl IntoElement> {
    (0..ROWS).map(|row| {
        let base = div()
            .id(("scroll-row", row))
            .h(px(ROW_HEIGHT))
            .px(px(12.0))
            .flex()
            .flex_none()
            .items_center()
            .hover(|style| style.bg(hex(0xddf4ff)));
        let base = if row % 2 == 1 {
            base.bg(hex(SURFACE))
        } else {
            base
        };
        base.child(format!(
            "Row {row}: the quick brown fox jumps over the lazy dog"
        ))
    })
}

struct WheelView {
    scroll: ScrollHandle,
    log: EventLog,
}

impl Render for WheelView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        div()
            .id("wheel-scroll")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .on_scroll_wheel(move |event, _, _| {
                log.borrow_mut().push(format!(
                    "wheel at {:.1},{:.1} delta {:?}",
                    f32::from(event.position.x),
                    f32::from(event.position.y),
                    event.delta
                ));
            })
            .children(rows())
    }
}

struct WheelDriver;

impl Driver for WheelDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let direction = if (frame / WHEEL_SWEEP).is_multiple_of(2) {
            -1.0
        } else {
            1.0
        };
        stage.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: point(px(WHEEL_POINT.0), px(WHEEL_POINT.1)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(direction * WHEEL_STEP))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        }));
    }
}

fn wheel_setup(harness: &mut dyn Host) -> Result<Box<dyn Driver>> {
    let log = harness.log();
    let scroll = ScrollHandle::new();
    harness.open_window(|_, cx| AnyView::from(cx.new(|_| WheelView { scroll, log })))?;
    // Hover state needs a pointer inside the window.
    harness.read_window(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(WHEEL_POINT.0), px(WHEEL_POINT.1)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
    })?;
    Ok(Box::new(WheelDriver))
}

const TRACK_X: f32 = 874.0;
const THUMB_WIDTH: f32 = 10.0;
const DRAG_CYCLE: u64 = 72;

struct DragView {
    scroll: ScrollHandle,
    /// Pointer offset inside the thumb while dragging.
    grab: Option<Pixels>,
    log: EventLog,
}

impl DragView {
    /// The thumb's top and height, from the scroll state.
    fn thumb(&self) -> (Pixels, Pixels) {
        let viewport = px(WINDOW_HEIGHT);
        let content = px(ROWS as f32 * ROW_HEIGHT);
        let height = (viewport * (viewport / content)).max(px(24.0));
        let max = self.scroll.max_offset().y;
        let travel = viewport - height;
        let top = if max > px(0.0) {
            travel * (-self.scroll.offset().y / max)
        } else {
            px(0.0)
        };
        (top, height)
    }
}

impl Render for DragView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (top, height) = self.thumb();
        div()
            .id("drag-root")
            .size_full()
            .relative()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.log.borrow_mut().push(format!(
                    "move at {:.1},{:.1}",
                    f32::from(event.position.x),
                    f32::from(event.position.y)
                ));
                if let Some(grab) = this.grab {
                    let viewport = px(WINDOW_HEIGHT);
                    let (_, height) = this.thumb();
                    let travel = viewport - height;
                    let ratio = ((event.position.y - grab) / travel).clamp(0.0, 1.0);
                    let max = this.scroll.max_offset().y;
                    this.scroll.set_offset(point(px(0.0), -(max * ratio)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.log.borrow_mut().push(format!(
                        "up at {:.1},{:.1}",
                        f32::from(event.position.x),
                        f32::from(event.position.y)
                    ));
                    this.grab = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("drag-content")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows()),
            )
            .child(
                div()
                    .id("drag-thumb")
                    .absolute()
                    .left(px(TRACK_X))
                    .top(top)
                    .w(px(THUMB_WIDTH))
                    .h(height)
                    .rounded(px(5.0))
                    .bg(hex(if self.grab.is_some() {
                        0x57606a
                    } else {
                        BORDER
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let (top, _) = this.thumb();
                            this.log.borrow_mut().push(format!(
                                "down at {:.1},{:.1}",
                                f32::from(event.position.x),
                                f32::from(event.position.y)
                            ));
                            this.grab = Some(event.position.y - top);
                            cx.notify();
                        }),
                    ),
            )
    }
}

struct DragDriver {
    view: Entity<DragView>,
    /// Pointer position of the drag in progress.
    pointer: Point<Pixels>,
}

impl Driver for DragDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let phase = frame % DRAG_CYCLE;
        match phase {
            0 => {
                let (top, height) = self.view.read(stage.app()).thumb();
                self.pointer = point(px(TRACK_X + THUMB_WIDTH / 2.0), top + height / 2.0);
                stage.dispatch(PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: self.pointer,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }));
            }
            1..=70 => {
                // 35 frames down, 35 frames back up.
                let direction = if phase <= 35 { 1.0 } else { -1.0 };
                self.pointer.y =
                    (self.pointer.y + px(direction * 5.0)).clamp(px(0.0), px(WINDOW_HEIGHT));
                stage.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
                    position: self.pointer,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }));
            }
            _ => stage.dispatch(PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: self.pointer,
                modifiers: Modifiers::default(),
                click_count: 1,
            })),
        }
    }
}

fn drag_setup(harness: &mut dyn Host) -> Result<Box<dyn Driver>> {
    let log = harness.log();
    let scroll = ScrollHandle::new();
    let mut view = None;
    harness.open_window(|_, cx| {
        let entity = cx.new(|_| DragView {
            scroll,
            grab: None,
            log,
        });
        view = Some(entity.clone());
        AnyView::from(entity)
    })?;
    // Lay the content out once so the scroll range is known before the first press.
    harness.settle()?;
    Ok(Box::new(DragDriver {
        view: view.expect("the window built the view"),
        pointer: Point::default(),
    }))
}

/// The wheel and scrollbar-drag workloads.
pub fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "wheel-scroll",
            family: Family::WheelScroll,
            description: "one 37 px wheel notch per frame, reversing every 60 frames",
            warmup: 30,
            frames: 200,
            setup: wheel_setup,
        },
        Workload {
            name: "scrollbar-drag",
            family: Family::ScrollbarDrag,
            description: "a scrollbar thumb is pressed, dragged down and back up, and released",
            warmup: 30,
            frames: 200,
            setup: drag_setup,
        },
    ]
}
