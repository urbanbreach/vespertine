//! CMP-013 A1: a Mermaid diagram that renders lazily, toggles between diagram and source and
//! follows the appearance.
//!
//! The window shows one diagram block. Until the background render finishes the block shows its
//! source; the "Show source" / "Show diagram" action switches views; the appearance chip switches
//! between the dark and the light theme. The renderer is deliberately slow (1.2 s) so that the
//! appearance can be changed while a render is pending: the result of the previous theme is
//! dropped and the diagram is rendered again with the new palette, so the obsolete palette is
//! never displayed.
//!
//! Without arguments the example runs interactively. With `DIAGRAM_TOGGLE_SCRIPT=1` it changes the
//! appearance while the first render is pending, waits for the diagram, toggles the source view
//! twice and exits 0 when every check holds (1 otherwise).

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vespertine::{
    App, AppContext as _, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowHandle, WindowOptions, div, px,
    size,
};
use vespertine_components::diagram::{
    DiagramHandler, DiagramHost, DiagramRenderer, DiagramView, MermanRenderer, Palette, View,
};
use vespertine_platform::application;
use vespertine_theme::document::DocumentTheme;

const SOURCE: &str = "flowchart TD\n  Start[Open the document] --> Check{Diagram ready?}\n  Check -->|no| Source[Show the source]\n  Check -->|yes| Diagram[Show the diagram]\n  Source --> Check\n";
const FRAME: &str = "row-1-mermaid-0";

/// `merman` made slow, to keep a render pending long enough to change the appearance.
struct Slow {
    inner: MermanRenderer,
    renders: Arc<Mutex<Vec<bool>>>,
}

impl DiagramRenderer for Slow {
    fn render(&self, source: &str, palette: &Palette) -> Result<String, String> {
        std::thread::sleep(Duration::from_millis(1200));
        self.renders.lock().unwrap().push(palette.dark);
        self.inner.render(source, palette)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Shown {
    Source,
    Failed,
    Diagram { show_source: bool },
}

struct Example {
    host: DiagramHost,
    handler: DiagramHandler,
    dark: bool,
    shown: Vec<(bool, Shown, usize)>,
    renders: Arc<Mutex<Vec<bool>>>,
    toggle: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl Example {
    fn theme(&self) -> DocumentTheme {
        if self.dark {
            DocumentTheme::dark()
        } else {
            DocumentTheme::light()
        }
    }
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let view = View {
            width: 520.0,
            scale: window.scale_factor(),
        };
        self.host.begin_pass(&theme, view, window, cx);
        let handler = self.handler.clone();
        let result = handler(SOURCE, FRAME, &theme, window, cx);
        let render_count = self.renders.lock().unwrap().len();
        let code = || {
            div()
                .font_family(theme.font_mono.clone())
                .text_size(px(12.0))
                .children(SOURCE.lines().map(|line| div().child(line.to_string())))
        };
        let chip = |id: &'static str, label: String| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .bg(theme.border)
                .cursor_pointer()
                .child(label)
        };
        let mut header = div().flex().gap_2().child(div().child("mermaid")).child(
            chip(
                "appearance",
                format!("Appearance: {}", if self.dark { "dark" } else { "light" }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.dark = !this.dark;
                cx.notify();
            })),
        );
        let body = match result {
            DiagramView::Source => {
                self.shown.push((self.dark, Shown::Source, render_count));
                code().into_any_element()
            }
            DiagramView::Failed(reason) => {
                self.shown.push((self.dark, Shown::Failed, render_count));
                div()
                    .child(format!("\u{26a0} {reason}"))
                    .child(code())
                    .into_any_element()
            }
            DiagramView::Diagram {
                body,
                show_source,
                toggle,
            } => {
                self.shown
                    .push((self.dark, Shown::Diagram { show_source }, render_count));
                self.toggle = Some(toggle.clone());
                header = header.child(
                    chip(
                        "toggle",
                        if show_source {
                            "Show diagram"
                        } else {
                            "Show source"
                        }
                        .to_string(),
                    )
                    .on_click(move |_, window, cx| toggle(window, cx)),
                );
                if show_source {
                    code().into_any_element()
                } else {
                    body
                }
            }
        };
        div()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .child(header)
            .child(body)
    }
}

