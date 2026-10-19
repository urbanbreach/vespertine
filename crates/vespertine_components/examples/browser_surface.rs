//! CMP-022 A1: the embedded browser surface in an 800 x 480 window.
//!
//! The window (background #181A1F) hosts a toolbar 720 x 40 at (40, 40) filled #30343D with an
//! address field 540 x 24 at (140, 48) filled #181A1F (radius 4, text 12 px on 20 px lines in
//! #F0F1F3), a page viewport 720 x 340 at (40, 80) showing a local test page ("Embedded page" in
//! bold 24 px #20242C on white, served from a loopback listener of this process because files are
//! rejected by the scheme policy) and a floating overlay 180 x 80 at (520, 100) filled #405470
//! with radius 6 and the text "Overlay". The address field shows the real address of the local
//! page.
//!
//! Without arguments the example runs interactively. With `BROWSER_SURFACE_SCRIPT=1` it runs the
//! acceptance script and exits 0 when every check passes (1 otherwise): it navigates the local
//! page, types "日本語" into a page field through the page's text-input handler (preedit, then
//! commit), opens the overlay over the page (the overlay receives clicks, the page does not),
//! resizes the hosting pane several times (the page's CSS viewport width stays within 2 pixels of
//! the pane's logical width), shows a tooltip over the page (the page field keeps its focus) and
//! finally checks that the page still accepts input.

use std::cell::Cell;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vespertine::{
    AnyView, App, AppContext, AsyncApp, Bounds, Context, Entity, EntityInputHandler, Hsla,
    InteractiveElement, IntoElement, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, ParentElement, PlatformInput, Render, StatefulInteractiveElement, Styled, Window,
    WindowBounds, WindowHandle, WindowOptions, div, point, prelude::FluentBuilder, px, rgb, size,
};
use vespertine_components::browser::{
    BrowserMetrics, BrowserSurface, BrowserTheme, Presentation, bind_keys,
};
use vespertine_platform::application;
use vespertine_webview::BrowserContext;

const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Embedded page</title>
<style>
  html, body { margin: 0; background: #ffffff; }
  h1 { margin: 20px 24px 12px; font: bold 24px sans-serif; color: #20242C; }
  input { display: block; margin: 8px 24px; width: 360px; font: 16px sans-serif; padding: 4px; }
</style></head>
<body>
<h1>Embedded page</h1>
<input id="field" autofocus>
<script>
  var field = document.getElementById('field');
  var clicks = 0;
  field.focus();
  document.addEventListener('mousedown', function () { clicks += 1; });
  function report() {
    fetch('/report?w=' + window.innerWidth + '&c=' + clicks + '&f=' +
          (document.activeElement === field ? 1 : 0) + '&v=' + encodeURIComponent(field.value),
          { cache: 'no-store' }).catch(function () {});
  }
  setInterval(report, 100);
  report();
</script>
</body></html>"#;

/// What the local page last reported.
#[derive(Clone, Debug, Default)]
struct Report {
    reports: u64,
    width: f64,
    clicks: u32,
    focused: bool,
    value: String,
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[index + 1..index + 3], 16)
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn serve(mut stream: TcpStream, report: &Arc<Mutex<Report>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buffer = [0u8; 4096];
    let Ok(count) = stream.read(&mut buffer) else {
        return;
    };
    let request = String::from_utf8_lossy(&buffer[..count]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    if let Some(query) = target.strip_prefix("/report?") {
        let mut current = report.lock().unwrap();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "w" => current.width = value.parse().unwrap_or(0.0),
                "c" => current.clicks = value.parse().unwrap_or(0),
                "f" => current.focused = value == "1",
                "v" => current.value = percent_decode(value),
                _ => {}
            }
        }
        current.reports += 1;
        drop(current);
        let _ = stream.write_all(
            b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        );
    } else {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\
             Content-Length: {}\r\n\r\n{PAGE}",
            PAGE.len()
        );
        let _ = stream.write_all(response.as_bytes());
    }
}

