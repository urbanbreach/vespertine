use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use super::*;
use crate::{
    Context, CursorStyle, DragMoveEvent, Empty, FileDropEvent, FocusHandle, KeyDownEvent,
    Keystroke, Modifiers, MouseButton, Render, ScrollHandle, Stateful, TestAppContext,
    VisualTestContext, Window, div, prelude::*, size,
};

fn pt(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

fn close(actual: Pixels, expected: f32) {
    assert!(
        (actual.0 - expected).abs() < 1e-3,
        "expected {expected}, got {}",
        actual.0
    );
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn press(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_down(pt(x, y), MouseButton::Left, Modifiers::default());
    draw(cx);
}

fn drag_to(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_move(pt(x, y), MouseButton::Left, Modifiers::default());
    draw(cx);
}

fn release(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_up(pt(x, y), MouseButton::Left, Modifiers::default());
    draw(cx);
}

fn has_drag(cx: &mut VisualTestContext) -> bool {
    cx.update(|_, cx| cx.has_active_drag())
}

fn drag_cursor(cx: &mut VisualTestContext) -> Option<CursorStyle> {
    cx.update(|_, cx| cx.active_drag_cursor_style())
}

fn frame_cursor(cx: &mut VisualTestContext) -> Option<CursorStyle> {
    cx.update(|window, _| window.rendered_frame.cursor_style(window))
}

fn advance(cx: &mut VisualTestContext, millis: u64) {
    cx.executor().advance_clock(Duration::from_millis(millis));
    cx.run_until_parked();
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("vespertine-drag-drop-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, b"contents").unwrap();
        path
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Restore permissions a test may have removed so the tree can be deleted.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(entries) = std::fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    let _ = std::fs::set_permissions(
                        entry.path(),
                        std::fs::Permissions::from_mode(0o755),
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------------------------
// A2: slot geometry
// ---------------------------------------------------------------------------------------------

#[test]
fn content_coordinate_subtracts_origin_scroll_and_padding() {
    // pointer 130, origin 100, scrolled 50 px toward the origin side (offset -50), padding 8.
    close(
        content_coordinate(px(130.), px(100.), px(-50.), px(8.)),
        130. - 100. + 50. - 8.,
    );
}

#[test]
fn uniform_slots_clamp_and_include_padding() {
    let (h, n) = (px(40.), 5);
    // coordinate = pointer - origin - scroll offset - leading padding (8).
    assert_eq!(uniform_slot(px(-3.), h, n), Some(0));
    assert_eq!(uniform_slot(px(39.9), h, n), Some(0));
    assert_eq!(uniform_slot(px(40.), h, n), Some(1));
    assert_eq!(uniform_slot(px(10000.), h, n), Some(4));
    assert_eq!(uniform_slot(px(10.), h, 0), None);
    // The leading padding (pointer within 8 px of the origin) is slot 0, the trailing padding the
    // last slot.
    let origin = px(100.);
    let coordinate = |pointer: f32| content_coordinate(px(pointer), origin, px(-30.), px(8.));
    assert_eq!(uniform_slot(coordinate(103.), h, n), Some(0));
    assert_eq!(uniform_slot(coordinate(180.), h, n), Some(2));
    assert_eq!(uniform_slot(coordinate(400.), h, n), Some(4));
}

#[test]
fn variable_slots_strict_and_clamped() {
    let rows = [px(29.), px(29.)];
    let gap = px(2.);
    assert_eq!(
        variable_slot(px(31.), &rows, gap, SlotMode::Strict),
        Some(1)
    );
    assert_eq!(
        variable_slot(px(31.), &rows, gap, SlotMode::Clamped),
        Some(1)
    );
    assert_eq!(variable_slot(px(61.), &rows, gap, SlotMode::Strict), None);
    assert_eq!(
        variable_slot(px(61.), &rows, gap, SlotMode::Clamped),
        Some(1)
    );
    assert_eq!(variable_slot(px(-1.), &rows, gap, SlotMode::Strict), None);
    assert_eq!(
        variable_slot(px(-1.), &rows, gap, SlotMode::Clamped),
        Some(0)
    );
    assert_eq!(variable_slot(px(0.), &rows, gap, SlotMode::Strict), Some(0));

    let rows = [px(45.), px(63.)];
    assert_eq!(
        variable_slot(px(48.), &rows, gap, SlotMode::Strict),
        Some(1)
    );
    assert_eq!(
        variable_slot(px(46.), &rows, gap, SlotMode::Strict),
        Some(0)
    );

    let rows = [px(29.), px(45.)];
    assert_eq!(
        variable_slot(px(500.), &rows, gap, SlotMode::Clamped),
        Some(1)
    );
    assert_eq!(variable_slot(px(0.), &[], gap, SlotMode::Clamped), None);
    assert_eq!(variable_slot(px(0.), &[], gap, SlotMode::Strict), None);
}

#[test]
fn column_slots_use_desired_width_space() {
    let widths = [px(100.), px(200.), px(100.)];
    assert_eq!(column_slot(px(49.), &widths, px(400.)), Some(0));
    assert_eq!(column_slot(px(51.), &widths, px(400.)), Some(1));
    assert_eq!(column_slot(px(399.), &widths, px(400.)), Some(2));
    // Rendered at half width, x 49 maps to x' 98.
    assert_eq!(column_slot(px(49.), &widths, px(200.)), Some(1));
    // Positions outside the rendered width are clamped.
    assert_eq!(column_slot(px(-20.), &widths, px(400.)), Some(0));
    assert_eq!(column_slot(px(900.), &widths, px(400.)), Some(2));
    assert_eq!(column_slot(px(10.), &[], px(400.)), None);
    assert_eq!(column_slot(px(10.), &widths, px(0.)), None);
}

// ---------------------------------------------------------------------------------------------
// A3: reorder
// ---------------------------------------------------------------------------------------------

#[test]
fn insertion_index_follows_removal_rule() {
    // From 0 over 2 of 3: after the target, final position 2.
    assert_eq!(insertion_index(0, 2, 3), Some(2));
    let mut items = vec!['a', 'b', 'c'];
    reorder_move(0, 2, 3).unwrap().apply(&mut items);
    assert_eq!(items, ['b', 'c', 'a']);

    // From 2 over 0: before the target.
    assert_eq!(insertion_index(2, 0, 3), Some(0));
    let mut items = vec!['a', 'b', 'c'];
    reorder_move(2, 0, 3).unwrap().apply(&mut items);
    assert_eq!(items, ['c', 'a', 'b']);

    // Adjacent moves swap.
    let mut items = vec!['a', 'b', 'c'];
    reorder_move(0, 1, 3).unwrap().apply(&mut items);
    assert_eq!(items, ['b', 'a', 'c']);

    // Dropping onto itself changes nothing.
    assert_eq!(insertion_index(1, 1, 3), None);
    assert_eq!(reorder_move(1, 1, 3), None);
    // The result is clamped to the list length.
    assert_eq!(insertion_index(0, 99, 3), Some(2));
    assert_eq!(insertion_index(5, 0, 3), None);
}

#[test]
fn slide_offsets_for_the_gap_preview() {
    // ix 0 dragged from 0, previously hovering slot 0, now over slot 2.
    assert_eq!(
        slide_transition(0, 0, 0, 2),
        SlideTransition { from: 0, to: 2 }
    );
    assert_eq!(
        slide_transition(1, 0, 0, 2),
        SlideTransition { from: 0, to: -1 }
    );
    assert_eq!(
        slide_transition(2, 0, 0, 2),
        SlideTransition { from: 0, to: -1 }
    );
    assert_eq!(
        slide_transition(3, 0, 0, 2),
        SlideTransition { from: 0, to: 0 }
    );

    // Moving back from slot 2 to slot 1: the dragged row goes (2 -> 1) slots, row 1 stays at -1,
    // row 2 returns to 0.
    assert_eq!(
        slide_transition(0, 0, 2, 1),
        SlideTransition { from: 2, to: 1 }
    );
    let row1 = slide_transition(1, 0, 2, 1);
    assert_eq!(row1, SlideTransition { from: -1, to: -1 });
    assert!(!row1.restarts());
    let row2 = slide_transition(2, 0, 2, 1);
    assert_eq!(row2, SlideTransition { from: -1, to: 0 });
    assert!(row2.restarts());

    // Dragging backwards: rows in [over, from) shift by +1.
    assert_eq!(slide_shift(1, 3, 1), 1);
    assert_eq!(slide_shift(2, 3, 1), 1);
    assert_eq!(slide_shift(3, 3, 1), -2);
    assert_eq!(slide_shift(0, 3, 1), 0);
    assert_eq!(slide_shift(4, 3, 1), 0);

    close(row2.offset(0.5, px(40.)), -20.);
    close(row2.offset(2., px(40.)), 0.);
}

#[test]
fn reorder_preview_remembers_previous_slot() {
    let mut preview = ReorderPreview::new();
    assert!(!preview.is_active());
    assert_eq!(preview.transition(0), None);
    preview.begin(0);
    assert!(preview.hover(2));
    assert!(!preview.hover(2));
    assert_eq!(
        preview.transition(0),
        Some(SlideTransition { from: 0, to: 2 })
    );
    assert_eq!(
        preview.transition(1),
        Some(SlideTransition { from: 0, to: -1 })
    );
    assert!(preview.hover(1));
    assert_eq!(
        preview.transition(0),
        Some(SlideTransition { from: 2, to: 1 })
    );
    assert_eq!(
        preview.transition(2),
        Some(SlideTransition { from: -1, to: 0 })
    );
    assert_eq!(
        preview.pending_move(3),
        Some(ReorderMove { from: 0, to: 1 })
    );
    assert!(preview.leave());
    assert_eq!(
        preview.transition(0),
        Some(SlideTransition { from: 1, to: 0 })
    );
    assert_eq!(preview.pending_move(3), None);
    preview.clear();
    assert!(!preview.is_active());
}

// ---------------------------------------------------------------------------------------------
// A4: groups and snapshots
// ---------------------------------------------------------------------------------------------

#[test]
fn group_identity_and_snapshot_validity() {
    let group = DragGroup::new("tabs", "all", "window-1");
    let payload = ReorderPayload::new(group.clone(), 2u32, vec![1, 2, 3]);
    assert_eq!(payload.validate(&group, &[1, 2, 3]), Ok(1));
    assert_eq!(
        payload.resolve_drop(&group, &[1, 2, 3], 2),
        Ok(Some(ReorderMove { from: 1, to: 2 }))
    );
    assert_eq!(payload.resolve_drop(&group, &[1, 2, 3], 1), Ok(None));

    assert_eq!(
        payload.validate(&DragGroup::new("other", "all", "window-1"), &[1, 2, 3]),
        Err(ReorderRejection::ForeignGroup)
    );
    assert_eq!(
        payload.validate(&DragGroup::new("tabs", "pinned", "window-1"), &[1, 2, 3]),
        Err(ReorderRejection::ForeignGroup)
    );
    assert_eq!(
        payload.validate(&DragGroup::new("tabs", "all", "window-2"), &[1, 2, 3]),
        Err(ReorderRejection::ForeignGroup)
    );
    assert_eq!(
        payload.validate(&group, &[1, 3, 2]),
        Err(ReorderRejection::StaleSnapshot)
    );
    assert_eq!(
        payload.validate(&group, &[1, 2]),
        Err(ReorderRejection::StaleSnapshot)
    );
    assert_eq!(
        payload.validate(&group, &[1, 2, 3, 4]),
        Err(ReorderRejection::StaleSnapshot)
    );

    let missing = ReorderPayload::new(group.clone(), 9u32, vec![1, 2, 3]);
    assert_eq!(
        missing.validate(&group, &[1, 2, 3]),
        Err(ReorderRejection::DraggedNotInSnapshot)
    );
    assert_eq!(missing.source_index(), None);
}

struct GroupDropView {
    ids: Vec<u32>,
    group: DragGroup,
    payload: ReorderPayload<u32>,
    rejection: Option<ReorderRejection>,
    commits: usize,
}

impl Render for GroupDropView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                div()
                    .id("source")
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .size(px(50.))
                    .on_drag(self.payload.clone(), |_, _, _, cx| cx.new(|_| Empty)),
            )
            .child(
                div()
                    .id("target")
                    .absolute()
                    .left(px(100.))
                    .top(px(100.))
                    .w(px(200.))
                    .h(px(120.))
                    .on_drop(
                        cx.listener(|this, payload: &ReorderPayload<u32>, window, cx| {
                            let over = uniform_slot(
                                content_coordinate(
                                    window.mouse_position().y,
                                    px(100.),
                                    px(0.),
                                    px(0.),
                                ),
                                px(40.),
                                this.ids.len(),
                            )
                            .unwrap();
                            match payload.resolve_drop(&this.group, &this.ids, over) {
                                Ok(Some(reorder)) => {
                                    reorder.apply(&mut this.ids);
                                    this.commits += 1;
                                }
                                Ok(None) => {}
                                Err(rejection) => this.rejection = Some(rejection),
                            }
                            cx.notify();
                        }),
                    ),
            )
    }
}

/// Drags the source onto slot 2 of the target and reports the resulting ids, the rejection and
/// the commit count.
fn run_group_drop(
    cx: &mut TestAppContext,
    payload: ReorderPayload<u32>,
    ids: Vec<u32>,
) -> (Vec<u32>, Option<ReorderRejection>, usize) {
    let (view, cx) = cx.add_window_view(|_, _| GroupDropView {
        ids,
        group: DragGroup::new("tabs", "all", "window-1"),
        payload,
        rejection: None,
        commits: 0,
    });
    draw(cx);
    press(cx, 10., 10.);
    drag_to(cx, 20., 20.);
    assert!(has_drag(cx));
    drag_to(cx, 150., 190.);
    release(cx, 150., 190.);
    assert!(!has_drag(cx), "drag state must be cleared after the drop");
    view.read_with(cx, |view, _| {
        (view.ids.clone(), view.rejection, view.commits)
    })
}

#[crate::test]
fn foreign_or_stale_payload_does_not_reorder_and_clears_drag(cx: &mut TestAppContext) {
    let group = DragGroup::new("tabs", "all", "window-1");

    let foreign_list =
        ReorderPayload::new(DragGroup::new("other", "all", "window-1"), 1, vec![1, 2, 3]);
    assert_eq!(
        run_group_drop(cx, foreign_list, vec![1, 2, 3]),
        (vec![1, 2, 3], Some(ReorderRejection::ForeignGroup), 0)
    );

    let foreign_filter = ReorderPayload::new(
        DragGroup::new("tabs", "pinned", "window-1"),
        1,
        vec![1, 2, 3],
    );
    assert_eq!(
        run_group_drop(cx, foreign_filter, vec![1, 2, 3]),
        (vec![1, 2, 3], Some(ReorderRejection::ForeignGroup), 0)
    );

    let stale = ReorderPayload::new(group.clone(), 1, vec![1, 2, 3]);
    assert_eq!(
        run_group_drop(cx, stale, vec![1, 3, 2]),
        (vec![1, 3, 2], Some(ReorderRejection::StaleSnapshot), 0)
    );

    let not_in_snapshot = ReorderPayload::new(group.clone(), 9, vec![1, 2, 3]);
    assert_eq!(
        run_group_drop(cx, not_in_snapshot, vec![1, 2, 3]),
        (
            vec![1, 2, 3],
            Some(ReorderRejection::DraggedNotInSnapshot),
            0
        )
    );

    // A matching payload commits exactly once: item 1 dropped on slot 2 ends up last.
    let valid = ReorderPayload::new(group, 1, vec![1, 2, 3]);
    assert_eq!(
        run_group_drop(cx, valid, vec![1, 2, 3]),
        (vec![2, 3, 1], None, 1)
    );
}

// ---------------------------------------------------------------------------------------------
// A5: trees
// ---------------------------------------------------------------------------------------------

fn path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

#[test]
fn selection_roots_collapse_descendants() {
    let visible = [
        TreeEntry::directory("src"),
        TreeEntry::file("src/lib.rs"),
        TreeEntry::file("src/main.rs"),
        TreeEntry::file("README.md"),
    ];
    let selected: HashSet<PathBuf> = [path("src"), path("src/main.rs"), path("README.md")].into();
    let roots = selection_roots(&visible, |entry| selected.contains(&entry.path));
    assert_eq!(
        roots
            .iter()
            .map(|entry| entry.path.as_path())
            .collect::<Vec<_>>(),
        [Path::new("src"), Path::new("README.md")]
    );
    // Without the ancestor, descendants stay roots, in visible order.
    let selected: HashSet<PathBuf> = [path("README.md"), path("src/lib.rs")].into();
    let roots = selection_roots(&visible, |entry| selected.contains(&entry.path));
    assert_eq!(
        roots
            .iter()
            .map(|entry| entry.path.as_path())
            .collect::<Vec<_>>(),
        [Path::new("src/lib.rs"), Path::new("README.md")]
    );
}

#[test]
fn tree_destination_resolution() {
    let region = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(300.)));
    let dir = TreeRow {
        path: path("src"),
        kind: TreeRowKind::Directory,
    };
    let file = TreeRow {
        path: path("src/lib.rs"),
        kind: TreeRowKind::File,
    };
    let top_file = TreeRow {
        path: path("README.md"),
        kind: TreeRowKind::File,
    };
    let link = TreeRow {
        path: path("src/link"),
        kind: TreeRowKind::Link,
    };
    let root = TreeRow {
        path: PathBuf::new(),
        kind: TreeRowKind::Root,
    };
    let at = pt(50., 40.);
    assert_eq!(
        destination_directory(region, at, TreeHit::Row(&dir)),
        Some(path("src"))
    );
    // A file row targets its parent directory.
    assert_eq!(
        destination_directory(region, at, TreeHit::Row(&file)),
        Some(path("src"))
    );
    assert_eq!(
        destination_directory(region, at, TreeHit::Row(&top_file)),
        Some(PathBuf::new())
    );
    // A link row is not a target.
    assert_eq!(destination_directory(region, at, TreeHit::Row(&link)), None);
    assert_eq!(
        destination_directory(region, at, TreeHit::Row(&root)),
        Some(PathBuf::new())
    );
    // The empty area below the last row targets the root.
    assert_eq!(
        destination_directory(region, pt(50., 250.), TreeHit::BelowLastRow),
        Some(PathBuf::new())
    );
    // The rightmost 12 px (the scroll rail) are never a target.
    assert_eq!(
        destination_directory(region, pt(187.9, 40.), TreeHit::Row(&dir)),
        Some(path("src"))
    );
    assert_eq!(
        destination_directory(region, pt(188., 40.), TreeHit::Row(&dir)),
        None
    );
    assert_eq!(
        destination_directory(region, pt(199., 250.), TreeHit::BelowLastRow),
        None
    );
    // Outside the drag region.
    assert_eq!(
        destination_directory(region, pt(250., 40.), TreeHit::Row(&dir)),
        None
    );
    assert_eq!(
        destination_directory(region, pt(50., -1.), TreeHit::Row(&dir)),
        None
    );
}

