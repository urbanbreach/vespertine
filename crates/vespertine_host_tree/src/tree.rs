//! The retained host tree and its atomic batch application (APP-006, PRF-015).

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::hash::BuildHasherDefault;
use std::sync::Arc;

use rustc_hash::FxHasher;
use serde_json::Value;

use crate::decode::{Op, StyleSource, decode_batch};
use crate::style::{
    SharedStyle, StyleTable, StyleTableStats, StyleValidator, content_eq, require_object,
};

/// Stable element identity chosen by the sender (0 ..= 2^53 - 1).
pub type NodeId = u64;

const NONE: u64 = u64::MAX;
const ANCHORED_TYPE: &str = "anchored";
/// Listener types that give an element a keyboard focus handle.
const FOCUS_LISTENERS: [&str; 4] = ["keyDown", "keyUp", "focus", "blur"];
/// Element types that always need a keyboard focus handle.
const FOCUS_TYPES: [&str; 2] = ["input", "textarea"];
const PROP_AUTO_FOCUS: &str = "autoFocus";
const PROP_TEST_ID: &str = "testId";
const PROP_TAB_INDEX: &str = "tabIndex";
const PROP_MOTION: &str = "motion";
const PROP_HIGHLIGHT: &str = "highlight";

type FxBuild = BuildHasherDefault<FxHasher>;

/// Kind of a rejected batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchErrorKind {
    /// Phase 1: the batch could not be decoded into operations.
    Decode,
    /// Phase 2: a setStyle payload could not be resolved into a style.
    Style,
}

/// Why a batch was rejected. Nothing was applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchError {
    kind: BatchErrorKind,
    op_index: Option<usize>,
    message: String,
}

impl BatchError {
    pub fn kind(&self) -> BatchErrorKind {
        self.kind
    }

    /// Zero-based index of the failing operation, when the failure belongs to one.
    pub fn op_index(&self) -> Option<usize> {
        self.op_index
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for BatchError {}

/// Outcome of an applied batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchResult {
    /// Identities actually destroyed, in operation order and pre-order per destroy.
    pub destroyed: Vec<NodeId>,
    /// Whether anything a frame would draw changed (the redraw flag, read and cleared
    /// once per batch). A batch of equal resends leaves this false.
    pub redraw: bool,
    /// Whether the searchable text of any subtree changed (searchable revision bumped).
    pub searchable_changed: bool,
}

/// A batch that decoded and resolved its styles completely (phases 1 and 2); committing it
/// cannot fail. Borrows the serialized batch it was decoded from.
pub struct PreparedBatch<'a> {
    ops: Vec<Op<'a>>,
    creates: usize,
}

impl PreparedBatch<'_> {
    /// Number of operations in the batch.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// Construction options.
#[derive(Clone, Copy)]
pub struct TreeOptions {
    /// Validates a parsed style against the style schema. The default requires an object.
    pub style_validator: StyleValidator,
    /// Test hook: forces every style hash (raw and content) to this constant so collision
    /// handling can be exercised.
    pub forced_style_hash: Option<u64>,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            style_validator: require_object,
            forced_style_hash: None,
        }
    }
}

/// Test-support counters. They never alter semantics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeStats {
    pub batches_applied: u64,
    /// Applied batches that set the redraw flag (window-invalidating commits).
    pub redraw_commits: u64,
    /// Applied batches that changed nothing observable.
    pub idle_commits: u64,
    /// Rejected batches.
    pub rejected_batches: u64,
    /// Searchable-content revision: bumped once per batch that changed searchable text.
    pub searchable_revision: u64,
    pub live_elements: usize,
    pub style: StyleTableStats,
    /// Nodes visited by focus resolution since the last reset.
    pub focus_visits: u64,
    /// Nodes visited by animation advancement since the last reset.
    pub animation_visits: u64,
}

