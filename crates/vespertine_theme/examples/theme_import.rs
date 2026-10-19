//! THM-004 A1: previews a candidate theme, cancels (the committed theme is restored), previews
//! again, rejects an invalid candidate, installs a valid one, reloads a linked source after
//! edits (and after breaking it, showing the last good family and a warning) and removes it.
//!
//! A per-frame hook samples the resolved theme on every painted frame and counts frames whose
//! palette is invalid or partially replaced; the example exits non-zero if there are any.
//!
//! Run `cargo run -p vespertine_theme --example theme_import`.

use serde_json::json;
use std::{cell::RefCell, path::Path, rc::Rc, time::Duration};
use vespertine::{
    App, AppContext as _, Bounds, Context, Hsla, IntoElement, ParentElement as _, Render,
    Styled as _, Window, WindowBounds, WindowOptions, div, px, size,
};
use vespertine_platform::application;
use vespertine_theme::{
    appearance::{self, style_generation, theme_of},
    color::Color,
    import::{ImportAdapter as _, ImportOptions, VsCodeAdapter},
    library::{EntryStatus, InstallMode, Library},
    preview,
    selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
    theme::ThemeFamily,
};

#[derive(Default)]
struct Hook {
    frames: usize,
    violations: Vec<String>,
    log: Vec<String>,
}

type SharedHook = Rc<RefCell<Hook>>;

struct Status {
    hook: SharedHook,
}

impl Render for Status {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme_of(cx).clone();
        // The per-frame test hook: the resolved theme must be a complete, valid palette.
        {
            let mut hook = self.hook.borrow_mut();
            hook.frames += 1;
            let frame = hook.frames;
            if let Err(message) = theme.check_complete() {
                hook.violations
                    .push(format!("frame {frame} ({}): {message}", theme.id));
            }
            if theme.generation != style_generation() {
                hook.violations.push(format!(
                    "frame {}: painted generation {} but the current one is {}",
                    frame,
                    theme.generation,
                    style_generation()
                ));
            }
        }
        let hook = self.hook.borrow();
        div()
            .size_full()
            .p_4()
            .gap_2()
            .flex()
            .flex_col()
            .bg(Hsla::from(theme.colors.background))
            .text_color(Hsla::from(theme.colors.text))
            .child(format!(
                "{} {} (generation {}, {} frames)",
                if theme.is_preview {
                    "PREVIEW"
                } else {
                    "committed"
                },
                theme.id,
                theme.generation,
                hook.frames
            ))
            .children(hook.log.iter().rev().take(14).map(|line| {
                div()
                    .text_color(Hsla::from(theme.colors.text_muted))
                    .child(line.clone())
            }))
    }
}

fn write_theme(path: &Path, background: &str, accent: &str) {
    let theme = json!({
        "name": "Sample",
        "type": "dark",
        "colors": {
            "editor.background": background,
            "editor.foreground": "#d4d4d4",
            "focusBorder": accent,
            "button.background": accent,
            "errorForeground": "#f48771"
        },
        "tokenColors": [{ "scope": "keyword", "settings": { "foreground": accent } }]
    });
    std::fs::write(path, serde_json::to_vec_pretty(&theme).unwrap()).unwrap();
}

fn step(cx: &mut App, hook: &SharedHook, message: &str) {
    hook.borrow_mut().log.push(message.to_string());
    println!("{message}");
    cx.refresh_windows();
}

fn expect(hook: &SharedHook, condition: bool, message: &str) {
    if !condition {
        hook.borrow_mut()
            .violations
            .push(format!("expectation failed: {message}"));
    }
}

