//! THM-003 A6-A8 and R3: color model, helpers, accent derivation, hardening and frost coverage.

use vespertine::Hsla;
use vespertine_theme::{
    accent::{derive_accent, frosted_tint_alpha},
    builtin::{neutral_family, neutral_variant},
    color::{Color, harden_foreground, worst_contrast},
    theme::Appearance,
};

fn hex(text: &str) -> Color {
    Color::parse(text).unwrap()
}

#[test]
fn text_forms_parse_and_format() {
    assert_eq!(hex("#fff"), Color::rgb(255, 255, 255));
    assert_eq!(hex("#F0a8"), Color::rgba(0xff, 0x00, 0xaa, 0x88));
    assert_eq!(hex("  #AbCdEf  "), Color::rgb(0xab, 0xcd, 0xef));
    assert_eq!(hex("#11223344"), Color::rgba(0x11, 0x22, 0x33, 0x44));
    assert_eq!(hex("#ff0000").to_hex(), "#ff0000");
    assert_eq!(Color::rgba(255, 0, 0, 255).to_hex(), "#ff0000");
    assert_eq!(Color::rgba(255, 0, 0, 128).to_hex(), "#ff000080");
    for bad in [
        "",
        "ffffff",
        "#ff",
        "#fffff",
        "#fffffff",
        "#ggg",
        "#fffffffff",
        "rgb(0 0 0)",
    ] {
        assert!(Color::parse(bad).is_err(), "{bad:?} must not parse");
    }
    assert_eq!(
        serde_json::to_string(&hex("#0a0b0c")).unwrap(),
        "\"#0a0b0c\""
    );
    assert_eq!(
        serde_json::from_str::<Color>("\"#0A0B0C80\"").unwrap(),
        hex("#0a0b0c80")
    );
}

#[test]
fn paint_conversion_is_hsl_plus_alpha() {
    let paint: Hsla = hex("#ff000080").to_hsla();
    assert!((paint.h - 0.0).abs() < 1e-6);
    assert!((paint.s - 1.0).abs() < 1e-6);
    assert!((paint.l - 0.5).abs() < 1e-6);
    assert!((paint.a - 128.0 / 255.0).abs() < 1e-6);
}

#[test]
fn helpers_round_exactly() {
    assert_eq!(hex("#000000").mix(hex("#ffffff"), 0.5), hex("#808080"));
    assert_eq!(hex("#000000").mix(hex("#ffffff"), 2.0), hex("#ffffff"));
    assert_eq!(hex("#000000").mix(hex("#ffffff"), -1.0), hex("#000000"));
    // Alpha takes part in the mix.
    assert_eq!(
        hex("#00000000").mix(hex("#ffffffff"), 0.5),
        hex("#80808080")
    );
    assert_eq!(hex("#ffffff").with_alpha(0.5).a, 128);
    assert_eq!(hex("#ffffff").with_alpha(2.0).a, 255);
    assert_eq!(hex("#ffffff").with_alpha(-1.0).a, 0);
    assert_eq!(hex("#ff000080").blend_over(hex("#ffffff")), hex("#ff7f7f"));
    assert!((Color::WHITE.contrast(Color::BLACK) - 21.0).abs() < 1e-9);
    assert!((Color::BLACK.contrast(Color::WHITE) - 21.0).abs() < 1e-9);
    // A translucent foreground is blended over the background first.
    assert!((hex("#ffffff00").contrast(Color::BLACK) - 1.0).abs() < 1e-9);
    assert_eq!(hex("#555555").best_on_color(), Color::WHITE);
    assert_eq!(hex("#777777").best_on_color(), Color::BLACK);
}

#[test]
fn ensure_contrast_keeps_a_passing_color_and_ends_at_the_target() {
    let background = hex("#808080");
    let passing = Color::BLACK;
    assert_eq!(passing.ensure_contrast(background, 4.5), passing);

    // Unreachable minimum: the result is the target (black contrasts more with #808080).
    assert_eq!(
        hex("#777777").ensure_contrast(background, 22.0),
        Color::BLACK
    );
    // Dark background: the target is white.
    assert_eq!(
        hex("#222222").ensure_contrast(hex("#101010"), 22.0),
        Color::WHITE
    );
    // The first qualifying candidate of the 20 steps is returned.
    let raised = hex("#777777").ensure_contrast(background, 4.5);
    assert!(raised.contrast(background) >= 4.5);
    let steps_before: Vec<Color> = (1..=20)
        .map(|step| hex("#777777").mix(Color::BLACK, f64::from(step) / 20.0))
        .take_while(|candidate| *candidate != raised)
        .collect();
    assert!(
        steps_before
            .iter()
            .all(|candidate| candidate.contrast(background) < 4.5)
    );
}

