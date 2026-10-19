//! THM-004 A5-A7: palette hint extraction, the asynchronous service and the tint recipe.

mod support;

use anyhow::anyhow;
use std::{
    cell::RefCell,
    io::Cursor,
    rc::Rc,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use support::hex;
use vespertine::TestAppContext;
use vespertine_theme::{
    appearance::{self, style_generation, theme_of},
    builtin::{neutral_family, neutral_variant},
    color::Color,
    hint::{
        HintImage, HintSource, TintRecipe, extract, extract_from_encoded, set_hint_extractor,
        set_hint_image, tint_variant,
    },
    selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
    theme::Appearance,
};

static GENERATION_LOCK: Mutex<()> = Mutex::new(());

fn serialize() -> MutexGuard<'static, ()> {
    GENERATION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn pixels(groups: &[(usize, [u8; 4])]) -> Vec<u8> {
    groups
        .iter()
        .flat_map(|(count, pixel)| std::iter::repeat(*pixel).take(*count).flatten())
        .collect()
}

// --- A5 --------------------------------------------------------------------------------------

#[test]
fn extraction_picks_the_heaviest_bin_and_returns_its_weighted_mean() {
    let input = pixels(&[
        (100, [90, 90, 90, 255]),
        (80, [30, 130, 220, 255]),
        (1000, [255, 0, 0, 0]),
    ]);
    assert_eq!(extract(&input), Some(Color::rgb(30, 130, 220)));
    assert_eq!(
        extract(&pixels(&[(10, [100, 100, 100, 255])])),
        Some(Color::rgb(100, 100, 100))
    );
}

#[test]
fn extraction_without_countable_pixels_is_none() {
    assert_eq!(extract(&[]), None);
    assert_eq!(extract(&pixels(&[(50, [10, 200, 30, 127])])), None);
    // A trailing partial pixel is ignored.
    assert_eq!(extract(&[1, 2, 3]), None);
    // Alpha exactly at the cutoff counts.
    assert_eq!(
        extract(&pixels(&[(1, [10, 200, 30, 128])])),
        Some(Color::rgb(10, 200, 30))
    );
}

#[test]
fn ties_go_to_the_highest_bin_index() {
    // Both groups are neutral with equal counts, hence equal weight: the bin with the higher
    // index ((R>>4)<<8 | (G>>4)<<4 | B>>4) wins.
    let input = pixels(&[(5, [16, 16, 16, 255]), (5, [200, 200, 200, 255])]);
    assert_eq!(extract(&input), Some(Color::rgb(200, 200, 200)));
    let reversed = pixels(&[(5, [200, 200, 200, 255]), (5, [16, 16, 16, 255])]);
    assert_eq!(extract(&reversed), Some(Color::rgb(200, 200, 200)));
}

#[test]
fn a_saturated_minority_beats_a_larger_neutral_majority() {
    // 100 greys weigh 100 * 0.2 = 20; 30 pure reds weigh 30 * 1.2 = 36.
    let input = pixels(&[(100, [128, 128, 128, 255]), (30, [255, 0, 0, 255])]);
    assert_eq!(extract(&input), Some(Color::rgb(255, 0, 0)));
    // With few reds the majority still wins: 10 * 1.2 = 12 < 20.
    let input = pixels(&[(100, [128, 128, 128, 255]), (10, [255, 0, 0, 255])]);
    assert_eq!(extract(&input), Some(Color::rgb(128, 128, 128)));
    // The weight scales with alpha / 255.
    let input = pixels(&[(10, [255, 0, 0, 200]), (9, [0, 0, 255, 255])]);
    assert_eq!(extract(&input), Some(Color::rgb(0, 0, 255)));
}

#[test]
fn extraction_of_a_64_by_64_buffer_is_deterministic() {
    let mut buffer = Vec::new();
    for y in 0..64u32 {
        for x in 0..64u32 {
            let value = (x * 4) as u8;
            buffer.extend_from_slice(&[value, (y * 4) as u8, value ^ (y as u8), 255]);
        }
    }
    let first = extract(&buffer);
    assert!(first.is_some());
    assert_eq!(first, extract(&buffer));
    // Order matters only through the tie rule: a rotated buffer is itself deterministic.
    let mut rotated = buffer.clone();
    rotated.rotate_left(4 * 100);
    assert_eq!(extract(&rotated), extract(&rotated));
}

fn png(color: [u8; 4], size: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(size, size, image::Rgba(color));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn encoded_images_are_reduced_to_a_thumbnail_first() {
    let bytes = png([30, 130, 220, 255], 300);
    assert_eq!(extract_from_encoded(&bytes).unwrap(), Some(hex("#1e82dc")));
    assert!(extract_from_encoded(&[1, 2, 3, 4]).is_err());
    assert_eq!(extract_from_encoded(&png([5, 5, 5, 0], 10)).unwrap(), None);
}

// --- A6 --------------------------------------------------------------------------------------

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        appearance::init(
            cx,
            AppearanceMode::Dark,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        )
    });
}

