//! Tests for DEV-005: committed paint geometry observation (`on_painted`).

use crate::{
    self as vespertine, AppContext as _, Bounds, Context, Div, Entity, InteractiveElement,
    IntoElement, ListAlignment, ListOffset, ListState, ParentElement, Pixels, Render,
    StyleRefinement, Styled, TestAppContext, Window, black, canvas, div, list, point, px, size,
    svg,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16"/></svg>"#;

type Reports = Rc<RefCell<Vec<Bounds<Pixels>>>>;

/// What the scene's root observed about the frame being built.
#[derive(Debug, Default, PartialEq)]
struct Observed {
    /// Bounds of the root's children after prepaint: before, leaf, after.
    children: Vec<Bounds<Pixels>>,
    /// Dispatch nodes and hitboxes of the frame once the root finished painting.
    nodes: usize,
    hitboxes: usize,
}

/// A column of a sibling, the observed leaf vector image (absolutely positioned at (8, 4),
/// 16 x 16) and another sibling. The root records the shape of the frame it is part of.
fn scene(reports: Option<&Reports>, hidden: bool, observed: &Rc<RefCell<Observed>>) -> Div {
    let mut leaf = svg()
        .data(SVG)
        .text_color(black())
        .absolute()
        .left(px(8.))
        .top(px(4.))
        .size(px(16.));
    if hidden {
        leaf = leaf.invisible();
    }
    if let Some(reports) = reports {
        let reports = reports.clone();
        leaf = leaf.on_painted(move |bounds, _, _| reports.borrow_mut().push(bounds));
    }
    let children = observed.clone();
    let frame = observed.clone();
    div()
        .relative()
        .size(px(100.))
        .flex()
        .flex_col()
        .on_children_prepainted(move |bounds, _, _| {
            children.borrow_mut().children = bounds;
        })
        .on_painted(move |_, window, _| {
            let mut observed = frame.borrow_mut();
            observed.nodes = window.next_frame.dispatch_tree.len();
            observed.hitboxes = window.next_frame.hitboxes.len();
        })
        .child(div().h(px(10.)))
        .child(leaf)
        .child(div().h(px(12.)))
}

fn draw_scene(
    cx: &mut crate::VisualTestContext,
    reports: Option<&Reports>,
    hidden: bool,
) -> Observed {
    let observed = Rc::new(RefCell::new(Observed::default()));
    cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
        scene(reports, hidden, &observed)
    });
    observed.take()
}

#[vespertine::test]
fn leaf_vector_image_reports_its_own_bounds(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let reports = Reports::default();
    draw_scene(cx, Some(&reports), false);
    assert_eq!(
        reports.borrow().as_slice(),
        &[Bounds {
            origin: point(px(8.), px(4.)),
            size: size(px(16.), px(16.)),
        }]
    );
}

#[vespertine::test]
fn hidden_leaf_reports_nothing(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let reports = Reports::default();
    draw_scene(cx, Some(&reports), true);
    draw_scene(cx, Some(&reports), true);
    assert!(reports.borrow().is_empty());

    draw_scene(cx, Some(&reports), false);
    assert_eq!(reports.borrow().len(), 1);
}

#[vespertine::test]
fn observer_changes_neither_layout_nor_tree_shape(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let reports = Reports::default();
    let plain = draw_scene(cx, None, false);
    let observed = draw_scene(cx, Some(&reports), false);

    let leaf = Bounds {
        origin: point(px(8.), px(4.)),
        size: size(px(16.), px(16.)),
    };
    assert_eq!(plain.children.len(), 3);
    assert_eq!(plain.children[1], leaf);
    assert_eq!(
        plain.children, observed.children,
        "leaf and sibling bounds are unchanged by the observer"
    );
    assert!(plain.nodes > 0);
    assert_eq!(plain.nodes, observed.nodes, "no wrapper node was inserted");
    assert_eq!(
        plain.hitboxes, observed.hitboxes,
        "the observer requires no hitbox on a leaf without pointer listeners"
    );
    assert_eq!(reports.borrow().as_slice(), &[leaf]);
}

#[vespertine::test]
fn each_paint_reports_once(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let reports = Reports::default();
    draw_scene(cx, Some(&reports), false);
    assert_eq!(reports.borrow().len(), 1);
    // A second paint of a freshly built element is a second actual paint.
    draw_scene(cx, Some(&reports), false);
    assert_eq!(reports.borrow().len(), 2);
    assert_eq!(reports.borrow()[0], reports.borrow()[1]);
}

