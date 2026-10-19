//! Renderer-level pixel tests for ELM-006 (gradient interpolation, element opacity) and ELM-008
//! (outlines), through the production painting path.

use crate::tests_host::RenderHost;
use image::RgbaImage;
use vespertine::private::serde_json::json;
use vespertine::{
    AnyElement, HostStyle, InteractiveElement as _, IntoElement, Outline, ParentElement as _,
    Styled as _, div, px, rgb,
};

fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = image.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

#[track_caller]
fn assert_color(actual: [u8; 3], expected: u32, tolerance: u8, what: &str) {
    let expected = [
        (expected >> 16) as u8,
        (expected >> 8) as u8,
        expected as u8,
    ];
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| a.abs_diff(e) <= tolerance),
        "{what}: got #{:02X}{:02X}{:02X}, expected #{:02X}{:02X}{:02X} (±{tolerance})",
        actual[0],
        actual[1],
        actual[2],
        expected[0],
        expected[1],
        expected[2],
    );
}

fn gradient_scene(
    color_space: Option<&str>,
) -> impl Fn(&mut vespertine::Window, &mut vespertine::App) -> AnyElement {
    let mut gradient = json!({
        "type": "linear-gradient", "angle": 90,
        "stops": [
            { "color": "#FF0000", "position": 0 },
            { "color": "#0000FF", "position": 1 },
        ],
    });
    if let Some(space) = color_space {
        gradient["colorSpace"] = json!(space);
    }
    let style = HostStyle::from_json(
        &json!({ "width": 300, "height": 40, "background": gradient }),
        "test",
    )
    .expect("valid style");
    move |_, _| {
        div()
            .size_full()
            .bg(rgb(0x181818))
            .pl(px(50.))
            .pt(px(50.))
            .child(style.apply(div().id("box")))
            .into_any_element()
    }
}

/// Sample column of the gradient parameter `t` across the 300 px wide box at x = 50 (scale 2).
fn gradient_x(t: f32) -> u32 {
    (((50. + 300. * t) * 2.) as u32).clamp(101, 698)
}

/// ELM-006 A9: gradient interpolation reproduces the upstream renderer.
#[test]
fn gradient_interpolation_matches_the_upstream_renderer() {
    let mut host = RenderHost::new();
    let oklab = host.render_logical(400., 200., gradient_scene(Some("oklab")));
    let y = 140;
    let expected = [
        (0., 0xFF0000),
        (0.25, 0x901127),
        (0.5, 0x43165D),
        (0.75, 0x1510A4),
        (1., 0x0000FF),
    ];
    for (t, color) in expected {
        assert_color(
            pixel(&oklab, gradient_x(t), y),
            color,
            3,
            &format!("oklab t={t}"),
        );
    }
    let srgb = host.render_logical(400., 200., gradient_scene(None));
    assert_color(pixel(&srgb, gradient_x(0.5), y), 0x370037, 3, "srgb t=0.5");
}

/// ELM-006 A9 / R22: element opacity multiplies each primitive's alpha separately, so the glyph
/// composites over the already dimmed fill.
#[test]
fn element_opacity_dims_fill_and_glyphs_separately() {
    let style = HostStyle::from_json(
        &json!({
            "width": 140, "height": 44, "backgroundColor": "#303030", "borderRadius": 6,
            "display": "flex", "alignItems": "center", "justifyContent": "center",
            "color": "#FFFFFF", "fontFamily": "Noto Sans", "fontSize": 14, "lineHeight": 20,
            "opacity": 0.4,
        }),
        "test",
    )
    .expect("valid style");
    let mut host = RenderHost::new();
    let image = host.render_logical(200., 100., move |_, _| {
        div()
            .size_full()
            .bg(rgb(0x181818))
            .pl(px(24.))
            .pt(px(24.))
            .child(style.apply(div().id("control")).child("First"))
            .into_any_element()
    });
    // A fill pixel well clear of the text.
    assert_color(pixel(&image, 2 * 30, 2 * 46), 0x222222, 3, "dimmed fill");
    // The brightest pixel of the glyphs is a fully covered interior.
    let mut brightest = 0;
    for y in 2 * 24..2 * 68 {
        for x in 2 * 24..2 * 164 {
            brightest = brightest.max(pixel(&image, x, y)[0]);
        }
    }
    assert!(
        brightest.abs_diff(0x7A) <= 3,
        "glyph interiors should be #7A7A7A, brightest pixel is {brightest:#04X}"
    );
}

