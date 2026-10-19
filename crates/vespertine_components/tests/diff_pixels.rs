//! The native patch viewer against FID-CMP-015-1 (CMP-015 R9-R12), rendered through the
//! production offscreen host.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, Entity, IntoElement, ParentElement, Rgba, Styled, Window, div, px, rgb,
};
use vespertine_components::diff::state::DiffViewState;
use vespertine_components::diff::view::DiffView;
use vespertine_test_host::{HostOptions, TestHost, require_backend};
use vespertine_theme::document::DocumentTheme;

const PATCH: &str = "diff --git a/sample.txt b/sample.txt\n--- a/sample.txt\n+++ b/sample.txt\n@@ -1 +1 @@\n-old value\n+new value\n";

fn fid_theme() -> Arc<DocumentTheme> {
    let mut theme = DocumentTheme::dark();
    theme.bg = rgb(0x101010);
    theme.text = rgb(0xeeeeee);
    theme.text_dim = rgb(0x999999);
    theme.text_faint = rgb(0x777777);
    theme.diff_add = rgb(0x00cc88);
    theme.diff_del = rgb(0xff5555);
    theme.diff_hunk_bg = Rgba {
        r: 64. / 255.,
        g: 96. / 255.,
        b: 128. / 255.,
        a: 32. / 255.,
    };
    theme.font_mono = "Liberation Mono".into();
    theme.font_sans = "Liberation Mono".into();
    Arc::new(theme)
}

fn scene(radius: Option<f32>) -> TestHost {
    let state: Rc<RefCell<Option<Entity<DiffViewState>>>> = Rc::default();
    TestHost::with_element(
        HostOptions::with_size(700., 300.),
        move |_window: &mut Window, cx: &mut App| -> AnyElement {
            let entity = state
                .borrow_mut()
                .get_or_insert_with(|| DiffViewState::new(cx))
                .clone();
            let viewer = DiffView::new(&entity)
                .patch(PATCH)
                .theme(fid_theme())
                .absolute()
                .left(px(20.))
                .top(px(20.))
                .w(px(660.))
                .h(px(114.));
            let viewer = match radius {
                Some(radius) => viewer.rounded(px(radius)),
                None => viewer,
            };
            div()
                .size_full()
                .bg(rgb(0x181818))
                .child(viewer)
                .into_any_element()
        },
    )
    .expect("host")
}

fn blend(fg: [f32; 3], alpha: f32, bg: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| fg[i] * alpha + bg[i] * (1. - alpha))
}

fn near(actual: [u8; 4], expected: [f32; 3]) -> bool {
    actual[..3]
        .iter()
        .zip(expected)
        .all(|(a, b)| f32::from(*a) - b <= 2.5 && b - f32::from(*a) <= 2.5)
}

fn pixel(image: &image::RgbaImage, x: f32, y: f32) -> [u8; 4] {
    image.get_pixel((x * 2.) as u32, (y * 2.) as u32).0
}

#[test]
fn fid_cmp_015_1_rows_have_the_specified_fills_lanes_and_heights() {
    require_backend!();
    let mut host = scene(None);
    host.flush();
    let image = host.screenshot_image().unwrap();
    let viewer = [16., 16., 16.];
    // (a) File header y 20..56: white at 0.025 over the viewer fill, a top border at y = 20.
    let header = blend([255.; 3], 0.025, viewer);
    assert!(
        near(pixel(&image, 300., 40.), header),
        "{:?}",
        pixel(&image, 300., 40.)
    );
    assert!(
        near(pixel(&image, 300., 20.2), blend([255.; 3], 0.04, header))
            || near(pixel(&image, 300., 20.2), blend([255.; 3], 0.04, viewer)),
        "1 px top border: {:?}",
        pixel(&image, 300., 20.2)
    );
    // (b) Hunk row y 56..84: #406080 at alpha 0.125 over the viewer fill.
    assert!(near(
        pixel(&image, 300., 70.),
        blend([64., 96., 128.], 32. / 255., viewer)
    ));
    // (c) Deleted row y 84..105: #ff5555 at 0.055; lane x 20..23 at 0.55.
    let deleted = blend([255., 85., 85.], 0.055, viewer);
    assert!(near(pixel(&image, 400., 95.), deleted));
    assert!(near(
        pixel(&image, 21., 95.),
        blend([255., 85., 85.], 0.55, deleted)
    ));
    // (d) Added row y 105..126.
    let added = blend([0., 204., 136.], 0.055, viewer);
    assert!(near(pixel(&image, 400., 116.), added));
    assert!(near(
        pixel(&image, 21., 116.),
        blend([0., 204., 136.], 0.55, added)
    ));
    // (e) Bottom padding y 126..134 shows the viewer fill; below it the window background.
    assert!(near(pixel(&image, 400., 130.), viewer));
    assert!(near(pixel(&image, 400., 136.), [24., 24., 24.]));
    // Context of the specified geometry: the old gutter reads "1" at its right edge.
    let texts = host.painted_text();
    for expected in [
        "sample.txt",
        "+1",
        "\u{2212}1",
        "@@ -1 +1 @@",
        "old value",
        "new value",
    ] {
        assert!(
            texts.iter().any(|t| t == expected),
            "{expected:?} in {texts:?}"
        );
    }
}

#[test]
fn r9_the_first_header_carries_the_viewers_radius_on_its_top_corners() {
    require_backend!();
    let mut host = scene(Some(12.));
    host.flush();
    let image = host.screenshot_image().unwrap();
    // The very corner of the viewer is outside the arc: the window background shows.
    assert!(
        near(pixel(&image, 20.5, 20.5), [24., 24., 24.]),
        "{:?}",
        pixel(&image, 20.5, 20.5)
    );
    assert!(near(pixel(&image, 679.5, 20.5), [24., 24., 24.]));
    // Inside the arc the header fill (white at 0.025 over the viewer fill) shows.
    let header = blend([255.; 3], 0.025, [16., 16., 16.]);
    assert!(near(pixel(&image, 40., 30.), header));
    assert!(
        near(pixel(&image, 21., 40.), header),
        "straight below the arc"
    );
}
