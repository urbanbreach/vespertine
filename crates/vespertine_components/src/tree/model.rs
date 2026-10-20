//! The pure model of the file tree (CMP-016 R1-R6, R11-R14, R22).
//!
//! [`TreeModel`] owns the entries, their lazy load states, the expansion set, the selection set,
//! the keyboard cursor and the Shift-range anchor. It contains no file-system access and no
//! framework types: the owner supplies pages of entries ([`Page`]) and mutates the model through
//! the operations below, and the view only renders [`TreeModel::visible_rows`].
//!
//! Paths are `/`-separated strings (the root is the empty path and is never a row). Status rows
//! (loading, empty, error, load-more) have synthetic unique paths of the form
//! `<directory>\0<kind>`; NUL is never valid in an entry name, so they cannot collide with
//! entries.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

/// The path of the root directory.
pub const ROOT: &str = "";

/// The parent of `path`: the text before its last `/`, the empty path for a top-level entry.
pub fn parent_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[..index],
        None => ROOT,
    }
}

/// The last component of `path`.
pub fn file_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[index + 1..],
        None => path,
    }
}

/// `directory/name`, or `name` for the root directory.
pub fn join_path(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

/// Whether `path` is `ancestor` itself or lies inside it. Every path lies inside the root.
pub fn is_within(path: &str, ancestor: &str) -> bool {
    ancestor.is_empty()
        || path == ancestor
        || (path.len() > ancestor.len()
            && path.starts_with(ancestor)
            && path.as_bytes()[ancestor.len()] == b'/')
}

/// The parent of a row path: the directory of a status row, the parent of an entry.
pub fn row_parent(path: &str) -> &str {
    match path.find('\0') {
        Some(index) => &path[..index],
        None => parent_path(path),
    }
}

/// The kind of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A directory.
    Directory,
    /// A regular file.
    File,
    /// A symbolic link (never a drop destination, never mutable).
    Symlink,
}

impl EntryKind {
    fn rank(self) -> u8 {
        match self {
            EntryKind::Directory => 0,
            EntryKind::File => 1,
            EntryKind::Symlink => 2,
        }
    }
}

/// One entry of a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The stable path.
    pub path: String,
    /// The kind.
    pub kind: EntryKind,
    /// The display name.
    pub name: String,
    /// Whether the entry is ignored; set on a directory it applies to every descendant.
    pub ignored: bool,
    /// The owner's revision token, used to guard mutations. Entries without one are not mutable.
    pub revision: Option<String>,
    /// The size in bytes, when the owner knows it.
    pub size: Option<u64>,
    /// The modification time, when the owner knows it.
    pub modified: Option<SystemTime>,
}

impl Entry {
    /// An entry named after the last component of `path`.
    pub fn new(path: impl Into<String>, kind: EntryKind) -> Self {
        let path = path.into();
        let name = file_name(&path).to_owned();
        Self {
            path,
            kind,
            name,
            ignored: false,
            revision: None,
            size: None,
            modified: None,
        }
    }

    /// A directory entry.
    pub fn directory(path: impl Into<String>) -> Self {
        Self::new(path, EntryKind::Directory)
    }

    /// A file entry.
    pub fn file(path: impl Into<String>) -> Self {
        Self::new(path, EntryKind::File)
    }

    /// A symlink entry.
    pub fn symlink(path: impl Into<String>) -> Self {
        Self::new(path, EntryKind::Symlink)
    }

    /// Sets the ignored flag.
    pub fn ignored(mut self, ignored: bool) -> Self {
        self.ignored = ignored;
        self
    }

    /// Sets the owner revision token.
    pub fn revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
    }

    /// Sets the display name.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Whether this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory
    }
}

/// The load state of a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// Never requested.
    Unloaded,
    /// A page is in flight; the cursor is the one the page was requested with.
    Loading(Option<String>),
    /// The last requested page arrived; the cursor of the next page, if there is one.
    Loaded(Option<String>),
    /// The last request failed; the cursor to retry with.
    Error(String, Option<String>),
}

/// A directory (or file) node of the model.
#[derive(Clone, Debug)]
pub struct Node {
    /// The entry.
    pub entry: Entry,
    /// The load state (meaningful for directories).
    pub load: LoadState,
    /// Whether the loaded listing is known to be out of date.
    pub stale: bool,
    /// Whether a listing was ever applied.
    pub has_loaded: bool,
    children: Vec<String>,
    listing: HashSet<String>,
}

impl Node {
    fn new(entry: Entry) -> Self {
        Self {
            entry,
            load: LoadState::Unloaded,
            stale: false,
            has_loaded: false,
            children: Vec::new(),
            listing: HashSet::new(),
        }
    }

    /// The paths of the children, in display order.
    pub fn children(&self) -> &[String] {
        &self.children
    }
}

/// A request for a page of a directory, issued by the model; the owner answers it with
/// [`TreeModel::apply_page`] (or [`TreeModel::fail_load`]) using the same `generation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadRequest {
    /// The directory to list.
    pub path: String,
    /// The cursor of the page, `None` for the first page.
    pub cursor: Option<String>,
    /// The generation token the answer must carry.
    pub generation: u64,
}

