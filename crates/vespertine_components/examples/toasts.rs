//! CMP-018 A10: the toast stack.
//!
//! The window starts with one notice of each kind (information, success, error). Use the buttons
//! to publish more: hover the stack to pause every countdown and fan the cards out, drag a card to
//! the right past 45 px to dismiss it, deactivate the window to pause the countdowns. "Pending
//! error" fills a view's pending-error slot, which `take_error` publishes on the next turn.

use vespertine::{
    App, AppContext, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px,
    size,
};
use vespertine_components::{
    nav_theme::NavTheme,
    toast::{self, ToastKind, render_toaster, take_error},
};
use vespertine_platform::application;

struct Example {
    pending_error: Option<SharedString>,
    published: usize,
}

fn button(
    label: &'static str,
    cx: &mut Context<Example>,
    on_click: impl Fn(&mut Example, &mut Window, &mut Context<Example>) + 'static,
) -> impl IntoElement {
    let theme = NavTheme::neutral();
    div()
        .id(label)
        .px(px(12.))
        .py(px(6.))
        .rounded(px(6.))
        .bg(theme.popup_surface)
        .border_1()
        .border_color(theme.popup_border)
        .cursor_pointer()
        .child(label)
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Publishing mutates global state, so a pending error is handed to the next turn.
        take_error(&mut self.pending_error, cx);
        let theme = NavTheme::neutral();
        let buttons = div()
            .flex()
            .flex_row()
            .gap(px(8.))
            .child(button("Error", cx, |this, _, cx| {
                this.published += 1;
                toast::error(cx, format!("Something failed ({})", this.published));
            }))
            .child(button("Success", cx, |this, _, cx| {
                this.published += 1;
                toast::success(cx, format!("Saved ({})", this.published));
            }))
            .child(button("Info", cx, |this, _, cx| {
                this.published += 1;
                toast::info(cx, format!("Heads up ({})", this.published));
            }))
            .child(button("Burst of five", cx, |this, _, cx| {
                for _ in 0..5 {
                    this.published += 1;
                    toast::info(cx, format!("Notice number {}", this.published));
                }
            }))
            .child(button("Same error twice", cx, |_, _, cx| {
                toast::error(cx, "The same error");
                toast::error(cx, "The same error");
            }))
            .child(button("Pending error", cx, |this, _, cx| {
                this.pending_error = Some("Raised from a pending-error slot".into());
                cx.notify();
            }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .bg(theme.background)
            .text_color(theme.text)
            .text_size(px(13.))
            .child("Toasts stack in the bottom-right corner. Hover to pause and fan out; swipe right to dismiss.")
            .child(buttons)
            .children(render_toaster(window, cx))
    }
}

fn main() {
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(720.), px(420.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                toast::info(cx, "Information: this notice lives four seconds");
                toast::success(cx, "Success: everything was saved");
                toast::show(cx, ToastKind::Error, "Error: something went wrong");
                cx.new(|_| Example {
                    pending_error: None,
                    published: 0,
                })
            },
        )
        .expect("open the window");
        cx.activate(true);
    });
}
