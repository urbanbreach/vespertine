//! Tests proving the composable responsive layout contract (LAY-003): flex and
//! grid sizing, paint-time displacement, clipping, scrolling and unit
//! independence keep measured bounds, hitboxes and painted children consistent.

use crate::{
    AnyElement, App, AppContext as _, Bounds, Context, Element, ElementId, GlobalElementId,
    InputEvent as _, InspectorElementId, InteractiveElement as _, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point, Render, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, WindowHandle, div, point,
    px, rems,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Host {
    build: Rc<dyn Fn() -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        (self.build)()
    }
}

fn open(cx: &mut TestAppContext, build: impl Fn() -> AnyElement + 'static) -> WindowHandle<Host> {
    let build: Rc<dyn Fn() -> AnyElement> = Rc::new(build);
    cx.add_window(move |_, _| Host { build })
}

fn draw(cx: &mut TestAppContext, window: WindowHandle<Host>) {
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

/// The measured (and painted) bounds recorded for a debug selector.
fn bounds(
    cx: &mut TestAppContext,
    window: WindowHandle<Host>,
    selector: &'static str,
) -> Bounds<Pixels> {
    cx.update_window(window.into(), |_, window, _| {
        window
            .rendered_frame
            .debug_bounds
            .get(selector)
            .copied()
            .unwrap_or_else(|| panic!("no bounds recorded for {selector}"))
    })
    .unwrap()
}

fn mouse_down(cx: &mut TestAppContext, window: WindowHandle<Host>, position: Point<Pixels>) {
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                position,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
}

type Hits = Rc<RefCell<Vec<&'static str>>>;

fn recorder(hits: Hits, name: &'static str) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) {
    move |_, _, _| hits.borrow_mut().push(name)
}

fn assert_close(actual: Pixels, expected: f32) {
    assert!(
        (actual.0 - expected).abs() < 0.01,
        "expected {expected}, got {}",
        actual.0
    );
}

/// An element that translates its child in prepaint, as scrolling or dragging
/// elements do; layout is untouched.
struct Displaced {
    offset: Point<Pixels>,
    child: AnyElement,
}

impl Element for Displaced {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let offset = self.offset;
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl IntoElement for Displaced {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Reports the width Taffy solved for its child's layout node, before the
/// post-layout snapping to device pixels, in prepaint.
struct Probe {
    child: AnyElement,
    raw_width: Rc<Cell<f32>>,
}

impl Element for Probe {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let layout_id = self.child.request_layout(window, cx);
        (layout_id, layout_id)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout_id: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.raw_width
            .set(window.unsnapped_layout_size(*layout_id).width.0);
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl IntoElement for Probe {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

const LONG_TEXT: &str = "A very long label that would push its neighbours off screen if the \
    flexible cell were not allowed to shrink below its content width, repeated: A very long \
    label that would push its neighbours off screen if the flexible cell were not allowed to \
    shrink below its content width.";

/// The behavioral scene of the spec: a row of the given width with 12 pixel
/// gaps, fixed 100 pixel cells at both ends and a flexible, truncating center.
fn responsive_row(width: Rc<Cell<f32>>) -> impl Fn() -> AnyElement {
    move || {
        div()
            .w(px(width.get()))
            .h(px(120.))
            .flex()
            .gap(px(12.))
            .child(
                div()
                    .debug_selector(|| "left".into())
                    .flex_none()
                    .w(px(100.))
                    .h(px(120.)),
            )
            .child(
                div()
                    .debug_selector(|| "center".into())
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(
                        div()
                            .debug_selector(|| "label".into())
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(LONG_TEXT),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "right".into())
                    .flex_none()
                    .w(px(100.))
                    .h(px(120.)),
            )
            .into_any_element()
    }
}

#[vespertine::test]
fn flexible_center_shrinks_while_fixed_cells_keep_their_size(cx: &mut TestAppContext) {
    let width = Rc::new(Cell::new(760.));
    let window = open(cx, responsive_row(width.clone()));
    for (row_width, center) in [(760., 536.), (600., 376.), (400., 176.), (300., 76.)] {
        width.set(row_width);
        draw(cx, window);
        let left = bounds(cx, window, "left");
        let middle = bounds(cx, window, "center");
        let right = bounds(cx, window, "right");
        let label = bounds(cx, window, "label");
        assert_close(left.size.width, 100.);
        assert_close(right.size.width, 100.);
        assert_close(left.origin.x, 0.);
        assert_close(middle.origin.x, 112.);
        assert_close(middle.size.width, center);
        // The right cell stays on screen: its right edge is the row's edge.
        assert_close(right.origin.x + right.size.width, row_width);
        // The truncating label fits its cell instead of pushing the neighbours.
        assert_close(label.size.width, center);
    }
}

#[vespertine::test]
fn without_a_zero_minimum_the_flexible_cell_would_overflow(cx: &mut TestAppContext) {
    // The contrast case: the minimum width of zero is what lets the cell shrink.
    let window = open(cx, || {
        div()
            .w(px(400.))
            .flex()
            .gap(px(12.))
            .child(div().flex_none().w(px(100.)).h(px(20.)))
            .child(
                div()
                    .debug_selector(|| "greedy".into())
                    .flex_1()
                    .whitespace_nowrap()
                    .child(LONG_TEXT),
            )
            .into_any_element()
    });
    draw(cx, window);
    assert!(bounds(cx, window, "greedy").size.width.0 > 188.);
}

fn grid_cell(index: usize) -> AnyElement {
    let (name, content) = (CELLS[index], CONTENTS[index]);
    div()
        .debug_selector(move || name.into())
        .h(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .child(div().debug_selector(move || content.into()).size(px(20.)))
        .into_any_element()
}

#[vespertine::test]
fn three_equal_grid_columns_share_the_width_after_gaps(cx: &mut TestAppContext) {
    let raw_widths: Vec<Rc<Cell<f32>>> = (0..4).map(|_| Rc::new(Cell::new(0.))).collect();
    let window = open(cx, {
        let raw_widths = raw_widths.clone();
        move || {
            div()
                .w(px(760.))
                .h(px(110.))
                .grid()
                .grid_cols(3)
                .gap(px(10.))
                .children((0..4).map(|index| Probe {
                    child: grid_cell(index),
                    raw_width: raw_widths[index].clone(),
                }))
                .into_any_element()
        }
    });
    // (760 - 2 x 10) / 3 = 246 2/3 logical pixels per track.
    //
    // The layout engine solves the tracks exactly (Taffy's own rounding is off), but
    // element bounds are then snapped to whole device pixels so painted edges land on
    // physical pixels. The exact share is therefore observable in painted bounds only
    // at a scale factor where 246 2/3 is a whole number of device pixels (3); at other
    // scales the solved size (read before snapping) is still 246 2/3 while the painted
    // bounds are the nearest device pixels.
    let track = (760. - 20.) / 3.;
    let assert_solved_exactly = |raw_widths: &[Rc<Cell<f32>>]| {
        for raw in &raw_widths[..3] {
            assert_close(Pixels(raw.get()), track);
        }
    };
    cx.simulate_window_scale_factor_change(window.into(), 3.0);
    draw(cx, window);
    assert_solved_exactly(&raw_widths);
    for (index, column) in [0, 1, 2, 0].into_iter().enumerate() {
        let cell = bounds(cx, window, CELLS[index]);
        assert_close(cell.size.width, track);
        assert_close(cell.origin.x, column as f32 * (track + 10.));
        // Centered content.
        let content = bounds(cx, window, CONTENTS[index]);
        assert_close(content.origin.x, cell.origin.x.0 + (track - 20.) / 2.);
        assert_close(content.origin.y, cell.origin.y.0 + 10.);
    }
    // The fourth cell wraps to the second 50 pixel row, below the 10 pixel row gap.
    assert_close(bounds(cx, window, "cell3").origin.y, 60.);

    // At other scale factors the same shares are snapped to the device pixel grid:
    // each track stays within one device pixel of the exact share, the gaps are
    // exactly 10 pixels and the three tracks still span the 760 pixel container.
    for scale in [1.0, 2.0] {
        cx.simulate_window_scale_factor_change(window.into(), scale);
        draw(cx, window);
        assert_solved_exactly(&raw_widths);
        let cells: Vec<_> = CELLS[..3].iter().map(|c| bounds(cx, window, c)).collect();
        for cell in &cells {
            assert!(
                (cell.size.width.0 - track).abs() <= 1. / scale,
                "{scale}: {cell:?}"
            );
        }
        assert_close(cells[0].origin.x, 0.);
        assert_close(
            cells[1].origin.x - (cells[0].origin.x + cells[0].size.width),
            10.,
        );
        assert_close(
            cells[2].origin.x - (cells[1].origin.x + cells[1].size.width),
            10.,
        );
        assert_close(cells[2].origin.x + cells[2].size.width, 760.);
    }
}

const CELLS: [&str; 4] = ["cell0", "cell1", "cell2", "cell3"];
const CONTENTS: [&str; 4] = ["content0", "content1", "content2", "content3"];

#[vespertine::test]
fn paint_translated_child_hitboxes_follow_its_pixels(cx: &mut TestAppContext) {
    let hits: Hits = Rc::default();
    let offset = Rc::new(Cell::new(point(px(0.), px(0.))));
    let window = open(cx, {
        let (hits, offset) = (hits.clone(), offset.clone());
        move || {
            div()
                .size_full()
                .flex()
                .child(Displaced {
                    offset: offset.get(),
                    child: div()
                        .debug_selector(|| "displaced".into())
                        .w(px(40.))
                        .h(px(40.))
                        .on_mouse_down(MouseButton::Left, recorder(hits.clone(), "displaced"))
                        .into_any_element(),
                })
                .child(
                    div()
                        .debug_selector(|| "sibling".into())
                        .w(px(40.))
                        .h(px(40.))
                        .on_mouse_down(MouseButton::Left, recorder(hits.clone(), "sibling")),
                )
                .into_any_element()
        }
    });
    draw(cx, window);
    let sibling = bounds(cx, window, "sibling");
    assert_eq!(
        bounds(cx, window, "displaced").origin,
        point(px(0.), px(0.))
    );
    assert_eq!(sibling.origin, point(px(40.), px(0.)));

    offset.set(point(px(200.), px(100.)));
    draw(cx, window);
    // The siblings keep their measured positions; the displaced element's bounds
    // moved with its pixels.
    assert_eq!(bounds(cx, window, "sibling"), sibling);
    assert_eq!(
        bounds(cx, window, "displaced").origin,
        point(px(200.), px(100.))
    );

    // Its hitbox moved too: the old position no longer hits it ...
    mouse_down(cx, window, point(px(20.), px(20.)));
    assert!(hits.borrow().is_empty());
    // ... and the displaced position does.
    mouse_down(cx, window, point(px(220.), px(120.)));
    assert_eq!(*hits.borrow(), ["displaced"]);
    // The sibling's hitbox did not move.
    mouse_down(cx, window, point(px(60.), px(20.)));
    assert_eq!(*hits.borrow(), ["displaced", "sibling"]);
}

#[vespertine::test]
fn clipped_regions_do_not_hit_test(cx: &mut TestAppContext) {
    let hits: Hits = Rc::default();
    let window = open(cx, {
        let hits = hits.clone();
        move || {
            div()
                .size_full()
                .child(
                    div()
                        .debug_selector(|| "clip".into())
                        .w(px(50.))
                        .h(px(50.))
                        .overflow_hidden()
                        .child(
                            div()
                                .debug_selector(|| "wide".into())
                                .w(px(120.))
                                .h(px(80.))
                                .on_mouse_down(MouseButton::Left, recorder(hits.clone(), "wide")),
                        ),
                )
                .into_any_element()
        }
    });
    draw(cx, window);
    // The measured bounds are not clipped ...
    assert_close(bounds(cx, window, "wide").size.width, 120.);
    assert_close(bounds(cx, window, "wide").size.height, 80.);
    // ... but the clip rectangle bounds both painting and hit testing.
    mouse_down(cx, window, point(px(49.), px(10.)));
    assert_eq!(*hits.borrow(), ["wide"]);
    for position in [
        point(px(51.), px(10.)),
        point(px(100.), px(10.)),
        point(px(10.), px(51.)),
        point(px(10.), px(70.)),
    ] {
        mouse_down(cx, window, position);
    }
    assert_eq!(
        *hits.borrow(),
        ["wide"],
        "clipped-away regions get no events"
    );
}

#[vespertine::test]
fn logical_pixels_and_rem_units_are_independent(cx: &mut TestAppContext) {
    let window = open(cx, || {
        div()
            .size_full()
            .child(div().debug_selector(|| "px".into()).w(px(100.)).h(px(10.)))
            .child(
                div()
                    .debug_selector(|| "rem".into())
                    .w(rems(10.))
                    .h(rems(2.)),
            )
            .into_any_element()
    });
    let rem_size = |cx: &mut TestAppContext, size: f32| {
        cx.update_window(window.into(), |_, window, _| window.set_rem_size(px(size)))
            .unwrap();
        draw(cx, window);
    };
    draw(cx, window);
    let default_rem = cx
        .update_window(window.into(), |_, window, _| window.rem_size())
        .unwrap()
        .0;
    assert_close(bounds(cx, window, "px").size.width, 100.);
    assert_close(bounds(cx, window, "rem").size.width, 10. * default_rem);
    for size in [8., 20., 32.] {
        rem_size(cx, size);
        // Logical pixels do not change with the root font size; rems scale with it.
        assert_close(bounds(cx, window, "px").size.width, 100.);
        assert_close(bounds(cx, window, "px").size.height, 10.);
        assert_close(bounds(cx, window, "rem").size.width, 10. * size);
        assert_close(bounds(cx, window, "rem").size.height, 2. * size);
    }
}

/// A 100 x 100 scroller around a 300 x 300 content with a 30 x 30 target at
/// content position (150, 180).
fn scroller(handle: &ScrollHandle, hits: &Hits) -> AnyElement {
    let scroller = div()
        .id("scroller")
        .debug_selector(|| "scroller".into())
        .w(px(100.))
        .h(px(100.))
        .overflow_scroll()
        .track_scroll(handle);
    div()
        .size_full()
        .child(
            scroller.child(
                div().w(px(300.)).h(px(300.)).relative().child(
                    div()
                        .debug_selector(|| "target".into())
                        .absolute()
                        .left(px(150.))
                        .top(px(180.))
                        .w(px(30.))
                        .h(px(30.))
                        .on_mouse_down(MouseButton::Left, recorder(hits.clone(), "target")),
                ),
            ),
        )
        .into_any_element()
}

#[vespertine::test]
fn scrolled_children_keep_hitboxes_aligned_with_their_pixels(cx: &mut TestAppContext) {
    let hits: Hits = Rc::default();
    let handle = ScrollHandle::new();
    let window = open(cx, {
        let (handle, hits) = (handle.clone(), hits.clone());
        move || scroller(&handle, &hits)
    });
    draw(cx, window);
    // Unscrolled, the target lies outside the 100 x 100 viewport.
    assert_eq!(
        bounds(cx, window, "target").origin,
        point(px(150.), px(180.))
    );
    mouse_down(cx, window, point(px(40.), px(40.)));
    assert!(hits.borrow().is_empty());

    // Both axes at once: the target is painted at (150 - 120, 180 - 150).
    handle.set_offset(point(px(-120.), px(-150.)));
    draw(cx, window);
    assert_eq!(bounds(cx, window, "target").origin, point(px(30.), px(30.)));
    mouse_down(cx, window, point(px(40.), px(40.)));
    assert_eq!(*hits.borrow(), ["target"]);
    // Its old (unscrolled) pixels are no longer the target.
    mouse_down(cx, window, point(px(160.), px(190.)));
    mouse_down(cx, window, point(px(29.), px(40.)));
    mouse_down(cx, window, point(px(61.), px(40.)));
    assert_eq!(*hits.borrow(), ["target"]);

    // Scrolling back and forth on one axis keeps hitbox and paint aligned.
    handle.set_offset(point(px(-120.), px(0.)));
    draw(cx, window);
    assert_eq!(
        bounds(cx, window, "target").origin,
        point(px(30.), px(180.))
    );
    mouse_down(cx, window, point(px(40.), px(185.)));
    assert_eq!(*hits.borrow(), ["target"], "below the viewport clip");

    handle.set_offset(point(px(0.), px(-150.)));
    draw(cx, window);
    assert_eq!(
        bounds(cx, window, "target").origin,
        point(px(150.), px(30.))
    );
    mouse_down(cx, window, point(px(160.), px(40.)));
    assert_eq!(*hits.borrow(), ["target"], "right of the viewport clip");
    assert_eq!(handle.offset(), point(px(0.), px(-150.)));
}
