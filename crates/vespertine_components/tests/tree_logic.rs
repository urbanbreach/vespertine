//! CMP-016 pure-logic acceptance tests: scroll anchors (A7), drag helpers (A8), the search
//! tree and reveal (A9), rename (A10) and the context menu model (A5).

use std::time::Duration;
use vespertine::{ListAlignment, ListOffset, ListState, px};
use vespertine_components::tree::{
    ContextMenu, DragItem, DragSession, DropHit, ENTRY_CHANGED_MESSAGE, Entry, EntryKind,
    INVALID_NAME_MESSAGE, LoadRequest, Page, RenameCommit, RenameSession, RevealCoordinator,
    RevealIntent, RevealProgress, RowKind, RowVisual, SearchActivation, SearchResult, SearchTree,
    TreeDragPayload, TreeModel, TreeWashes, VisibleRow, basename_range, copy_path_text,
    delete_subject, destination_paths, edge_scroll_speed, fade_flags, menu_items, resolve_anchor,
    resolve_destination, splice_plan, sync_list_rows, validate_name,
};

fn entry_row(path: &str) -> VisibleRow {
    VisibleRow {
        path: path.to_owned(),
        depth: path.matches('/').count(),
        kind: RowKind::Entry,
    }
}

fn status_row(directory: &str, kind: RowKind, label: &str) -> VisibleRow {
    VisibleRow {
        path: format!("{directory}\0{label}"),
        depth: if directory.is_empty() {
            0
        } else {
            directory.matches('/').count() + 1
        },
        kind,
    }
}

fn rows(paths: &[&str]) -> Vec<VisibleRow> {
    paths.iter().map(|path| entry_row(path)).collect()
}

fn list_at(count: usize, item_ix: usize, offset: f32) -> ListState {
    let list = ListState::new(count, ListAlignment::Top, px(100.));
    list.scroll_to(ListOffset {
        item_ix,
        offset_in_item: px(offset),
    });
    list
}

fn top(list: &ListState) -> (usize, f32) {
    let top = list.logical_scroll_top();
    (top.item_ix, f32::from(top.offset_in_item))
}

// ---- A7: anchors and fade flags ------------------------------------------------------------

#[test]
fn a7_a_loading_row_and_then_children_keep_the_viewport_on_the_same_path() {
    let old = rows(&["a", "src", "z"]);
    let list = list_at(3, 2, 7.0);
    let with_loading = vec![
        entry_row("a"),
        entry_row("src"),
        status_row("src", RowKind::Loading, "loading"),
        entry_row("z"),
    ];
    sync_list_rows(&list, &old, &with_loading);
    assert_eq!(list.item_count(), 4);
    assert_eq!(top(&list), (3, 7.0));
    let loaded = rows(&["a", "src", "src/a", "src/b", "z"]);
    sync_list_rows(&list, &with_loading, &loaded);
    assert_eq!(list.item_count(), 5);
    assert_eq!(top(&list), (4, 7.0));
}

#[test]
fn a7_the_list_is_spliced_only_between_the_common_prefix_and_suffix() {
    let old = rows(&["a", "src", "z"]);
    let new = rows(&["a", "src", "src/a", "src/b", "z"]);
    assert_eq!(splice_plan(&old, &new), Some((2..2, 2)));
    assert_eq!(splice_plan(&new, &old), Some((2..4, 0)));
    assert_eq!(splice_plan(&old, &old), None);
    assert_eq!(splice_plan(&[], &old), Some((0..0, 3)));
    assert_eq!(
        splice_plan(&rows(&["a", "b", "c"]), &rows(&["a", "x", "c"])),
        Some((1..2, 1))
    );
}

#[test]
fn a7_collapsing_the_anchors_directory_moves_the_anchor_to_it() {
    let old = rows(&["a", "src", "src/child", "z"]);
    let list = list_at(4, 2, 5.0);
    sync_list_rows(&list, &old, &rows(&["a", "src", "z"]));
    // The anchor row src/child vanished: its nearest surviving ancestor src.
    assert_eq!(top(&list), (1, 0.0));
}