fn current_hint(cx: &mut TestAppContext) -> Option<Color> {
    cx.update(|cx| appearance::hint(cx).0)
}

#[vespertine::test]
fn the_hint_is_computed_on_the_background_executor(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let on_foreground = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let executor = cx.executor();
    cx.update(|cx| {
        set_hint_extractor(
            cx,
            Arc::new({
                let on_foreground = on_foreground.clone();
                let calls = calls.clone();
                let executor = executor.clone();
                move |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    on_foreground.store(executor.is_main_thread(), Ordering::SeqCst);
                    Ok(Some(Color::rgb(10, 20, 30)))
                }
            }),
        );
        set_hint_image(cx, Some(HintImage::bytes("a", Vec::<u8>::new())));
    });
    // Nothing was computed synchronously on the caller's (foreground) thread.
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(current_hint(cx), None);
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        !on_foreground.load(Ordering::SeqCst),
        "extraction ran off the foreground"
    );
    assert_eq!(current_hint(cx), Some(Color::rgb(10, 20, 30)));
}

#[vespertine::test]
fn a_request_for_a_pending_image_is_not_started_twice(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let calls = Arc::new(AtomicUsize::new(0));
    cx.update(|cx| {
        set_hint_extractor(
            cx,
            Arc::new({
                let calls = calls.clone();
                move |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(Color::rgb(1, 2, 3)))
                }
            }),
        );
        set_hint_image(cx, Some(HintImage::bytes("same", Vec::<u8>::new())));
        set_hint_image(cx, Some(HintImage::bytes("same", Vec::<u8>::new())));
    });
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Once delivered, a new request for the image computes again.
    cx.update(|cx| set_hint_image(cx, Some(HintImage::bytes("same", Vec::<u8>::new()))));
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[vespertine::test]
fn a_result_for_a_replaced_image_is_discarded(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    cx.update(|cx| {
        set_hint_extractor(
            cx,
            Arc::new(|source| match source {
                HintSource::Bytes(bytes) if bytes.first() == Some(&1) => {
                    Ok(Some(Color::rgb(255, 0, 0)))
                }
                _ => Ok(Some(Color::rgb(0, 0, 255))),
            }),
        );
    });
    // Record every hint value the theme state ever holds.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let _subscription = cx.update(|cx| {
        let seen = seen.clone();
        appearance::observe_changes(cx, move |cx| seen.borrow_mut().push(appearance::hint(cx).0))
    });
    cx.update(|cx| {
        set_hint_image(cx, Some(HintImage::bytes("first", vec![1u8])));
        set_hint_image(cx, Some(HintImage::bytes("second", vec![2u8])));
    });
    cx.run_until_parked();
    assert_eq!(current_hint(cx), Some(Color::rgb(0, 0, 255)));
    assert!(
        seen.borrow()
            .iter()
            .all(|hint| *hint != Some(Color::rgb(255, 0, 0))),
        "the stale result was applied: {:?}",
        seen.borrow()
    );
}

