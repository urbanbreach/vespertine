//! Typography resolution of the code components (CMP-014 R4): one value used for shaping,
//! per-run fonts and row geometry alike.

use vespertine::{FontWeight, Hsla, Pixels, SharedString, px};
use vespertine_theme::document::DocumentTheme;

use crate::doc_theme::hsla;

/// Style overrides of the typography; absent or nonpositive entries fall back to the theme.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypographyOverrides {
    /// Font family override.
    pub family: Option<SharedString>,
    /// Font weight override.
    pub weight: Option<FontWeight>,
    /// Font size override; a nonpositive size is ignored.
    pub size: Option<Pixels>,
    /// Line height override; a nonpositive height is ignored.
    pub line_height: Option<Pixels>,
    /// Plain text color override.
    pub color: Option<Hsla>,
}

/// Resolved typography of a code block or editor.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeTypography {
    /// Font family.
    pub family: SharedString,
    /// Font weight.
    pub weight: FontWeight,
    /// Font size.
    pub size: Pixels,
    /// Height of one row.
    pub line_height: Pixels,
    /// Plain (unclassified) text color.
    pub color: Hsla,
}

impl CodeTypography {
    /// Resolves the typography: overrides first, then the theme (monospace family, code text
    /// size and line height, the text token). A positive size override without a line-height
    /// override scales the theme's line height by `size / theme size`; when the theme size is
    /// zero or negative the theme line height is used unchanged.
    pub fn resolve(overrides: &TypographyOverrides, theme: &DocumentTheme) -> Self {
        let theme_size = theme.metrics.code_text_size;
        let theme_line_height = theme.metrics.code_line_height;
        let size_override = overrides.size.filter(|size| size.as_f32() > 0.);
        let line_height_override = overrides.line_height.filter(|height| height.as_f32() > 0.);
        let size = size_override.unwrap_or(px(theme_size));
        let line_height = match (line_height_override, size_override) {
            (Some(height), _) => height,
            (None, Some(size)) if theme_size > 0. => {
                px(theme_line_height * size.as_f32() / theme_size)
            }
            _ => px(theme_line_height),
        };
        Self {
            family: overrides
                .family
                .clone()
                .unwrap_or_else(|| theme.font_mono.clone()),
            weight: overrides.weight.unwrap_or(FontWeight::NORMAL),
            size,
            line_height,
            color: overrides.color.unwrap_or_else(|| hsla(theme.text)),
        }
    }

    /// The font of every run: family and weight, regular style.
    pub fn font(&self) -> vespertine::Font {
        let mut font = vespertine::font(self.family.clone());
        font.weight = self.weight;
        font
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> DocumentTheme {
        DocumentTheme::dark()
    }

    #[test]
    fn theme_defaults_apply_without_overrides() {
        let typography = CodeTypography::resolve(&TypographyOverrides::default(), &theme());
        assert_eq!(typography.size, px(12.5));
        assert_eq!(typography.line_height, px(18.));
        assert_eq!(typography.family, theme().font_mono);
    }

    #[test]
    fn style_wins_over_theme_and_size_scales_the_line_height() {
        let overrides = TypographyOverrides {
            size: Some(px(25.)),
            family: Some("Liberation Mono".into()),
            ..Default::default()
        };
        let typography = CodeTypography::resolve(&overrides, &theme());
        assert_eq!(typography.family.as_ref(), "Liberation Mono");
        assert_eq!(typography.size, px(25.));
        assert_eq!(typography.line_height, px(36.));
    }

    #[test]
    fn explicit_line_height_is_not_scaled() {
        let overrides = TypographyOverrides {
            size: Some(px(25.)),
            line_height: Some(px(30.)),
            ..Default::default()
        };
        assert_eq!(
            CodeTypography::resolve(&overrides, &theme()).line_height,
            px(30.)
        );
    }

    #[test]
    fn zero_theme_size_keeps_the_theme_line_height() {
        let mut theme = theme();
        theme.metrics.code_text_size = 0.;
        let overrides = TypographyOverrides {
            size: Some(px(20.)),
            ..Default::default()
        };
        let typography = CodeTypography::resolve(&overrides, &theme);
        assert!(typography.line_height.as_f32().is_finite());
        assert_eq!(typography.line_height, px(18.));
    }

    #[test]
    fn nonpositive_overrides_are_ignored() {
        let overrides = TypographyOverrides {
            size: Some(px(0.)),
            line_height: Some(px(-3.)),
            ..Default::default()
        };
        let typography = CodeTypography::resolve(&overrides, &theme());
        assert_eq!(typography.size, px(12.5));
        assert_eq!(typography.line_height, px(18.));
    }
}
