//! The native patch viewer through the production offscreen host (CMP-015 A2, A3, A5, A6, A9).

#[path = "support/diff_rig.rs"]
mod diff_rig;

use diff_rig::{Config, Rig, many_files};
use std::sync::Arc;
use vespertine::{ListOffset, px, rgb};
use vespertine_components::diff::model::parse_count;
use vespertine_components::diff::rows::Row;
use vespertine_test_host::require_backend;

fn anchor(rig: &mut Rig) -> (usize, f32) {
    rig.read(|state| {
        let top = state.list_state().logical_scroll_top();
        (top.item_ix, f32::from(top.offset_in_item))
    })
}

fn scroll_to(rig: &mut Rig, row: usize, offset: f32) {
    rig.update(|state, _| {
        state.list_state().scroll_to(ListOffset {
            item_ix: row,
            offset_in_item: px(offset),
        })
    });
}

#[test]
fn a2_live_updates_keep_the_logical_anchor_clamp_it_and_survive_color_only_theme_changes() {
    require_backend!();
    let patch = many_files(40, 2, 20);
    let mut rig = Rig::new(Config::new(patch.clone(), 400., 300.), (400., 300.));
    let count = rig.read(|state| state.flat().rows.len());
    assert_eq!(count, 40 * (1 + 2 * (1 + 40) + 1));
    scroll_to(&mut rig, 120, 7.);
    assert_eq!(anchor(&mut rig).0, 120);
    let offset = anchor(&mut rig).1;
    assert!((offset - 7.).abs() < 0.5, "{offset}");

    // A live patch update with an unchanged row count and changed row metrics: the logical
    // anchor (row index and offset) is restored after the heights are reset.
    let updated = patch.replace("let old_", "let renamed_");
    let mut theme = (*rig.config.borrow().theme).clone();
    theme.metrics.diff_line_height = 24.;
    rig.set(|config| {
        config.patch = updated.clone();
        config.theme = Arc::new(theme);
    });
    assert_eq!(rig.read(|state| state.flat().rows.len()), count);
    let (row, offset) = anchor(&mut rig);
    assert_eq!(row, 120);
    assert!((offset - 7.).abs() < 0.5, "{offset}");

    // A color-only theme change keeps the scroll position and never reparses.
    let parses = parse_count();
    let mut recolored = (*rig.config.borrow().theme).clone();
    recolored.diff_add = rgb(0x44ddff);
    recolored.text = rgb(0xffeecc);
    recolored.bg = rgb(0x202030);
    rig.set(|config| config.theme = Arc::new(recolored));
    assert_eq!(parse_count(), parses, "no reparse for a color-only change");
    assert_eq!(anchor(&mut rig).0, 120);
    assert_eq!(rig.read(|state| state.flat().rows.len()), count);

    // The row count shrinks: the anchor is clamped to the new row count minus one.
    let small = many_files(2, 1, 3);
    let small_count = {
        rig.set(|config| config.patch = small.clone());
        rig.read(|state| state.flat().rows.len())
    };
    assert!(small_count < 120);
    let (row, _) = anchor(&mut rig);
    assert!(row < small_count, "clamped to {small_count}: {row}");
}

#[test]
fn a2_only_the_visible_slice_plus_overdraw_is_built() {
    require_backend!();
    let mut rig = Rig::new(Config::new(many_files(60, 1, 30), 400., 300.), (400., 300.));
    let rows = rig.read(|state| state.flat().rows.len());
    assert!(rows > 3000);
    let built = rig
        .host
        .painted_text()
        .iter()
        .filter(|text| text.starts_with("let "))
        .count();
    // (300 + 1024) px of 21 px lines.
    assert!(
        built > 10 && built < 100,
        "{built} code lines built of {rows} rows"
    );
    scroll_to(&mut rig, 2000, 0.);
    let first = rig
        .host
        .painted_text()
        .into_iter()
        .find(|text| text.starts_with("let "))
        .expect("code painted after scrolling");
    assert!(first.starts_with("let "), "{first}");
}

#[test]
fn a2_a_rerender_keeps_the_scroll_position() {
    require_backend!();
    let mut rig = Rig::new(Config::new(many_files(20, 1, 30), 400., 300.), (400., 300.));
    scroll_to(&mut rig, 300, 3.);
    let before = anchor(&mut rig);
    rig.refresh();
    rig.refresh();
    assert_eq!(anchor(&mut rig).0, before.0);
    assert!(matches!(
        rig.read(|state| state.flat().rows[0]),
        Row::FileHeader { file: 0 }
    ));
}

const SAMPLE: &str = "diff --git a/sample.txt b/sample.txt\n--- a/sample.txt\n+++ b/sample.txt\n@@ -1,2 +1,3 @@\n ctx line\n-old line\n+new line\n+extra line\n";

fn sample_rig() -> Rig {
    let mut config = Config::new(SAMPLE, 400., 300.);
    config.scroll = false;
    Rig::new(config, (400., 300.))
}

