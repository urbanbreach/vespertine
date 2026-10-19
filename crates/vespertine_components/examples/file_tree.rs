//! CMP-016 A1: the interactive virtual file tree over an in-memory file system.
//!
//! Click `a.txt`, Ctrl-click `c.txt`, Shift-click a folder; Escape collapses the selection to
//! the cursor; Shift+Down extends it. F2 on one file renames it with the basename pre-selected
//! (for `report.final.txt` the field selects `report.final`) and Enter relocates it. Dragging
//! the selected group over a collapsed folder for 650 ms expands it; the drop moves the group
//! in visible order while the viewport stays on the same path. Directories load lazily in
//! pages of 12 ("Load more…"). Typing in the search field shows the hierarchical result tree;
//! activating a result reveals it in the real tree.
//!
//! Run with: `cargo run -p vespertine_components --example file_tree`

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use vespertine::{
    App, AppContext as _, Bounds, Context, Entity, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, WeakEntity, Window, WindowBounds, WindowOptions, div, px, size,
};
use vespertine_components::browser::{AddressEvent, AddressInput, BrowserTheme};
use vespertine_components::tree::{
    Entry, EntryKind, Page, RevealIntent, SearchEvent, SearchResult, SearchResults, Tree,
    TreeEvent, TreeTheme, delete_subject, file_name, is_within, parent_path,
};
use vespertine_platform::application;

const PAGE_SIZE: usize = 12;

/// An in-memory file system: path to kind.
struct Fs {
    entries: BTreeMap<String, EntryKind>,
}

impl Fs {
    fn demo() -> Self {
        let mut entries = BTreeMap::new();
        for dir in ["docs", "src", "src/components", "assets"] {
            entries.insert(dir.to_owned(), EntryKind::Directory);
        }
        for file in [
            "a.txt",
            "b.txt",
            "c.txt",
            "report.final.txt",
            "docs/guide.md",
            "docs/notes.md",
            "src/lib.rs",
            "src/main.rs",
            "src/components/tree.rs",
            "src/components/table.rs",
            "assets/logo.svg",
        ] {
            entries.insert(file.to_owned(), EntryKind::File);
        }
        for n in 0..30 {
            entries.insert(format!("src/module_{n:02}.rs"), EntryKind::File);
        }
        Self { entries }
    }

    fn children(&self, directory: &str) -> Vec<(String, EntryKind)> {
        self.entries
            .iter()
            .filter(|(path, _)| parent_path(path) == directory)
            .map(|(path, kind)| (path.clone(), *kind))
            .collect()
    }

    /// One page of a directory listing; the cursor is the offset of the page.
    fn page(&self, directory: &str, cursor: Option<&str>) -> (Vec<Entry>, Option<String>) {
        let offset = cursor.and_then(|cursor| cursor.parse().ok()).unwrap_or(0);
        let children = self.children(directory);
        let end = (offset + PAGE_SIZE).min(children.len());
        let entries = children[offset..end]
            .iter()
            .map(|(path, kind)| Entry::new(path.clone(), *kind).revision("1"))
            .collect();
        (entries, (end < children.len()).then(|| end.to_string()))
    }

    fn relocate(&mut self, from: &str, to: &str) {
        let moved: Vec<(String, EntryKind)> = self
            .entries
            .iter()
            .filter(|(path, _)| is_within(path, from))
            .map(|(path, kind)| (path.clone(), *kind))
            .collect();
        for (path, kind) in moved {
            self.entries.remove(&path);
            self.entries
                .insert(format!("{to}{}", &path[from.len()..]), kind);
        }
    }

    fn remove(&mut self, path: &str) {
        self.entries
            .retain(|candidate, _| !is_within(candidate, path));
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let query = query.to_lowercase();
        self.entries
            .iter()
            .filter(|(path, _)| file_name(path).to_lowercase().contains(&query))
            .map(|(path, kind)| {
                SearchResult::new(path.clone(), *kind, 100.0 - file_name(path).len() as f64)
            })
            .collect()
    }
}

struct FileTreeExample {
    tree: Entity<Tree>,
    field: Entity<AddressInput>,
    results: Entity<SearchResults>,
    searching: bool,
    theme: TreeTheme,
    fs: Rc<RefCell<Fs>>,
}

