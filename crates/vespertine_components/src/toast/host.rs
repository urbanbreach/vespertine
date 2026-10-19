//! The toast host (CMP-018 R4-R6, R12-R14): the process-wide queue as an application global, the
//! publishing functions and the per-window host element.

use super::model::{CardFrame, OVERLAY_PRIORITY, ToastKind, ToastQueue, WakePlan, stack_frame};
use crate::nav_theme::NavTheme;
use std::time::Duration;
use vespertine::{
    AnyElement, App, Global, Hsla, InteractiveElement, IntoElement, MotionClock, MouseButton,
    ParentElement, PathBuilder, Role, SharedString, StatefulInteractiveElement, Styled, Task,
    Window, anchored, canvas, deferred, div, point, px,
};

/// The queue and its single wake-up timer, stored as the application global.
#[derive(Default)]
struct ToastState {
    queue: ToastQueue,
    wake: Option<(f64, Task<()>)>,
}

impl Global for ToastState {}

/// Publishes a notice. The message is trimmed; one that is empty after trimming is ignored.
/// Every window is asked to refresh so mounted hosts redraw. Publishing mutates global state:
/// call it from event handlers and tasks, never from a render pass (see [`take_error`]).
pub fn show(cx: &mut App, kind: ToastKind, message: impl Into<SharedString>) {
    let message: SharedString = message.into();
    let now = MotionClock::now_ms(cx);
    let published = cx
        .default_global::<ToastState>()
        .queue
        .publish(kind, &message, now);
    if published.is_some() {
        cx.refresh_windows();
    }
}

/// Publishes an error notice.
pub fn error(cx: &mut App, message: impl Into<SharedString>) {
    show(cx, ToastKind::Error, message);
}

/// Publishes a success notice.
pub fn success(cx: &mut App, message: impl Into<SharedString>) {
    show(cx, ToastKind::Success, message);
}

/// Publishes an information notice.
pub fn info(cx: &mut App, message: impl Into<SharedString>) {
    show(cx, ToastKind::Info, message);
}

/// Removes the message from a view's pending-error slot and publishes it as an error toast on
/// the next turn of the application loop, never synchronously: publishing mutates global state
/// and refreshes windows, which a render pass must not do. An empty slot does nothing.
pub fn take_error<M: Into<SharedString>>(slot: &mut Option<M>, cx: &mut App) {
    if let Some(message) = slot.take() {
        let message: SharedString = message.into();
        cx.defer(move |cx| error(cx, message));
    }
}

/// The live message texts, oldest first. A test accessor.
pub fn live_messages(cx: &App) -> Vec<SharedString> {
    cx.try_global::<ToastState>()
        .map_or_else(Vec::new, |state| state.queue.live_messages())
}

/// The deadline (in motion-clock milliseconds) the single wake-up timer is set for, if any.
/// A test accessor.
pub fn scheduled_wakeup(cx: &App) -> Option<f64> {
    cx.try_global::<ToastState>()
        .and_then(|state| state.wake.as_ref().map(|(deadline, _)| *deadline))
}

/// Reads the queue. A test accessor.
pub fn with_queue<R>(cx: &App, f: impl FnOnce(&ToastQueue) -> R) -> Option<R> {
    cx.try_global::<ToastState>().map(|state| f(&state.queue))
}

/// Mutates the queue (hover, swipe, dismissal): the internal operations of the host, reachable
/// by tests and embedders that drive the stack without a pointer.
pub fn update_queue<R>(cx: &mut App, f: impl FnOnce(&mut ToastQueue) -> R) -> R {
    let result = f(&mut cx.default_global::<ToastState>().queue);
    cx.refresh_windows();
    result
}

fn queue_mut(cx: &mut App) -> &mut ToastQueue {
    &mut cx.default_global::<ToastState>().queue
}

/// The host element of `window`, or nothing when the queue is empty. Mount it once at the root
/// of the window's element tree. Rendering advances countdowns, exits and glides: it asks for
/// animation frames only while something animates, and otherwise keeps exactly one wake-up
/// timer for the earliest upcoming deadline.
pub fn render_toaster(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if !cx.has_global::<ToastState>() {
        return None;
    }
    let now = MotionClock::now_ms(cx);
    let reduced = cx.reduce_motion();
    let active = window.is_window_active();
    let theme = NavTheme::of(cx);
    let viewport = window.viewport_size();
    let (frame, plan) = {
        let state = cx.global_mut::<ToastState>();
        state.queue.tick(now, active);
        if state.queue.is_empty() {
            state.wake = None;
            return None;
        }
        let frame = state.queue.frame(now, reduced);
        let plan = state.queue.plan(&frame);
        (frame, plan)
    };
    match plan {
        WakePlan::Frames => {
            cx.global_mut::<ToastState>().wake = None;
            window.request_animation_frame();
        }
        WakePlan::At(deadline) => schedule_wakeup(cx, deadline, now),
        WakePlan::Idle => cx.global_mut::<ToastState>().wake = None,
    }

    let (x, y, width, height) = stack_frame(
        viewport.width.as_f32(),
        viewport.height.as_f32(),
        frame.height,
    );
    let cards: Vec<AnyElement> = frame
        .cards
        .iter()
        .filter(|card| !card.skipped)
        .map(|card| card_element(card, width, &theme).into_any_element())
        .collect();
    let stack = div()
        .id("toast-stack")
        .debug_selector(|| "toast-stack".to_owned())
        .role(Role::Group)
        .aria_label("Notifications")
        .accessibility_id("vespertine-toast-stack")
        .occlude()
        .relative()
        .w(px(width))
        .h(px(height))
        .on_hover(move |hovered, _window, cx| {
            let now = MotionClock::now_ms(cx);
            let reduced = cx.reduce_motion();
            queue_mut(cx).set_hovered(*hovered, now, reduced);
            cx.refresh_windows();
        })
        .children(cards);
    Some(
        deferred(
            anchored()
                .position(point(px(x), px(y)))
                .snap_to_window()
                .child(stack),
        )
        .with_priority(OVERLAY_PRIORITY)
        .into_any_element(),
    )
}