/// A page of a directory listing supplied by the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The listed directory.
    pub directory: String,
    /// Its entries (only direct children are applied).
    pub entries: Vec<Entry>,
    /// The cursor of the following page; `None` marks the last page.
    pub next_cursor: Option<String>,
}

/// Why a page was not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageError {
    /// The page's generation token is not the tree's (a reset or invalidation discarded it).
    StaleGeneration,
    /// The page's directory is not in the model.
    UnknownDirectory,
    /// The page's parent is not a directory.
    NotDirectory,
    /// An entry of the page is the directory itself or one of its ancestors.
    AncestorEntry,
}

/// The kind of a visible row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// An entry.
    Entry,
    /// A "Loading…" status row.
    Loading,
    /// An "Empty folder" status row.
    Empty,
    /// An error status row carrying the message (retried with the directory's error cursor).
    Error(String),
    /// A "Load more…" status row carrying the cursor of the next page.
    LoadMore(String),
}

/// One visible row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleRow {
    /// The entry path, or the synthetic path of a status row.
    pub path: String,
    /// The nesting depth (children of the root have depth 0; status rows have the depth of the
    /// children of their directory).
    pub depth: usize,
    /// The kind.
    pub kind: RowKind,
}

impl VisibleRow {
    /// Whether this is an entry row.
    pub fn is_entry(&self) -> bool {
        self.kind == RowKind::Entry
    }

    /// The directory of a status row.
    pub fn status_directory(&self) -> Option<&str> {
        (!self.is_entry()).then(|| row_parent(&self.path))
    }

    fn selectable(&self) -> bool {
        matches!(self.kind, RowKind::Entry | RowKind::LoadMore(_))
    }
}

/// The result of an operation that can change the model and ask for a load.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the model changed.
    pub changed: bool,
    /// A page the owner must now supply.
    pub load: Option<LoadRequest>,
}

/// What activating a row did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activation {
    /// Nothing to activate.
    None,
    /// A directory was toggled (and possibly a load requested).
    Toggled(Outcome),
    /// A file or symlink must be opened by the owner.
    Open(String),
    /// A status row asked for a page.
    Load(Option<LoadRequest>),
}

/// One step of revealing a path (R22).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevealStep {
    /// The ancestors are expanded and the path is selected.
    Done,
    /// A page of an ancestor must be loaded first.
    Load(LoadRequest),
    /// A load that the reveal waits for is in flight.
    Pending,
    /// An ancestor failed to load.
    Failed(String),
    /// The path does not exist in the fully loaded listing.
    Missing,
}

fn status_path(directory: &str, kind: &str) -> String {
    format!("{directory}\0{kind}")
}

/// The model of an interactive file tree.
#[derive(Clone, Debug)]
pub struct TreeModel {
    nodes: HashMap<String, Node>,
    expanded: HashSet<String>,
    selection: HashSet<String>,
    cursor: Option<String>,
    anchor: Option<String>,
    generation: u64,
    include_ignored: bool,
    rows: Vec<VisibleRow>,
    index: HashMap<String, usize>,
    rows_revision: u64,
}

impl Default for TreeModel {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeModel {
    /// An empty tree whose root is expanded and unloaded.
    pub fn new() -> Self {
        let mut model = Self {
            nodes: HashMap::new(),
            expanded: HashSet::new(),
            selection: HashSet::new(),
            cursor: None,
            anchor: None,
            generation: 0,
            include_ignored: true,
            rows: Vec::new(),
            index: HashMap::new(),
            rows_revision: 0,
        };
        model.install_root();
        model.rebuild();
        model
    }

    fn install_root(&mut self) {
        self.nodes.clear();
        self.nodes.insert(
            ROOT.to_owned(),
            Node::new(Entry::new(ROOT, EntryKind::Directory)),
        );
        self.expanded.clear();
        self.expanded.insert(ROOT.to_owned());
    }

    // ---- queries ----------------------------------------------------------------------------

    /// The current generation token of the tree.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The visible rows in display order.
    pub fn visible_rows(&self) -> &[VisibleRow] {
        &self.rows
    }

    /// A counter that changes whenever the visible rows may have changed (selection changes do
    /// not touch it).
    pub fn rows_revision(&self) -> u64 {
        self.rows_revision
    }

    /// The index of the visible row with the given (entry or status) path.
    pub fn row_index(&self, path: &str) -> Option<usize> {
        self.index.get(path).copied()
    }

    /// The node of `path` (the root is the empty path).
    pub fn node(&self, path: &str) -> Option<&Node> {
        self.nodes.get(path)
    }

    /// Whether `path` is an expanded directory (the root always is).
    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    /// Whether `path` is selected.
    pub fn is_selected(&self, path: &str) -> bool {
        self.selection.contains(path)
    }

    /// The keyboard cursor: an entry path, or the synthetic path of a load-more row.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// The anchor of Shift ranges.
    pub fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    /// The number of selected entries.
    pub fn selection_len(&self) -> usize {
        self.selection.len()
    }

    /// Whether ignored entries are shown.
    pub fn include_ignored(&self) -> bool {
        self.include_ignored
    }