#[test]
fn accent_derivation_meets_contrast_for_any_primary_over_any_background() {
    let primaries = [
        Color::BLACK,
        Color::WHITE,
        hex("#808080"),
        hex("#fadc1e"),
        hex("#1446c8"),
        hex("#ff0000"),
    ];
    for (background, appearance) in [
        (hex("#060606"), Appearance::Dark),
        (hex("#ffffff"), Appearance::Light),
    ] {
        for primary in primaries {
            let accent = derive_accent(primary, background, appearance);
            assert!(
                accent.primary.contrast(background) >= 3.0,
                "{primary} over {background}"
            );
            assert!(
                accent.on.contrast(accent.strong) >= 4.5,
                "{primary} over {background}"
            );
            assert_eq!(accent.caret, accent.primary);
            assert_eq!(accent.activity, accent.primary);
            assert_eq!(accent.glyph[1], accent.primary);
        }
    }
}

#[test]
fn accent_derivation_constants() {
    let primary = hex("#7c86ff");
    let dark = derive_accent(primary, hex("#060606"), Appearance::Dark);
    assert_eq!(dark.wash, primary.with_alpha(0.22));
    assert_eq!(dark.selection, primary.with_alpha(0.35));
    assert_eq!(dark.glyph[0], primary.mix(Color::WHITE, 0.28));
    assert_eq!(dark.glyph[2], primary.mix(Color::BLACK, 0.18));
    let light_background = hex("#ffffff");
    let light = derive_accent(hex("#4f39f6"), light_background, Appearance::Light);
    assert_eq!(light.wash.a, 31); // round(0.12 * 255)
    assert_eq!(light.selection.a, 61); // round(0.24 * 255)
    assert_eq!(light.glyph[0], hex("#4f39f6").mix(light_background, 0.18));
    assert_eq!(light.glyph[2], hex("#4f39f6").mix(Color::BLACK, 0.26));
    assert_eq!(light.on, Color::WHITE);
}

#[test]
fn hardening_raises_text_on_mid_grey_surfaces() {
    let surface = hex("#808080");
    let surfaces = [surface; 7];
    let text = hex("#777777");
    assert!(worst_contrast(text, &surfaces) < 4.5);
    let hardened = harden_foreground(text, &surfaces, 4.5, None);
    assert!(worst_contrast(hardened, &surfaces) >= 4.5);
    // A color that qualifies is returned untouched.
    assert_eq!(
        harden_foreground(Color::BLACK, &surfaces, 4.5, None),
        Color::BLACK
    );
    // The preferred target is tried first.
    let toward_white =
        harden_foreground(hex("#777777"), &[hex("#303030")], 4.5, Some(Color::WHITE));
    assert!(worst_contrast(toward_white, &[hex("#303030")]) >= 4.5);
    // Nothing reaches the minimum: the best candidate seen is returned.
    let best = harden_foreground(
        hex("#777777"),
        &[hex("#000000"), hex("#ffffff")],
        21.0,
        None,
    );
    let best_contrast = worst_contrast(best, &[hex("#000000"), hex("#ffffff")]);
    for step in 1..=100 {
        for target in [Color::BLACK, Color::WHITE] {
            let candidate = hex("#777777").mix(target, f64::from(step) / 100.0);
            assert!(worst_contrast(candidate, &[hex("#000000"), hex("#ffffff")]) <= best_contrast);
        }
    }
}