#[test]
fn tree_move_resolution() {
    let moves =
        |items: &[TreeEntry], destination: &str| resolve_tree_moves(items, Path::new(destination));

    // A root drop yields name-only destinations.
    assert_eq!(
        moves(
            &[
                TreeEntry::file("src/lib.rs"),
                TreeEntry::directory("docs/guide")
            ],
            ""
        ),
        Ok(vec![
            TreeMove {
                source: path("src/lib.rs"),
                destination: path("lib.rs")
            },
            TreeMove {
                source: path("docs/guide"),
                destination: path("guide")
            },
        ])
    );
    // Dropping an entry on its own parent is skipped; the rest still move.
    assert_eq!(
        moves(
            &[TreeEntry::file("src/lib.rs"), TreeEntry::file("docs/a.md")],
            "src"
        ),
        Ok(vec![TreeMove {
            source: path("docs/a.md"),
            destination: path("src/a.md")
        }])
    );
    assert_eq!(
        moves(&[TreeEntry::file("src/lib.rs")], "src"),
        Err(TreeMoveError::NothingToMove)
    );
    assert_eq!(
        moves(&[TreeEntry::file("lib.rs")], ""),
        Err(TreeMoveError::NothingToMove)
    );
    assert_eq!(moves(&[], "src"), Err(TreeMoveError::NothingToMove));
    // A folder into itself or a descendant is invalid, and invalidates the whole gesture.
    assert_eq!(
        moves(&[TreeEntry::directory("src")], "src"),
        Err(TreeMoveError::IntoSelf {
            source: path("src")
        })
    );
    assert_eq!(
        moves(&[TreeEntry::directory("src")], "src/inner"),
        Err(TreeMoveError::IntoSelf {
            source: path("src")
        })
    );
    assert_eq!(
        moves(
            &[TreeEntry::file("a.txt"), TreeEntry::directory("src")],
            "src/inner"
        ),
        Err(TreeMoveError::IntoSelf {
            source: path("src")
        })
    );
    // A sibling directory with a name sharing a prefix is not a descendant.
    assert!(moves(&[TreeEntry::directory("src")], "src2").is_ok());
    // Two entries with the same name from different folders collide.
    assert_eq!(
        moves(
            &[TreeEntry::file("a/x.txt"), TreeEntry::file("b/x.txt")],
            "dest"
        ),
        Err(TreeMoveError::NameCollision {
            destination: path("dest/x.txt")
        })
    );
    assert_eq!(
        moves(
            &[TreeEntry::file("a/x.txt"), TreeEntry::file("b/x.txt")],
            ""
        ),
        Err(TreeMoveError::NameCollision {
            destination: path("x.txt")
        })
    );
}

