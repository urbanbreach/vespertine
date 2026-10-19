//! The seams of the rich text with the rest of the framework: the PLT-010 link policy as the
//! default of `LinkOptions`, the `vespertine_theme` adapter for accents and syntax palettes, and
//! `userSelect` / `selectionColor` as the framework's inherited text style properties (ELM-006).

mod support;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use support::{Build, Harness, at, near, rgb_at};
use vespertine::{
    AppContext as _, Context, Entity, Hsla, IntoElement, LinkPolicy, LinkRoutes, MouseButton,
    ParentElement, Render, Rgba, SharedString, Styled, Window, div, px, rgb,
};
use vespertine_richtext::syntax::SyntaxColors;
use vespertine_richtext::{
    Link, LinkActionKind, LinkActivation, LinkOptions, RichText, SELECTION_ALPHA, TextTheme,
    UserSelect, selection_color, text_root, user_select,
};
use vespertine_theme::appearance::{self, theme_of};
use vespertine_theme::color::Color;
use vespertine_theme::document::DocumentTheme;
use vespertine_theme::selection::{
    AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection,
};

const WIDTH: f32 = 240.0;

fn page(child: impl IntoElement) -> vespertine::AnyElement {
    text_root(
        div()
            .size_full()
            .bg(rgb(0x181818))
            .font_family("Noto Sans")
            .text_size(px(16.))
            .line_height(px(24.))
            .text_color(rgb(0xeeeeee))
            .child(child),
    )
    .into_any_element()
}

// ------------------------------------------------------------------- PLT-010 link policy

#[test]
fn link_options_default_to_the_framework_link_policy() {
    let options = LinkOptions::default();
    let allowed = &options.navigation_allowed;
    for accepted in [
        "https://example.com/a",
        "HTTP://EXAMPLE.COM",
        "http://x.y/%41",
    ] {
        assert!(allowed(accepted), "{accepted}");
    }
    // Everything the default policy rejects, including what the old private scheme list allowed.
    for rejected in [
        "javascript:alert(1)",
        "data:text/plain,hi",
        "mailto:a@b.com",
        "file:///tmp/a",
        "src/a.rs:12",
        "/abs/path",
        "example.com",
        "https://user:pass@example.com",
        "https://",
        "https://example.com/%GG",
        "https://example.com/a b",
    ] {
        assert!(!allowed(rejected), "{rejected}");
    }
    // The default is the very policy of the framework.
    for text in ["https://example.com", "mailto:a@b.com", "x"] {
        assert_eq!(
            allowed(text),
            LinkPolicy::default().validate(text).is_ok(),
            "{text}"
        );
    }
}

#[test]
fn a_caller_policy_routes_and_predicate_replace_the_default() {
    let only_ok = LinkOptions::default().policy(LinkPolicy::new(|original| {
        if original == "ok" {
            Ok(SharedString::from("ok"))
        } else {
            Err(SharedString::from("no"))
        }
    }));
    assert!((only_ok.navigation_allowed)("ok"));
    assert!(!(only_ok.navigation_allowed)("https://example.com"));

    let routes = LinkOptions::default().routes(
        LinkRoutes::new()
            .with_mail()
            .with_files(Vec::<PathBuf>::new()),
    );
    for accepted in ["mailto:a@b.com", "/abs/path", "https://example.com"] {
        assert!((routes.navigation_allowed)(accepted), "{accepted}");
    }
    for rejected in ["javascript:alert(1)", "relative/path.rs", "MAILTO:a b@c"] {
        assert!(!(routes.navigation_allowed)(rejected), "{rejected}");
    }

    // The predicate seam stays available and wins when it is set last.
    let custom = LinkOptions::default().navigation_allowed(|d| d.starts_with("custom:"));
    assert!((custom.navigation_allowed)("custom:thing"));
    assert!(!(custom.navigation_allowed)("https://example.com"));
}

fn mail_link_harness(
    options: LinkOptions,
) -> (Harness, Rc<std::cell::RefCell<Vec<LinkActivation>>>) {
    let log: Rc<std::cell::RefCell<Vec<LinkActivation>>> = Rc::default();
    let sink = log.clone();
    let options =
        options.on_activate(move |activation, _, _| sink.borrow_mut().push(activation.clone()));
    let build: Build = Rc::new(move |_, _| {
        page(
            RichText::new("t", "write to us")
                .links(vec![Link {
                    range: 0..8,
                    destination: "mailto:a@b.com".to_string(),
                }])
                .link_options(options.clone()),
        )
    });
    (Harness::new(WIDTH, 100., build), log)
}