struct Node {
    element_type: Arc<str>,
    parent: u64,
    first_child: u64,
    last_child: u64,
    prev_sibling: u64,
    next_sibling: u64,
    text: Option<Box<str>>,
    style: Option<SharedStyle>,
    // Boxed on purpose: a lazily created map costs one pointer in every node that has none.
    #[allow(clippy::box_collection)]
    listeners: Option<Box<BTreeSet<Box<str>>>>,
    #[allow(clippy::box_collection)]
    props: Option<Box<BTreeMap<Box<str>, Value>>>,
    test_id: Option<Box<str>>,
    subtree_revision: u32,
    search_revision: u32,
    auto_focus: bool,
}

impl Node {
    fn new(element_type: Arc<str>, revision: u32) -> Self {
        Self {
            element_type,
            parent: NONE,
            first_child: NONE,
            last_child: NONE,
            prev_sibling: NONE,
            next_sibling: NONE,
            text: None,
            style: None,
            listeners: None,
            props: None,
            test_id: None,
            subtree_revision: revision,
            search_revision: revision,
            auto_focus: false,
        }
    }

    fn prop(&self, key: &str) -> Option<&Value> {
        self.props.as_ref().and_then(|props| props.get(key))
    }

    fn needs_focus_handle(&self) -> bool {
        if FOCUS_TYPES.contains(&&*self.element_type) {
            return true;
        }
        if let Some(Value::Number(number)) = self.prop(PROP_TAB_INDEX) {
            let integral = number.is_i64()
                || number.is_u64()
                || number.as_f64().is_some_and(|f| f.fract() == 0.0);
            if integral {
                return true;
            }
        }
        self.listeners
            .as_ref()
            .is_some_and(|set| FOCUS_LISTENERS.iter().any(|name| set.contains(*name)))
    }

    fn is_animated(&self) -> bool {
        self.prop(PROP_MOTION).is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// Subtree revisions only (style, properties).
    Paint,
    /// Subtree and search revisions of the element and its ancestors.
    Search,
    /// Subtree revisions of the chain, search revisions of the ancestors only.
    AncestorSearch,
}

/// Read access to one retained node.
#[derive(Clone, Copy)]
pub struct NodeRef<'a> {
    tree: &'a HostTree,
    id: NodeId,
    node: &'a Node,
}

impl<'a> NodeRef<'a> {
    pub fn id(&self) -> NodeId {
        self.id
    }

    pub fn element_type(&self) -> &'a str {
        &self.node.element_type
    }

    pub fn parent(&self) -> Option<NodeId> {
        (self.node.parent != NONE).then_some(self.node.parent)
    }

    /// Children in order.
    pub fn children(&self) -> Children<'a> {
        Children {
            tree: self.tree,
            next: self.node.first_child,
        }
    }

    pub fn text(&self) -> Option<&'a str> {
        self.node.text.as_deref()
    }

    pub fn style(&self) -> Option<&'a SharedStyle> {
        self.node.style.as_ref()
    }

    pub fn has_listener(&self, event: &str) -> bool {
        self.node
            .listeners
            .as_ref()
            .is_some_and(|set| set.contains(event))
    }

    /// Listener event types with a handler, sorted.
    pub fn listeners(&self) -> impl Iterator<Item = &'a str> {
        self.node
            .listeners
            .iter()
            .flat_map(|set| set.iter().map(|name| &**name))
    }

    /// Whether the lazily created listener map is currently allocated.
    pub fn listener_map_allocated(&self) -> bool {
        self.node.listeners.is_some()
    }

    /// General custom properties (excludes the lifted autoFocus and testId), sorted by key.
    pub fn props(&self) -> impl Iterator<Item = (&'a str, &'a Value)> {
        self.node
            .props
            .iter()
            .flat_map(|props| props.iter().map(|(key, value)| (&**key, value)))
    }

    pub fn prop(&self, key: &str) -> Option<&'a Value> {
        self.node.prop(key)
    }

    /// Whether the lazily created property map is currently allocated.
    pub fn prop_map_allocated(&self) -> bool {
        self.node.props.is_some()
    }

    pub fn auto_focus(&self) -> bool {
        self.node.auto_focus
    }

    pub fn test_id(&self) -> Option<&'a str> {
        self.node.test_id.as_deref()
    }

    pub fn subtree_revision(&self) -> u32 {
        self.node.subtree_revision
    }

    pub fn search_revision(&self) -> u32 {
        self.node.search_revision
    }
}