// ---------------------------------------------------------------------------------------------
// A6: autoscroll profiles and loop
// ---------------------------------------------------------------------------------------------

#[test]
fn linear_autoscroll_profile_a() {
    let (top, bottom) = (px(100.), px(500.));
    let speed = |y: f32| linear_autoscroll_speed(px(y), top, bottom);
    assert_eq!(speed(100.), px(-540.));
    assert_eq!(speed(500.), px(540.));
    assert_eq!(speed(300.), px(0.));
    assert_eq!(speed(99.), px(0.));
    assert_eq!(speed(501.), px(0.));
    assert!(speed(486.) > px(0.));
    // Exactly half way into the 28 px band.
    assert_eq!(speed(114.), px(-270.));
    assert_eq!(speed(486.), px(270.));
    // The speed is linear in depth and zero where the band ends.
    assert_eq!(speed(128.), px(0.));
    assert_eq!(speed(472.), px(0.));
    // Degenerate viewports.
    assert_eq!(
        linear_autoscroll_speed(px(100.), px(100.), px(100.)),
        px(0.)
    );
    assert_eq!(
        linear_autoscroll_speed(px(100.), px(200.), px(100.)),
        px(0.)
    );
    // A short viewport shrinks the band to half its height: 20 px band for a 40 px viewport.
    assert_eq!(
        linear_autoscroll_speed(px(110.), px(100.), px(140.)),
        px(-270.)
    );
    assert_eq!(
        linear_autoscroll_speed(px(120.), px(100.), px(140.)),
        px(0.)
    );
}

#[test]
fn linear_autoscroll_tick_caps_elapsed() {
    let speed = px(540.);
    assert_eq!(
        linear_autoscroll_delta(speed, Duration::from_millis(200)),
        speed * 0.05
    );
    close(
        linear_autoscroll_delta(speed, Duration::from_millis(16)),
        540. * 0.016,
    );
    assert_eq!(linear_autoscroll_delta(speed, Duration::ZERO), px(0.));
    assert_eq!(
        linear_autoscroll_delta(px(-540.), Duration::from_secs(1)),
        px(-540.) * 0.05
    );
}

#[test]
fn ramp_autoscroll_profile_b() {
    let (top, bottom) = (px(100.), px(300.));
    let step = |y: f32| ramp_autoscroll_step(px(y), top, bottom);
    assert_eq!(step(200.), px(0.));
    assert_eq!(step(124.), px(-6.));
    assert_eq!(step(276.), px(6.));
    // Full speed at and beyond the edge, including beyond the viewport.
    assert_eq!(step(52.), px(-12.));
    assert_eq!(step(100.), px(-12. * 48. / 48.));
    assert_eq!(step(-500.), px(-12.));
    assert_eq!(step(348.), px(12.));
    assert_eq!(step(900.), px(12.));
    assert_eq!(step(148.), px(0.));
    assert_eq!(step(252.), px(0.));
    assert_eq!(ramp_autoscroll_step(px(100.), px(300.), px(300.)), px(0.));
    assert_eq!(ramp_autoscroll_step(px(100.), px(300.), px(100.)), px(0.));
}

