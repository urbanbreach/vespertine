//! THM-004 A2 and A3: import limits and failures, validation and the hardening rules.

mod support;

use serde_json::{Value, json};
use std::path::Path;
use support::*;
use vespertine_theme::{
    builtin::{neutral_family, neutral_variant},
    color::worst_contrast,
    import::{MAX_INCLUDE_DEPTH, MAX_PACKAGE_VARIANTS, MAX_SOURCE_BYTES, SourceKind},
    theme::{
        Appearance, IssueCategory, IssueSeverity, SyntaxCategory, ThemeFamily, ThemeRegistry,
        has_blocking_issue,
    },
};

fn write(dir: &Path, name: &str, value: &Value) -> std::path::PathBuf {
    let path = dir.join(name);
    write_json(&path, value);
    path
}

// ---------------------------------------------------------------------------------------------
// A2: failures and limits
// ---------------------------------------------------------------------------------------------

#[test]
fn malformed_json_is_rejected_with_a_readable_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.json");
    std::fs::write(&path, "{ \"colors\": { \"editor.background\": ").unwrap();
    let error = compile(&path, "broken").unwrap_err();
    assert!(error.message.contains("not valid JSON"), "{error}");
    assert_eq!(error.failures.len(), 1);
    assert_eq!(error.failures[0].path, path.canonicalize().unwrap());
}

#[test]
fn comments_and_trailing_commas_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jsonc.json");
    std::fs::write(
        &path,
        r##"{
            // a line comment
            "type": "dark",
            /* a block comment */
            "colors": { "editor.background": "#101010", "editor.foreground": "#eeeeee", },
        }"##,
    )
    .unwrap();
    let compilation = compile(&path, "jsonc").unwrap();
    assert_eq!(compilation.source_kind, SourceKind::File);
    assert_eq!(
        compilation.family.variants[0].colors.background,
        hex("#101010")
    );
}

#[test]
fn a_source_over_four_mebibytes_is_rejected_and_exactly_four_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let body = theme_json(good_dark_colors()).to_string();
    let pad = |length: u64| {
        let mut text = body.clone();
        text.extend(std::iter::repeat(' ').take((length as usize) - body.len()));
        text
    };
    let exact = dir.path().join("exact.json");
    std::fs::write(&exact, pad(MAX_SOURCE_BYTES)).unwrap();
    assert!(compile(&exact, "exact").is_ok());

    let over = dir.path().join("over.json");
    std::fs::write(&over, pad(MAX_SOURCE_BYTES + 1)).unwrap();
    let error = compile(&over, "over").unwrap_err();
    assert!(error.message.contains("limit"), "{error}");
}

#[test]
fn an_include_cycle_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.json", &json!({ "include": "./b.json" }));
    write(dir.path(), "b.json", &json!({ "include": "./c.json" }));
    write(dir.path(), "c.json", &json!({ "include": "./a.json" }));
    let error = compile(&dir.path().join("a.json"), "cycle").unwrap_err();
    assert!(error.message.contains("include cycle"), "{error}");
    // A file including itself is the shortest cycle.
    write(
        dir.path(),
        "self.json",
        &json!({ "include": "./self.json" }),
    );
    let error = compile(&dir.path().join("self.json"), "self").unwrap_err();
    assert!(error.message.contains("include cycle"), "{error}");
}

fn include_chain(dir: &Path, hops: usize) -> std::path::PathBuf {
    for index in 0..=hops {
        let mut value = if index == hops {
            theme_json(good_dark_colors())
        } else {
            json!({ "include": format!("./chain{}.json", index + 1) })
        };
        if index == 0 && hops > 0 {
            value["name"] = json!("Chain");
        }
        write(dir, &format!("chain{index}.json"), &value);
    }
    dir.join("chain0.json")
}

#[test]
fn include_depth_is_limited_to_thirty_two() {
    let dir = tempfile::tempdir().unwrap();
    let ok = tempfile::tempdir_in(dir.path()).unwrap();
    let path = include_chain(ok.path(), MAX_INCLUDE_DEPTH);
    let compilation = compile(&path, "deep").expect("32 includes are accepted");
    // Included values arrive through the chain.
    assert_eq!(
        compilation.family.variants[0].colors.background,
        hex("#1e1e1e")
    );

    let too_deep = tempfile::tempdir_in(dir.path()).unwrap();
    let path = include_chain(too_deep.path(), MAX_INCLUDE_DEPTH + 1);
    let error = compile(&path, "deeper").unwrap_err();
    assert!(error.message.contains("deeper than 32"), "{error}");
}