    /// The selected entries in visible order.
    pub fn selected(&self) -> Vec<&Entry> {
        self.selected_paths()
            .iter()
            .filter_map(|path| self.nodes.get(path).map(|node| &node.entry))
            .collect()
    }

    /// The selected paths in visible order.
    pub fn selected_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for row in &self.rows {
            if row.is_entry() && self.selection.contains(&row.path) {
                paths.push(row.path.clone());
            }
        }
        paths
    }

    /// Whether any proper ancestor of `path` is selected.
    fn has_selected_ancestor(&self, path: &str) -> bool {
        let mut ancestor = parent_path(path);
        while !ancestor.is_empty() {
            if self.selection.contains(ancestor) {
                return true;
            }
            ancestor = parent_path(ancestor);
        }
        false
    }

    /// The selected paths that have no selected ancestor, in visible order. Ancestry is checked
    /// by walking the entry's own parents.
    pub fn selection_roots(&self) -> Vec<String> {
        let mut roots = self.selected_paths();
        roots.retain(|path| !self.has_selected_ancestor(path));
        roots
    }

    /// The paths an action (rename, delete, context menu) applies to: the selection roots when
    /// the cursor is part of the selection, else the cursor alone (an entry), else none.
    pub fn action_paths(&self) -> Vec<String> {
        match self.cursor.as_deref() {
            Some(cursor) if self.selection.contains(cursor) => self.selection_roots(),
            Some(cursor) if self.nodes.contains_key(cursor) => vec![cursor.to_owned()],
            _ => Vec::new(),
        }
    }

    /// Whether `path` or any of its ancestors is ignored.
    pub fn is_ignored(&self, path: &str) -> bool {
        let mut current = path;
        loop {
            if self
                .nodes
                .get(current)
                .is_some_and(|node| node.entry.ignored)
            {
                return true;
            }
            if current.is_empty() {
                return false;
            }
            current = parent_path(current);
        }
    }

    // ---- loading ----------------------------------------------------------------------------

    /// Starts a load of `path` with `cursor`. `None` when the path is not a directory or a load
    /// of it is already in flight (an in-flight load is never duplicated).
    pub fn begin_load(&mut self, path: &str, cursor: Option<String>) -> Option<LoadRequest> {
        let node = self.nodes.get_mut(path)?;
        if !node.entry.is_dir() || matches!(node.load, LoadState::Loading(_)) {
            return None;
        }
        node.load = LoadState::Loading(cursor.clone());
        let request = LoadRequest {
            path: path.to_owned(),
            cursor,
            generation: self.generation,
        };
        self.rebuild();
        Some(request)
    }

    /// Records a failed load. Ignored when `generation` is stale or the directory is not
    /// loading. The same cursor is retried by the retry row.
    pub fn fail_load(&mut self, path: &str, message: impl Into<String>, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(node) = self.nodes.get_mut(path) else {
            return false;
        };
        let LoadState::Loading(cursor) = &node.load else {
            return false;
        };
        node.load = LoadState::Error(message.into(), cursor.clone());
        self.rebuild();
        true
    }

    /// Applies a page (R3).
    pub fn apply_page(&mut self, page: Page, generation: u64) -> Result<(), PageError> {
        if generation != self.generation {
            return Err(PageError::StaleGeneration);
        }
        let directory = page.directory.as_str();
        let node = self
            .nodes
            .get(directory)
            .ok_or(PageError::UnknownDirectory)?;
        if !node.entry.is_dir() {
            return Err(PageError::NotDirectory);
        }
        if page
            .entries
            .iter()
            .any(|entry| is_within(directory, &entry.path))
        {
            return Err(PageError::AncestorEntry);
        }
        let first = !matches!(node.load, LoadState::Loading(Some(_)));
        let mut changed_kind: HashSet<String> = HashSet::new();
        let mut incoming = Vec::new();
        for entry in page.entries {
            if entry.path.is_empty() || parent_path(&entry.path) != directory {
                continue;
            }
            if let Some(existing) = self.nodes.get(&entry.path) {
                if existing.entry.kind != entry.kind {
                    changed_kind.insert(entry.path.clone());
                }
            }
            incoming.push(entry);
        }
        for path in &changed_kind {
            self.remove_subtree(path);
        }
        let node = self.nodes.get_mut(directory).expect("checked above");
        let mut children = std::mem::take(&mut node.children);
        let mut listing = std::mem::take(&mut node.listing);
        if first {
            listing.clear();
        }
        children.retain(|child| !changed_kind.contains(child));
        let mut present: HashSet<String> = children.iter().cloned().collect();
        for entry in incoming {
            let path = entry.path.clone();
            listing.insert(path.clone());
            match self.nodes.get_mut(&path) {
                Some(existing) if present.contains(&path) => existing.entry = entry,
                _ => {
                    present.insert(path.clone());
                    children.push(path.clone());
                    self.nodes.insert(path, Node::new(entry));
                }
            }
        }
        if page.next_cursor.is_none() {
            // Only the last page proves deletion: children absent from the whole listing go.
            for child in children.iter().filter(|child| !listing.contains(*child)) {
                self.remove_subtree(child);
            }
            children.retain(|child| listing.contains(child));
            listing.clear();
        }
        children.retain(|child| self.nodes.contains_key(child));
        let node = self.nodes.get_mut(directory).expect("checked above");
        node.children = children;
        node.listing = listing;
        node.load = LoadState::Loaded(page.next_cursor);
        node.stale = false;
        node.has_loaded = true;
        self.sort_children(directory);
        self.rebuild();
        Ok(())
    }