#[test]
fn selection_autoscroll_profile_c() {
    let (top, bottom, line) = (px(100.), px(300.), px(22.75));
    let step = |y: f32| selection_autoscroll_step(px(y), top, bottom, line);
    assert_eq!(step(200.), px(0.));
    close(step(90.), -2.);
    close(step(315.), 3.);
    assert_eq!(step(-100.), px(-22.75));
    assert_eq!(step(500.), px(22.75));
    // At least one pixel per tick however close to the edge.
    close(step(99.9), -1.);
    close(step(300.1), 1.);

    let bounds = Bounds::new(pt(10., 100.), size(px(200.), px(200.)));
    assert_eq!(
        clamp_selection_end(pt(400., 500.), bounds),
        pt(209.5, 299.5)
    );
    assert_eq!(clamp_selection_end(pt(0., 0.), bounds), pt(10., 100.));
    assert_eq!(clamp_selection_end(pt(50., 150.), bounds), pt(50., 150.));
}

#[test]
fn scroll_position_stays_within_limits() {
    close(advance_scroll_position(px(10.), px(-30.), px(100.)), 0.);
    close(advance_scroll_position(px(90.), px(30.), px(100.)), 100.);
    close(advance_scroll_position(px(50.), px(5.), px(100.)), 55.);
    assert!(!scroll_can_change(px(0.), -1., px(100.)));
    assert!(scroll_can_change(px(0.), 1., px(100.)));
    assert!(!scroll_can_change(px(100.), 1., px(100.)));
    assert!(scroll_can_change(px(100.), -1., px(100.)));
    assert!(!scroll_can_change(px(50.), 0., px(100.)));
}

#[derive(Clone)]
struct Marker;

struct ScrollView {
    handle: ScrollHandle,
}

impl Render for ScrollView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let scroller = div()
            .id("scroller")
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .w(px(200.))
            .h(px(200.))
            .overflow_y_scroll()
            .track_scroll(&self.handle)
            .flex()
            .flex_col()
            .children((0..100).map(|ix| div().id(ix).h(px(20.)).flex_none().w_full()));
        let source = div()
            .id("source")
            .absolute()
            .left(px(300.))
            .top(px(10.))
            .size(px(20.))
            .on_drag(Marker, |_, _, _, cx| cx.new(|_| Empty));
        div().size_full().child(scroller).child(source)
    }
}

/// A drawn scroll view (viewport 0..200 vertically, content 2000 px) with an active drag whose
/// pointer is in the bottom band of the viewport at y 195.
fn scroll_fixture(cx: &mut TestAppContext) -> (ScrollHandle, &mut VisualTestContext) {
    let handle = ScrollHandle::new();
    let (_, cx) = cx.add_window_view({
        let handle = handle.clone();
        move |_, _| ScrollView { handle }
    });
    draw(cx);
    close(handle.max_offset().y, 1800.);
    press(cx, 310., 20.);
    drag_to(cx, 310., 30.);
    assert!(has_drag(cx));
    drag_to(cx, 100., 195.);
    (handle, cx)
}

#[crate::test]
fn autoscroll_loop_profile_a_scrolls_by_speed_times_capped_elapsed(cx: &mut TestAppContext) {
    let (handle, cx) = scroll_fixture(cx);
    let autoscroll = AutoscrollLoop::new();
    cx.update(|window, cx| {
        autoscroll.start_vertical(window, cx, &handle, EdgeAutoscroll::TimeLinear)
    });
    cx.run_until_parked();
    // The first tick has an elapsed time of zero.
    assert!(autoscroll.is_running());
    close(handle.offset().y, 0.);

    let speed = 540. * (1. - 5. / 28.);
    advance(cx, 16);
    close(handle.offset().y, -speed * 0.016);

    // Each tick moves by speed times the time since the previous tick.
    advance(cx, 16);
    close(handle.offset().y, -speed * 0.032);
    assert!(autoscroll.is_running());

    // Releasing the drag ends the loop at its next tick without scrolling further.
    let before = handle.offset().y;
    release(cx, 100., 195.);
    advance(cx, 16);
    assert!(!autoscroll.is_running());
    advance(cx, 100);
    assert_eq!(handle.offset().y, before);
}

#[crate::test]
fn autoscroll_tick_elapsed_is_zero_first_and_capped(cx: &mut TestAppContext) {
    let (_handle, cx) = scroll_fixture(cx);
    let autoscroll = AutoscrollLoop::new();
    let executor = cx.executor();
    let elapsed = Rc::new(RefCell::new(Vec::new()));
    cx.update(|window, cx| {
        let elapsed = elapsed.clone();
        autoscroll.start(window, cx, move |tick, _, _| {
            elapsed.borrow_mut().push(tick.elapsed);
            if elapsed.borrow().len() == 3 {
                // A tick that takes 200 ms (a stalled frame).
                executor.advance_clock(Duration::from_millis(200));
            }
            true
        });
    });
    cx.run_until_parked();
    advance(cx, 16);
    advance(cx, 16);
    advance(cx, 16);
    let millis = |value: u64| Duration::from_millis(value);
    assert_eq!(
        *elapsed.borrow(),
        [
            Duration::ZERO,
            millis(16),
            millis(16),
            AUTOSCROLL_MAX_ELAPSED
        ]
    );
}

#[crate::test]
fn autoscroll_loop_ends_when_pointer_leaves_region(cx: &mut TestAppContext) {
    let (handle, cx) = scroll_fixture(cx);
    let autoscroll = AutoscrollLoop::new();
    cx.update(|window, cx| {
        autoscroll.start_vertical(window, cx, &handle, EdgeAutoscroll::TimeLinear)
    });
    cx.run_until_parked();
    advance(cx, 16);
    assert!(autoscroll.is_running());
    let before = handle.offset().y;
    assert!(before < px(0.));

    // Leaving the viewport horizontally stops scrolling.
    drag_to(cx, 250., 195.);
    advance(cx, 16);
    assert!(!autoscroll.is_running());
    assert_eq!(handle.offset().y, before);
    assert!(has_drag(cx));
}

#[crate::test]
fn autoscroll_loop_ends_at_scroll_limit(cx: &mut TestAppContext) {
    let (handle, cx) = scroll_fixture(cx);
    handle.set_offset(pt(0., -1799.));
    let autoscroll = AutoscrollLoop::new();
    cx.update(|window, cx| {
        autoscroll.start_vertical(window, cx, &handle, EdgeAutoscroll::TimeLinear)
    });
    cx.run_until_parked();
    advance(cx, 16);
    // The tick clamps to the maximum scroll.
    assert_eq!(handle.offset().y, px(-1800.));
    assert!(autoscroll.is_running());
    // The next tick could not change the offset, so the loop ends.
    advance(cx, 16);
    assert!(!autoscroll.is_running());
    assert_eq!(handle.offset().y, px(-1800.));

    // Scrolling toward the top at the top limit ends the loop too.
    handle.set_offset(pt(0., 0.));
    drag_to(cx, 100., 5.);
    cx.update(|window, cx| {
        autoscroll.start_vertical(window, cx, &handle, EdgeAutoscroll::TimeLinear)
    });
    cx.run_until_parked();
    assert!(!autoscroll.is_running());
    assert_eq!(handle.offset().y, px(0.));
}