#[test]
fn paths_escaping_the_root_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "outside.json", &theme_json(good_dark_colors()));
    let root = dir.path().join("root");
    let theme = write(
        &root,
        "theme.json",
        &json!({ "include": "../outside.json" }),
    );
    let error = compile(&theme, "escape").unwrap_err();
    assert!(error.message.contains("outside the theme root"), "{error}");

    // An external token file outside the root is refused too.
    write(dir.path(), "tokens.json", &json!([]));
    let theme = write(
        &root,
        "tokens-theme.json",
        &json!({ "colors": {}, "tokenColors": "../tokens.json" }),
    );
    assert!(
        compile(&theme, "tokens")
            .unwrap_err()
            .message
            .contains("outside the theme root")
    );
}

#[cfg(unix)]
#[test]
fn symlinks_are_allowed_only_when_their_targets_stay_inside_the_root() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "outside.json", &theme_json(good_dark_colors()));
    let root = dir.path().join("root");
    write(&root, "real.json", &theme_json(good_dark_colors()));
    std::os::unix::fs::symlink(
        dir.path().join("outside.json"),
        root.join("escape-link.json"),
    )
    .unwrap();
    std::os::unix::fs::symlink(root.join("real.json"), root.join("inside-link.json")).unwrap();

    write(
        &root,
        "via-escape.json",
        &json!({ "include": "./escape-link.json" }),
    );
    let error = compile(&root.join("via-escape.json"), "escape").unwrap_err();
    assert!(error.message.contains("outside the theme root"), "{error}");

    write(
        &root,
        "via-inside.json",
        &json!({ "include": "./inside-link.json" }),
    );
    assert!(compile(&root.join("via-inside.json"), "inside").is_ok());
}

fn manifest_entry(label: &str, path: &str, ui_theme: &str) -> Value {
    json!({ "label": label, "uiTheme": ui_theme, "path": path })
}

#[test]
fn a_package_with_more_than_128_variants_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "themes/t.json", &theme_json(good_dark_colors()));
    let entries = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|index| manifest_entry(&format!("Theme {index}"), "./themes/t.json", "vs-dark"))
            .collect()
    };
    write(
        dir.path(),
        "package.json",
        &package_manifest(entries(MAX_PACKAGE_VARIANTS)),
    );
    let compilation = compile(dir.path(), "pkg").expect("128 variants are accepted");
    assert_eq!(compilation.source_kind, SourceKind::Package);
    assert_eq!(compilation.family.variants.len(), MAX_PACKAGE_VARIANTS);

    write(
        dir.path(),
        "package.json",
        &package_manifest(entries(MAX_PACKAGE_VARIANTS + 1)),
    );
    let error = compile(dir.path(), "pkg").unwrap_err();
    assert!(error.message.contains("129"), "{error}");
}

#[test]
fn one_bad_variant_does_not_hide_the_valid_ones() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "themes/good.json",
        &theme_json(good_dark_colors()),
    );
    std::fs::write(dir.path().join("themes/bad.json"), "{ nope").unwrap();
    write(
        dir.path(),
        "package.json",
        &package_manifest(vec![
            manifest_entry("Good", "./themes/good.json", "vs-dark"),
            manifest_entry("Bad", "./themes/bad.json", "vs-dark"),
            manifest_entry("Missing", "./themes/missing.json", "vs-dark"),
        ]),
    );
    let compilation = compile(dir.path(), "pkg").unwrap();
    assert_eq!(compilation.family.variants.len(), 1);
    assert_eq!(compilation.family.variants[0].id, "pkg-good");
    assert_eq!(compilation.reports.len(), 1);
    assert_eq!(compilation.failures.len(), 2);
    let bad = &compilation.failures[0];
    assert_eq!((bad.id.as_str(), bad.name.as_str()), ("pkg-bad", "Bad"));
    assert!(bad.message.contains("not valid JSON"));
    let missing = &compilation.failures[1];
    assert_eq!(missing.id, "pkg-missing");
    assert!(missing.path.ends_with("themes/missing.json"));
    assert!(
        missing.message.contains("cannot read"),
        "{}",
        missing.message
    );
}