    /// Marks one directory stale: the next expansion refreshes it, keeping its rows meanwhile.
    pub fn invalidate_directory(&mut self, path: &str) {
        if let Some(node) = self.nodes.get_mut(path) {
            if node.entry.is_dir() {
                node.stale = true;
            }
        }
    }

    /// Discards every in-flight page: increments the generation, marks every directory stale
    /// and turns Loading directories back to Unloaded.
    pub fn invalidate_loads(&mut self) {
        self.generation += 1;
        for node in self.nodes.values_mut() {
            if !node.entry.is_dir() {
                continue;
            }
            node.stale = true;
            if matches!(node.load, LoadState::Loading(_)) {
                node.load = LoadState::Unloaded;
            }
        }
        self.rebuild();
    }

    /// Clears the whole tree (entries, expansion, selection) and increments the generation.
    pub fn reset(&mut self) {
        self.generation += 1;
        self.install_root();
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.rebuild();
    }

    /// Changes whether ignored entries are shown. A change invalidates every load (the owner
    /// lists with the new setting); it returns whether the setting changed.
    pub fn set_include_ignored(&mut self, include: bool) -> bool {
        if self.include_ignored == include {
            return false;
        }
        self.include_ignored = include;
        self.invalidate_loads();
        true
    }

    /// Begins a load for every visible expanded directory that is unloaded or stale and not
    /// loading (used after construction, a reset or an invalidation).
    pub fn request_loads(&mut self) -> Vec<LoadRequest> {
        let mut wanted: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        self.collect_wanted(ROOT, &mut wanted, &mut seen);
        wanted
            .into_iter()
            .filter_map(|path| self.begin_load(&path, None))
            .collect()
    }

    fn collect_wanted(&self, directory: &str, out: &mut Vec<String>, seen: &mut HashSet<String>) {
        if !seen.insert(directory.to_owned()) {
            return;
        }
        let Some(node) = self.nodes.get(directory) else {
            return;
        };
        let wants = match node.load {
            LoadState::Unloaded => true,
            LoadState::Loaded(_) => node.stale,
            LoadState::Loading(_) | LoadState::Error(..) => false,
        };
        if wants {
            out.push(directory.to_owned());
        }
        for child in &node.children {
            if self.expanded.contains(child) {
                self.collect_wanted(child, out, seen);
            }
        }
    }

    /// Retries the failed load of `directory` with its error cursor, or requests the next page
    /// announced by a load-more row.
    pub fn load_more(&mut self, directory: &str) -> Option<LoadRequest> {
        let cursor = match &self.nodes.get(directory)?.load {
            LoadState::Error(_, cursor) => cursor.clone(),
            LoadState::Loaded(Some(cursor)) => Some(cursor.clone()),
            _ => return None,
        };
        self.begin_load(directory, cursor)
    }

    // ---- expansion --------------------------------------------------------------------------

    fn needs_load(&self, path: &str) -> Option<Option<String>> {
        let node = self.nodes.get(path)?;
        match &node.load {
            LoadState::Unloaded => Some(None),
            LoadState::Error(_, cursor) => Some(cursor.clone()),
            LoadState::Loaded(_) if node.stale => Some(None),
            _ => None,
        }
    }

    /// Expands a directory, loading it when it is unloaded, failed or stale.
    pub fn expand(&mut self, path: &str) -> Outcome {
        let is_dir = self.nodes.get(path).is_some_and(|node| node.entry.is_dir());
        if !is_dir {
            return Outcome::default();
        }
        let newly = self.expanded.insert(path.to_owned());
        let load = self
            .needs_load(path)
            .and_then(|cursor| self.begin_load(path, cursor));
        if newly {
            self.rebuild();
        }
        Outcome {
            changed: newly || load.is_some(),
            load,
        }
    }

    /// Collapses a directory (the root cannot be collapsed). A cursor inside it moves to the
    /// directory; the directory joins the selection only if the hidden cursor was selected.
    pub fn collapse(&mut self, path: &str) -> bool {
        if path.is_empty() || !self.expanded.contains(path) {
            return false;
        }
        if let Some(cursor) = self.cursor.clone() {
            if is_within(row_parent_or_self(&cursor), path) && cursor != path {
                let was_selected = self.selection.contains(&cursor);
                self.cursor = Some(path.to_owned());
                if was_selected {
                    self.selection.insert(path.to_owned());
                }
            }
        }
        self.expanded.remove(path);
        self.rebuild();
        true
    }

    /// Toggles a directory.
    pub fn toggle_expanded(&mut self, path: &str) -> Outcome {
        if self.expanded.contains(path) {
            Outcome {
                changed: self.collapse(path),
                load: None,
            }
        } else {
            self.expand(path)
        }
    }