#[test]
fn a_mail_link_navigates_only_when_the_caller_enables_the_mail_route() {
    let (mut blocked, blocked_log) = mail_link_harness(LinkOptions::default());
    blocked.click(at(10., 12.));
    blocked.focus_next();
    blocked.key("enter");
    assert!(
        blocked_log.borrow().is_empty(),
        "the default policy is web only"
    );
    blocked.key("shift-f10");
    blocked.key("enter");
    assert_eq!(
        blocked.clipboard_text().as_deref(),
        Some("mailto:a@b.com"),
        "copying is always allowed"
    );

    let (mut enabled, enabled_log) =
        mail_link_harness(LinkOptions::default().routes(LinkRoutes::new().with_mail()));
    enabled.click(at(10., 12.));
    assert_eq!(enabled_log.borrow().len(), 1);
    assert_eq!(enabled_log.borrow()[0].destination, "mailto:a@b.com");
    assert_eq!(enabled_log.borrow()[0].action, LinkActionKind::Open);
}

// -------------------------------------------------------------------- THM adapter

fn assert_hsla_near(actual: Hsla, expected: Hsla, label: &str) {
    let (a, e) = (Rgba::from(actual), Rgba::from(expected));
    for (name, x, y) in [
        ("r", a.r, e.r),
        ("g", a.g, e.g),
        ("b", a.b, e.b),
        ("a", a.a, e.a),
    ] {
        assert!((x - y).abs() <= 1.0 / 255.0, "{label}.{name}: {x} vs {y}");
    }
}

#[test]
fn without_an_installed_theme_the_neutral_dark_variant_answers() {
    let accent = Rgba::from(TextTheme::default().accent);
    assert_eq!(
        [accent.r, accent.g, accent.b].map(|c| (c * 255.0).round() as u8),
        [0x7c, 0x86, 0xff]
    );
    // The neutral dark palette is the THM-002 dark document theme.
    let document = SyntaxColors::from_document(&DocumentTheme::dark());
    let dark = SyntaxColors::dark();
    for key in vespertine_richtext::syntax::CATEGORY_KEYS {
        assert_hsla_near(
            dark.category(key).unwrap(),
            document.category(key).unwrap(),
            key,
        );
    }
    assert_hsla_near(dark.plain, document.plain, "plain");
    assert_eq!(dark, SyntaxColors::default());
}

fn themed_harness(build: Build) -> Harness {
    let mut harness = Harness::new(WIDTH, 120., build);
    harness.cx.update(|cx| {
        appearance::init(
            cx,
            AppearanceMode::Dark,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        );
    });
    harness.redraw();
    harness
}

fn wash_build() -> Build {
    Rc::new(|_, _| {
        page(
            div()
                .child(RichText::new("a", "wash row one"))
                .child(RichText::new("b", "wash row two")),
        )
    })
}

fn washed(harness: &mut Harness) -> [u8; 3] {
    harness.drag(at(0., 12.), at(100., 36.));
    let image = harness.screenshot();
    rgb_at(&image, WIDTH, 2., 3.)
}

fn over_background(accent: Color) -> [u8; 3] {
    let mix = |a: u8, b: u8| {
        (f32::from(a) * SELECTION_ALPHA + f32::from(b) * (1.0 - SELECTION_ALPHA)).round() as u8
    };
    [
        mix(accent.r, 0x18),
        mix(accent.g, 0x18),
        mix(accent.b, 0x18),
    ]
}

#[test]
fn accent_and_palette_follow_the_active_theme() {
    let mut harness = themed_harness(wash_build());
    let (theme_accent, theme_colors) = harness.cx.update(|cx| {
        let theme = theme_of(cx);
        assert_eq!(
            TextTheme::current(cx).accent,
            theme.accent.primary.to_hsla()
        );
        assert_eq!(SyntaxColors::current(cx), SyntaxColors::from_variant(theme));
        (theme.accent.primary, SyntaxColors::current(cx))
    });
    assert!(near(washed(&mut harness), over_background(theme_accent), 2));

    // A new accent reaches the next frame's selection wash.
    harness.cx.update(|cx| {
        appearance::set_accent(cx, AccentSelection::Color(Color::rgb(0x20, 0xb0, 0x50)));
    });
    harness.redraw();
    let changed = harness.cx.update(|cx| theme_of(cx).accent.primary);
    assert_ne!(changed, theme_accent);
    assert!(near(washed(&mut harness), over_background(changed), 2));

    // A light theme recolors the syntax palette.
    harness
        .cx
        .update(|cx| appearance::set_mode(cx, AppearanceMode::Light));
    harness.redraw();
    let light = harness.cx.update(|cx| SyntaxColors::current(cx));
    assert_ne!(light, theme_colors);
    assert_ne!(light.plain, theme_colors.plain);
    assert_eq!(
        harness.cx.update(|cx| TextTheme::current(cx).accent),
        harness
            .cx
            .update(|cx| theme_of(cx).accent.primary.to_hsla())
    );
}

