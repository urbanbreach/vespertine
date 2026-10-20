//! A11Y-002 A3, scrolled-block and synthetic-child cases: a retained scroll block (PRF-004)
//! replays its recorded accessibility nodes translated by the scroll, so the tree of a window
//! that composites the block equals the tree of a window that draws it directly; content with
//! a synthetic child whose bounds do not follow its owner is drawn again instead of replayed.

use crate::{
    AnyWindowHandle, AppContext as _, Context, InputEvent as _, InteractiveElement as _,
    IntoElement, Modifiers, ParentElement as _, Render, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase, Window, div, point,
    px,
};
use accesskit::Role;

const ROWS: usize = 60;
const ROW_HEIGHT: f32 = 40.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Synthetic {
    /// Rows contribute no synthetic child.
    None,
    /// Each row adds a leaf inside its own bounds.
    Inside,
    /// Each row adds a leaf outside its own bounds.
    Outside,
}

struct Scrolled {
    synthetic: Synthetic,
}

fn row(ix: usize, synthetic: Synthetic) -> impl IntoElement {
    let row = div()
        .id(("row", ix))
        .role(Role::ListItem)
        .aria_label(format!("row {ix}"))
        .h(px(ROW_HEIGHT))
        .w_full()
        .child(format!("Item {ix}"));
    match synthetic {
        Synthetic::None => row,
        kind => row.a11y_synthetic_children(move |builder| {
            let parent = builder.parent_node().bounds();
            let mut leaf = accesskit::Node::new(Role::Label);
            leaf.set_label(format!("leaf {ix}"));
            leaf.set_bounds(match (kind, parent) {
                (Synthetic::Inside, Some(parent)) => accesskit::Rect {
                    x0: parent.x0 + 2.,
                    y0: parent.y0 + 2.,
                    x1: parent.x0 + 12.,
                    y1: parent.y0 + 12.,
                },
                _ => accesskit::Rect {
                    x0: -80.,
                    y0: -80.,
                    x1: -60.,
                    y1: -60.,
                },
            });
            builder.push_child(builder.synthetic_node_id(0), leaf);
        }),
    }
}

impl Render for Scrolled {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let synthetic = self.synthetic;
        div().size_full().bg(crate::rgb(0xeeeeee)).child(
            div()
                .id("scroller")
                .w(px(300.))
                .h(px(200.))
                .bg(crate::white())
                .overflow_y_scroll()
                .children((0..ROWS).map(move |ix| row(ix, synthetic))),
        )
    }
}

fn open(cx: &mut TestAppContext, synthetic: Synthetic, layers: bool) -> AnyWindowHandle {
    let handle = cx.add_window(move |_, _| Scrolled { synthetic });
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, _| {
        window.set_scroll_layers_enabled(layers);
        window.set_a11y_active_for_tests(true);
    })
    .unwrap();
    for _ in 0..3 {
        draw(cx, window);
    }
    window
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn wheel(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) {
    let event = ScrollWheelEvent {
        position: point(px(50.), px(50.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    };
    // The test window draws a frame for the event itself.
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(event.to_platform_input(), cx)
    })
    .unwrap();
}

/// Every node of the last tree the platform received, in order, with its role, label, bounds
/// and number of children (node ids are left out: they are not comparable across windows).
fn tree(cx: &mut TestAppContext, window: AnyWindowHandle) -> Vec<String> {
    cx.update_window(window, |_, window, _| {
        let update = window
            .platform_window
            .as_test()
            .expect("a test window")
            .last_accessibility_update()
            .expect("the platform received a tree update");
        update
            .nodes
            .iter()
            .map(|(_, node)| {
                format!(
                    "{:?} {:?} {:?} children {}",
                    node.role(),
                    node.label(),
                    node.bounds(),
                    node.children().len()
                )
            })
            .collect()
    })
    .unwrap()
}

fn stats(cx: &mut TestAppContext, window: AnyWindowHandle) -> crate::FrameStats {
    cx.update_window(window, |_, window, _| window.frame_stats())
        .unwrap()
}

/// Scrolls both windows frame by frame (past promotion of the block to a layer) and compares
/// the trees after every frame. Returns the window that may use layers.
fn scroll_and_compare(cx: &mut TestAppContext, synthetic: Synthetic) -> AnyWindowHandle {
    let layered = open(cx, synthetic, true);
    let direct = open(cx, synthetic, false);
    let initial = tree(cx, layered);
    assert_eq!(initial, tree(cx, direct), "{synthetic:?}: before scrolling");
    for step in 0..16 {
        let dy = if step < 12 { -3.7 } else { 4.1 };
        for window in [layered, direct] {
            wheel(cx, window, dy);
        }
        let (retained, full) = (tree(cx, layered), tree(cx, direct));
        assert_eq!(retained, full, "{synthetic:?}: step {step}");
    }
    assert_ne!(
        tree(cx, layered),
        initial,
        "{synthetic:?}: scrolling moved the nodes"
    );
    layered
}

/// A scrolled block that is composited replays its recorded nodes translated by the scroll:
/// the tree equals the tree of a window that draws the block directly on every frame.
#[crate::test]
fn a_composited_scroll_block_replays_translated_nodes(cx: &mut TestAppContext) {
    for synthetic in [Synthetic::None, Synthetic::Inside] {
        let layered = scroll_and_compare(cx, synthetic);
        let layers = cx
            .update_window(layered, |_, window, _| window.scroll_layer_count())
            .unwrap();
        assert_eq!(layers, 1, "{synthetic:?}: the block is a layer");
        cx.update_window(layered, |_, window, _| window.reset_frame_stats())
            .unwrap();
        for _ in 0..6 {
            wheel(cx, layered, -2.9);
        }
        let stats = stats(cx, layered);
        assert!(
            stats.layer_composites >= 6 && stats.layer_repaints == 0,
            "{synthetic:?}: the nodes came from replaying the layer: {stats:?}"
        );
    }
}

/// A synthetic child whose bounds do not follow its owner makes the block draw its content
/// again on every frame instead of replaying it, and the tree still equals the direct one.
#[crate::test]
fn independent_synthetic_children_are_repainted_instead_of_replayed(cx: &mut TestAppContext) {
    let layered = scroll_and_compare(cx, Synthetic::Outside);
    let (layers, composites) = cx
        .update_window(layered, |_, window, _| {
            (
                window.scroll_layer_count(),
                window.frame_stats().layer_composites,
            )
        })
        .unwrap();
    assert_eq!(layers, 0, "no layer holds content with independent nodes");
    assert_eq!(composites, 0, "nothing was composited");
}
