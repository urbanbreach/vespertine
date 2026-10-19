//! CMP-016 view tests on the offscreen test host: keyboard and pointer selection, context
//! menus, empty-space clicks, fade flags, drag and drop with autoscroll and inline rename.

use std::cell::RefCell;
use std::rc::Rc;
mod controls_support;
use controls_support::{center, node};
use vespertine::EntityInputHandler as _;
use vespertine::{AppContext as _, Context, Entity, Window};
use vespertine_components::floating::layers;
use vespertine_components::tree::{
    Entry, LoadRequest, Page, RevealIntent, RowKind, Tree, TreeEvent, list_fade_flags,
};
use vespertine_test_host::{HostOptions, TestHost};

#[derive(Default)]
struct Log {
    opened: Vec<String>,
    loads: Vec<LoadRequest>,
    moves: Vec<Vec<(String, String)>>,
    renames: Vec<(String, String)>,
    deletes: Vec<Vec<String>>,
    menus: Vec<Vec<String>>,
    events: Vec<TreeEvent>,
}

struct Harness {
    host: TestHost,
    tree: Entity<Tree>,
    log: Rc<RefCell<Log>>,
}

impl Harness {
    fn new(height: f64, configure: impl FnOnce(Tree) -> Tree + 'static) -> Harness {
        let log = Rc::new(RefCell::new(Log::default()));
        let slot: Rc<RefCell<Option<Entity<Tree>>>> = Rc::new(RefCell::new(None));
        let (build_log, build_slot) = (log.clone(), slot.clone());
        let mut host =
            TestHost::with_view(HostOptions::with_size(320.0, height), move |window, cx| {
                let tree = cx.new(|cx| {
                    let tree = Tree::new(window, cx)
                        .on_open({
                            let log = build_log.clone();
                            move |path, _, _| log.borrow_mut().opened.push(path.to_owned())
                        })
                        .load_directory({
                            let log = build_log.clone();
                            move |request, _, _| log.borrow_mut().loads.push(request.clone())
                        })
                        .on_context_menu({
                            let log = build_log.clone();
                            move |paths, _, _, _| log.borrow_mut().menus.push(paths.to_vec())
                        })
                        .on_rename({
                            let log = build_log.clone();
                            move |from, to, _, _| {
                                log.borrow_mut()
                                    .renames
                                    .push((from.to_owned(), to.to_owned()))
                            }
                        })
                        .on_delete({
                            let log = build_log.clone();
                            move |paths, _, _| log.borrow_mut().deletes.push(paths.to_vec())
                        })
                        .on_move({
                            let log = build_log.clone();
                            move |moves, _, _| log.borrow_mut().moves.push(moves.to_vec())
                        });
                    configure(tree)
                });
                *build_slot.borrow_mut() = Some(tree.clone());
                tree
            })
            .expect("test host");
        let tree = slot.borrow_mut().take().expect("tree built");
        let events = log.clone();
        host.app().update(|cx| {
            cx.subscribe(&tree, move |_, event: &TreeEvent, _| {
                events.borrow_mut().events.push(event.clone())
            })
            .detach();
        });
        Harness { host, tree, log }
    }

    fn update<R>(&mut self, f: impl FnOnce(&mut Tree, &mut Window, &mut Context<Tree>) -> R) -> R {
        let tree = self.tree.clone();
        let window = self.host.window();
        let result = self
            .host
            .app()
            .update_window(window, |_, window, cx| {
                tree.update(cx, |tree, cx| f(tree, window, cx))
            })
            .expect("window");
        self.host.flush();
        result
    }

    fn read<R>(&mut self, f: impl FnOnce(&Tree) -> R) -> R {
        let tree = self.tree.clone();
        self.host.app().update(|cx| f(tree.read(cx)))
    }

    fn load(&mut self) {
        self.update(|tree, window, cx| tree.load(window, cx));
    }

    /// Answers the pending load of `directory` with `entries`.
    fn answer(&mut self, directory: &str, entries: Vec<Entry>, next: Option<&str>) {
        let request = {
            let mut log = self.log.borrow_mut();
            let ix = log
                .loads
                .iter()
                .rposition(|load| load.path == directory)
                .unwrap_or_else(|| panic!("no pending load of {directory:?}"));
            log.loads.remove(ix)
        };
        let page = Page {
            directory: directory.to_owned(),
            entries,
            next_cursor: next.map(str::to_owned),
        };
        self.update(|tree, window, cx| {
            tree.apply_page(page, request.generation, window, cx)
                .unwrap()
        });
    }

    fn selected(&mut self) -> Vec<String> {
        self.read(|tree| tree.model().selected_paths())
    }

    fn cursor(&mut self) -> Option<String> {
        self.read(|tree| tree.model().cursor().map(str::to_owned))
    }

    fn rows(&mut self) -> Vec<String> {
        self.read(|tree| {
            tree.model()
                .visible_rows()
                .iter()
                .map(|row| row.path.clone())
                .collect()
        })
    }