#[test]
fn a7_a_removed_anchor_falls_back_to_the_nearest_neighbor_forward_then_backward() {
    let old = rows(&["a", "b", "c", "d"]);
    // Removing b (the anchor): the first surviving row scanning forward is c, now at index 1.
    let list = list_at(4, 1, 9.0);
    sync_list_rows(&list, &old, &rows(&["a", "c", "d"]));
    assert_eq!(top(&list), (1, 0.0));
    // Removing the last rows (anchor d): forward finds nothing, backward finds c.
    let list = list_at(4, 3, 4.0);
    sync_list_rows(&list, &old, &rows(&["a", "b", "c"]));
    assert_eq!(top(&list), (2, 0.0));
    // Nothing survives: index 0, offset 0.
    let list = list_at(4, 2, 4.0);
    sync_list_rows(&list, &old, &rows(&["x", "y"]));
    assert_eq!(top(&list), (0, 0.0));
    assert_eq!(resolve_anchor(&old, &rows(&["x"]), 3, px(2.)), (0, px(0.)));
}

#[test]
fn a7_an_empty_new_list_sets_count_and_scroll_top_to_zero() {
    let old = rows(&["a", "b", "c"]);
    let list = list_at(3, 2, 3.0);
    sync_list_rows(&list, &old, &[]);
    assert_eq!(list.item_count(), 0);
    assert_eq!(top(&list), (0, 0.0));
    // And rows arriving in an empty list land at the top.
    sync_list_rows(&list, &[], &old);
    assert_eq!(list.item_count(), 3);
    assert_eq!(top(&list), (0, 0.0));
}

#[test]
fn a7_the_anchor_index_is_clamped_to_the_old_length() {
    let old = rows(&["a", "b"]);
    let list = list_at(2, 0, 0.0);
    // A stale scroll top beyond the old rows resolves against the last old row.
    assert_eq!(resolve_anchor(&old, &rows(&["b"]), 9, px(5.)), (0, px(5.)));
    sync_list_rows(&list, &old, &rows(&["a", "b", "c"]));
    assert_eq!(top(&list), (0, 0.0));
}

#[test]
fn a7_fade_flags_at_scroll_start_middle_end_and_after_clamping() {
    let max = px(100.);
    assert_eq!(fade_flags(px(0.), max), (false, true));
    assert_eq!(fade_flags(px(50.), max), (true, true));
    assert_eq!(fade_flags(px(100.), max), (true, false));
    // A taller viewport or removed rows clamp the scroll: no content overflows.
    assert_eq!(fade_flags(px(0.), px(0.)), (false, false));
    // The tolerance is 0.5 px.
    assert_eq!(fade_flags(px(0.5), max), (false, true));
    assert_eq!(fade_flags(px(0.51), max), (true, true));
    assert_eq!(fade_flags(px(99.5), max), (true, false));
    assert_eq!(fade_flags(px(99.49), max), (true, true));
}

// ---- A8: drag ------------------------------------------------------------------------------

#[test]
fn a8_edge_scroll_speed_landmarks() {
    let (top, bottom) = (100.0, 400.0);
    assert_eq!(
        edge_scroll_speed(50.0, top, bottom),
        0.0,
        "above the viewport"
    );
    assert_eq!(
        edge_scroll_speed(450.0, top, bottom),
        0.0,
        "below the viewport"
    );
    assert_eq!(edge_scroll_speed(250.0, top, bottom), 0.0, "middle");
    assert_eq!(edge_scroll_speed(100.0, top, bottom), -540.0);
    assert_eq!(
        edge_scroll_speed(114.0, top, bottom),
        -270.0,
        "half-way into the 28 px band"
    );
    assert_eq!(edge_scroll_speed(400.0, top, bottom), 540.0);
    assert_eq!(edge_scroll_speed(386.0, top, bottom), 270.0);
    assert_eq!(edge_scroll_speed(128.0, top, bottom), 0.0, "band edge");
    // A short viewport narrows the band to half its height.
    assert_eq!(edge_scroll_speed(110.0, 100.0, 120.0), 0.0);
    assert_eq!(edge_scroll_speed(105.0, 100.0, 120.0), -270.0);
}

fn item(path: &str, kind: EntryKind) -> DragItem {
    DragItem {
        path: path.to_owned(),
        kind,
    }
}