    // ---- selection --------------------------------------------------------------------------

    fn entry_row(&self, path: &str) -> Option<usize> {
        let ix = *self.index.get(path)?;
        self.rows[ix].is_entry().then_some(ix)
    }

    fn range_paths(&self, from: usize, to: usize) -> Vec<String> {
        let (low, high) = if from <= to { (from, to) } else { (to, from) };
        self.rows[low..=high]
            .iter()
            .filter(|row| row.is_entry())
            .map(|row| row.path.clone())
            .collect()
    }

    /// Plain selection: the entry alone, cursor and anchor on it. `false` for a path that is
    /// not a visible entry or when nothing changed.
    pub fn select(&mut self, path: &str) -> bool {
        if self.entry_row(path).is_none() {
            return false;
        }
        let before = (
            self.selection.len() == 1 && self.selection.contains(path),
            self.cursor.as_deref() == Some(path),
            self.anchor.as_deref() == Some(path),
        );
        self.selection.clear();
        self.selection.insert(path.to_owned());
        self.cursor = Some(path.to_owned());
        self.anchor = Some(path.to_owned());
        before != (true, true, true)
    }

    /// Modifier-click: toggles the entry; anchor and cursor move to it.
    pub fn toggle_selected(&mut self, path: &str) -> bool {
        if self.entry_row(path).is_none() {
            return false;
        }
        if !self.selection.remove(path) {
            self.selection.insert(path.to_owned());
        }
        self.cursor = Some(path.to_owned());
        self.anchor = Some(path.to_owned());
        true
    }

    /// Moves the cursor onto a selected entry without changing the selection (a secondary
    /// click inside the selection). `false` when `path` is not a selected visible entry.
    pub fn set_cursor(&mut self, path: &str) -> bool {
        if self.entry_row(path).is_none() || !self.selection.contains(path) {
            return false;
        }
        let changed = self.cursor.as_deref() != Some(path);
        self.cursor = Some(path.to_owned());
        changed
    }

    /// Puts the cursor on a selectable row: an entry is selected alone, a load-more row only
    /// takes the cursor. `false` when the row is not selectable or nothing changed.
    pub fn focus_row_path(&mut self, path: &str) -> bool {
        let Some(&ix) = self.index.get(path) else {
            return false;
        };
        let row = self.rows[ix].clone();
        if !row.selectable() {
            return false;
        }
        self.focus_row(&row)
    }

    /// Shift-click: the entries between the anchor (the cursor when there is no anchor, the
    /// target when neither exists) and `path`, replacing the selection or, when `additive`,
    /// joining it. The cursor moves to the target; the anchor stays.
    pub fn select_range_to(&mut self, path: &str, additive: bool) -> bool {
        let Some(target) = self.entry_row(path) else {
            return false;
        };
        let base = self
            .anchor
            .clone()
            .filter(|anchor| self.entry_row(anchor).is_some())
            .or_else(|| {
                self.cursor
                    .clone()
                    .filter(|cursor| self.entry_row(cursor).is_some())
            })
            .unwrap_or_else(|| path.to_owned());
        let base_ix = self.entry_row(&base).unwrap_or(target);
        let range = self.range_paths(base_ix, target);
        if !additive {
            self.selection.clear();
        }
        self.selection.extend(range);
        self.cursor = Some(path.to_owned());
        self.anchor = Some(base);
        true
    }

