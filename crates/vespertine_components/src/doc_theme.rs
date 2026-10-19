//! Helpers shared by the document components (code, Markdown, diff) for reading a
//! [`DocumentTheme`] (THM-002): token colors as [`Hsla`], the ink color and alpha derivations,
//! and the syntax palette.

use vespertine::{Hsla, Rgba};
use vespertine_richtext::syntax::{CATEGORY_KEYS, SyntaxColors};
use vespertine_theme::document::{DocumentTheme, SyntaxCategory};

/// A token color as [`Hsla`].
pub fn hsla(color: Rgba) -> Hsla {
    Hsla::from(color)
}

/// `color` with its alpha REPLACED by `alpha`.
pub fn alpha(color: impl Into<Hsla>, alpha: f32) -> Hsla {
    Hsla {
        a: alpha,
        ..color.into()
    }
}

/// `color` with its alpha MULTIPLIED by `factor`.
pub fn scale_alpha(color: impl Into<Hsla>, factor: f32) -> Hsla {
    let color = color.into();
    Hsla {
        a: color.a * factor,
        ..color
    }
}

/// The ink color of a theme: white when the background lightness is below 0.5, black otherwise.
pub fn ink(theme: &DocumentTheme) -> Hsla {
    let background = hsla(theme.bg);
    if background.l < 0.5 {
        Hsla {
            h: 0.,
            s: 0.,
            l: 1.,
            a: 1.,
        }
    } else {
        Hsla {
            h: 0.,
            s: 0.,
            l: 0.,
            a: 1.,
        }
    }
}

/// The ink color at `alpha`.
pub fn ink_alpha(theme: &DocumentTheme, a: f32) -> Hsla {
    alpha(ink(theme), a)
}

/// The syntax palette of a theme: its twenty-four categories, with the text token as the plain
/// foreground.
pub fn syntax_colors(theme: &DocumentTheme) -> SyntaxColors {
    let mut colors = SyntaxColors::dark().with_plain(hsla(theme.text));
    for key in CATEGORY_KEYS {
        if let Some(category) = SyntaxCategory::from_key(key) {
            colors.set_category(key, hsla(theme.syntax.get(category)));
        }
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_follows_background_lightness() {
        assert_eq!(ink(&DocumentTheme::dark()).l, 1.);
        assert_eq!(ink(&DocumentTheme::light()).l, 0.);
    }

    #[test]
    fn syntax_colors_come_from_the_theme_palette() {
        let theme = DocumentTheme::dark();
        let colors = syntax_colors(&theme);
        assert_eq!(colors.keyword, hsla(theme.syntax.keyword));
        assert_eq!(colors.plain, hsla(theme.text));
    }
}