#[test]
fn custom_variants_are_hardened_and_curated_variants_are_authored() {
    let surface = hex("#808080");
    let mut custom = neutral_variant(Appearance::Dark);
    custom.curated = false;
    custom.colors.background = surface;
    custom.colors.shell = surface;
    custom.colors.raised = surface;
    custom.colors.card = surface;
    custom.colors.dialog = surface;
    custom.colors.overlay = surface;
    custom.colors.input = surface;
    custom.colors.solid = surface;
    custom.colors.text = hex("#777777");
    custom.colors.text_muted = hex("#7a7a7a");
    custom.colors.on_solid = hex("#777777");
    custom.terminal.background = surface;
    custom.terminal.foreground = hex("#777777");

    let hardened = custom.hardened();
    let surfaces = hardened.colors.text_surfaces();
    assert!(worst_contrast(hardened.colors.text, &surfaces) >= 4.5);
    assert!(worst_contrast(hardened.colors.text_muted, &surfaces) >= 4.5);
    assert!(hardened.colors.on_solid.contrast(hardened.colors.solid) >= 4.5);
    assert!(
        hardened
            .terminal
            .foreground
            .contrast(hardened.terminal.background)
            >= 4.5
    );
    // Only the foregrounds move.
    assert_eq!(hardened.colors.background, custom.colors.background);
    assert_eq!(hardened.accent, custom.accent);

    let mut curated = custom;
    curated.curated = true;
    assert_eq!(curated.hardened(), curated);
    for appearance in [Appearance::Dark, Appearance::Light] {
        let builtin = neutral_variant(appearance);
        assert!(builtin.curated);
        assert_eq!(builtin.hardened(), builtin);
    }
}

#[test]
fn builtin_family_follows_the_neutral_tokens() {
    let family = neutral_family();
    let dark = family.variant("default-dark").unwrap();
    let light = family.variant("default-light").unwrap();
    assert_eq!(dark.appearance, Appearance::Dark);
    assert_eq!(light.appearance, Appearance::Light);
    assert_eq!(dark.colors.background, hex("#060606"));
    assert_eq!(light.colors.background, hex("#ffffff"));
    assert_eq!(dark.colors.text, hex("#e5e5e5"));
    assert_eq!(light.colors.text, hex("#222222"));
    assert_eq!(dark.colors.border, hex("#ffffff14"));
    assert_eq!(light.colors.border, hex("#0000001a"));
    assert_eq!(dark.accent.primary, hex("#7c86ff"));
    assert_eq!(light.accent.primary, hex("#4f39f6"));
    assert_eq!(dark.colors.diff_add, hex("#00d492"));
    assert_eq!(light.colors.diff_delete, hex("#e7000b"));
    assert!(family.validate().is_empty(), "{:?}", family.validate());
}

#[test]
fn frosted_coverage_returns_the_first_qualifying_candidate() {
    // Dark appearance: the adverse backdrop is white. A black tint with white text qualifies
    // once the composite (white minus alpha) is dark enough: candidates are 0.40 + 0.03 i, and
    // the first with contrast >= 4.5 is i = 5 (alpha 0.55; i = 4 reaches only 4.29:1).
    let alpha = frosted_tint_alpha(
        Color::BLACK,
        0.40,
        Color::WHITE,
        Color::WHITE,
        Appearance::Dark,
    );
    assert!((alpha - 0.55).abs() < 1e-9, "{alpha}");

    // A tint that already qualifies at the base alpha keeps it.
    let alpha = frosted_tint_alpha(
        Color::BLACK,
        0.95,
        Color::WHITE,
        Color::WHITE,
        Appearance::Dark,
    );
    assert!((alpha - 0.95).abs() < 1e-9, "{alpha}");

    // No candidate qualifies: a white tint over the white adverse backdrop never helps white
    // text.
    let alpha = frosted_tint_alpha(
        Color::WHITE,
        0.40,
        Color::WHITE,
        Color::WHITE,
        Appearance::Dark,
    );
    assert_eq!(alpha, 1.0);

    // Light appearance: black adverse backdrop, dark text.
    let alpha = frosted_tint_alpha(
        Color::WHITE,
        0.40,
        Color::BLACK,
        Color::BLACK,
        Appearance::Light,
    );
    assert!(alpha > 0.40 && alpha < 1.0, "{alpha}");
    let variant = neutral_variant(Appearance::Dark);
    assert!(variant.frosted_alpha(hex("#101010")) >= 0.40);
}
