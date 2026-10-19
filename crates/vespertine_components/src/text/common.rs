//! Behavior shared by the single-line field and the multiline editor: theme roles, the caret
//! blink cycle, drag autoscroll steps, clipboard classification and the context-menu state.

use std::path::PathBuf;
use vespertine::{
    App, ClipboardEntry, ClipboardItem, Hsla, Image, MotionClock, Pixels, SharedString, hsla, px,
    rgb,
};

/// Caret blink half-period in milliseconds (CMP-007 R14).
pub const BLINK_HALF_PERIOD_MS: f64 = 500.0;
/// Drag autoscroll interval in milliseconds (CMP-007 R10, CMP-008 R9).
pub const AUTOSCROLL_INTERVAL_MS: u64 = 16;
/// Caret width in logical pixels.
pub const CARET_WIDTH: f32 = 2.0;
/// Inset of the clamped drag position from the far field edge.
pub const DRAG_INSET: f32 = 0.5;

/// Text size and line height of an editor (CMP-007 R1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    /// Font size.
    pub size: Pixels,
    /// Line height.
    pub line_height: Pixels,
}

impl Default for TextMetrics {
    fn default() -> Self {
        Self {
            size: px(16.0),
            line_height: px(20.0),
        }
    }
}

/// The theme roles of an editor (CMP-007 R27). Defaults come from the active theme
/// (`vespertine_theme`) and fall back to a neutral dark set when none is installed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldTheme {
    /// Primary text.
    pub text: Hsla,
    /// Faint text: the placeholder.
    pub faint: Hsla,
    /// Muted text: disabled text.
    pub muted: Hsla,
    /// Selection wash.
    pub selection: Hsla,
    /// Caret.
    pub caret: Hsla,
    /// Field surface.
    pub surface: Hsla,
    /// Field border.
    pub border: Hsla,
    /// Focus-visible ring and accent border.
    pub accent: Hsla,
    /// Disabled surface.
    pub disabled_surface: Hsla,
}

impl Default for FieldTheme {
    fn default() -> Self {
        Self {
            text: rgb(0xF0F1F3).into(),
            faint: rgb(0x727886).into(),
            muted: rgb(0xA3A9B5).into(),
            selection: hsla(0.62, 0.9, 0.62, 0.35),
            caret: rgb(0xF0F1F3).into(),
            surface: rgb(0x20232A).into(),
            border: rgb(0x3F4450).into(),
            accent: rgb(0x7C86FF).into(),
            disabled_surface: rgb(0x2A2D35).into(),
        }
    }
}

impl FieldTheme {
    /// The roles of the active application theme, or the neutral defaults before a theme is
    /// installed.
    pub fn resolve(cx: &App) -> Self {
        match vespertine_theme::appearance::try_theme_of(cx) {
            Some(theme) => Self {
                text: theme.colors.text.to_hsla(),
                faint: theme.colors.text_faint.to_hsla(),
                muted: theme.colors.text_muted.to_hsla(),
                selection: theme.accent.selection.to_hsla(),
                caret: theme.accent.caret.to_hsla(),
                surface: theme.colors.input.to_hsla(),
                border: theme.colors.border.to_hsla(),
                accent: theme.accent.primary.to_hsla(),
                disabled_surface: theme.colors.raised.to_hsla(),
            },
            None => Self::default(),
        }
    }
}

/// The enablement of the context editing actions (CMP-007 R24).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditMenuState {
    /// Cut: a non-empty selection in an editable editor.
    pub cut: bool,
    /// Copy: a non-empty selection.
    pub copy: bool,
    /// Paste: an editable editor and insertable clipboard content.
    pub paste: bool,
    /// Select All: a non-empty value.
    pub select_all: bool,
}

/// Which clipboard representation a paste prefers (PLT-009 R4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PastePreference {
    /// Text first, unchanged from ordinary text behavior; the default.
    #[default]
    TextFirst,
    /// Images and file paths first, falling back to text when neither is present.
    MediaFirst,
}

/// The classified content of the clipboard.
#[derive(Clone, Debug, Default)]
pub struct ClipboardContent {
    /// The concatenated text of the string entries (paths are never turned into text).
    pub text: Option<String>,
    /// Metadata of a single string entry.
    pub metadata: Option<String>,
    /// External file paths.
    pub paths: Vec<PathBuf>,
    /// Images.
    pub images: Vec<Image>,
}

impl ClipboardContent {
    /// Classifies a clipboard item. Entries that are file paths or images are not text.
    pub fn from_item(item: &ClipboardItem) -> Self {
        let mut content = Self::default();
        let mut text = String::new();
        let mut strings = 0;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::String(string) => {
                    text.push_str(string.text());
                    strings += 1;
                    if strings == 1 {
                        content.metadata = string.metadata.clone();
                    }
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    content.paths.extend(paths.paths().iter().cloned());
                }
                ClipboardEntry::Image(image) => content.images.push(image.clone()),
            }
        }
        if strings != 1 {
            content.metadata = None;
        }
        content.text = (!text.is_empty()).then_some(text);
        content
    }

    /// The classified content of the system clipboard; empty when it holds nothing.
    pub fn read(cx: &App) -> Self {
        cx.read_from_clipboard()
            .map(|item| Self::from_item(&item))
            .unwrap_or_default()
    }

    /// Whether any media (paths or images) is present.
    pub fn has_media(&self) -> bool {
        !self.paths.is_empty() || !self.images.is_empty()
    }
}