/// ELM-008 A4: FID-ELM-008 at scale factor 2, including the square top-left corner of the
/// second element's ring.
#[test]
fn fid_elm_008_outline_pixels() {
    let mut host = RenderHost::new();
    let image = host.render_logical(400., 200., |_, _| {
        let blue = rgb(0x0066FF);
        let orange = rgb(0xFF8000);
        div()
            .size_full()
            .relative()
            .bg(rgb(0xFFFFFF))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(50.))
                    .w(px(120.))
                    .h(px(80.))
                    .bg(rgb(0x202020))
                    .rounded(px(8.))
                    .outline(Outline::new(px(2.), blue, px(3.))),
            )
            .child(
                div()
                    .absolute()
                    .left(px(220.))
                    .top(px(50.))
                    .w(px(120.))
                    .h(px(80.))
                    .bg(rgb(0x202020))
                    .rounded_tr(px(8.))
                    .rounded_br(px(8.))
                    .rounded_bl(px(8.))
                    .outline(Outline::new(px(3.), orange, px(-5.))),
            )
            .into_any_element()
    });
    let at = |x: f32, y: f32| pixel(&image, (x * 2.) as u32, (y * 2.) as u32);
    // First element: fill, a 3 px white gap, then the 2 px blue ring (x = 35..37).
    assert_color(at(100., 90.), 0x202020, 2, "first fill");
    assert_color(at(38.5, 90.), 0xFFFFFF, 2, "gap between fill and ring");
    assert_color(at(36., 90.), 0x0066FF, 2, "ring (left edge)");
    assert_color(at(34.5, 90.), 0xFFFFFF, 2, "outside the ring");
    assert_color(at(100., 46.), 0x0066FF, 2, "ring (top edge)");
    // Rounded outer radius 13: the ring's top-left corner pixel at the bounding corner is empty.
    assert_color(at(35.25, 45.25), 0xFFFFFF, 2, "round outer corner");
    // Second element: ring inside the fill (x = 222..225), square at the top-left corner.
    assert_color(at(260., 90.), 0x202020, 2, "second fill");
    assert_color(at(223.5, 90.), 0xFF8000, 2, "ring (left edge)");
    assert_color(
        at(222.25, 52.25),
        0xFF8000,
        2,
        "square top-left corner of the ring",
    );
    // The ring's rounded top-right corner (outer radius 6, center (332, 58)) leaves the part of the
    // element's own corner (radius 8) outside of it showing the fill.
    assert_color(at(336.95, 53.05), 0x202020, 4, "rounded top-right corner");
}

/// ELM-006 A8 (card landmarks of FID-ELM-006): the data-described card reproduces the Oklab
/// gradient landmarks along its vertical middle.
#[test]
fn fid_elm_006_card_gradient_landmarks() {
    let card = HostStyle::from_json(
        &json!({
            "position": "absolute", "left": 24, "top": 24, "width": 300, "height": 120,
            "borderRadius": 12, "borderWidth": 2, "borderColor": "#808080", "padding": 16,
            "background": {
                "type": "linear-gradient", "angle": 90, "colorSpace": "oklab",
                "stops": [
                    { "color": "#FF0000", "position": 0 },
                    { "color": "#0000FF", "position": 1 },
                ],
            },
            "boxShadow": [
                { "offsetY": 1, "blurRadius": 4, "color": "#00000040", "inset": true },
                { "offsetY": 4, "blurRadius": 12, "color": "#00000033" },
            ],
        }),
        "test",
    )
    .expect("valid style");
    let mut host = RenderHost::new();
    let image = host.render_logical(720., 400., move |_, _| {
        div()
            .size_full()
            .relative()
            .bg(rgb(0x181818))
            .child(card.apply(div().id("card")))
            .into_any_element()
    });
    let y = 84 * 2;
    for (x, color) in [(99, 0x901127), (174, 0x43165D), (249, 0x1510A4)] {
        assert_color(pixel(&image, x * 2, y), color, 3, &format!("card x={x}"));
    }
}
