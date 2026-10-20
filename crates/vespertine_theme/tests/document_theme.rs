//! THM-002 acceptance tests: document theme base selection, overrides, defaults, metric
//! validation, layout identity and list cache invalidation.

use serde_json::{Value, json};
use vespertine::{
    AppContext as _, Context, Element as _, IntoElement, ListAlignment, ListState, Render, Rgba,
    Styled as _, TestAppContext, Window, div, list, point, px, size,
};
use vespertine_theme::document::{
    Appearance, DocumentMetrics, DocumentTheme, LayoutIdentity, SyntaxCategory,
};

fn bytes(color: Rgba) -> [u8; 3] {
    [color.r, color.g, color.b].map(|c| (c * 255.).round() as u8)
}

/// Asserts the 8-bit channels are within one step of `hex` and the alpha matches.
fn assert_step(label: &str, color: Rgba, hex: u32, alpha: f32) {
    let expected = [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8];
    for (actual, expected) in bytes(color).into_iter().zip(expected) {
        assert!(
            actual.abs_diff(expected) <= 1,
            "{label}: got {:02x?}, expected #{hex:06X}",
            bytes(color)
        );
    }
    assert!(
        (color.a - alpha).abs() < 1e-6,
        "{label}: alpha {} != {alpha}",
        color.a
    );
}

const SYNTAX_KEYS_HEAD: [&str; 18] = [
    "comment",
    "keyword",
    "string",
    "stringSpecial",
    "escape",
    "number",
    "boolean",
    "typeName",
    "typeBuiltin",
    "constructor",
    "function",
    "functionBuiltin",
    "macroName",
    "property",
    "constant",
    "variable",
    "variableSpecial",
    "parameter",
];

const SYNTAX_KEYS_STRUCTURE: [&str; 3] = ["operator", "punctuation", "tag"];

const SYNTAX_KEYS_MARKUP: [&str; 3] = ["attribute", "label", "invalid"];

fn syntax_keys() -> Vec<&'static str> {
    [
        &SYNTAX_KEYS_HEAD[..],
        &SYNTAX_KEYS_STRUCTURE[..],
        &SYNTAX_KEYS_MARKUP[..],
    ]
    .concat()
}

#[test]
fn a1_invalid_values_select_the_dark_base() {
    let dark = DocumentTheme::dark();
    assert_eq!(DocumentTheme::from_value(None), dark);
    assert_eq!(DocumentTheme::from_value(Some(&Value::Null)), dark);
    for value in [
        json!(5),
        json!("light"),
        json!([1, 2]),
        json!(true),
        // Wrong types in recognized fields reject the whole value, including valid siblings.
        json!({"appearance": "light", "accent": 5}),
        json!({"appearance": "light", "accent": null}),
        json!({"appearance": "light", "fontSans": 3}),
        json!({"appearance": 7, "accent": "#ff0000"}),
        json!({"appearance": "light", "metrics": 3}),
        json!({"appearance": "light", "metrics": {"codeTextSize": "13"}}),
        json!({"appearance": "light", "metrics": {"mdHeadingSizes": 19}}),
        json!({"appearance": "light", "metrics": {"mdHeadingSizes": [19, "x"]}}),
        json!({"appearance": "light", "syntax": ["red"]}),
        json!({"appearance": "light", "syntax": {"keyword": 1}}),
        json!({"accent": "#ff0000", "syntax": null}),
    ] {
        assert_eq!(DocumentTheme::from_value(Some(&value)), dark, "{value}");
    }
    // Entries beyond the fourth heading tier are ignored entirely.
    let theme = DocumentTheme::from_value(Some(
        &json!({"metrics": {"mdHeadingSizes": [20, 17, 16, 15, "ignored"]}}),
    ));
    assert_eq!(theme.metrics.md_heading_sizes, [20., 17., 16., 15.]);
}

