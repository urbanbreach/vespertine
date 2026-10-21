//! RND-010: the `backdrop_blur` style property, rendered through the offscreen GPU host.

use std::{cell::Cell, rc::Rc};
use vespertine::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
    rgb,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

const WIDTH: f32 = 200.0;
const HEIGHT: f32 = 120.0;
const STRIPE: f32 = 4.0;
const BLUE: u32 = 0x0000FF;

/// Black and white vertical stripes, `STRIPE` logical pixels wide; the first stripe is black,
/// or white when `shift` is odd.
fn stripes(shift: usize) -> impl IntoElement {
    div()
        .absolute()
        .size_full()
        .flex()
        .children((0..(WIDTH / STRIPE) as usize).map(|index| {
            div()
                .w(px(STRIPE))
                .h_full()
                .bg(rgb(if (index + shift).is_multiple_of(2) {
                    0x000000
                } else {
                    0xFFFFFF
                }))
        }))
}

/// The channels of the frame pixel under the logical point `(x, y)`.
fn pixel(image: &image::RgbaImage, scale: f32, x: f32, y: f32) -> [u8; 3] {
    let pixel = image.get_pixel((x * scale) as u32, (y * scale) as u32);
    [pixel[0], pixel[1], pixel[2]]
}

fn assert_stripe(color: [u8; 3], x: f32, shift: usize, at: &str) {
    let expected = if ((x / STRIPE) as usize + shift).is_multiple_of(2) {
        0
    } else {
        255
    };
    assert_eq!(color, [expected; 3], "{at}: sharp stripe expected");
}

fn assert_blurred(color: [u8; 3], at: &str) {
    assert!(
        color.iter().all(|&channel| (64..=192).contains(&channel)),
        "{at}: blurred grey expected, got {color:?}"
    );
}

fn host(element: impl Fn() -> vespertine::AnyElement + 'static) -> (TestHost, f32) {
    let mut host = TestHost::with_element(
        HostOptions::with_size(f64::from(WIDTH), f64::from(HEIGHT)),
        move |_, _| element(),
    )
    .expect("host");
    let scale = host.scale_factor();
    (host, scale)
}

#[test]
fn backdrop_blur_blurs_earlier_content_inside_rounded_bounds_and_children_stay_sharp() {
    require_backend!();
    let (mut host, scale) = host(|| {
        div()
            .size_full()
            .child(stripes(0))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(20.))
                    .w(px(120.))
                    .h(px(80.))
                    .rounded(px(16.))
                    .backdrop_blur(px(6.))
                    .child(
                        div()
                            .absolute()
                            .left(px(50.))
                            .top(px(30.))
                            .size(px(20.))
                            .bg(rgb(BLUE)),
                    ),
            )
            .into_any_element()
    });
    let image = host.screenshot_image().expect("screenshot");

    assert_blurred(pixel(&image, scale, 62., 40.), "inside the panel");
    assert_blurred(pixel(&image, scale, 150., 90.), "inside the panel");
    assert_eq!(
        pixel(&image, scale, 100., 60.),
        [0, 0, 255],
        "the child paints over the blur"
    );
    for (x, y, at) in [
        (10., 60., "left of the panel"),
        (170., 60., "right of the panel"),
        (102., 10., "above the panel"),
        (102., 110., "below the panel"),
        (42., 22., "outside the rounded top-left corner"),
        (158., 98., "outside the rounded bottom-right corner"),
    ] {
        assert_stripe(pixel(&image, scale, x, y), x, 0, at);
    }
}

#[test]
fn backdrop_blur_is_clipped_by_an_ancestor_overflow_mask() {
    require_backend!();
    let (mut host, scale) = host(|| {
        div()
            .size_full()
            .child(stripes(0))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(20.))
                    .w(px(60.))
                    .h(px(80.))
                    .overflow_hidden()
                    .child(div().w(px(120.)).h(px(80.)).backdrop_blur(px(6.))),
            )
            .into_any_element()
    });
    let image = host.screenshot_image().expect("screenshot");

    assert_blurred(pixel(&image, scale, 70., 60.), "inside the mask");
    assert_stripe(
        pixel(&image, scale, 130., 60.),
        130.,
        0,
        "inside the panel, outside the mask",
    );
}

