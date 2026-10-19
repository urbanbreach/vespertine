//! THM-003 A2-A5: appearance modes, fallback resolution, style generation and the Linux
//! window rule.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Mutex, MutexGuard},
};
use vespertine::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    StyleRefinement, Styled as _, TestAppContext, Window, WindowAppearance,
    WindowBackgroundAppearance, canvas, div,
};
use vespertine_theme::{
    appearance::{
        self, StyleCache, current_appearance, style_generation, theme_of, window_glass_alpha,
    },
    builtin::neutral_variant,
    color::Color,
    selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
    theme::{
        Appearance, DEFAULT_DARK_ID, DEFAULT_LIGHT_ID, SurfaceTreatment, ThemeFamily,
        TypographyRoles,
    },
};

/// The style generation is process-wide, so tests that assert on it run one at a time.
static GENERATION_LOCK: Mutex<()> = Mutex::new(());

fn serialize() -> MutexGuard<'static, ()> {
    GENERATION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn init(cx: &mut TestAppContext, mode: AppearanceMode) {
    cx.update(|cx| {
        appearance::init(
            cx,
            mode,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        )
    });
}

fn custom_family(id: &str, appearance: Appearance, background: Color) -> ThemeFamily {
    let mut variant = neutral_variant(appearance);
    variant.id = format!(
        "{id}-{}",
        if appearance.is_dark() {
            "dark"
        } else {
            "light"
        }
    );
    variant.family_id = id.to_string();
    variant.name = id.to_string();
    variant.colors.background = background;
    variant.curated = false;
    ThemeFamily {
        id: id.to_string(),
        name: id.to_string(),
        variants: vec![variant],
    }
}

fn active_id(cx: &mut TestAppContext) -> String {
    cx.update(|cx| theme_of(cx).id.clone())
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[test]
fn appearance_mode_defaults_to_system_and_serializes_lowercase() {
    assert_eq!(AppearanceMode::default(), AppearanceMode::System);
    for (mode, text) in [
        (AppearanceMode::System, "\"system\""),
        (AppearanceMode::Light, "\"light\""),
        (AppearanceMode::Dark, "\"dark\""),
    ] {
        assert_eq!(serde_json::to_string(&mode).unwrap(), text);
        assert_eq!(serde_json::from_str::<AppearanceMode>(text).unwrap(), mode);
    }
}

#[vespertine::test]
fn system_mode_follows_the_recorded_platform_value(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::System);
    cx.update(|cx| {
        appearance::set_platform_appearance(cx, Appearance::Light);
        assert_eq!(theme_of(cx).id, DEFAULT_LIGHT_ID);
        assert_eq!(current_appearance(), Appearance::Light);
        appearance::set_platform_appearance(cx, Appearance::Dark);
        assert_eq!(theme_of(cx).id, DEFAULT_DARK_ID);
        assert_eq!(current_appearance(), Appearance::Dark);
        assert_eq!(appearance::effective_appearance(cx), Appearance::Dark);
    });
}

#[vespertine::test]
fn pinned_modes_ignore_but_record_the_platform_value(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Light);
    cx.update(|cx| {
        appearance::set_platform_appearance(cx, Appearance::Dark);
        assert_eq!(appearance::platform_appearance(cx), Appearance::Dark);
        assert_eq!(theme_of(cx).id, DEFAULT_LIGHT_ID);

        appearance::set_mode(cx, AppearanceMode::Dark);
        appearance::set_platform_appearance(cx, Appearance::Light);
        assert_eq!(appearance::platform_appearance(cx), Appearance::Light);
        assert_eq!(theme_of(cx).id, DEFAULT_DARK_ID);

        // Switching back to system uses the recorded value immediately.
        appearance::set_platform_appearance(cx, Appearance::Dark);
        appearance::set_mode(cx, AppearanceMode::Light);
        appearance::set_mode(cx, AppearanceMode::System);
        assert_eq!(theme_of(cx).id, DEFAULT_DARK_ID);
        appearance::set_platform_appearance(cx, Appearance::Light);
        assert_eq!(theme_of(cx).id, DEFAULT_LIGHT_ID);
    });
}

#[vespertine::test]
fn window_appearance_notification_reresolves(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::System);
    let window = cx.add_window(|_, _| Empty);
    window
        .update(cx, |_, window, cx| appearance::observe_window(window, cx))
        .unwrap();
    let test_window = cx.test_window(window.into());
    assert_eq!(active_id(cx), DEFAULT_LIGHT_ID);

    test_window.simulate_appearance_change(WindowAppearance::Dark);
    cx.run_until_parked();
    assert_eq!(active_id(cx), DEFAULT_DARK_ID);
    test_window.simulate_appearance_change(WindowAppearance::VibrantLight);
    cx.run_until_parked();
    assert_eq!(active_id(cx), DEFAULT_LIGHT_ID);
}