    fn entry_row_indices(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.is_entry())
            .map(|(ix, _)| ix)
            .collect()
    }

    /// Shift+Up/Down: moves the cursor `delta` entry rows (clamped) and replaces the selection
    /// with the range from the anchor, which is set to the starting cursor when there is none.
    pub fn extend_selection(&mut self, delta: isize) -> bool {
        let entries = self.entry_row_indices();
        if entries.is_empty() {
            return false;
        }
        let start = self.cursor.clone();
        let target_pos = match start.as_deref().and_then(|cursor| self.index.get(cursor)) {
            None => {
                if delta >= 0 {
                    0
                } else {
                    entries.len() - 1
                }
            }
            Some(&row_ix) => {
                let (pos, on_entry) = match entries.binary_search(&row_ix) {
                    Ok(pos) => (pos as isize, true),
                    Err(pos) => (pos as isize, false),
                };
                let moved = if on_entry {
                    pos + delta
                } else if delta > 0 {
                    pos + delta - 1
                } else {
                    pos + delta
                };
                moved.clamp(0, entries.len() as isize - 1) as usize
            }
        };
        let target_row = entries[target_pos];
        let target = self.rows[target_row].path.clone();
        let anchor = self
            .anchor
            .clone()
            .filter(|anchor| self.entry_row(anchor).is_some())
            .or_else(|| start.filter(|cursor| self.entry_row(cursor).is_some()))
            .unwrap_or_else(|| target.clone());
        let anchor_ix = self.entry_row(&anchor).unwrap_or(target_row);
        let range = self.range_paths(anchor_ix, target_row);
        let new_selection: HashSet<String> = range.into_iter().collect();
        let changed = new_selection != self.selection
            || self.cursor.as_deref() != Some(&target)
            || self.anchor.as_deref() != Some(&anchor);
        self.selection = new_selection;
        self.cursor = Some(target);
        self.anchor = Some(anchor);
        changed
    }

    /// Clears the selection, the cursor and the anchor. `true` when something was set.
    pub fn clear_selection(&mut self) -> bool {
        let changed = !self.selection.is_empty() || self.cursor.is_some() || self.anchor.is_some();
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        changed
    }

    /// Escape: collapses the selection to the cursor alone. `false` when it already is.
    pub fn collapse_selection(&mut self) -> bool {
        let Some(cursor) = self.cursor.clone() else {
            return false;
        };
        if self.entry_row(&cursor).is_none() {
            return false;
        }
        if self.selection.len() == 1 && self.selection.contains(&cursor) {
            return false;
        }
        self.selection.clear();
        self.selection.insert(cursor.clone());
        self.anchor = Some(cursor);
        true
    }

    fn move_cursor(&mut self, delta: isize) -> bool {
        let selectable: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.selectable())
            .map(|(ix, _)| ix)
            .collect();
        if selectable.is_empty() {
            return false;
        }
        let pos = match self
            .cursor
            .as_deref()
            .and_then(|cursor| self.index.get(cursor))
            .and_then(|row_ix| selectable.binary_search(row_ix).ok())
        {
            Some(pos) => (pos as isize + delta).clamp(0, selectable.len() as isize - 1) as usize,
            None if delta >= 0 => 0,
            None => selectable.len() - 1,
        };
        let row = self.rows[selectable[pos]].clone();
        self.focus_row(&row)
    }

    fn focus_row(&mut self, row: &VisibleRow) -> bool {
        match row.kind {
            RowKind::Entry => self.select(&row.path),
            _ => {
                let changed =
                    self.cursor.as_deref() != Some(&row.path) || !self.selection.is_empty();
                self.selection.clear();
                self.cursor = Some(row.path.clone());
                changed
            }
        }
    }

    /// Down: moves the cursor to the next selectable row (entry or load-more) and selects only it.
    pub fn select_next(&mut self) -> bool {
        self.move_cursor(1)
    }

    /// Up: moves the cursor to the previous selectable row and selects only it.
    pub fn select_previous(&mut self) -> bool {
        self.move_cursor(-1)
    }

    /// Left: collapses the cursor's directory when it is expanded, else selects its parent
    /// (a top-level entry stays).
    pub fn select_parent(&mut self) -> Outcome {
        let Some(cursor) = self.cursor.clone() else {
            return Outcome::default();
        };
        let is_expanded_dir = self.entry_row(&cursor).is_some()
            && self.expanded.contains(&cursor)
            && self
                .nodes
                .get(&cursor)
                .is_some_and(|node| node.entry.is_dir());
        if is_expanded_dir {
            return Outcome {
                changed: self.collapse(&cursor),
                load: None,
            };
        }
        let parent = row_parent(&cursor).to_owned();
        if parent.is_empty() {
            return Outcome::default();
        }
        Outcome {
            changed: self.select(&parent),
            load: None,
        }
    }

    /// Right: selects the first child of an expanded directory, expands a collapsed one.
    pub fn select_first_child(&mut self) -> Outcome {
        let Some(cursor) = self.cursor.clone() else {
            return Outcome::default();
        };
        let Some(row_ix) = self.entry_row(&cursor) else {
            return Outcome::default();
        };
        if !self
            .nodes
            .get(&cursor)
            .is_some_and(|node| node.entry.is_dir())
        {
            return Outcome::default();
        }
        if self.expanded.contains(&cursor) {
            let child = self
                .rows
                .get(row_ix + 1)
                .filter(|row| row.is_entry() && parent_path(&row.path) == cursor)
                .map(|row| row.path.clone());
            return Outcome {
                changed: child.is_some_and(|child| self.select(&child)),
                load: None,
            };
        }
        self.expand(&cursor)
    }

    /// Activates the cursor row (Enter, Space, plain click): a directory toggles, a file or
    /// symlink is opened by the owner, a load-more row requests its page.
    pub fn activate(&mut self, path: &str) -> Activation {
        let Some(&row_ix) = self.index.get(path) else {
            return Activation::None;
        };
        match self.rows[row_ix].kind.clone() {
            RowKind::Entry => {
                let is_dir = self.nodes.get(path).is_some_and(|node| node.entry.is_dir());
                if is_dir {
                    Activation::Toggled(self.toggle_expanded(path))
                } else {
                    Activation::Open(path.to_owned())
                }
            }
            RowKind::LoadMore(_) | RowKind::Error(_) => {
                let directory = row_parent(path).to_owned();
                Activation::Load(self.load_more(&directory))
            }
            RowKind::Loading | RowKind::Empty => Activation::None,
        }
    }

    // ---- mutation ---------------------------------------------------------------------------

    fn descendants(&self, path: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![path.to_owned()];
        while let Some(current) = stack.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            if let Some(node) = self.nodes.get(&current) {
                stack.extend(node.children.iter().cloned());
            }
            out.push(current);
        }
        out
    }

    fn remove_subtree(&mut self, path: &str) {
        for descendant in self.descendants(path) {
            self.nodes.remove(&descendant);
            self.expanded.remove(&descendant);
        }
    }

    fn sort_children(&mut self, directory: &str) {
        let Some(node) = self.nodes.get(directory) else {
            return;
        };
        let mut children = node.children.clone();
        children.sort_by_cached_key(|path| {
            let entry = self.nodes.get(path).map(|node| &node.entry);
            (
                entry.map_or(u8::MAX, |entry| entry.kind.rank()),
                entry.map_or_else(String::new, |entry| entry.name.to_lowercase()),
                path.clone(),
            )
        });
        if let Some(node) = self.nodes.get_mut(directory) {
            node.children = children;
        }
    }

    /// Removes an entry and its subtree. A cursor inside it moves to the nearest existing
    /// parent (never the root), which joins the selection only when every selected entry lay
    /// inside the removed subtree.
    pub fn remove(&mut self, path: &str) -> bool {
        if path.is_empty() || !self.nodes.contains_key(path) {
            return false;
        }
        let cursor_inside = self
            .cursor
            .as_deref()
            .is_some_and(|cursor| is_within(row_parent_or_self(cursor), path));
        let any_selected_inside = self.selection.iter().any(|entry| is_within(entry, path));
        let all_selected_inside = self.selection.iter().all(|entry| is_within(entry, path));
        let parent = parent_path(path).to_owned();
        self.remove_subtree(path);
        if let Some(node) = self.nodes.get_mut(&parent) {
            node.children.retain(|child| child != path);
        }
        if cursor_inside {
            let mut candidate = parent.as_str();
            while !candidate.is_empty() && !self.nodes.contains_key(candidate) {
                candidate = parent_path(candidate);
            }
            if candidate.is_empty() {
                self.cursor = None;
            } else {
                self.cursor = Some(candidate.to_owned());
                if any_selected_inside && all_selected_inside {
                    self.selection.insert(candidate.to_owned());
                }
            }
        }
        self.selection.retain(|entry| !is_within(entry, path));
        self.rebuild();
        true
    }

    /// Re-keys `old` and every descendant to `new` (a rename or move) without losing expansion
    /// or selection; marks the moved subtree and both parents stale, expands the destination's
    /// ancestors and re-sorts. Refused (`false`) for a move onto itself or into its own subtree,
    /// for an unknown source, or when the destination has no known parent directory.
    pub fn relocate_subtree(&mut self, old: &str, new: &str) -> bool {
        if old.is_empty()
            || new.is_empty()
            || old == new
            || is_within(new, old)
            || !self.nodes.contains_key(old)
        {
            return false;
        }
        let new_parent = parent_path(new).to_owned();
        if !self
            .nodes
            .get(&new_parent)
            .is_some_and(|node| node.entry.is_dir())
        {
            return false;
        }
        let old_parent = parent_path(old).to_owned();
        let moved = self.descendants(old);
        // Anything already at the destination is replaced.
        if self.nodes.contains_key(new) {
            self.remove_subtree(new);
            if let Some(node) = self.nodes.get_mut(&new_parent) {
                node.children.retain(|child| child != new);
            }
        }
        let rekey = |path: &str| -> String { format!("{new}{}", &path[old.len()..]) };
        let mut rekeyed_nodes = Vec::with_capacity(moved.len());
        for path in &moved {
            if let Some(mut node) = self.nodes.remove(path) {
                let new_path = rekey(path);
                node.entry.path = new_path.clone();
                if path == old {
                    node.entry.name = file_name(new).to_owned();
                }
                node.children = node.children.iter().map(|child| rekey(child)).collect();
                node.listing = node.listing.iter().map(|child| rekey(child)).collect();
                if node.entry.is_dir() {
                    node.stale = true;
                    node.load = match node.load {
                        LoadState::Loading(_) => LoadState::Unloaded,
                        ref other => other.clone(),
                    };
                }
                rekeyed_nodes.push((new_path, node));
            }
        }
        for (path, node) in rekeyed_nodes {
            self.nodes.insert(path, node);
        }
        let was_expanded: Vec<String> = moved
            .iter()
            .filter(|path| self.expanded.remove(*path))
            .cloned()
            .collect();
        for path in was_expanded {
            self.expanded.insert(rekey(&path));
        }
        self.selection = self
            .selection
            .iter()
            .map(|path| {
                if is_within(path, old) {
                    rekey(path)
                } else {
                    path.clone()
                }
            })
            .collect();
        for slot in [&mut self.cursor, &mut self.anchor] {
            if let Some(path) = slot.as_mut() {
                if is_within(row_parent_or_self(path), old) {
                    *path = rekey(path);
                }
            }
        }
        if let Some(node) = self.nodes.get_mut(&old_parent) {
            node.children.retain(|child| child != old);
            node.stale = true;
            if matches!(node.load, LoadState::Loading(_)) {
                node.load = LoadState::Unloaded;
            }
        }
        if let Some(node) = self.nodes.get_mut(&new_parent) {
            node.children.push(new.to_owned());
            node.stale = true;
            if matches!(node.load, LoadState::Loading(_)) {
                node.load = LoadState::Unloaded;
            }
        }
        let mut ancestor = new_parent.as_str();
        while !ancestor.is_empty() {
            self.expanded.insert(ancestor.to_owned());
            ancestor = parent_path(ancestor);
        }
        self.sort_children(&new_parent);
        self.rebuild();
        true
    }

    // ---- reveal -----------------------------------------------------------------------------

    /// One step of revealing `path` (R22): walks the ancestors top-down; the first ancestor
    /// that does not list the next component yields the load to perform (following pagination
    /// cursors), or `Pending`/`Failed`/`Missing`. When every ancestor lists its child, the
    /// ancestors are expanded and the path is selected.
    pub fn reveal_search_match(&mut self, path: &str) -> RevealStep {
        if path.is_empty() {
            return RevealStep::Missing;
        }
        let mut components: Vec<String> = Vec::new();
        let mut current = path;
        while !current.is_empty() {
            components.push(current.to_owned());
            current = parent_path(current);
        }
        components.reverse();
        let mut directory = ROOT.to_owned();
        for next in &components {
            let Some(node) = self.nodes.get(&directory) else {
                return RevealStep::Missing;
            };
            if !node.entry.is_dir() {
                return RevealStep::Missing;
            }
            if !node.children.contains(next) {
                let cursor = match &node.load {
                    LoadState::Loading(_) => return RevealStep::Pending,
                    LoadState::Error(message, _) => return RevealStep::Failed(message.clone()),
                    LoadState::Loaded(None) if !node.stale => return RevealStep::Missing,
                    LoadState::Loaded(Some(cursor)) if !node.stale => Some(cursor.clone()),
                    _ => None,
                };
                return match self.begin_load(&directory.clone(), cursor) {
                    Some(request) => RevealStep::Load(request),
                    None => RevealStep::Pending,
                };
            }
            directory = next.clone();
        }
        let mut ancestor = parent_path(path);
        while !ancestor.is_empty() {
            self.expanded.insert(ancestor.to_owned());
            ancestor = parent_path(ancestor);
        }
        self.rebuild();
        self.select(path);
        RevealStep::Done
    }

    // ---- rows -------------------------------------------------------------------------------

    fn rebuild(&mut self) {
        self.rows_revision += 1;
        let mut rows = Vec::new();
        let mut visited = HashSet::new();
        self.flatten(ROOT, 0, &mut rows, &mut visited);
        self.index.clear();
        for (ix, row) in rows.iter().enumerate() {
            self.index.entry(row.path.clone()).or_insert(ix);
        }
        self.rows = rows;
        let index = &self.index;
        let rows = &self.rows;
        let visible_entry = |path: &str| index.get(path).is_some_and(|ix| rows[*ix].is_entry());
        self.selection.retain(|path| visible_entry(path));
        if let Some(cursor) = &self.cursor {
            if !index.contains_key(cursor) {
                self.cursor = None;
            }
        }
        if let Some(anchor) = &self.anchor {
            if !visible_entry(anchor) {
                self.anchor = None;
            }
        }
    }

    fn flatten(
        &self,
        directory: &str,
        depth: usize,
        rows: &mut Vec<VisibleRow>,
        visited: &mut HashSet<String>,
    ) {
        if !visited.insert(directory.to_owned()) {
            return;
        }
        let Some(node) = self.nodes.get(directory) else {
            return;
        };
        let mut emitted = 0usize;
        for child in &node.children {
            let Some(child_node) = self.nodes.get(child) else {
                continue;
            };
            if !self.include_ignored && self.is_ignored(child) {
                continue;
            }
            emitted += 1;
            rows.push(VisibleRow {
                path: child.clone(),
                depth,
                kind: RowKind::Entry,
            });
            if child_node.entry.is_dir() && self.expanded.contains(child) {
                self.flatten(child, depth + 1, rows, visited);
            }
        }
        let status = |kind: &str| status_path(directory, kind);
        match &node.load {
            LoadState::Unloaded => {}
            LoadState::Loading(cursor) => {
                if cursor.is_some() || !node.has_loaded {
                    rows.push(VisibleRow {
                        path: status("loading"),
                        depth,
                        kind: RowKind::Loading,
                    });
                } else if emitted == 0 {
                    rows.push(VisibleRow {
                        path: status("empty"),
                        depth,
                        kind: RowKind::Empty,
                    });
                }
            }
            LoadState::Loaded(next) => {
                if emitted == 0 {
                    rows.push(VisibleRow {
                        path: status("empty"),
                        depth,
                        kind: RowKind::Empty,
                    });
                }
                if let Some(cursor) = next {
                    rows.push(VisibleRow {
                        path: status("load-more"),
                        depth,
                        kind: RowKind::LoadMore(cursor.clone()),
                    });
                }
            }
            LoadState::Error(message, _) => rows.push(VisibleRow {
                path: status("error"),
                depth,
                kind: RowKind::Error(message.clone()),
            }),
        }
    }
}

fn row_parent_or_self(path: &str) -> &str {
    if path.contains('\0') {
        row_parent(path)
    } else {
        path
    }
}

#[cfg(test)]
mod tests;
