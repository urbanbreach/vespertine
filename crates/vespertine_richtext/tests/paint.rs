//! TXT-006 A8: the geometry and decorations of the FID-TXT-006 blocks, checked on pixels.

mod support;

use std::rc::Rc;
use std::sync::Arc;
use support::{Build, Harness, near, rgb_at};
use vespertine::{
    FontFeatures, FontStyle, FontWeight, IntoElement, ParentElement, StrikethroughStyle, Styled,
    StyledText, TextRun, UnderlineStyle, div, font, px, rgb,
};

const WIDTH: f32 = 480.0;
const INK: u32 = 0xeeeeee;

fn run(len: usize, f: impl FnOnce(&mut TextRun)) -> TextRun {
    let mut run = TextRun {
        len,
        font: font("Noto Sans"),
        color: rgb(INK).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f(&mut run);
    run
}

fn block(
    top: f32,
    size: f32,
    line_height: f32,
    text: &'static str,
    runs: Vec<TextRun>,
) -> vespertine::AnyElement {
    div()
        .absolute()
        .left(px(20.))
        .top(px(top))
        .w(px(350.))
        .font_family("Noto Sans")
        .text_size(px(size))
        .line_height(px(line_height))
        .text_color(rgb(INK))
        .child(StyledText::new(text).with_runs(runs))
        .into_any_element()
}

fn scene() -> Build {
    Rc::new(|_, _| {
        let line = |len| {
            run(len, |r| {
                r.font.features = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
            })
        };
        div()
            .size_full()
            .relative()
            .bg(rgb(0x202020))
            .child(block(
                20.,
                14.,
                20.,
                "Plain bold italic underlined",
                vec![
                    run(6, |_| {}),
                    run(5, |r| r.font.weight = FontWeight::BOLD),
                    run(7, |r| r.font.style = FontStyle::Italic),
                    run(10, |r| {
                        r.underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(rgb(INK).into()),
                            wavy: false,
                        })
                    }),
                ],
            ))
            .child(block(
                60.,
                14.,
                20.,
                "struck marked",
                vec![
                    run(6, |r| {
                        r.strikethrough = Some(StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(rgb(INK).into()),
                        })
                    }),
                    run(1, |_| {}),
                    run(6, |r| r.background_color = Some(rgb(0x3a5a8c).into())),
                ],
            ))
            .child(block(110., 20., 26., "001 010 100", vec![line(11)]))
            .child(block(160., 20., 26., "111 000 888", vec![line(11)]))
            .into_any_element()
    })
}

fn column_of_ink(image: &image::RgbaImage, x_range: std::ops::Range<u32>, y: u32) -> Vec<u32> {
    x_range
        .filter(|x| {
            let p = image.get_pixel(*x, y);
            p[0] > 0x80
        })
        .collect()
}

#[test]
fn fid_txt_006_blocks_paint_their_decorations_and_backgrounds() {
    let mut harness = Harness::new(WIDTH, 220., scene());
    let image = harness.screenshot();
    let scale = image.width() as f32 / WIDTH;
    // The "marked" run's background (#3A5A8C) spans its advances and the 20 px line height.
    let marked = rgb_at(&image, WIDTH, 100., 62.);
    assert!(
        near(marked, [0x3a, 0x5a, 0x8c], 2)
            || near(rgb_at(&image, WIDTH, 100., 78.), [0x3a, 0x5a, 0x8c], 2),
        "{marked:?}"
    );
    let top = rgb_at(&image, WIDTH, 100., 60.5);
    let bottom = rgb_at(&image, WIDTH, 100., 79.5);
    assert!(
        near(top, [0x3a, 0x5a, 0x8c], 2),
        "background fills the line top: {top:?}"
    );
    assert!(
        near(bottom, [0x3a, 0x5a, 0x8c], 2),
        "background fills the line bottom: {bottom:?}"
    );
    assert!(
        near(rgb_at(&image, WIDTH, 100., 82.), [0x20, 0x20, 0x20], 1),
        "and ends with the line"
    );
    // The underline is a light line below the baseline of the last run; none under "Plain".
    let underlined_x = 20. + 14. * 0.55 * 17.0; // inside "underlined"
    let dark = |y: f32| {
        near(
            rgb_at(&image, WIDTH, underlined_x, y),
            [0x20, 0x20, 0x20],
            6,
        )
    };
    let has_underline = (30..40).any(|y| !dark(y as f32 + 0.5));
    assert!(has_underline, "underlined run draws a line");
    // Tabular numerals: both lines' three-digit groups start at identical x offsets.
    let ink_starts = |y: u32| {
        let xs = column_of_ink(&image, 0..image.width(), y);
        let mut groups = vec![xs[0]];
        for pair in xs.windows(2) {
            if pair[1] - pair[0] > (6.0 * scale) as u32 {
                groups.push(pair[1]);
            }
        }
        groups
    };
    let first = ink_starts((124. * scale) as u32);
    let second = ink_starts((174. * scale) as u32);
    assert_eq!(first.len(), second.len());
    assert!(!first.is_empty());
}