#[crate::test]
fn stale_autoscroll_generation_does_not_act(cx: &mut TestAppContext) {
    let (_handle, cx) = scroll_fixture(cx);
    let autoscroll = AutoscrollLoop::new();
    let ticks = Rc::new(Cell::new(0));
    cx.update(|window, cx| {
        let ticks = ticks.clone();
        autoscroll.start(window, cx, move |_, _, _| {
            ticks.set(ticks.get() + 1);
            true
        });
    });
    cx.run_until_parked();
    assert_eq!(ticks.get(), 1);
    advance(cx, 16);
    assert_eq!(ticks.get(), 2);
    let generation = autoscroll.generation();

    // The drag the loop belonged to finished: the loop must not act on any later state, even
    // though the same drag-less checks would still pass for a new drag.
    autoscroll.invalidate();
    assert_ne!(autoscroll.generation(), generation);
    assert!(!autoscroll.is_running());
    advance(cx, 16);
    advance(cx, 16);
    assert_eq!(ticks.get(), 2);

    // A new loop gets a new generation and ticks again; cancelling drops the task right away.
    cx.update(|window, cx| {
        let ticks = ticks.clone();
        autoscroll.start(window, cx, move |_, _, _| {
            ticks.set(ticks.get() + 100);
            true
        });
    });
    cx.run_until_parked();
    assert_eq!(ticks.get(), 102);
    autoscroll.cancel();
    advance(cx, 16);
    assert_eq!(ticks.get(), 102);
    assert!(!autoscroll.is_running());
}

#[crate::test]
fn autoscroll_loop_requires_an_active_drag_unless_for_selection(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| ScrollView {
        handle: ScrollHandle::new(),
    });
    draw(cx);
    let autoscroll = AutoscrollLoop::new();
    let ticks = Rc::new(Cell::new(0));
    cx.update(|window, cx| {
        let ticks = ticks.clone();
        autoscroll.start(window, cx, move |_, _, _| {
            ticks.set(ticks.get() + 1);
            true
        });
    });
    cx.run_until_parked();
    assert_eq!(ticks.get(), 0);
    assert!(!autoscroll.is_running());

    cx.update(|window, cx| {
        let ticks = ticks.clone();
        autoscroll.start_for_selection(window, cx, move |_, _, _| {
            ticks.set(ticks.get() + 1);
            ticks.get() < 3
        });
    });
    cx.run_until_parked();
    assert_eq!(ticks.get(), 1);
    advance(cx, 16);
    assert_eq!(ticks.get(), 2);
    assert!(autoscroll.is_running());
    // The tick function ends the loop.
    advance(cx, 16);
    assert_eq!(ticks.get(), 3);
    assert!(!autoscroll.is_running());
    advance(cx, 100);
    assert_eq!(ticks.get(), 3);
}

#[crate::test]
fn autoscroll_profile_b_ends_outside_its_section(cx: &mut TestAppContext) {
    let (handle, cx) = scroll_fixture(cx);
    let section = Rc::new(Cell::new(Bounds::new(pt(0., 0.), size(px(200.), px(200.)))));
    let autoscroll = AutoscrollLoop::new();
    let profile = EdgeAutoscroll::PenetrationRamp {
        section: Rc::new({
            let section = section.clone();
            move || section.get()
        }),
    };
    cx.update(|window, cx| autoscroll.start_vertical(window, cx, &handle, profile));
    cx.run_until_parked();
    // The step is per tick, not per elapsed time: 12 * (195 - 152) / 48.
    close(handle.offset().y, -12. * 43. / 48.);
    advance(cx, 16);
    close(handle.offset().y, -2. * 12. * 43. / 48.);
    assert!(autoscroll.is_running());

    // The edge zone extends beyond the section; once the pointer is outside the section the loop
    // ends although the pointer is still in the viewport's edge zone.
    section.set(Bounds::new(pt(0., 0.), size(px(200.), px(150.))));
    let before = handle.offset().y;
    advance(cx, 16);
    assert!(!autoscroll.is_running());
    assert_eq!(handle.offset().y, before);
}

// ---------------------------------------------------------------------------------------------
// A7 and R10: dwell, escape, leaving, deactivation, end
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct TreeDrag;

fn tree_rows() -> [TreeRow; 3] {
    [
        TreeRow {
            path: path("a"),
            kind: TreeRowKind::Directory,
        },
        TreeRow {
            path: path("b"),
            kind: TreeRowKind::Directory,
        },
        TreeRow {
            path: path("f.txt"),
            kind: TreeRowKind::File,
        },
    ]
}

struct TreeView {
    session: DragSession<PathBuf>,
    expanded: Rc<RefCell<Vec<PathBuf>>>,
    focus: FocusHandle,
    _deactivation: Subscription,
}

impl Render for TreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drag-derived state must be reset in any frame without an active drag.
        self.session.clear_if_idle(cx);
        div()
            .id("tree")
            .track_focus(&self.focus)
            .w(px(200.))
            .h(px(300.))
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.session.cancel_on_escape(event, window, cx);
            }))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<TreeDrag>, window, cx| {
                    let region = event.bounds;
                    let pointer = event.event.position;
                    if !this.session.pointer_moved(region, pointer, window, cx) {
                        return;
                    }
                    let rows = tree_rows();
                    let ix = ((pointer.y - region.top()) / px(30.)).floor() as usize;
                    let hit = rows.get(ix).map_or(TreeHit::BelowLastRow, TreeHit::Row);
                    let destination = destination_directory(region, pointer, hit);
                    let expanded = this.expanded.clone();
                    this.session
                        .set_destination(destination, window, cx, move |dir, _, _| {
                            expanded.borrow_mut().push(dir.clone())
                        });
                }),
            )
            .child(div().id("a").h(px(30.)).flex_none().w_full())
            .child(div().id("b").h(px(30.)).flex_none().w_full())
            .child(
                div()
                    .id("f")
                    .h(px(30.))
                    .flex_none()
                    .w_full()
                    .on_drag(TreeDrag, |_, _, _, cx| cx.new(|_| Empty)),
            )
    }
}

fn tree_fixture(
    cx: &mut TestAppContext,
) -> (
    DragSession<PathBuf>,
    Rc<RefCell<Vec<PathBuf>>>,
    &mut VisualTestContext,
) {
    let session = DragSession::new();
    let expanded = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let (session, expanded) = (session.clone(), expanded.clone());
        move |window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            window.activate_window();
            let _deactivation = session.cancel_on_deactivation(window);
            TreeView {
                session,
                expanded,
                focus,
                _deactivation,
            }
        }
    });
    draw(cx);
    // Drag the file row (y 60..90).
    press(cx, 50., 75.);
    drag_to(cx, 50., 80.);
    assert!(has_drag(cx));
    (session, expanded, cx)
}

#[crate::test]
fn dwell_expands_collapsed_destination_at_650ms(cx: &mut TestAppContext) {
    let (session, expanded, cx) = tree_fixture(cx);
    drag_to(cx, 50., 10.);
    assert_eq!(session.destination(), Some(path("a")));
    assert!(session.is_dwelling());
    // A destination shows the closed hand.
    assert_eq!(drag_cursor(cx), Some(CursorStyle::ClosedHand));
    assert_eq!(frame_cursor(cx), Some(CursorStyle::ClosedHand));

    advance(cx, 649);
    assert!(expanded.borrow().is_empty());
    advance(cx, 1);
    assert_eq!(*expanded.borrow(), [path("a")]);
    assert!(!session.is_dwelling());

    // Moving within the same folder neither restarts nor refires the timer.
    drag_to(cx, 60., 12.);
    advance(cx, 2000);
    assert_eq!(*expanded.borrow(), [path("a")]);
}

#[crate::test]
fn dwell_restarts_when_destination_changes(cx: &mut TestAppContext) {
    let (session, expanded, cx) = tree_fixture(cx);
    drag_to(cx, 50., 10.);
    advance(cx, 400);
    assert!(expanded.borrow().is_empty());

    // Moving within the same folder keeps the running timer.
    drag_to(cx, 70., 20.);
    // Moving to another folder restarts it: `a` would have expanded after 650 ms.
    drag_to(cx, 50., 40.);
    assert_eq!(session.destination(), Some(path("b")));
    advance(cx, 400);
    assert!(expanded.borrow().is_empty());
    advance(cx, 249);
    assert!(expanded.borrow().is_empty());
    advance(cx, 1);
    assert_eq!(*expanded.borrow(), [path("b")]);

    // Leaving every folder (a file row targets the root, the empty area targets the root).
    drag_to(cx, 50., 200.);
    assert_eq!(session.destination(), Some(PathBuf::new()));
    // The scroll rail is no destination: the prohibited cursor is shown.
    drag_to(cx, 195., 40.);
    assert_eq!(session.destination(), None);
    assert!(!session.is_dwelling());
    assert_eq!(drag_cursor(cx), Some(CursorStyle::OperationNotAllowed));
}

