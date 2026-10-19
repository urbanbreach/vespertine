//! THM-003 A1: two windows switch appearance mode, theme variants, accent and surface
//! preference independently.
//!
//! Run `cargo run -p vespertine_theme --example theme_appearance` and use the buttons, or pass
//! `--scripted` to drive every change automatically: after each step the example checks that
//! every window painted the new style generation and the active palette (no stale frame), that
//! a window opened afterwards shows the active palette in its first frame, and exits non-zero on
//! any mismatch.

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};
use vespertine::{
    App, AppContext as _, Bounds, Context, Div, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    WindowBounds, WindowId, WindowOptions, div, px, size,
};
use vespertine_platform::application;
use vespertine_theme::{
    appearance::{self, current_appearance, style_generation, theme_of},
    builtin::neutral_variant,
    color::Color,
    selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
    theme::{Appearance, ThemeFamily},
};

/// What a window painted in its most recent frame.
#[derive(Clone, Debug, PartialEq)]
struct Frame {
    generation: u32,
    background: Color,
    variant: String,
}

type FrameLog = Rc<RefCell<HashMap<WindowId, Frame>>>;

struct ThemedSurface {
    title: SharedString,
    frames: FrameLog,
}

fn button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = theme_of(cx);
    div()
        .id(id)
        .py_1()
        .px_2()
        .rounded_md()
        .bg(Hsla::from(theme.colors.raised))
        .border_1()
        .border_color(Hsla::from(theme.colors.border))
        .text_color(Hsla::from(theme.colors.text))
        .hover(|style| style.bg(Hsla::from(theme.colors.hover)))
        .child(label)
        .on_click(move |_, _, cx| on_click(cx))
}

/// A wrapping row of buttons.
fn button_row() -> Div {
    div().flex_wrap().flex().gap_2()
}

impl Render for ThemedSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme_of(cx).clone();
        self.frames.borrow_mut().insert(
            window.window_handle().window_id(),
            Frame {
                generation: theme.generation,
                background: theme.colors.background,
                variant: theme.id.clone(),
            },
        );
        let swatch = |color: Color| div().size_6().rounded_sm().bg(Hsla::from(color));
        div()
            .size_full()
            .p_4()
            .gap_3()
            .flex()
            .flex_col()
            .bg(Hsla::from(theme.colors.background))
            .text_color(Hsla::from(theme.colors.text))
            .child(format!("{}: {}", self.title, theme.name))
            .child(
                div()
                    .text_color(Hsla::from(theme.colors.text_muted))
                    .child(format!(
                        "variant {} | {:?} | surface {:?} | style generation {} | window glass {}",
                        theme.id,
                        theme.effective_appearance,
                        theme.treatment,
                        theme.generation,
                        theme.window_glass_alpha(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(swatch(theme.accent.primary))
                    .child(swatch(theme.accent.strong))
                    .child(swatch(theme.accent.wash))
                    .child(swatch(theme.accent.glyph[0]))
                    .child(swatch(theme.accent.glyph[2])),
            )
            .child(
                button_row()
                    .child(button(
                        "system",
                        "System",
                        |cx| set_mode(cx, AppearanceMode::System),
                        cx,
                    ))
                    .child(button(
                        "light",
                        "Light",
                        |cx| set_mode(cx, AppearanceMode::Light),
                        cx,
                    ))
                    .child(button(
                        "dark",
                        "Dark",
                        |cx| set_mode(cx, AppearanceMode::Dark),
                        cx,
                    )),
            )
            .child(
                button_row()
                    .child(button(
                        "midnight",
                        "Midnight (dark)",
                        |cx| set_variant(cx, Appearance::Dark, "midnight-dark"),
                        cx,
                    ))
                    .child(button(
                        "paper",
                        "Paper (light)",
                        |cx| set_variant(cx, Appearance::Light, "paper-light"),
                        cx,
                    ))
                    .child(button(
                        "default-dark",
                        "Default dark",
                        |cx| set_variant(cx, Appearance::Dark, "default-dark"),
                        cx,
                    ))
                    .child(button(
                        "default-light",
                        "Default light",
                        |cx| set_variant(cx, Appearance::Light, "default-light"),
                        cx,
                    )),
            )
            .child(
                button_row()
                    .child(button(
                        "accent-default",
                        "Accent: theme",
                        |cx| appearance::set_accent(cx, AccentSelection::ThemeDefault),
                        cx,
                    ))
                    .child(button(
                        "accent-yellow",
                        "Accent: yellow",
                        |cx| {
                            appearance::set_accent(
                                cx,
                                AccentSelection::Color(Color::rgb(250, 220, 30)),
                            )
                        },
                        cx,
                    ))
                    .child(button(
                        "accent-blue",
                        "Accent: blue",
                        |cx| {
                            appearance::set_accent(
                                cx,
                                AccentSelection::Color(Color::rgb(20, 70, 200)),
                            )
                        },
                        cx,
                    )),
            )
            .child(
                button_row()
                    .child(button(
                        "surface-default",
                        "Surface: theme",
                        |cx| appearance::set_surface(cx, SurfacePreference::ThemeDefault),
                        cx,
                    ))
                    .child(button(
                        "surface-frosted",
                        "Surface: frosted",
                        |cx| appearance::set_surface(cx, SurfacePreference::Frosted),
                        cx,
                    ))
                    .child(button(
                        "surface-opaque",
                        "Surface: opaque",
                        |cx| appearance::set_surface(cx, SurfacePreference::Opaque),
                        cx,
                    )),
            )
            .child(button(
                "new-window",
                "Open another window",
                |cx| {
                    let frames = cx.global::<SharedFrames>().0.clone();
                    open_window(cx, "Another window", frames);
                },
                cx,
            ))
    }
}

struct SharedFrames(FrameLog);

impl vespertine::Global for SharedFrames {}

fn set_mode(cx: &mut App, mode: AppearanceMode) {
    appearance::set_mode(cx, mode);
}

fn set_variant(cx: &mut App, appearance: Appearance, id: &str) {
    appearance::set_theme(cx, appearance, id);
}

fn open_window(cx: &mut App, title: &'static str, frames: FrameLog) -> WindowId {
    let bounds = Bounds::centered(None, size(px(640.0), px(420.0)), cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                // Reconcile with the window's own appearance before its first draw.
                appearance::observe_window(window, cx);
                cx.new(|_| ThemedSurface {
                    title: title.into(),
                    frames,
                })
            },
        )
        .unwrap();
    handle.window_id()
}