#[test]
fn a1_invalid_override_leaves_existing_theme_untouched() {
    let mut theme = DocumentTheme::light().with_override(&json!({"accent": "#00ff00"}));
    let before = theme.clone();
    assert!(
        theme
            .apply_override(&json!({"accent": "#ff0000", "metrics": 1}))
            .is_err()
    );
    assert!(theme.apply_override(&json!(null)).is_err());
    assert_eq!(theme, before);
}

#[test]
fn a1_appearance_selects_base_before_overrides() {
    let light = DocumentTheme::from_value(Some(&json!({"appearance": "light"})));
    assert_eq!(light, DocumentTheme::light());
    assert_eq!(light.appearance, Appearance::Light);

    let overridden = DocumentTheme::from_value(Some(
        &json!({"appearance": "light", "accent": "#ff0000", "metrics": {"codeLineHeight": 30}}),
    ));
    let mut expected = DocumentTheme::light();
    expected.accent = vespertine::rgb(0xff0000);
    expected.metrics.code_line_height = 30.;
    assert_eq!(overridden, expected);
    assert_eq!(overridden.bg, DocumentTheme::light().bg);

    for appearance in ["Light", "LIGHT", "dark", "", "sepia"] {
        let theme = DocumentTheme::from_value(Some(&json!({"appearance": appearance})));
        assert_eq!(theme, DocumentTheme::dark(), "{appearance}");
    }
}

#[test]
fn a1_unknown_keys_are_ignored() {
    let theme = DocumentTheme::from_value(Some(&json!({
        "bogus": 1,
        "metrics": {"nope": "x", "codeTextSize": 13},
        "syntax": {"nope": 4, "keyword": "#112233"},
    })));
    let mut expected = DocumentTheme::dark();
    expected.metrics.code_text_size = 13.;
    expected.syntax.keyword = vespertine::rgb(0x112233);
    assert_eq!(theme, expected);
}

#[test]
fn a1_color_overrides_replace_one_token_in_any_notation() {
    let dark = DocumentTheme::dark();
    for text in [
        "oklch(0.62796 0.25768 29.23388)",
        "#FF0000",
        "red",
        "rgb(255 0 0)",
    ] {
        let theme = DocumentTheme::from_value(Some(&json!({"accent": text})));
        assert_step("accent", theme.accent, 0xff0000, 1.);
        assert_eq!(theme.caret, dark.caret, "caret is unchanged ({text})");
        let mut restored = theme.clone();
        restored.accent = dark.accent;
        assert_eq!(restored, dark, "only accent differs ({text})");
    }

    let translucent = DocumentTheme::from_value(Some(&json!({"text": "rgb(0 0 0 / 40%)"})));
    assert_step("text", translucent.text, 0x000000, 0.4);

    // Transparent tokens stay transparent; no compositing happens at theme level.
    let transparent = DocumentTheme::from_value(Some(&json!({"bg": "transparent"})));
    assert_eq!(
        transparent.bg,
        Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.
        }
    );
}

#[test]
fn a1_unparsable_color_leaves_only_that_token() {
    let theme = DocumentTheme::from_value(Some(&json!({
        "accent": "not a color",
        "text": "#010203",
        "syntax": {"keyword": "color(display-p3 1 0 0)", "string": "#040506"},
    })));
    let dark = DocumentTheme::dark();
    assert_eq!(theme.accent, dark.accent);
    assert_eq!(theme.syntax.keyword, dark.syntax.keyword);
    assert_step("text", theme.text, 0x010203, 1.);
    assert_step("string", theme.syntax.string, 0x040506, 1.);
}

#[test]
fn a1_fonts_replace_families_unchanged() {
    let theme = DocumentTheme::from_value(Some(
        &json!({"fontSans": "Inter  Tight", "fontMono": "JetBrains Mono"}),
    ));
    assert_eq!(theme.font_sans.as_ref(), "Inter  Tight");
    assert_eq!(theme.font_mono.as_ref(), "JetBrains Mono");
    let dark = DocumentTheme::dark();
    assert_eq!(dark.font_sans.as_ref(), "DejaVu Sans");
    assert_eq!(dark.font_mono.as_ref(), "DejaVu Sans Mono");
}