/// Iterator over a node's children.
pub struct Children<'a> {
    tree: &'a HostTree,
    next: u64,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        if self.next == NONE {
            return None;
        }
        let id = self.next;
        self.next = self
            .tree
            .nodes
            .get(&id)
            .map_or(NONE, |node| node.next_sibling);
        Some(id)
    }
}

/// The retained host tree.
pub struct HostTree {
    nodes: HashMap<u64, Node, FxBuild>,
    root: Option<u64>,
    focus_dim: bool,
    revision: u32,
    searchable_revision: u64,
    focusable: BTreeSet<u64>,
    animated: BTreeSet<u64>,
    anchored_count: usize,
    element_types: HashSet<Arc<str>, FxBuild>,
    styles: StyleTable,
    /// Redraw flag accumulated by the batch being applied.
    redraw: bool,
    search_changed: bool,
    styles_touched: bool,
    destroy_stack: Vec<u64>,
    prop_reset: Option<Box<dyn FnMut(NodeId, &str) + Send>>,
    batches_applied: u64,
    redraw_commits: u64,
    idle_commits: u64,
    rejected_batches: u64,
    focus_visits: Cell<u64>,
    animation_visits: Cell<u64>,
}

impl Default for HostTree {
    fn default() -> Self {
        Self::new()
    }
}

impl HostTree {
    pub fn new() -> Self {
        Self::with_options(TreeOptions::default())
    }

    pub fn with_options(options: TreeOptions) -> Self {
        Self {
            nodes: HashMap::default(),
            root: None,
            focus_dim: true,
            revision: 0,
            searchable_revision: 0,
            focusable: BTreeSet::new(),
            animated: BTreeSet::new(),
            anchored_count: 0,
            element_types: HashSet::default(),
            styles: StyleTable::new(options.style_validator, options.forced_style_hash),
            redraw: false,
            search_changed: false,
            styles_touched: false,
            destroy_stack: Vec::new(),
            prop_reset: None,
            batches_applied: 0,
            redraw_commits: 0,
            idle_commits: 0,
            rejected_batches: 0,
            focus_visits: Cell::new(0),
            animation_visits: Cell::new(0),
        }
    }

    /// Registers the handler invoked when a custom property is removed, so native behavior
    /// driven by the property can be synchronized back to its default value
    /// (APP-006 R10). It is called with the element id and the property key.
    pub fn set_prop_reset_handler(&mut self, handler: impl FnMut(NodeId, &str) + Send + 'static) {
        self.prop_reset = Some(Box::new(handler));
    }

    // ------------------------------------------------------------------ queries