#[vespertine::test]
fn unknown_selection_falls_back_without_mutating_it(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    cx.update(|cx| {
        appearance::set_theme(cx, Appearance::Dark, "midnight-dark");
        appearance::set_theme(cx, Appearance::Light, "midnight-light");
        assert_eq!(theme_of(cx).id, DEFAULT_DARK_ID);
        assert_eq!(appearance::selection(cx).dark, "midnight-dark");
        assert_eq!(appearance::selection(cx).light, "midnight-light");

        let before = appearance::selection(cx).clone();
        appearance::apply_registry_change(
            cx,
            vec![custom_family(
                "midnight",
                Appearance::Dark,
                Color::rgb(1, 2, 3),
            )],
            &[],
        );
        // The same, unchanged selection now resolves to the newly registered theme.
        assert_eq!(theme_of(cx).id, "midnight-dark");
        assert_eq!(theme_of(cx).colors.background, Color::rgb(1, 2, 3));
        assert_eq!(*appearance::selection(cx), before);

        // The unavailable light selection still falls back to the built-in light variant.
        appearance::set_mode(cx, AppearanceMode::Light);
        assert_eq!(theme_of(cx).id, DEFAULT_LIGHT_ID);
        assert_eq!(appearance::selection(cx).light, "midnight-light");
    });
}

#[vespertine::test]
fn explicit_removal_repoints_only_the_removed_selection(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    cx.update(|cx| {
        let family = custom_family("midnight", Appearance::Dark, Color::rgb(1, 2, 3));
        appearance::apply_registry_change(cx, vec![family], &[]);
        appearance::set_theme(cx, Appearance::Dark, "midnight-dark");
        appearance::set_theme(cx, Appearance::Light, "never-installed");
        assert_eq!(theme_of(cx).id, "midnight-dark");

        let repointed =
            appearance::apply_registry_change(cx, Vec::new(), &["midnight-dark".to_string()]);
        assert!(repointed);
        assert_eq!(theme_of(cx).id, DEFAULT_DARK_ID);
        // Repointed to the first dark variant in registry order; unrelated unavailable
        // selections are preserved.
        assert_eq!(appearance::selection(cx).dark, DEFAULT_DARK_ID);
        assert_eq!(appearance::selection(cx).light, "never-installed");
    });
}

#[vespertine::test]
fn generation_advances_when_inputs_move_and_not_otherwise(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    cx.update(|cx| {
        let family = custom_family("midnight", Appearance::Dark, Color::rgb(1, 2, 3));
        appearance::apply_registry_change(cx, vec![family], &[]);

        let mut last = style_generation();
        let mut expect_advance = |cx: &mut App, change: &dyn Fn(&mut App), what: &str| {
            change(cx);
            let now = style_generation();
            assert_eq!(
                now,
                last.wrapping_add(1),
                "{what} must advance the generation"
            );
            last = now;
            assert_eq!(theme_of(cx).generation, now);
        };
        expect_advance(
            cx,
            &|cx| appearance::set_mode(cx, AppearanceMode::Light),
            "appearance",
        );
        expect_advance(
            cx,
            &|cx| appearance::set_mode(cx, AppearanceMode::Dark),
            "appearance back",
        );
        expect_advance(
            cx,
            &|cx| appearance::set_theme(cx, Appearance::Dark, "midnight-dark"),
            "variant",
        );
        expect_advance(
            cx,
            &|cx| appearance::set_accent(cx, AccentSelection::Color(Color::rgb(255, 0, 0))),
            "accent",
        );
        expect_advance(
            cx,
            &|cx| appearance::set_surface(cx, SurfacePreference::Frosted),
            "surface preference",
        );
        expect_advance(
            cx,
            &|cx| {
                appearance::set_typography(
                    cx,
                    TypographyRoles {
                        ui_size: 20.0,
                        ..TypographyRoles::default()
                    },
                )
            },
            "typography",
        );
        expect_advance(
            cx,
            &|cx| appearance::set_hint(cx, Some(Color::rgb(20, 70, 200))),
            "hint",
        );

        // Unchanged inputs: no advance.
        let settled = style_generation();
        appearance::apply(cx);
        appearance::set_mode(cx, AppearanceMode::Dark);
        appearance::set_surface(cx, SurfacePreference::Frosted);
        appearance::set_platform_appearance(cx, Appearance::Light);
        assert_eq!(style_generation(), settled);
        assert_eq!(theme_of(cx).generation, settled);
    });
}