    fn center(&mut self, path: &str) -> (f64, f64) {
        let tree = self.tree.clone();
        let center = self.host.app().update(|cx| {
            let tree = tree.read(cx);
            let ix = tree.model().row_index(path).expect("row exists");
            tree.list_state()
                .bounds_for_item(ix)
                .expect("row measured")
                .center()
        });
        (
            f64::from(f32::from(center.x)),
            f64::from(f32::from(center.y)),
        )
    }

    fn click(&mut self, path: &str, modifiers: &str) {
        let (x, y) = self.center(path);
        self.host.click(x, y, 0, modifiers).unwrap();
    }

    fn keys(&mut self, keys: &str) {
        self.host.simulate_keystrokes(keys, None).unwrap();
    }

    /// Lets a closing menu finish its exit phase (until then its overlay swallows presses).
    fn settle(&mut self) {
        self.host.advance_executor_time(200.0).unwrap();
        self.host.flush();
    }
}

fn file(path: &str) -> Entry {
    Entry::file(path).revision("r1")
}

fn dir(path: &str) -> Entry {
    Entry::directory(path).revision("r1")
}

fn standard_root() -> Vec<Entry> {
    vec![
        dir("docs"),
        dir("src"),
        file("a.txt"),
        file("b.txt"),
        file("c.txt"),
        file("report.final.txt"),
    ]
}

fn loaded(height: f64) -> Harness {
    loaded_with(height, |tree| tree)
}

fn loaded_with(height: f64, configure: impl FnOnce(Tree) -> Tree + 'static) -> Harness {
    let mut harness = Harness::new(height, configure);
    harness.load();
    harness.answer("", standard_root(), None);
    harness
}

fn many_files(count: usize) -> Vec<Entry> {
    (0..count).map(|n| file(&format!("f{n:03}.txt"))).collect()
}

// ---- A6: empty-space clicks ----------------------------------------------------------------

#[test]
fn a6_an_unmodified_click_on_empty_space_clears_selection_and_cursor() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.click("c.txt", "ctrl");
    assert_eq!(harness.selected(), ["a.txt", "c.txt"]);
    // Below the last row, inside the viewport.
    harness.host.click(100.0, 450.0, 0, "").unwrap();
    assert!(harness.selected().is_empty());
    assert_eq!(harness.cursor(), None);
    // The next Down selects the first row.
    harness.keys("down");
    assert_eq!(harness.selected(), ["docs"]);
}

#[test]
fn a6_shift_and_ctrl_clicks_on_empty_space_preserve_the_selection() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.click("c.txt", "ctrl");
    harness.host.click(100.0, 450.0, 0, "shift").unwrap();
    assert_eq!(harness.selected(), ["a.txt", "c.txt"]);
    harness.host.click(100.0, 450.0, 0, "ctrl").unwrap();
    assert_eq!(harness.selected(), ["a.txt", "c.txt"]);
    assert_eq!(harness.cursor().as_deref(), Some("c.txt"));
}

#[test]
fn a6_a_click_on_a_row_is_not_empty_space() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("b.txt", "");
    assert_eq!(harness.selected(), ["b.txt"]);
    assert_eq!(harness.log.borrow().opened, ["b.txt"]);
    harness.click("a.txt", "");
    assert_eq!(harness.selected(), ["a.txt"]);
    // Modified clicks never open.
    harness.click("c.txt", "ctrl");
    harness.click("b.txt", "shift");
    assert_eq!(harness.log.borrow().opened, ["b.txt", "a.txt"]);
}

// ---- A1: selection, keyboard and activation ------------------------------------------------

#[test]
fn a1_click_modifier_click_shift_click_escape_and_shift_down() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.click("c.txt", "ctrl");
    assert_eq!(harness.selected(), ["a.txt", "c.txt"]);
    harness.click("docs", "shift");
    // Range from the anchor (c.txt) to the folder, replacing the selection; no activation.
    assert_eq!(
        harness.selected(),
        ["docs", "src", "a.txt", "b.txt", "c.txt"]
    );
    assert!(!harness.read(|tree| tree.model().is_expanded("docs")));
    harness.keys("escape");
    assert_eq!(harness.selected(), ["docs"]);
    harness.keys("shift-down shift-down");
    assert_eq!(harness.selected(), ["docs", "src", "a.txt"]);
    harness.keys("shift-up");
    assert_eq!(harness.selected(), ["docs", "src"]);
}

#[test]
fn keyboard_navigation_expands_collapses_and_activates() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.keys("up up");
    assert_eq!(harness.selected(), ["docs"]);
    // Right on a collapsed directory expands it and requests its page.
    harness.keys("right");
    assert!(harness.read(|tree| tree.model().is_expanded("docs")));
    assert_eq!(harness.log.borrow().loads.len(), 1);
    harness.answer("docs", vec![file("docs/guide.md")], None);
    harness.keys("right");
    assert_eq!(harness.cursor().as_deref(), Some("docs/guide.md"));
    harness.keys("left");
    assert_eq!(harness.cursor().as_deref(), Some("docs"));
    harness.keys("left");
    assert!(!harness.read(|tree| tree.model().is_expanded("docs")));
    harness.keys("down down");
    harness.keys("enter");
    assert_eq!(
        harness.log.borrow().opened.last().map(String::as_str),
        Some("a.txt")
    );
    harness.keys("space");
    assert_eq!(
        harness.log.borrow().opened.len(),
        3,
        "click, Enter and Space each open the file"
    );
}

