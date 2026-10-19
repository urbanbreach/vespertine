//! CMP-031: classic thumb indicators (A2, A3, fidelity geometry), the overlay rail's visibility,
//! hit testing, look and press/drag behavior (A5-A8) and the virtual-list drag freeze (A9).

use std::{cell::Cell, rc::Rc};

use image::RgbaImage;
use vespertine::{
    AppContext as _, Context, Hsla, InteractiveElement, IntoElement, ListAlignment, ListOffset,
    ListState, MouseButton, ParentElement, Render, ScrollHandle, StatefulInteractiveElement,
    Styled, Window, div, hsla, list, point, px, rgb,
};
use vespertine_components::scroll::{
    HandleRail, ListRail, RailBar, RailHost, RailOrientation, render_rail,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

const SCALE: f32 = 2.0;
const VIEW_BG: [u8; 3] = [0x20, 0x20, 0x20];
const THUMB: [u8; 3] = [0xa0, 0xa0, 0xa0];

fn pixel(image: &RgbaImage, x: f32, y: f32) -> [u8; 3] {
    let p = image.get_pixel((x * SCALE) as u32, (y * SCALE) as u32);
    [p[0], p[1], p[2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 2)
}

/// The logical x range `[start, end)` painted in the thumb color on logical row `y`.
fn run_x(image: &RgbaImage, y: f32, x0: f32, x1: f32) -> Option<(f32, f32)> {
    let mut start = None;
    let mut end = None;
    let mut x = x0;
    while x < x1 {
        if near(pixel(image, x + 0.25, y), THUMB) {
            start.get_or_insert(x);
            end = Some(x + 0.5);
        }
        x += 0.5;
    }
    start.zip(end)
}

/// The logical y range painted in the thumb color on logical column `x`.
fn run_y(image: &RgbaImage, x: f32, y0: f32, y1: f32) -> Option<(f32, f32)> {
    let mut start = None;
    let mut end = None;
    let mut y = y0;
    while y < y1 {
        if near(pixel(image, x, y + 0.25), THUMB) {
            start.get_or_insert(y);
            end = Some(y + 0.5);
        }
        y += 0.5;
    }
    start.zip(end)
}

/// Asserts a painted span equals `expected` within a pixel: the rounded end caps are
/// anti-aliased where the sampled column or row is not exactly on the thumb's axis.
#[track_caller]
fn assert_span(actual: Option<(f32, f32)>, expected: (f32, f32)) {
    let (a, b) = actual.expect("a thumb was painted");
    assert!(
        (a - expected.0).abs() <= 1.0 && (b - expected.1).abs() <= 1.0,
        "{:?} vs {:?}",
        (a, b),
        expected
    );
}

// ---------------------------------------------------------------------------------------------
// Classic thumbs.
// ---------------------------------------------------------------------------------------------

struct Classic {
    handle: ScrollHandle,
    content: (f32, f32),
    color: Option<Hsla>,
    hits: Rc<Cell<u32>>,
    border: bool,
}

impl Render for Classic {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let hits = self.hits.clone();
        let mut viewport = div()
            .id("viewport")
            .absolute()
            .left(px(20.0))
            .top(px(20.0))
            .w(px(300.0))
            .h(px(240.0))
            .bg(rgb(0x202020))
            .overflow_scroll()
            .track_scroll(&self.handle)
            .on_mouse_down(MouseButton::Left, move |_, _, _| hits.set(hits.get() + 1));
        if let Some(color) = self.color {
            viewport = viewport.scrollbar_color(color).scrollbar_width(px(6.0));
        }
        if self.border {
            viewport = viewport.border_2().border_color(rgb(0x303030));
        }
        div()
            .size_full()
            .relative()
            .bg(rgb(0x101010))
            .child(
                viewport.child(
                    div()
                        .flex_none()
                        .w(px(self.content.0))
                        .h(px(self.content.1)),
                ),
            )
            .into_element()
    }
}

struct ClassicFixture {
    host: TestHost,
    handle: ScrollHandle,
    hits: Rc<Cell<u32>>,
}

fn classic(content: (f32, f32), color: Option<Hsla>, border: bool) -> ClassicFixture {
    let handle = ScrollHandle::new();
    let hits = Rc::new(Cell::new(0));
    let (view_handle, view_hits) = (handle.clone(), hits.clone());
    let mut host = TestHost::with_view(HostOptions::with_size(360.0, 300.0), move |_, cx| {
        cx.new(|_| Classic {
            handle: view_handle,
            content,
            color,
            hits: view_hits,
            border,
        })
    })
    .expect("host");
    host.flush();
    ClassicFixture { host, handle, hits }
}

fn grey() -> Option<Hsla> {
    Some(rgb(0xa0a0a0).into())
}

#[test]
fn classic_a2_content_that_fits_paints_nothing() {
    require_backend!();
    // Exactly fitting and overflowing by less than the 1 px threshold.
    for content in [(300.0, 240.0), (300.0, 240.5), (300.5, 240.5)] {
        let mut f = classic(content, grey(), false);
        let image = f.host.screenshot_image().unwrap();
        assert!(run_y(&image, 317.0, 20.0, 260.0).is_none(), "{content:?}");
        assert!(run_x(&image, 257.0, 20.0, 320.0).is_none(), "{content:?}");
    }
}

#[test]
fn classic_a2_thumbs_paint_for_vertical_horizontal_and_both_axes() {
    require_backend!();
    // Vertical only (FID-CMP-031-1 geometry).
    let mut f = classic((300.0, 960.0), grey(), false);
    f.handle.set_offset(point(px(0.0), px(-360.0)));
    f.host.paint_frame();
    let image = f.host.screenshot_image().unwrap();
    assert_span(run_y(&image, 317.0, 20.0, 260.0), (110.0, 170.0));
    assert_span(run_x(&image, 140.0, 300.0, 322.0), (314.0, 320.0));
    assert!(run_x(&image, 257.0, 20.0, 320.0).is_none());

    // Horizontal only.
    let mut f = classic((600.0, 240.0), grey(), false);
    f.handle.set_offset(point(px(-150.0), px(0.0)));
    f.host.paint_frame();
    let image = f.host.screenshot_image().unwrap();
    // track 300, length 300 x 300 / 600 = 150, start 0.5 x 150 = 75.
    assert_span(run_x(&image, 257.0, 20.0, 320.0), (95.0, 245.0));
    assert_span(run_y(&image, 170.0, 240.0, 262.0), (254.0, 260.0));
    assert!(run_y(&image, 317.0, 20.0, 260.0).is_none());

    // Both axes: FID-CMP-031-2 geometry, tracks shortened by the thickness.
    let mut f = classic((600.0, 960.0), grey(), false);
    f.handle.set_offset(point(px(-150.0), px(-360.0)));
    f.host.paint_frame();
    let image = f.host.screenshot_image().unwrap();
    assert_span(run_y(&image, 317.0, 20.0, 260.0), (108.0, 165.0));
    assert_span(run_x(&image, 257.0, 20.0, 320.0), (94.0, 240.0));
}

#[test]
fn classic_a2_the_thumb_moves_with_programmatic_scrolling_in_the_same_frame() {
    require_backend!();
    let mut f = classic((300.0, 960.0), grey(), false);
    let image = f.host.screenshot_image().unwrap();
    assert_span(run_y(&image, 317.0, 20.0, 260.0), (20.0, 80.0));
    for (offset, top) in [(-360.0, 110.0), (-720.0, 200.0), (-180.0, 65.0)] {
        f.handle.set_offset(point(px(0.0), px(offset)));
        // One frame is enough: the thumb derives from the offset clamped in this frame.
        f.host.paint_frame();
        let image = f.host.screenshot_image().unwrap();
        assert_span(run_y(&image, 317.0, 20.0, 260.0), (top, top + 60.0));
        let _ = offset;
    }
    // Offsets beyond the end clamp, and the thumb reaches the end of the track.
    f.handle.set_offset(point(px(0.0), px(-5000.0)));
    f.host.paint_frame();
    let image = f.host.screenshot_image().unwrap();
    assert_span(run_y(&image, 317.0, 20.0, 260.0), (200.0, 260.0));
}

#[test]
fn classic_a2_no_color_or_an_invalid_color_disables_all_painting() {
    require_backend!();
    let invalid = [
        None,
        Some(hsla(f32::NAN, 0.5, 0.5, 1.0)),
        Some(hsla(0.0, 0.0, 0.6, 0.0)),
    ];
    for color in invalid {
        let mut f = classic((600.0, 960.0), color, false);
        f.handle.set_offset(point(px(-150.0), px(-360.0)));
        f.host.paint_frame();
        let image = f.host.screenshot_image().unwrap();
        assert!(run_y(&image, 317.0, 20.0, 260.0).is_none(), "{color:?}");
        assert!(run_x(&image, 257.0, 20.0, 320.0).is_none(), "{color:?}");
        // Not even reserved: the scroll box keeps its full extent.
        assert_eq!(pixel(&image, 317.0, 100.0), VIEW_BG);
    }
}

#[test]
fn classic_a2_indicators_never_alter_layout_or_receive_hits() {
    require_backend!();
    let mut with = classic((300.0, 960.0), grey(), false);
    let without = classic((300.0, 960.0), None, false);
    // The content is as wide as the viewport either way: nothing is reserved.
    let a = with.handle.max_offset();
    let b = without.handle.max_offset();
    assert_eq!((a.x, a.y), (b.x, b.y));
    assert_eq!(a.x, px(0.0));
    assert_eq!(a.y, px(720.0));
    // A press on the thumb passes through to the container underneath.
    let image = with.host.screenshot_image().unwrap();
    assert_eq!(pixel(&image, 317.0, 40.0), THUMB);
    with.host.click(317.0, 40.0, 0, "").unwrap();
    assert_eq!(with.hits.get(), 1);
    // The wheel passes through too.
    with.host.scroll_wheel(317.0, 40.0, 0.0, -50.0, "").unwrap();
    with.host.paint_frame();
    assert_eq!(with.handle.offset().y, px(-50.0));
}

#[test]
fn classic_thumbs_are_anchored_to_the_padding_box_inside_a_border() {
    require_backend!();
    let mut f = classic((300.0, 960.0), grey(), true);
    f.handle.set_offset(point(px(0.0), px(-100.0)));
    f.host.paint_frame();
    let image = f.host.screenshot_image().unwrap();
    // The 2 px border moves the viewport's padding box inward: x = 22 + 296 - 6.
    let (x0, x1) = run_x(&image, 60.0, 300.0, 322.0).expect("thumb");
    assert_eq!((x0, x1), (312.0, 318.0));
}

// A long variable-height virtual list reaches its endpoint at the last item (A3).

struct ClassicList {
    state: ListState,
}

impl Render for ClassicList {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().relative().bg(rgb(0x101010)).child(
            list(self.state.clone(), |ix, _, _| {
                div()
                    .h(px(20.0 + (ix % 7) as f32 * 9.0))
                    .w_full()
                    .bg(rgb(0x202020))
                    .into_any_element()
            })
            .scrollbar_color(rgb(0xa0a0a0))
            .absolute()
            .left(px(20.0))
            .top(px(20.0))
            .w(px(300.0))
            .h(px(240.0)),
        )
    }
}

