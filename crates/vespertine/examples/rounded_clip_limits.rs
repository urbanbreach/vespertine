//! What the explicit rounded clip scope does NOT round (RND-002 R7, A6): paths, underline and
//! strikethrough decorations, and drop and inset shadows keep only the scope's rectangular
//! content mask. The example paints each of them into the cut-away top-left corner of a scope
//! with radius 40 and with radius 0: the images are identical, the pixels in the cut-away
//! corner are painted, while a gradient-filled quad in the same place is cut by the curve.
//!
//! ```sh
//! cargo run -p vespertine --features test-support --example rounded_clip_limits -- --out /tmp/limits
//! ```

#[path = "example_support/headless_host.rs"]
mod headless_host;

use headless_host::{Host, output_directory, save, scope};
use image::RgbaImage;
use vespertine::{
    AnyElement, Bounds, BoxShadow, Corners, IntoElement, ParentElement, PathBuilder,
    StrikethroughStyle, Styled, UnderlineStyle, Window, canvas, div, fill, linear_color_stop,
    linear_gradient, point, px, rgb, rounded_clip, size, white,
};

/// The scope in window logical pixels.
const SCOPE: [f32; 4] = [20.0, 20.0, 120.0, 80.0];

type Content = fn(&mut Window);

fn path(window: &mut Window) {
    let mut builder = PathBuilder::fill();
    builder.move_to(point(px(20.0), px(20.0)));
    builder.line_to(point(px(70.0), px(20.0)));
    builder.line_to(point(px(20.0), px(70.0)));
    builder.close();
    window.paint_path(builder.build().expect("path"), rgb(0x8fd16a));
}

fn underline(window: &mut Window) {
    window.paint_underline(
        point(px(22.0), px(24.0)),
        px(60.0),
        &UnderlineStyle {
            thickness: px(3.0),
            color: Some(white()),
            wavy: false,
        },
    );
    window.paint_strikethrough(
        point(px(22.0), px(34.0)),
        px(60.0),
        &StrikethroughStyle {
            thickness: px(3.0),
            color: Some(rgb(0xe0a040).into()),
        },
    );
}

fn shadow(window: &mut Window) {
    let element = Bounds {
        origin: point(px(26.0), px(26.0)),
        size: size(px(24.0), px(24.0)),
    };
    window.paint_drop_shadows(
        element,
        Corners::all(px(4.0)),
        &[BoxShadow {
            color: white().opacity(0.9),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(4.0),
            spread_radius: px(0.0),
            inset: false,
        }],
    );
}

fn gradient_quad(window: &mut Window) {
    window.paint_quad(fill(
        Bounds {
            origin: point(px(20.0), px(20.0)),
            size: size(px(50.0), px(50.0)),
        },
        linear_gradient(
            180.0,
            linear_color_stop(rgb(0xe0a040), 0.0),
            linear_color_stop(rgb(0xc06848), 1.0),
        ),
    ));
}

fn render(host: &mut Host, radius: f32, content: Content) -> RgbaImage {
    host.render(160.0, 120.0, move |_, _| -> AnyElement {
        div()
            .size_full()
            .bg(rgb(0x181a1f))
            .child(
                div()
                    .absolute()
                    .left(px(SCOPE[0]))
                    .top(px(SCOPE[1]))
                    .w(px(SCOPE[2]))
                    .h(px(SCOPE[3]))
                    .child(rounded_clip(
                        px(radius),
                        canvas(|_, _, _| (), move |_, _, window, _| content(window)).size_full(),
                    )),
            )
            .into_any_element()
    })
}

fn main() {
    let output = output_directory();
    let mut host = Host::new();
    let background = render(&mut host, 40.0, |_| {});
    // Device pixels (scale factor 2) of the cut-away corner of the radius-40 scope.
    let curve = scope(
        [
            SCOPE[0] * 2.0,
            SCOPE[1] * 2.0,
            (SCOPE[0] + SCOPE[2]) * 2.0,
            (SCOPE[1] + SCOPE[3]) * 2.0,
        ],
        80.0,
    );
    let corner_pixels: Vec<(u32, u32)> = (0..background.height())
        .flat_map(|y| (0..background.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            // Inside the scope's rectangle but cut away by the curve.
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            p[0] > 40.0
                && p[1] > 40.0
                && curve.rounded_coverage(p) == 0.0
                && p[0] < 140.0
                && p[1] < 140.0
        })
        .collect();
    assert!(corner_pixels.len() > 500);

    for (name, content) in [
        ("path", path as Content),
        ("underline_and_strikethrough", underline),
        ("shadow", shadow),
    ] {
        let rounded = render(&mut host, 40.0, content);
        let square = render(&mut host, 0.0, content);
        save(&output, &format!("{name}_radius_40"), &rounded);
        save(&output, &format!("{name}_radius_0"), &square);
        assert_eq!(
            rounded.as_raw(),
            square.as_raw(),
            "{name} must keep only the rectangular mask"
        );
        let painted = corner_pixels
            .iter()
            .filter(|&&(x, y)| rounded.get_pixel(x, y) != background.get_pixel(x, y))
            .count();
        assert!(
            painted > 20,
            "{name} must paint in the cut-away corner ({painted})"
        );
        println!("{name}: rectangular only; {painted} pixels painted in the cut-away corner");
    }

    // Control: a gradient-filled quad takes the per-fragment route and is rounded.
    let rounded = render(&mut host, 40.0, gradient_quad);
    let square = render(&mut host, 0.0, gradient_quad);
    save(&output, "gradient_quad_radius_40", &rounded);
    save(&output, "gradient_quad_radius_0", &square);
    assert_ne!(rounded.as_raw(), square.as_raw());
    let leaked = corner_pixels
        .iter()
        .filter(|&&(x, y)| rounded.get_pixel(x, y) != background.get_pixel(x, y))
        .count();
    assert_eq!(leaked, 0, "the gradient quad must be cut by the curve");
    println!("gradient quad: rounded, none of its pixels in the cut-away corner");
    println!("rounded_clip_limits: ok");
}
