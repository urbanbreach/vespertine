//! The hierarchical search-result tree and the reveal of a result in the real tree
//! (CMP-016 R21, R22). Both are pure models.

use super::model::{EntryKind, LoadRequest, RevealStep, TreeModel, file_name, parent_path};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// At most this many results are kept.
pub const RESULT_LIMIT: usize = 200;
/// Height of a search row in logical pixels.
pub const SEARCH_ROW_HEIGHT: f32 = 27.0;

/// One flat search result.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    /// The entry path.
    pub path: String,
    /// The display name.
    pub name: String,
    /// The entry kind.
    pub kind: EntryKind,
    /// The match score; higher is better.
    pub score: f64,
}

impl SearchResult {
    /// A result named after the last component of `path`.
    pub fn new(path: impl Into<String>, kind: EntryKind, score: f64) -> Self {
        let path = path.into();
        let name = file_name(&path).to_owned();
        Self {
            path,
            name,
            kind,
            score,
        }
    }
}

/// A node of the result tree: a result, or a directory implied by a result's path.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchNode {
    /// The path.
    pub path: String,
    /// The display name.
    pub name: String,
    /// The kind (implied directories are directories).
    pub kind: EntryKind,
    /// The node's own score; `None` for an implied directory.
    pub score: Option<f64>,
    /// The maximum of the node's own score and all descendants' scores.
    pub best_score: f64,
    /// The children, in display order.
    pub children: Vec<String>,
}

/// One visible row of the result tree.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchRow {
    /// The node path.
    pub path: String,
    /// The nesting depth.
    pub depth: usize,
    /// Whether the node has children (and so can be collapsed).
    pub has_children: bool,
    /// Whether it is collapsed.
    pub collapsed: bool,
}

/// What activating a result row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchActivation {
    /// A directory node with children was toggled; it stays the active row (scroll it into view).
    Toggled(String),
    /// Reveal this result in the real tree (open it when `kind` is a file).
    Reveal(String, EntryKind),
    /// Unknown row.
    None,
}

/// The result tree with its query state.
#[derive(Clone, Debug, Default)]
pub struct SearchTree {
    nodes: HashMap<String, SearchNode>,
    roots: Vec<String>,
    collapsed: HashSet<String>,
    rows: Vec<SearchRow>,
    active: Option<String>,
    query: String,
    generation: u64,
    error: Option<String>,
}

fn compare(nodes: &HashMap<String, SearchNode>, a: &str, b: &str) -> Ordering {
    let (a_node, b_node) = (&nodes[a], &nodes[b]);
    b_node
        .best_score
        .partial_cmp(&a_node.best_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| {
            let rank = |kind: EntryKind| u8::from(kind != EntryKind::Directory);
            rank(a_node.kind).cmp(&rank(b_node.kind))
        })
        .then_with(|| a_node.name.to_lowercase().cmp(&b_node.name.to_lowercase()))
        .then_with(|| a_node.path.cmp(&b_node.path))
}