// ------------------------------------------------- ELM-006 text style as the single source

#[test]
fn styled_user_select_is_the_scope_mechanism() {
    let build: Build = Rc::new(|_, _| {
        page(
            div()
                .child(
                    div()
                        .user_select(false)
                        .child(RichText::new("locked", "locked text"))
                        .child(
                            div()
                                .user_select(true)
                                .child(RichText::new("island", "island text")),
                        ),
                )
                .child(user_select(
                    UserSelect::None,
                    div().child(RichText::new("scoped", "scoped lock")),
                ))
                .child(RichText::new("free", "free text")),
        )
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    assert_eq!(
        service.registered_runs(),
        2,
        "island and free text register"
    );
    assert!(
        service
            .painted_text()
            .iter()
            .any(|entry| entry.text == "locked text" || entry.text == "scoped lock"),
        "locked text still paints"
    );
    // A drag across locked text never selects it.
    harness.drag(at(1., 36.), at(60., 100.));
    let text = service.selected_text().unwrap_or_default();
    assert!(
        !text.contains("locked") && !text.contains("scoped"),
        "{text:?}"
    );
}

#[test]
fn styled_selection_color_and_the_scope_paint_the_same_wash() {
    let color = Rgba {
        r: 0.27,
        g: 0.4,
        b: 0.8,
        a: 0.5,
    };
    let styled: Build = Rc::new(move |_, _| {
        page(
            div()
                .selection_color(color)
                .child(RichText::new("a", "wash row one"))
                .child(RichText::new("b", "wash row two")),
        )
    });
    let scoped: Build = Rc::new(move |_, _| {
        page(selection_color(
            color,
            div()
                .child(RichText::new("a", "wash row one"))
                .child(RichText::new("b", "wash row two")),
        ))
    });
    let from_style = washed(&mut Harness::new(WIDTH, 120., styled));
    let from_scope = washed(&mut Harness::new(WIDTH, 120., scoped));
    assert_eq!(from_style, from_scope);
    let expect = |c: f32, bg: f32| ((c * 0.5 + bg / 255.0 * 0.5) * 255.0).round() as u8;
    assert!(near(
        from_style,
        [expect(0.27, 24.0), expect(0.4, 24.0), expect(0.8, 24.0)],
        2
    ));
}

/// A line view inside a gate: the gate's inherited text style changes while the line is not
/// notified, so the line is only rebuilt because its inherited text style changed.
struct Line;

impl Render for Line {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(RichText::new("line", "gated text"))
    }
}

struct Gate {
    locked: Rc<Cell<bool>>,
    line: Entity<Line>,
}

impl Render for Gate {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        page(
            div()
                .user_select(!self.locked.get())
                .child(self.line.clone()),
        )
    }
}

#[test]
fn a_retained_view_follows_a_change_of_the_inherited_user_select() {
    let locked = Rc::new(Cell::new(false));
    let gate: Rc<std::cell::RefCell<Option<Entity<Gate>>>> = Rc::default();
    let build: Build = Rc::new({
        let locked = locked.clone();
        let gate = gate.clone();
        move |_, cx| {
            if gate.borrow().is_none() {
                let line = cx.new(|_| Line);
                *gate.borrow_mut() = Some(cx.new(|_| Gate {
                    locked: locked.clone(),
                    line,
                }));
            }
            let entity = gate.borrow().clone().expect("created above");
            div().size_full().child(entity).into_any_element()
        }
    });
    let mut harness = Harness::new(WIDTH, 100., build);
    let service = harness.service();
    assert_eq!(service.registered_runs(), 1);
    for expected_locked in [true, false, true] {
        locked.set(expected_locked);
        let entity = gate.borrow().clone().unwrap();
        harness
            .cx
            .update(|cx| entity.update(cx, |_, cx| cx.notify()));
        // An ordinary frame: unchanged views are replayed, the changed inherited style is not.
        harness.draw();
        assert_eq!(
            service.registered_runs(),
            usize::from(!expected_locked),
            "locked = {expected_locked}"
        );
    }
    harness.press(at(10., 12.), MouseButton::Left, 2);
    harness.release(at(10., 12.), MouseButton::Left, 2);
    assert!(
        service.selected_text().is_none(),
        "locked text does not select"
    );
}
