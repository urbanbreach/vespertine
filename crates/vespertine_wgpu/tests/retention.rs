//! PRF-014: size-specific shaping on the Linux text backend.
//!
//! The backend has no size-specific shaping object: harfrust shapes in font units and the line
//! layout scales by the font size afterwards. These tests prove that shaping output does not
//! depend on which sizes were shaped before, that every loaded face is wrapped once and kept
//! whatever number of lines and sizes pass through the backend, and that registering another
//! font leaves the existing font ids and their shaping unchanged.

use std::{borrow::Cow, collections::HashSet, ops::Range, process::Command};

use vespertine::{
    FontFallbacks, FontId, FontRun, LineLayout, PlatformTextSystem, RenderGlyphParams, ShapedGlyph,
    font, point, px,
};
use vespertine_wgpu::CosmicTextSystem;

#[path = "support/prf014_workload.rs"]
mod workload;

const IBM_PLEX_SANS: &[u8] =
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");
const LILEX: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf");

const KERNING_AND_LIGATURES: &str = "office affine AVA 1,234.56";
const SIZES: [f32; 3] = [16.0, 24.0, 16.0001];

#[derive(Debug, Clone, PartialEq)]
struct GlyphSnapshot {
    font_id: usize,
    glyph_id: u32,
    index: usize,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct LineSnapshot {
    width: f32,
    glyphs: Vec<GlyphSnapshot>,
}

fn snapshot(layout: &LineLayout) -> LineSnapshot {
    LineSnapshot {
        width: f32::from(layout.width),
        glyphs: layout
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs.iter().map(|glyph| GlyphSnapshot {
                    font_id: run.font_id.0,
                    glyph_id: glyph.id.0,
                    index: glyph.index,
                    x: f32::from(glyph.position.x),
                    y: f32::from(glyph.position.y),
                })
            })
            .collect(),
    }
}

fn shape(system: &CosmicTextSystem, text: &str, size: f32, font_id: FontId) -> LineLayout {
    system.layout_line(
        text,
        px(size),
        &[FontRun {
            len: text.len(),
            font_id,
        }],
    )
}

/// Requests glyph bounds at `size` (window scale factor 2) and rasterizes the glyph when it has
/// ink. Every call builds exactly one scaler from the retained scale context.
fn rasterize(
    system: &CosmicTextSystem,
    font_id: FontId,
    glyph: &ShapedGlyph,
    size: f32,
    bounds_calls: &mut usize,
) {
    let params = RenderGlyphParams {
        font_id,
        glyph_id: glyph.id,
        font_size: px(size),
        subpixel_variant: point(0, 0),
        scale_factor: 2.0,
        is_emoji: false,
        subpixel_rendering: true,
        dilation: 0,
    };
    let bounds = system.glyph_raster_bounds(&params).unwrap();
    *bounds_calls += 1;
    if bounds.size.width.0 == 0 || bounds.size.height.0 == 0 {
        return;
    }
    let (_, pixels) = system.rasterize_glyph(&params, bounds).unwrap();
    assert!(!pixels.is_empty());
}

/// Shapes `labels` labels of every frame in `frames` at the frame's size and rasterizes the
/// first glyph of each label.
fn run_workload(
    system: &CosmicTextSystem,
    font_id: FontId,
    frames: Range<u32>,
    labels: u32,
    bounds_calls: &mut usize,
) {
    for frame in frames {
        let size = workload::font_size(frame);
        for label in 0..labels {
            let text = workload::label_text(frame, label);
            let layout = shape(system, &text, size, font_id);
            let run = layout.runs.first().expect("label shapes to a run");
            rasterize(system, run.font_id, &run.glyphs[0], size, bounds_calls);
        }
    }
}

fn ibm_plex_system() -> (CosmicTextSystem, FontId) {
    let system = CosmicTextSystem::new_without_system_fonts("IBM Plex Sans");
    system
        .add_fonts(vec![Cow::Borrowed(IBM_PLEX_SANS)])
        .unwrap();
    let font_id = system.font_id(&font("IBM Plex Sans")).unwrap();
    (system, font_id)
}

/// Noto Sans as found by the system font database, the font of the PERF-PRF-014 scene.
fn noto_sans_system() -> (CosmicTextSystem, FontId) {
    let system = CosmicTextSystem::new("Noto Sans");
    assert!(
        system
            .all_font_names()
            .iter()
            .any(|name| name == "Noto Sans"),
        "Noto Sans is not installed (`fc-match \"Noto Sans\"`); the PRF-014 tests need it"
    );
    let font_id = system.font_id(&font("Noto Sans")).unwrap();
    (system, font_id)
}