impl SearchTree {
    /// An empty result tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the results (at most [`RESULT_LIMIT`] kept): every proper prefix of a result's
    /// path becomes a directory node unless it is itself a result; scores propagate upward;
    /// siblings sort by best score descending, then directories before files, then
    /// case-insensitive name, then path. The collapsed state of surviving nodes is kept; new
    /// nodes start expanded.
    pub fn rebuild(&mut self, results: &[SearchResult]) {
        self.nodes.clear();
        self.roots.clear();
        for result in results.iter().take(RESULT_LIMIT) {
            self.nodes.insert(
                result.path.clone(),
                SearchNode {
                    path: result.path.clone(),
                    name: result.name.clone(),
                    kind: result.kind,
                    score: Some(result.score),
                    best_score: result.score,
                    children: Vec::new(),
                },
            );
        }
        let result_paths: Vec<String> = self.nodes.keys().cloned().collect();
        for path in &result_paths {
            let mut ancestor = parent_path(path);
            while !ancestor.is_empty() {
                self.nodes
                    .entry(ancestor.to_owned())
                    .or_insert_with(|| SearchNode {
                        path: ancestor.to_owned(),
                        name: file_name(ancestor).to_owned(),
                        kind: EntryKind::Directory,
                        score: None,
                        best_score: f64::NEG_INFINITY,
                        children: Vec::new(),
                    });
                ancestor = parent_path(ancestor);
            }
        }
        let paths: Vec<String> = self.nodes.keys().cloned().collect();
        // Best scores: the maximum over the node and every descendant.
        for path in &paths {
            let own = self.nodes[path].score;
            let Some(score) = own else { continue };
            let mut ancestor = parent_path(path);
            while !ancestor.is_empty() {
                let node = self.nodes.get_mut(ancestor).expect("ancestors exist");
                if score > node.best_score {
                    node.best_score = score;
                }
                ancestor = parent_path(ancestor);
            }
        }
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for path in &paths {
            let parent = parent_path(path);
            if parent.is_empty() {
                self.roots.push(path.clone());
            } else {
                children
                    .entry(parent.to_owned())
                    .or_default()
                    .push(path.clone());
            }
        }
        for list in children.values_mut() {
            list.sort_by(|a, b| compare(&self.nodes, a, b));
        }
        for (parent, list) in children {
            self.nodes.get_mut(&parent).expect("parent exists").children = list;
        }
        let mut roots = std::mem::take(&mut self.roots);
        roots.sort_by(|a, b| compare(&self.nodes, a, b));
        self.roots = roots;
        let nodes = &self.nodes;
        self.collapsed.retain(|path| {
            nodes
                .get(path)
                .is_some_and(|node| !node.children.is_empty())
        });
        if self
            .active
            .as_ref()
            .is_some_and(|active| !self.nodes.contains_key(active))
        {
            self.active = None;
        }
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut stack: Vec<(String, usize)> = self
            .roots
            .iter()
            .rev()
            .map(|root| (root.clone(), 0))
            .collect();
        while let Some((path, depth)) = stack.pop() {
            let node = &self.nodes[&path];
            let collapsed = self.collapsed.contains(&path);
            rows.push(SearchRow {
                path: path.clone(),
                depth,
                has_children: !node.children.is_empty(),
                collapsed,
            });
            if !collapsed {
                stack.extend(
                    node.children
                        .iter()
                        .rev()
                        .map(|child| (child.clone(), depth + 1)),
                );
            }
        }
        self.rows = rows;
    }

    /// The visible rows.
    pub fn rows(&self) -> &[SearchRow] {
        &self.rows
    }

    /// The node of `path`.
    pub fn node(&self, path: &str) -> Option<&SearchNode> {
        self.nodes.get(path)
    }

    /// Whether the tree has no results.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Collapses or expands a directory node that has children. Returns whether it changed.
    pub fn toggle(&mut self, path: &str) -> bool {
        let has_children = self
            .nodes
            .get(path)
            .is_some_and(|node| !node.children.is_empty());
        if !has_children {
            return false;
        }
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_owned());
        }
        self.rebuild_rows();
        true
    }

    /// The active row.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The row index of the active row.
    pub fn active_row(&self) -> Option<usize> {
        let active = self.active.as_deref()?;
        self.rows.iter().position(|row| row.path == active)
    }

    /// Activates a row: a node with children toggles and stays active; any other row is
    /// revealed in the real tree.
    pub fn activate(&mut self, path: &str) -> SearchActivation {
        let Some(node) = self.nodes.get(path) else {
            return SearchActivation::None;
        };
        let kind = node.kind;
        let has_children = !node.children.is_empty();
        self.active = Some(path.to_owned());
        if has_children && kind == EntryKind::Directory {
            self.toggle(path);
            SearchActivation::Toggled(path.to_owned())
        } else {
            SearchActivation::Reveal(path.to_owned(), kind)
        }
    }

    /// Starts a new query: the trimmed text becomes current and a new generation is returned.
    /// Results of earlier generations are ignored.
    pub fn begin_query(&mut self, text: &str) -> u64 {
        self.generation += 1;
        self.query = text.trim().to_owned();
        self.error = None;
        self.generation
    }

    /// Accepts results only when both their generation and trimmed query text match the
    /// current ones; returns whether they were applied.
    pub fn accept_results(
        &mut self,
        generation: u64,
        query: &str,
        results: &[SearchResult],
    ) -> bool {
        if generation != self.generation || query.trim() != self.query {
            return false;
        }
        self.rebuild(results);
        true
    }

    /// The current trimmed query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The current generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Clears the search: query, results and error go and the generation advances so late
    /// results are ignored.
    pub fn clear(&mut self) {
        self.generation += 1;
        self.query.clear();
        self.error = None;
        self.active = None;
        self.collapsed.clear();
        self.rebuild(&[]);
    }

    /// A search error (for example a failed reveal), shown instead of changing the tree.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Sets or clears the search error.
    pub fn set_error(&mut self, message: Option<String>) {
        self.error = message;
    }

    /// Applies the effect of a finished reveal.
    pub fn apply_reveal(&mut self, effect: &RevealEffect) {
        if effect.clear_search {
            self.clear();
        }
    }
}