#[test]
fn a3_gutters_markers_headers_and_show_more_are_chrome_while_line_text_is_content() {
    require_backend!();
    let mut config = Config::new(SAMPLE, 400., 300.);
    config.scroll = false;
    config.max_lines = Some(3);
    config.search = Some("line".into());
    let mut rig = Rig::new(config, (400., 300.));
    let service = rig.service();
    let painted = service.painted_text();
    let chrome: Vec<&str> = painted
        .iter()
        .filter(|entry| entry.chrome)
        .map(|entry| entry.text.as_str())
        .collect();
    let content: Vec<&str> = painted
        .iter()
        .filter(|entry| !entry.chrome)
        .map(|entry| entry.text.as_str())
        .collect();
    assert_eq!(content, ["ctx line", "old line", "new line"]);
    for expected in [
        "sample.txt",
        "+2",
        "\u{2212}1",
        "@@ -1,2 +1,3 @@",
        "\u{b7}",
        "\u{2212}",
        "+",
        "Show 1 more line",
    ] {
        assert!(
            chrome.contains(&expected),
            "{expected:?} is chrome: {chrome:?}"
        );
    }
    assert_eq!(
        service.registered_runs(),
        3,
        "only the code lines are selectable runs"
    );
    // Search washes match line text only; the show-more label also contains "line".
    let washes = service.painted_highlights();
    assert_eq!(washes.len(), 3, "{washes:?}");
    assert!(
        washes
            .iter()
            .all(|wash| content.contains(&wash.text.as_str()))
    );

    // A query that only the chrome contains finds nothing.
    rig.set(|config| config.search = Some("1".into()));
    assert!(rig.service().painted_highlights().is_empty());
}

#[test]
fn a3_dragging_across_rows_selects_line_text_only() {
    require_backend!();
    let mut rig = sample_rig();
    // Rows: header 0-36, hunk 36-64, context 64-85, deleted 85-106, added 106-127.
    rig.host.mouse_down(116., 70., 0, "").unwrap();
    rig.host.paint_frame();
    rig.host.mouse_move(300., 115., Some(0), "").unwrap();
    rig.host.paint_frame();
    rig.host.mouse_up(300., 115., 0, "").unwrap();
    rig.host.paint_frame();
    let selected = rig.service().selected_text().expect("a selection");
    assert!(selected.contains("ctx line"), "{selected:?}");
    assert!(selected.contains("old line"), "{selected:?}");
    assert!(
        !selected.chars().any(|c| c.is_ascii_digit()) && !selected.contains('@'),
        "gutters, markers and headers stay out: {selected:?}"
    );
}

#[test]
fn a3_row_colors_follow_the_specified_tints_and_lanes() {
    require_backend!();
    let mut config = Config::new(SAMPLE, 400., 300.);
    config.scroll = false;
    let mut theme = (*config.theme).clone();
    theme.bg = rgb(0x101010);
    theme.diff_add = rgb(0x00cc88);
    theme.diff_del = rgb(0xff5555);
    config.theme = Arc::new(theme);
    let mut rig = Rig::new(config, (400., 300.));
    let image = rig.host.screenshot_image().unwrap();
    let px = |x: f32, y: f32| diff_rig::image_pixel(&image, x, y);
    // Deleted row (y 85..106): tint = #ff5555 at alpha 0.055 over #101010; lane alpha 0.55.
    let blend_over = |fg: [f32; 3], alpha: f32, bg: [f32; 3]| -> [f32; 3] {
        [0, 1, 2].map(|i| fg[i] * alpha + bg[i] * (1. - alpha))
    };
    let blend = |fg: [f32; 3], alpha: f32| -> [u8; 3] {
        blend_over(fg, alpha, [16., 16., 16.]).map(|v| v.round() as u8)
    };
    // The lane is painted over the row tint.
    let lane_over_tint = |fg: [f32; 3]| -> [u8; 3] {
        let tint = blend_over(fg, 0.055, [16., 16., 16.]);
        blend_over(fg, 0.55, tint).map(|v| v.round() as u8)
    };
    let close = |actual: [u8; 4], expected: [u8; 3]| {
        actual[..3]
            .iter()
            .zip(expected)
            .all(|(a, b)| a.abs_diff(b) <= 2)
    };
    let tint = blend([255., 85., 85.], 0.055);
    assert!(
        close(px(300., 95.), tint),
        "{:?} vs {tint:?}",
        px(300., 95.)
    );
    let lane = lane_over_tint([255., 85., 85.]);
    assert!(close(px(1., 95.), lane), "{:?} vs {lane:?}", px(1., 95.));
    let added_tint = blend([0., 204., 136.], 0.055);
    assert!(close(px(300., 116.), added_tint));
    let added_lane = lane_over_tint([0., 204., 136.]);
    assert!(close(px(1., 116.), added_lane));
    // Context rows have no tint and an invisible lane.
    assert!(close(px(1., 75.), [16, 16, 16]));
    assert!(close(px(300., 75.), [16, 16, 16]));
}