#[test]
fn a_package_whose_variants_all_fail_is_an_error_with_the_failures() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        &package_manifest(vec![manifest_entry("Missing", "./nope.json", "vs")]),
    );
    let error = compile(dir.path(), "pkg").unwrap_err();
    assert_eq!(error.failures.len(), 1);
}

#[test]
fn appearance_detection_follows_the_documented_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut light_bg = good_dark_colors();
    light_bg["editor.background"] = json!("#ffffff");
    light_bg["editor.foreground"] = json!("#222222");
    // No `type`: the background decides.
    let mut theme = theme_json(light_bg.clone());
    theme.as_object_mut().unwrap().remove("type");
    write(dir.path(), "themes/light-bg.json", &theme);
    // `type` beats the background.
    let mut typed = theme_json(light_bg.clone());
    typed["type"] = json!("DARK");
    write(dir.path(), "themes/typed.json", &typed);
    // `uiTheme` beats both.
    write(dir.path(), "themes/ui.json", &typed);
    // No signal at all: dark by default.
    write(dir.path(), "themes/bare.json", &json!({ "colors": {} }));
    write(
        dir.path(),
        "package.json",
        &package_manifest(vec![
            manifest_entry("Light BG", "./themes/light-bg.json", "unknown-ui-theme"),
            manifest_entry("Typed", "./themes/typed.json", "unknown-ui-theme"),
            manifest_entry("Ui", "./themes/ui.json", "HC-Light"),
            manifest_entry("Bare", "./themes/bare.json", "unknown-ui-theme"),
            manifest_entry("Ui Dark", "./themes/light-bg.json", "vs-dark"),
        ]),
    );
    let compilation = compile(dir.path(), "pkg").unwrap();
    let appearances: Vec<Appearance> = compilation
        .family
        .variants
        .iter()
        .map(|variant| variant.appearance)
        .collect();
    assert_eq!(
        appearances,
        [
            Appearance::Light,
            Appearance::Dark,
            Appearance::Light,
            Appearance::Dark,
            Appearance::Dark
        ]
    );
}

#[test]
fn local_imports_record_provenance_and_a_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "theme.json", &theme_json(good_dark_colors()));
    let compilation = compile(&path, "local").unwrap();
    let source = &compilation.family.variants[0].source;
    assert_eq!(source.format, "vscode");
    assert_eq!(source.revision, "local");
    assert_eq!(source.license, "User supplied");
    assert_eq!(source.url, path.display().to_string());
    assert_eq!(source.asset_hash.len(), 64);
    assert!(
        compilation.family.validate().is_empty(),
        "{:?}",
        compilation.family.validate()
    );
}

#[test]
fn mappings_syntax_and_accent_candidates_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "theme.json", &theme_json(good_dark_colors()));
    let compilation = compile(&path, "report").unwrap();
    let variant = &compilation.family.variants[0];
    let report = &compilation.reports[0];
    assert_eq!(report.variant_id, variant.id);
    let mapping = |role: &str| {
        report
            .mappings
            .iter()
            .find(|mapping| mapping.role == role)
            .unwrap()
    };
    assert_eq!(mapping("background").source_key, "editor.background");
    assert_eq!(mapping("background").value, "#1e1e1e");
    assert_eq!(mapping("text").source_key, "editor.foreground");
    assert_eq!(mapping("syntax.keyword").source_key, "tokenColors[keyword]");
    assert_eq!(
        variant.syntax.get(SyntaxCategory::Keyword),
        Some(hex("#569cd6"))
    );
    assert_eq!(
        variant.syntax.get(SyntaxCategory::Number),
        Some(hex("#b5cea8"))
    );
    assert_eq!(
        variant.syntax.get(SyntaxCategory::Function),
        Some(hex("#dcdcaa"))
    );
    // Categories the source does not mention stay absent and fall back to the neutral default.
    assert_eq!(variant.syntax.get(SyntaxCategory::Tag), None);
    assert_eq!(
        variant.syntax_color(SyntaxCategory::Tag),
        neutral_variant(Appearance::Dark).syntax_color(SyntaxCategory::Tag)
    );
    let accents: Vec<(&str, &str)> = report
        .accent_candidates
        .iter()
        .map(|candidate| (candidate.source_key.as_str(), candidate.value.as_str()))
        .collect();
    assert_eq!(
        accents,
        [
            ("focusBorder", "#4aa3ff"),
            ("button.background", "#0e639c"),
            ("editorCursor.foreground", "#aeafad")
        ]
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
}