#[vespertine::test]
fn forced_reinstall_advances_even_for_an_identical_selection(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    cx.update(|cx| {
        let family = custom_family("midnight", Appearance::Dark, Color::rgb(1, 2, 3));
        appearance::apply_registry_change(cx, vec![family], &[]);
        appearance::set_theme(cx, Appearance::Dark, "midnight-dark");
        let before = style_generation();
        // The linked theme keeps its identifier while its colors change.
        let edited = custom_family("midnight", Appearance::Dark, Color::rgb(9, 9, 9));
        appearance::apply_registry_change(cx, vec![edited], &[]);
        assert_eq!(style_generation(), before.wrapping_add(1));
        assert_eq!(theme_of(cx).colors.background, Color::rgb(9, 9, 9));
    });
}

#[vespertine::test]
fn style_cache_drops_entries_when_the_generation_moves(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    let computed = Rc::new(Cell::new(0));
    let mut cache = StyleCache::<&'static str, u32>::new();
    let lookup = |cache: &mut StyleCache<&'static str, u32>| {
        let computed = computed.clone();
        *cache.get_or_insert_with("key", move || {
            computed.set(computed.get() + 1);
            7
        })
    };
    assert_eq!(lookup(&mut cache), 7);
    assert_eq!(lookup(&mut cache), 7);
    assert_eq!(computed.get(), 1);
    assert_eq!(cache.len(), 1);

    cx.update(|cx| appearance::set_mode(cx, AppearanceMode::Light));
    assert_eq!(cache.len(), 0, "the stale entry is discarded");
    assert_eq!(lookup(&mut cache), 7);
    assert_eq!(computed.get(), 2);
}

#[vespertine::test]
fn window_background_is_reapplied_whether_or_not_inputs_moved(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    let first = cx.add_window(|_, _| Empty);
    let second = cx.add_window(|_, _| Empty);
    let windows = [cx.test_window(first.into()), cx.test_window(second.into())];
    cx.run_until_parked();

    let counts = || {
        windows
            .each_ref()
            .map(|window| window.background_appearance_set_count())
    };
    let before = counts();
    // Inputs moved.
    cx.update(|cx| appearance::set_mode(cx, AppearanceMode::Light));
    let after_moved = counts();
    for (before, after) in before.iter().zip(&after_moved) {
        assert_eq!(*after, before + 1);
    }
    // Nothing moved: steps (a)-(c) are skipped but the background is still reapplied.
    let generation = style_generation();
    cx.update(|cx| appearance::apply(cx));
    assert_eq!(style_generation(), generation);
    for (before, after) in after_moved.iter().zip(&counts()) {
        assert_eq!(*after, before + 1);
    }
    for window in &windows {
        use vespertine::PlatformWindow as _;
        assert_eq!(
            window.background_appearance(),
            WindowBackgroundAppearance::Transparent
        );
    }
}

struct CachedChild {
    paints: Rc<Cell<usize>>,
}

impl Render for CachedChild {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let paints = self.paints.clone();
        canvas(|_, _, _| (), move |_, _, _, _| paints.set(paints.get() + 1)).size_full()
    }
}

struct CachedRoot {
    child: Entity<CachedChild>,
}

impl Render for CachedRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.child
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

#[vespertine::test]
fn applying_a_change_refreshes_windows_with_paint_caching_disabled(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    let paints = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let paints = paints.clone();
        move |_, cx| CachedRoot {
            child: cx.new(|_| CachedChild { paints }),
        }
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap()
    };
    draw(cx);
    let baseline = paints.get();
    assert!(baseline >= 1);
    draw(cx);
    assert_eq!(paints.get(), baseline, "an untouched cached view is reused");

    // Unchanged inputs do not refresh: the cached view is still reused.
    cx.update(|cx| appearance::apply(cx));
    draw(cx);
    assert_eq!(paints.get(), baseline);

    // A moved input refreshes every window and disables caching for the next frame only.
    cx.update(|cx| appearance::set_mode(cx, AppearanceMode::Light));
    draw(cx);
    assert_eq!(paints.get(), baseline + 1);
    draw(cx);
    assert_eq!(paints.get(), baseline + 1);
}