fn assert_close(actual: f32, expected: f32, relative_tolerance: f32, what: &str) {
    let tolerance = relative_tolerance * expected.abs().max(1.0);
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} is not within {relative_tolerance} (relative) of {expected}"
    );
}

/// A1 on one backend and font.
fn check_shaping_is_size_independent(system: &CosmicTextSystem, font_id: FontId) {
    let mut first: [Option<LineSnapshot>; 3] = [None, None, None];
    for _ in 0..5 {
        for (slot, &size) in first.iter_mut().zip(&SIZES) {
            let shaped = snapshot(&shape(system, KERNING_AND_LIGATURES, size, font_id));
            match slot {
                None => *slot = Some(shaped),
                Some(first_at_size) => assert_eq!(
                    *first_at_size, shaped,
                    "result at size {size} changed after other sizes were shaped"
                ),
            }
        }
    }
    let [at_16, at_24, at_16_0001] = first.map(|snapshot| snapshot.unwrap());
    assert!(!at_16.glyphs.is_empty());

    // Shaping is size-independent: same glyph identities and indices at every size.
    for other in [&at_24, &at_16_0001] {
        let ids = |line: &LineSnapshot| {
            line.glyphs
                .iter()
                .map(|glyph| (glyph.font_id, glyph.glyph_id, glyph.index))
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&at_16), ids(other));
    }

    assert_close(
        at_24.width,
        at_16.width * 1.5,
        1e-4,
        "width at 24 vs 1.5 * width at 16",
    );
    for (glyph_24, glyph_16) in at_24.glyphs.iter().zip(&at_16.glyphs) {
        assert_close(
            glyph_24.x,
            glyph_16.x * 1.5,
            1e-4,
            "x at 24 vs 1.5 * x at 16",
        );
    }

    // 16.0001 is a different size: positions follow the exact scale ratio and are not the
    // positions shaped at 16.
    let ratio = 16.0001_f32 / 16.0_f32;
    assert_close(
        at_16_0001.width,
        at_16.width * ratio,
        3e-6,
        "width at 16.0001",
    );
    for (glyph_b, glyph_a) in at_16_0001.glyphs.iter().zip(&at_16.glyphs) {
        assert_close(glyph_b.x, glyph_a.x * ratio, 3e-6, "x at 16.0001 vs 16");
    }
    assert_ne!(at_16_0001.width, at_16.width);
    let last_16 = at_16.glyphs.last().unwrap();
    let last_16_0001 = at_16_0001.glyphs.last().unwrap();
    assert!(last_16.x > 1.0);
    assert_ne!(
        last_16.x, last_16_0001.x,
        "16.0001 must not be served with the 16 result"
    );
}

#[test]
fn a1_shaping_is_size_independent_ibm_plex_sans() {
    let (system, font_id) = ibm_plex_system();
    check_shaping_is_size_independent(&system, font_id);
}

#[test]
fn a1_shaping_is_size_independent_noto_sans() {
    let (system, font_id) = noto_sans_system();
    check_shaping_is_size_independent(&system, font_id);
}

#[test]
fn a4_added_font_keeps_existing_ids_and_shaping() {
    let (system, plex_id) = ibm_plex_system();
    let before = snapshot(&shape(&system, KERNING_AND_LIGATURES, 16.0, plex_id));

    system.add_fonts(vec![Cow::Borrowed(LILEX)]).unwrap();

    // Existing ids still resolve to the same faces and shape the same way.
    assert_eq!(system.font_id(&font("IBM Plex Sans")).unwrap(), plex_id);
    assert_eq!(
        snapshot(&shape(&system, KERNING_AND_LIGATURES, 16.0, plex_id)),
        before
    );

    // The appended font becomes resolvable, behind the existing ids.
    let lilex_id = system.font_id(&font("Lilex")).unwrap();
    assert_ne!(lilex_id, plex_id);
    assert!(
        !shape(&system, KERNING_AND_LIGATURES, 16.0, lilex_id)
            .runs
            .is_empty()
    );
}

