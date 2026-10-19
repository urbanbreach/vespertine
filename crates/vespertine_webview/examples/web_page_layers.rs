//! PLT-015 A10: layering of framework content around an embedded page.
//!
//! The page is an ordinary scene primitive, so a dropdown menu (a blocking overlay) and a frosted
//! dialog with a scrim (a backdrop-blurred overlay that samples the page) paint above it, work the
//! same on Wayland and X11, and take pointer input away from the page. The example plays the
//! checks through the window's own input path and exits 0 when they pass:
//!
//! * a click on a page button counts once;
//! * with the menu open, pointer events inside the menu never reach the page: the click is
//!   handled by the menu, the page counter stays, and the page's hover ends;
//! * with the frosted dialog and scrim open, a click on the scrim closes it and the page stays
//!   untouched;
//! * showing a tooltip over the page changes neither the page's keyboard focus nor a pending
//!   press: pressing on the page, hovering the tooltip target and releasing completes one click.
//!
//! `WEB_PAGE_INTERACTIVE=1` keeps the window open; press `m` for the menu, `d` for the dialog.

#[cfg(target_os = "linux")]
mod linux {
    use serde_json::Value;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    use vespertine::{
        AnyView, App, AppContext, AsyncApp, Bounds, Context, Corners, Entity, IntoElement,
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
        PlatformInput, Render, SharedString, Styled, Window, WindowBounds, WindowHandle,
        WindowOptions, canvas, deferred, div, hsla, point, prelude::*, px, rgb, size,
    };
    use vespertine_platform::application;
    use vespertine_webview::protocol::Command;
    use vespertine_webview::{BrowserContext, Presentation, WebPage, WebPageEvent, page_element};

