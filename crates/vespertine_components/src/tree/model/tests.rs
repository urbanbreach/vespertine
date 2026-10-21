//! Unit tests of the pure tree model (CMP-016 A2, A3, A4 and the model requirements).

use super::*;

fn page(directory: &str, entries: Vec<Entry>, next: Option<&str>) -> Page {
    Page {
        directory: directory.to_owned(),
        entries,
        next_cursor: next.map(str::to_owned),
    }
}

/// The A3 fixture: `src/` (lib.rs, main.rs), a.txt, b.txt, c.txt with `src` expanded.
fn fixture() -> TreeModel {
    let mut model = TreeModel::new();
    let generation = model.generation();
    assert_eq!(model.request_loads().len(), 1);
    model
        .apply_page(
            page(
                "",
                vec![
                    Entry::file("c.txt"),
                    Entry::file("a.txt"),
                    Entry::directory("src"),
                    Entry::file("b.txt"),
                ],
                None,
            ),
            generation,
        )
        .unwrap();
    let outcome = model.expand("src");
    assert!(outcome.load.is_some());
    model
        .apply_page(
            page(
                "src",
                vec![Entry::file("src/main.rs"), Entry::file("src/lib.rs")],
                None,
            ),
            generation,
        )
        .unwrap();
    model
}

fn row_paths(model: &TreeModel) -> Vec<&str> {
    model
        .visible_rows()
        .iter()
        .map(|row| row.path.as_str())
        .collect()
}

fn selected(model: &TreeModel) -> Vec<String> {
    model.selected_paths()
}

#[test]
fn children_are_ordered_directories_first_then_files_then_symlinks_case_insensitively() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    model.request_loads();
    model
        .apply_page(
            page(
                "",
                vec![
                    Entry::symlink("link"),
                    Entry::file("b.txt"),
                    Entry::file("A.txt"),
                    Entry::directory("zeta"),
                    Entry::directory("Alpha"),
                ],
                None,
            ),
            generation,
        )
        .unwrap();
    assert_eq!(
        row_paths(&model),
        ["Alpha", "zeta", "A.txt", "b.txt", "link"]
    );
}

#[test]
fn ignored_propagates_to_descendants() {
    let mut model = fixture();
    assert!(!model.is_ignored("src/lib.rs"));
    let generation = model.generation();
    model
        .apply_page(
            page(
                "",
                vec![
                    Entry::directory("src").ignored(true),
                    Entry::file("a.txt"),
                    Entry::file("b.txt"),
                    Entry::file("c.txt"),
                ],
                None,
            ),
            generation,
        )
        .unwrap();
    assert!(model.is_ignored("src"));
    assert!(model.is_ignored("src/lib.rs"));
    assert!(!model.is_ignored("a.txt"));
}

#[test]
fn a3_selection_arithmetic_on_the_fixture() {
    let mut model = fixture();
    assert_eq!(
        row_paths(&model),
        [
            "src",
            "src/lib.rs",
            "src/main.rs",
            "a.txt",
            "b.txt",
            "c.txt"
        ]
    );
    // select a; toggle c -> [a, c]
    assert!(model.select("a.txt"));
    assert!(model.toggle_selected("c.txt"));
    assert_eq!(selected(&model), ["a.txt", "c.txt"]);
    // toggle a -> [c] with the cursor on a
    assert!(model.toggle_selected("a.txt"));
    assert_eq!(selected(&model), ["c.txt"]);
    assert_eq!(model.cursor(), Some("a.txt"));
    // Shift to src/lib.rs -> [src/lib.rs, src/main.rs, a.txt]
    assert!(model.select_range_to("src/lib.rs", false));
    assert_eq!(selected(&model), ["src/lib.rs", "src/main.rs", "a.txt"]);
    assert_eq!(model.cursor(), Some("src/lib.rs"));
    assert_eq!(model.anchor(), Some("a.txt"));
    // Shift to b.txt -> [a.txt, b.txt]
    assert!(model.select_range_to("b.txt", false));
    assert_eq!(selected(&model), ["a.txt", "b.txt"]);
    // toggle src then additive Shift to src/lib.rs
    assert!(model.toggle_selected("src"));
    assert!(model.select_range_to("src/lib.rs", true));
    assert_eq!(selected(&model), ["src", "src/lib.rs", "a.txt", "b.txt"]);
    assert_eq!(model.selection_roots(), ["src", "a.txt", "b.txt"]);
    // Escape -> [src/lib.rs]
    assert!(model.collapse_selection());
    assert_eq!(selected(&model), ["src/lib.rs"]);
    assert!(
        !model.collapse_selection(),
        "already collapsed reports no change"
    );
    // clear -> empty and cursor none
    assert!(model.clear_selection());
    assert!(selected(&model).is_empty());
    assert_eq!(model.cursor(), None);
    assert!(!model.clear_selection());
}