#[test]
fn a8_multi_drop_skips_items_already_in_the_destination() {
    let items = [
        item("a.txt", EntryKind::File),
        item("dest/b.txt", EntryKind::File),
        item("src", EntryKind::Directory),
    ];
    assert_eq!(
        destination_paths(&items, "dest"),
        Some(vec![
            ("a.txt".to_owned(), "dest/a.txt".to_owned()),
            ("src".to_owned(), "dest/src".to_owned()),
        ])
    );
    // Root destinations use the bare name.
    assert_eq!(
        destination_paths(&[item("dest/b.txt", EntryKind::File)], ""),
        Some(vec![("dest/b.txt".to_owned(), "b.txt".to_owned())])
    );
    // Nothing left to move is invalid.
    assert_eq!(
        destination_paths(&[item("dest/b.txt", EntryKind::File)], "dest"),
        None
    );
    assert_eq!(destination_paths(&[], "dest"), None);
}

#[test]
fn a8_a_folder_into_itself_or_its_subtree_invalidates_the_whole_gesture() {
    let items = [
        item("a.txt", EntryKind::File),
        item("src", EntryKind::Directory),
    ];
    assert_eq!(destination_paths(&items, "src"), None);
    assert_eq!(destination_paths(&items, "src/inner"), None);
    // A file named like a directory prefix is not a subtree.
    assert!(destination_paths(&[item("src", EntryKind::Directory)], "srcx").is_some());
}

#[test]
fn a8_two_items_resolving_to_the_same_name_invalidate_the_whole_gesture() {
    let items = [
        item("x/a.txt", EntryKind::File),
        item("y/a.txt", EntryKind::File),
    ];
    assert_eq!(destination_paths(&items, "dest"), None);
    assert!(destination_paths(&items[..1], "dest").is_some());
}

fn page(directory: &str, entries: Vec<Entry>) -> Page {
    Page {
        directory: directory.to_owned(),
        entries,
        next_cursor: None,
    }
}

fn drag_model() -> TreeModel {
    let mut model = TreeModel::new();
    let generation = model.generation();
    model.request_loads();
    model
        .apply_page(
            page(
                "",
                vec![
                    Entry::directory("folder"),
                    Entry::directory("src"),
                    Entry::file("a.txt"),
                    Entry::symlink("link"),
                ],
            ),
            generation,
        )
        .unwrap();
    model.expand("src");
    model
        .apply_page(page("src", vec![Entry::file("src/lib.rs")]), generation)
        .unwrap();
    model
}

#[test]
fn a8_destination_resolution_for_folder_file_symlink_root_row_and_empty_space() {
    let model = drag_model();
    let ix = |path: &str| model.row_index(path).unwrap();
    assert_eq!(
        resolve_destination(&model, DropHit::Row(ix("folder"))),
        Some("folder".to_owned())
    );
    assert_eq!(
        resolve_destination(&model, DropHit::Row(ix("src/lib.rs"))),
        Some("src".to_owned())
    );
    assert_eq!(
        resolve_destination(&model, DropHit::Row(ix("a.txt"))),
        Some(String::new())
    );
    assert_eq!(resolve_destination(&model, DropHit::Row(ix("link"))), None);
    assert_eq!(
        resolve_destination(&model, DropHit::RootRow),
        Some(String::new())
    );
    assert_eq!(
        resolve_destination(&model, DropHit::Empty),
        Some(String::new())
    );
    assert_eq!(resolve_destination(&model, DropHit::Outside), None);
    assert_eq!(resolve_destination(&model, DropHit::Rail), None);
    assert_eq!(resolve_destination(&model, DropHit::Row(99)), None);
}

#[test]
fn a8_the_payload_carries_selection_roots_once_each_in_visible_order() {
    let mut model = drag_model();
    model.select("a.txt");
    model.toggle_selected("src/lib.rs");
    model.toggle_selected("src");
    // Dragging a selected row of a multi-selection moves the roots (src once, not src/lib.rs).
    let payload = TreeDragPayload::for_drag(&model, "a.txt").unwrap();
    let paths: Vec<&str> = payload
        .items
        .iter()
        .map(|item| item.path.as_str())
        .collect();
    assert_eq!(paths, ["src", "a.txt"]);
    assert_eq!(payload.dragged.path, "a.txt");
    assert_eq!(
        destination_paths(&payload.items, "folder"),
        Some(vec![
            ("src".to_owned(), "folder/src".to_owned()),
            ("a.txt".to_owned(), "folder/a.txt".to_owned()),
        ])
    );
    // An unselected row drags alone even when other rows are selected.
    let alone = TreeDragPayload::for_drag(&model, "folder").unwrap();
    assert_eq!(alone.items.len(), 1);
    // A single selected row also drags alone.
    model.select("a.txt");
    assert_eq!(
        TreeDragPayload::for_drag(&model, "a.txt")
            .unwrap()
            .items
            .len(),
        1
    );
}