/// Whether the caret is in the visible half of its blink cycle `elapsed_ms` after the last edit
/// or caret move: visible when `floor(elapsed / 500)` is even (CMP-007 R14).
pub fn caret_phase_visible(elapsed_ms: f64) -> bool {
    if elapsed_ms < 0.0 {
        return true;
    }
    ((elapsed_ms / BLINK_HALF_PERIOD_MS).floor() as u64).is_multiple_of(2)
}

/// The signed per-tick autoscroll step for a pointer `distance` logical pixels beyond an edge:
/// `clamp(0.2 x distance, 1, line_height)` (CMP-007 R10, CMP-008 R9). The sign is the caller's.
pub fn autoscroll_step(distance: f32, line_height: f32) -> f32 {
    (0.2 * distance.abs()).clamp(1.0, line_height.max(1.0))
}

/// Blink bookkeeping of an editor.
#[derive(Debug)]
pub struct Blink {
    /// Motion-clock time of the last edit or caret move.
    pub last_activity_ms: f64,
    /// The visibility last painted, so the driver repaints only on a flip.
    pub painted_visible: bool,
    /// Whether a repaint driver task is running.
    pub driver_running: bool,
}

impl Blink {
    /// A fresh cycle starting now.
    pub fn new(cx: &App) -> Self {
        Self {
            last_activity_ms: MotionClock::now_ms(cx),
            painted_visible: true,
            driver_running: false,
        }
    }

    /// Restarts the solid phase.
    pub fn reset(&mut self, cx: &App) {
        self.last_activity_ms = MotionClock::now_ms(cx);
    }

    /// Whether the cycle is in its visible half at `now_ms`.
    pub fn phase_visible(&self, now_ms: f64) -> bool {
        caret_phase_visible(now_ms - self.last_activity_ms)
    }

    /// Milliseconds until the next phase flip at `now_ms`, at least one frame.
    pub fn next_flip_ms(&self, now_ms: f64) -> u64 {
        let into = (now_ms - self.last_activity_ms).max(0.0) % BLINK_HALF_PERIOD_MS;
        ((BLINK_HALF_PERIOD_MS - into).ceil() as u64).max(AUTOSCROLL_INTERVAL_MS)
    }
}

/// The byte index of the character boundary of `line` closest to `x` (relative to the line
/// origin): the nearest glyph start, or the line end when that is nearer (CMP-007 R15).
pub fn closest_boundary(line: &vespertine::ShapedLine, x: Pixels) -> usize {
    let mut best = 0;
    let mut best_distance = x.abs();
    for run in &line.runs {
        for glyph in &run.glyphs {
            let distance = (glyph.position.x - x).abs();
            if distance < best_distance {
                best = glyph.index;
                best_distance = distance;
            }
        }
    }
    if (line.width() - x).abs() < best_distance {
        best = line.len();
    }
    best
}

/// Placeholder text accessor used by accessibility (kept as a shared type for both editors).
pub type Placeholder = SharedString;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_cycle_is_solid_for_500_ms_then_alternates() {
        assert!(caret_phase_visible(0.0));
        assert!(caret_phase_visible(499.0));
        assert!(!caret_phase_visible(500.0));
        assert!(!caret_phase_visible(999.0));
        assert!(caret_phase_visible(1000.0));
        assert!(!caret_phase_visible(1500.0));
    }

    #[test]
    fn autoscroll_steps_follow_the_distance_with_bounds() {
        assert_eq!(autoscroll_step(10.0, 20.0), 2.0);
        assert_eq!(autoscroll_step(100.0, 20.0), 20.0);
        assert_eq!(autoscroll_step(1000.0, 20.0), 20.0);
        assert_eq!(autoscroll_step(1.0, 20.0), 1.0);
        assert_eq!(autoscroll_step(0.0, 20.0), 1.0);
        assert_eq!(autoscroll_step(1000.0, 16.0), 16.0);
    }

    #[test]
    fn clipboard_paths_and_images_are_not_text() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(vespertine::ExternalPaths(
                vec![PathBuf::from("/tmp/a.txt")].into(),
            ))],
        };
        let content = ClipboardContent::from_item(&item);
        assert_eq!(content.text, None);
        assert_eq!(content.paths, vec![PathBuf::from("/tmp/a.txt")]);
        assert!(content.has_media());
        let content = ClipboardContent::from_item(&ClipboardItem::new_string("héllo".into()));
        assert_eq!(content.text.as_deref(), Some("héllo"));
        assert!(!content.has_media());
    }
}