#[test]
fn a3_shift_down_extends_to_c_and_shrinks_back_past_a() {
    let mut model = fixture();
    model.select("a.txt");
    assert!(model.extend_selection(1));
    assert_eq!(selected(&model), ["a.txt", "b.txt"]);
    assert!(model.extend_selection(1));
    assert_eq!(selected(&model), ["a.txt", "b.txt", "c.txt"]);
    assert!(!model.extend_selection(1) || selected(&model) == ["a.txt", "b.txt", "c.txt"]);
    assert!(model.extend_selection(-1));
    assert!(model.extend_selection(-1));
    assert_eq!(selected(&model), ["a.txt"]);
    assert!(model.extend_selection(-1));
    assert_eq!(selected(&model), ["src/main.rs", "a.txt"]);
    assert_eq!(model.anchor(), Some("a.txt"));
}

#[test]
fn shift_arrow_without_an_anchor_anchors_at_the_starting_cursor() {
    let mut model = fixture();
    model.toggle_selected("b.txt");
    model.toggle_selected("b.txt");
    assert_eq!(model.cursor(), Some("b.txt"));
    assert!(model.extend_selection(1));
    assert_eq!(selected(&model), ["b.txt", "c.txt"]);
}

#[test]
fn plain_navigation_moves_over_selectable_rows() {
    let mut model = fixture();
    assert!(model.select_next());
    assert_eq!(selected(&model), ["src"]);
    assert!(model.select_next());
    assert_eq!(selected(&model), ["src/lib.rs"]);
    assert!(model.select_previous());
    assert_eq!(selected(&model), ["src"]);
    // Left on an expanded directory collapses it, Right on a collapsed one expands it.
    assert!(model.select_parent().changed);
    assert!(!model.is_expanded("src"));
    let right = model.select_first_child();
    assert!(right.changed);
    assert!(model.is_expanded("src"));
    // Right on an expanded directory selects its first child.
    assert!(model.select_first_child().changed);
    assert_eq!(model.cursor(), Some("src/lib.rs"));
    // Left on a child selects the parent; on a top-level entry it stays.
    assert!(model.select_parent().changed);
    assert_eq!(model.cursor(), Some("src"));
    model.collapse("src");
    model.select("a.txt");
    assert!(!model.select_parent().changed);
    assert_eq!(model.cursor(), Some("a.txt"));
}

#[test]
fn a2_collapsing_an_ancestor_prunes_the_selection_and_moves_a_selected_hidden_cursor() {
    let mut model = fixture();
    model.select("a.txt");
    model.toggle_selected("src/lib.rs");
    model.toggle_selected("src/main.rs");
    assert_eq!(selected(&model), ["src/lib.rs", "src/main.rs", "a.txt"]);
    assert_eq!(model.cursor(), Some("src/main.rs"));
    assert!(model.collapse("src"));
    // The hidden entries left the selection; the selected hidden cursor moved to the folder,
    // which joined the selection.
    assert_eq!(selected(&model), ["src", "a.txt"]);
    assert_eq!(model.cursor(), Some("src"));
    assert_eq!(model.anchor(), None);
}