#[test]
fn clicking_a_directory_toggles_it_and_loads_lazily_once() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("src", "");
    assert_eq!(harness.log.borrow().loads.len(), 1);
    assert_eq!(
        harness.rows().last().map(String::as_str),
        Some("report.final.txt")
    );
    let rows = harness.rows();
    assert!(rows.contains(&"src\0loading".to_owned()));
    harness.answer("src", vec![file("src/lib.rs")], None);
    harness.click("src", "");
    assert!(!harness.read(|tree| tree.model().is_expanded("src")));
    harness.click("src", "");
    assert!(
        harness.log.borrow().loads.is_empty(),
        "a loaded directory is not reloaded"
    );
    assert!(harness.rows().contains(&"src/lib.rs".to_owned()));
}

#[test]
fn a4_a_failed_load_row_says_retry_and_a_click_retries_with_the_same_cursor() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(600.0, |tree| tree);
    harness.load();
    let request = harness.log.borrow_mut().loads.remove(0);
    harness.update(|tree, window, cx| {
        tree.fail_load("", "disk on fire", request.generation, window, cx);
    });
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "disk on fire — Retry")
    );
    let error_row = harness.rows().remove(0);
    harness.click(&error_row, "");
    let retry = harness.log.borrow_mut().loads.remove(0);
    assert_eq!(retry.cursor, None);
    harness.answer_with(retry, vec![file("a")], Some("c1"));
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "Load more…")
    );
    let more = harness.rows().remove(1);
    harness.click(&more, "");
    assert_eq!(harness.log.borrow().loads[0].cursor.as_deref(), Some("c1"));
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "Loading…")
    );
}

impl Harness {
    fn answer_with(&mut self, request: LoadRequest, entries: Vec<Entry>, next: Option<&str>) {
        let page = Page {
            directory: request.path.clone(),
            entries,
            next_cursor: next.map(str::to_owned),
        };
        self.update(|tree, window, cx| {
            tree.apply_page(page, request.generation, window, cx)
                .unwrap()
        });
    }
}

// ---- A5: context menus ---------------------------------------------------------------------

#[test]
fn a5_menu_key_and_shift_f10_open_the_menu_for_the_selection_roots() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.click("c.txt", "ctrl");
    harness.keys("menu");
    assert_eq!(
        harness.log.borrow().menus,
        [vec!["a.txt".to_owned(), "c.txt".to_owned()]]
    );
    let open = harness.read(|tree| tree.context_menu().map(|menu| menu.paths.clone()));
    assert_eq!(open, Some(vec!["a.txt".to_owned(), "c.txt".to_owned()]));
    harness.keys("escape");
    assert!(harness.read(|tree| tree.context_menu().is_none()));
    harness.keys("shift-f10");
    assert_eq!(harness.log.borrow().menus.len(), 2);
    harness.keys("escape");
    // A cursor that is not selected acts alone.
    harness.click("c.txt", "ctrl");
    assert_eq!(harness.cursor().as_deref(), Some("c.txt"));
    harness.keys("menu");
    assert_eq!(
        harness.log.borrow().menus.last().unwrap(),
        &["c.txt".to_owned()]
    );
}

#[test]
fn a5_a_secondary_click_inside_the_selection_keeps_it_and_outside_selects_the_entry() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.click("c.txt", "ctrl");
    let (x, y) = harness.center("a.txt");
    harness.host.click(x, y, 2, "").unwrap();
    assert_eq!(harness.selected(), ["a.txt", "c.txt"]);
    assert_eq!(
        harness.log.borrow().menus.last().unwrap(),
        &["a.txt".to_owned(), "c.txt".to_owned()]
    );
    harness.keys("escape");
    harness.settle();
    let (x, y) = harness.center("b.txt");
    harness.host.click(x, y, 2, "").unwrap();
    assert_eq!(harness.selected(), ["b.txt"]);
    assert_eq!(
        harness.log.borrow().menus.last().unwrap(),
        &["b.txt".to_owned()]
    );
}

