//! The [`SearchResults`] view: the hierarchical search-result tree of CMP-016 R21 rendered with
//! the virtual list, 27 px rows and the same row metrics as the file tree.

use super::model::EntryKind;
use super::search::{SearchActivation, SearchNode, SearchResult, SearchRow, SearchTree};
use super::style::{
    CHEVRON_SIZE, DISCLOSURE_SLOT, GAP, ICON_SIZE, NAME_SIZE, ROW_HEIGHT, ROW_PADDING_RIGHT,
    STATUS_SIZE, TreeTheme, TreeWashes, chevron, row_padding_left,
};
use super::view::default_identity;
use vespertine::{
    AnyElement, App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ListAlignment, ListState, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, list, prelude::FluentBuilder, px,
};

/// Notifications of the result view.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    /// A result row (not a collapsible directory) was activated: reveal it in the real tree
    /// with [`super::RevealIntent::OpenFile`] for a file or
    /// [`super::RevealIntent::ActivateDirectory`] for a directory.
    Reveal {
        /// The result path.
        path: String,
        /// Its kind.
        kind: EntryKind,
    },
}

/// The search-result tree view.
pub struct SearchResults {
    search: SearchTree,
    list: ListState,
    keys: Vec<String>,
    focus_handle: FocusHandle,
    theme: Option<TreeTheme>,
    washes: TreeWashes,
    message: Option<SharedString>,
}

impl EventEmitter<SearchEvent> for SearchResults {}

impl Focusable for SearchResults {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SearchResults {
    /// An empty result view.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(ROW_HEIGHT * 4.0));
        list.set_estimated_item_height(Some(px(ROW_HEIGHT)));
        Self {
            search: SearchTree::new(),
            list,
            keys: Vec::new(),
            focus_handle: cx.focus_handle(),
            theme: None,
            washes: TreeWashes::default(),
            message: None,
        }
    }

    /// The message shown instead of rows (for example "No results").
    pub fn set_message(&mut self, message: Option<SharedString>, cx: &mut Context<Self>) {
        self.message = message;
        cx.notify();
    }

    /// Replaces the theme roles (by default the active theme of the application).
    pub fn set_theme(&mut self, theme: Option<TreeTheme>, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// The result tree model.
    pub fn search(&self) -> &SearchTree {
        &self.search
    }

    /// The list state of the rows.
    pub fn list_state(&self) -> &ListState {
        &self.list
    }

    /// Runs `f` on the result tree and keeps the viewport attached to the same rows.
    pub fn update_search<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut SearchTree) -> R,
    ) -> R {
        let result = f(&mut self.search);
        let new_keys: Vec<String> = self
            .search
            .rows()
            .iter()
            .map(|row| row.path.clone())
            .collect();
        self.list.reconcile_keys(&self.keys, &new_keys);
        self.keys = new_keys;
        cx.notify();
        result
    }

    /// Accepts results of the current query (see [`SearchTree::accept_results`]).
    pub fn accept_results(
        &mut self,
        generation: u64,
        query: &str,
        results: &[SearchResult],
        cx: &mut Context<Self>,
    ) -> bool {
        self.update_search(cx, |search| {
            search.accept_results(generation, query, results)
        })
    }

    /// Activates a row: a directory with children toggles, stays active and is scrolled into
    /// view; any other row raises [`SearchEvent::Reveal`].
    pub fn activate(&mut self, path: &str, cx: &mut Context<Self>) {
        let activation = self.update_search(cx, |search| search.activate(path));
        match activation {
            SearchActivation::Toggled(path) => {
                if let Some(ix) = self.search.rows().iter().position(|row| row.path == path) {
                    self.list.scroll_to_reveal_item(ix);
                }
            }
            SearchActivation::Reveal(path, kind) => cx.emit(SearchEvent::Reveal { path, kind }),
            SearchActivation::None => {}
        }
    }

    fn render_row(&mut self, ix: usize, theme: &TreeTheme, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.search.rows().get(ix).cloned() else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let Some(node) = self.search.node(&row.path).cloned() else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let active = self.search.active() == Some(row.path.as_str());
        search_row(ix, &row, &node, active, theme, &self.washes, cx)
    }
}

fn search_row(
    ix: usize,
    row: &SearchRow,
    node: &SearchNode,
    active: bool,
    theme: &TreeTheme,
    washes: &TreeWashes,
    cx: &mut Context<SearchResults>,
) -> AnyElement {
    let path = row.path.clone();
    let slot = if row.has_children {
        div()
            .w(px(DISCLOSURE_SLOT))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(chevron(!row.collapsed, px(CHEVRON_SIZE), theme.text_muted))
    } else {
        div().w(px(DISCLOSURE_SLOT)).h_full().flex_none()
    };
    let is_dir = node.kind == EntryKind::Directory;
    let label: SharedString = node.name.clone().into();
    div()
        .id(("search-row", ix))
        .role(Role::TreeItem)
        .aria_label(label.clone())
        .aria_selected(active)
        .when(row.has_children, |row_el| {
            row_el.aria_expanded(!row.collapsed)
        })
        .flex()
        .items_center()
        .gap(px(GAP))
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(row_padding_left(row.depth)))
        .pr(px(ROW_PADDING_RIGHT))
        .when(active, |el| el.bg(theme.wash(washes.selected_focused)))
        .when(!active, |el| {
            let hover = theme.wash(washes.hover);
            el.hover(move |style| style.bg(hover))
        })
        .child(slot)
        .child(
            div()
                .size(px(ICON_SIZE))
                .flex_none()
                .child(default_identity(node.kind, !row.collapsed && is_dir, theme)),
        )
        .child(
            crate::layout::flexible_cell()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(NAME_SIZE))
                .text_color(if node.score.is_some() {
                    theme.text
                } else {
                    theme.text_muted
                })
                .child(label),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            this.activate(&path, cx);
        }))
        .into_any_element()
}

impl Render for SearchResults {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone().unwrap_or_else(|| TreeTheme::current(cx));
        let message = self.message.clone().or_else(|| {
            self.search
                .error()
                .map(|error| SharedString::from(error.to_owned()))
        });
        let list_theme = theme.clone();
        div()
            .id("search-results")
            .role(Role::Tree)
            .aria_label("Search results")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .text_color(theme.text)
            .when_some(message, |el, message| {
                el.child(
                    div()
                        .px(px(8.0))
                        .py(px(6.0))
                        .text_size(px(STATUS_SIZE))
                        .text_color(theme.text_muted)
                        .child(message),
                )
            })
            .child(
                div().relative().flex_1().min_h_0().child(
                    list(
                        self.list.clone(),
                        cx.processor(move |this, ix: usize, _window, cx| {
                            this.render_row(ix, &list_theme, cx)
                        }),
                    )
                    .size_full(),
                ),
            )
    }
}