#[test]
fn a2_color_token_defaults() {
    let dark = DocumentTheme::dark();
    let light = DocumentTheme::light();
    // (key, dark hex, dark alpha, light hex, light alpha)
    let table: [(&str, u32, f32, u32, f32); 13] = [
        ("bg", 0x060606, 1., 0xFFFFFF, 1.),
        ("border", 0xFFFFFF, 0.08, 0x000000, 0.10),
        ("text", 0xE5E5E5, 1., 0x222222, 1.),
        ("textMuted", 0xA1A1A1, 1., 0x525252, 1.),
        ("textFaint", 0x737373, 1., 0x6D6D6D, 1.),
        ("textDim", 0x989898, 1., 0x636363, 1.),
        ("accent", 0x7C86FF, 1., 0x4F39F6, 1.),
        ("caret", 0x7C86FF, 1., 0x4F39F6, 1.),
        ("codeText", 0xC4B4FF, 1., 0x7008E7, 1.),
        ("codeWash", 0xA684FF, 0.12, 0x7F22FE, 0.10),
        ("diffAdd", 0x00D492, 1., 0x009966, 1.),
        ("diffDel", 0xFF6467, 1., 0xE7000B, 1.),
        ("diffHunkBg", 0x7592BD, 0.05, 0x3A5378, 0.07),
    ];
    for (key, dark_hex, dark_alpha, light_hex, light_alpha) in table {
        assert_step(
            &format!("dark {key}"),
            dark.color_token(key).unwrap(),
            dark_hex,
            dark_alpha,
        );
        assert_step(
            &format!("light {key}"),
            light.color_token(key).unwrap(),
            light_hex,
            light_alpha,
        );
    }
    assert_eq!(dark.color_token("nope"), None);
    // Neutral(0.922) is an OKLCH lightness, not an HSL lightness (which would be #EBEBEB).
    assert_ne!(bytes(dark.text), [0xEB, 0xEB, 0xEB]);
    assert_eq!(bytes(dark.text), [0xE5, 0xE5, 0xE5]);
}

#[test]
fn a2_syntax_category_defaults() {
    for (name, theme, text_hex, comment_hex, danger_hex) in [
        ("dark", DocumentTheme::dark(), 0xE5E5E5, 0x808080, 0xE9797C),
        (
            "light",
            DocumentTheme::light(),
            0x222222,
            0x5D5D5D,
            0xA61B20,
        ),
    ] {
        let (indigo, pink, emerald, amber) = (0x8F96ED, 0xE679B4, 0x1EB787, 0xDBA924);
        let expected: [(&str, u32); 24] = [
            ("comment", comment_hex),
            ("keyword", indigo),
            ("string", emerald),
            ("stringSpecial", pink),
            ("escape", pink),
            ("number", amber),
            ("boolean", amber),
            ("typeName", amber),
            ("typeBuiltin", emerald),
            ("constructor", amber),
            ("function", indigo),
            ("functionBuiltin", pink),
            ("macroName", pink),
            ("property", amber),
            ("constant", emerald),
            ("variable", text_hex),
            ("variableSpecial", pink),
            ("parameter", text_hex),
            ("operator", text_hex),
            ("punctuation", text_hex),
            ("tag", pink),
            ("attribute", amber),
            ("label", amber),
            ("invalid", danger_hex),
        ];
        assert_eq!(syntax_keys(), expected.map(|(key, _)| key));
        for (key, hex) in expected {
            let category = SyntaxCategory::from_key(key).unwrap();
            assert_eq!(category.key(), key);
            assert_step(
                &format!("{name} syntax {key}"),
                theme.syntax.get(category),
                hex,
                1.,
            );
        }
    }
    assert_eq!(syntax_keys(), SyntaxCategory::ALL.map(SyntaxCategory::key));
    assert!(SyntaxCategory::from_key("nope").is_none());
    // Softened indigo is the 0.72-saturation version of the accent base.
    assert_step(
        "softened indigo",
        DocumentTheme::dark().syntax.keyword,
        0x8F96ED,
        1.,
    );
    // The syntax text value equals the base text color exactly.
    assert_eq!(
        DocumentTheme::dark().syntax.variable,
        DocumentTheme::dark().text
    );
    assert_eq!(
        DocumentTheme::light().syntax.punctuation,
        DocumentTheme::light().text
    );
}