fn session() -> DragSession {
    DragSession::new(TreeDragPayload {
        dragged: item("a.txt", EntryKind::File),
        items: vec![item("a.txt", EntryKind::File)].into(),
    })
}

#[test]
fn a8_the_tick_autoscrolls_with_the_capped_elapsed_time() {
    let mut drag = session();
    drag.set_pointer_y(Some(100.0));
    // At the very top: -540 px/s for 16 ms.
    let out = drag.tick(Duration::from_millis(16), 100.0, 400.0);
    assert!((out.scroll - (-540.0 * 0.016)).abs() < 1e-3);
    // A long gap between ticks is capped at 0.05 s.
    let out = drag.tick(Duration::from_secs(2), 100.0, 400.0);
    assert!((out.scroll - (-540.0 * 0.05)).abs() < 1e-3);
    // In the middle there is no scroll; outside the viewport (no pointer) neither.
    drag.set_pointer_y(Some(250.0));
    assert_eq!(
        drag.tick(Duration::from_millis(16), 100.0, 400.0).scroll,
        0.0
    );
    drag.set_pointer_y(None);
    assert_eq!(
        drag.tick(Duration::from_millis(16), 100.0, 400.0).scroll,
        0.0
    );
}

#[test]
fn a8_hovering_a_collapsed_directory_expands_it_after_650_ms_once() {
    let mut drag = session();
    assert!(drag.set_destination(Some("folder".to_owned()), true, true));
    assert!(drag.is_valid());
    let mut expanded = None;
    for tick in 1..=50 {
        let out = drag.tick(Duration::from_millis(16), 0.0, 400.0);
        if out.expand.is_some() {
            assert_eq!(tick, 41, "the 41st tick reaches 656 ms");
            expanded = out.expand;
        }
    }
    assert_eq!(expanded.as_deref(), Some("folder"));
    // Changing the destination restarts the timer (and the highlight changes immediately).
    assert!(drag.set_destination(Some("other".to_owned()), true, true));
    assert_eq!(drag.destination(), Some("other"));
    assert!(
        drag.tick(Duration::from_millis(600), 0.0, 400.0)
            .expand
            .is_none()
    );
    assert_eq!(
        drag.tick(Duration::from_millis(60), 0.0, 400.0)
            .expand
            .as_deref(),
        Some("other")
    );
    // An expanded (non-expandable) destination never fires.
    drag.set_destination(Some("src".to_owned()), true, false);
    assert!(
        drag.tick(Duration::from_secs(5), 0.0, 400.0)
            .expand
            .is_none()
    );
}

// ---- A9: search ----------------------------------------------------------------------------

fn result(path: &str, kind: EntryKind, score: f64) -> SearchResult {
    SearchResult::new(path, kind, score)
}

#[test]
fn a9_results_group_under_their_real_ancestors_with_best_scores() {
    let mut search = SearchTree::new();
    search.rebuild(&[
        result("a/b/x.ts", EntryKind::File, 5.0),
        result("a/y.ts", EntryKind::File, 9.0),
    ]);
    let best = |path: &str| search.node(path).unwrap().best_score;
    assert_eq!(best("a"), 9.0);
    assert_eq!(best("a/b"), 5.0);
    assert_eq!(best("a/b/x.ts"), 5.0);
    assert_eq!(best("a/y.ts"), 9.0);
    assert_eq!(search.node("a").unwrap().kind, EntryKind::Directory);
    assert_eq!(search.node("a").unwrap().score, None);
    let order: Vec<&str> = search.rows().iter().map(|row| row.path.as_str()).collect();
    assert_eq!(order, ["a", "a/y.ts", "a/b", "a/b/x.ts"]);
    let depths: Vec<usize> = search.rows().iter().map(|row| row.depth).collect();
    assert_eq!(depths, [0, 1, 1, 2]);
}