#[test]
fn a2_a_deselected_hidden_cursor_does_not_pull_its_folder_in() {
    let mut model = fixture();
    model.select("a.txt");
    model.toggle_selected("src/lib.rs");
    model.toggle_selected("src/lib.rs");
    assert_eq!(model.cursor(), Some("src/lib.rs"));
    assert_eq!(selected(&model), ["a.txt"]);
    assert!(model.collapse("src"));
    assert_eq!(model.cursor(), Some("src"));
    assert_eq!(selected(&model), ["a.txt"]);
}

#[test]
fn a2_selection_roots_skip_selected_descendants() {
    let mut model = fixture();
    model.select("src");
    model.toggle_selected("src/lib.rs");
    model.toggle_selected("c.txt");
    assert_eq!(selected(&model), ["src", "src/lib.rs", "c.txt"]);
    assert_eq!(model.selection_roots(), ["src", "c.txt"]);
}

#[test]
fn a2_relocation_rekeys_selection_cursor_anchor_and_expansion() {
    let mut model = fixture();
    model.select("src");
    model.toggle_selected("src/lib.rs");
    assert!(model.relocate_subtree("src", "lib"));
    assert_eq!(
        row_paths(&model),
        [
            "lib",
            "lib/lib.rs",
            "lib/main.rs",
            "a.txt",
            "b.txt",
            "c.txt"
        ]
        .iter()
        .copied()
        .collect::<Vec<_>>()
        .as_slice()
    );
    assert!(model.is_expanded("lib"));
    assert_eq!(selected(&model), ["lib", "lib/lib.rs"]);
    assert_eq!(model.cursor(), Some("lib/lib.rs"));
    assert_eq!(model.anchor(), Some("lib/lib.rs"));
    assert_eq!(model.node("lib").unwrap().entry.name, "lib");
    // Both parents are stale and the moved directory refreshes on the next load.
    assert!(model.node("").unwrap().stale);
    assert!(model.node("lib").unwrap().stale);
}

#[test]
fn a2_relocation_refuses_a_move_onto_itself_or_into_its_own_subtree() {
    let mut model = fixture();
    assert!(!model.relocate_subtree("src", "src"));
    assert!(!model.relocate_subtree("src", "src/inner"));
    assert!(!model.relocate_subtree("missing", "other"));
    // Moving a file into a directory expands the destination.
    model.collapse("src");
    assert!(model.relocate_subtree("a.txt", "src/a.txt"));
    assert!(model.is_expanded("src"));
    assert!(row_paths(&model).contains(&"src/a.txt"));
}

#[test]
fn a2_removing_the_selected_entry_moves_the_cursor_to_its_parent() {
    let mut model = fixture();
    model.select("src/lib.rs");
    assert!(model.remove("src/lib.rs"));
    assert_eq!(model.cursor(), Some("src"));
    assert_eq!(selected(&model), ["src"]);
    assert!(!row_paths(&model).contains(&"src/lib.rs"));
}

#[test]
fn removing_never_moves_the_cursor_to_the_root_and_keeps_outside_selection_unchanged() {
    let mut model = fixture();
    model.select("a.txt");
    assert!(model.remove("a.txt"));
    assert_eq!(model.cursor(), None);
    assert!(selected(&model).is_empty());

    let mut model = fixture();
    model.select("c.txt");
    model.toggle_selected("src/lib.rs");
    assert!(model.remove("src/lib.rs"));
    // Not every selected entry lay inside the removed subtree: the parent does not join.
    assert_eq!(model.cursor(), Some("src"));
    assert_eq!(selected(&model), ["c.txt"]);
}

