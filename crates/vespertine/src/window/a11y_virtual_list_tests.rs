//! A11Y-002 A3, virtual-list case: row replay (PRF-005) is bypassed while accessibility is
//! active, so the accessibility tree of a window whose rows were retained before activation
//! equals the tree of an ordinary window and holds the visible rows only.

use crate::{
    AnyWindowHandle, AppContext as _, Context, InputEvent as _, IntoElement, ListAlignment,
    ListState, Modifiers, ParentElement as _, Render, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, TouchPhase,
    UniformListScrollHandle, Window, div, list, point, px, uniform_list,
};
use accesskit::Role;

const ROWS: usize = 80;
const ROW_HEIGHT: f32 = 20.;
const VIEWPORT: f32 = 100.;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Uniform,
    Variable,
}

struct Rows {
    kind: Kind,
    scroll: UniformListScrollHandle,
    state: ListState,
}

fn row(ix: usize) -> crate::AnyElement {
    use crate::InteractiveElement as _;
    div()
        .id(("row", ix))
        .role(Role::ListItem)
        .aria_label(format!("row {ix}"))
        .h(px(ROW_HEIGHT))
        .child(format!("row {ix}"))
        .into_any_element()
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = match self.kind {
            Kind::Uniform => uniform_list("rows", ROWS, |range, _, _| {
                range.map(row).collect::<Vec<_>>()
            })
            .track_scroll(&self.scroll)
            .h(px(VIEWPORT))
            .into_any_element(),
            Kind::Variable => list(self.state.clone(), |ix, _, _| row(ix))
                .id("rows")
                .w_full()
                .h(px(VIEWPORT))
                .into_any_element(),
        };
        div().size_full().child(rows)
    }
}

fn open(cx: &mut TestAppContext, kind: Kind, retained: bool) -> AnyWindowHandle {
    let handle = cx.add_window(move |_, _| Rows {
        kind,
        scroll: UniformListScrollHandle::new(),
        state: ListState::new(ROWS, ListAlignment::Top, px(0.)).measure_all(),
    });
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, _| {
        window.set_retention_enabled(retained)
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

fn scroll(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) {
    let wheel = ScrollWheelEvent {
        touch_phase: TouchPhase::Moved,
        modifiers: Modifiers::default(),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        position: point(px(10.), px(10.)),
    };
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(wheel.to_platform_input(), cx)
    })
    .unwrap();
    draw(cx, window);
}

/// The labels of the rows in the last tree the platform received, with the tree itself
/// (node ids replaced by the order of their appearance).
fn rows_in_tree(cx: &mut TestAppContext, window: AnyWindowHandle) -> (Vec<String>, String) {
    cx.update_window(window, |_, window, _| {
        let update = window
            .platform_window
            .as_test()
            .expect("a test window")
            .last_accessibility_update()
            .expect("the platform received a tree update");
        let mut labels: Vec<String> = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::ListItem)
            .filter_map(|(_, node)| node.label().map(str::to_owned))
            .collect();
        labels.sort();
        let roles: Vec<_> = update
            .nodes
            .iter()
            .map(|(_, node)| (node.role(), node.label().map(str::to_owned)))
            .collect();
        (labels, format!("{roles:?}"))
    })
    .unwrap()
}

#[crate::test]
fn retained_rows_never_leak_overscan_nodes_into_the_tree(cx: &mut TestAppContext) {
    for kind in [Kind::Uniform, Kind::Variable] {
        let retained = open(cx, kind, true);
        let full = open(cx, kind, false);
        // Scroll while accessibility is off, so that rows beyond the viewport are retained.
        for window in [retained, full] {
            scroll(cx, window, -45.);
        }
        let rows_before = cx
            .update_window(retained, |_, window, _| window.frame_stats().rows_built)
            .unwrap();
        assert!(rows_before > 0, "{kind:?}: rows were built and retained");

        for window in [retained, full] {
            cx.update_window(window, |_, window, _| {
                window.set_a11y_active_for_tests(true)
            })
            .unwrap();
        }
        for step in 0..10 {
            for window in [retained, full] {
                scroll(cx, window, if step < 6 { -17. } else { 31. });
            }
            let (labels, tree) = rows_in_tree(cx, retained);
            let (full_labels, full_tree) = rows_in_tree(cx, full);
            assert_eq!(labels, full_labels, "{kind:?} step {step}");
            assert_eq!(tree, full_tree, "{kind:?} step {step}");
            assert!(
                labels.len() as f32 <= VIEWPORT / ROW_HEIGHT + 2.,
                "{kind:?} step {step}: only the visible rows are in the tree: {labels:?}"
            );
            assert!(!labels.is_empty(), "{kind:?} step {step}");
        }
        // Row replay did not run while accessibility was active.
        cx.update_window(retained, |_, window, _| window.reset_frame_stats())
            .unwrap();
        scroll(cx, retained, -9.);
        let stats = cx
            .update_window(retained, |_, window, _| window.frame_stats())
            .unwrap();
        assert_eq!(
            (stats.rows_built, stats.rows_replayed),
            (0, 0),
            "{kind:?}: {stats:?}"
        );
    }
}