const METRIC_DEFAULTS: [(&str, f32); 27] = [
    ("codeTextSize", 12.5),
    ("codeLineHeight", 18.),
    ("codeGutterDigitWidth", 7.),
    ("codeGutterPaddingRight", 12.),
    ("codeGutterMinWidth", 28.),
    ("diffTextSize", 12.),
    ("diffLineHeight", 21.),
    ("diffFileHeaderHeight", 36.),
    ("diffHunkHeaderHeight", 28.),
    ("diffNoticeHeight", 24.),
    ("diffBodyBottomPad", 8.),
    ("mdCodePaddingX", 12.),
    ("mdCodeRadius", 10.),
    ("mdCodeHeaderTextSize", 11.),
    ("diffGutterWidth", 36.),
    ("diffMarkerWidth", 28.),
    ("diffAccentBarWidth", 3.),
    ("diffRowPaddingX", 16.),
    ("mdTextSize", 14.),
    ("mdLineHeight", 22.),
    ("mdBlockGap", 12.),
    ("mdTableCellPadding", 12.),
    ("mdTableMinColumnWidth", 96.),
    ("mdTableMinColumnContent", 48.),
    ("mdInlineCodeRadius", 4.5),
    ("mdCodePaddingY", 10.),
    ("mdCodeHeaderPaddingY", 5.),
];

#[test]
fn a2_metric_defaults() {
    for theme in [DocumentTheme::dark(), DocumentTheme::light()] {
        for (key, default) in METRIC_DEFAULTS {
            assert_eq!(theme.metrics.scalar(key), Some(default), "{key}");
        }
        assert_eq!(theme.metrics.md_heading_sizes, [19., 16., 15., 14.]);
        assert_eq!(theme.metrics.md_heading_line_heights, [27., 24., 22., 22.]);
    }
    let mut keys = DocumentMetrics::SCALAR_KEYS.to_vec();
    keys.sort_unstable();
    let mut expected = METRIC_DEFAULTS.map(|(key, _)| key).to_vec();
    expected.sort_unstable();
    assert_eq!(
        keys, expected,
        "the key table is exactly the specified metrics"
    );
    assert_eq!(DocumentMetrics::default(), DocumentTheme::dark().metrics);
}

fn metric(key: &str, value: Value) -> DocumentTheme {
    DocumentTheme::from_value(Some(&json!({"metrics": {key: value}})))
}