/// Why a path is revealed in the real tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealIntent {
    /// Follow the owner's active document: changes the tree selection and scroll only; never
    /// touches the search and never opens anything.
    SynchronizeSelection,
    /// A directory result was activated: clears the search.
    ActivateDirectory,
    /// A file result was activated: clears the search and raises the open request.
    OpenFile,
}

/// What a finished reveal asks the owner to do besides selecting and scrolling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealEffect {
    /// Whether the search is cleared.
    pub clear_search: bool,
    /// The file to open.
    pub open_file: Option<String>,
}

impl RevealIntent {
    /// The effect of finishing a reveal of `path` with this intent.
    pub fn effect(self, path: &str) -> RevealEffect {
        match self {
            RevealIntent::SynchronizeSelection => RevealEffect {
                clear_search: false,
                open_file: None,
            },
            RevealIntent::ActivateDirectory => RevealEffect {
                clear_search: true,
                open_file: None,
            },
            RevealIntent::OpenFile => RevealEffect {
                clear_search: true,
                open_file: Some(path.to_owned()),
            },
        }
    }
}

/// An in-progress reveal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealRequest {
    /// The reveal's identity; a newer reveal supersedes it.
    pub id: u64,
    /// The path being revealed.
    pub path: String,
    /// The intent.
    pub intent: RevealIntent,
    /// The tree generation the reveal began in.
    pub tree_generation: u64,
    /// The owner's request context (for example the checkout) the reveal began in.
    pub context: u64,
}

/// The progress of a reveal after one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevealProgress {
    /// The reveal is no longer current (cancelled by a newer one, or the tree generation or the
    /// request context changed): drop it.
    Dropped,
    /// An ancestor page must be loaded; call [`RevealCoordinator::step`] again after applying it.
    Load(LoadRequest),
    /// Waiting for a load already in flight.
    Pending,
    /// A load failed: surface this as a search error; the tree is unchanged.
    Failed(String),
    /// The path is not in the listing.
    Missing,
    /// The path is selected and its ancestors are expanded; apply the effect.
    Done(RevealEffect),
}

/// Sequences reveals: a new reveal cancels any earlier one, and a reveal is dropped when the
/// tree generation or the request context changed.
#[derive(Clone, Debug, Default)]
pub struct RevealCoordinator {
    counter: u64,
    active: Option<u64>,
}

impl RevealCoordinator {
    /// A coordinator with no reveal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Begins revealing `path`, cancelling any earlier reveal.
    pub fn begin(
        &mut self,
        path: &str,
        intent: RevealIntent,
        model: &TreeModel,
        context: u64,
    ) -> RevealRequest {
        self.counter += 1;
        self.active = Some(self.counter);
        RevealRequest {
            id: self.counter,
            path: path.to_owned(),
            intent,
            tree_generation: model.generation(),
            context,
        }
    }

    /// Cancels the active reveal.
    pub fn cancel(&mut self) {
        self.active = None;
    }

    /// Whether `request` is still the current reveal in this tree generation and context.
    pub fn is_current(&self, request: &RevealRequest, model: &TreeModel, context: u64) -> bool {
        self.active == Some(request.id)
            && request.tree_generation == model.generation()
            && request.context == context
    }

    /// Advances `request` by one step against `model`.
    pub fn step(
        &mut self,
        request: &RevealRequest,
        model: &mut TreeModel,
        context: u64,
    ) -> RevealProgress {
        if !self.is_current(request, model, context) {
            return RevealProgress::Dropped;
        }
        match model.reveal_search_match(&request.path) {
            RevealStep::Load(load) => RevealProgress::Load(load),
            RevealStep::Pending => RevealProgress::Pending,
            RevealStep::Failed(message) => {
                self.active = None;
                RevealProgress::Failed(message)
            }
            RevealStep::Missing => {
                self.active = None;
                RevealProgress::Missing
            }
            RevealStep::Done => {
                self.active = None;
                RevealProgress::Done(request.intent.effect(&request.path))
            }
        }
    }
}