#[test]
fn r19_events_are_wired_when_subscribed_and_never_change_controlled_properties() {
    require_backend!();
    let mut config = Config::new(SAMPLE, 400., 300.);
    config.scroll = false;
    config.collapsed = Some(Vec::new());
    config.max_lines = Some(2);
    let mut rig = Rig::new(config, (400., 300.));
    let rows_before = rig.read(|state| state.flat().rows.len());
    // Header click: toggleFile(path); the owner decides, so nothing changes.
    rig.host.click(100., 10., 0, "").unwrap();
    assert_eq!(rig.events(), ["toggle sample.txt"]);
    rig.refresh();
    assert_eq!(rig.read(|state| state.flat().rows.len()), rows_before);
    assert!(rig.read(|state| state.collapsed_paths()).is_empty());
    // Line click: text and the numbers present.
    rig.host.click(200., 75., 0, "").unwrap();
    rig.host.click(200., 96., 0, "").unwrap();
    let events = rig.events();
    assert_eq!(
        events[1],
        "line ctx line Some(1) Some(1) Some(SourceRef { side: New, line: 1 })"
    );
    assert_eq!(
        events[2],
        "line old line Some(2) None Some(SourceRef { side: Old, line: 2 })"
    );
    // Show more: the remaining count (2 lines left after the cap of 2).
    let more_y = 36. + 28. + 2. * 21. + 18.;
    rig.host.click(200., more_y, 0, "").unwrap();
    assert_eq!(rig.events().last().map(String::as_str), Some("more 2"));
    assert_eq!(rig.read(|state| state.flat().rows.len()), rows_before);

    // Without subscriptions nothing is reported; an uncontrolled viewer tracks its own folds.
    let mut config = Config::new(SAMPLE, 400., 300.);
    config.scroll = false;
    config.subscribe = false;
    let mut quiet = Rig::new(config, (400., 300.));
    quiet.host.click(100., 10., 0, "").unwrap();
    quiet.refresh();
    assert!(quiet.events().is_empty());
    assert_eq!(quiet.read(|state| state.collapsed_paths()), ["sample.txt"]);
}

#[test]
fn r6_empty_patches_show_no_changes_and_a_collapsed_file_removes_its_body() {
    require_backend!();
    let mut config = Config::new("", 400., 300.);
    config.scroll = false;
    let mut rig = Rig::new(config, (400., 300.));
    assert!(rig.host.painted_text().iter().any(|t| t == "No changes"));
    rig.set(|config| config.patch = SAMPLE.to_owned());
    assert!(!rig.host.painted_text().iter().any(|t| t == "No changes"));
    rig.set(|config| config.collapsed = Some(vec!["sample.txt".into()]));
    // The fold tweens for 180 ms and settles after 400 ms (R26).
    rig.host.motion_pause();
    rig.host.motion_fast_forward(500.).unwrap();
    rig.refresh();
    let painted = rig.host.painted_text();
    assert!(painted.iter().any(|t| t == "sample.txt"));
    assert!(
        !painted.iter().any(|t| t.contains("ctx line")),
        "{painted:?}"
    );
    assert_eq!(rig.read(|state| state.flat().rows.len()), 1);
}

#[test]
fn a1_huge_hunk_numbers_render_even_though_that_side_is_not_highlighted() {
    require_backend!();
    let patch =
        "diff --git a/a.rs b/a.rs\n@@ -3000000000,1 +1,1 @@\n-let old = 1;\n+let new = 2;\n";
    let mut config = Config::new(patch, 400., 300.);
    config.scroll = false;
    let mut rig = Rig::new(config, (400., 300.));
    let painted = rig.host.painted_text();
    assert!(painted.iter().any(|t| t == "let old = 1;"));
    assert!(painted.iter().any(|t| t == "let new = 2;"));
    assert!(painted.iter().any(|t| t == "3000000000"), "{painted:?}");
    let highlights = rig.read(|state| {
        let model = state.model().clone();
        (
            model.highlights(0).old_side().is_highlighted(),
            model.highlights(0).new_side().is_highlighted(),
        )
    });
    assert_eq!(highlights, (false, true));
}

// --- A9: wrap, horizontal scroll ---

fn long_line_patch() -> String {
    let long = "x".repeat(300);
    format!(
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n short\n-{long}\n+{long}!\n tail\ndiff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1000,2 +1000,2 @@\n -b1\n+b2\n tail\n"
    )
}

fn row_height_of(rig: &mut Rig, ix: usize) -> f32 {
    rig.read(|state| {
        state
            .list_state()
            .bounds_for_item(ix)
            .map(|bounds| f32::from(bounds.size.height))
            .unwrap_or(f32::NAN)
    })
}