/// Keeps exactly one wake-up timer, set for `deadline`, replacing it when the deadline moves.
fn schedule_wakeup(cx: &mut App, deadline: f64, now: f64) {
    let state = cx.global_mut::<ToastState>();
    if state
        .wake
        .as_ref()
        .is_some_and(|(scheduled, _)| (*scheduled - deadline).abs() < 0.5)
    {
        return;
    }
    let delay = Duration::from_secs_f64(((deadline - now).max(0.0)) / 1000.0);
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        cx.update(|cx| cx.refresh_windows());
    });
    cx.global_mut::<ToastState>().wake = Some((deadline, task));
}

fn card_element(card: &CardFrame, stack_width: f32, theme: &NavTheme) -> impl IntoElement {
    let id = card.id;
    let role = if card.kind == ToastKind::Error {
        Role::Alert
    } else {
        Role::Status
    };
    let icon_color = match card.kind {
        ToastKind::Error => theme.danger,
        ToastKind::Success => theme.success,
        ToastKind::Info => theme.muted,
    };
    let kind = card.kind;
    let content = div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .px(px(14.))
        .py(px(13.))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.))
        .child(kind_icon(kind, icon_color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .text_color(theme.text)
                .child(card.message.clone()),
        )
        .child(
            canvas(
                move |bounds, _window, cx| {
                    // The natural height of the card: content plus the 1 px border on both sides.
                    let height = bounds.size.height.as_f32() + 2.0;
                    if queue_mut(cx).set_measured(id, height) {
                        cx.refresh_windows();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
    div()
        .id(("toast", id))
        .debug_selector(move || format!("toast:{id}"))
        .role(role)
        .aria_label(card.message.clone())
        .absolute()
        .bottom(px(card.bottom))
        .left(px(card.inset + card.swipe))
        .w(px((stack_width - 2.0 * card.inset).max(0.0)))
        .h(px(card.clip))
        .overflow_hidden()
        .rounded(theme.popup_radius)
        .border_1()
        .border_color(theme.popup_border)
        .bg(theme.popup_surface)
        .opacity(card.opacity)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            queue_mut(cx).press(id, event.position.x.as_f32());
            window.refresh();
            cx.refresh_windows();
        })
        .on_mouse_move(move |event, window, cx| {
            let held = event.pressed_button == Some(MouseButton::Left);
            queue_mut(cx).swipe_to(id, event.position.x.as_f32(), held);
            window.refresh();
            cx.refresh_windows();
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            release(id, window, cx)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
            release(id, window, cx)
        })
        .child(content)
}

fn release(id: u64, window: &mut Window, cx: &mut App) {
    let now = MotionClock::now_ms(cx);
    queue_mut(cx).release(id, now);
    window.refresh();
    cx.refresh_windows();
}

/// The 16 px kind icon: a warning triangle, a check or an information circle, drawn as strokes
/// in `color`.
fn kind_icon(kind: ToastKind, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
            let mut builder = PathBuilder::stroke(px(1.5));
            match kind {
                ToastKind::Error => {
                    builder.move_to(at(8.0, 1.5));
                    builder.line_to(at(15.0, 14.0));
                    builder.line_to(at(1.0, 14.0));
                    builder.close();
                    builder.move_to(at(8.0, 6.0));
                    builder.line_to(at(8.0, 9.5));
                    builder.move_to(at(8.0, 11.5));
                    builder.line_to(at(8.0, 12.3));
                }
                ToastKind::Success => {
                    builder.move_to(at(2.5, 8.5));
                    builder.line_to(at(6.5, 12.5));
                    builder.line_to(at(13.5, 3.5));
                }
                ToastKind::Info => {
                    let center = (8.0_f32, 8.0_f32);
                    let radius = 6.5_f32;
                    let ring: Vec<_> = (0..32)
                        .map(|step| {
                            let angle = step as f32 / 32.0 * std::f32::consts::TAU;
                            at(
                                center.0 + radius * angle.cos(),
                                center.1 + radius * angle.sin(),
                            )
                        })
                        .collect();
                    builder.add_polygon(&ring, true);
                    builder.move_to(at(8.0, 4.8));
                    builder.line_to(at(8.0, 5.6));
                    builder.move_to(at(8.0, 7.4));
                    builder.line_to(at(8.0, 11.2));
                }
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .flex_none()
    .size(px(16.))
}