#[vespertine::test]
fn rolled_back_list_prepaint_reports_only_the_final_rows(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let state = ListState::new(6, ListAlignment::Top, px(10.));
    // Starting at item 2 makes the first prepaint cover items 2..5; item 2 then requests an
    // autoscroll that moves the list up, so that range is discarded.
    state.scroll_to(ListOffset {
        item_ix: 2,
        offset_in_item: px(0.),
    });
    let reports: Rc<RefCell<Vec<(usize, Bounds<Pixels>)>>> = Rc::default();
    let item_two_prepaints = Rc::new(Cell::new(0));

    struct TestView {
        state: ListState,
        reports: Rc<RefCell<Vec<(usize, Bounds<Pixels>)>>>,
        item_two_prepaints: Rc<Cell<usize>>,
    }

    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let reports = self.reports.clone();
            let item_two_prepaints = self.item_two_prepaints.clone();
            list(self.state.clone(), move |ix, _, _| {
                let reports = reports.clone();
                let row = div()
                    .h(px(20.))
                    .w_full()
                    .on_painted(move |bounds, _, _| reports.borrow_mut().push((ix, bounds)));
                if ix == 2 {
                    let item_two_prepaints = item_two_prepaints.clone();
                    // Autoscrolling to 30px above this row forces the list to discard the
                    // row range it prepainted and lay out a different one.
                    row.child(
                        canvas(
                            move |bounds, window, _| {
                                item_two_prepaints.set(item_two_prepaints.get() + 1);
                                window.request_autoscroll(Bounds::from_corners(
                                    point(bounds.left(), bounds.top() - px(30.)),
                                    point(bounds.right(), bounds.top() + px(5.)),
                                ));
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .into_any_element()
                } else {
                    row.into_any_element()
                }
            })
            .w_full()
            .h_full()
        }
    }

    let view_state = state.clone();
    let view_reports = reports.clone();
    let view_prepaints = item_two_prepaints.clone();
    cx.draw(
        point(px(0.), px(0.)),
        size(px(100.), px(60.)),
        move |_, cx| {
            cx.new(|_| TestView {
                state: view_state,
                reports: view_reports,
                item_two_prepaints: view_prepaints,
            })
            .into_any_element()
        },
    );

    assert!(
        item_two_prepaints.get() >= 2,
        "the first row range was prepainted and discarded (item 2 prepainted {} times)",
        item_two_prepaints.get()
    );

    let scroll_top = state.logical_scroll_top();
    let origin_y = -scroll_top.offset_in_item;
    let mut reports = reports.borrow().clone();
    reports.sort_by_key(|(ix, _)| *ix);
    let expected: Vec<(usize, Bounds<Pixels>)> = (scroll_top.item_ix..6)
        .map(|ix| {
            let top = origin_y + px(20.) * (ix - scroll_top.item_ix) as f32;
            (
                ix,
                Bounds {
                    origin: point(px(0.), top),
                    size: size(px(100.), px(20.)),
                },
            )
        })
        .filter(|(_, bounds)| bounds.origin.y < px(60.))
        .collect();
    assert_eq!(
        reports, expected,
        "exactly the finally painted rows report, once each, at their final positions"
    );
    assert_eq!(scroll_top.item_ix, 0);
    assert_eq!(scroll_top.offset_in_item, px(10.));
}

struct CachedRoot {
    child: Entity<CachedChild>,
}

impl Render for CachedRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.child
            .clone()
            .cached(StyleRefinement::default().size(px(50.)))
    }
}

struct CachedChild {
    paints: Rc<Cell<usize>>,
}

impl Render for CachedChild {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let paints = self.paints.clone();
        div()
            .size_full()
            .on_painted(move |_, _, _| paints.set(paints.get() + 1))
    }
}

#[vespertine::test]
fn reused_cached_paint_invents_no_report(cx: &mut TestAppContext) {
    let paints = Rc::new(Cell::new(0));
    let child_paints = paints.clone();
    let (root, cx) = cx.add_window_view(|_, cx| CachedRoot {
        child: cx.new(|_| CachedChild {
            paints: child_paints,
        }),
    });
    let draw = |cx: &mut crate::VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    };

    draw(cx);
    let first = paints.get();
    assert!(first >= 1, "the first frame paints the child");

    for _ in 0..5 {
        draw(cx);
    }
    assert_eq!(
        paints.get(),
        first,
        "frames that reuse the cached paint range report nothing"
    );

    let child = root.read_with(cx, |root, _| root.child.clone());
    child.update(cx, |_, cx| cx.notify());
    draw(cx);
    assert_eq!(paints.get(), first + 1, "a repaint reports once");
}