#[vespertine::test]
fn decode_errors_and_empty_results_keep_the_previous_hint(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    // The default extractor: a real PNG, then undecodable bytes.
    cx.update(|cx| {
        set_hint_image(
            cx,
            Some(HintImage::bytes("png", png([30, 130, 220, 255], 40))),
        )
    });
    cx.run_until_parked();
    assert_eq!(current_hint(cx), Some(Color::rgb(30, 130, 220)));
    cx.update(|cx| set_hint_image(cx, Some(HintImage::bytes("junk", vec![0u8, 1, 2, 3]))));
    cx.run_until_parked();
    assert_eq!(
        current_hint(cx),
        Some(Color::rgb(30, 130, 220)),
        "decode failure keeps the hint"
    );
    // A missing file is a decode error too.
    cx.update(|cx| set_hint_image(cx, Some(HintImage::file("/nonexistent/wallpaper.png"))));
    cx.run_until_parked();
    assert_eq!(current_hint(cx), Some(Color::rgb(30, 130, 220)));
    // A fully transparent image yields no result.
    cx.update(|cx| set_hint_image(cx, Some(HintImage::bytes("clear", png([9, 9, 9, 0], 8)))));
    cx.run_until_parked();
    assert_eq!(current_hint(cx), Some(Color::rgb(30, 130, 220)));
    // A failing custom extractor behaves the same.
    cx.update(|cx| {
        set_hint_extractor(cx, Arc::new(|_| Err(anyhow!("boom"))));
        set_hint_image(cx, Some(HintImage::bytes("boom", Vec::<u8>::new())));
    });
    cx.run_until_parked();
    assert_eq!(current_hint(cx), Some(Color::rgb(30, 130, 220)));
    // Removing the image removes the hint.
    cx.update(|cx| set_hint_image(cx, None));
    assert_eq!(current_hint(cx), None);
}

#[vespertine::test]
fn a_late_result_after_removal_is_discarded(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    cx.update(|cx| {
        set_hint_extractor(cx, Arc::new(|_| Ok(Some(Color::rgb(1, 1, 1)))));
        set_hint_image(cx, Some(HintImage::bytes("x", Vec::<u8>::new())));
        set_hint_image(cx, None);
    });
    cx.run_until_parked();
    assert_eq!(current_hint(cx), None);
}

#[vespertine::test]
fn hint_changes_update_the_theme_and_refresh_windows(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let before = cx.update(|cx| theme_of(cx).variant.clone());
    let generation = style_generation();
    cx.update(|cx| appearance::set_hint(cx, Some(hex("#fadc1e"))));
    assert_eq!(style_generation(), generation.wrapping_add(1));
    let tinted = cx.update(|cx| theme_of(cx).variant.clone());
    assert_ne!(tinted.colors.background, before.colors.background);

    // Toggling the feature restores the manually chosen palette exactly; the stored hint stays.
    cx.update(|cx| appearance::set_hint_enabled(cx, false));
    assert_eq!(cx.update(|cx| theme_of(cx).variant.clone()), before);
    assert_eq!(
        cx.update(|cx| appearance::hint(cx)),
        (Some(hex("#fadc1e")), false)
    );
    cx.update(|cx| appearance::set_hint_enabled(cx, true));
    assert_eq!(cx.update(|cx| theme_of(cx).variant.clone()), tinted);
    cx.update(|cx| appearance::set_hint(cx, None));
    assert_eq!(cx.update(|cx| theme_of(cx).variant.clone()), before);
    // Stored user choices are never modified by the recipe.
    cx.update(|cx| {
        assert_eq!(appearance::selection(cx), &ThemeSelection::default());
        assert_eq!(appearance::accent(cx), AccentSelection::ThemeDefault);
        assert_eq!(appearance::surface(cx), SurfacePreference::ThemeDefault);
    });
}

// --- A7 --------------------------------------------------------------------------------------