#[test]
fn classic_a3_a_long_variable_height_list_has_a_proportional_thumb_and_reaches_the_end() {
    require_backend!();
    let state = ListState::new(400, ListAlignment::Top, px(200.0));
    let view_state = state.clone();
    let mut host = TestHost::with_view(HostOptions::with_size(360.0, 300.0), move |_, cx| {
        cx.new(|_| ClassicList { state: view_state })
    })
    .expect("host");
    host.flush();
    let image = host.screenshot_image().unwrap();
    let (top, bottom) = run_y(&image, 317.0, 20.0, 260.0).expect("thumb");
    assert!((top - 20.0).abs() <= 1.0);
    assert!(
        bottom - top >= 18.0 && bottom - top < 240.0,
        "{top} {bottom}"
    );

    state.scroll_to(ListOffset {
        item_ix: 399,
        offset_in_item: px(0.0),
    });
    host.paint_frame();
    host.paint_frame();
    eprintln!(
        "final: {:?} {:?}",
        state.logical_scroll_top(),
        state.scrollbar_extents()
    );
    let image = host.screenshot_image().unwrap();
    let (_, bottom) = run_y(&image, 317.0, 20.0, 262.0).expect("thumb");
    assert!(
        (bottom - 260.0).abs() <= 1.0,
        "the thumb reaches the end at the final item, got {bottom}"
    );
}

