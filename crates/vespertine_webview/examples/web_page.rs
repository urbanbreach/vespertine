//! PLT-016 A7/A8/A11: an embedded page with a text field, driven through the framework's own
//! input path.
//!
//! The example serves a local test page from a loopback thread and shows it in a window with the
//! real WebKitGTK helper. It then plays the scenario of the specification through the window:
//! it clicks the text field (a real pointer event), types "hello linux" with individual
//! keystrokes, pastes " — café 日本語" with control+V, selects all and copies with control+C
//! (the system clipboard equals the field value), copies with no selection (the clipboard stays),
//! composes "にほ" -> "日本語" -> commit through the page element's text-input handler (marked range
//! 0..3 while marked, none after the commit), checks that password text never reaches the
//! application, and that wheel input scrolls and clicks count exactly once while the page keeps
//! animating. It exits with status 0 when every check passes and 1 otherwise. Set
//! `WEB_PAGE_INTERACTIVE=1` to skip the script and use the page by hand.

#[cfg(target_os = "linux")]
mod linux {
    use serde_json::Value;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    use vespertine::{
        App, AppContext, AsyncApp, Bounds, ClipboardItem, Context, Entity, EntityInputHandler,
        IntoElement, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, ParentElement, PlatformInput, Render, ScrollDelta, ScrollWheelEvent, Styled,
        TouchPhase, Window, WindowBounds, WindowHandle, WindowOptions, div, point, px, rgb, size,
    };
    use vespertine_platform::application;
    use vespertine_webview::protocol::Command;
    use vespertine_webview::{BrowserContext, WebPage, WebPageEvent, page_element};

    const PAGE: &str = r##"<!doctype html><meta charset=utf-8><title>Embedded page</title>
<style>body{margin:0;background:#fff;font:16px sans-serif}h1{font-size:24px;margin:8px}
input,button{margin:6px;font-size:16px}#f{width:420px}#tall{height:3000px;background:linear-gradient(#fff,#ccc)}</style>
<h1>Embedded page</h1>
<input id=f type=text><input id=p type=password value="s3cret">
<button id=inc onclick="window.counter=(window.counter||0)+1">count</button>
<script>window.frameCounter=0;(function t(){window.frameCounter++;requestAnimationFrame(t)})();</script>
<div id=tall></div>"##;

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

    struct Demo {
        page: Entity<WebPage>,
        waiters: Vec<futures::channel::oneshot::Sender<Value>>,
    }

    impl Render for Demo {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(rgb(0x181A1F)).p(px(20.)).child(
                div()
                    .w(px(720.))
                    .h(px(420.))
                    .child(page_element(&self.page, cx)),
            )
        }
    }

    struct Script {
        handle: WindowHandle<Demo>,
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
                .read_with(&self.cx, |demo, _| demo.page.clone())
                .expect("the window exists")
        }

        /// Evaluates a script in the page and returns its JSON result.
        async fn eval(&self, script: &str) -> Value {
            let (sender, receiver) = futures::channel::oneshot::channel();
            let page = self.page();
            self.handle
                .update(&mut self.cx.clone(), |demo, _, cx| {
                    demo.waiters.push(sender);
                    page.update(cx, |page, _| {
                        page.send_command(Command::Eval {
                            script: script.to_owned(),
                        })
                    });
                })
                .expect("the window exists");
            let mut receiver = receiver;
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
                .expect("the window exists");
        }

        fn key(&self, keystroke: &str) {
            let keystroke = Keystroke::parse(keystroke).expect("valid keystroke");
            self.cx
                .clone()
                .update_window(self.handle.into(), |_, window, cx| {
                    window.dispatch_keystroke(keystroke, cx);
                })
                .expect("the window exists");
        }

        fn page_origin(&self) -> (f32, f32) {
            self.handle
                .update(&mut self.cx.clone(), |_, window, _| {
                    let bounds: Bounds<_> = window.bounds();
                    let _ = bounds;
                    (20.0, 20.0)
                })
                .expect("the window exists")
        }