#[crate::test]
fn escape_cancels_tick_task_dwell_destination_and_drag(cx: &mut TestAppContext) {
    let (session, expanded, cx) = tree_fixture(cx);
    drag_to(cx, 50., 10.);
    let ticks = Rc::new(Cell::new(0));
    cx.update(|window, cx| {
        let ticks = ticks.clone();
        session.autoscroll().start(window, cx, move |_, _, _| {
            ticks.set(ticks.get() + 1);
            true
        });
    });
    cx.run_until_parked();
    advance(cx, 16);
    assert!(session.autoscroll().is_running());
    assert_eq!(ticks.get(), 2);
    assert_eq!(frame_cursor(cx), Some(CursorStyle::ClosedHand));

    cx.simulate_keystrokes("escape");
    assert!(!has_drag(cx));
    assert_eq!(session.destination(), None);
    assert!(!session.is_dwelling());
    assert!(!session.autoscroll().is_running());
    draw(cx);
    // No drag cursor remains in the next frame (the arrow is the default).
    assert_eq!(frame_cursor(cx), None);

    // Neither the tick task nor the dwell timer survives.
    advance(cx, 2000);
    assert_eq!(ticks.get(), 2);
    assert!(expanded.borrow().is_empty());
}

#[crate::test]
fn leaving_the_region_ends_the_drag(cx: &mut TestAppContext) {
    let (session, expanded, cx) = tree_fixture(cx);
    drag_to(cx, 50., 10.);
    assert_eq!(session.destination(), Some(path("a")));
    drag_to(cx, 300., 10.);
    assert!(!has_drag(cx));
    assert_eq!(session.destination(), None);
    assert!(!session.is_dwelling());
    assert_eq!(frame_cursor(cx), None);
    advance(cx, 2000);
    assert!(expanded.borrow().is_empty());
}

#[crate::test]
fn window_deactivation_clears_session_and_cursor(cx: &mut TestAppContext) {
    let (session, expanded, cx) = tree_fixture(cx);
    drag_to(cx, 50., 10.);
    assert_eq!(frame_cursor(cx), Some(CursorStyle::ClosedHand));
    cx.deactivate_window();
    assert!(!has_drag(cx));
    assert_eq!(session.destination(), None);
    assert!(!session.is_dwelling());
    assert!(!session.autoscroll().is_running());
    draw(cx);
    assert_eq!(frame_cursor(cx), None);
    advance(cx, 2000);
    assert!(expanded.borrow().is_empty());
}

#[crate::test]
fn drag_end_without_drop_clears_session_next_frame(cx: &mut TestAppContext) {
    let (session, expanded, cx) = tree_fixture(cx);
    drag_to(cx, 50., 10.);
    assert!(session.is_dwelling());
    // Mouse-up outside every accepting target ends the drag silently; the next frame clears the
    // consumer's state.
    release(cx, 300., 300.);
    assert!(!has_drag(cx));
    assert_eq!(session.destination(), None);
    assert!(!session.is_dwelling());
    assert_eq!(frame_cursor(cx), None);
    advance(cx, 2000);
    assert!(expanded.borrow().is_empty());
}

#[crate::test]
fn incompatible_payload_cancels_session(cx: &mut TestAppContext) {
    let (session, _expanded, cx) = tree_fixture(cx);
    drag_to(cx, 50., 10.);
    cx.update(|window, cx| {
        assert!(session.retain_if_compatible(true, window, cx));
    });
    assert!(has_drag(cx));
    cx.update(|window, cx| {
        assert!(!session.retain_if_compatible(false, window, cx));
    });
    assert!(!has_drag(cx));
    assert_eq!(session.destination(), None);
}

#[crate::test]
fn drag_cursor_helpers(cx: &mut TestAppContext) {
    let (_session, _expanded, cx) = tree_fixture(cx);
    cx.update(|window, cx| {
        assert!(set_drag_cursor(window, cx, true));
        assert_eq!(cx.active_drag_cursor_style(), Some(CursorStyle::ClosedHand));
        assert!(set_drag_cursor(window, cx, false));
        assert_eq!(
            cx.active_drag_cursor_style(),
            Some(CursorStyle::OperationNotAllowed)
        );
        assert!(clear_drag_cursor(window, cx));
        assert_eq!(cx.active_drag_cursor_style(), Some(CursorStyle::Arrow));
    });
    release(cx, 300., 300.);
    cx.update(|window, cx| {
        assert!(!set_drag_cursor(window, cx, true));
        assert!(!clear_drag_cursor(window, cx));
    });
}

// ---------------------------------------------------------------------------------------------
// A1: reorder a scrolled list with padding
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct RowDrag {
    from: usize,
}

const ROW_HEIGHT: f32 = 40.;
const LIST_PADDING: f32 = 8.;
const LIST_TOP: f32 = 20.;

struct ReorderList {
    items: Vec<u32>,
    preview: ReorderPreview,
    scroll: ScrollHandle,
    drops: usize,
}

impl Render for ReorderList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A mouse-up outside the list ends the drag without a drop callback.
        self.preview.clear_if_idle(cx);
        div().size_full().child(
            div()
                .id("list")
                .absolute()
                .left(px(0.))
                .top(px(LIST_TOP))
                .w(px(200.))
                .h(px(120.))
                .py(px(LIST_PADDING))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .flex()
                .flex_col()
                .on_drag_move(cx.listener(|this, event: &DragMoveEvent<RowDrag>, _, cx| {
                    let pointer = event.event.position;
                    if !event.bounds.contains(&pointer) {
                        if this.preview.leave() {
                            cx.notify();
                        }
                        return;
                    }
                    if this.preview.from().is_none() {
                        this.preview.begin(event.drag(cx).from);
                    }
                    let coordinate = content_coordinate(
                        pointer.y,
                        event.bounds.top(),
                        this.scroll.offset().y,
                        px(LIST_PADDING),
                    );
                    if let Some(over) = uniform_slot(coordinate, px(ROW_HEIGHT), this.items.len()) {
                        if this.preview.hover(over) {
                            cx.notify();
                        }
                    }
                }))
                .on_drop(cx.listener(|this, _: &RowDrag, _, cx| {
                    if let Some(reorder) = this.preview.pending_move(this.items.len()) {
                        reorder.apply(&mut this.items);
                    }
                    this.drops += 1;
                    this.preview.clear();
                    cx.notify();
                }))
                .children(self.items.iter().enumerate().map(|(ix, item)| {
                    div()
                        .id(("row", *item as usize))
                        .h(px(ROW_HEIGHT))
                        .flex_none()
                        .w_full()
                        .on_drag(RowDrag { from: ix }, |_, _, _, cx| cx.new(|_| Empty))
                })),
        )
    }
}