// ---------------------------------------------------------------------------------------------
// Overlay rail.
// ---------------------------------------------------------------------------------------------

struct RailView {
    handle: ScrollHandle,
    bar: RailBar,
    hits: Rc<Cell<u32>>,
}

impl Render for RailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hits = self.hits.clone();
        let host: Rc<dyn RailHost> =
            Rc::new(HandleRail::new(self.handle.clone(), self.bar.clone()));
        let rail = render_rail("rail", host, RailOrientation::Vertical, window, cx);
        div().size_full().relative().bg(rgb(0x101010)).child(
            div()
                .id("wrap")
                .absolute()
                .left(px(20.0))
                .top(px(20.0))
                .w(px(200.0))
                .h(px(300.0))
                .bg(rgb(0x202020))
                .child(
                    div()
                        .id("scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.handle)
                        .on_mouse_down(MouseButton::Left, move |_, _, _| hits.set(hits.get() + 1))
                        .child(div().flex_none().w_full().h(px(900.0))),
                )
                .children(rail),
        )
    }
}

struct RailFixture {
    host: TestHost,
    handle: ScrollHandle,
    bar: RailBar,
    hits: Rc<Cell<u32>>,
}

fn rail_fixture() -> RailFixture {
    let handle = ScrollHandle::new();
    let bar = RailBar::new();
    let hits = Rc::new(Cell::new(0));
    let (h, b, c) = (handle.clone(), bar.clone(), hits.clone());
    let mut host = TestHost::with_view(HostOptions::with_size(300.0, 400.0), move |_, cx| {
        cx.new(|_| RailView {
            handle: h,
            bar: b,
            hits: c,
        })
    })
    .expect("host");
    host.motion_set(1000.0).unwrap();
    host.flush();
    RailFixture {
        host,
        handle,
        bar,
        hits,
    }
}