#[test]
fn a9_a_result_that_is_also_an_ancestor_keeps_its_own_score_and_kind() {
    let mut search = SearchTree::new();
    search.rebuild(&[
        result("a", EntryKind::Directory, 3.0),
        result("a/x.ts", EntryKind::File, 7.0),
    ]);
    let a = search.node("a").unwrap();
    assert_eq!(a.score, Some(3.0));
    assert_eq!(a.best_score, 7.0);
    // Siblings: directories before files only after the score.
    search.rebuild(&[
        result("b.ts", EntryKind::File, 5.0),
        result("d", EntryKind::Directory, 5.0),
        result("c.ts", EntryKind::File, 5.0),
    ]);
    let order: Vec<&str> = search.rows().iter().map(|row| row.path.as_str()).collect();
    assert_eq!(order, ["d", "b.ts", "c.ts"]);
}

#[test]
fn a9_at_most_200_results_are_kept() {
    let mut search = SearchTree::new();
    let results: Vec<SearchResult> = (0..300)
        .map(|n| result(&format!("f{n:03}.ts"), EntryKind::File, f64::from(n)))
        .collect();
    search.rebuild(&results);
    assert_eq!(search.rows().len(), 200);
}

#[test]
fn a9_directory_nodes_collapse_and_restore() {
    let mut search = SearchTree::new();
    search.rebuild(&[
        result("a/b/x.ts", EntryKind::File, 5.0),
        result("a/y.ts", EntryKind::File, 9.0),
    ]);
    assert!(search.toggle("a/b"));
    let order: Vec<&str> = search.rows().iter().map(|row| row.path.as_str()).collect();
    assert_eq!(order, ["a", "a/y.ts", "a/b"]);
    assert!(search.rows()[2].collapsed);
    assert!(search.toggle("a/b"));
    assert_eq!(search.rows().len(), 4);
    assert!(!search.toggle("a/y.ts"), "a leaf cannot collapse");
    // Activating a directory with children toggles it, keeps it active; others reveal.
    assert_eq!(
        search.activate("a"),
        SearchActivation::Toggled("a".to_owned())
    );
    assert_eq!(search.active(), Some("a"));
    assert_eq!(search.rows().len(), 1);
    assert_eq!(search.active_row(), Some(0));
    search.toggle("a");
    assert_eq!(
        search.activate("a/y.ts"),
        SearchActivation::Reveal("a/y.ts".to_owned(), EntryKind::File)
    );
}

#[test]
fn a9_superseded_results_are_ignored() {
    let mut search = SearchTree::new();
    let first = search.begin_query("  foo ");
    let second = search.begin_query("foob");
    assert_eq!(search.query(), "foob");
    let results = [result("a.ts", EntryKind::File, 1.0)];
    assert!(
        !search.accept_results(first, "foo", &results),
        "older generation"
    );
    assert!(
        !search.accept_results(second, "fo", &results),
        "other query text"
    );
    assert!(
        search.accept_results(second, " foob ", &results),
        "trimmed text matches"
    );
    assert_eq!(search.rows().len(), 1);
}

fn reveal_model() -> (TreeModel, u64) {
    let mut model = TreeModel::new();
    let generation = model.generation();
    model.request_loads();
    model
        .apply_page(
            page("", vec![Entry::directory("src"), Entry::file("a.txt")]),
            generation,
        )
        .unwrap();
    (model, generation)
}

#[test]
fn a9_synchronize_selection_leaves_query_and_results_untouched_and_opens_nothing() {
    let mut search = SearchTree::new();
    let generation = search.begin_query("lib");
    search.accept_results(
        generation,
        "lib",
        &[result("src/lib.rs", EntryKind::File, 4.0)],
    );
    let (mut model, tree_generation) = reveal_model();
    let mut coordinator = RevealCoordinator::new();
    let request = coordinator.begin("src/lib.rs", RevealIntent::SynchronizeSelection, &model, 1);
    let RevealProgress::Load(load) = coordinator.step(&request, &mut model, 1) else {
        panic!("loads src");
    };
    assert_eq!(
        load,
        LoadRequest {
            path: "src".to_owned(),
            cursor: None,
            generation: tree_generation
        }
    );
    model
        .apply_page(
            page("src", vec![Entry::file("src/lib.rs")]),
            tree_generation,
        )
        .unwrap();
    let RevealProgress::Done(effect) = coordinator.step(&request, &mut model, 1) else {
        panic!("done");
    };
    assert_eq!(effect.open_file, None);
    assert!(!effect.clear_search);
    search.apply_reveal(&effect);
    assert_eq!(search.query(), "lib");
    assert_eq!(search.rows().len(), 2);
    assert_eq!(model.selected_paths(), ["src/lib.rs"]);
    assert!(model.is_expanded("src"));
}