#[test]
fn a9_wrapped_rows_get_measured_heights_and_unwrapped_rows_are_exactly_one_line() {
    require_backend!();
    let mut config = Config::new(long_line_patch(), 400., 400.);
    config.scroll = true;
    let mut rig = Rig::new(config, (400., 400.));
    let rows = rig.read(|state| state.flat().rows.clone());
    let long_rows: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            matches!(
                row,
                Row::Line {
                    file: 0,
                    line: 1 | 2,
                    ..
                }
            )
        })
        .map(|(ix, _)| ix)
        .collect();
    assert_eq!(long_rows.len(), 2);
    for ix in 0..8 {
        if matches!(rows[ix], Row::Line { .. }) {
            assert_eq!(row_height_of(&mut rig, ix), 21., "unwrapped row {ix}");
        }
    }
    rig.set(|config| config.wrap = true);
    let rows = rig.read(|state| state.flat().rows.clone());
    let short = rows
        .iter()
        .position(|row| {
            matches!(
                row,
                Row::Line {
                    file: 0,
                    line: 0,
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(
        row_height_of(&mut rig, short),
        21.,
        "short rows keep the minimum"
    );
    for ix in &long_rows {
        let height = row_height_of(&mut rig, *ix);
        assert!(
            height > 40. && (height / 21.).fract().abs() < 1e-3,
            "wrapped row {ix}: {height}"
        );
    }
}

#[test]
fn a9_each_file_pans_independently_with_the_specified_content_widths_and_switches_reset_it() {
    require_backend!();
    let mut config = Config::new(long_line_patch(), 500., 400.);
    config.scroll = false;
    let mut rig = Rig::new(config, (500., 400.));
    let (g0, g1) = rig.read(|state| (state.model().gutters[0], state.model().gutters[1]));
    assert_eq!(g0, 36.);
    assert!((g1 - 40.4).abs() < 1e-3);
    // Unified: widest + 12 + 24 + 2 x (largest gutter - this gutter).
    let advance = rig.with_window(|state, window| {
        let (size, _) = state.text_metrics().unwrap();
        let system = window.text_system().clone();
        let font = vespertine::font("Liberation Mono");
        f32::from(
            system
                .em_advance(system.resolve_font(&font), px(size))
                .unwrap(),
        )
    });
    let (w0, w1) = rig.with_window(|state, window| {
        (
            state.content_width(0, window),
            state.content_width(1, window),
        )
    });
    let widest0 = 301. * advance; // the added line is 300 x plus "!"
    assert!(
        (w0 - (widest0 + 12. + 24. + 2. * (g1 - g0))).abs() < 1.5,
        "{w0} {widest0}"
    );
    assert!(w1 >= 12. + 24. + 3. * advance - 1., "{w1}");
    // Wheel over file 0's code pans file 0 only; gutters stay (the offset is per file).
    let y = 36. + 28. + 21. * 2. + 10.;
    rig.host.scroll_wheel(300., y, -400., 0., "").unwrap();
    rig.refresh();
    let (o0, o1) = rig.read(|state| (state.horizontal_offset(0), state.horizontal_offset(1)));
    assert!((o0 - 400.).abs() < 0.5, "{o0}");
    assert_eq!(o1, 0.);
    let max = rig.with_window(|state, window| state.max_horizontal_offset(0, window));
    rig.host.scroll_wheel(300., y, -100_000., 0., "").unwrap();
    rig.refresh();
    assert!((rig.read(|state| state.horizontal_offset(0)) - max).abs() < 0.5);
    // Vertical wheel never pans.
    rig.set(|config| config.wrap = false);
    let before = rig.read(|state| state.horizontal_offset(0));
    rig.host.scroll_wheel(300., y, 0., -30., "").unwrap();
    assert_eq!(rig.read(|state| state.horizontal_offset(0)), before);

    // Split: widest + 6 + 24 + (largest gutter - this gutter).
    rig.set(|config| config.mode = vespertine_components::diff::rows::DiffMode::Split);
    assert_eq!(
        rig.read(|state| state.horizontal_offset(0)),
        0.,
        "mode switch resets"
    );
    let split0 = rig.with_window(|state, window| state.content_width(0, window));
    assert!(
        (split0 - (widest0 + 6. + 24. + (g1 - g0))).abs() < 1.5,
        "{split0}"
    );
    // Pan in split (a code half), then toggle wrap: reset again.
    let y_split = 36. + 28. + 21. + 10.;
    rig.host.scroll_wheel(150., y_split, -200., 0., "").unwrap();
    rig.refresh();
    assert!(rig.read(|state| state.horizontal_offset(0)) > 0.);
    rig.set(|config| config.wrap = true);
    assert_eq!(
        rig.read(|state| state.horizontal_offset(0)),
        0.,
        "wrap switch resets"
    );
}

#[test]
fn a9_replacing_the_patch_or_widening_the_viewer_never_leaves_code_panned_past_its_end() {
    require_backend!();
    let mut config = Config::new(long_line_patch(), 500., 400.);
    config.scroll = false;
    let mut rig = Rig::new(config, (500., 400.));
    let y = 36. + 28. + 21. * 2. + 10.;
    rig.host.scroll_wheel(300., y, -100_000., 0., "").unwrap();
    rig.refresh();
    assert!(rig.read(|state| state.horizontal_offset(0)) > 1000.);
    // A wider viewer shows more of the line: the offset shrinks to the new maximum.
    rig.set(|config| config.width = 1500.);
    let max = rig.with_window(|state, window| state.max_horizontal_offset(0, window));
    assert!(max > 0.);
    assert!(
        (rig.read(|state| state.horizontal_offset(0)) - max).abs() < 0.5,
        "clamped to {max}"
    );
    // A new patch of short lines starts unpanned.
    rig.set(|config| {
        config.patch = "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-alpha\n+beta\n".to_owned()
    });
    assert_eq!(rig.read(|state| state.horizontal_offset(0)), 0.);
}

// --- R23: text size ---

#[test]
fn r23_the_owner_scaled_code_size_scales_the_row_boxes() {
    require_backend!();
    let patch = "diff --git a/a b/a\n@@ -1 +1 @@\n-x\n+y\n";
    let mut rig = Rig::new(Config::new(patch, 400., 300.), (400., 300.));
    assert_eq!(row_height_of(&mut rig, 2), 21.);
    assert_eq!(rig.read(|state| state.text_metrics()), Some((12., 21.)));
}

// --- A5: sticky headers ---

#[test]
fn a5_sticky_header_follows_the_first_visible_row_and_is_pushed_by_the_next_header() {
    require_backend!();
    let mut config = Config::new(many_files(4, 1, 10), 400., 300.);
    config.sticky = true;
    let mut rig = Rig::new(config, (400., 300.));
    assert_eq!(
        rig.read(|state| state.sticky()),
        None,
        "own header, not scrolled"
    );
    // Inside file 0.
    scroll_to(&mut rig, 5, 0.);
    assert_eq!(rig.read(|state| state.sticky()), Some((0, 0.)));
    // The header itself scrolled partly past.
    scroll_to(&mut rig, 0, 10.);
    assert_eq!(rig.read(|state| state.sticky()), Some((0, 0.)));
    // File 1's header 20 px from the viewport top pushes the sticky header up by 16.
    let header1 = rig.read(|state| state.flat().files[1].range.start);
    scroll_to(&mut rig, header1 - 1, 0.);
    let bottom_pad_height = 8.;
    scroll_to(&mut rig, header1 - 1, bottom_pad_height - 20.);
    let sticky = rig.read(|state| state.sticky()).expect("sticky");
    assert_eq!(sticky.0, 0);
    assert!((sticky.1 - -16.).abs() < 0.5, "{sticky:?}");
    // With the header row scrolled out of view the overlay is what paints its path.
    scroll_to(&mut rig, 8, 0.);
    let count = |rig: &mut Rig| {
        rig.host
            .painted_text()
            .iter()
            .filter(|text| text.as_str() == "src/file0.rs")
            .count()
    };
    assert_eq!(count(&mut rig), 1, "the sticky overlay");
    rig.set(|config| config.sticky = false);
    assert_eq!(count(&mut rig), 0, "no overlay without sticky headers");
}

#[test]
fn a5_sticky_frost_uses_a_backdrop_blur_and_opaque_themes_flatten() {
    require_backend!();
    let mut config = Config::new(many_files(3, 1, 10), 400., 300.);
    config.sticky = true;
    config.frosted = true;
    let mut rig = Rig::new(config, (400., 300.));
    scroll_to(&mut rig, 6, 0.);
    let frosted = rig.host.screenshot_image().unwrap();
    rig.set(|config| config.frosted = false);
    let flat = rig.host.screenshot_image().unwrap();
    // Rows of alternating deleted and added lines scroll under the header: the frost shows them
    // through the blur, the opaque flatten hides them behind the background.
    let differs = (0..80).any(|i| {
        let (x, y) = (100. + i as f32 * 3., 24.);
        diff_rig::image_pixel(&frosted, x, y) != diff_rig::image_pixel(&flat, x, y)
    });
    assert!(differs, "frosting must differ from the flat fill");
}

// --- R26: folding ---

fn stand_in_height(rig: &mut Rig, file: usize) -> Option<f32> {
    let row = rig.read(|state| {
        state
            .flat()
            .rows
            .iter()
            .position(|row| matches!(row, Row::StandIn { file: f, .. } if *f == file))
    })?;
    Some(row_height_of(rig, row))
}

#[test]
fn r26_a_fold_tweens_the_body_height_for_180ms_and_settles_after_400ms() {
    require_backend!();
    let mut rig = Rig::new(Config::new(many_files(2, 1, 10), 400., 300.), (400., 300.));
    let body = 28. + 20. * 21. + 8.;
    rig.host.motion_set(1000.).unwrap();
    rig.refresh();
    // Collapse: the header activation toggles the viewer's own tracking and starts the tween.
    rig.host.click(100., 10., 0, "").unwrap();
    rig.refresh();
    assert_eq!(rig.read(|state| state.folding_files()), [0]);
    assert!(
        (stand_in_height(&mut rig, 0).unwrap() - body).abs() < 0.5,
        "starts at the old height"
    );
    rig.host.motion_set(1090.).unwrap();
    rig.refresh();
    let middle = stand_in_height(&mut rig, 0).unwrap();
    assert!(
        middle > 5. && middle < body - 5.,
        "mid-tween height {middle}"
    );
    rig.host.motion_set(1180.).unwrap();
    rig.refresh();
    assert!(
        stand_in_height(&mut rig, 0).unwrap() < 0.5,
        "ends at the new height"
    );
    // The stand-in only settles after the arming window.
    rig.host.motion_set(1300.).unwrap();
    rig.refresh();
    assert_eq!(rig.read(|state| state.folding_files()), [0]);
    rig.host.motion_set(1450.).unwrap();
    rig.refresh();
    assert!(rig.read(|state| state.folding_files()).is_empty());
    let rows = rig.read(|state| state.flat().rows.clone());
    assert_eq!(rows[0], Row::FileHeader { file: 0 });
    assert_eq!(
        rows[1],
        Row::FileHeader { file: 1 },
        "the body rows are removed, not hidden"
    );

    // Expand: the body grows from nothing to the analytic height.
    rig.host.motion_set(2000.).unwrap();
    rig.host.click(100., 10., 0, "").unwrap();
    rig.refresh();
    assert!(stand_in_height(&mut rig, 0).unwrap() < 0.5);
    rig.host.motion_set(2090.).unwrap();
    rig.refresh();
    let middle = stand_in_height(&mut rig, 0).unwrap();
    assert!(middle > 5. && middle < body - 5., "{middle}");
    rig.host.motion_set(2500.).unwrap();
    rig.refresh();
    assert!(rig.read(|state| state.folding_files()).is_empty());
    assert!(rig.read(|state| state.flat().rows.len()) > 40);
}

#[test]
fn r26_wrapped_sections_settle_immediately_and_a_wrap_switch_settles_a_running_tween() {
    require_backend!();
    let mut config = Config::new(many_files(2, 1, 10), 400., 300.);
    config.wrap = true;
    let mut rig = Rig::new(config, (400., 300.));
    rig.host.motion_set(1000.).unwrap();
    rig.host.click(100., 10., 0, "").unwrap();
    rig.refresh();
    assert!(
        rig.read(|state| state.folding_files()).is_empty(),
        "no tween when wrapped"
    );
    assert_eq!(
        rig.read(|state| state.flat().rows[1]),
        Row::FileHeader { file: 1 }
    );
    // Unwrapped: expand starts a tween; switching wrap settles it first.
    rig.set(|config| config.wrap = false);
    rig.host.click(100., 10., 0, "").unwrap();
    rig.refresh();
    assert_eq!(rig.read(|state| state.folding_files()), [0]);
    rig.set(|config| config.wrap = true);
    assert!(rig.read(|state| state.folding_files()).is_empty());
    assert!(
        rig.read(|state| state.flat().rows.len()) > 40,
        "steady rows"
    );
}

#[test]
fn r26_collapse_all_is_a_steady_write_that_keeps_the_nearest_header() {
    require_backend!();
    let mut rig = Rig::new(Config::new(many_files(30, 1, 2), 400., 300.), (400., 300.));
    let header10 = rig.read(|state| state.flat().files[10].range.start);
    scroll_to(&mut rig, header10 + 4, 3.);
    let collapsed = rig.update(|state, cx| state.collapse_all(cx));
    assert_eq!(collapsed.len(), 30);
    assert!(
        rig.read(|state| state.folding_files()).is_empty(),
        "no tweens"
    );
    assert_eq!(rig.read(|state| state.flat().rows.len()), 30);
    assert_eq!(
        anchor(&mut rig).0,
        10,
        "the anchor sits on the nearest header"
    );
    // All collapsed: the next collapse-all expands every file.
    let collapsed = rig.update(|state, cx| state.collapse_all(cx));
    assert!(collapsed.is_empty());
    assert!(rig.read(|state| state.flat().rows.len()) > 30 * 6);
    assert!(rig.read(|state| state.collapsed_paths()).is_empty());
}

// --- R13: word washes ---

#[test]
fn r13_the_word_wash_is_painted_under_the_changed_token_only_when_enabled() {
    require_backend!();
    let patch = "diff --git a/a b/a\n@@ -1 +1 @@\n-let value = old;\n+let value = new;\n";
    let mut config = Config::new(patch, 400., 300.);
    config.scroll = false;
    config.syntax = false;
    let mut rig = Rig::new(config, (400., 300.));
    let off = rig.host.screenshot_image().unwrap();
    rig.set(|config| config.word_diff = true);
    let on = rig.host.screenshot_image().unwrap();
    // Text starts at 3 + 36 + 36 + 28 + 12 = 115; "old" begins at column 12, 7.2 px each.
    let x = 115. + 12. * 7.2 + 3.6;
    let y = 36. + 28. + 4.;
    let before = diff_rig::image_pixel(&off, x, y);
    let after = diff_rig::image_pixel(&on, x, y);
    assert_ne!(before, after, "the wash tints the changed token");
    // Unchanged text is untouched.
    let unchanged = (115. + 2. * 7.2, y);
    assert_eq!(
        diff_rig::image_pixel(&off, unchanged.0, unchanged.1),
        diff_rig::image_pixel(&on, unchanged.0, unchanged.1)
    );
}

// --- R20-R22: split presentation ---

#[test]
fn a8_a4_split_halves_are_equal_with_a_divider_and_fillers_where_a_side_is_empty() {
    require_backend!();
    let mut config = Config::new(SAMPLE, 401., 300.);
    config.scroll = false;
    config.mode = vespertine_components::diff::rows::DiffMode::Split;
    let mut theme = (*config.theme).clone();
    theme.bg = rgb(0x101010);
    config.theme = Arc::new(theme);
    let mut rig = Rig::new(config, (401., 300.));
    let image = rig.host.screenshot_image().unwrap();
    let px = |x: f32, y: f32| diff_rig::image_pixel(&image, x, y);
    // Rows: header 0-36, hunk 36-64, context 64-85, pair(-old,+new) 85-106, one-sided
    // (+extra) 106-127. The divider sits at x = 200 (halves of (401 - 1) / 2 = 200).
    assert_eq!(
        &px(200.5, 75.)[..3],
        &blend_white(0.06, 16)[..],
        "divider ink 0.06"
    );
    // The left half of the last row is a flat filler of ink at alpha 0.03.
    assert_eq!(
        &px(100., 116.)[..3],
        &blend_white(0.03, 16)[..],
        "filler ink 0.03"
    );
    // The same text shows on both sides of a context row.
    let texts = rig.host.painted_text();
    assert_eq!(texts.iter().filter(|t| t.as_str() == "ctx line").count(), 2);
    assert_eq!(texts.iter().filter(|t| t.as_str() == "old line").count(), 1);
    assert_eq!(texts.iter().filter(|t| t.as_str() == "new line").count(), 1);
    // Split rows pair -old with +new.
    let rows = rig.read(|state| state.flat().rows.clone());
    assert!(rows.iter().any(|row| matches!(
        row,
        Row::Split {
            left: Some(1),
            right: Some(2),
            ..
        }
    )));
    assert!(rows.iter().any(|row| matches!(
        row,
        Row::Split {
            left: None,
            right: Some(3),
            ..
        }
    )));
}

#[test]
fn r19_split_line_clicks_report_the_line_of_the_clicked_half() {
    require_backend!();
    let mut config = Config::new(SAMPLE, 401., 300.);
    config.scroll = false;
    config.mode = vespertine_components::diff::rows::DiffMode::Split;
    let mut rig = Rig::new(config, (401., 300.));
    // The paired row (-old line, +new line) spans y 85-106; the divider sits at x = 200.
    rig.host.click(100., 95., 0, "").unwrap();
    rig.host.click(300., 95., 0, "").unwrap();
    assert_eq!(
        rig.events(),
        [
            "line old line Some(2) None Some(SourceRef { side: Old, line: 2 })",
            "line new line None Some(2) Some(SourceRef { side: New, line: 2 })",
        ]
    );
}

fn blend_white(alpha: f32, background: u8) -> [u8; 3] {
    let value = (255. * alpha + f32::from(background) * (1. - alpha)).round() as u8;
    [value; 3]
}

// --- R24: stable anchors across mode switches ---

#[test]
fn r24_switching_modes_keeps_the_logical_source_line_in_view() {
    require_backend!();
    let mut rig = Rig::new(Config::new(many_files(3, 1, 30), 600., 300.), (600., 300.));
    // Scroll to the deleted line "old_1_0_20" of file 1: find its row.
    let target = rig.read(|state| {
        state
            .flat()
            .rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    Row::Line {
                        file: 1,
                        line: 40,
                        ..
                    }
                )
            })
            .unwrap()
    });
    scroll_to(&mut rig, target, 0.);
    rig.set(|config| config.mode = vespertine_components::diff::rows::DiffMode::Split);
    let row = anchor(&mut rig).0;
    let flat = rig.read(|state| state.flat().rows.clone());
    match flat[row] {
        Row::Split {
            file: 1,
            hunk: 0,
            left,
            right,
        } => {
            assert!(
                left == Some(40) || right == Some(40) || left == Some(41) || right == Some(41),
                "{left:?} {right:?}"
            )
        }
        other => panic!("anchor moved to {other:?}"),
    }
    rig.set(|config| config.mode = vespertine_components::diff::rows::DiffMode::Unified);
    let row = anchor(&mut rig).0;
    let flat = rig.read(|state| state.flat().rows.clone());
    assert!(
        matches!(flat[row], Row::Line { file: 1, .. }),
        "{:?}",
        flat[row]
    );
}

