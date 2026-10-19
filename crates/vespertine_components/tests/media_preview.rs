//! CMP-020 R8: the inline preview.

mod media_support;

use media_support::{png, renderer};
use std::cell::Cell;
use std::rc::Rc;
use vespertine::{
    Bounds, Context, InteractiveElement, IntoElement, Modifiers, ParentElement, Pixels, Render,
    StatefulInteractiveElement, Styled, TestAppContext, Window, div, point, px, size,
};
use vespertine_components::media::{PreviewMedia, decode_image, preview_element};

struct Host {
    media: PreviewMedia,
    clicks: Rc<Cell<usize>>,
    outer: Rc<Cell<usize>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        let outer = self.outer.clone();
        div()
            .id("outer")
            .w(px(1000.))
            .on_click(move |_, _, _| outer.set(outer.get() + 1))
            .child(preview_element(self.media.clone(), "pv", move |_, _, _| {
                clicks.set(clicks.get() + 1)
            }))
    }
}

fn bounds_of(
    media: PreviewMedia,
    cx: &mut TestAppContext,
) -> (Bounds<Pixels>, Rc<Cell<usize>>, Rc<Cell<usize>>) {
    let clicks = Rc::new(Cell::new(0));
    let outer = Rc::new(Cell::new(0));
    let (_, vcx) = cx.add_window_view({
        let (clicks, outer) = (clicks.clone(), outer.clone());
        move |_, _| Host {
            media,
            clicks,
            outer,
        }
    });
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let bounds = vcx.debug_bounds("preview:pv").expect("preview bounds");
    let centre = bounds.center();
    vcx.simulate_click(centre, Modifiers::none());
    (bounds, clicks, outer)
}

#[vespertine::test]
fn the_preview_is_centered_bounded_by_natural_width_and_keeps_its_aspect(cx: &mut TestAppContext) {
    let media = decode_image("image/png", &png(400, 200), &renderer()).unwrap();
    let (bounds, clicks, outer) = bounds_of(PreviewMedia::Ready(media), cx);
    assert_eq!(bounds.size, size(px(400.), px(200.)));
    assert_eq!(bounds.origin.x, px(300.), "centered in the 1000 px column");
    assert_eq!(clicks.get(), 1, "a click opens the viewer once");
    assert_eq!(outer.get(), 0, "and stops propagation");
}

#[vespertine::test]
fn a_small_image_is_never_enlarged_and_a_tall_one_stops_at_480(cx: &mut TestAppContext) {
    let small = decode_image("image/png", &png(100, 50), &renderer()).unwrap();
    let (bounds, ..) = bounds_of(PreviewMedia::Ready(small), cx);
    assert_eq!(bounds.size, size(px(100.), px(50.)));
    let tall = decode_image("image/png", &png(900, 900), &renderer()).unwrap();
    let (bounds, ..) = bounds_of(PreviewMedia::Ready(tall), cx);
    assert_eq!(bounds.size.height, px(480.));
}

#[vespertine::test]
fn loading_and_failed_states_reserve_the_same_box_as_the_loaded_image(cx: &mut TestAppContext) {
    let natural = Some(size(px(400.), px(200.)));
    let (loading, ..) = bounds_of(
        PreviewMedia::Loading {
            natural_size: natural,
        },
        cx,
    );
    let (failed, clicks, _) = bounds_of(
        PreviewMedia::Failed {
            message: "boom".into(),
            natural_size: natural,
        },
        cx,
    );
    let media = decode_image("image/png", &png(400, 200), &renderer()).unwrap();
    let (loaded, ..) = bounds_of(PreviewMedia::Ready(media), cx);
    assert_eq!(loading, loaded);
    assert_eq!(failed, loaded);
    assert_eq!(clicks.get(), 1);
    let _ = point(px(0.), px(0.));
}