#[test]
fn a3_scalar_metric_validation() {
    let dark = DocumentTheme::dark();
    for rejected in [
        json!(-1),
        json!(-0.001),
        json!(1e300),
        json!(-1e300),
        json!(3.5e38),
    ] {
        assert_eq!(metric("codeTextSize", rejected.clone()), dark, "{rejected}");
    }
    assert_eq!(metric("codeTextSize", json!(0)).metrics.code_text_size, 0.);
    assert_eq!(
        metric("codeTextSize", json!(0.0)).metrics.code_text_size,
        0.
    );
    assert_eq!(
        metric("codeTextSize", json!(-0.0)).metrics.code_text_size,
        0.
    );
    assert_eq!(
        metric("codeTextSize", json!(13.25)).metrics.code_text_size,
        13.25
    );
    assert_eq!(
        metric("codeTextSize", json!(3.4e38)).metrics.code_text_size,
        3.4e38_f32
    );

    // Non-finite numbers cannot appear in JSON; the validation entry point rejects them.
    for rejected in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.,
        1e300,
        f64::MAX,
    ] {
        assert_eq!(DocumentMetrics::validate(rejected), None, "{rejected}");
    }
    assert_eq!(DocumentMetrics::validate(0.), Some(0.));
    assert_eq!(DocumentMetrics::validate(18.5), Some(18.5));

    // A rejected key does not affect others in the same group.
    let theme = DocumentTheme::from_value(Some(
        &json!({"metrics": {"codeTextSize": -1, "codeLineHeight": 30}}),
    ));
    assert_eq!(theme.metrics.code_text_size, 12.5);
    assert_eq!(theme.metrics.code_line_height, 30.);
}

#[test]
fn a3_heading_arrays() {
    let sizes = |value: Value| {
        DocumentTheme::from_value(Some(&json!({"metrics": {"mdHeadingSizes": value}})))
            .metrics
            .md_heading_sizes
    };
    assert_eq!(sizes(json!([20])), [20., 16., 15., 14.]);
    assert_eq!(sizes(json!([20, 17, 16, 15, 99])), [20., 17., 16., 15.]);
    assert_eq!(sizes(json!([20, -1])), [20., 16., 15., 14.]);
    assert_eq!(sizes(json!([])), [19., 16., 15., 14.]);
    assert_eq!(sizes(json!([-5, 1e300, 0, 8])), [19., 16., 0., 8.]);

    let theme = DocumentTheme::from_value(Some(
        &json!({"metrics": {"mdHeadingLineHeights": [30, 26, 24, 23, 1000]}}),
    ));
    assert_eq!(theme.metrics.md_heading_line_heights, [30., 26., 24., 23.]);
    assert_eq!(theme.metrics.md_heading_sizes, [19., 16., 15., 14.]);
}

#[test]
fn a3_heading_level_to_tier() {
    let theme = DocumentTheme::from_value(Some(&json!({"metrics": {
        "mdHeadingSizes": [20, 17, 16, 15],
        "mdHeadingLineHeights": [30, 26, 24, 23],
    }})));
    let tiers = [(20., 30.), (17., 26.), (16., 24.), (15., 23.)];
    for (level, tier) in [(1, 0), (2, 1), (3, 2), (4, 3), (5, 3), (6, 3)] {
        assert_eq!(theme.heading_tier(level), tiers[tier], "level {level}");
    }
    assert_eq!(theme.heading_tier(0), tiers[0]);
    assert_eq!(theme.heading_tier(-3), tiers[0]);
    assert_eq!(theme.heading_tier(7), tiers[3]);
    assert_eq!(theme.heading_tier(i32::MAX), tiers[3]);
    assert_eq!(DocumentTheme::dark().heading_tier(2), (16., 24.));
}

/// Every metric, as a single-key override that differs from the default.
fn single_metric_overrides() -> Vec<(String, Value)> {
    let mut overrides = Vec::new();
    for (key, default) in METRIC_DEFAULTS {
        overrides.push((key.to_string(), json!({ key: default + 1. })));
    }
    for tier in 0..4 {
        for key in ["mdHeadingSizes", "mdHeadingLineHeights"] {
            let mut array = [19., 16., 15., 14.];
            array[tier] += 3.;
            overrides.push((format!("{key}[{tier}]"), json!({ key: array.to_vec() })));
        }
    }
    overrides
}

fn themed(metrics: &Value) -> DocumentTheme {
    DocumentTheme::from_value(Some(&json!({ "metrics": metrics })))
}