/// Serves the test page on a loopback port until the process exits.
fn start_server(report: Arc<Mutex<Report>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let address = listener.local_addr().expect("local address");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let report = report.clone();
            std::thread::spawn(move || serve(stream, &report));
        }
    });
    address
}

struct Tooltip;

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(rgb(0x30343D))
            .text_color(rgb(0xF0F1F3))
            .text_size(px(11.))
            .child("A tooltip over the page")
    }
}

struct Example {
    surface: Entity<BrowserSurface>,
    pane_width: f32,
    overlay: bool,
    overlay_clicks: Rc<Cell<u32>>,
    script: bool,
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.overlay_clicks.clone();
        div()
            .size_full()
            .relative()
            .bg(rgb(0x181A1F))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .w(px(self.pane_width))
                    .h(px(380.))
                    .child(self.surface.clone()),
            )
            // A passive element over the page whose tooltip must not blur the page.
            .child(
                div()
                    .id("badge")
                    .absolute()
                    .left(px(60.))
                    .top(px(300.))
                    .size(px(24.))
                    .tooltip(|_, cx| AnyView::from(cx.new(|_| Tooltip))),
            )
            .when(self.overlay || !self.script, |root| {
                root.child(
                    div()
                        .id("overlay")
                        .absolute()
                        .left(px(520.))
                        .top(px(100.))
                        .w(px(180.))
                        .h(px(80.))
                        .rounded(px(6.))
                        .bg(rgb(0x405470))
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0xFFFFFF))
                        .text_size(px(14.))
                        .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                        .child("Overlay"),
                )
            })
    }
}

/// The A1 look: toolbar 720 x 40 filled #30343D, address field 540 x 24 at x = 140 filled
/// #181A1F with radius 4 and 12 px text on 20 px lines in #F0F1F3.
fn a1_theme() -> (BrowserTheme, BrowserMetrics) {
    let theme = BrowserTheme {
        raised: rgb(0x30343D).into(),
        page_background: rgb(0x181A1F).into(),
        text: rgb(0xF0F1F3).into(),
        border: Hsla::transparent_black(),
        ..BrowserTheme::default()
    };
    let metrics = BrowserMetrics {
        toolbar_height: px(40.),
        address_height: px(24.),
        address_radius: px(4.),
        address_text_size: px(12.),
        address_line_height: px(20.),
        // 8 + 3 buttons of 28 and gaps of 4 end at x = 104 of the toolbar; the field starts at 100
        // + 40 = 140 of the window and ends 40 before the toolbar's right padding and button.
        address_margin_left: px(36.),
        address_margin_right: px(40.),
        ..BrowserMetrics::default()
    };
    (theme, metrics)
}

// ---- the acceptance script ----------------------------------------------------------------------

struct Script {
    cx: AsyncApp,
    window: WindowHandle<Example>,
    surface: Entity<BrowserSurface>,
    report: Arc<Mutex<Report>>,
    overlay_clicks: Rc<Cell<u32>>,
    failures: Vec<String>,
}

impl Script {
    async fn sleep(&self, duration: Duration) {
        self.cx.background_executor().timer(duration).await;
    }