impl FileTreeExample {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fs = Rc::new(RefCell::new(Fs::demo()));
        let slot: Rc<RefCell<Option<WeakEntity<Tree>>>> = Rc::new(RefCell::new(None));
        let tree_of = |slot: &Rc<RefCell<Option<WeakEntity<Tree>>>>| {
            slot.borrow().as_ref().and_then(WeakEntity::upgrade)
        };
        let tree = cx.new(|cx| {
            Tree::new(window, cx)
                .label("Project files")
                .with_root_drop_row(true)
                .on_open(|path, _, _| println!("open {path}"))
                .load_directory({
                    let (fs, slot) = (fs.clone(), slot.clone());
                    move |request, window, cx| {
                        let (entries, next_cursor) =
                            fs.borrow().page(&request.path, request.cursor.as_deref());
                        let page = Page {
                            directory: request.path.clone(),
                            entries,
                            next_cursor,
                        };
                        if let Some(tree) = tree_of(&slot) {
                            tree.update(cx, |tree, cx| {
                                tree.apply_page(page, request.generation, window, cx).ok();
                            });
                        }
                    }
                })
                .on_rename({
                    let (fs, slot) = (fs.clone(), slot.clone());
                    move |from, to, window, cx| {
                        fs.borrow_mut().relocate(from, to);
                        if let Some(tree) = tree_of(&slot) {
                            tree.update(cx, |tree, cx| {
                                tree.relocate_subtree(from, to, cx);
                                tree.finish_rename(Ok(()), window, cx);
                            });
                        }
                    }
                })
                .on_move({
                    let (fs, slot) = (fs.clone(), slot.clone());
                    move |moves, _, cx| {
                        for (from, to) in moves {
                            fs.borrow_mut().relocate(from, to);
                            if let Some(tree) = tree_of(&slot) {
                                tree.update(cx, |tree, cx| {
                                    tree.relocate_subtree(from, to, cx);
                                });
                            }
                        }
                    }
                })
                .on_delete({
                    let (fs, slot) = (fs.clone(), slot.clone());
                    move |paths, _, cx| {
                        println!("delete {}", delete_subject(paths));
                        for path in paths {
                            fs.borrow_mut().remove(path);
                            if let Some(tree) = tree_of(&slot) {
                                tree.update(cx, |tree, cx| {
                                    tree.remove(path, cx);
                                });
                            }
                        }
                    }
                })
                .context_menu_item("add", "Add to conversation", |paths, _, _| {
                    println!("add {paths:?}");
                })
        });
        *slot.borrow_mut() = Some(tree.downgrade());
        tree.update(cx, |tree, cx| tree.load(window, cx));
        window.focus(&tree.focus_handle(cx), cx);

        let theme = TreeTheme::default();
        let field_theme = BrowserTheme {
            text: theme.text,
            muted: theme.text_muted,
            faint: theme.text_faint,
            border: theme.border,
            strong_border: theme.border_strong,
            raised: theme.overlay,
            page_background: theme.input,
            danger: theme.danger,
            hover: theme.wash(0.055),
        };
        let field = cx.new(|cx| {
            let mut field = AddressInput::new("Search files", field_theme, window, cx);
            field.set_label("Search");
            field
        });
        let results = cx.new(SearchResults::new);

        cx.subscribe_in(
            &field,
            window,
            |this, field, event: &AddressEvent, window, cx| match event {
                AddressEvent::Edited => this.query_changed(window, cx),
                AddressEvent::Escape => {
                    field.update(cx, |field, cx| field.set_text("", window, cx));
                    this.query_changed(window, cx);
                }
                AddressEvent::Submit => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &results,
            window,
            |this, _, event: &SearchEvent, window, cx| {
                let SearchEvent::Reveal { path, kind } = event;
                let intent = if *kind == EntryKind::Directory {
                    RevealIntent::ActivateDirectory
                } else {
                    RevealIntent::OpenFile
                };
                this.tree
                    .update(cx, |tree, cx| tree.reveal_path(path, intent, window, cx));
            },
        )
        .detach();
        cx.subscribe_in(&tree, window, |this, _, event: &TreeEvent, window, cx| {
            if let TreeEvent::RevealFinished { effect, .. } = event {
                if effect.clear_search {
                    this.field
                        .update(cx, |field, cx| field.set_text("", window, cx));
                    this.query_changed(window, cx);
                }
            }
        })
        .detach();

        Self {
            tree,
            field,
            results,
            searching: false,
            theme,
            fs,
        }
    }

    fn query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.field.read(cx).text().to_owned();
        let query = text.trim().to_owned();
        self.searching = !query.is_empty();
        let results = self.fs.borrow().search(&query);
        self.results.update(cx, |view, cx| {
            if query.is_empty() {
                view.update_search(cx, |search| search.clear());
            } else {
                let generation = view.update_search(cx, |search| search.begin_query(&text));
                view.accept_results(generation, &text, &results, cx);
            }
        });
        if !self.searching {
            window.focus(&self.tree.focus_handle(cx), cx);
        }
        cx.notify();
    }
}

impl Render for FileTreeExample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.searching {
            self.results.clone().into_any_element()
        } else {
            self.tree.clone().into_any_element()
        };
        div()
            .id("file-tree-example")
            .size_full()
            .flex()
            .flex_col()
            .bg(self.theme.input.opacity(0.35))
            .text_size(px(12.0))
            .text_color(self.theme.text)
            .child(
                div().h(px(36.0)).px(px(8.0)).flex().items_center().child(
                    div()
                        .h(px(24.0))
                        .px(px(8.0))
                        .flex_1()
                        .flex()
                        .items_center()
                        .rounded(px(4.0))
                        .bg(self.theme.input)
                        .child(self.field.clone()),
                ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| FileTreeExample::new(window, cx)),
        )
        .expect("window");
        cx.activate(true);
    });
}