#[test]
fn a5_menu_keys_move_the_active_item_skipping_disabled_ones_and_do_not_reach_the_tree() {
    vespertine_test_host::require_backend!();
    // `a.txt` is mutable, `b.txt` has no revision token and so is not.
    let mut harness = Harness::new(600.0, |tree| tree);
    harness.load();
    harness.answer("", vec![file("a.txt"), Entry::file("b.txt")], None);
    harness.click("b.txt", "");
    harness.keys("menu");
    let items = harness.read(|tree| tree.context_menu().unwrap().items.clone());
    let enabled: Vec<(&str, bool)> = items
        .iter()
        .map(|item| (item.id.as_str(), item.enabled))
        .collect();
    assert_eq!(
        enabled,
        [("copy-path", true), ("rename", false), ("delete", false)]
    );
    harness.keys("down");
    assert_eq!(
        harness.read(|tree| tree.context_menu().unwrap().active()),
        Some(0)
    );
    harness.keys("down");
    assert_eq!(
        harness.read(|tree| tree.context_menu().unwrap().active()),
        Some(0),
        "only one item is enabled"
    );
    // Down did not move the tree cursor.
    assert_eq!(harness.cursor().as_deref(), Some("b.txt"));
    harness.keys("escape");
    harness.settle();
    harness.click("a.txt", "");
    harness.keys("menu");
    let items = harness.read(|tree| tree.context_menu().unwrap().items.clone());
    assert!(items.iter().all(|item| item.enabled));
    harness.keys("down down down");
    assert_eq!(
        harness.read(|tree| tree.context_menu().unwrap().active()),
        Some(2)
    );
    harness.keys("down");
    assert_eq!(
        harness.read(|tree| tree.context_menu().unwrap().active()),
        Some(0),
        "wraps"
    );
    harness.keys("down down enter");
    assert_eq!(harness.log.borrow().deletes, [vec!["a.txt".to_owned()]]);
    assert!(harness.read(|tree| tree.context_menu().is_none()));
}

#[test]
fn a5_the_menu_closes_on_scroll_and_when_interactions_are_suspended() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(200.0, |tree| tree);
    harness.load();
    harness.answer("", many_files(40), None);
    harness.click("f001.txt", "");
    harness.keys("menu");
    assert!(harness.read(|tree| tree.context_menu().is_some()));
    harness
        .host
        .scroll_wheel(250.0, 150.0, 0.0, -30.0, "")
        .unwrap();
    assert!(harness.read(|tree| tree.context_menu().is_none()));
    harness.keys("menu");
    assert!(harness.read(|tree| tree.context_menu().is_some()));
    harness.update(|tree, window, cx| tree.suspend_interactions(window, cx));
    assert!(harness.read(|tree| tree.context_menu().is_none()));
}

#[test]
fn delete_requires_every_path_to_be_mutable_and_goes_to_the_owner() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(600.0, |tree| tree.can_mutate(|path, _| path != "c.txt"));
    harness.load();
    harness.answer("", vec![file("a.txt"), file("b.txt"), file("c.txt")], None);
    harness.click("a.txt", "");
    harness.click("b.txt", "ctrl");
    harness.keys("delete");
    assert_eq!(
        harness.log.borrow().deletes,
        [vec!["a.txt".to_owned(), "b.txt".to_owned()]]
    );
    harness.click("c.txt", "ctrl");
    harness.keys("delete");
    assert_eq!(
        harness.log.borrow().deletes.len(),
        1,
        "one immutable path blocks the delete"
    );
}

// ---- A7: fade flags at layout --------------------------------------------------------------

#[test]
fn a7_fade_flags_follow_the_scroll_offset_and_clear_when_the_scroll_is_clamped() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(270.0, |tree| tree);
    harness.load();
    harness.answer("", many_files(60), None);
    let flags = |harness: &mut Harness| harness.read(|tree| list_fade_flags(tree.list_state()));
    assert_eq!(flags(&mut harness), (false, true), "at the top");
    harness
        .host
        .scroll_wheel(100.0, 100.0, 0.0, -400.0, "")
        .unwrap();
    assert_eq!(flags(&mut harness), (true, true), "in the middle");
    harness
        .host
        .scroll_wheel(100.0, 100.0, 0.0, -100000.0, "")
        .unwrap();
    assert_eq!(flags(&mut harness), (true, false), "at the end");
    // Removing rows clamps the scroll to the top in the same frame: both flags clear.
    harness.update(|tree, _, cx| {
        for n in 5..60 {
            tree.remove(&format!("f{n:03}.txt"), cx);
        }
    });
    assert_eq!(flags(&mut harness), (false, false));
}

// ---- A8: drag and drop ---------------------------------------------------------------------

fn drag_to(harness: &mut Harness, from: (f64, f64), to: (f64, f64)) {
    harness.host.mouse_down(from.0, from.1, 0, "").unwrap();
    harness
        .host
        .mouse_move(from.0 + 6.0, from.1 + 6.0, Some(0), "")
        .unwrap();
    harness.host.mouse_move(to.0, to.1, Some(0), "").unwrap();
}

fn advance(harness: &mut Harness, ms: f64) {
    let mut left = ms;
    while left > 0.0 {
        let step = left.min(16.0);
        harness.host.advance_executor_time(step).unwrap();
        left -= step;
    }
}