#[test]
fn a9_open_file_and_activate_directory_clear_the_search() {
    for (intent, opens) in [
        (RevealIntent::OpenFile, true),
        (RevealIntent::ActivateDirectory, false),
    ] {
        let mut search = SearchTree::new();
        let generation = search.begin_query("a");
        search.accept_results(generation, "a", &[result("a.txt", EntryKind::File, 4.0)]);
        let (mut model, _) = reveal_model();
        let mut coordinator = RevealCoordinator::new();
        let request = coordinator.begin("a.txt", intent, &model, 0);
        let RevealProgress::Done(effect) = coordinator.step(&request, &mut model, 0) else {
            panic!("done");
        };
        assert_eq!(effect.open_file.is_some(), opens);
        search.apply_reveal(&effect);
        assert_eq!(search.query(), "");
        assert!(search.rows().is_empty());
        // Results of the cleared search are ignored.
        assert!(!search.accept_results(generation, "a", &[result("a.txt", EntryKind::File, 4.0)]));
    }
}

#[test]
fn a9_a_newer_reveal_or_a_context_switch_rejects_an_older_reveal() {
    let (mut model, tree_generation) = reveal_model();
    let mut coordinator = RevealCoordinator::new();
    let older = coordinator.begin("src/lib.rs", RevealIntent::OpenFile, &model, 1);
    let newer = coordinator.begin("a.txt", RevealIntent::SynchronizeSelection, &model, 1);
    assert_eq!(
        coordinator.step(&older, &mut model, 1),
        RevealProgress::Dropped
    );
    assert!(matches!(
        coordinator.step(&newer, &mut model, 1),
        RevealProgress::Done(_)
    ));
    // A checkout switch (context change) drops the reveal.
    let request = coordinator.begin("src/lib.rs", RevealIntent::OpenFile, &model, 1);
    assert_eq!(
        coordinator.step(&request, &mut model, 2),
        RevealProgress::Dropped
    );
    // A tree generation change drops it too.
    let request = coordinator.begin("src/lib.rs", RevealIntent::OpenFile, &model, 1);
    model.invalidate_loads();
    assert_eq!(
        coordinator.step(&request, &mut model, 1),
        RevealProgress::Dropped
    );
    let _ = tree_generation;
}

#[test]
fn a9_a_load_error_surfaces_as_a_search_error_without_changing_the_tree() {
    let (mut model, tree_generation) = reveal_model();
    let mut search = SearchTree::new();
    let mut coordinator = RevealCoordinator::new();
    let request = coordinator.begin("src/lib.rs", RevealIntent::OpenFile, &model, 0);
    let RevealProgress::Load(load) = coordinator.step(&request, &mut model, 0) else {
        panic!("loads src");
    };
    model.fail_load(&load.path, "permission denied", tree_generation);
    let RevealProgress::Failed(message) = coordinator.step(&request, &mut model, 0) else {
        panic!("failed");
    };
    search.set_error(Some(message));
    assert_eq!(search.error(), Some("permission denied"));
    assert!(model.selected_paths().is_empty());
    assert!(!model.is_expanded("src"));
}

// ---- A10: rename ---------------------------------------------------------------------------

#[test]
fn a10_basename_selection() {
    assert_eq!(basename_range(".bashrc", false), 0..7);
    assert_eq!(basename_range("a.tar.gz", false), 0..5);
    assert_eq!(
        &"report.final.txt"[basename_range("report.final.txt", false)],
        "report.final"
    );
    assert_eq!(basename_range("README", false), 0..6);
    assert_eq!(
        basename_range("v1.2.d", true),
        0..6,
        "directories select the whole name"
    );
}