    fn check(&mut self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_owned());
        }
    }

    fn report(&self) -> Report {
        self.report.lock().unwrap().clone()
    }

    /// Polls `condition` every 50 ms for up to `seconds`.
    async fn wait(&self, seconds: u64, mut condition: impl FnMut(&Script) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if condition(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            self.sleep(Duration::from_millis(50)).await;
        }
    }

    fn with_window<R>(
        &mut self,
        f: impl FnOnce(&mut Example, &mut Window, &mut Context<Example>) -> R,
    ) -> R {
        self.window
            .update(&mut self.cx, |example, window, cx| f(example, window, cx))
            .expect("the window is open")
    }

    fn click(&mut self, x: f32, y: f32) {
        let position = point(px(x), px(y));
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                for event in [
                    PlatformInput::MouseMove(vespertine::MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::none(),
                    }),
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::none(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::none(),
                        click_count: 1,
                    }),
                ] {
                    window.dispatch_event(event, cx);
                }
            })
            .expect("the window is open");
    }

    fn page(&mut self) -> Entity<vespertine_webview::WebPage> {
        self.surface
            .read_with(&self.cx, |surface, _| surface.page().clone())
    }

    async fn run(&mut self, address: SocketAddr) {
        // 1. Navigate the local page.
        let url = format!("127.0.0.1:{}", address.port());
        let surface = self.surface.clone();
        let accepted = self.with_window(|_, window, cx| {
            surface.update(cx, |surface, cx| {
                surface.set_presentation(Presentation::Live, window, cx);
                surface.navigate(&url, window, cx)
            })
        });
        self.check(accepted, "the local address is accepted");
        let loaded = self.wait(30, |s| s.report().reports > 0).await;
        self.check(loaded, "the local page loaded inside the surface");
        if !loaded {
            return;
        }
        let state = self
            .surface
            .read_with(&self.cx, |surface, _| surface.state.clone());
        self.check(
            state.title == "Embedded page" || state.url.is_some(),
            "the surface reports the page state",
        );
        let focused = self.wait(10, |s| s.report().focused).await;
        self.check(focused, "the page field has focus inside the page");

        // 2. Non-Latin text through preedit and commit.
        let page = self.page();
        let marked = self.with_window(|_, window, cx| {
            page.update(cx, |page, cx| {
                page.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
                page.replace_and_mark_text_in_range(None, "日本語", None, window, cx);
                page.marked_text_range(window, cx)
            })
        });
        self.check(
            marked == Some(0..3),
            "the marked range covers the preedit 日本語",
        );
        self.sleep(Duration::from_millis(300)).await;
        self.with_window(|_, window, cx| {
            page.update(cx, |page, cx| {
                page.replace_text_in_range(None, "日本語", window, cx);
            })
        });
        let typed = self
            .wait(10, |s| s.report().value.ends_with("日本語"))
            .await;
        let value = self.report().value;
        self.check(
            typed,
            &format!("the committed text reached the page field ({value:?})"),
        );

        // 3. The overlay covers page pixels and receives clicks; the page does not.
        self.with_window(|example, _, cx| {
            example.overlay = true;
            cx.notify();
        });
        self.sleep(Duration::from_millis(400)).await;
        let clicks_before = self.report().clicks;
        self.click(610., 140.);
        self.sleep(Duration::from_millis(500)).await;
        self.check(
            self.overlay_clicks.get() == 1,
            "the overlay received the click",
        );
        self.check(
            self.report().clicks == clicks_before,
            "the page did not receive the click",
        );
        self.click(100., 400.);
        let delivered = self.wait(5, |s| s.report().clicks > clicks_before).await;
        self.check(delivered, "a click outside the overlay reaches the page");

        // 4. Resize the hosting pane several times.
        for width in [600.0_f32, 480., 700., 720.] {
            self.with_window(|example, _, cx| {
                example.pane_width = width;
                cx.notify();
            });
            let followed = self
                .wait(10, |s| (s.report().width - f64::from(width)).abs() <= 2.0)
                .await;
            let reported = self.report().width;
            self.check(
                followed,
                &format!("the page viewport follows the pane: {width} -> {reported}"),
            );
        }

        // 5. A tooltip over the page leaves the page field focused. The click above blurred it
        // inside the page, so focus it again first.
        let page_for_focus = self.page();
        self.with_window(|_, _, cx| {
            page_for_focus.update(cx, |page, _| {
                page.send_command(vespertine_webview::protocol::Command::Eval {
                    script: "document.getElementById('field').focus()".to_owned(),
                })
            })
        });
        let refocused = self.wait(10, |s| s.report().focused).await;
        self.check(refocused, "the page field was focused again");
        let badge = point(px(72.), px(312.));
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(vespertine::MouseMoveEvent {
                        position: badge,
                        pressed_button: None,
                        modifiers: Modifiers::none(),
                    }),
                    cx,
                );
            })
            .expect("the window is open");
        self.sleep(Duration::from_millis(1200)).await;
        let page_focus = self.page();
        let still_focused =
            self.with_window(|_, window, cx| page_focus.read(cx).focus_handle().is_focused(window));
        self.check(
            still_focused,
            "the page keeps keyboard focus while a tooltip shows",
        );
        self.check(
            self.report().focused,
            "the page field keeps focus inside the page",
        );

        // 6. The page still accepts input.
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse("x").expect("keystroke"), cx);
            })
            .expect("the window is open");
        let accepted = self
            .wait(10, |s| s.report().value.ends_with("日本語x"))
            .await;
        let value = self.report().value;
        self.check(
            accepted,
            &format!("the page still accepts input ({value:?})"),
        );
    }
}

