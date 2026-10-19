//! CMP-026 A1: an animated procedural image driven by time and the pointer while a text input in
//! the same window stays responsive.
//!
//! The window shows a shader surface (a field of glowing points that follow the pointer and
//! orbit with time, rendered with bloom) and a text input. A script types into the input while
//! the surface renders; every frame is made slow on purpose (set `SHADER_PLAYGROUND_FAST=1` to
//! drop that), so the check is meaningful: frames render off the foreground thread, and the time
//! between a keystroke and the input showing it stays small. It prints the longest frame, the
//! longest typing latency and the longest stall of the foreground executor, and exits with
//! status 1 when the foreground stalled for more than 250 ms. Without a GPU adapter the surface
//! reports "no GPU is available" and the example shows that error instead.
//!
//! `SHADER_PLAYGROUND_EXIT=1` exits after the script; otherwise the window stays open.

use std::time::{Duration, Instant};
use vespertine::{
    App, AppContext, Bounds, Context, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, Keystroke, ParentElement, Render, Styled, Window, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use vespertine_components::scene3d::SceneSurfaces;
use vespertine_components::shader::{BlendMode, ShaderSurface};
use vespertine_platform::application;

const FAST: &str = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    vec2 pointer = iMouse.z >= 0.0 ? iMouse.zw : iMouse.xy;
    vec2 m = (pointer - 0.5 * iResolution.xy) / iResolution.y;
    vec3 col = vec3(0.0);
    for (int i = 0; i < 48; i++) {
        float fi = float(i);
        vec2 p = 0.35 * vec2(cos(iTime * 0.7 + fi * 0.41), sin(iTime * 0.9 + fi * 0.57)) + m * 0.4;
        float d = length(uv - p);
        col += 0.0016 / (d * d + 0.0004) * (0.5 + 0.5 * cos(vec3(0.0, 2.0, 4.0) + fi * 0.3 + iTime));
    }
    fragColor = vec4(col, 1.0);
}";

/// The same field with an extra loop that makes every frame slow.
const SLOW: &str = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    vec2 pointer = iMouse.z >= 0.0 ? iMouse.zw : iMouse.xy;
    vec2 m = (pointer - 0.5 * iResolution.xy) / iResolution.y;
    vec3 col = vec3(0.0);
    for (int i = 0; i < 48; i++) {
        float fi = float(i);
        vec2 p = 0.35 * vec2(cos(iTime * 0.7 + fi * 0.41), sin(iTime * 0.9 + fi * 0.57)) + m * 0.4;
        float d = length(uv - p);
        col += 0.0016 / (d * d + 0.0004) * (0.5 + 0.5 * cos(vec3(0.0, 2.0, 4.0) + fi * 0.3 + iTime));
    }
    float burn = 0.0;
    for (int k = 0; k < 6000; k++) {
        burn += sin(float(k) * 0.001 + uv.x) * 0.0000001;
    }
    fragColor = vec4(col + burn, 1.0);
}";

struct Page {
    focus: FocusHandle,
    text: String,
    pending_since: Option<Instant>,
    latencies: Vec<Duration>,
    program: &'static str,
}

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(since) = self.pending_since.take() {
            self.latencies.push(since.elapsed());
        }
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        div()
            .size_full()
            .flex_col()
            .flex()
            .gap_2()
            .p_2()
            .bg(rgb(0x101216))
            .text_color(rgb(0xF0F1F3))
            .child(
                div()
                    .id("input")
                    .track_focus(&self.focus)
                    .h(px(28.))
                    .px_2()
                    .bg(rgb(0x20242C))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if let Some(text) = &event.keystroke.key_char {
                            this.text.push_str(text);
                            this.pending_since = Some(Instant::now());
                            cx.notify();
                        }
                    }))
                    .child(format!("typed: {}", self.text)),
            )
            .child(
                ShaderSurface::new("playground", self.program)
                    .blend(BlendMode::Light)
                    .bloom(0.8)
                    .animate(true)
                    .height(px(380.))
                    .on_error(move |message| eprintln!("shader_playground: {message}")),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(720.), px(480.)), cx);
        let slow = std::env::var_os("SHADER_PLAYGROUND_FAST").is_none();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |_, cx| {
                    cx.new(|cx| Page {
                        focus: cx.focus_handle(),
                        text: String::new(),
                        pending_since: None,
                        latencies: Vec::new(),
                        program: if slow { SLOW } else { FAST },
                    })
                },
            )
            .unwrap();
        let page = window.entity(cx).unwrap();
        let window_id = window.window_id();
        let any: vespertine::AnyWindowHandle = window.into();
        cx.spawn(async move |cx| {
            let wait = |cx: &mut vespertine::AsyncApp, ms| {
                cx.background_executor().timer(Duration::from_millis(ms))
            };
            wait(cx, 500).await;
            // Foreground stall monitor: a timer that should fire every 16 ms.
            let mut max_stall = Duration::ZERO;
            let started = Instant::now();
            let mut typed = 0;
            let mut next_key = Instant::now();
            while started.elapsed() < Duration::from_secs(5) {
                let before = Instant::now();
                wait(cx, 16).await;
                let stall = before.elapsed().saturating_sub(Duration::from_millis(16));
                max_stall = max_stall.max(stall);
                if Instant::now() >= next_key && typed < 20 {
                    let key = (b'a' + typed as u8 % 26) as char;
                    any.update(cx, |_, window, cx| {
                        window.dispatch_keystroke(Keystroke::parse(&key.to_string()).unwrap(), cx);
                    })
                    .ok();
                    typed += 1;
                    next_key = Instant::now() + Duration::from_millis(150);
                }
            }
            let (latency, text) = page.read_with(cx, |page, _| {
                (
                    page.latencies.iter().copied().max().unwrap_or_default(),
                    page.text.clone(),
                )
            });
            let handle = cx.update(|cx| SceneSurfaces::get(cx, window_id, &ElementId::from("playground")));
            let (frames, frame_time, error) = match &handle {
                Some(handle) => (
                    handle.displayed_sequence().unwrap_or(0),
                    handle.last_render_time().unwrap_or_default(),
                    handle.last_error(),
                ),
                None => (0, Duration::ZERO, None),
            };
            println!(
                "shader_playground: typed {text:?}; frames displayed (sequence) {frames}; last frame took {frame_time:?}; longest typing latency {latency:?}; longest foreground stall {max_stall:?}{}",
                error.map(|e| format!("; surface error: {e}")).unwrap_or_default()
            );
            if max_stall > Duration::from_millis(250) {
                eprintln!("shader_playground: FAILED: the foreground stalled for {max_stall:?}");
                std::process::exit(1);
            }
            if std::env::var_os("SHADER_PLAYGROUND_EXIT").is_some() {
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
        cx.activate(true);
    });
}
