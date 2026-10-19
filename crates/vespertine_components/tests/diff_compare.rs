//! The composed comparison viewer through the production offscreen host (CMP-015 A8,
//! FID-CMP-015-2).

use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px, rgb};
use vespertine_components::diff::compare::{ComparisonHunk, ComparisonView};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

fn hunk(old_start: u32, new_start: u32, lines: &[&str]) -> ComparisonHunk {
    ComparisonHunk {
        old_start,
        old_count: lines.len() as u32,
        new_start,
        new_count: lines.len() as u32,
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
    }
}

fn host(hunks: Vec<ComparisonHunk>, split: bool, size: (f64, f64)) -> TestHost {
    TestHost::with_element(
        HostOptions::with_size(size.0, size.1),
        move |_window: &mut Window, _cx: &mut App| -> AnyElement {
            div()
                .size_full()
                .bg(rgb(0x181818))
                .child(
                    ComparisonView::new(hunks.clone())
                        .split_view(split)
                        .font_family("Liberation Mono")
                        .font_size(px(13.))
                        .line_height(px(20.))
                        .absolute()
                        .left(px(20.))
                        .top(px(20.))
                        .w(px(660.)),
                )
                .into_any_element()
        },
    )
    .expect("host")
}

fn blend(fg: [f32; 3], alpha: f32, bg: [f32; 3]) -> [u8; 3] {
    [0, 1, 2].map(|i| (fg[i] * alpha + bg[i] * (1. - alpha)).round() as u8)
}

fn near(actual: [u8; 4], expected: [u8; 3]) -> bool {
    actual[..3]
        .iter()
        .zip(expected)
        .all(|(a, b)| a.abs_diff(b) <= 2)
}

fn pixel(image: &image::RgbaImage, x: f32, y: f32) -> [u8; 4] {
    image.get_pixel((x * 2.) as u32, (y * 2.) as u32).0
}

#[test]
fn fid_cmp_015_2_split_rows_have_the_specified_fills_washes_and_texts() {
    require_backend!();
    let hunks = vec![hunk(1, 1, &["-same old", "+same new", " unchanged"])];
    let mut host = host(hunks, true, (700., 240.));
    host.flush();
    let image = host.screenshot_image().unwrap();
    let window_bg = [24., 24., 24.];
    // Row 1 (y 20..40): left gutter x 20..51.2 #3c0000, body rgba(255,0,0,0.125).
    assert!(near(pixel(&image, 25., 30.), [0x3c, 0, 0]));
    let removal_body = blend([255., 0., 0.], 0.125, window_bg);
    assert!(
        near(pixel(&image, 60., 22.), removal_body),
        "{:?}",
        pixel(&image, 60., 22.)
    );
    // "old" (x = 55.2 + 5 x 7.8 .. +3 chars) sits on rgba(255,50,50,0.39) over the body.
    let wash_over_body = blend([255., 50., 50.], 0.39, removal_body.map(f32::from));
    assert!(
        near(pixel(&image, 100., 21.), wash_over_body),
        "{:?}",
        pixel(&image, 100., 21.)
    );
    // Right half starts at x = 350: gutter #003200, body rgba(100,250,120,0.047).
    assert!(near(pixel(&image, 355., 30.), [0, 0x32, 0]));
    assert!(near(
        pixel(&image, 500., 22.),
        blend([100., 250., 120.], 0.047, window_bg)
    ));
    let addition_body = blend([100., 250., 120.], 0.047, window_bg);
    let new_wash = blend([0., 200., 0.], 0.39, addition_body.map(f32::from));
    assert!(
        near(pixel(&image, 436., 21.), new_wash),
        "{:?}",
        pixel(&image, 436., 21.)
    );
    // Row 2 (y 40..60): context gutters #050505, bodies #0f0f0f, on both halves.
    assert!(near(pixel(&image, 25., 50.), [5, 5, 5]));
    assert!(near(pixel(&image, 60., 42.), [0x0f, 0x0f, 0x0f]));
    assert!(near(pixel(&image, 355., 50.), [5, 5, 5]));
    assert!(near(pixel(&image, 500., 42.), [0x0f, 0x0f, 0x0f]));
    // The component paints no surface: everywhere else is the window background.
    assert!(near(pixel(&image, 10., 30.), [24, 24, 24]));
    assert!(near(pixel(&image, 300., 100.), [24, 24, 24]));
    let texts = host.painted_text();
    for expected in ["  1 ", "  2 ", "same ", "old", "new", "unchanged"] {
        assert!(
            texts.iter().any(|t| t == expected),
            "{expected:?} in {texts:?}"
        );
    }
}

#[test]
fn a8_halves_are_equal_and_each_has_its_own_gutter_in_split_mode() {
    require_backend!();
    let hunks = vec![hunk(1, 1, &["-a", "+b", " c"])];
    let mut host = host(hunks, true, (700., 240.));
    host.flush();
    let image = host.screenshot_image().unwrap();
    // The left half ends and the right one starts at x = 20 + 330 = 350.
    assert!(
        near(pixel(&image, 349., 50.), [0x0f, 0x0f, 0x0f]),
        "left body to the edge"
    );
    assert!(
        near(pixel(&image, 351., 50.), [5, 5, 5]),
        "right gutter right after"
    );
    assert!(near(pixel(&image, 679., 50.), [0x0f, 0x0f, 0x0f]));
    assert!(near(pixel(&image, 681., 50.), [24, 24, 24]));
}

#[test]
fn a8_unified_stream_has_one_gutter_separators_between_hunks_and_blank_zero_counters() {
    require_backend!();
    let hunks = vec![
        hunk(0, 1, &["+first", " second"]),
        hunk(10, 10, &["-gone", "+came"]),
    ];
    let mut host = host(hunks, false, (700., 240.));
    host.flush();
    let texts = host.painted_text();
    // Counter 0 is blank in the unified stream; the second line shows 1.
    assert!(texts.iter().any(|t| t == "    "), "blank gutter: {texts:?}");
    assert!(texts.iter().any(|t| t == "  1 "));
    // The separator: (max(left, right) + 2) spaces then three periods.
    assert!(texts.iter().any(|t| t == "    ..."), "{texts:?}");
    // Both counters of the second hunk start at its old start (the preserved quirk).
    assert!(
        texts.iter().filter(|t| t.as_str() == " 10 ").count() >= 2,
        "{texts:?}"
    );
}

#[test]
fn a8_empty_hunks_show_no_changes_with_sixteen_pixels_of_padding() {
    require_backend!();
    let mut host = host(Vec::new(), false, (700., 240.));
    host.flush();
    assert_eq!(host.painted_text(), ["No changes"]);
    let image = host.screenshot_image().unwrap();
    // Faint number color #6c7086 text starts 16 px inside the component (x = 36): find the
    // leftmost non-background pixel of the text row.
    let mut leftmost = None;
    for x in 0..700 {
        for y in 0..240 {
            let p = image.get_pixel(x * 2, y * 2).0;
            if p[..3] != [24, 24, 24] {
                leftmost = Some(leftmost.map_or(x, |l: u32| l.min(x)));
            }
        }
        if leftmost.is_some() {
            break;
        }
    }
    let left = leftmost.expect("text painted");
    assert!(
        (35..=38).contains(&left),
        "text starts at x = 20 + 16: {left}"
    );
}