#[test]
fn a4_a_failed_load_shows_the_retry_row_and_retries_with_the_same_cursor() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    let request = model.request_loads().remove(0);
    assert_eq!(request.cursor, None);
    assert_eq!(model.visible_rows()[0].kind, RowKind::Loading);
    assert!(model.fail_load("", "disk on fire", generation));
    let error = model.visible_rows()[0].clone();
    assert_eq!(error.kind, RowKind::Error("disk on fire".to_owned()));
    let Activation::Load(Some(retry)) = model.activate(&error.path) else {
        panic!("the error row retries");
    };
    assert_eq!(retry.cursor, None);
    assert_eq!(model.visible_rows()[0].kind, RowKind::Loading);

    // A failed follow-up page keeps its cursor.
    model
        .apply_page(page("", vec![Entry::file("a")], Some("c1")), generation)
        .unwrap();
    let more = model.load_more("").unwrap();
    assert_eq!(more.cursor.as_deref(), Some("c1"));
    assert!(model.fail_load("", "timeout", generation));
    let error = model.visible_rows().last().unwrap().clone();
    assert_eq!(error.kind, RowKind::Error("timeout".to_owned()));
    let Activation::Load(Some(retry)) = model.activate(&error.path) else {
        panic!("the error row retries");
    };
    assert_eq!(retry.cursor.as_deref(), Some("c1"));
    // The rows loaded so far stay.
    assert_eq!(model.visible_rows()[0].path, "a");
}

#[test]
fn a4_status_rows_follow_the_load_state() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    // Unloaded: no status row.
    assert!(model.visible_rows().is_empty());
    model.request_loads();
    assert_eq!(model.visible_rows().len(), 1);
    assert_eq!(model.visible_rows()[0].kind, RowKind::Loading);
    // Loaded and empty: an Empty row.
    model
        .apply_page(page("", vec![], None), generation)
        .unwrap();
    assert_eq!(model.visible_rows()[0].kind, RowKind::Empty);
    // A refresh of a directory that loaded before keeps old rows and shows no Loading row.
    model.invalidate_directory("");
    let refresh = model.expand("").load.unwrap();
    assert_eq!(refresh.cursor, None);
    assert_eq!(model.visible_rows()[0].kind, RowKind::Empty);
    model
        .apply_page(page("", vec![Entry::file("a")], Some("c1")), generation)
        .unwrap();
    let kinds: Vec<_> = model
        .visible_rows()
        .iter()
        .map(|r| r.kind.clone())
        .collect();
    assert_eq!(kinds, [RowKind::Entry, RowKind::LoadMore("c1".to_owned())]);
    // A further page shows a Loading row below the loaded rows.
    model.load_more("").unwrap();
    let kinds: Vec<_> = model
        .visible_rows()
        .iter()
        .map(|r| r.kind.clone())
        .collect();
    assert_eq!(kinds, [RowKind::Entry, RowKind::Loading]);
    // Status rows have unique synthetic paths, are never selected and only LoadMore holds the cursor.
    assert!(!model.select("\0loading"));
}

#[test]
fn a4_status_rows_sit_at_the_depth_of_their_directorys_children() {
    let mut model = fixture();
    let generation = model.generation();
    model
        .apply_page(
            page("src", vec![Entry::file("src/lib.rs")], Some("n")),
            generation,
        )
        .unwrap();
    let rows = model.visible_rows();
    let more = rows
        .iter()
        .find(|row| matches!(row.kind, RowKind::LoadMore(_)))
        .unwrap();
    assert_eq!(more.depth, 1);
    assert_eq!(more.status_directory(), Some("src"));
    assert!(model.focus_row_path(&more.path.clone()));
    assert_eq!(model.cursor(), Some("src\0load-more"));
    assert!(selected(&model).is_empty());
    // Collapsing the directory moves a cursor on its load-more row to the directory.
    model.collapse("src");
    assert_eq!(model.cursor(), Some("src"));
}

#[test]
fn a4_pagination_appends_and_deduplicates() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    model.request_loads();
    model
        .apply_page(
            page("", vec![Entry::file("a"), Entry::file("b")], Some("c1")),
            generation,
        )
        .unwrap();
    model.load_more("").unwrap();
    model
        .apply_page(
            page("", vec![Entry::file("b"), Entry::file("c")], None),
            generation,
        )
        .unwrap();
    assert_eq!(row_paths(&model), ["a", "b", "c"]);
    assert_eq!(model.node("").unwrap().children().len(), 3);
}