// --- R27-R29, A6: annotations and the draft ---

use vespertine_components::diff::rows::{Annotation, DiffMode, Side};

fn notes() -> Vec<Annotation> {
    vec![
        Annotation {
            id: "n1".into(),
            path: "sample.txt".into(),
            side: Side::Old,
            line: 2,
            body: "old note".into(),
        },
        Annotation {
            id: "n2".into(),
            path: "sample.txt".into(),
            side: Side::New,
            line: 2,
            body: "x".repeat(130),
        },
    ]
}

fn annotated_rig(mode: DiffMode) -> Rig {
    let mut config = Config::new(SAMPLE, 400., 400.);
    config.scroll = false;
    config.editor = true;
    config.annotations = notes();
    config.mode = mode;
    Rig::new(config, (400., 400.))
}

#[test]
fn a7_cards_follow_their_rows_and_have_the_analytic_height() {
    require_backend!();
    let mut config = Config::new(SAMPLE, 400., 400.);
    config.editor = true;
    config.annotations = notes();
    let mut rig = Rig::new(config, (400., 400.));
    let rows = rig.read(|state| state.flat().rows.clone());
    let kinds: Vec<&str> = rows
        .iter()
        .map(|row| match row {
            Row::Card { annotation: 0, .. } => "card-old",
            Row::Card { annotation: 1, .. } => "card-new",
            Row::Line { .. } => "line",
            _ => "other",
        })
        .collect();
    // ctx, -old, [old note], +new, [new note], +extra
    assert_eq!(
        &kinds[2..8],
        ["line", "line", "card-old", "line", "card-new", "line"]
    );
    let ix = |card: usize| {
        rows.iter()
            .position(|row| matches!(row, Row::Card { annotation, .. } if *annotation == card))
            .unwrap()
    };
    // 20 + 22 + 18 x lines + 6: one line, and 130 characters = 3 estimated lines of 64.
    assert_eq!(row_height_of(&mut rig, ix(0)), 20. + 22. + 18. + 6.);
    assert_eq!(row_height_of(&mut rig, ix(1)), 20. + 22. + 18. * 3. + 6.);
    let texts = rig.host.painted_text();
    assert!(texts.iter().any(|t| t == "sample.txt:2"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "old note"));
}