#[vespertine::test]
fn frosted_preference_keeps_stored_choice_and_window_glass_opaque(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    cx.update(|cx| {
        let recommended = theme_of(cx).variant.surface;
        appearance::set_surface(cx, SurfacePreference::Frosted);
        assert_eq!(appearance::surface(cx), SurfacePreference::Frosted);
        assert_eq!(theme_of(cx).treatment, SurfaceTreatment::Frosted);
        assert_eq!(theme_of(cx).variant.surface, recommended);
        assert_eq!(theme_of(cx).window_glass_alpha(), 1.0);
        assert_eq!(window_glass_alpha(SurfaceTreatment::Frosted), 1.0);

        appearance::set_surface(cx, SurfacePreference::Opaque);
        assert_eq!(theme_of(cx).treatment, SurfaceTreatment::Opaque);
        assert_eq!(theme_of(cx).window_glass_alpha(), 1.0);

        appearance::set_surface(cx, SurfacePreference::ThemeDefault);
        assert_eq!(theme_of(cx).treatment, recommended);
        assert_eq!(
            SurfacePreference::ThemeDefault.resolve(SurfaceTreatment::Frosted),
            SurfaceTreatment::Frosted
        );
    });
}

#[vespertine::test]
fn accent_and_surface_overrides_touch_nothing_else(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    cx.update(|cx| {
        let before = theme_of(cx).variant.clone();
        appearance::set_accent(cx, AccentSelection::Color(Color::rgb(250, 220, 30)));
        appearance::set_surface(cx, SurfacePreference::Frosted);
        let after = theme_of(cx).variant.clone();
        assert_ne!(after.accent, before.accent);
        assert_eq!(after.colors, before.colors);
        assert_eq!(after.syntax, before.syntax);
        assert_eq!(after.terminal, before.terminal);
        assert_eq!(appearance::selection(cx), &ThemeSelection::default());
        assert_eq!(after.surface, before.surface);
    });
}

#[vespertine::test]
fn typography_is_carried_by_the_resolved_theme(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    cx.update(|cx| {
        let roles = TypographyRoles {
            ui_family: "Noto Sans".into(),
            ui_size: 18.0,
            ..TypographyRoles::default()
        };
        appearance::set_typography(cx, roles.clone());
        assert_eq!(theme_of(cx).typography, roles);
    });
}

#[vespertine::test]
fn a_window_opened_with_a_pinned_mode_shows_the_palette_in_its_first_frame(
    cx: &mut TestAppContext,
) {
    let _guard = serialize();
    init(cx, AppearanceMode::Dark);
    struct Probe(Rc<RefCell<Vec<Color>>>);
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.borrow_mut().push(theme_of(cx).colors.background);
            div()
        }
    }
    let frames = Rc::new(RefCell::new(Vec::new()));
    // The test window reports a light platform appearance, but the pinned dark mode holds.
    let window = cx.add_window({
        let frames = frames.clone();
        move |window, cx| {
            appearance::observe_window(window, cx);
            Probe(frames)
        }
    });
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let dark_background = neutral_variant(Appearance::Dark).colors.background;
    assert_eq!(frames.borrow().first().copied(), Some(dark_background));
    assert!(
        frames
            .borrow()
            .iter()
            .all(|color| *color == dark_background)
    );
    assert_eq!(
        cx.update(|cx| appearance::platform_appearance(cx)),
        Appearance::Light
    );
}