#[test]
fn external_token_files_and_semantic_colors_are_read() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "tokens.json",
        &json!([{ "scope": "keyword.control", "settings": { "foreground": "#c586c0" } }]),
    );
    let mut theme = theme_json(good_dark_colors());
    theme["tokenColors"] = json!("./tokens.json");
    theme["semanticTokenColors"] = json!({
        "function": "#ffd700",
        "variable.readonly": { "foreground": "#4fc1ff" }
    });
    let path = write(dir.path(), "theme.json", &theme);
    let compilation = compile(&path, "ext").unwrap();
    let syntax = &compilation.family.variants[0].syntax;
    assert_eq!(syntax.get(SyntaxCategory::Keyword), Some(hex("#c586c0")));
    assert_eq!(syntax.get(SyntaxCategory::Function), Some(hex("#ffd700")));
    assert_eq!(syntax.get(SyntaxCategory::Variable), Some(hex("#4fc1ff")));

    // Property lists are not JSON: a readable error, not a silent skip.
    std::fs::write(
        dir.path().join("tokens.tmTheme"),
        "<?xml version=\"1.0\"?><plist/>",
    )
    .unwrap();
    theme["tokenColors"] = json!("./tokens.tmTheme");
    let path = write(dir.path(), "plist.json", &theme);
    let error = compile(&path, "plist").unwrap_err();
    assert!(error.message.contains("property lists"), "{error}");
}

// ---------------------------------------------------------------------------------------------
// A2: validation
// ---------------------------------------------------------------------------------------------

fn family_of(variants: Vec<vespertine_theme::theme::ThemeVariant>) -> ThemeFamily {
    ThemeFamily {
        id: "test".into(),
        name: "Test".into(),
        variants,
    }
}

fn custom(id: &str) -> vespertine_theme::theme::ThemeVariant {
    let mut variant = neutral_variant(Appearance::Dark);
    variant.id = id.to_string();
    variant.source.url = "https://example.test".into();
    variant
}

#[test]
fn builtin_family_validates_cleanly() {
    assert!(neutral_family().validate().is_empty());
    assert!(ThemeRegistry::builtin().validate().is_empty());
}

#[test]
fn empty_and_duplicate_identifiers_are_blocking() {
    let issues = family_of(vec![custom(""), custom("a"), custom("a")]).validate();
    let structural: Vec<_> = issues
        .iter()
        .filter(|issue| issue.category == IssueCategory::Structural)
        .collect();
    assert_eq!(structural.len(), 2);
    assert!(structural.iter().all(|issue| issue.is_blocking()));
    assert!(structural[0].message.contains("empty"));
    assert!(structural[1].message.contains("duplicated"));
    assert_eq!(structural[1].variant_id, "a");

    // Duplicates across families of a registry are caught by the registry validation.
    let registry = ThemeRegistry::with_custom([family_of(vec![custom("default-dark")])]);
    assert!(has_blocking_issue(&registry.validate()));
}

#[test]
fn incomplete_provenance_is_blocking() {
    for field in 0..4 {
        let mut variant = custom("x");
        match field {
            0 => variant.source.url.clear(),
            1 => variant.source.revision.clear(),
            2 => variant.source.license.clear(),
            _ => variant.source.asset_hash.clear(),
        }
        let issues = family_of(vec![variant]).validate();
        assert!(has_blocking_issue(&issues), "field {field}");
    }
    assert!(!has_blocking_issue(
        &family_of(vec![custom("x")]).validate()
    ));
}

#[test]
fn low_contrast_is_a_non_blocking_error_with_the_formatted_message() {
    let mut variant = custom("low");
    variant.colors.text = variant.colors.background; // 1.00:1
    let issues = family_of(vec![variant.clone()]).validate();
    assert!(!has_blocking_issue(&issues));
    let issue = issues
        .iter()
        .find(|issue| issue.message.starts_with("text contrast"))
        .unwrap();
    assert_eq!(issue.category, IssueCategory::Contrast);
    assert_eq!(issue.severity, IssueSeverity::Error);
    assert!(!issue.is_blocking());
    assert_eq!(issue.message, "text contrast is 1.00:1; expected 4.5:1");

    variant.colors.text_muted = variant.colors.background;
    variant.accent.primary = variant.colors.background;
    variant.accent.on = variant.accent.strong;
    variant.terminal.foreground = variant.terminal.background;
    let messages: Vec<String> = family_of(vec![variant])
        .validate()
        .into_iter()
        .map(|issue| issue.message)
        .collect();
    for expected in [
        "textMuted contrast is 1.00:1; expected 4.5:1",
        "accent primary contrast is 1.00:1; expected 3.0:1",
        "on-accent contrast is 1.00:1; expected 4.5:1",
        "terminal foreground contrast is 1.00:1; expected 4.5:1",
    ] {
        assert!(
            messages.iter().any(|message| message == expected),
            "{expected}: {messages:?}"
        );
    }
}