#[test]
fn a4_every_single_metric_changes_the_layout_hash() {
    let base = DocumentTheme::dark();
    let overrides = single_metric_overrides();
    assert_eq!(overrides.len(), 27 + 8);
    let mut seen = std::collections::HashSet::from([base.layout_hash()]);
    for (label, metrics) in &overrides {
        let theme = themed(metrics);
        assert_ne!(theme.layout_hash(), base.layout_hash(), "{label}");
        assert!(
            seen.insert(theme.layout_hash()),
            "{label} collides with another override"
        );
    }
    // The hash is a pure function of the metrics; colors do not feed it.
    let recolored = DocumentTheme::from_value(Some(&json!({"accent": "#123456"})));
    assert_eq!(recolored.layout_hash(), base.layout_hash());
    assert_eq!(
        DocumentTheme::dark().layout_hash(),
        DocumentTheme::light().layout_hash()
    );
}

#[test]
fn a4_diff_row_hash_depends_only_on_the_five_row_height_metrics() {
    let base = DocumentTheme::dark();
    let row_keys = [
        "diffLineHeight",
        "diffFileHeaderHeight",
        "diffHunkHeaderHeight",
        "diffNoticeHeight",
        "diffBodyBottomPad",
    ];
    for (label, metrics) in single_metric_overrides() {
        let theme = themed(&metrics);
        if row_keys.contains(&label.as_str()) {
            assert_ne!(
                theme.diff_row_height_hash(),
                base.diff_row_height_hash(),
                "{label}"
            );
        } else {
            assert_eq!(
                theme.diff_row_height_hash(),
                base.diff_row_height_hash(),
                "{label}"
            );
        }
    }
    let markdown = themed(&json!({"mdTextSize": 17, "mdHeadingSizes": [30, 20, 18, 16]}));
    assert_eq!(markdown.diff_row_height_hash(), base.diff_row_height_hash());
    assert_ne!(markdown.layout_hash(), base.layout_hash());
}

#[test]
fn a4_layout_identity_tracks_changes_per_key() {
    let base = DocumentTheme::dark();
    let mut tracker = LayoutIdentity::new();
    assert!(!tracker.layout_changed(&base), "first sight records only");
    assert!(!tracker.layout_changed(&base.clone()));
    let markdown = themed(&json!({"mdTextSize": 17}));
    assert!(tracker.layout_changed(&markdown));
    assert!(!tracker.layout_changed(&markdown));

    let mut diff_tracker = LayoutIdentity::new();
    assert!(!diff_tracker.diff_rows_changed(&base));
    assert!(
        !diff_tracker.diff_rows_changed(&markdown),
        "a Markdown metric keeps diff rows"
    );
    assert!(diff_tracker.diff_rows_changed(&themed(&json!({"diffLineHeight": 30}))));
}

/// A minimal code view: one row per line with a height taken from the theme metrics.
struct RowsView {
    state: ListState,
    theme: DocumentTheme,
    identity: LayoutIdentity,
}

impl Render for RowsView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.identity
            .remeasure_on_layout_change(&self.theme, &self.state);
        let row_height = px(self.theme.metrics.code_line_height);
        list(self.state.clone(), move |_, _, _| {
            div().h(row_height).w_full().into_any()
        })
        .w_full()
        .h_full()
    }
}

