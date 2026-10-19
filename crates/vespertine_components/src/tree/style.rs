//! Theme roles, metrics and row washes of the tree (CMP-016 R7, R8).

use vespertine::{App, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Styled, canvas, point, px};
use vespertine_theme::appearance::try_theme_of;
use vespertine_theme::builtin::neutral_variant;
use vespertine_theme::theme::{Appearance, ThemeVariant};

/// Row height of the tree and of the search rows.
pub const ROW_HEIGHT: f32 = 27.0;
/// Indent per nesting level.
pub const INDENT: f32 = 14.0;
/// Left padding of a row.
pub const ROW_PADDING_LEFT: f32 = 8.0;
/// Right padding of a row.
pub const ROW_PADDING_RIGHT: f32 = 8.0;
/// Width of the disclosure slot.
pub const DISCLOSURE_SLOT: f32 = 14.0;
/// Size of the chevron inside the slot.
pub const CHEVRON_SIZE: f32 = 11.0;
/// Size of the entry identity icon.
pub const ICON_SIZE: f32 = 14.0;
/// Gap between the disclosure slot, the icon and the name.
pub const GAP: f32 = 4.0;
/// Width of a guide line.
pub const GUIDE_WIDTH: f32 = 1.0;
/// Band of the edge fade.
pub const FADE_BAND: f32 = 24.0;
/// Font size of entry names.
pub const NAME_SIZE: f32 = 11.5;
/// Font size of status rows and search messages.
pub const STATUS_SIZE: f32 = 10.5;
/// Height of the rename field.
pub const RENAME_HEIGHT: f32 = 23.0;
/// Horizontal padding of the rename field.
pub const RENAME_PADDING: f32 = 4.0;
/// Corner radius of the rename field.
pub const RENAME_RADIUS: f32 = 4.0;
/// Opacity of ignored entries without a decoration.
pub const IGNORED_OPACITY: f32 = 0.52;

/// The x position of the guide of nesting `level` inside a row.
pub fn guide_x(level: usize) -> f32 {
    ROW_PADDING_LEFT + DISCLOSURE_SLOT / 2.0 + level as f32 * INDENT
}

/// The left padding of a row at `depth`.
pub fn row_padding_left(depth: usize) -> f32 {
    ROW_PADDING_LEFT + depth as f32 * INDENT
}

/// The theme roles the tree uses (THM-003).
#[derive(Clone, Debug, PartialEq)]
pub struct TreeTheme {
    /// Primary text; also the neutral wash tint.
    pub text: Hsla,
    /// Muted text.
    pub text_muted: Hsla,
    /// Faint text.
    pub text_faint: Hsla,
    /// Hairline borders (indentation guides).
    pub border: Hsla,
    /// Strong border.
    pub border_strong: Hsla,
    /// Accent (rename border, drop target).
    pub accent: Hsla,
    /// Input surface (rename field fill).
    pub input: Hsla,
    /// Overlay surface (context menu, drag ghost).
    pub overlay: Hsla,
    /// Danger (error rows, rejected rename).
    pub danger: Hsla,
}

impl TreeTheme {
    /// The theme roles of a resolved variant.
    pub fn from_variant(variant: &ThemeVariant) -> Self {
        let colors = &variant.colors;
        Self {
            text: colors.text.to_hsla(),
            text_muted: colors.text_muted.to_hsla(),
            text_faint: colors.text_faint.to_hsla(),
            border: colors.border.to_hsla(),
            border_strong: colors.border_strong.to_hsla(),
            accent: variant.accent.primary.to_hsla(),
            input: colors.input.to_hsla(),
            overlay: colors.overlay.to_hsla(),
            danger: colors.danger.to_hsla(),
        }
    }

    /// The active theme of the application, or the neutral dark theme before the theme system
    /// is initialized.
    pub fn current(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_variant(&theme.variant),
            None => Self::default(),
        }
    }

    /// The neutral wash: the text colour at `alpha`.
    pub fn wash(&self, alpha: f32) -> Hsla {
        self.text.opacity(alpha)
    }
}

impl Default for TreeTheme {
    fn default() -> Self {
        Self::from_variant(&neutral_variant(Appearance::Dark))
    }
}

/// The default alphas of the neutral wash (R8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeWashes {
    /// Selected row while the tree has focus.
    pub selected_focused: f32,
    /// Selected row while the tree has no focus.
    pub selected_unfocused: f32,
    /// A distinct keyboard cursor on a selected row.
    pub cursor_selected: f32,
    /// A distinct keyboard cursor on an unselected row.
    pub cursor_unselected: f32,
    /// Hovered unselected row.
    pub hover: f32,
    /// Active drop-target directory.
    pub drop_target: f32,
    /// Workspace-root drop target.
    pub root_drop: f32,
}

impl Default for TreeWashes {
    fn default() -> Self {
        Self {
            selected_focused: 0.12,
            selected_unfocused: 0.08,
            cursor_selected: 0.16,
            cursor_unselected: 0.04,
            hover: 0.055,
            drop_target: 0.18,
            root_drop: 0.16,
        }
    }
}

/// The state of a row that decides its wash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowVisual {
    /// The row is selected.
    pub selected: bool,
    /// The tree has keyboard focus.
    pub focused: bool,
    /// The row holds the keyboard cursor.
    pub is_cursor: bool,
    /// The selection has more than one entry.
    pub multiple_selected: bool,
    /// The pointer is over the row.
    pub hovered: bool,
    /// The row is the active drop-target directory.
    pub drop_target: bool,
}

impl RowVisual {
    /// Whether the cursor is marked as a distinct row: the tree is focused and either the
    /// cursor is not selected or the selection has more than one entry.
    pub fn cursor_marked(&self) -> bool {
        self.focused && self.is_cursor && (!self.selected || self.multiple_selected)
    }
}

impl TreeWashes {
    /// The alpha of the neutral wash for a row, or `None` for no wash: the drop target wins,
    /// then selection (with the distinct-cursor emphasis), then the cursor marker, then hover
    /// (unselected rows only).
    pub fn row_alpha(&self, row: &RowVisual) -> Option<f32> {
        if row.drop_target {
            return Some(self.drop_target);
        }
        if row.selected {
            return Some(if row.cursor_marked() {
                self.cursor_selected
            } else if row.focused {
                self.selected_focused
            } else {
                self.selected_unfocused
            });
        }
        let marked = row.cursor_marked().then_some(self.cursor_unselected);
        match (marked, row.hovered) {
            (Some(cursor), true) => Some(cursor.max(self.hover)),
            (Some(cursor), false) => Some(cursor),
            (None, true) => Some(self.hover),
            (None, false) => None,
        }
    }
}

/// A chevron of `size` in `color`: pointing down when `expanded`, right otherwise.
pub fn chevron(expanded: bool, size: Pixels, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window, _| {
            let center = bounds.center();
            let arm = size * 0.22;
            let (a, b, c) = if expanded {
                (
                    point(center.x - arm * 1.4, center.y - arm * 0.7),
                    point(center.x, center.y + arm * 0.7),
                    point(center.x + arm * 1.4, center.y - arm * 0.7),
                )
            } else {
                (
                    point(center.x - arm * 0.7, center.y - arm * 1.4),
                    point(center.x + arm * 0.7, center.y),
                    point(center.x - arm * 0.7, center.y + arm * 1.4),
                )
            };
            let mut builder = PathBuilder::stroke(px(1.25));
            builder.move_to(a);
            builder.line_to(b);
            builder.line_to(c);
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(size)
}