impl RailFixture {
    fn frame(&mut self) -> RgbaImage {
        self.host.paint_frame();
        self.host.screenshot_image().unwrap()
    }

    fn scroll_to(&mut self, y: f32) {
        self.handle.set_offset(point(px(0.0), px(-y)));
        self.host.paint_frame();
    }

    /// The horizontal extent and the strongest pixel of the rail's thumb on logical row `y`.
    fn thumb_on_row(&mut self, y: f32) -> Option<(f32, f32, f32)> {
        let image = self.frame();
        let mut x0 = None;
        let mut x1 = 0.0;
        let mut strongest = 0u8;
        let mut x = 200.0;
        while x < 222.0 {
            let p = pixel(&image, x + 0.25, y);
            if p[0] > VIEW_BG[0] + 3 {
                x0.get_or_insert(x);
                x1 = x + 0.5;
                strongest = strongest.max(p[0]);
            }
            x += 0.5;
        }
        // Opacity of the grey (0.62 lightness: about 158) blended over 0x20.
        let alpha = (strongest as f32 - VIEW_BG[0] as f32) / (158.0 - VIEW_BG[0] as f32);
        x0.map(|x0| (x0, x1, alpha))
    }
}

#[test]
fn a6_a7_a_hidden_rail_has_no_strip_and_cannot_be_hit() {
    require_backend!();
    let mut f = rail_fixture();
    // Initial mount: only a baseline, nothing shown and nothing hit-testable.
    assert!(f.thumb_on_row(60.0).is_none());
    f.host.click(215.0, 60.0, 0, "").unwrap();
    assert_eq!(f.hits.get(), 1, "the press reached the content");
    // Hovering the idle list reveals nothing either.
    f.host.mouse_move(210.0, 60.0, None, "").unwrap();
    assert!(f.thumb_on_row(60.0).is_none());
    assert!(!f.bar.with(|s| s.bar_hovered()));
}

#[test]
fn a6_scrolling_reveals_the_rail_which_lingers_fades_and_disappears() {
    require_backend!();
    let mut f = rail_fixture();
    f.scroll_to(100.0);
    // Visible at full opacity right away (50% of the faint foreground at rest).
    let (x0, x1, alpha) = f.thumb_on_row(60.0).expect("rail visible after scrolling");
    assert_eq!((x0, x1), (215.0, 218.0), "3 px thumb, 2 px from the edge");
    assert!((alpha - 0.5).abs() < 0.04, "alpha {alpha}");
    assert!(f.bar.with(|s| s.wake_pending()), "one wake is in flight");

    f.host.motion_set(1000.0 + 1400.0).unwrap();
    let (_, _, alpha) = f
        .thumb_on_row(60.0)
        .expect("still visible at the linger end");
    assert!((alpha - 0.5).abs() < 0.04, "alpha {alpha}");
    f.host.motion_set(1000.0 + 1530.0).unwrap();
    let (_, _, alpha) = f.thumb_on_row(60.0).expect("fading");
    assert!((alpha - 0.25).abs() < 0.04, "alpha {alpha}");
    f.host.motion_set(1000.0 + 1660.0).unwrap();
    assert!(f.thumb_on_row(60.0).is_none(), "gone after 1660 ms");
    // And gone means no strip: presses reach the content again.
    f.host.click(215.0, 60.0, 0, "").unwrap();
    assert_eq!(f.hits.get(), 1);
}

