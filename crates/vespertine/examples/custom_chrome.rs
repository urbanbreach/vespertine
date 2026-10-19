//! A window with fully custom chrome (PLT-007).
//!
//! The window asks for client-side decorations. What it draws depends on who the compositor says
//! owns the decorations, every frame:
//!
//! * client-side: a rounded, shadowed frame with a titlebar. The titlebar is a drag region
//!   (press to move, double click to maximize or restore, right click for the window menu), carries
//!   the caption controls in the order of the desktop's button layout, and the whole window can
//!   be resized from all eight edges.
//! * server-side (or a forced fallback): only the content; the compositor draws the frame.
//!
//! Use the buttons in the body to switch between the two modes and between an opaque and a
//! blurred background (blur needs a compositor that offers it; otherwise the window stays opaque).

#[path = "example_support/fonts.rs"]
mod example_support;

use vespertine::{
    App, Bounds, BoxShadow, CaptionControls, CaptionControlsSide, Context, CursorStyle,
    Decorations, Hsla, SharedString, Window, WindowBackgroundAppearance, WindowBounds,
    WindowChromeElement, WindowDecorations, WindowOptions, div, hsla, prelude::*, px, rgb, size,
    transparent_black, window_resize_handles,
};
use vespertine_platform::application;

/// Width of the invisible margin around the frame that holds the drop shadow.
const SHADOW_INSET: f32 = 10.0;

struct CustomChrome;

impl CustomChrome {
    fn button(
        id: &'static str,
        label: &'static str,
        on_click: impl Fn(&mut Window) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xbbbbbb))
            .bg(rgb(0xffffff))
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(rgb(0xeeeeee)))
            .active(|style| style.bg(rgb(0xdddddd)))
            .on_click(move |_, window, _| on_click(window))
            .child(label)
    }

    fn describe(window: &Window) -> SharedString {
        let decorations = match window.window_decorations() {
            Decorations::Server => "server-side (the compositor draws the frame)".to_string(),
            Decorations::Client { tiling } => format!(
                "client-side (tiled: top {} left {} right {} bottom {})",
                tiling.top, tiling.left, tiling.right, tiling.bottom
            ),
        };
        let capabilities = window.background_capabilities();
        format!(
            "Decorations: {decorations}\n\
             Maximized: {}   Fullscreen: {}\n\
             Transparency available: {}   Blur available: {}\n\
             Background: requested {:?}, effective {:?}",
            window.is_maximized(),
            window.is_fullscreen(),
            capabilities.transparency,
            capabilities.blur,
            window.requested_background_appearance(),
            window.effective_background_appearance(),
        )
        .into()
    }
}

impl Render for CustomChrome {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let client_decorated = window.is_client_decorated();
        if client_decorated {
            // The inset is the room around the frame for the shadow. The window zeroes it per
            // edge for tiled, maximized and fullscreen states and for the opaque fallback.
            window.set_client_inset(px(SHADOW_INSET));
        }
        let inset = window.effective_client_inset();
        let radii = window.window_corner_radii();

        let surface: Hsla = rgb(0xf3f3f3).into();
        let opaque_background: Hsla = rgb(0xfafafa).into();
        let background =
            window.resolve_window_background(hsla(0.0, 0.0, 0.98, 0.85), opaque_background);

        let titlebar = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(38.0))
            .w_full()
            .flex_none()
            .bg(surface)
            .window_drag_region()
            .child(CaptionControls::new(CaptionControlsSide::Left))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .text_color(rgb(0x333333))
                    .child("Custom chrome"),
            )
            .child(CaptionControls::new(CaptionControlsSide::Right));

        let body = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .text_color(rgb(0x222222))
            .child(Self::describe(window))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(Self::button("server", "Request server-side", |window| {
                        window.request_decorations(WindowDecorations::Server)
                    }))
                    .child(Self::button("client", "Request client-side", |window| {
                        window.request_decorations(WindowDecorations::Client)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(Self::button("opaque", "Opaque background", |window| {
                        window.set_background_appearance(WindowBackgroundAppearance::Opaque)
                    }))
                    .child(Self::button("blurred", "Blurred background", |window| {
                        window.set_background_appearance(WindowBackgroundAppearance::Blurred)
                    })),
            );

        div()
            .size_full()
            .bg(transparent_black())
            // Resize hit areas exist only where the compositor does not own the edge.
            .child(window_resize_handles())
            .child(
                div()
                    .size_full()
                    .pt(inset.top)
                    .pr(inset.right)
                    .pb(inset.bottom)
                    .pl(inset.left)
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .cursor(CursorStyle::Arrow)
                            .bg(background)
                            .rounded_tl(radii.top_left)
                            .rounded_tr(radii.top_right)
                            .rounded_br(radii.bottom_right)
                            .rounded_bl(radii.bottom_left)
                            .when(client_decorated, |frame| {
                                frame.border_1().border_color(rgb(0x999999)).when(
                                    inset.top > px(0.0),
                                    |frame| {
                                        frame.shadow(vec![
                                            BoxShadow::new(
                                                px(0.0),
                                                px(0.0),
                                                hsla(0.0, 0.0, 0.0, 0.4),
                                            )
                                            .blur_radius(px(SHADOW_INSET / 2.0)),
                                        ])
                                    },
                                )
                            })
                            .when(client_decorated, |frame| frame.child(titlebar))
                            .child(body),
                    ),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(640.0), px(420.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Opaque,
                window_decorations: Some(WindowDecorations::Client),
                window_min_size: Some(size(px(320.0), px(200.0))),
                app_id: Some("vespertine.custom_chrome".to_string()),
                ..Default::default()
            },
            |_, cx| cx.new(|_| CustomChrome),
        )
        .unwrap();
        cx.on_window_closed(|cx, _window_id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