        async fn click_selector(&self, selector: &str) {
            let rect = self
                .eval(&format!(
                    "(() => {{ const r = document.querySelector({selector:?}).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()"
                ))
                .await;
            let (ox, oy) = self.page_origin();
            let position = point(
                px(ox + rect[0].as_f64().unwrap_or(0.0) as f32),
                px(oy + rect[1].as_f64().unwrap_or(0.0) as f32),
            );
            self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::none(),
            }));
            self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            }));
            self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::none(),
                click_count: 1,
            }));
        }

        fn clipboard(&self) -> Option<String> {
            self.cx
                .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
        }

        async fn run(mut self) -> Vec<String> {
            let page = self.page();
            // Wait for the page to load.
            let mut loaded = false;
            for _ in 0..600 {
                let state = self.cx.update(|cx| page.read(cx).state().clone());
                if !state.loading && state.title == "Embedded page" {
                    loaded = true;
                    break;
                }
                self.sleep(50).await;
            }
            self.check(loaded, "the local page loads");
            if !loaded {
                return self.failures;
            }
            let width = self.eval("window.innerWidth").await;
            self.check(
                width.as_f64().is_some_and(|w| (w - 720.0).abs() <= 2.0),
                "the CSS viewport matches the element width",
            );

            // A real pointer click focuses the text field and the page.
            self.click_selector("#f").await;
            let focused = self
                .until("document.activeElement && document.activeElement.id", |v| {
                    v == "f"
                })
                .await;
            self.check(focused, "a pointer click focuses the text field");
            for _ in 0..50 {
                if self.cx.update(|cx| {
                    page.read(cx)
                        .native_page()
                        .is_some_and(|p| p.input_state().focused)
                }) {
                    break;
                }
                self.sleep(50).await;
            }

            // Type "hello linux" with individual key events.
            for key in ["h", "e", "l", "l", "o", "space", "l", "i", "n", "u", "x"] {
                self.key(key);
            }
            let typed = self
                .until("document.querySelector('#f').value", |v| v == "hello linux")
                .await;
            self.check(typed, "typing with key events produces 'hello linux'");

            // The system clipboard is only reachable from a window that has a recent input
            // serial on Wayland; without one (an unfocused window in a headless session) the
            // clipboard checks cannot run and are reported as skipped, never as passed.
            self.cx
                .update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("probe".into())));
            self.sleep(100).await;
            let clipboard_works = self.clipboard().as_deref() == Some("probe");
            if clipboard_works {
                // Paste with control+V.
                self.cx.update(|cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(" — café 日本語".into()))
                });
                self.key("ctrl-v");
                let pasted = self
                    .until("document.querySelector('#f').value", |v| {
                        v == "hello linux — café 日本語"
                    })
                    .await;
                self.check(pasted, "control+V pastes non-ASCII text exactly");

                // Select all and copy with control+C.
                self.eval("document.querySelector('#f').select(); true")
                    .await;
                self.cx
                    .update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("stale".into())));
                self.key("ctrl-c");
                let mut copied = false;
                for _ in 0..100 {
                    if self.clipboard().as_deref() == Some("hello linux — café 日本語") {
                        copied = true;
                        break;
                    }
                    self.sleep(50).await;
                }
                self.check(
                    copied,
                    "control+C puts the field value on the system clipboard",
                );

                // Copy with no selection leaves the clipboard untouched.
                self.eval("const f = document.querySelector('#f'); f.setSelectionRange(f.value.length, f.value.length); true").await;
                self.cx
                    .update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("keep".into())));
                self.key("ctrl-c");
                self.sleep(600).await;
                self.check(
                    self.clipboard().as_deref() == Some("keep"),
                    "copy with no selection keeps the clipboard",
                );
            } else {
                println!(
                    "SKIP clipboard checks: the compositor gives this window no clipboard access (try WAYLAND_DISPLAY= for X11)"
                );
                // Insert the text the paste would have inserted so the later steps still apply.
                self.eval(
                    "const f = document.querySelector('#f'); f.value += ' — café 日本語'; true",
                )
                .await;
            }

            // Composition through the page element's text-input handler.
            let marked_during = {
                let page = page.clone();
                self.cx
                    .clone()
                    .update_window(self.handle.into(), |_, window, cx| {
                        page.update(cx, |page, cx| {
                            page.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
                            page.replace_and_mark_text_in_range(None, "日本語", None, window, cx);
                            page.marked_text_range(window, cx)
                        })
                    })
                    .expect("the window exists")
            };
            self.check(
                marked_during == Some(0..3),
                "the marked range is 0..3 while '日本語' is marked",
            );
            self.sleep(200).await;
            let page_for_commit = page.clone();
            let after = self
                .cx
                .clone()
                .update_window(self.handle.into(), |_, window, cx| {
                    page_for_commit.update(cx, |page, cx| {
                        page.replace_text_in_range(None, "日本語", window, cx);
                        page.marked_text_range(window, cx)
                    })
                })
                .expect("the window exists");
            self.check(after.is_none(), "no marked range after the commit");
            let composed = self
                .until("document.querySelector('#f').value", |v| {
                    v == "hello linux — café 日本語日本語"
                })
                .await;
            self.check(
                composed,
                "preedit then commit inserts '日本語' after the earlier text",
            );

            // Password fields: surrounding text never reaches the application.
            self.click_selector("#p").await;
            self.until("document.activeElement && document.activeElement.id", |v| {
                v == "p"
            })
            .await;
            self.eval("const p = document.querySelector('#p'); p.setSelectionRange(0, p.value.length); true").await;
            self.sleep(400).await;
            let leaked = self.cx.update(|cx| {
                page.read(cx)
                    .native_page()
                    .and_then(|p| p.input_state().text)
                    .is_some_and(|text| text.contains("s3cret"))
            });
            self.check(
                !leaked,
                "password text is never reported to the application",
            );
            if clipboard_works {
                self.cx
                    .update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("keep".into())));
                self.key("ctrl-c");
                self.sleep(600).await;
                self.check(
                    self.clipboard().as_deref() == Some("keep"),
                    "copy in a password field returns nothing",
                );
            }

            // Clicks and wheel input, while the page keeps animating.
            let frames_before = self
                .eval("window.frameCounter")
                .await
                .as_f64()
                .unwrap_or(0.0);
            self.click_selector("#inc").await;
            self.click_selector("#inc").await;
            let counted = self
                .until("window.counter", |v| v.as_i64() == Some(2))
                .await;
            self.check(
                counted,
                "each click increments the page counter exactly once",
            );
            let (ox, oy) = self.page_origin();
            self.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: point(px(ox + 300.0), px(oy + 300.0)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-250.))),
                modifiers: Modifiers::none(),
                touch_phase: TouchPhase::Moved,
            }));
            let scrolled = self
                .until("window.scrollY", |v| v.as_f64().is_some_and(|y| y > 50.0))
                .await;
            self.check(
                scrolled,
                "wheel input scrolls the page by more than 50 CSS pixels",
            );
            self.sleep(2000).await;
            let frames_after = self
                .eval("window.frameCounter")
                .await
                .as_f64()
                .unwrap_or(0.0);
            println!(
                "animation frames during the interactions: {}",
                frames_after - frames_before
            );
            self.check(
                frames_after - frames_before > 60.0,
                "the page keeps rendering more than 60 animation frames",
            );
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
                            cx.subscribe(&page, |demo: &mut Demo, _, event: &WebPageEvent, _| {
                                if let WebPageEvent::EvalResult(text) = event
                                    && !demo.waiters.is_empty()
                                {
                                    let value = serde_json::from_str(text).unwrap_or(Value::Null);
                                    let _ = demo.waiters.remove(0).send(value);
                                }
                            })
                            .detach();
                            Demo {
                                page: page.clone(),
                                waiters: Vec::new(),
                            }
                        })
                    },
                )
                .expect("open the window");
            cx.activate(true);
            let page = handle
                .read_with(cx, |demo, _| demo.page.clone())
                .expect("window");
            handle
                .update(cx, |_, window, cx| {
                    page.update(cx, |page, cx| {
                        page.set_presentation(vespertine_webview::Presentation::Live, window, cx);
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