fn hover_then_click(rig: &mut Rig, x: f64, y: f64) {
    rig.host.mouse_move(x, y, None, "").unwrap();
    rig.refresh();
    rig.host.click(x, y, 0, "").unwrap();
    rig.refresh();
}

#[test]
fn a6_the_add_affordance_opens_a_focused_draft_and_escape_cancels_it_without_activating_code() {
    require_backend!();
    let mut rig = annotated_rig(DiffMode::Unified);
    // The deleted line row (y 85..106): the affordance sits in the OLD gutter,
    // left = 3 + (36 - 16) / 2 = 13, 16 wide.
    hover_then_click(&mut rig, 21., 95.);
    assert!(
        rig.events().iter().all(|e| !e.starts_with("line")),
        "{:?}",
        rig.events()
    );
    let draft = rig
        .read(|state| state.draft().cloned())
        .expect("a draft opened");
    assert_eq!((draft.side, draft.line), (Side::Old, 2));
    let rows = rig.read(|state| state.flat().rows.clone());
    assert!(rows.iter().any(|row| matches!(row, Row::Draft { .. })));
    // The owner's editor is focused (the viewer focuses it on the next frame).
    rig.with_app(|window, cx| {
        window.simulate_next_frame(cx);
    });
    let focused = rig.with_window(|state, window| state.draft_focus_handle().is_focused(window));
    assert!(focused, "the draft editor takes focus");
    // Escape cancels the draft before the focused editor sees it and activates no code.
    rig.host.simulate_keystrokes("escape", None).unwrap();
    rig.refresh();
    assert!(rig.read(|state| state.draft().is_none()));
    let events = rig.events();
    assert!(
        !events.iter().any(|e| e.contains("editor key")),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| e.starts_with("line") || e.starts_with("add"))
    );
    assert!(!rig.read(|state| {
        state
            .flat()
            .rows
            .iter()
            .any(|r| matches!(r, Row::Draft { .. }))
    }));
}