/// Frame pixels over a white (x = 62) and a black (x = 66) stripe under a blurred panel of
/// `opacity`.
fn panel_at_opacity(opacity: f32) -> [[u8; 3]; 2] {
    let (mut host, scale) = host(move || {
        div()
            .size_full()
            .child(stripes(0))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(20.))
                    .w(px(120.))
                    .h(px(80.))
                    .opacity(opacity)
                    .backdrop_blur(px(6.)),
            )
            .into_any_element()
    });
    let image = host.screenshot_image().expect("screenshot");
    [
        pixel(&image, scale, 62., 40.),
        pixel(&image, scale, 66., 40.),
    ]
}

#[test]
fn backdrop_blur_at_opacity_zero_leaves_the_backdrop_unchanged() {
    require_backend!();
    let [white, black] = panel_at_opacity(0.0);
    assert_stripe(white, 62., 0, "white stripe");
    assert_stripe(black, 66., 0, "black stripe");
}

#[test]
fn backdrop_blur_at_full_opacity_replaces_the_backdrop_with_the_blur() {
    require_backend!();
    for color in panel_at_opacity(1.0) {
        assert_blurred(color, "full opacity");
    }
}

#[test]
fn backdrop_blur_at_half_opacity_is_halfway_between_backdrop_and_blur() {
    require_backend!();
    let blurred = panel_at_opacity(1.0);
    let half = panel_at_opacity(0.5);
    for ((half, blurred), sharp) in half.iter().zip(blurred).zip([255.0_f32, 0.0]) {
        for (half, blurred) in half.iter().zip(blurred) {
            let expected = (sharp + f32::from(blurred)) / 2.0;
            assert!(
                (f32::from(*half) - expected).abs() <= 2.0,
                "half opacity: {half} expected about {expected} (sharp {sharp}, blurred {blurred})"
            );
        }
    }
}

struct Backdrop {
    shift: usize,
    panel: Entity<Panel>,
}

impl Render for Backdrop {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let backdrop = stripes(self.shift);
        div().size_full().child(backdrop).child(self.panel.clone())
    }
}

struct Panel {
    renders: Rc<Cell<usize>>,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .absolute()
            .left(px(40.))
            .top(px(20.))
            .w(px(120.))
            .h(px(80.))
            .rounded(px(16.))
            .backdrop_blur(px(6.))
    }
}

#[test]
fn backdrop_blur_of_a_replayed_view_blurs_the_rebuilt_backdrop() {
    require_backend!();
    let renders = Rc::new(Cell::new(0));
    let root = Rc::new(Cell::new(None));
    let mut host = TestHost::with_view(
        HostOptions {
            complete_frames: false,
            ..HostOptions::with_size(f64::from(WIDTH), f64::from(HEIGHT))
        },
        {
            let renders = renders.clone();
            let root = root.clone();
            move |_, cx| {
                let panel = cx.new(|_| Panel { renders });
                let backdrop = cx.new(|_| Backdrop { shift: 0, panel });
                root.set(Some(backdrop.clone()));
                backdrop
            }
        },
    )
    .expect("host");
    let scale = host.scale_factor();
    let backdrop: Entity<Backdrop> = root.take().expect("root view");

    let striped = host.screenshot_image().expect("screenshot");
    assert_blurred(pixel(&striped, scale, 62., 40.), "over stripes");
    let panel_renders = renders.get();

    host.app().update(|cx| {
        backdrop.update(cx, |backdrop, cx| {
            backdrop.shift = 1;
            cx.notify();
        })
    });
    let shifted = host.screenshot_image().expect("screenshot");

    assert_eq!(
        renders.get(),
        panel_renders,
        "the panel view is replayed, not rendered again"
    );
    assert_stripe(
        pixel(&shifted, scale, 10., 60.),
        10.,
        1,
        "the rebuilt backdrop",
    );
    assert_blurred(
        pixel(&shifted, scale, 62., 40.),
        "the replayed panel still blurs",
    );
}