    pub fn node(&self, id: NodeId) -> Option<NodeRef<'_>> {
        self.nodes.get(&id).map(|node| NodeRef {
            tree: self,
            id,
            node,
        })
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Number of live elements.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Ids of all live elements in unspecified order.
    pub fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.keys().copied()
    }

    /// The root, or `None` when absent or destroyed.
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// Whether other focusable elements dim while a control has keyboard focus.
    pub fn keyboard_focus_dim(&self) -> bool {
        self.focus_dim
    }

    pub fn anchored_count(&self) -> usize {
        self.anchored_count
    }

    /// Ids of elements that need a keyboard focus handle, ascending.
    pub fn focusable_ids(&self) -> impl ExactSizeIterator<Item = NodeId> + '_ {
        self.focusable.iter().copied()
    }

    /// Ids of elements carrying a motion property, ascending.
    pub fn animated_ids(&self) -> impl ExactSizeIterator<Item = NodeId> + '_ {
        self.animated.iter().copied()
    }

    /// The tree-wide searchable-content revision (bumped once per batch that changed
    /// searchable text; render-only batches never bump it).
    pub fn searchable_revision(&self) -> u64 {
        self.searchable_revision
    }

    /// The most recent value of the revision counter (0 before any element exists).
    pub fn revision_counter(&self) -> u32 {
        self.revision
    }

    pub fn style_table_stats(&self) -> StyleTableStats {
        self.styles.stats()
    }

    pub fn stats(&self) -> TreeStats {
        TreeStats {
            batches_applied: self.batches_applied,
            redraw_commits: self.redraw_commits,
            idle_commits: self.idle_commits,
            rejected_batches: self.rejected_batches,
            searchable_revision: self.searchable_revision,
            live_elements: self.nodes.len(),
            style: self.styles.stats(),
            focus_visits: self.focus_visits.get(),
            animation_visits: self.animation_visits.get(),
        }
    }

    pub fn reset_visit_counters(&self) {
        self.focus_visits.set(0);
        self.animation_visits.set(0);
    }

    /// Calls `visit` for every focusable element in ascending id order. The per-frame
    /// focus resolution iterates only the focusable index.
    pub fn for_each_focusable(&self, mut visit: impl FnMut(NodeRef<'_>)) {
        for &id in &self.focusable {
            self.focus_visits.set(self.focus_visits.get() + 1);
            if let Some(node) = self.node(id) {
                visit(node);
            }
        }
    }

    /// Moves focus from `current` to the next (or previous) focusable element in ascending
    /// id order, wrapping around; `None` when there is no focusable element. Visits exactly
    /// the focusable index.
    pub fn move_focus(&self, current: Option<NodeId>, forward: bool) -> Option<NodeId> {
        let (mut first, mut last, mut before, mut after) = (None, None, None, None);
        for &id in &self.focusable {
            self.focus_visits.set(self.focus_visits.get() + 1);
            first.get_or_insert(id);
            last = Some(id);
            if let Some(current) = current {
                if id < current {
                    before = Some(id);
                } else if id > current && after.is_none() {
                    after = Some(id);
                }
            }
        }
        match (current, forward) {
            (None, true) => first,
            (None, false) => last,
            (Some(_), true) => after.or(first),
            (Some(_), false) => before.or(last),
        }
    }

    /// Calls `advance` with the element id and motion value of every animated element in
    /// ascending id order. Visits only the animated index.
    pub fn advance_animations(&self, mut advance: impl FnMut(NodeId, &Value)) {
        for &id in &self.animated {
            self.animation_visits.set(self.animation_visits.get() + 1);
            if let Some(motion) = self.nodes.get(&id).and_then(|node| node.prop(PROP_MOTION)) {
                advance(id, motion);
            }
        }
    }

    // ------------------------------------------------------------------ batches

    /// Applies one serialized batch atomically. Returns the destroyed identities.
    pub fn apply_batch(&mut self, batch: &str) -> Result<BatchResult, BatchError> {
        let prepared = self.prepare_batch(batch)?;
        Ok(self.commit_batch(prepared))
    }

    /// Phases 1 and 2 of a batch: decodes every operation and resolves every `setStyle`
    /// payload into a shared style. The only fallible phases; on failure nothing has changed.
    /// Pass the result to [`commit_batch`](Self::commit_batch).
    pub fn prepare_batch<'a>(&mut self, batch: &'a str) -> Result<PreparedBatch<'a>, BatchError> {
        let mut ops = match decode_batch(batch) {
            Ok(ops) => ops,
            Err((failure, error)) => {
                self.rejected_batches += 1;
                return Err(match failure {
                    Some(failure) => BatchError {
                        kind: BatchErrorKind::Decode,
                        op_index: Some(failure.index),
                        message: format!(
                            "failed to parse batch: Batch op {}: {}",
                            failure.index, failure.message
                        ),
                    },
                    None => BatchError {
                        kind: BatchErrorKind::Decode,
                        op_index: None,
                        message: format!("failed to parse batch: {error}"),
                    },
                });
            }
        };

        // Phase 2: resolve every setStyle payload into a shared style.
        let mut creates = 0usize;
        let mut failure = None;
        for (index, op) in ops.iter_mut().enumerate() {
            match op {
                Op::Create { .. } => creates += 1,
                Op::SetStyle { style, .. } => {
                    self.styles_touched = true;
                    if let StyleSource::Raw(raw) = style {
                        let raw = *raw;
                        match self.styles.intern(raw.get().as_bytes()) {
                            Ok(resolved) => *style = StyleSource::Resolved(resolved),
                            Err(cause) => {
                                failure = Some(BatchError {
                                    kind: BatchErrorKind::Style,
                                    op_index: Some(index),
                                    message: format!(
                                        "Batch op {index} setStyle parse error: {cause}"
                                    ),
                                });
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(error) = failure {
            // Styles interned by the failed batch must not persist.
            drop(ops);
            self.styles_touched = false;
            self.styles.sweep();
            self.rejected_batches += 1;
            return Err(error);
        }

        Ok(PreparedBatch { ops, creates })
    }

    /// Phase 3 of a batch: applies a prepared batch. It cannot fail.
    pub fn commit_batch(&mut self, prepared: PreparedBatch<'_>) -> BatchResult {
        let PreparedBatch { ops, creates } = prepared;
        self.nodes.reserve(creates);
        let mut destroyed = Vec::new();
        for op in ops {
            self.apply_op(op, &mut destroyed);
        }

        let redraw = std::mem::take(&mut self.redraw);
        let searchable_changed = std::mem::take(&mut self.search_changed);
        if searchable_changed {
            self.searchable_revision += 1;
        }
        self.batches_applied += 1;
        if redraw {
            self.redraw_commits += 1;
        } else {
            self.idle_commits += 1;
        }
        if std::mem::take(&mut self.styles_touched) && self.styles.sweep_due(self.nodes.len()) {
            self.styles.sweep();
        }
        BatchResult {
            destroyed,
            redraw,
            searchable_changed,
        }
    }

    /// As [`apply_batch`](Self::apply_batch) for transport bytes; the bytes must be UTF-8.
    pub fn apply_batch_bytes(&mut self, batch: &[u8]) -> Result<BatchResult, BatchError> {
        match std::str::from_utf8(batch) {
            Ok(text) => self.apply_batch(text),
            Err(error) => {
                self.rejected_batches += 1;
                Err(BatchError {
                    kind: BatchErrorKind::Decode,
                    op_index: None,
                    message: format!("failed to parse batch: {error}"),
                })
            }
        }
    }

    fn apply_op(&mut self, op: Op<'_>, destroyed: &mut Vec<NodeId>) {
        match op {
            Op::Create { id, element_type } => self.create(id, &element_type),
            Op::Destroy(id) => self.destroy(id, destroyed),
            Op::Append { parent, child } => self.reparent(parent, child, None),
            Op::InsertBefore {
                parent,
                child,
                before,
            } => self.reparent(parent, child, Some(before)),
            Op::SetStyle { id, style } => {
                if let StyleSource::Resolved(style) = style {
                    self.set_style(id, style);
                }
            }
            Op::SetText { id, text } => self.set_text(id, &text),
            Op::SetEventListener {
                id,
                event,
                has_handler,
            } => self.set_listener(id, &event, has_handler),
            Op::SetRoot(id) => {
                if self.nodes.contains_key(&id) && self.root != Some(id) {
                    self.root = Some(id);
                    self.redraw = true;
                }
            }
            Op::SetKeyboardFocusDim(enabled) => {
                if self.focus_dim != enabled {
                    self.focus_dim = enabled;
                    self.redraw = true;
                }
            }
            Op::SetCustomProp { id, key, value } => self.set_custom_prop(id, &key, *value),
        }
    }

    // --------------------------------------------------------------- revisions

    fn next_revision(&mut self) -> u32 {
        self.revision = self.revision.wrapping_add(1);
        if self.revision == 0 {
            self.revision = 1;
        }
        self.revision
    }

    /// Marks `start` and every ancestor with one fresh revision.
    fn mark(&mut self, start: u64, kind: Mark) {
        let revision = self.next_revision();
        if kind != Mark::Paint {
            self.search_changed = true;
        }
        let mut current = start;
        let mut first = true;
        while current != NONE {
            let Some(node) = self.nodes.get_mut(&current) else {
                break;
            };
            node.subtree_revision = revision;
            if kind == Mark::Search || (kind == Mark::AncestorSearch && !first) {
                node.search_revision = revision;
            }
            first = false;
            current = node.parent;
        }
    }

    // --------------------------------------------------------------- structure

    fn intern_type(&mut self, element_type: &str) -> Arc<str> {
        if let Some(known) = self.element_types.get(element_type) {
            return known.clone();
        }
        let interned: Arc<str> = Arc::from(element_type);
        self.element_types.insert(interned.clone());
        interned
    }

    fn create(&mut self, id: u64, element_type: &str) {
        if self.nodes.contains_key(&id) {
            // Unsupported usage: replace the record with a fresh empty one. The old record
            // leaves its parent and orphans its children; the identity keeps any root role.
            self.detach_for_replace(id);
        }
        let element_type = self.intern_type(element_type);
        if &*element_type == ANCHORED_TYPE {
            self.anchored_count += 1;
        }
        let revision = self.next_revision();
        let node = Node::new(element_type, revision);
        let needs_focus = node.needs_focus_handle();
        self.nodes.insert(id, node);
        if needs_focus {
            self.focusable.insert(id);
        }
        self.redraw = true;
    }

    fn detach_for_replace(&mut self, id: u64) {
        let old_parent = self.nodes.get(&id).map_or(NONE, |node| node.parent);
        self.unlink(id);
        if old_parent != NONE {
            self.mark(old_parent, Mark::Search);
        }
        let mut child = self.nodes.get(&id).map_or(NONE, |node| node.first_child);
        while child != NONE {
            let Some(node) = self.nodes.get_mut(&child) else {
                break;
            };
            node.parent = NONE;
            node.prev_sibling = NONE;
            child = std::mem::replace(&mut node.next_sibling, NONE);
        }
        if let Some(node) = self.nodes.remove(&id) {
            self.forget(id, &node);
        }
    }

    /// Drops a removed node from the derived indexes and counters.
    fn forget(&mut self, id: u64, node: &Node) {
        self.focusable.remove(&id);
        self.animated.remove(&id);
        if &*node.element_type == ANCHORED_TYPE {
            self.anchored_count -= 1;
        }
        if node.style.is_some() {
            self.styles_touched = true;
        }
    }

    /// Unlinks `child` from its parent's child list.
    fn unlink(&mut self, child: u64) {
        let Some(node) = self.nodes.get_mut(&child) else {
            return;
        };
        let (parent, prev, next) = (node.parent, node.prev_sibling, node.next_sibling);
        node.parent = NONE;
        node.prev_sibling = NONE;
        node.next_sibling = NONE;
        if parent == NONE {
            return;
        }
        if prev != NONE {
            if let Some(prev) = self.nodes.get_mut(&prev) {
                prev.next_sibling = next;
            }
        } else if let Some(parent) = self.nodes.get_mut(&parent) {
            parent.first_child = next;
        }
        if next != NONE {
            if let Some(next) = self.nodes.get_mut(&next) {
                next.prev_sibling = prev;
            }
        } else if let Some(parent) = self.nodes.get_mut(&parent) {
            parent.last_child = prev;
        }
    }

    fn is_self_or_ancestor(&self, candidate: u64, of: u64) -> bool {
        let mut current = of;
        while current != NONE {
            if current == candidate {
                return true;
            }
            current = self.nodes.get(&current).map_or(NONE, |node| node.parent);
        }
        false
    }

    /// appendChild (`before == None`) and insertBefore.
    fn reparent(&mut self, parent: u64, child: u64, before: Option<u64>) {
        let Some(child_node) = self.nodes.get(&child) else {
            return;
        };
        let old_parent = child_node.parent;
        if !self.nodes.contains_key(&parent) {
            // Missing parent: the child is only unlinked from its old parent.
            if old_parent != NONE {
                self.unlink(child);
                self.mark(old_parent, Mark::Search);
                self.redraw = true;
            }
            return;
        }
        // Operations that would make an element its own ancestor are unspecified; they are
        // ignored so the chain walks always terminate.
        if self.is_self_or_ancestor(child, parent) {
            return;
        }
        // The sibling anchor, valid only if it is a child of `parent` other than `child`.
        let anchor = before
            .filter(|&sibling| sibling != child)
            .filter(|sibling| {
                self.nodes
                    .get(sibling)
                    .is_some_and(|node| node.parent == parent)
            })
            .unwrap_or(NONE);
        // Re-placing a child where it already is changes nothing.
        if old_parent == parent && self.nodes.get(&child).map(|n| n.next_sibling) == Some(anchor) {
            return;
        }

        self.unlink(child);
        if old_parent != NONE {
            self.mark(old_parent, Mark::Search);
        }
        let (prev, next) = if anchor == NONE {
            (self.nodes.get(&parent).map_or(NONE, |n| n.last_child), NONE)
        } else {
            (
                self.nodes.get(&anchor).map_or(NONE, |n| n.prev_sibling),
                anchor,
            )
        };
        if let Some(node) = self.nodes.get_mut(&child) {
            node.parent = parent;
            node.prev_sibling = prev;
            node.next_sibling = next;
        }
        if prev != NONE {
            if let Some(node) = self.nodes.get_mut(&prev) {
                node.next_sibling = child;
            }
        } else if let Some(node) = self.nodes.get_mut(&parent) {
            node.first_child = child;
        }
        if next != NONE {
            if let Some(node) = self.nodes.get_mut(&next) {
                node.prev_sibling = child;
            }
        } else if let Some(node) = self.nodes.get_mut(&parent) {
            node.last_child = child;
        }
        self.mark(parent, Mark::Search);
        self.redraw = true;
    }

    fn destroy(&mut self, id: u64, destroyed: &mut Vec<NodeId>) {
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        let former_parent = node.parent;
        self.unlink(id);
        let mut stack = std::mem::take(&mut self.destroy_stack);
        stack.push(id);
        while let Some(current) = stack.pop() {
            let Some(node) = self.nodes.remove(&current) else {
                continue;
            };
            destroyed.push(current);
            // Children are pushed last to first so they pop in child order.
            let mut child = node.last_child;
            while child != NONE {
                stack.push(child);
                child = self.nodes.get(&child).map_or(NONE, |n| n.prev_sibling);
            }
            self.forget(current, &node);
            if self.root == Some(current) {
                self.root = None;
            }
        }
        self.destroy_stack = stack;
        if former_parent != NONE {
            self.mark(former_parent, Mark::Search);
        }
        self.redraw = true;
    }

    // ------------------------------------------------------- content and style

    fn set_style(&mut self, id: u64, style: SharedStyle) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        // Interning makes equivalent content one shared value, so pointer equality is
        // exact equivalence.
        if node
            .style
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &style))
        {
            return;
        }
        node.style = Some(style);
        self.styles_touched = true;
        self.mark(id, Mark::Paint);
        self.redraw = true;
    }

    fn set_text(&mut self, id: u64, text: &str) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        if node.text.as_deref() == Some(text) {
            return;
        }
        node.text = Some(text.into());
        self.mark(id, Mark::Search);
        self.redraw = true;
    }

    fn set_listener(&mut self, id: u64, event: &str, has_handler: bool) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        let changed = if has_handler {
            node.listeners
                .get_or_insert_with(Box::default)
                .insert(event.into())
        } else {
            match node.listeners.as_mut() {
                Some(set) => {
                    let removed = set.remove(event);
                    if set.is_empty() {
                        node.listeners = None;
                    }
                    removed
                }
                None => false,
            }
        };
        if changed {
            self.redraw = true;
            self.refresh_indexes(id);
        }
    }

    fn set_custom_prop(&mut self, id: u64, key: &str, value: Value) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        match key {
            PROP_AUTO_FOCUS => {
                let enabled = value.as_bool().unwrap_or(false);
                if node.auto_focus != enabled {
                    node.auto_focus = enabled;
                    self.redraw = true;
                }
                return;
            }
            PROP_TEST_ID => {
                let test_id = value.as_str();
                if node.test_id.as_deref() != test_id {
                    node.test_id = test_id.map(Into::into);
                    self.redraw = true;
                }
                return;
            }
            _ => {}
        }

        let had_highlight = node.prop(PROP_HIGHLIGHT).is_some();
        let removed = value.is_null();
        if removed {
            let Some(props) = node.props.as_mut() else {
                return;
            };
            if props.remove(key).is_none() {
                return;
            }
            if props.is_empty() {
                node.props = None;
            }
        } else {
            let props = node.props.get_or_insert_with(Box::default);
            match props.get_mut(key) {
                Some(existing) if content_eq(existing, &value) => return,
                Some(existing) => *existing = value,
                None => {
                    props.insert(key.into(), value);
                }
            }
        }
        let has_highlight = node.prop(PROP_HIGHLIGHT).is_some();

        if key == PROP_HIGHLIGHT && has_highlight != had_highlight {
            self.mark(id, Mark::AncestorSearch);
        } else {
            self.mark(id, Mark::Paint);
        }
        self.redraw = true;
        self.refresh_indexes(id);
        if removed {
            if let Some(handler) = self.prop_reset.as_mut() {
                handler(id, key);
            }
        }
    }

    /// Recomputes the focusable and animated index membership of one node.
    fn refresh_indexes(&mut self, id: u64) {
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        let (focusable, animated) = (node.needs_focus_handle(), node.is_animated());
        if focusable {
            self.focusable.insert(id);
        } else {
            self.focusable.remove(&id);
        }
        if animated {
            self.animated.insert(id);
        } else {
            self.animated.remove(&id);
        }
    }
}

/// Pre-order (document order) iterator over the live tree below the root.
pub struct DocumentOrder<'a> {
    tree: &'a HostTree,
    stack: Vec<u64>,
}

impl Iterator for DocumentOrder<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let id = self.stack.pop()?;
        let mut child = self
            .tree
            .nodes
            .get(&id)
            .map_or(NONE, |node| node.last_child);
        while child != NONE {
            self.stack.push(child);
            child = self
                .tree
                .nodes
                .get(&child)
                .map_or(NONE, |node| node.prev_sibling);
        }
        Some(id)
    }
}

impl HostTree {
    /// Ids of the elements reachable from the root, in pre-order (document order).
    pub fn document_order(&self) -> DocumentOrder<'_> {
        DocumentOrder {
            tree: self,
            stack: self.root.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_counter_wraps_without_returning_zero() {
        let mut tree = HostTree::new();
        assert_eq!(tree.next_revision(), 1);
        tree.revision = u32::MAX - 1;
        assert_eq!(tree.next_revision(), u32::MAX);
        assert_eq!(tree.next_revision(), 1);
        assert_eq!(tree.next_revision(), 2);
    }
}