#[test]
fn r28_new_side_affordance_commit_trims_and_discards_empty_and_cites_the_pre_rename_path() {
    require_backend!();
    let patch = "diff --git a/old.txt b/new.txt\nrename from old.txt\nrename to new.txt\n--- a/old.txt\n+++ b/new.txt\n@@ -1,2 +1,2 @@\n ctx\n-gone\n+came\n";
    let mut config = Config::new(patch, 400., 400.);
    config.scroll = false;
    config.editor = true;
    config.annotations = vec![Annotation {
        id: "o".into(),
        path: "new.txt".into(),
        side: Side::Old,
        line: 2,
        body: "was here".into(),
    }];
    let slot = config.editor_slot.clone();
    let mut rig = Rig::new(config, (400., 400.));
    // Header 0-36, notice 36-60, hunk 60-88, ctx 88-109, deleted 109-130, [card], added.
    let texts = rig.host.painted_text();
    assert!(
        texts.iter().any(|t| t == "old.txt:2"),
        "old-side cards cite the old path: {texts:?}"
    );
    // The added line's affordance is in the NEW gutter: left = 3 + 36 + 10.
    let added_y = 109. + 21. + 66. + 10.;
    hover_then_click(&mut rig, 3. + 36. + 18., f64::from(added_y as f32));
    let draft = rig.read(|state| state.draft().cloned()).expect("draft");
    assert_eq!((draft.side, draft.line), (Side::New, 2));
    let editor = slot.borrow().clone().expect("editor built");
    rig.with_app(|window, cx| editor.commit("   ", window, cx));
    assert!(
        rig.events().iter().all(|e| !e.starts_with("add")),
        "empty body discarded"
    );
    assert!(rig.read(|state| state.draft().is_none()));
    hover_then_click(&mut rig, 3. + 36. + 18., f64::from(added_y as f32));
    let editor = slot.borrow().clone().expect("editor");
    rig.with_app(|window, cx| editor.commit("  Looks good \n", window, cx));
    assert!(
        rig.events()
            .contains(&"add new.txt New 2 \"Looks good\" Some(\"old.txt\")".to_owned()),
        "{:?}",
        rig.events()
    );
    assert!(rig.read(|state| state.draft().is_none()));
}

