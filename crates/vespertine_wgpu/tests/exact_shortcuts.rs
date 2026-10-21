//! The shaping shortcuts of the text system against the real Linux backend: a layout a shortcut
//! produces is the backend's own shaping of the same line, up to the rounding of the additions
//! that move a reused part.

use std::{borrow::Cow, sync::Arc};
use vespertine::{
    FontRun, LineLayout, Pixels, PlatformTextSystem, TextRun, TextSystem, WindowTextSystem, font,
    px,
};
use vespertine_wgpu::CosmicTextSystem;

const IBM_PLEX: &[u8] =
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");
const LILEX: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf");

const FAMILIES: [&str; 2] = ["IBM Plex Sans", "Lilex"];

struct Fixture {
    platform: Arc<CosmicTextSystem>,
    system: Arc<TextSystem>,
    window: WindowTextSystem,
}

fn fixture() -> Fixture {
    let platform = Arc::new(CosmicTextSystem::new_without_system_fonts("IBM Plex Sans"));
    platform
        .add_fonts(vec![Cow::Borrowed(IBM_PLEX), Cow::Borrowed(LILEX)])
        .expect("bundled fonts load");
    let system = Arc::new(TextSystem::new(platform.clone()));
    let window = WindowTextSystem::new(system.clone());
    Fixture {
        platform,
        system,
        window,
    }
}

impl Fixture {
    /// Lays `text` out through the text system's caches and shortcuts.
    fn cached(&self, text: &str, family: &str) -> Arc<LineLayout> {
        let run = TextRun {
            len: text.len(),
            font: font(family.to_owned()),
            ..Default::default()
        };
        self.window.layout_line(text, px(15.), &[run], None)
    }

    /// Shapes `text` directly on the backend.
    fn platform(&self, text: &str, family: &str) -> LineLayout {
        let font_id = self.system.resolve_font(&font(family.to_owned()));
        self.platform.layout_line(
            text,
            px(15.),
            &[FontRun {
                len: text.len(),
                font_id,
            }],
        )
    }
}

/// Asserts equal glyphs, clusters and metrics, and positions and width equal up to a few units
/// in the last place of the line width: the rounding of one more addition per glyph.
#[track_caller]
fn assert_same_up_to_rounding(actual: &LineLayout, expected: &LineLayout, text: &str) {
    let tolerance = 4.0 * f32::EPSILON * f32::from(expected.width).abs().max(1.0);
    let close = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() <= tolerance;
    assert!(close(actual.width, expected.width), "width of `{text}`");
    assert_eq!(
        (actual.len, actual.ascent, actual.descent),
        (expected.len, expected.ascent, expected.descent),
        "metrics of `{text}`"
    );
    assert_eq!(actual.runs.len(), expected.runs.len(), "runs of `{text}`");
    for (a, b) in actual.runs.iter().zip(&expected.runs) {
        assert_eq!(a.font_id, b.font_id, "font of `{text}`");
        assert_eq!(a.glyphs.len(), b.glyphs.len(), "glyph count of `{text}`");
        for (x, y) in a.glyphs.iter().zip(&b.glyphs) {
            assert_eq!(
                (x.id, x.index, x.is_emoji),
                (y.id, y.index, y.is_emoji),
                "glyph of `{text}`"
            );
            assert!(
                close(x.position.x, y.position.x) && close(x.position.y, y.position.y),
                "glyph {} of `{text}`: {:?} vs {:?}",
                x.index,
                x.position,
                y.position
            );
        }
    }
}

/// Words with kerning pairs, ligatures and code ligatures on both sides of every junction.
const WORDS: [&str; 12] = [
    "AVATAR", "office", "Wave", "To", "fiord", "x->y", "a==b", "Lorem", "ipsum", "Yacht", "ffl",
    "T.V.",
];

#[test]
fn appended_words_land_where_full_shaping_puts_them() {
    let f = fixture();
    for family in FAMILIES {
        let mut text = String::from("Lorem ipsum dolor sit amet consectetur adipiscing elit");
        let before = f.system.shape_stats().appended_lines;
        for step in 0..48 {
            text.push(' ');
            text.push_str(WORDS[step % WORDS.len()]);
            let cached = f.cached(&text, family);
            assert_same_up_to_rounding(&cached, &f.platform(&text, family), &text);
        }
        assert!(
            f.system.shape_stats().appended_lines > before,
            "{family}: the growing line reused the line it extends"
        );
    }
}