#[crate::test]
fn scrolled_padded_list_reorders_once_and_clears_visuals(cx: &mut TestAppContext) {
    let scroll = ScrollHandle::new();
    let (view, cx) = cx.add_window_view({
        let scroll = scroll.clone();
        move |_, _| ReorderList {
            items: vec![1, 2, 3, 4, 5],
            preview: ReorderPreview::new(),
            scroll,
            drops: 0,
        }
    });
    draw(cx);
    // 5 rows of 40 px plus 8 px of padding on both sides in a 120 px viewport.
    close(
        scroll.max_offset().y,
        5. * ROW_HEIGHT + 2. * LIST_PADDING - 120.,
    );

    let over = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.preview.over());
    let items = |cx: &mut VisualTestContext| view.read_with(cx, |view, _| view.items.clone());

    // Scroll 30 px, then grab row 0 (visible from y -2 to 38).
    scroll.set_offset(pt(0., -30.));
    draw(cx);
    press(cx, 50., 30.);
    drag_to(cx, 50., 35.);
    assert!(has_drag(cx));
    // Leading padding (top + 3): coordinate 3 - (-30) - 8 = 25, slot 0.
    drag_to(cx, 50., LIST_TOP + 3.);
    assert_eq!(over(cx), Some(0));
    // Over rows, adjusted by the scroll offset: 80 - 20 + 30 - 8 = 82, slot 2.
    drag_to(cx, 50., 80.);
    assert_eq!(over(cx), Some(2));

    // Scroll to the end: the trailing padding resolves to the last slot.
    scroll.set_offset(pt(0., -scroll.max_offset().y.0));
    draw(cx);
    drag_to(cx, 50., LIST_TOP + 119.);
    assert_eq!(over(cx), Some(4));
    // And in the leading padding again: 3 + 96 - 8 = 91, slot 2.
    drag_to(cx, 50., LIST_TOP + 3.);
    assert_eq!(over(cx), Some(2));

    // Dropping applies the insertion rule once: row 0 over slot 2 lands at position 2.
    release(cx, 50., LIST_TOP + 3.);
    assert!(!has_drag(cx));
    assert_eq!(items(cx), [2, 3, 1, 4, 5]);
    assert_eq!(view.read_with(cx, |view, _| view.drops), 1);
    assert!(!view.read_with(cx, |view, _| view.preview.is_active()));
    draw(cx);
    assert_eq!(items(cx), [2, 3, 1, 4, 5]);

    // Releasing outside the list clears every visual and leaves the order unchanged. The last row
    // (item 5) is at y 92..132 with the list scrolled to its end.
    press(cx, 50., 100.);
    drag_to(cx, 50., 105.);
    drag_to(cx, 50., 60.);
    assert!(over(cx).is_some());
    assert!(view.read_with(cx, |view, _| view.preview.is_active()));
    release(cx, 350., 300.);
    assert!(!has_drag(cx));
    assert!(!view.read_with(cx, |view, _| view.preview.is_active()));
    assert_eq!(over(cx), None);
    assert_eq!(items(cx), [2, 3, 1, 4, 5]);
    assert_eq!(view.read_with(cx, |view, _| view.drops), 1);
}

// ---------------------------------------------------------------------------------------------
// A9 and R1/R2: drag-move delivery and drop routing
// ---------------------------------------------------------------------------------------------

struct ResizeView {
    moves: Rc<RefCell<Vec<Point<Pixels>>>>,
}

impl Render for ResizeView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let moves = self.moves.clone();
        div().size_full().child(
            div()
                .id("handle")
                .absolute()
                .left(px(10.))
                .top(px(10.))
                .size(px(10.))
                // An invisible preview, as used by resize handles.
                .on_drag(Marker, |_, _, _, cx| cx.new(|_| Empty))
                .on_drag_move(move |event: &DragMoveEvent<Marker>, _, _| {
                    moves.borrow_mut().push(event.event.position)
                }),
        )
    }
}

#[crate::test]
fn resize_drag_with_empty_preview_tracks_pointer_far_outside_and_ends_on_mouse_up(
    cx: &mut TestAppContext,
) {
    let moves = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let moves = moves.clone();
        move |_, _| ResizeView { moves }
    });
    draw(cx);
    press(cx, 15., 15.);
    drag_to(cx, 25., 25.);
    assert!(has_drag(cx));
    // Far outside the 10 px handle.
    drag_to(cx, 700., 500.);
    drag_to(cx, 640., 480.);
    assert_eq!(moves.borrow().as_slice(), [pt(700., 500.), pt(640., 480.)]);
    assert!(has_drag(cx));

    release(cx, 640., 480.);
    assert!(!has_drag(cx));
    drag_to(cx, 100., 100.);
    assert_eq!(
        moves.borrow().len(),
        2,
        "no notifications after the drag ended"
    );
}

#[crate::test]
fn drag_starts_only_after_moving_more_than_two_pixels(cx: &mut TestAppContext) {
    let moves = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view(move |_, _| ResizeView { moves });
    draw(cx);
    press(cx, 15., 15.);
    drag_to(cx, 17., 15.);
    assert!(!has_drag(cx));
    drag_to(cx, 17.5, 15.);
    assert!(has_drag(cx));
}

#[crate::test]
fn deactivation_clears_internal_drag_without_consumer_help(cx: &mut TestAppContext) {
    let moves = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view(move |_, _| ResizeView { moves });
    cx.update(|window, _| window.activate_window());
    draw(cx);
    press(cx, 15., 15.);
    drag_to(cx, 25., 25.);
    assert!(has_drag(cx));
    cx.deactivate_window();
    assert!(!has_drag(cx));
    draw(cx);
    assert_eq!(frame_cursor(cx), None);
}

struct ChainView {
    log: Rc<RefCell<Vec<&'static str>>>,
    child_accepts: Rc<Cell<bool>>,
}

impl Render for ChainView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (parent_log, child_log) = (self.log.clone(), self.log.clone());
        let child_accepts = self.child_accepts.clone();
        div()
            .size_full()
            .child(
                div()
                    .id("source")
                    .absolute()
                    .left(px(300.))
                    .top(px(0.))
                    .size(px(20.))
                    .on_drag(Marker, |_, _, _, cx| cx.new(|_| Empty)),
            )
            .child(
                div()
                    .id("parent")
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .size(px(200.))
                    .on_drop(move |_: &Marker, _, _| parent_log.borrow_mut().push("parent"))
                    .child(
                        div()
                            .id("child")
                            .size(px(50.))
                            .can_drop(move |_, _, _| child_accepts.get())
                            .on_drop(move |_: &Marker, _, _| child_log.borrow_mut().push("child")),
                    ),
            )
    }
}

#[crate::test]
fn drop_goes_to_the_accepting_element_under_the_pointer(cx: &mut TestAppContext) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let child_accepts = Rc::new(Cell::new(false));
    let (_, cx) = cx.add_window_view({
        let (log, child_accepts) = (log.clone(), child_accepts.clone());
        move |_, _| ChainView { log, child_accepts }
    });
    draw(cx);

    let drag_onto = |cx: &mut VisualTestContext, x: f32, y: f32| {
        press(cx, 310., 10.);
        drag_to(cx, 310., 20.);
        assert!(has_drag(cx));
        drag_to(cx, x, y);
        release(cx, x, y);
        assert!(!has_drag(cx));
    };

    // The child's predicate rejects the payload, so the parent under the pointer receives it.
    drag_onto(cx, 25., 25.);
    assert_eq!(*log.borrow(), ["parent"]);
    // An accepting child gets it, and the parent does not.
    child_accepts.set(true);
    drag_onto(cx, 25., 25.);
    assert_eq!(*log.borrow(), ["parent", "child"]);
    // A mouse-up over no accepting element ends the drag silently.
    drag_onto(cx, 350., 300.);
    assert_eq!(*log.borrow(), ["parent", "child"]);
}

// ---------------------------------------------------------------------------------------------
// A8: external file drops
// ---------------------------------------------------------------------------------------------

type DropLog = Rc<RefCell<Vec<(&'static str, Result<Vec<PathBuf>, String>)>>>;

struct PaneView {
    focus: FocusHandle,
    log: DropLog,
}

impl PaneView {
    fn pane(&self, name: &'static str) -> Stateful<crate::Div> {
        let log = self.log.clone();
        div()
            .id(name)
            .w(px(200.))
            .h(px(300.))
            .on_drop(move |paths: &ExternalPaths, _, _| {
                // A consumer that accepts only regular files rejects the entire drop.
                let outcome = paths
                    .require_regular_files()
                    .map(<[PathBuf]>::to_vec)
                    .map_err(|rejection| rejection.to_string());
                log.borrow_mut().push((name, outcome));
            })
    }
}

impl Render for PaneView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .child(self.pane("left").track_focus(&self.focus))
            .child(self.pane("right"))
    }
}

fn external_paths(paths: &[&Path]) -> ExternalPaths {
    ExternalPaths(paths.iter().map(|path| path.to_path_buf()).collect())
}

fn drop_external(cx: &mut VisualTestContext, at: Point<Pixels>, paths: &[&Path]) {
    cx.simulate_event(FileDropEvent::Entered {
        position: at,
        paths: external_paths(paths),
    });
    draw(cx);
    cx.simulate_event(FileDropEvent::Pending { position: at });
    draw(cx);
    cx.simulate_event(FileDropEvent::Submit { position: at });
    draw(cx);
}