#[test]
fn a8_dragging_the_selected_group_over_a_collapsed_folder_expands_it_after_650_ms_and_drops_in_visible_order()
 {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.click("c.txt", "ctrl");
    let from = harness.center("c.txt");
    let to = harness.center("docs");
    drag_to(&mut harness, from, to);
    let destination = harness.read(|tree| {
        tree.drag_session()
            .map(|session| (session.destination().map(str::to_owned), session.is_valid()))
    });
    assert_eq!(destination, Some((Some("docs".to_owned()), true)));
    advance(&mut harness, 600.0);
    assert!(
        !harness.read(|tree| tree.model().is_expanded("docs")),
        "not yet"
    );
    advance(&mut harness, 80.0);
    assert!(
        harness.read(|tree| tree.model().is_expanded("docs")),
        "expanded after 650 ms"
    );
    assert_eq!(harness.log.borrow().loads.len(), 1, "and loaded");
    harness.host.mouse_up(to.0, to.1, 0, "").unwrap();
    assert_eq!(
        harness.log.borrow().moves,
        [vec![
            ("a.txt".to_owned(), "docs/a.txt".to_owned()),
            ("c.txt".to_owned(), "docs/c.txt".to_owned()),
        ]]
    );
    assert!(
        harness.read(|tree| tree.drag_session().is_none()),
        "state cleared by the drop"
    );
}

#[test]
fn a8_dropping_onto_a_file_row_targets_its_parent_and_a_symlink_is_no_destination() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(600.0, |tree| tree);
    harness.load();
    harness.answer(
        "",
        vec![
            dir("src"),
            file("a.txt"),
            file("b.txt"),
            Entry::symlink("link").revision("r"),
        ],
        None,
    );
    harness.click("src", "");
    harness.answer("src", vec![file("src/lib.rs")], None);
    let from = harness.center("a.txt");
    let to = harness.center("src/lib.rs");
    drag_to(&mut harness, from, to);
    assert_eq!(
        harness.read(|tree| tree
            .drag_session()
            .and_then(|s| s.destination().map(str::to_owned))),
        Some("src".to_owned())
    );
    harness.host.mouse_up(to.0, to.1, 0, "").unwrap();
    assert_eq!(
        harness.log.borrow().moves,
        [vec![("a.txt".to_owned(), "src/a.txt".to_owned())]]
    );
    // Over a symlink: no destination, the cursor would be not-allowed, nothing is issued.
    let from = harness.center("b.txt");
    let to = harness.center("link");
    drag_to(&mut harness, from, to);
    assert_eq!(
        harness.read(|tree| tree
            .drag_session()
            .map(|s| (s.destination().is_none(), s.is_valid()))),
        Some((true, false))
    );
    harness.host.mouse_up(to.0, to.1, 0, "").unwrap();
    assert_eq!(harness.log.borrow().moves.len(), 1);
}

#[test]
fn a8_the_empty_area_below_the_rows_targets_the_workspace_root() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(600.0, |tree| tree);
    harness.load();
    harness.answer("", vec![dir("src"), file("a.txt")], None);
    harness.click("src", "");
    harness.answer("src", vec![file("src/lib.rs")], None);
    let from = harness.center("src/lib.rs");
    drag_to(&mut harness, from, (100.0, 400.0));
    assert_eq!(
        harness.read(|tree| tree
            .drag_session()
            .and_then(|s| s.destination().map(str::to_owned))),
        Some(String::new())
    );
    harness.host.mouse_up(100.0, 400.0, 0, "").unwrap();
    assert_eq!(
        harness.log.borrow().moves,
        [vec![("src/lib.rs".to_owned(), "lib.rs".to_owned())]]
    );
}

#[test]
fn a8_the_rail_strip_and_the_outside_have_no_destination() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    let from = harness.center("a.txt");
    drag_to(&mut harness, from, (315.0, 400.0));
    assert_eq!(
        harness.read(|tree| tree.drag_session().map(|s| s.destination().is_none())),
        Some(true),
        "the rightmost 12 px are reserved for the rail"
    );
    harness.host.mouse_move(100.0, 700.0, Some(0), "").unwrap();
    assert!(
        harness.read(|tree| tree.drag_session().is_none()),
        "leaving the tree clears the drag"
    );
    harness.host.mouse_up(100.0, 700.0, 0, "").unwrap();
    assert!(harness.log.borrow().moves.is_empty());
}

#[test]
fn a8_escape_cancels_the_drag_first() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    let from = harness.center("a.txt");
    let to = harness.center("src");
    drag_to(&mut harness, from, to);
    assert!(harness.read(|tree| tree.drag_session().is_some()));
    harness.keys("escape");
    assert!(harness.read(|tree| tree.drag_session().is_none()));
    // The selection was not collapsed by that Escape and nothing moves on release.
    harness.host.mouse_up(to.0, to.1, 0, "").unwrap();
    assert!(harness.log.borrow().moves.is_empty());
    assert_eq!(harness.selected(), ["a.txt"]);
}