async fn wait_for(
    cx: &mut vespertine::AsyncApp,
    window: WindowHandle<Example>,
    seconds: u64,
    condition: impl Fn(&Example) -> bool,
) -> bool {
    for _ in 0..seconds * 10 {
        if window
            .read_with(cx, |example, _| condition(example))
            .unwrap_or(false)
        {
            return true;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    false
}

fn run_script(cx: &mut App, window: WindowHandle<Example>) {
    cx.spawn(async move |cx| {
        let mut failures = Vec::new();
        let mut check = |name: &str, ok: bool| {
            println!(
                "diagram_toggle: {name}: {}",
                if ok { "ok" } else { "FAILED" }
            );
            if !ok {
                failures.push(name.to_string());
            }
        };
        let pending = wait_for(cx, window, 3, |e| {
            e.shown.iter().any(|(_, shown, _)| *shown == Shown::Source)
        })
        .await;
        check("the source is shown while the render is pending", pending);
        // Change the appearance while the first (dark) render is pending.
        window
            .update(cx, |example, _, cx| {
                example.dark = false;
                cx.notify();
            })
            .ok();
        let ready = wait_for(cx, window, 15, |e| {
            e.shown
                .iter()
                .any(|(_, shown, _)| matches!(shown, Shown::Diagram { .. }))
        })
        .await;
        check("the diagram is shown after the render", ready);
        let (renders, first_diagram) = window
            .read_with(cx, |example, _| {
                (
                    example.renders.lock().unwrap().clone(),
                    example
                        .shown
                        .iter()
                        .find(|(_, shown, _)| matches!(shown, Shown::Diagram { .. }))
                        .cloned(),
                )
            })
            .expect("window state");
        check(
            "the stale dark render ran and the diagram was rendered again with the light palette",
            renders == vec![true, false],
        );
        check(
            "no diagram was shown before the light render finished, and none for the dark theme",
            first_diagram.is_some_and(|(dark, _, renders)| !dark && renders >= 2),
        );
        for (name, expect_source) in [("source view", true), ("diagram view", false)] {
            let toggle = window
                .read_with(cx, |example, _| example.toggle.clone())
                .ok()
                .flatten();
            if let Some(toggle) = toggle {
                cx.update_window(window.into(), |_, window, cx| toggle(window, cx))
                    .ok();
            }
            let switched = wait_for(cx, window, 3, |e| {
                e.shown.last().is_some_and(|(_, shown, _)| {
                    *shown
                        == Shown::Diagram {
                            show_source: expect_source,
                        }
                })
            })
            .await;
            check(&format!("the toggle shows the {name}"), switched);
        }
        println!(
            "diagram_toggle: {}",
            if failures.is_empty() {
                "all checks passed".to_string()
            } else {
                format!("{} check(s) failed", failures.len())
            }
        );
        std::process::exit(i32::from(!failures.is_empty()));
    })
    .detach();
}

fn main() {
    let script = std::env::var_os("DIAGRAM_TOGGLE_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(640.0), px(480.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    let renders = Arc::new(Mutex::new(Vec::new()));
                    let host = DiagramHost::with_renderer(Arc::new(Slow {
                        inner: MermanRenderer,
                        renders: renders.clone(),
                    }));
                    cx.new(|_| Example {
                        handler: host.handler(),
                        host,
                        dark: true,
                        shown: Vec::new(),
                        renders,
                        toggle: None,
                    })
                },
            )
            .expect("open the window");
        if script {
            run_script(cx, window);
        }
        cx.activate(true);
    });
}