fn custom_family(id: &str, name: &str, appearance: Appearance, background: Color) -> ThemeFamily {
    let mut variant = neutral_variant(appearance);
    let suffix = if appearance.is_dark() {
        "dark"
    } else {
        "light"
    };
    variant.id = format!("{id}-{suffix}");
    variant.family_id = id.to_string();
    variant.name = name.to_string();
    variant.colors.background = background;
    variant.curated = false;
    ThemeFamily {
        id: id.to_string(),
        name: name.to_string(),
        variants: vec![variant],
    }
}

/// Every window must have painted the current generation and palette.
fn check(cx: &App, frames: &FrameLog, step: &str) -> bool {
    let theme = theme_of(cx);
    let frames = frames.borrow();
    let mut ok = true;
    for (window, frame) in frames.iter() {
        if frame.generation != style_generation() || frame.background != theme.colors.background {
            eprintln!(
                "FAIL {step}: window {window:?} painted {frame:?}, active is {} / {}",
                style_generation(),
                theme.id
            );
            ok = false;
        }
    }
    if !ok {
        return false;
    }
    println!(
        "ok   {step}: {} window(s) painted generation {} ({})",
        frames.len(),
        style_generation(),
        theme.id
    );
    true
}

fn run() {
    let scripted = std::env::args().any(|argument| argument == "--scripted");
    application().run(move |cx: &mut App| {
        appearance::init(
            cx,
            AppearanceMode::System,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        );
        appearance::apply_registry_change(
            cx,
            vec![
                custom_family(
                    "midnight",
                    "Midnight",
                    Appearance::Dark,
                    Color::rgb(14, 20, 38),
                ),
                custom_family(
                    "paper",
                    "Paper",
                    Appearance::Light,
                    Color::rgb(250, 246, 236),
                ),
            ],
            &[],
        );
        let frames: FrameLog = Rc::default();
        cx.set_global(SharedFrames(frames.clone()));
        open_window(cx, "Window one", frames.clone());
        open_window(cx, "Window two", frames.clone());
        cx.activate(true);

        if !scripted {
            return;
        }
        cx.spawn(async move |cx| {
            let settle = || cx.background_executor().timer(Duration::from_millis(400));
            settle().await;
            let mut all_ok = cx.update(|cx| check(cx, &frames, "initial frames"));
            type Step = (&'static str, Box<dyn Fn(&mut App)>);
            let steps: Vec<Step> = vec![
                (
                    "pin dark",
                    Box::new(|cx| appearance::set_mode(cx, AppearanceMode::Dark)),
                ),
                (
                    "pin light",
                    Box::new(|cx| appearance::set_mode(cx, AppearanceMode::Light)),
                ),
                (
                    "follow system",
                    Box::new(|cx| appearance::set_mode(cx, AppearanceMode::System)),
                ),
                (
                    "select midnight",
                    Box::new(|cx| {
                        appearance::set_mode(cx, AppearanceMode::Dark);
                        appearance::set_theme(cx, Appearance::Dark, "midnight-dark");
                    }),
                ),
                (
                    "accent yellow",
                    Box::new(|cx| {
                        appearance::set_accent(cx, AccentSelection::Color(Color::rgb(250, 220, 30)))
                    }),
                ),
                (
                    "surface frosted",
                    Box::new(|cx| appearance::set_surface(cx, SurfacePreference::Frosted)),
                ),
                (
                    "light variant while dark is active (no visible change)",
                    Box::new(|cx| appearance::set_theme(cx, Appearance::Light, "paper-light")),
                ),
                (
                    "pin light shows the stored light variant",
                    Box::new(|cx| appearance::set_mode(cx, AppearanceMode::Light)),
                ),
                (
                    "accent back to theme",
                    Box::new(|cx| appearance::set_accent(cx, AccentSelection::ThemeDefault)),
                ),
            ];
            for (name, step) in steps {
                cx.update(|cx| step(cx));
                settle().await;
                all_ok &= cx.update(|cx| check(cx, &frames, name));
            }
            // A window opened afterwards paints the active palette in its first frame.
            cx.update(|cx| {
                open_window(cx, "Opened afterwards", frames.clone());
            });
            settle().await;
            all_ok &= cx.update(|cx| check(cx, &frames, "window opened afterwards"));
            println!("current appearance flag: {:?}", current_appearance());
            if !all_ok {
                std::process::exit(1);
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

fn main() {
    run();
}