#[test]
fn a4_a_partial_listing_never_removes_children_but_the_last_page_does() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    model.request_loads();
    model
        .apply_page(
            page(
                "",
                vec![Entry::file("a"), Entry::file("b"), Entry::file("c")],
                None,
            ),
            generation,
        )
        .unwrap();
    model.invalidate_directory("");
    assert!(model.expand("").load.is_some());
    model
        .apply_page(page("", vec![Entry::file("a")], Some("x")), generation)
        .unwrap();
    assert_eq!(row_paths(&model)[..3], ["a", "b", "c"]);
    model.load_more("").unwrap();
    model
        .apply_page(page("", vec![Entry::file("b")], None), generation)
        .unwrap();
    assert_eq!(row_paths(&model), ["a", "b"]);
}

#[test]
fn a4_an_entry_whose_kind_changed_loses_its_old_subtree() {
    let mut model = fixture();
    let generation = model.generation();
    assert!(model.node("src/lib.rs").is_some());
    model
        .apply_page(
            page(
                "",
                vec![
                    Entry::file("src"),
                    Entry::file("a.txt"),
                    Entry::file("b.txt"),
                    Entry::file("c.txt"),
                ],
                None,
            ),
            generation,
        )
        .unwrap();
    assert!(model.node("src/lib.rs").is_none());
    assert_eq!(model.node("src").unwrap().entry.kind, EntryKind::File);
}

#[test]
fn a4_a_stale_generation_is_rejected_and_invalidation_discards_late_pages() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    model.request_loads();
    model.invalidate_loads();
    assert_eq!(
        model.apply_page(page("", vec![Entry::file("late")], None), generation),
        Err(PageError::StaleGeneration)
    );
    assert!(
        model.visible_rows().is_empty(),
        "loading turned back to unloaded"
    );
    assert!(model.node("").unwrap().stale);
    let loads = model.request_loads();
    assert_eq!(loads[0].generation, generation + 1);
    // reset and set_include_ignored also advance the generation.
    let before = model.generation();
    model.reset();
    assert_eq!(model.generation(), before + 1);
    assert!(model.set_include_ignored(false));
    assert_eq!(model.generation(), before + 2);
    assert!(!model.set_include_ignored(false));
}

#[test]
fn an_in_flight_load_is_never_duplicated() {
    let mut model = TreeModel::new();
    assert_eq!(model.request_loads().len(), 1);
    assert!(model.request_loads().is_empty());
    assert!(model.begin_load("", None).is_none());
    assert!(model.expand("src").load.is_none());
}

#[test]
fn a4_pages_with_non_direct_children_or_ancestor_parents_are_rejected() {
    let mut model = fixture();
    let generation = model.generation();
    // Entries that are not direct children are ignored.
    model
        .apply_page(
            page(
                "src",
                vec![
                    Entry::file("src/lib.rs"),
                    Entry::file("other/x.rs"),
                    Entry::file("src/deep/x.rs"),
                ],
                None,
            ),
            generation,
        )
        .unwrap();
    assert!(model.node("other/x.rs").is_none());
    assert!(model.node("src/deep/x.rs").is_none());
    assert_eq!(model.node("src").unwrap().children(), ["src/lib.rs"]);
    // A page carrying the directory itself or an ancestor is rejected.
    assert_eq!(
        model.apply_page(page("src", vec![Entry::directory("src")], None), generation),
        Err(PageError::AncestorEntry)
    );
    assert_eq!(
        model.apply_page(page("a/b", vec![Entry::directory("a")], None), generation),
        Err(PageError::UnknownDirectory)
    );
    // A non-directory parent is rejected.
    assert_eq!(
        model.apply_page(
            page("a.txt", vec![Entry::file("a.txt/x")], None),
            generation
        ),
        Err(PageError::NotDirectory)
    );
}

