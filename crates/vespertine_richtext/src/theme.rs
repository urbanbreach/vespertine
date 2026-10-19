//! The adapter between the text service and `vespertine_theme`: the accent of washes and focus
//! treatments comes from the active THM-003 theme (`accent.primary`), and syntax palettes are
//! built from the THM-002 [`SyntaxPalette`](vespertine_theme::document::SyntaxPalette) or the
//! semantic syntax colors of a resolved variant. Nothing here owns a color of its own: before
//! a theme is installed the built-in neutral dark variant answers.

use std::sync::OnceLock;
use vespertine::{App, Hsla};
use vespertine_theme::appearance::try_theme_of;
use vespertine_theme::builtin::neutral_variant;
use vespertine_theme::theme::{Appearance, ThemeVariant};

/// The built-in neutral dark variant, built once.
pub(crate) fn neutral_dark() -> &'static ThemeVariant {
    static NEUTRAL: OnceLock<ThemeVariant> = OnceLock::new();
    NEUTRAL.get_or_init(|| neutral_variant(Appearance::Dark))
}

/// Colors the text service needs from the theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextTheme {
    /// The accent: selection and search washes default to it with their own alpha, and the
    /// keyboard focus treatment of links draws it as a border.
    pub accent: Hsla,
}

impl TextTheme {
    /// The text colors of a theme variant (`accent.primary`, THM-003 R2).
    pub fn from_variant(variant: &ThemeVariant) -> Self {
        Self {
            accent: variant.accent.primary.to_hsla(),
        }
    }

    /// The text colors of the active theme, or of the built-in neutral dark variant when no
    /// theme is installed. Read once per frame, so a theme change reaches the next frame.
    pub fn current(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_variant(theme),
            None => Self::default(),
        }
    }
}

impl Default for TextTheme {
    /// The text colors of the built-in neutral dark variant.
    fn default() -> Self {
        Self::from_variant(neutral_dark())
    }
}