/// THM-003 A1 (logic): the two-window example's guarantees, headless. Every change repaints
/// every window with the new generation and palette, and a window opened afterwards paints the
/// active palette in its first frame.
#[vespertine::test]
fn every_window_repaints_the_new_palette_and_new_windows_start_current(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx, AppearanceMode::System);
    cx.update(|cx| {
        appearance::apply_registry_change(
            cx,
            vec![custom_family(
                "midnight",
                Appearance::Dark,
                Color::rgb(1, 2, 3),
            )],
            &[],
        );
    });
    #[derive(Clone, Copy, PartialEq, Debug)]
    struct Painted {
        generation: u32,
        background: Color,
    }
    struct Surface(Rc<Cell<Option<Painted>>>);
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = theme_of(cx);
            self.0.set(Some(Painted {
                generation: theme.generation,
                background: theme.colors.background,
            }));
            div()
        }
    }
    let open = |cx: &mut TestAppContext| {
        let painted = Rc::new(Cell::new(None));
        let window = cx.add_window({
            let painted = painted.clone();
            move |window, cx| {
                appearance::observe_window(window, cx);
                Surface(painted)
            }
        });
        (window, painted)
    };
    let draw = |cx: &mut TestAppContext, window: vespertine::AnyWindowHandle| {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    let (first, first_painted) = open(cx);
    let (second, second_painted) = open(cx);
    let windows: Vec<(vespertine::AnyWindowHandle, _)> = vec![
        (first.into(), first_painted),
        (second.into(), second_painted),
    ];
    for (window, _) in &windows {
        draw(cx, *window);
    }

    let steps: [(&str, &dyn Fn(&mut App)); 4] = [
        ("pin light", &|cx| {
            appearance::set_mode(cx, AppearanceMode::Light)
        }),
        ("pin dark", &|cx| {
            appearance::set_mode(cx, AppearanceMode::Dark)
        }),
        ("select the custom variant", &|cx| {
            appearance::set_theme(cx, Appearance::Dark, "midnight-dark")
        }),
        ("accent", &|cx| {
            appearance::set_accent(cx, AccentSelection::Color(Color::rgb(20, 70, 200)))
        }),
    ];
    for (name, step) in steps {
        cx.update(|cx| step(cx));
        for (window, _) in &windows {
            draw(cx, *window);
        }
        let (generation, background) =
            cx.update(|cx| (theme_of(cx).generation, theme_of(cx).colors.background));
        assert_eq!(generation, style_generation(), "{name}");
        for (_, painted) in &windows {
            assert_eq!(
                painted.get(),
                Some(Painted {
                    generation,
                    background
                }),
                "{name}: a window painted a stale palette"
            );
        }
    }

    // A window opened afterwards shows the active palette in its first frame.
    let (late, late_painted) = open(cx);
    draw(cx, late.into());
    let (generation, background) =
        cx.update(|cx| (theme_of(cx).generation, theme_of(cx).colors.background));
    assert_eq!(
        late_painted.get(),
        Some(Painted {
            generation,
            background
        })
    );
    assert_eq!(background, Color::rgb(1, 2, 3));
}

/// THM-003 R18 with the TXT-004 typography global: a typography change is a theme change.
#[vespertine::test]
fn typography_global_changes_are_theme_changes(cx: &mut TestAppContext) {
    use vespertine::{FontCatalog, Typography, TypographySettings};
    let _guard = serialize();
    let before = Cell::new(0);
    cx.update(|cx| {
        let catalog = FontCatalog::register(cx, &[]);
        let settings = TypographySettings::defaults(&catalog);
        Typography::init(catalog, settings, cx);
        // Typography exists before the theme state: init binds to it.
        appearance::init(
            cx,
            AppearanceMode::Dark,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        );
        assert_eq!(
            theme_of(cx).typography,
            TypographyRoles::from_typography(Typography::global(cx))
        );
    });
    cx.update(|cx| {
        before.set(style_generation());
        assert!(Typography::set_code_size(18.0, cx));
    });
    // Observers run when the effect queue flushes.
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(theme_of(cx).typography.code_size, 18.0);
        assert_eq!(style_generation(), before.get().wrapping_add(1));
        assert_eq!(
            theme_of(cx).typography,
            TypographyRoles::from_typography(Typography::global(cx))
        );
    });
}

/// The typography global installed after the theme state is followed too: the binding does not
/// depend on the initialization order, and the generation of every accepted change reaches the
/// theme.
#[vespertine::test]
fn typography_installed_after_the_theme_is_followed(cx: &mut TestAppContext) {
    use vespertine::{FontCatalog, Typography, TypographySettings};
    let _guard = serialize();
    cx.update(|cx| {
        appearance::init(
            cx,
            AppearanceMode::Dark,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        );
        assert_eq!(theme_of(cx).typography, TypographyRoles::default());
        let catalog = FontCatalog::register(cx, &[]);
        let settings = TypographySettings::defaults(&catalog);
        Typography::init(catalog, settings, cx);
    });
    cx.run_until_parked();
    let before = Cell::new(0);
    cx.update(|cx| {
        assert_eq!(
            theme_of(cx).typography,
            TypographyRoles::from_typography(Typography::global(cx)),
            "installing the global is a typography change"
        );
        before.set(style_generation());
        let typography_generation = Typography::global(cx).generation();
        assert!(Typography::set_code_size(19.0, cx));
        assert_ne!(Typography::global(cx).generation(), typography_generation);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(theme_of(cx).typography.code_size, 19.0);
        assert_eq!(style_generation(), before.get().wrapping_add(1));
    });
}