#[test]
fn a8_autoscroll_moves_the_list_while_the_pointer_rests_in_the_edge_band() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(270.0, |tree| tree);
    harness.load();
    harness.answer("", many_files(60), None);
    let from = harness.center("f002.txt");
    drag_to(&mut harness, from, (100.0, 268.0));
    let before = harness.read(|tree| tree.list_state().logical_scroll_top());
    advance(&mut harness, 320.0);
    let after = harness.read(|tree| tree.list_state().logical_scroll_top());
    let moved = (after.item_ix as f32 * 27.0 + f32::from(after.offset_in_item))
        - (before.item_ix as f32 * 27.0 + f32::from(before.offset_in_item));
    assert!(moved > 100.0, "scrolled down by {moved}px");
    harness.host.mouse_up(100.0, 268.0, 0, "").unwrap();
}

// ---- A10: rename ---------------------------------------------------------------------------

fn rename_ready() -> Harness {
    let mut harness = loaded(600.0);
    harness.click("report.final.txt", "");
    harness
}

#[test]
fn a10_f2_selects_the_basename_and_enter_commits_once() {
    vespertine_test_host::require_backend!();
    let mut harness = rename_ready();
    harness.keys("f2");
    assert_eq!(
        harness
            .read(|tree| tree.renaming_path().map(str::to_owned))
            .as_deref(),
        Some("report.final.txt")
    );
    let field = harness.read(|tree| tree.rename_field().cloned().unwrap());
    let (text, selection) = harness.host.app().update(|cx| {
        let field = field.read(cx);
        (field.value_text().to_owned(), field.selected_range())
    });
    assert_eq!(text, "report.final.txt");
    assert_eq!(&text[selection], "report.final");
    // Edit and commit; a second Enter while in flight is ignored.
    let window = harness.host.window();
    harness
        .host
        .app()
        .update_window(window, |_, window, cx| {
            field.update(cx, |field, cx| field.set_value("report.md", window, cx))
        })
        .unwrap();
    harness.keys("enter");
    harness.keys("enter");
    assert_eq!(
        harness.log.borrow().renames,
        [("report.final.txt".to_owned(), "report.md".to_owned())]
    );
    // The owner relocates the subtree and reports success: the rename closes and the tree
    // takes focus.
    harness.update(|tree, window, cx| {
        assert!(tree.relocate_subtree("report.final.txt", "report.md", cx));
        tree.finish_rename(Ok(()), window, cx);
    });
    assert!(!harness.read(|tree| tree.is_renaming()));
    assert_eq!(harness.selected(), ["report.md"]);
    harness.keys("up");
    assert_eq!(
        harness.cursor().as_deref(),
        Some("c.txt"),
        "the tree has focus again"
    );
}

#[test]
fn a10_an_invalid_name_is_rejected_with_the_message_and_the_field_stays() {
    vespertine_test_host::require_backend!();
    let mut harness = rename_ready();
    harness.keys("f2");
    let field = harness.read(|tree| tree.rename_field().cloned().unwrap());
    let window = harness.host.window();
    harness
        .host
        .app()
        .update_window(window, |_, window, cx| {
            field.update(cx, |field, cx| field.set_value("a/b", window, cx))
        })
        .unwrap();
    harness.keys("enter");
    assert!(harness.log.borrow().renames.is_empty());
    assert!(harness.read(|tree| tree.is_renaming()));
    assert_eq!(
        harness
            .read(|tree| tree.rename_error().map(str::to_owned))
            .as_deref(),
        Some("Enter a single file name without path separators")
    );
    assert!(harness.log.borrow().events.iter().any(|event| matches!(
        event,
        TreeEvent::RenameRejected(message) if message == "Enter a single file name without path separators"
    )));
}

#[test]
fn a10_escape_cancels_and_returns_focus_to_the_tree() {
    vespertine_test_host::require_backend!();
    let mut harness = rename_ready();
    harness.keys("f2");
    assert!(harness.read(|tree| tree.is_renaming()));
    harness.keys("escape");
    assert!(!harness.read(|tree| tree.is_renaming()));
    assert!(harness.log.borrow().renames.is_empty());
    harness.keys("up");
    assert_eq!(harness.cursor().as_deref(), Some("c.txt"));
}

#[test]
fn a10_losing_focus_cancels_unless_a_commit_is_in_flight() {
    vespertine_test_host::require_backend!();
    let mut harness = rename_ready();
    harness.keys("f2");
    harness.host.blur().unwrap();
    assert!(!harness.read(|tree| tree.is_renaming()), "blur cancels");

    let mut harness = rename_ready();
    harness.keys("f2");
    let field = harness.read(|tree| tree.rename_field().cloned().unwrap());
    let window = harness.host.window();
    harness
        .host
        .app()
        .update_window(window, |_, window, cx| {
            field.update(cx, |field, cx| field.set_value("x.txt", window, cx))
        })
        .unwrap();
    harness.keys("enter");
    harness.host.blur().unwrap();
    assert!(
        harness.read(|tree| tree.is_renaming()),
        "a commit in flight survives blur"
    );
}