fn run_script(
    cx: &mut App,
    window: WindowHandle<Example>,
    surface: Entity<BrowserSurface>,
    report: Arc<Mutex<Report>>,
    overlay_clicks: Rc<Cell<u32>>,
    address: SocketAddr,
) {
    cx.spawn(async move |cx| {
        let mut script = Script {
            cx: cx.clone(),
            window,
            surface,
            report,
            overlay_clicks,
            failures: Vec::new(),
        };
        // Give the window a moment to map.
        script.sleep(Duration::from_millis(500)).await;
        script.run(address).await;
        let code = if script.failures.is_empty() {
            println!("browser_surface: all checks passed");
            0
        } else {
            println!("browser_surface: {} check(s) failed", script.failures.len());
            1
        };
        std::process::exit(code);
    })
    .detach();
}

fn main() {
    let script = std::env::var_os("BROWSER_SURFACE_SCRIPT").is_some_and(|value| value == "1");
    let report = Arc::new(Mutex::new(Report::default()));
    let address = start_server(report.clone());
    application().run(move |cx: &mut App| {
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(800.), px(480.)), cx);
        let overlay_clicks = Rc::new(Cell::new(0));
        let clicks = overlay_clicks.clone();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |window, cx| {
                    let surface = cx.new(|cx| {
                        let mut surface =
                            BrowserSurface::new(BrowserContext::ephemeral(), window, cx);
                        let (theme, metrics) = a1_theme();
                        surface.set_theme(theme, cx);
                        surface.set_metrics(metrics, cx);
                        surface
                    });
                    surface.update(cx, |surface, cx| {
                        let presentation = Presentation::for_host(window.is_window_active(), false);
                        surface.set_presentation(presentation, window, cx);
                    });
                    cx.new(|cx| {
                        if !script {
                            // The host's activity decides the surface's presentation (R16).
                            let surface = surface.clone();
                            cx.observe_window_activation(window, move |_, window, cx| {
                                let presentation =
                                    Presentation::for_host(window.is_window_active(), false);
                                surface.update(cx, |surface, cx| {
                                    surface.set_presentation(presentation, window, cx)
                                });
                            })
                            .detach();
                        }
                        Example {
                            surface,
                            pane_width: 720.,
                            overlay: false,
                            overlay_clicks: clicks,
                            script,
                        }
                    })
                },
            )
            .expect("open the window");
        let surface = window
            .read_with(cx, |example, _| example.surface.clone())
            .expect("window state");
        if script {
            run_script(cx, window, surface, report.clone(), overlay_clicks, address);
        } else {
            let url = format!("127.0.0.1:{}", address.port());
            window
                .update(cx, |example, window, cx| {
                    example.surface.update(cx, |surface, cx| {
                        surface.navigate(&url, window, cx);
                    });
                })
                .expect("window state");
        }
        cx.activate(true);
    });
}