#[test]
fn a8_hovering_the_strip_widens_and_brightens_the_thumb_without_changing_the_strip() {
    require_backend!();
    let mut f = rail_fixture();
    f.scroll_to(100.0);
    f.host.motion_set(1000.0 + 1000.0).unwrap();
    f.host.paint_frame();
    // Pointer on the strip but not on the thumb: 5 px at 68%.
    f.host.mouse_move(211.0, 300.0, None, "").unwrap();
    let (x0, x1, alpha) = f.thumb_on_row(60.0).expect("visible");
    assert_eq!((x0, x1), (213.0, 218.0), "5 px thumb");
    assert!((alpha - 0.68).abs() < 0.04, "alpha {alpha}");
    // Directly over the thumb: 85%.
    f.host.mouse_move(216.0, 60.0, None, "").unwrap();
    let (_, _, alpha) = f.thumb_on_row(60.0).expect("visible");
    assert!((alpha - 0.85).abs() < 0.04, "alpha {alpha}");
    // The strip is 10 px wide however wide the thumb is: x = 210 is on it, 209 is not.
    let before = f.hits.get();
    f.host.click(209.0, 300.0, 0, "").unwrap();
    assert_eq!(f.hits.get(), before + 1);
    let before = f.hits.get();
    f.host.click(211.0, 300.0, 0, "").unwrap();
    assert_eq!(f.hits.get(), before, "the strip consumes the press");
    // Hovering froze the countdown: long after the linger it is still there.
    f.host.mouse_move(211.0, 300.0, None, "").unwrap();
    f.host.paint_frame();
    f.host.motion_set(1000.0 + 30_000.0).unwrap();
    // (The press scrolled to the pointer, so look for the thumb anywhere along the track.)
    let image = f.frame();
    let any = (24..316)
        .step_by(4)
        .any(|y| pixel(&image, 216.25, y as f32 + 0.25)[0] > VIEW_BG[0] + 3);
    assert!(any, "the hovered rail stays open");
}

#[test]
fn a5_pressing_the_track_centers_the_thumb_and_scrolls_at_press_time_and_drags_continue_outside() {
    require_backend!();
    let mut f = rail_fixture();
    f.scroll_to(100.0);
    f.host.mouse_move(211.0, 300.0, None, "").unwrap();
    f.frame();
    // Viewport 300, content 900, max 600; track 292, thumb 292 x 300 / 900 = 97.33 (>= 24).
    // Press the track at window y = 20 + 4 + 146 = 170 (the track center): the grab is half the
    // thumb, so the target is 146 - 48.67 = 97.33 of travel 194.67: half of the range.
    f.host.mouse_down(215.0, 170.0, 0, "").unwrap();
    assert!(f.bar.with(|s| s.dragging()));
    let y = -f32::from(f.handle.offset().y);
    assert!((y - 300.0).abs() < 1.0, "scrolled at press time to {y}");
    f.frame();
    // Drag far below the rail, outside its strip and the wrapper: clamps at the maximum.
    f.host.mouse_move(40.0, 390.0, Some(0), "").unwrap();
    let y = -f32::from(f.handle.offset().y);
    assert!((y - 600.0).abs() < 0.01, "clamped at the maximum, got {y}");
    f.frame();
    f.host.mouse_move(40.0, -50.0, Some(0), "").unwrap();
    assert_eq!(f.handle.offset().y, px(0.0), "clamped at zero");
    // Release outside the rail ends the drag, which then lingers.
    f.host.mouse_up(40.0, 390.0, 0, "").unwrap();
    assert!(!f.bar.with(|s| s.dragging()));
    f.frame();
    f.host.mouse_move(100.0, 390.0, Some(0), "").unwrap();
    assert_eq!(f.handle.offset().y, px(0.0), "no drag after the release");
    assert!(
        f.bar.with(|s| s.visible(f.host.motion_now())),
        "the rail lingers"
    );
}

