//! The built-in neutral family `default-dark` / `default-light` (THM-003 R4).
//!
//! Background, text, border, accent, code and diff values are the THM-002 tokens of each
//! appearance, read through [`neutral_tokens`] — the single seam to the component palette
//! defaults. Every other role is a neutral derivation of those tokens. Light and dark are
//! authored independently; neither is derived from the other.

use crate::{
    accent::derive_accent,
    color::Color,
    document::{DocumentTheme, SyntaxCategory as DocumentCategory},
    theme::{
        Appearance, ColorTable, DEFAULT_DARK_ID, DEFAULT_FAMILY_ID, DEFAULT_FROST_TINT_ALPHA,
        DEFAULT_LIGHT_ID, SourceMetadata, SurfaceTreatment, SyntaxCategory, SyntaxMap,
        TerminalPalette, ThemeFamily, ThemeVariant,
    },
};
use sha2::{Digest, Sha256};

/// THM-002 token defaults of one appearance (the neutral component palette), taken from
/// [`DocumentTheme`].
#[derive(Clone, Copy, Debug)]
pub struct NeutralTokens {
    /// `bg`.
    pub bg: Color,
    /// `border`.
    pub border: Color,
    /// `text`.
    pub text: Color,
    /// `textMuted`.
    pub text_muted: Color,
    /// `textFaint`.
    pub text_faint: Color,
    /// `accent` (and `caret`).
    pub accent: Color,
    /// `codeText`.
    pub code_text: Color,
    /// `codeWash`.
    pub code_wash: Color,
    /// `diffAdd`.
    pub diff_add: Color,
    /// `diffDel`.
    pub diff_del: Color,
    /// `diffHunkBg`.
    pub diff_hunk_bg: Color,
}

fn hex(text: &str) -> Color {
    Color::parse(text).expect("built-in color literal is valid")
}

/// The THM-002 token defaults for `appearance`.
pub fn neutral_tokens(appearance: Appearance) -> NeutralTokens {
    let document = DocumentTheme::for_appearance(appearance);
    NeutralTokens {
        bg: Color::from_rgba(document.bg),
        border: Color::from_rgba(document.border),
        text: Color::from_rgba(document.text),
        text_muted: Color::from_rgba(document.text_muted),
        text_faint: Color::from_rgba(document.text_faint),
        accent: Color::from_rgba(document.accent),
        code_text: Color::from_rgba(document.code_text),
        code_wash: Color::from_rgba(document.code_wash),
        diff_add: Color::from_rgba(document.diff_add),
        diff_del: Color::from_rgba(document.diff_del),
        diff_hunk_bg: Color::from_rgba(document.diff_hunk_bg),
    }
}

/// The neutral default color of a syntax category: the THM-002 syntax palette for the 24 core
/// categories; markup categories reuse the palette color of the closest core category.
pub fn neutral_syntax_color(appearance: Appearance, category: SyntaxCategory) -> Color {
    use SyntaxCategory::*;
    let core = match category {
        MarkupHeading => Keyword,
        MarkupRaw => String,
        MarkupLink => Tag,
        MarkupReference => Number,
        MarkupEmphasis | MarkupStrong => Variable,
        other => other,
    };
    let key =
        DocumentCategory::from_key(core.name()).expect("every core category is a THM-002 category");
    Color::from_rgba(DocumentTheme::for_appearance(appearance).syntax.get(key))
}

/// The neutral ANSI palette of an appearance (0-7 normal, 8-15 bright).
pub fn neutral_ansi(appearance: Appearance) -> [Color; 16] {
    let table: [&str; 16] = match appearance {
        Appearance::Dark => [
            "#1c1c1c", "#ff6467", "#00d492", "#dba924", "#8f96ed", "#e679b4", "#5bc8d6", "#c8c8c8",
            "#737373", "#ff8a8c", "#2fe6ab", "#efc24a", "#aab0ff", "#f79ac9", "#7fdce8", "#f5f5f5",
        ],
        Appearance::Light => [
            "#222222", "#c4161c", "#00794f", "#8a6400", "#3b4cca", "#a8266f", "#00717f", "#6b6b6b",
            "#525252", "#d6242b", "#008a5a", "#9a7200", "#4a5ee0", "#bc2f82", "#008496", "#3a3a3a",
        ],
    };
    table.map(hex)
}

fn neutral_colors(appearance: Appearance) -> ColorTable {
    let tokens = neutral_tokens(appearance);
    let surface = |amount: f64| tokens.bg.mix(tokens.text, amount);
    let (solid, on_solid) = (tokens.text, tokens.bg);
    let warning = neutral_syntax_color(appearance, SyntaxCategory::Number);
    ColorTable {
        background: tokens.bg,
        shell: surface(0.02),
        raised: surface(0.05),
        card: surface(0.07),
        dialog: surface(0.06),
        overlay: surface(0.09),
        input: surface(0.04),
        solid,
        hover: tokens.text.with_alpha(0.06),
        active: tokens.text.with_alpha(0.10),
        border: tokens.border,
        border_strong: tokens.text.with_alpha(0.30),
        text: tokens.text,
        text_muted: tokens.text_muted,
        text_faint: tokens.text_faint,
        on_solid,
        danger: tokens.diff_del,
        danger_muted: tokens.diff_del.with_alpha(0.16),
        warning,
        warning_muted: warning.with_alpha(0.16),
        success: tokens.diff_add,
        success_muted: tokens.diff_add.with_alpha(0.16),
        cursor: tokens.accent,
        diff_add: tokens.diff_add,
        diff_delete: tokens.diff_del,
        diff_hunk: tokens.diff_hunk_bg,
    }
}

/// Builds the neutral variant of an appearance, authored independently of the other.
pub fn neutral_variant(appearance: Appearance) -> ThemeVariant {
    let tokens = neutral_tokens(appearance);
    let colors = neutral_colors(appearance);
    let accent = derive_accent(tokens.accent, colors.background, appearance);
    let (id, name) = match appearance {
        Appearance::Dark => (DEFAULT_DARK_ID, "Default Dark"),
        Appearance::Light => (DEFAULT_LIGHT_ID, "Default Light"),
    };
    let mut syntax = SyntaxMap::new();
    for category in SyntaxCategory::ALL {
        syntax.set(category, neutral_syntax_color(appearance, category));
    }
    let provenance = format!("vespertine builtin neutral theme {id} v1");
    ThemeVariant {
        id: id.to_string(),
        family_id: DEFAULT_FAMILY_ID.to_string(),
        name: name.to_string(),
        appearance,
        surface: SurfaceTreatment::Opaque,
        terminal: TerminalPalette {
            background: colors.background,
            foreground: colors.text,
            selection: accent.selection,
            ansi: neutral_ansi(appearance),
        },
        colors,
        accent,
        syntax,
        source: SourceMetadata {
            format: "builtin".to_string(),
            url: "https://github.com/urbanbreach/vespertine".to_string(),
            revision: "1".to_string(),
            license: "Apache-2.0".to_string(),
            asset_hash: format!("{:x}", Sha256::digest(provenance.as_bytes())),
        },
        frost_tint_alpha: DEFAULT_FROST_TINT_ALPHA,
        curated: true,
    }
}

/// The built-in neutral family: the dark variant then the light variant.
pub fn neutral_family() -> ThemeFamily {
    ThemeFamily {
        id: DEFAULT_FAMILY_ID.to_string(),
        name: "Default".to_string(),
        variants: vec![
            neutral_variant(Appearance::Dark),
            neutral_variant(Appearance::Light),
        ],
    }
}