    const PAGE: &str = r##"<!doctype html><meta charset=utf-8><title>Layers</title>
<style>body{margin:0;background:#fff;font:16px sans-serif}
button{position:absolute;left:300px;top:150px;width:160px;height:60px;font-size:18px}
#f{position:absolute;left:20px;top:20px;width:260px;font-size:16px}
#hover{position:absolute;left:20px;top:80px}</style>
<input id=f><button id=inc onclick="window.counter=(window.counter||0)+1">count</button>
<div id=hover>stripes</div>
<script>window.counter=0;</script>"##;

    fn serve(listener: TcpListener) {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{PAGE}",
                    PAGE.len()
                );
            });
        }
    }

    struct Layers {
        page: Entity<WebPage>,
        waiters: Vec<futures::channel::oneshot::Sender<Value>>,
        menu_open: bool,
        dialog_open: bool,
        menu_clicks: u32,
        scrim_clicks: u32,
    }

    struct Tip;

    impl Render for Tip {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .px(px(8.))
                .py(px(4.))
                .bg(rgb(0x30343D))
                .text_color(rgb(0xF0F1F3))
                .child(SharedString::from("A tooltip over the page"))
        }
    }

    impl Render for Layers {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut root = div()
                .relative()
                .size_full()
                .bg(rgb(0x181A1F))
                .child(
                    div()
                        .absolute()
                        .left(px(20.))
                        .top(px(20.))
                        .w(px(720.))
                        .h(px(420.))
                        .child(page_element(&self.page, cx)),
                )
                // A tooltip target over the page that does not block it.
                .child(
                    div()
                        .id("tip-target")
                        .absolute()
                        .left(px(700.))
                        .top(px(30.))
                        .w(px(30.))
                        .h(px(30.))
                        .tooltip(|_, cx| AnyView::from(cx.new(|_| Tip))),
                );
            if self.menu_open {
                root = root.child(deferred(
                    div()
                        .id("menu")
                        .absolute()
                        .left(px(300.))
                        .top(px(150.))
                        .w(px(220.))
                        .h(px(140.))
                        .bg(rgb(0x405470))
                        .text_color(rgb(0xFFFFFF))
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.menu_clicks += 1;
                                cx.notify();
                            }),
                        )
                        .child("Menu"),
                ));
            }
            if self.dialog_open {
                let blur = canvas(
                    |_, _, _| (),
                    |bounds: Bounds<_>, _, window, _| {
                        window.paint_backdrop_blur(bounds, Corners::all(px(12.)), px(12.));
                    },
                )
                .absolute()
                .size_full();
                root = root.child(deferred(
                    div()
                        .id("scrim")
                        .absolute()
                        .size_full()
                        .bg(hsla(0., 0., 0., 0.4))
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.scrim_clicks += 1;
                                this.dialog_open = false;
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(200.))
                                .top(px(120.))
                                .w(px(360.))
                                .h(px(220.))
                                .child(blur)
                                .child(
                                    div()
                                        .absolute()
                                        .size_full()
                                        .rounded(px(12.))
                                        .bg(hsla(0., 0., 1., 0.15))
                                        .text_color(rgb(0xFFFFFF))
                                        .child("Frosted dialog"),
                                ),
                        ),
                ));
            }
            root
        }
    }

    struct Script {
        handle: WindowHandle<Layers>,
        cx: AsyncApp,
        failures: Vec<String>,
    }

    impl Script {
        fn check(&mut self, ok: bool, what: &str) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
            if !ok {
                self.failures.push(what.to_owned());
            }
        }

        async fn sleep(&self, millis: u64) {
            self.cx
                .background_executor()
                .timer(Duration::from_millis(millis))
                .await;
        }

        fn page(&self) -> Entity<WebPage> {
            self.handle
                .read_with(&self.cx, |layers, _| layers.page.clone())
                .expect("window")
        }

        async fn eval(&self, script: &str) -> Value {
            let (sender, mut receiver) = futures::channel::oneshot::channel();
            let page = self.page();
            self.handle
                .update(&mut self.cx.clone(), |layers, _, cx| {
                    layers.waiters.push(sender);
                    page.update(cx, |page, _| {
                        page.send_command(Command::Eval {
                            script: script.to_owned(),
                        })
                    });
                })
                .expect("window");
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(Some(value)) = receiver.try_recv() {
                    return value;
                }
                if Instant::now() > deadline {
                    return Value::Null;
                }
                self.sleep(10).await;
            }
        }

        async fn until(&self, script: &str, accept: impl Fn(&Value) -> bool) -> bool {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if accept(&self.eval(script).await) {
                    return true;
                }
                self.sleep(50).await;
            }
            false
        }

        fn dispatch(&self, event: PlatformInput) {
            self.cx
                .clone()
                .update_window(self.handle.into(), |_, window, cx| {
                    window.dispatch_event(event, cx);
                })
                .expect("window");
        }

        fn mouse_move(&self, x: f32, y: f32) {
            self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(x), px(y)),
                pressed_button: None,
                modifiers: Modifiers::none(),
            }));
        }

        fn press(&self, x: f32, y: f32) {
            self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(x), px(y)),
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            }));
        }

        fn release(&self, x: f32, y: f32) {
            self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(x), px(y)),
                modifiers: Modifiers::none(),
                click_count: 1,
            }));
        }

        fn click(&self, x: f32, y: f32) {
            self.mouse_move(x, y);
            self.press(x, y);
            self.release(x, y);
        }

        fn set(&self, f: impl FnOnce(&mut Layers) + 'static) {
            self.handle
                .update(&mut self.cx.clone(), |layers, _, cx| {
                    f(layers);
                    cx.notify();
                })
                .expect("window");
        }

        fn read<R: 'static>(&self, f: impl FnOnce(&Layers) -> R) -> R {
            self.handle
                .read_with(&self.cx, |layers, _| f(layers))
                .expect("window")
        }

        async fn run(mut self) -> Vec<String> {
            let page = self.page();
            let mut loaded = false;
            for _ in 0..600 {
                let state = self.cx.update(|cx| page.read(cx).state().clone());
                if !state.loading && state.title == "Layers" {
                    loaded = true;
                    break;
                }
                self.sleep(50).await;
            }
            self.check(loaded, "the local page loads");
            if !loaded {
                return self.failures;
            }
            self.sleep(300).await;
            // The button is at page (300..460, 150..210); the page element sits at (20, 20).
            let (button_x, button_y) = (20.0 + 380.0, 20.0 + 180.0);

            self.click(button_x, button_y);
            let counted = self
                .until("window.counter", |v| v.as_i64() == Some(1))
                .await;
            self.check(counted, "a click on the page counts once");

            // A dropdown menu over the button: the page receives nothing there.
            self.mouse_move(button_x, button_y);
            self.until("document.querySelector('#inc').matches(':hover')", |v| {
                v == true
            })
            .await;
            self.set(|layers| layers.menu_open = true);
            self.sleep(300).await;
            self.mouse_move(button_x + 2.0, button_y + 2.0);
            self.press(button_x + 2.0, button_y + 2.0);
            self.release(button_x + 2.0, button_y + 2.0);
            self.sleep(400).await;
            let menu_clicks = self.read(|layers| layers.menu_clicks);
            self.check(
                menu_clicks == 1,
                "the click inside the menu is handled by the menu",
            );
            let counter = self.eval("window.counter").await;
            self.check(
                counter == 1,
                "the page did not receive the click inside the menu",
            );
            let hovered = self
                .eval("document.querySelector('#inc').matches(':hover')")
                .await;
            self.check(
                hovered == false,
                "the page's hover ended when the menu covered the pointer",
            );
            self.set(|layers| layers.menu_open = false);

            // A frosted dialog with a scrim above the page: a click on the scrim closes it.
            self.set(|layers| layers.dialog_open = true);
            self.sleep(300).await;
            self.click(button_x, button_y);
            self.sleep(400).await;
            let (scrim_clicks, still_open) =
                self.read(|layers| (layers.scrim_clicks, layers.dialog_open));
            self.check(
                scrim_clicks == 1 && !still_open,
                "the scrim handles the click and closes the dialog",
            );
            let counter = self.eval("window.counter").await;
            self.check(
                counter == 1,
                "the page did not receive the click on the scrim",
            );

            // A tooltip over the page changes neither focus nor a pending press.
            self.sleep(200).await;
            self.click(20.0 + 100.0, 20.0 + 30.0);
            let focused = self
                .until("document.activeElement && document.activeElement.id", |v| {
                    v == "f"
                })
                .await;
            self.check(focused, "the page field has focus");
            let focus_events_before = self
                .cx
                .update(|cx| page.read(cx).native_page().map(|p| p.input_state().focused));
            self.mouse_move(button_x, button_y);
            self.press(button_x, button_y);
            // Move onto the tooltip target and wait for the tooltip to show.
            self.mouse_move(20.0 + 715.0, 20.0 + 25.0 + 10.0);
            self.sleep(1500).await;
            let still_focused = self
                .eval("document.activeElement && document.activeElement.id")
                .await;
            self.check(
                still_focused == "inc",
                "the press moved page focus to the button and the tooltip did not take it away",
            );
            self.mouse_move(button_x, button_y);
            self.release(button_x, button_y);
            let clicked = self
                .until("window.counter", |v| v.as_i64() == Some(2))
                .await;
            self.check(
                clicked,
                "the pending press completed one click after the tooltip appeared",
            );
            let _ = focus_events_before;
            let _ = size(px(0.), px(0.));
            self.failures
        }
    }

    pub fn run() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("address").port();
        std::thread::spawn(move || serve(listener));
        let interactive = std::env::var_os("WEB_PAGE_INTERACTIVE").is_some();
        application().run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(760.), px(460.)), cx);
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |window, cx| {
                        let page =
                            cx.new(|cx| WebPage::new(BrowserContext::ephemeral(), window, cx));
                        cx.new(|cx| {
                            cx.subscribe(
                                &page,
                                |layers: &mut Layers, _, event: &WebPageEvent, _| {
                                    if let WebPageEvent::EvalResult(text) = event
                                        && !layers.waiters.is_empty()
                                    {
                                        let value =
                                            serde_json::from_str(text).unwrap_or(Value::Null);
                                        let _ = layers.waiters.remove(0).send(value);
                                    }
                                },
                            )
                            .detach();
                            Layers {
                                page: page.clone(),
                                waiters: Vec::new(),
                                menu_open: false,
                                dialog_open: false,
                                menu_clicks: 0,
                                scrim_clicks: 0,
                            }
                        })
                    },
                )
                .expect("open the window");
            cx.activate(true);
            let page = handle
                .read_with(cx, |layers, _| layers.page.clone())
                .expect("window");
            handle
                .update(cx, |_, window, cx| {
                    page.update(cx, |page, cx| {
                        page.set_presentation(Presentation::Live, window, cx);
                        page.navigate_to(format!("http://127.0.0.1:{port}/"), window, cx);
                    });
                })
                .expect("window");
            if interactive {
                return;
            }
            cx.spawn(async move |cx| {
                let script = Script {
                    handle,
                    cx: cx.clone(),
                    failures: Vec::new(),
                };
                let failures = script.run().await;
                let code = i32::from(!failures.is_empty());
                if code != 0 {
                    eprintln!("failed checks: {failures:?}");
                }
                std::process::exit(code);
            })
            .detach();
        });
    }
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("the embedded web view runs on Linux only");
}
