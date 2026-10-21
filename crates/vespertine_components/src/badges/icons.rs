//! The small icon set the CMP-028 components draw themselves (copy, warning, comment, edit,
//! remove, plus). Domain icons (a badge's icon) are supplied by the host by name.
//!
//! The components address icons by asset path. [`IconAssets`] serves these paths from embedded
//! SVGs; hosts chain it before their own asset source, or provide the same paths themselves.

use std::borrow::Cow;

use anyhow::Result;
use vespertine::{AssetSource, SharedString};

/// Copy glyph (two overlapping rectangles).
pub const ICON_COPY: &str = "vespertine/icons/copy.svg";
/// Warning triangle.
pub const ICON_WARNING: &str = "vespertine/icons/warning.svg";
/// Comment bubble.
pub const ICON_COMMENT: &str = "vespertine/icons/comment.svg";
/// Pencil.
pub const ICON_EDIT: &str = "vespertine/icons/edit.svg";
/// Cross.
pub const ICON_REMOVE: &str = "vespertine/icons/remove.svg";
/// Plus.
pub const ICON_PLUS: &str = "vespertine/icons/plus.svg";

const STROKE: &str = r#"fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round""#;

fn svg(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" {STROKE}>{body}</svg>"#)
}

fn icon(path: &str) -> Option<String> {
    Some(match path {
        ICON_COPY => svg(
            r#"<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/>"#,
        ),
        ICON_WARNING => {
            svg(r#"<path d="M12 3 22 20H2Z"/><path d="M12 10v5"/><path d="M12 18v.01"/>"#)
        }
        ICON_COMMENT => svg(r#"<path d="M4 4h16v12H9l-5 4Z"/>"#),
        ICON_EDIT => svg(r#"<path d="M4 20l1-5L16 4l4 4L9 19Z"/><path d="M14 6l4 4"/>"#),
        ICON_REMOVE => svg(r#"<path d="M6 6l12 12"/><path d="M18 6 6 18"/>"#),
        ICON_PLUS => svg(r#"<path d="M12 5v14"/><path d="M5 12h14"/>"#),
        _ => return None,
    })
}

/// An asset source serving the icons of this module.
#[derive(Clone, Copy, Debug, Default)]
pub struct IconAssets;

impl AssetSource for IconAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(icon(path).map(|svg| Cow::Owned(svg.into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok([
            ICON_COPY,
            ICON_WARNING,
            ICON_COMMENT,
            ICON_EDIT,
            ICON_REMOVE,
            ICON_PLUS,
        ]
        .into_iter()
        .filter(|candidate| candidate.starts_with(path))
        .map(SharedString::from)
        .collect())
    }
}