#[test]
fn a10_invalid_names_are_rejected_with_the_message() {
    for name in ["", ".", "..", ".git", ".GIT", "a/b", "a\\b", "a\0b", "c:d"] {
        assert_eq!(validate_name(name), Err(INVALID_NAME_MESSAGE), "{name:?}");
    }
    for name in ["a", ".gitignore", "a.b", "git", "ünï"] {
        assert_eq!(validate_name(name), Ok(()), "{name:?}");
    }
    let mut session = RenameSession::new("src/a.txt", Some("r1".to_owned()));
    assert_eq!(
        session.commit("bad/name", Some("r1")),
        RenameCommit::Rejected(INVALID_NAME_MESSAGE.to_owned())
    );
    assert_eq!(session.error(), Some(INVALID_NAME_MESSAGE));
    assert!(!session.in_flight(), "the field stays and can retry");
}

#[test]
fn a10_commit_happens_once_and_an_unchanged_name_just_closes() {
    let mut session = RenameSession::new("src/a.txt", Some("r1".to_owned()));
    assert_eq!(session.commit("a.txt", Some("r1")), RenameCommit::Unchanged);
    assert_eq!(
        session.commit("b.txt", Some("r1")),
        RenameCommit::Request {
            from: "src/a.txt".to_owned(),
            to: "src/b.txt".to_owned()
        }
    );
    assert_eq!(session.commit("b.txt", Some("r1")), RenameCommit::Ignored);
    assert_eq!(session.commit("c.txt", Some("r1")), RenameCommit::Ignored);
    // A failure re-enables the commit; success ends the session.
    assert!(!session.finish(Err("exists".to_owned())));
    assert_eq!(session.error(), Some("exists"));
    assert!(matches!(
        session.commit("b.txt", Some("r1")),
        RenameCommit::Request { .. }
    ));
    assert!(session.finish(Ok(())));
    // A top-level entry renames to a bare name.
    let mut top_level = RenameSession::new("a.txt", None);
    assert_eq!(
        top_level.commit("z.txt", None),
        RenameCommit::Request {
            from: "a.txt".to_owned(),
            to: "z.txt".to_owned()
        }
    );
}

#[test]
fn a10_blur_cancels_unless_a_commit_is_in_flight() {
    let mut session = RenameSession::new("a.txt", Some("r".to_owned()));
    assert!(session.cancels_on_blur());
    session.commit("b.txt", Some("r"));
    assert!(!session.cancels_on_blur());
    session.finish(Err("x".to_owned()));
    assert!(session.cancels_on_blur());
}

#[test]
fn a10_a_changed_revision_blocks_the_commit() {
    let mut session = RenameSession::new("a.txt", Some("r1".to_owned()));
    assert_eq!(
        session.commit("b.txt", Some("r2")),
        RenameCommit::Rejected(ENTRY_CHANGED_MESSAGE.to_owned())
    );
    assert_eq!(
        session.commit("b.txt", None),
        RenameCommit::Rejected(ENTRY_CHANGED_MESSAGE.to_owned())
    );
    assert!(!session.in_flight());
}

#[test]
fn r17_delete_subject_text() {
    let paths = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(delete_subject(&paths(&["src/a.txt"])), "a.txt");
    assert_eq!(delete_subject(&paths(&["a", "src/b"])), "2 items (a and b)");
    assert_eq!(
        delete_subject(&paths(&["a", "src/b", "c"])),
        "3 items (a, b and 1 more)"
    );
    assert_eq!(
        delete_subject(&paths(&["a", "b", "c", "d", "e"])),
        "5 items (a, b and 3 more)"
    );
}

// ---- A5: context menu model ----------------------------------------------------------------

#[test]
fn a5_rename_is_enabled_for_exactly_one_mutable_path_and_delete_for_all_mutable() {
    let owner = vec![("add".to_owned(), "Add to conversation".to_owned())];
    let enabled = |items: &[vespertine_components::tree::MenuItem], id: &str| {
        items.iter().find(|item| item.id == id).unwrap().enabled
    };
    let one = menu_items(&owner, 1, &[true], &[true]);
    assert!(enabled(&one, "add") && enabled(&one, "copy-path"));
    assert!(enabled(&one, "rename") && enabled(&one, "delete"));
    assert_eq!(one.last().unwrap().label, "Delete…");
    let two = menu_items(&owner, 2, &[true, true], &[true, true]);
    assert!(!enabled(&two, "rename"));
    assert!(enabled(&two, "delete"));
    assert_eq!(two.last().unwrap().label, "Delete 2 items…");
    let mixed = menu_items(&owner, 2, &[true, false], &[true, false]);
    assert!(!enabled(&mixed, "delete"));
    assert!(
        enabled(&mixed, "add"),
        "non-mutating owner items are always enabled"
    );
    let immutable = menu_items(&owner, 1, &[false], &[false]);
    assert!(!enabled(&immutable, "rename") && !enabled(&immutable, "delete"));
}