#[vespertine::test]
fn a4_layout_change_invalidates_a_variable_height_list_row_model(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(100, ListAlignment::Top, px(0.)).measure_all();
    let view = cx.update(|_, cx| {
        cx.new(|_| RowsView {
            state: state.clone(),
            theme: DocumentTheme::dark(),
            identity: LayoutIdentity::new(),
        })
    });
    let draw = |cx: &mut vespertine::VisualTestContext| {
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(90.)), |_, _| {
            view.clone().into_any_element()
        });
    };

    draw(cx);
    // 100 rows of 18px in a 90px viewport.
    let max_offset = |state: &ListState| state.max_offset_for_scrollbar().y;
    assert_eq!(max_offset(&state), px(100. * 18. - 90.));

    // Changing a metric re-lays out the visible rows but only the identity-driven remeasure
    // refreshes the cached heights of off-screen rows.
    let taller = themed(&json!({"codeLineHeight": 30}));
    view.update(cx, |view, cx| {
        view.theme = taller.clone();
        cx.notify();
    });
    draw(cx);
    draw(cx);
    assert_eq!(
        max_offset(&state),
        px(100. * 30. - 90.),
        "row model rebuilt from new metric"
    );

    // A metric the rows do not use still changes the identity and remeasures (harmless).
    let before = max_offset(&state);
    view.update(cx, |view, cx| {
        view.theme = view
            .theme
            .clone()
            .with_override(&json!({"metrics": {"mdTextSize": 20}}));
        cx.notify();
    });
    draw(cx);
    assert_eq!(max_offset(&state), before);
}

#[vespertine::test]
fn a4_without_the_identity_sync_offscreen_rows_stay_stale(cx: &mut TestAppContext) {
    // Control for the test above: the cached row model really is stale unless invalidated.
    let cx = cx.add_empty_window();
    let state = ListState::new(100, ListAlignment::Top, px(0.)).measure_all();
    let view = cx.update(|_, cx| {
        cx.new(|_| RowsView {
            state: state.clone(),
            theme: DocumentTheme::dark(),
            identity: LayoutIdentity::new(),
        })
    });
    let draw = |cx: &mut vespertine::VisualTestContext| {
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(90.)), |_, _| {
            view.clone().into_any_element()
        });
    };
    draw(cx);
    view.update(cx, |view, cx| {
        // Bypass the tracker by recording the new identity before the render sees it.
        let taller = themed(&json!({"codeLineHeight": 30}));
        view.identity.layout_changed(&taller);
        view.theme = taller;
        cx.notify();
    });
    draw(cx);
    draw(cx);
    assert_ne!(state.max_offset_for_scrollbar().y, px(100. * 30. - 90.));
}

#[test]
fn a6_text_override_does_not_recolor_syntax_defaults() {
    let dark = DocumentTheme::dark();
    let theme = DocumentTheme::from_value(Some(&json!({"text": "#ff0000"})));
    assert_step("text", theme.text, 0xff0000, 1.);
    for category in [
        SyntaxCategory::Variable,
        SyntaxCategory::Parameter,
        SyntaxCategory::Operator,
        SyntaxCategory::Punctuation,
    ] {
        assert_eq!(
            theme.syntax.get(category),
            dark.syntax.get(category),
            "{}",
            category.key()
        );
        assert_eq!(theme.syntax.get(category), dark.text);
    }
    // A caller that wants those categories recolored supplies them under `syntax` as well.
    let explicit = DocumentTheme::from_value(Some(
        &json!({"text": "#ff0000", "syntax": {"variable": "#ff0000"}}),
    ));
    assert_step("syntax.variable", explicit.syntax.variable, 0xff0000, 1.);
    assert_eq!(explicit.syntax.parameter, dark.text);

    let light = DocumentTheme::from_value(Some(&json!({"appearance": "light", "text": "#ff0000"})));
    assert_eq!(light.syntax.operator, DocumentTheme::light().text);
}

#[test]
fn fid_thm_002_swatch_colors() {
    let dark = DocumentTheme::dark();
    let light = DocumentTheme::light();
    let overridden = DocumentTheme::from_value(Some(&json!({"accent": "#FF0000"})));
    assert_step("dark bg", dark.bg, 0x060606, 1.);
    assert_step("light bg", light.bg, 0xFFFFFF, 1.);
    assert_step("dark accent", dark.accent, 0x7C86FF, 1.);
    assert_step("overridden accent", overridden.accent, 0xFF0000, 1.);
    assert_step("dark text", dark.text, 0xE5E5E5, 1.);
    assert_step("light accent", light.accent, 0x4F39F6, 1.);
}