#[test]
fn a4_cycles_in_a_corrupted_model_do_not_loop() {
    let mut model = fixture();
    // Corrupt the model: directory `src` lists itself and the root as children.
    model
        .nodes
        .get_mut("src")
        .unwrap()
        .children
        .push("src".to_owned());
    model
        .nodes
        .get_mut("")
        .unwrap()
        .children
        .push("src".to_owned());
    model.rebuild();
    assert!(model.visible_rows().len() < 32);
    assert!(model.visible_rows().iter().any(|row| row.path == "src"));
}

#[test]
fn reveal_loads_ancestors_top_down_following_pagination_then_selects_the_path() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    // Nothing loaded: the first step loads the root.
    let RevealStep::Load(request) = model.reveal_search_match("src/deep/x.rs") else {
        panic!("loads the root");
    };
    assert_eq!(request.path, "");
    assert_eq!(
        model.reveal_search_match("src/deep/x.rs"),
        RevealStep::Pending
    );
    model
        .apply_page(page("", vec![Entry::file("a")], Some("p2")), generation)
        .unwrap();
    // `src` is not listed yet: follow the cursor.
    let RevealStep::Load(request) = model.reveal_search_match("src/deep/x.rs") else {
        panic!("loads the next root page");
    };
    assert_eq!(
        (request.path.as_str(), request.cursor.as_deref()),
        ("", Some("p2"))
    );
    model
        .apply_page(page("", vec![Entry::directory("src")], None), generation)
        .unwrap();
    let RevealStep::Load(request) = model.reveal_search_match("src/deep/x.rs") else {
        panic!("loads src");
    };
    assert_eq!(request.path, "src");
    model
        .apply_page(
            page("src", vec![Entry::directory("src/deep")], None),
            generation,
        )
        .unwrap();
    let RevealStep::Load(request) = model.reveal_search_match("src/deep/x.rs") else {
        panic!("loads src/deep");
    };
    assert_eq!(request.path, "src/deep");
    model.fail_load("src/deep", "denied", generation);
    assert_eq!(
        model.reveal_search_match("src/deep/x.rs"),
        RevealStep::Failed("denied".to_owned())
    );
    model.load_more("src/deep").unwrap();
    model
        .apply_page(
            page("src/deep", vec![Entry::file("src/deep/x.rs")], None),
            generation,
        )
        .unwrap();
    assert_eq!(model.reveal_search_match("src/deep/x.rs"), RevealStep::Done);
    assert!(model.is_expanded("src") && model.is_expanded("src/deep"));
    assert_eq!(selected(&model), ["src/deep/x.rs"]);
    assert_eq!(
        model.reveal_search_match("src/deep/missing.rs"),
        RevealStep::Missing
    );
}

#[test]
fn reveal_of_an_entry_hidden_by_the_ignored_filter_is_missing_and_keeps_the_selection() {
    let mut model = TreeModel::new();
    let generation = model.generation();
    model
        .apply_page(
            page(
                "",
                vec![
                    Entry::file("alpha.txt"),
                    Entry::directory("build").ignored(true),
                ],
                None,
            ),
            generation,
        )
        .unwrap();
    model
        .apply_page(
            page("build", vec![Entry::file("build/out.log")], None),
            generation,
        )
        .unwrap();
    assert!(model.select("alpha.txt"));
    assert!(model.set_include_ignored(false));
    assert_eq!(
        model.reveal_search_match("build/out.log"),
        RevealStep::Missing
    );
    assert_eq!(selected(&model), ["alpha.txt"]);
    assert!(
        !model.is_expanded("build"),
        "a failed reveal expands nothing"
    );
}

#[test]
fn action_paths_follow_the_cursor_and_selection() {
    let mut model = fixture();
    assert!(model.action_paths().is_empty());
    model.select("src");
    model.toggle_selected("src/lib.rs");
    model.toggle_selected("a.txt");
    assert_eq!(model.action_paths(), ["src", "a.txt"]);
    // A cursor that is not selected acts alone.
    model.toggle_selected("a.txt");
    assert_eq!(model.cursor(), Some("a.txt"));
    assert_eq!(model.action_paths(), ["a.txt"]);
    assert!(model.set_cursor("src/lib.rs"));
    assert_eq!(model.action_paths(), ["src"]);
}
