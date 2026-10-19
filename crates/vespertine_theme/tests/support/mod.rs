//! Shared fixtures for the import and library tests.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use vespertine_theme::{
    builtin::neutral_variant,
    color::Color,
    import::{Compilation, ImportAdapter, ImportOptions, SourceKind, VariantReport, VsCodeAdapter},
    theme::{Appearance, SourceMetadata, ThemeFamily, ThemeVariant},
};

pub fn hex(text: &str) -> Color {
    Color::parse(text).unwrap()
}

/// A complete dark theme whose roles all pass hardening untouched.
pub fn good_dark_colors() -> Value {
    json!({
        "editor.background": "#1e1e1e",
        "editor.foreground": "#d4d4d4",
        "foreground": "#cccccc",
        "descriptionForeground": "#b0b0b0",
        "disabledForeground": "#8a8a8a",
        "sideBar.background": "#252526",
        "editorWidget.background": "#2b2b2c",
        "editorGroupHeader.tabsBackground": "#2d2d2e",
        "quickInput.background": "#2a2a2b",
        "editorHoverWidget.background": "#303031",
        "input.background": "#3c3c3c",
        "button.background": "#0e639c",
        "button.foreground": "#ffffff",
        "list.hoverBackground": "#2a2d2e80",
        "list.activeSelectionBackground": "#094771",
        "panel.border": "#80808059",
        "focusBorder": "#4aa3ff",
        "errorForeground": "#f48771",
        "editorWarning.foreground": "#cca700",
        "gitDecoration.addedResourceForeground": "#89d185",
        "gitDecoration.deletedResourceForeground": "#c74e39",
        "editorGutter.modifiedBackground": "#1b81a8",
        "editorCursor.foreground": "#aeafad",
        "terminal.ansiBlack": "#000000",
        "terminal.ansiRed": "#cd3131",
        "terminal.ansiGreen": "#0dbc79",
        "terminal.ansiYellow": "#e5e510",
        "terminal.ansiBlue": "#2472c8",
        "terminal.ansiMagenta": "#bc3fbc",
        "terminal.ansiCyan": "#11a8cd",
        "terminal.ansiWhite": "#e5e5e5",
        "terminal.ansiBrightBlack": "#666666",
        "terminal.ansiBrightRed": "#f14c4c",
        "terminal.ansiBrightGreen": "#23d18b",
        "terminal.ansiBrightYellow": "#f5f543",
        "terminal.ansiBrightBlue": "#3b8eea",
        "terminal.ansiBrightMagenta": "#d670d6",
        "terminal.ansiBrightCyan": "#29b8db",
        "terminal.ansiBrightWhite": "#e5e5e5"
    })
}

pub fn theme_json(colors: Value) -> Value {
    json!({
        "name": "Fixture",
        "type": "dark",
        "colors": colors,
        "tokenColors": [
            { "scope": "comment", "settings": { "foreground": "#6a9955" } },
            { "scope": ["keyword", "storage.type"], "settings": { "foreground": "#569cd6" } },
            { "scope": "string", "settings": { "foreground": "#ce9178" } },
            { "scope": "constant.numeric", "settings": { "foreground": "#b5cea8" } },
            { "scope": "entity.name.function", "settings": { "foreground": "#dcdcaa" } }
        ]
    })
}

pub fn write_json(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

pub fn options(family: &str, path: &Path) -> ImportOptions {
    ImportOptions::local(family, family, path)
}

pub fn compile(
    path: &Path,
    family: &str,
) -> Result<Compilation, vespertine_theme::import::ImportError> {
    VsCodeAdapter.compile(path, &options(family, path))
}

pub fn package_manifest(themes: Vec<Value>) -> Value {
    json!({ "name": "pkg", "contributes": { "themes": themes } })
}

/// A hand-built compilation of `count` independent variants, for library tests.
pub fn fake_compilation(family_id: &str, count: usize) -> Compilation {
    let variants: Vec<ThemeVariant> = (0..count)
        .map(|index| {
            let mut variant = neutral_variant(if index % 2 == 0 {
                Appearance::Dark
            } else {
                Appearance::Light
            });
            variant.id = format!("{family_id}-v{index}");
            variant.family_id = family_id.to_string();
            variant.name = format!("Variant {index}");
            variant.curated = false;
            variant.source = SourceMetadata {
                format: "test".to_string(),
                url: "https://example.test".to_string(),
                revision: "1".to_string(),
                license: "MIT".to_string(),
                asset_hash: "00".to_string(),
            };
            variant
        })
        .collect();
    let reports = variants
        .iter()
        .map(|variant| VariantReport {
            variant_id: variant.id.clone(),
            ..VariantReport::default()
        })
        .collect();
    Compilation {
        source_kind: SourceKind::File,
        source_path: PathBuf::from("/tmp/fake-theme.json"),
        format: "test".to_string(),
        options: ImportOptions::default(),
        family: ThemeFamily {
            id: family_id.to_string(),
            name: family_id.to_string(),
            variants,
        },
        reports,
        failures: Vec::new(),
    }
}