#[test]
fn a2_faces_are_built_once_per_distinct_face() {
    let system = CosmicTextSystem::new_without_system_fonts("IBM Plex Sans");
    system
        .add_fonts(vec![Cow::Borrowed(IBM_PLEX_SANS), Cow::Borrowed(LILEX)])
        .unwrap();
    let mut primary = font("IBM Plex Sans");
    primary.fallbacks = Some(FontFallbacks::from_fonts(vec!["Lilex".to_string()]));
    let plex_id = system.font_id(&primary).unwrap();
    let lilex_id = system.font_id(&font("Lilex")).unwrap();
    assert_ne!(plex_id, lilex_id);

    // A character only the fallback face covers, so that both faces take part in shaping.
    let fallback_char = [0x2500..0x2580_u32, 0x2190..0x2200, 0x2580..0x25A0]
        .into_iter()
        .flatten()
        .filter_map(char::from_u32)
        .find(|&ch| {
            system.glyph_for_char(plex_id, ch).is_none()
                && system.glyph_for_char(lilex_id, ch).is_some()
        })
        .expect("a character covered by Lilex but not by IBM Plex Sans");

    let mut lines = HashSet::new();
    let mut used_faces = HashSet::new();
    let mut bounds_calls = 0;
    let mut after_first_line = None;
    for line_ix in 0..120_u32 {
        let text = format!(
            "{} {fallback_char}",
            workload::label_text(line_ix, line_ix % workload::LABEL_COUNT)
        );
        for size in SIZES {
            let layout = shape(&system, &text, size, plex_id);
            for run in &layout.runs {
                used_faces.insert(run.font_id);
                for glyph in run.glyphs.iter().take(3) {
                    rasterize(&system, run.font_id, glyph, size, &mut bounds_calls);
                }
            }
        }
        lines.insert(text);
        after_first_line.get_or_insert_with(|| system.retention_counters());
    }
    assert_eq!(lines.len(), 120);
    assert_eq!(
        used_faces,
        HashSet::from([plex_id, lilex_id]),
        "both faces must take part in shaping"
    );

    let counters = system.retention_counters();
    // One face object per distinct face used, and none created after the first line.
    assert_eq!(counters.face_constructions, used_faces.len());
    assert_eq!(
        counters.face_constructions,
        after_first_line.unwrap().face_constructions
    );
    // Every glyph request builds one scaler from the retained scale context.
    assert!(bounds_calls > 100);
    assert_eq!(counters.scaler_builds, bounds_calls);
}

/// A5, counter part: after warm-up, 100 distinct strings per frame at the sizes of
/// configuration 1 (16, 24, 16.0001 in blocks of ten frames) construct no further face.
#[test]
fn a5_workload_constructs_nothing_after_warmup() {
    let (system, font_id) = noto_sans_system();
    let mut bounds_calls = 0;
    // Thirty frames cover all three sizes of configuration 1.
    run_workload(
        &system,
        font_id,
        0..30,
        workload::LABEL_COUNT,
        &mut bounds_calls,
    );
    let warm = system.retention_counters();
    assert!(warm.face_constructions >= 1);

    run_workload(
        &system,
        font_id,
        30..60,
        workload::LABEL_COUNT,
        &mut bounds_calls,
    );
    let after = system.retention_counters();
    assert_eq!(after.face_constructions, warm.face_constructions);
    assert_eq!(after.scaler_builds, bounds_calls);
}

#[test]
fn workload_strings_are_unique_and_sizes_follow_the_spec() {
    let mut texts = HashSet::new();
    for frame in 0..workload::CONFIG1_FRAMES {
        for label in 0..workload::LABEL_COUNT {
            assert!(texts.insert(workload::label_text(frame, label)));
        }
    }
    assert_eq!(workload::label_text(0, 0), "let let office affine 0");
    assert_eq!(workload::label_text(1, 1), "pub pub office affine 101");

    assert_eq!(workload::font_size(0), 16.0);
    assert_eq!(workload::font_size(9), 16.0);
    assert_eq!(workload::font_size(10), 24.0);
    assert_eq!(workload::font_size(20), 16.0001);
    assert_eq!(workload::font_size(30), 16.0);
    assert_eq!(workload::font_size(719), 16.0001);
    assert_eq!(workload::font_size(720), 12.0);
    assert_eq!(workload::font_size(1819), (12.0_f64 + 0.01 * 1099.0) as f32);
    assert_eq!(workload::font_size(1820), 23.0);
    assert_eq!(workload::font_size(1919), 23.0);
}

/// R4d: cosmic-text's whole-run shaping cache (`shape-run-cache`) must stay disabled; recently
/// shaped lines are retained by PRF-012 and a second store would duplicate it.
#[test]
#[allow(
    clippy::disallowed_methods,
    reason = "a synchronous test may block on `cargo tree`"
)]
fn r4d_shape_run_cache_stays_disabled() {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "tree",
            "--offline",
            "--workspace",
            "-e",
            "features",
            "-i",
            "cosmic-text",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run `cargo tree`");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("cosmic-text"),
        "unexpected cargo tree output:\n{stdout}"
    );
    assert!(
        !stdout.contains("\"shape-run-cache\""),
        "cosmic-text's shape-run-cache feature is enabled:\n{stdout}"
    );
}