#[test]
fn a10_a_changed_revision_blocks_the_commit_in_the_open_field() {
    vespertine_test_host::require_backend!();
    let mut harness = rename_ready();
    harness.keys("f2");
    let field = harness.read(|tree| tree.rename_field().cloned().unwrap());
    let window = harness.host.window();
    harness
        .host
        .app()
        .update_window(window, |_, window, cx| {
            field.update(cx, |field, cx| field.set_value("renamed.txt", window, cx))
        })
        .unwrap();
    // The entry changed under the field: a refreshed listing brings a new revision token.
    let mut entries = standard_root();
    entries[5] = Entry::file("report.final.txt").revision("r2");
    harness.update(|tree, window, cx| {
        let generation = tree.model().generation();
        let page = Page {
            directory: String::new(),
            entries,
            next_cursor: None,
        };
        tree.apply_page(page, generation, window, cx).unwrap();
    });
    harness.keys("enter");
    assert!(harness.log.borrow().renames.is_empty());
    assert_eq!(
        harness
            .read(|tree| tree.rename_error().map(str::to_owned))
            .as_deref(),
        Some("Entry changed; cancel and reopen Rename")
    );
    assert!(harness.read(|tree| tree.is_renaming()));
}

#[test]
fn rename_is_refused_for_an_entry_without_a_revision_token_or_a_symlink() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(600.0, |tree| tree);
    harness.load();
    harness.answer(
        "",
        vec![
            Entry::file("a.txt"),
            Entry::symlink("link").revision("r"),
            file("b.txt"),
        ],
        None,
    );
    harness.click("a.txt", "");
    harness.keys("f2");
    assert!(!harness.read(|tree| tree.is_renaming()));
    harness.click("link", "");
    harness.keys("f2");
    assert!(!harness.read(|tree| tree.is_renaming()));
    harness.click("b.txt", "");
    harness.keys("f2");
    assert!(harness.read(|tree| tree.is_renaming()));
}

// ---- reveal ---------------------------------------------------------------------------------

#[test]
fn reveal_loads_ancestors_selects_the_path_and_reports_the_effect() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.update(|tree, window, cx| {
        tree.reveal_path("src/deep/x.rs", RevealIntent::OpenFile, window, cx)
    });
    harness.answer("src", vec![dir("src/deep")], None);
    harness.answer("src/deep", vec![file("src/deep/x.rs")], None);
    assert_eq!(harness.selected(), ["src/deep/x.rs"]);
    assert_eq!(harness.log.borrow().opened, ["src/deep/x.rs"]);
    assert!(harness.log.borrow().events.iter().any(|event| matches!(
        event,
        TreeEvent::RevealFinished { path, effect } if path == "src/deep/x.rs" && effect.clear_search
    )));
    // SynchronizeSelection opens nothing.
    harness.update(|tree, window, cx| {
        tree.reveal_path("a.txt", RevealIntent::SynchronizeSelection, window, cx)
    });
    assert_eq!(harness.selected(), ["a.txt"]);
    assert_eq!(harness.log.borrow().opened.len(), 1);
}

#[test]
fn row_kinds_and_status_text_are_painted() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(600.0, |tree| tree);
    harness.load();
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "Loading…")
    );
    harness.answer("", vec![], None);
    assert!(
        harness
            .host
            .painted_text()
            .iter()
            .any(|text| text == "Empty folder")
    );
    let rows = harness.read(|tree| tree.model().visible_rows().to_vec());
    assert_eq!(rows[0].kind, RowKind::Empty);
}

// ---- seams: CMP-002 menu layer, CMP-031 rail with INP-001 capture, CMP-007 field ------------

fn layer_count(harness: &mut Harness) -> usize {
    let window = harness.host.window();
    harness
        .host
        .app()
        .update_window(window, |_, window, cx| layers(window, cx).len())
        .unwrap()
}

#[test]
fn a5_the_context_menu_is_a_dismissal_layer_that_owns_escape_and_returns_focus() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.click("c.txt", "ctrl");
    harness.keys("menu");
    assert_eq!(layer_count(&mut harness), 1);
    harness.keys("escape");
    assert_eq!(layer_count(&mut harness), 0);
    assert!(harness.read(|tree| tree.context_menu().is_none()));
    assert_eq!(
        harness.selected(),
        ["a.txt", "c.txt"],
        "the menu consumed the Escape; the selection did not collapse"
    );
    harness.settle();
    harness.keys("down");
    assert_eq!(
        harness.cursor().as_deref(),
        Some("report.final.txt"),
        "focus returned to the tree"
    );
}

#[test]
fn a5_an_outside_press_dismisses_the_menu_without_clicking_through() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    let (_, y) = harness.center("a.txt");
    harness.host.click(20.0, y, 2, "").unwrap();
    assert!(harness.read(|tree| tree.context_menu().is_some()));
    let (_, other) = harness.center("c.txt");
    harness.host.click(260.0, other, 0, "").unwrap();
    assert!(harness.read(|tree| tree.context_menu().is_none()));
    assert_eq!(
        harness.selected(),
        ["a.txt"],
        "the dismissing press did not select the row under it"
    );
    assert_eq!(layer_count(&mut harness), 0);
}