fn pane_fixture(cx: &mut TestAppContext) -> (DropLog, &mut VisualTestContext) {
    let log = DropLog::default();
    let (_, cx) = cx.add_window_view({
        let log = log.clone();
        move |window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            PaneView { focus, log }
        }
    });
    draw(cx);
    (log, cx)
}

#[crate::test]
fn external_drop_routes_to_the_pane_under_the_pointer_not_the_focused_one(cx: &mut TestAppContext) {
    let tmp = TempDir::new("route");
    let (a, b) = (tmp.file("a.txt"), tmp.file("b.txt"));
    let (log, cx) = pane_fixture(cx);

    // The left pane is focused; the pointer is over the right pane.
    drop_external(cx, pt(300., 50.), &[&a, &b]);
    assert_eq!(*log.borrow(), [("right", Ok(vec![a.clone(), b.clone()]))]);
    assert!(!has_drag(cx));

    // Dropping over the focused pane reaches it, and only it.
    drop_external(cx, pt(50., 50.), &[&a]);
    assert_eq!(log.borrow().len(), 2);
    assert_eq!(log.borrow()[1], ("left", Ok(vec![a.clone()])));
}

#[crate::test]
fn external_drop_routes_after_keyboard_input(cx: &mut TestAppContext) {
    let tmp = TempDir::new("keyboard");
    let a = tmp.file("a.txt");
    let (log, cx) = pane_fixture(cx);
    // Typing puts the window in keyboard modality, which suppresses hover; an external drag is
    // pointer input and must still resolve the element under the pointer.
    cx.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
    drop_external(cx, pt(300., 50.), &[&a]);
    assert_eq!(*log.borrow(), [("right", Ok(vec![a]))]);
}

#[crate::test]
fn external_drop_with_a_directory_is_rejected_as_a_whole(cx: &mut TestAppContext) {
    let tmp = TempDir::new("reject");
    let file = tmp.file("a.txt");
    let folder = tmp.dir("folder");
    let (log, cx) = pane_fixture(cx);

    drop_external(cx, pt(300., 50.), &[&file, &folder]);
    let log = log.borrow();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "right");
    let message = log[0].1.as_ref().expect_err("the whole drop is rejected");
    assert!(message.contains(&folder.display().to_string()), "{message}");
    assert!(!message.contains(&file.display().to_string()), "{message}");

    let rejection = external_paths(&[&file, &folder])
        .require_regular_files()
        .unwrap_err();
    assert_eq!(rejection.rejected, [(folder, PathKind::Directory)]);
}

#[test]
fn paths_are_classified_as_file_directory_or_neither() {
    let tmp = TempDir::new("classify");
    let file = tmp.file("a.txt");
    let folder = tmp.dir("folder");
    let missing = tmp.0.join("missing");
    assert_eq!(classify_path(&file), PathKind::File);
    assert_eq!(classify_path(&folder), PathKind::Directory);
    assert_eq!(classify_path(&missing), PathKind::Other);

    let classified = external_paths(&[&file, &folder, &missing]);
    assert_eq!(
        classified.classified(),
        [
            (file.as_path(), PathKind::File),
            (folder.as_path(), PathKind::Directory),
            (missing.as_path(), PathKind::Other),
        ]
    );
    assert!(classified.require_regular_files().is_err());
    assert!(external_paths(&[&file]).require_regular_files().is_ok());
    assert!(ExternalPaths::default().require_regular_files().is_ok());
}

#[cfg(unix)]
#[test]
fn unreadable_and_special_paths_are_classified_as_neither() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let tmp = TempDir::new("unreadable");
    let file = tmp.file("a.txt");
    // A link through a file resolves like the file; a dangling one is neither.
    let link = tmp.0.join("link");
    symlink(&file, &link).unwrap();
    assert_eq!(classify_path(&link), PathKind::File);
    let dangling = tmp.0.join("dangling");
    symlink(tmp.0.join("nowhere"), &dangling).unwrap();
    assert_eq!(classify_path(&dangling), PathKind::Other);
    // Devices are neither a regular file nor a directory.
    assert_eq!(classify_path(Path::new("/dev/null")), PathKind::Other);

    // A path behind an unsearchable directory cannot be read. The check is skipped only when the
    // process can read it anyway (running as the superuser).
    let locked = tmp.dir("locked");
    let hidden = locked.join("secret.txt");
    std::fs::write(&hidden, b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::metadata(&hidden).is_ok();
    let kind = classify_path(&hidden);
    let dropped = external_paths(&[&hidden]);
    let rejection = dropped.require_regular_files().map(<[PathBuf]>::to_vec);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert_eq!(kind, PathKind::Other);
        assert_eq!(rejection.unwrap_err().rejected, [(hidden, PathKind::Other)]);
    } else {
        assert_eq!(kind, PathKind::File);
        assert!(rejection.is_ok());
    }
}

struct FeedbackView;

impl Render for FeedbackView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let target = |id: &'static str, left: f32| {
            div()
                .id(id)
                .debug_selector(|| id.to_string())
                .absolute()
                .left(px(left))
                .top(px(0.))
                .w(px(50.))
                .h(px(50.))
                .bg(crate::white())
                .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(crate::red()))
                .drag_over::<Marker>(|style, _, _, _| style.bg(crate::red()))
        };
        div()
            .size_full()
            .child(target("accepting", 0.))
            .child(target("rejecting", 100.).can_drop(|_, _, _| false))
            .child(
                div()
                    .id("source")
                    .absolute()
                    .left(px(300.))
                    .top(px(0.))
                    .size(px(20.))
                    .on_drag(Marker, |_, _, _, cx| cx.new(|_| Empty)),
            )
    }
}

/// The solid fill painted for the element with the given debug selector in the last frame.
fn fill(cx: &mut VisualTestContext, id: &'static str) -> crate::Hsla {
    let bounds = cx.debug_bounds(id).expect("debug bounds");
    cx.update(|window, _| {
        let scale = window.scale_factor();
        window
            .rendered_frame
            .scene
            .quads
            .iter()
            .find(|quad| {
                (quad.bounds.origin.x.0 - bounds.origin.x.0 * scale).abs() < 0.5
                    && (quad.bounds.origin.y.0 - bounds.origin.y.0 * scale).abs() < 0.5
            })
            .and_then(|quad| quad.background.as_solid())
            .expect("a solid quad for the element")
    })
}

#[crate::test]
fn external_drag_shows_the_same_feedback_as_an_internal_payload(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| FeedbackView);
    draw(cx);
    let rest = fill(cx, "accepting");
    assert_eq!(fill(cx, "rejecting"), rest);
    let accepting = |cx: &mut VisualTestContext| fill(cx, "accepting") != rest;
    let rejecting = |cx: &mut VisualTestContext| fill(cx, "rejecting") != rest;

    // Internal payload over an accepting and a rejecting target.
    press(cx, 310., 10.);
    drag_to(cx, 310., 20.);
    drag_to(cx, 25., 25.);
    assert!(accepting(cx));
    drag_to(cx, 125., 25.);
    assert!(!accepting(cx));
    assert!(
        !rejecting(cx),
        "a target whose predicate rejects shows no feedback"
    );
    release(cx, 600., 400.);
    assert!(!has_drag(cx));
    assert!(!accepting(cx));

    // External paths over the same targets show the same feedback.
    let tmp = TempDir::new("feedback");
    let file = tmp.file("a.txt");
    cx.simulate_event(FileDropEvent::Entered {
        position: pt(25., 25.),
        paths: external_paths(&[&file]),
    });
    draw(cx);
    assert!(accepting(cx));
    cx.simulate_event(FileDropEvent::Pending {
        position: pt(125., 25.),
    });
    draw(cx);
    assert!(!accepting(cx));
    assert!(!rejecting(cx));
    cx.simulate_event(FileDropEvent::Pending {
        position: pt(25., 25.),
    });
    draw(cx);
    assert!(accepting(cx));
    // Leaving clears the feedback and the drag.
    cx.simulate_event(FileDropEvent::Exited);
    draw(cx);
    assert!(!has_drag(cx));
    assert!(!accepting(cx));
    cx.simulate_event(FileDropEvent::Ended);
}