fn run() {
    application().run(|cx: &mut App| {
        appearance::init(
            cx,
            AppearanceMode::Dark,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        );
        let hook: SharedHook = Rc::default();
        let bounds = Bounds::centered(None, size(px(720.0), px(420.0)), cx);
        let window_hook = hook.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            appearance::observe_window(window, cx);
            cx.new(|_| Status { hook: window_hook })
        })
        .unwrap();
        cx.activate(true);

        cx.spawn(async move |cx| {
            let settle = || cx.background_executor().timer(Duration::from_millis(250));
            let workspace = tempfile::tempdir().unwrap();
            let data = workspace.path().join("data");
            let source = workspace.path().join("sample.json");
            write_theme(&source, "#10202c", "#4aa3ff");
            let options = ImportOptions::local("sample", "Sample", &source);
            let mut library = Library::load(&data);
            let committed = cx.update(|cx| theme_of(cx).variant.clone());
            let compilation = VsCodeAdapter.compile(&source, &options).unwrap();
            settle().await;

            cx.update(|cx| {
                step(cx, &hook, "preview the candidate");
                preview::preview(cx, &compilation.family, None).unwrap();
                expect(&hook, preview::is_previewing(cx), "previewing");
                expect(
                    &hook,
                    theme_of(cx).colors.background == Color::rgb(0x10, 0x20, 0x2c),
                    "candidate palette active",
                );
                expect(
                    &hook,
                    appearance::registry(cx).variant("sample").is_none(),
                    "registry untouched by a preview",
                );
            });
            settle().await;
            cx.update(|cx| {
                step(cx, &hook, "cancel: the committed theme is restored");
                preview::cancel(cx);
                expect(
                    &hook,
                    theme_of(cx).variant == committed,
                    "committed theme restored",
                );
            });
            settle().await;
            cx.update(|cx| {
                step(cx, &hook, "preview again");
                preview::preview(cx, &compilation.family, None).unwrap();
            });
            settle().await;

            cx.update(|cx| {
                step(cx, &hook, "an invalid candidate is rejected");
                let mut invalid: ThemeFamily = compilation.family.clone();
                invalid.variants[0].source.license.clear();
                let rejected = preview::preview(cx, &invalid, None);
                expect(&hook, rejected.is_err(), "invalid candidate rejected");
                // The previously previewed (valid) candidate is still shown, not half replaced.
                expect(
                    &hook,
                    theme_of(cx).colors.background == Color::rgb(0x10, 0x20, 0x2c),
                    "preview unchanged by the rejection",
                );
                let mut broken = compilation.clone();
                broken.family = invalid;
                let installed =
                    preview::commit(cx, &mut library, &broken, &[], InstallMode::Link, None);
                expect(
                    &hook,
                    installed.is_err(),
                    "invalid candidate cannot be installed",
                );
                expect(
                    &hook,
                    library.entries().is_empty(),
                    "library untouched by a failed install",
                );
            });
            settle().await;

            let id = cx.update(|cx| {
                step(
                    cx,
                    &hook,
                    "install the valid candidate (linked) and select it",
                );
                let id = preview::commit(
                    cx,
                    &mut library,
                    &compilation,
                    &[],
                    InstallMode::Link,
                    Some("sample"),
                )
                .unwrap();
                expect(&hook, !preview::is_previewing(cx), "preview ended");
                expect(
                    &hook,
                    appearance::registry(cx).variant("sample").is_some(),
                    "runtime registry installed",
                );
                id
            });
            settle().await;

            write_theme(&source, "#2a1020", "#ff7ac6");
            cx.update(|cx| {
                step(cx, &hook, "edit the source and reload");
                library.reload(&id).unwrap();
                library.install_runtime(cx, &[]);
                expect(
                    &hook,
                    theme_of(cx).colors.background == Color::rgb(0x2a, 0x10, 0x20),
                    "reloaded palette active",
                );
            });
            settle().await;

            std::fs::write(&source, "{ this is not json").unwrap();
            cx.update(|cx| {
                step(
                    cx,
                    &hook,
                    "break the source: reload fails, the last good family stays",
                );
                let error = library.reload(&id).unwrap_err();
                library.install_runtime(cx, &[]);
                expect(
                    &hook,
                    matches!(
                        library.entry(&id).unwrap().status,
                        EntryStatus::Warning { .. }
                    ),
                    "warning status",
                );
                expect(
                    &hook,
                    theme_of(cx).colors.background == Color::rgb(0x2a, 0x10, 0x20),
                    "last good family still active",
                );
                hook.borrow_mut().log.push(format!("warning: {error}"));
            });
            settle().await;

            cx.update(|cx| {
                step(cx, &hook, "remove the theme: the selection is repointed");
                let removed = library.remove(&id).unwrap();
                let repointed = library.install_runtime(cx, &removed);
                expect(&hook, repointed, "selection repointed");
                expect(
                    &hook,
                    theme_of(cx).id == "default-dark",
                    "fell back to the built-in theme",
                );
            });
            settle().await;

            let hook = hook.borrow();
            println!("{} frames sampled", hook.frames);
            if !hook.violations.is_empty() {
                for violation in &hook.violations {
                    eprintln!("VIOLATION {violation}");
                }
                std::process::exit(1);
            }
            drop(hook);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

fn main() {
    run();
}