#[test]
fn the_tint_recipe_keeps_accessibility_and_semantic_colors() {
    let recipe = TintRecipe::default();
    let hints = [
        hex("#000000"),
        hex("#ffffff"),
        hex("#fadc1e"),
        hex("#1446c8"),
    ];
    for variant in neutral_family().variants {
        for hint in hints {
            let tinted = tint_variant(&variant, hint, &recipe);
            let label = format!("{} with {hint}", variant.id);
            let background = tinted.colors.background;
            assert!(tinted.accent.primary.contrast(background) >= 3.0, "{label}");
            assert!(
                tinted.accent.on.contrast(tinted.accent.strong) >= 4.5,
                "{label}"
            );
            // Semantic roles are unchanged.
            for (name, before, after) in [
                ("danger", variant.colors.danger, tinted.colors.danger),
                ("success", variant.colors.success, tinted.colors.success),
                ("warning", variant.colors.warning, tinted.colors.warning),
                ("diffAdd", variant.colors.diff_add, tinted.colors.diff_add),
                (
                    "diffDelete",
                    variant.colors.diff_delete,
                    tinted.colors.diff_delete,
                ),
                (
                    "diffHunk",
                    variant.colors.diff_hunk,
                    tinted.colors.diff_hunk,
                ),
            ] {
                assert_eq!(before, after, "{name} {label}");
            }
            assert_eq!(variant.syntax, tinted.syntax, "{label}");
            assert_eq!(variant.terminal.ansi, tinted.terminal.ansi, "{label}");
            // Text stays readable on the tinted surfaces.
            for surface in tinted.colors.text_surfaces() {
                assert!(tinted.colors.text.contrast(surface) >= 4.5, "{label}");
                assert!(tinted.colors.text_muted.contrast(surface) >= 4.5, "{label}");
            }
            assert!(
                tinted
                    .terminal
                    .foreground
                    .contrast(tinted.terminal.background)
                    >= 4.5,
                "{label}"
            );
            // Recipe constants.
            assert_eq!(
                tinted.colors.hover,
                tinted.accent.primary.with_alpha(0.09),
                "{label}"
            );
            assert_eq!(
                tinted.colors.active,
                tinted.accent.primary.with_alpha(0.15),
                "{label}"
            );
            assert_eq!(
                tinted.colors.border,
                tinted.accent.primary.with_alpha(0.14),
                "{label}"
            );
            assert_eq!(
                tinted.colors.border_strong,
                tinted.accent.primary.with_alpha(0.30),
                "{label}"
            );
            assert_eq!(
                tinted.terminal.selection, tinted.accent.selection,
                "{label}"
            );
        }
    }
}

#[test]
fn the_tint_moves_surfaces_toward_the_tinted_neutral() {
    let variant = neutral_variant(Appearance::Dark);
    let hint = hex("#1446c8");
    let tinted = tint_variant(&variant, hint, &TintRecipe::default());
    let tint = hint.mix(Color::BLACK, 0.88);
    assert_eq!(
        tinted.colors.background,
        variant.colors.background.mix(tint, 0.4)
    );
    assert_eq!(tinted.colors.card, variant.colors.card.mix(tint, 0.4));
    assert_eq!(
        tinted.terminal.background,
        variant.terminal.background.mix(tint, 0.4)
    );
    let light = neutral_variant(Appearance::Light);
    let tinted = tint_variant(&light, hint, &TintRecipe::default());
    assert_eq!(
        tinted.colors.background,
        light
            .colors
            .background
            .mix(hint.mix(Color::WHITE, 0.94), 0.4)
    );
}

#[vespertine::test]
fn the_hint_overrides_the_accent_only_while_it_exists(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    cx.update(|cx| {
        appearance::set_accent(cx, AccentSelection::Color(hex("#ff0000")));
        let manual = theme_of(cx).variant.clone();
        assert_eq!(
            manual.accent.primary,
            hex("#ff0000").ensure_contrast(manual.colors.background, 3.0)
        );
        appearance::set_hint(cx, Some(hex("#1446c8")));
        assert_ne!(theme_of(cx).variant.accent, manual.accent);
        appearance::set_hint(cx, None);
        assert_eq!(theme_of(cx).variant, manual);
        assert_eq!(
            appearance::accent(cx),
            AccentSelection::Color(hex("#ff0000"))
        );
    });
}