#[test]
fn r28_split_only_the_right_column_hosts_the_affordance_and_modes_clear_hover() {
    require_backend!();
    let mut rig = annotated_rig(DiffMode::Split);
    // The left column is inert: a click at its gutter opens nothing.
    hover_then_click(&mut rig, 21., 95.);
    assert!(rig.read(|state| state.draft().is_none()));
    // Right column: half width (400 - 1) / 2 = 199.5, left = 200.5 + 3 + 10.
    hover_then_click(&mut rig, 200.5 + 3. + 18., 95.);
    let draft = rig
        .read(|state| state.draft().cloned())
        .expect("draft on the right");
    assert_eq!((draft.side, draft.line), (Side::New, 2));
    // The draft is discarded when the owner's context key changes.
    rig.set(|config| config.draft_key = 7);
    assert!(rig.read(|state| state.draft().is_none()));
    // Old-side cards staged in unified mode still display in split mode.
    let rows = rig.read(|state| state.flat().rows.clone());
    assert!(
        rows.iter()
            .any(|row| matches!(row, Row::Card { annotation: 0, .. }))
    );
}

#[test]
fn r28_hover_state_clears_on_a_mode_switch() {
    require_backend!();
    let mut config = Config::new(SAMPLE, 400., 400.);
    config.scroll = false;
    config.editor = true;
    let mut rig = Rig::new(config, (400., 400.));
    // Unified rows end at 148; split merges the -old/+new pair so its rows end at 127.
    rig.host.mouse_move(100., 140., None, "").unwrap();
    rig.refresh();
    assert!(rig.read(|state| state.hovered_row()).is_some());
    rig.set(|config| config.mode = DiffMode::Split);
    assert_eq!(
        rig.read(|state| state.hovered_row()),
        None,
        "that column no longer exists"
    );
}