#[test]
fn a5_menu_navigation_skips_disabled_items_and_wraps() {
    let items = menu_items(&[], 1, &[false], &[true]);
    // copy-path (enabled), rename (disabled), delete (enabled)
    let mut menu = ContextMenu::new(vec!["a".to_owned()], items);
    assert_eq!(menu.active(), None);
    menu.next();
    assert_eq!(menu.active(), Some(0));
    menu.next();
    assert_eq!(menu.active(), Some(2), "the disabled item is skipped");
    menu.next();
    assert_eq!(menu.active(), Some(0), "wraps");
    menu.previous();
    assert_eq!(menu.active(), Some(2), "wraps backwards");
    assert_eq!(menu.activate().unwrap().id, "delete");
    menu.set_active(Some(1));
    assert_eq!(menu.active(), None, "a disabled item cannot become active");
    let mut empty = ContextMenu::new(vec![], vec![]);
    empty.next();
    assert_eq!(empty.activate(), None);
}

#[test]
fn a5_menu_home_and_end_jump_to_the_first_and_last_enabled_item() {
    let items = menu_items(&[], 1, &[false], &[false]);
    // copy-path is the only enabled item.
    let mut menu = ContextMenu::new(vec!["a".to_owned()], items);
    menu.last();
    assert_eq!(menu.active(), Some(0));
    let items = menu_items(&[], 1, &[true], &[false]);
    let mut menu = ContextMenu::new(vec!["a".to_owned()], items);
    menu.last();
    assert_eq!(menu.active(), Some(1), "delete is disabled, rename is last");
    menu.first();
    assert_eq!(menu.active(), Some(0));
}

#[test]
fn a5_copy_path_resolves_by_the_hosts_rules_joined_by_newlines_in_order() {
    let paths = vec!["src/a.rs".to_owned(), "b.txt".to_owned()];
    let text = copy_path_text(&paths, |path| {
        format!("C:\\repo\\{}", path.replace('/', "\\"))
    });
    assert_eq!(text, "C:\\repo\\src\\a.rs\nC:\\repo\\b.txt");
    assert_eq!(copy_path_text(&paths, str::to_owned), "src/a.rs\nb.txt");
}

// ---- R8: wash alphas -----------------------------------------------------------------------

#[test]
fn r8_row_washes_use_the_default_alphas() {
    let washes = TreeWashes::default();
    let base = RowVisual {
        focused: true,
        ..RowVisual::default()
    };
    let selected = RowVisual {
        selected: true,
        ..base
    };
    assert_eq!(washes.row_alpha(&base), None);
    assert_eq!(washes.row_alpha(&selected), Some(0.12));
    assert_eq!(
        washes.row_alpha(&RowVisual {
            focused: false,
            ..selected
        }),
        Some(0.08)
    );
    // The cursor is a distinct row only when unselected or among several selected entries.
    let cursor_alone = RowVisual {
        is_cursor: true,
        ..selected
    };
    assert_eq!(washes.row_alpha(&cursor_alone), Some(0.12));
    assert_eq!(
        washes.row_alpha(&RowVisual {
            multiple_selected: true,
            ..cursor_alone
        }),
        Some(0.16)
    );
    assert_eq!(
        washes.row_alpha(&RowVisual {
            is_cursor: true,
            ..base
        }),
        Some(0.04)
    );
    // Without focus the cursor is not marked.
    assert_eq!(
        washes.row_alpha(&RowVisual {
            is_cursor: true,
            focused: false,
            ..RowVisual::default()
        }),
        None
    );
    // Hover tints unselected rows only; drop targets win.
    assert_eq!(
        washes.row_alpha(&RowVisual {
            hovered: true,
            ..base
        }),
        Some(0.055)
    );
    assert_eq!(
        washes.row_alpha(&RowVisual {
            hovered: true,
            ..selected
        }),
        Some(0.12)
    );
    assert_eq!(
        washes.row_alpha(&RowVisual {
            drop_target: true,
            ..selected
        }),
        Some(0.18)
    );
    assert_eq!(washes.root_drop, 0.16);
}