#[test]
fn ansi_warnings_cover_chromatic_slots_only() {
    let mut variant = custom("ansi");
    let background = variant.terminal.background;
    variant.terminal.ansi = [background; 16];
    let issues = family_of(vec![variant]).validate();
    let warned: Vec<usize> = issues
        .iter()
        .filter(|issue| issue.severity == IssueSeverity::Warning)
        .map(|issue| {
            issue
                .message
                .strip_prefix("terminal ansi ")
                .and_then(|rest| rest.split(' ').next())
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    let expected: Vec<usize> = (0..16).filter(|index| *index != 0 && *index != 8).collect();
    assert_eq!(warned, expected);
    assert!(issues.iter().all(|issue| !issue.is_blocking()));
}

// ---------------------------------------------------------------------------------------------
// A3: hardening rules
// ---------------------------------------------------------------------------------------------

fn compile_colors(
    colors: Value,
) -> (
    vespertine_theme::theme::ThemeVariant,
    vespertine_theme::import::VariantReport,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "theme.json", &theme_json(colors));
    let mut compilation = compile(&path, "h").unwrap();
    (
        compilation.family.variants.remove(0),
        compilation.reports.remove(0),
    )
}

#[test]
fn nothing_changed_records_no_adjustments() {
    let (variant, report) = compile_colors(good_dark_colors());
    assert!(report.adjustments.is_empty(), "{:?}", report.adjustments);
    assert_eq!(variant.colors.text, hex("#d4d4d4"));
    assert_eq!(variant.accent.primary, hex("#4aa3ff"));
}

#[test]
fn translucent_surfaces_are_flattened_and_recorded() {
    let mut colors = good_dark_colors();
    colors["sideBar.background"] = json!("#ffffff80");
    let (variant, report) = compile_colors(colors);
    let background = hex("#1e1e1e");
    assert_eq!(
        variant.colors.shell,
        hex("#ffffff80").blend_over(background)
    );
    assert_eq!(variant.colors.shell.a, 255);
    let adjustment = report
        .adjustments
        .iter()
        .find(|adjustment| adjustment.role == "shell")
        .unwrap();
    assert_eq!(adjustment.original, "#ffffff80");
    assert_eq!(adjustment.resolved, variant.colors.shell.to_hex());
    assert!(adjustment.reason.contains("flattened"));

    // The background itself is flattened over the fallback background of its appearance.
    let mut colors = good_dark_colors();
    colors["editor.background"] = json!("#ffffff80");
    let (variant, report) = compile_colors(colors);
    assert_eq!(
        variant.colors.background,
        hex("#ffffff80").blend_over(hex("#060606"))
    );
    assert!(
        report
            .adjustments
            .iter()
            .any(|adjustment| adjustment.role == "background")
    );
}

#[test]
fn low_contrast_text_is_raised_against_every_surface() {
    let mut colors = good_dark_colors();
    for key in [
        "editor.background",
        "sideBar.background",
        "editorWidget.background",
        "editorGroupHeader.tabsBackground",
        "quickInput.background",
        "editorHoverWidget.background",
        "input.background",
    ] {
        colors[key] = json!("#808080");
    }
    colors["editor.foreground"] = json!("#777777");
    colors.as_object_mut().unwrap().remove("foreground");
    let (variant, report) = compile_colors(colors);
    let surfaces = variant.colors.text_surfaces();
    assert!(worst_contrast(variant.colors.text, &surfaces) >= 4.5);
    assert!(worst_contrast(variant.colors.text_muted, &surfaces) >= 4.5);
    assert!(worst_contrast(variant.colors.text_faint, &surfaces) >= 3.0);
    let adjustment = report
        .adjustments
        .iter()
        .find(|adjustment| adjustment.role == "text")
        .unwrap();
    assert_eq!(adjustment.original, "#777777");
    assert_eq!(adjustment.resolved, variant.colors.text.to_hex());
    // The source value stays visible in the mapping.
    assert!(
        report
            .mappings
            .iter()
            .any(|mapping| mapping.role == "text" && mapping.value == "#777777")
    );
}

#[test]
fn an_alternative_source_key_is_preferred_over_mixing() {
    let mut colors = good_dark_colors();
    colors["editor.foreground"] = json!("#2a2a2a"); // unreadable on #1e1e1e
    colors["foreground"] = json!("#f0f0f0"); // the next candidate qualifies
    let (variant, report) = compile_colors(colors);
    assert_eq!(variant.colors.text, hex("#f0f0f0"));
    let adjustment = report
        .adjustments
        .iter()
        .find(|adjustment| adjustment.role == "text")
        .unwrap();
    assert!(
        adjustment
            .reason
            .contains("alternative source key `foreground`")
    );
    assert!(
        report
            .mappings
            .iter()
            .any(|mapping| mapping.role == "text" && mapping.source_key == "foreground")
    );
}

#[test]
fn status_cursor_and_strong_border_are_raised_to_three_to_one() {
    let mut colors = good_dark_colors();
    colors["errorForeground"] = json!("#251010");
    colors["editorCursor.foreground"] = json!("#202020");
    colors["focusBorder"] = json!("#1f1f1f");
    let (variant, report) = compile_colors(colors);
    for (name, color) in [
        ("danger", variant.colors.danger),
        ("cursor", variant.colors.cursor),
        ("borderStrong", variant.colors.border_strong),
    ] {
        assert!(
            worst_contrast(color, &[variant.colors.background, variant.colors.shell]) >= 3.0,
            "{name}"
        );
        assert!(
            report
                .adjustments
                .iter()
                .any(|adjustment| adjustment.role == name),
            "{name}"
        );
    }
}

#[test]
fn accent_primary_is_raised_and_roles_are_rederived() {
    let mut colors = good_dark_colors();
    colors["focusBorder"] = json!("#202020");
    colors["button.background"] = json!("#212121");
    colors
        .as_object_mut()
        .unwrap()
        .remove("editorCursor.foreground");
    let (variant, report) = compile_colors(colors);
    let surfaces = [
        variant.colors.background,
        variant.colors.shell,
        variant.colors.raised,
        variant.colors.card,
    ];
    assert!(worst_contrast(variant.accent.primary, &surfaces) >= 3.0);
    assert!(variant.accent.on.contrast(variant.accent.strong) >= 4.5);
    assert_eq!(variant.accent.caret, variant.accent.primary);
    let adjustment = report
        .adjustments
        .iter()
        .find(|adjustment| adjustment.role == "accent.primary")
        .unwrap();
    assert_eq!(adjustment.original, "#202020");

    // A later candidate that already qualifies is chosen instead of mixing.
    let mut colors = good_dark_colors();
    colors["focusBorder"] = json!("#202020");
    colors["button.background"] = json!("#3794ff");
    let (variant, report) = compile_colors(colors);
    assert_eq!(variant.accent.primary, hex("#3794ff"));
    assert!(report.adjustments.iter().any(|adjustment| {
        adjustment.role == "accent.primary"
            && adjustment
                .reason
                .contains("alternative accent candidate `button.background`")
    }));
}

#[test]
fn missing_roles_are_derived_and_reported() {
    let (variant, report) = compile_colors(json!({ "editor.background": "#101820" }));
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("no accent"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("ANSI"))
    );
    assert!(
        report
            .mappings
            .iter()
            .any(|mapping| mapping.role == "shell" && mapping.source_key == "(derived)")
    );
    // The result is complete and valid.
    assert!(!has_blocking_issue(&variant_family(variant).validate()));
}

fn variant_family(variant: vespertine_theme::theme::ThemeVariant) -> ThemeFamily {
    family_of(vec![variant])
}

#[test]
fn imported_variants_are_not_curated_and_survive_resolution_hardening() {
    let (variant, _) = compile_colors(good_dark_colors());
    assert!(!variant.curated);
    assert_eq!(variant.hardened().colors.text, variant.colors.text);
    assert!(!variant.hardened().colors.text_surfaces().is_empty());
}