#[test]
fn a5_pressing_the_thumb_keeps_the_grab_point_under_the_pointer() {
    require_backend!();
    let mut f = rail_fixture();
    f.scroll_to(300.0);
    f.host.mouse_move(216.0, 150.0, None, "").unwrap();
    f.frame();
    // Thumb at scroll 300 of 600: origin 97.33 of the track; its window top = 24 + 97.33.
    let thumb_top = 24.0 + 97.33;
    f.host.mouse_down(215.0, thumb_top + 10.0, 0, "").unwrap();
    let y = -f32::from(f.handle.offset().y);
    assert!((y - 300.0).abs() < 1.0, "unchanged by the press, got {y}");
    f.frame();
    f.host
        .mouse_move(215.0, thumb_top + 10.0 + 19.467, Some(0), "")
        .unwrap();
    let y = -f32::from(f.handle.offset().y);
    assert!(
        (y - 360.0).abs() < 1.5,
        "a 10th of the travel moves 60 of 600, got {y}"
    );
    f.host.mouse_up(215.0, 200.0, 0, "").unwrap();
}

// ---------------------------------------------------------------------------------------------
// A9: virtual-list drag freezes the reported content height.
// ---------------------------------------------------------------------------------------------

struct RailList {
    state: ListState,
    bar: RailBar,
}

impl Render for RailList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host: Rc<dyn RailHost> = Rc::new(ListRail::new(self.state.clone(), self.bar.clone()));
        let rail = render_rail("list-rail", host, RailOrientation::Vertical, window, cx);
        div()
            .size_full()
            .relative()
            .bg(rgb(0x101010))
            .child(
                list(self.state.clone(), |ix, _, _| {
                    div()
                        .h(px(10.0 + ((ix * 37) % 11) as f32 * 12.0))
                        .w_full()
                        .into_any_element()
                })
                .absolute()
                .left(px(20.0))
                .top(px(20.0))
                .w(px(200.0))
                .h(px(300.0)),
            )
            .children(rail.map(|rail| {
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(20.0))
                    .w(px(200.0))
                    .h(px(300.0))
                    .child(rail)
            }))
    }
}

#[test]
fn a9_the_content_height_reported_to_the_scrollbar_is_frozen_while_dragging() {
    require_backend!();
    let state = ListState::new(2000, ListAlignment::Top, px(50.0));
    let bar = RailBar::new();
    let (s, b) = (state.clone(), bar.clone());
    let mut host = TestHost::with_view(HostOptions::with_size(300.0, 400.0), move |_, cx| {
        cx.new(|_| RailList { state: s, bar: b })
    })
    .expect("host");
    host.flush();
    let rail = ListRail::new(state.clone(), bar);

    let before = rail
        .rail_metrics(RailOrientation::Vertical)
        .expect("overflows");
    assert!(!state.is_scrollbar_dragging());
    rail.rail_press();
    assert!(state.is_scrollbar_dragging());
    let frozen = state.scrollbar_extents().0;
    assert_eq!(frozen, state.max_offset_for_scrollbar().y);

    // Drag through the list: new rows get measured, the height stays constant.
    for fraction in [0.1, 0.35, 0.6, 0.9, 0.2] {
        rail.rail_drag_to(RailOrientation::Vertical, fraction);
        host.paint_frame();
        host.paint_frame();
        assert_eq!(state.scrollbar_extents().0, frozen, "fraction {fraction}");
        // The list offset was set from the scrollbar with the same height.
        let position = -f32::from(state.scroll_px_offset_for_scrollbar().y);
        assert!(
            (position - f32::from(frozen) * fraction).abs() < 40.0,
            "position {position} for fraction {fraction} of {frozen:?}"
        );
    }
    rail.rail_release();
    assert!(!state.is_scrollbar_dragging());
    // Without a drag the extents are recomputed from the rows measured since.
    let after = rail
        .rail_metrics(RailOrientation::Vertical)
        .expect("overflows");
    assert!(after.max_scroll > 0.0 && before.max_scroll > 0.0);
}