#[test]
fn a5_hovering_highlights_a_menu_row_and_clicking_it_runs_the_item() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    let (_, y) = harness.center("a.txt");
    harness.host.click(20.0, y, 2, "").unwrap();
    // Finish the entrance so the rows sit at their resting positions.
    harness.host.motion_fast_forward(400.0).unwrap();
    harness.host.paint_frame();
    let rename = node(&mut harness.host, "menuItem", Some("Rename"));
    let (x, ry) = center(&mut harness.host, &rename);
    harness.host.mouse_move(x, ry, None, "").unwrap();
    assert_eq!(
        harness.read(|tree| tree.context_menu().unwrap().active()),
        Some(1)
    );
    let delete = node(&mut harness.host, "menuItem", Some("Delete…"));
    let (x, dy) = center(&mut harness.host, &delete);
    harness.host.click(x, dy, 0, "").unwrap();
    assert_eq!(harness.log.borrow().deletes, [vec!["a.txt".to_owned()]]);
    assert!(harness.read(|tree| tree.context_menu().is_none()));
    assert_eq!(layer_count(&mut harness), 0);
}

#[test]
fn a5_home_end_and_ctrl_n_step_the_menu_like_every_cmp002_list() {
    vespertine_test_host::require_backend!();
    let mut harness = loaded(600.0);
    harness.click("a.txt", "");
    harness.keys("menu");
    let active = |harness: &mut Harness| harness.read(|tree| tree.context_menu().unwrap().active());
    harness.keys("end");
    assert_eq!(active(&mut harness), Some(2));
    harness.keys("home");
    assert_eq!(active(&mut harness), Some(0));
    harness.keys("ctrl-n");
    assert_eq!(active(&mut harness), Some(1));
    harness.keys("ctrl-p");
    assert_eq!(active(&mut harness), Some(0));
}

#[test]
fn r9_the_overlay_rail_drives_the_list_and_its_drag_is_captured_beyond_the_tree() {
    vespertine_test_host::require_backend!();
    let mut harness = Harness::new(270.0, |tree| tree);
    harness.load();
    harness.answer("", many_files(60), None);
    harness.host.motion_set(1000.0).unwrap();
    harness.host.paint_frame();
    harness
        .host
        .scroll_wheel(100.0, 100.0, 0.0, -300.0, "")
        .unwrap();
    harness.host.paint_frame();
    let extents = |harness: &mut Harness| {
        harness.read(|tree| {
            let (max, position) = tree.list_state().scrollbar_extents();
            (f32::from(max), f32::from(position))
        })
    };
    let (max, before) = extents(&mut harness);
    assert!(max > 1000.0 && before > 0.0, "{max} {before}");
    // Pressing the track scrolls at press time; the strip took the press, not a row.
    harness.host.mouse_down(315.0, 135.0, 0, "").unwrap();
    let (_, pressed) = extents(&mut harness);
    assert!(
        (pressed - before).abs() > 10.0,
        "the press scrolled: {before} -> {pressed}"
    );
    assert!(harness.selected().is_empty());
    // The drag follows the strip far outside the tree (pointer capture).
    harness.host.paint_frame();
    harness.host.mouse_move(100.0, 900.0, Some(0), "").unwrap();
    let (_, bottom) = extents(&mut harness);
    assert!((bottom - max).abs() < 1.0, "clamped at the end: {bottom}");
    harness.host.paint_frame();
    harness.host.mouse_move(100.0, -50.0, Some(0), "").unwrap();
    let (_, top) = extents(&mut harness);
    assert!(top.abs() < 1.0, "clamped at the start: {top}");
    harness.host.mouse_up(100.0, -50.0, 0, "").unwrap();
    harness.host.mouse_move(100.0, 200.0, Some(0), "").unwrap();
    let (_, after) = extents(&mut harness);
    assert!(after.abs() < 1.0, "no drag after the release: {after}");
    assert!(harness.selected().is_empty());
    assert!(harness.read(|tree| tree.drag_session().is_none()));
}

#[test]
fn a10_the_rename_field_is_a_labelled_text_field_and_typing_replaces_the_basename() {
    vespertine_test_host::require_backend!();
    let mut harness = rename_ready();
    harness.keys("f2");
    node(&mut harness.host, "textInput", Some("Rename"));
    let field = harness.read(|tree| tree.rename_field().cloned().unwrap());
    let window = harness.host.window();
    harness
        .host
        .app()
        .update_window(window, |_, window, cx| {
            field.update(cx, |field, cx| {
                field.replace_text_in_range(None, "summary", window, cx)
            })
        })
        .unwrap();
    let text = harness
        .host
        .app()
        .update(|cx| field.read(cx).value_text().to_owned());
    assert_eq!(text, "summary.txt");
    harness.keys("enter");
    assert_eq!(
        harness.log.borrow().renames,
        [("report.final.txt".to_owned(), "summary.txt".to_owned())]
    );
}
