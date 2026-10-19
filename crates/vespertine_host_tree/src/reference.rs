//! A deliberately straightforward reference implementation of the batch contract, used by
//! tests and the large-mount example to prove that the compact implementation builds an
//! identical tree (PRF-015 A4, A7).
//!
//! It decodes the whole batch into a general-purpose [`serde_json::Value`] tree, parses a
//! separate style value per node (no sharing), keeps plain child vectors and eager
//! property/listener collections, and scans linearly. It is intentionally slow and large.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;

use crate::style::content_eq;
use crate::{HostTree, MAX_ID};

struct RefNode {
    element_type: String,
    parent: Option<u64>,
    children: Vec<u64>,
    text: Option<String>,
    style: Option<Value>,
    listeners: BTreeSet<String>,
    props: BTreeMap<String, Value>,
    auto_focus: bool,
    test_id: Option<String>,
}

enum RefOp {
    Create(u64, String),
    Destroy(u64),
    Append(u64, u64),
    InsertBefore(u64, u64, u64),
    SetStyle(u64, Value),
    SetText(u64, String),
    SetEventListener(u64, String, bool),
    SetRoot(u64),
    SetKeyboardFocusDim(bool),
    SetCustomProp(u64, String, Value),
}

/// The reference tree.
pub struct ReferenceTree {
    nodes: HashMap<u64, RefNode>,
    root: Option<u64>,
    focus_dim: bool,
}

impl Default for ReferenceTree {
    fn default() -> Self {
        Self::new()
    }
}

fn id_at(items: &[Value], index: usize) -> Result<u64, String> {
    let value = items.get(index).ok_or("missing argument")?;
    let Value::Number(number) = value else {
        return Err("identifier is not a number".into());
    };
    if let Some(id) = number.as_u64() {
        return (id <= MAX_ID)
            .then_some(id)
            .ok_or_else(|| "identifier too large".into());
    }
    match number.as_f64() {
        Some(f) if f.is_finite() && f.fract() == 0.0 && (0.0..=MAX_ID as f64).contains(&f) => {
            Ok(f as u64)
        }
        _ => Err("invalid identifier".into()),
    }
}

fn str_at(items: &[Value], index: usize) -> Result<String, String> {
    match items.get(index) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err("argument is not a string".into()),
        None => Err("missing argument".into()),
    }
}

fn bool_at(items: &[Value], index: usize) -> Result<bool, String> {
    match items.get(index) {
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err("argument is not a boolean".into()),
        None => Err("missing argument".into()),
    }
}

fn decode_op(item: &mut Value) -> Result<RefOp, String> {
    let items = item.as_array_mut().ok_or("operation is not an array")?;
    let name = match items.first() {
        Some(Value::String(name)) => name.clone(),
        _ => return Err("operation name is not a string".into()),
    };
    Ok(match name.as_str() {
        "createElement" => RefOp::Create(id_at(items, 1)?, str_at(items, 2)?),
        "destroyElement" => RefOp::Destroy(id_at(items, 1)?),
        "appendChild" => RefOp::Append(id_at(items, 1)?, id_at(items, 2)?),
        "insertBefore" => RefOp::InsertBefore(id_at(items, 1)?, id_at(items, 2)?, id_at(items, 3)?),
        "setStyle" => {
            let id = id_at(items, 1)?;
            let style = items.get_mut(2).ok_or("missing argument")?;
            if !style.is_object() {
                return Err("style must be a JSON object".into());
            }
            RefOp::SetStyle(id, style.take())
        }
        "setText" => RefOp::SetText(id_at(items, 1)?, str_at(items, 2)?),
        "setEventListener" => {
            RefOp::SetEventListener(id_at(items, 1)?, str_at(items, 2)?, bool_at(items, 3)?)
        }
        "setRoot" => RefOp::SetRoot(id_at(items, 1)?),
        "setKeyboardFocusDim" => RefOp::SetKeyboardFocusDim(bool_at(items, 1)?),
        "setCustomProp" => {
            let id = id_at(items, 1)?;
            let key = str_at(items, 2)?;
            let value = items.get_mut(3).ok_or("missing argument")?.take();
            RefOp::SetCustomProp(id, key, value)
        }
        other => return Err(format!("unknown operation {other:?}")),
    })
}

impl ReferenceTree {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            root: None,
            focus_dim: true,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Applies a batch atomically and returns the destroyed identities.
    pub fn apply_batch(&mut self, batch: &str) -> Result<Vec<u64>, String> {
        let mut value: Value = serde_json::from_str(batch).map_err(|e| e.to_string())?;
        let items = value.as_array_mut().ok_or("batch is not an array")?;
        let mut ops = Vec::with_capacity(items.len());
        for (index, item) in items.iter_mut().enumerate() {
            ops.push(decode_op(item).map_err(|e| format!("Batch op {index}: {e}"))?);
        }
        let mut destroyed = Vec::new();
        for op in ops {
            self.apply(op, &mut destroyed);
        }
        Ok(destroyed)
    }

    fn unlink(&mut self, child: u64) {
        let parent = self.nodes.get_mut(&child).and_then(|n| n.parent.take());
        if let Some(parent) = parent.and_then(|p| self.nodes.get_mut(&p)) {
            parent.children.retain(|&c| c != child);
        }
    }

    fn apply(&mut self, op: RefOp, destroyed: &mut Vec<u64>) {
        match op {
            RefOp::Create(id, element_type) => {
                if self.nodes.contains_key(&id) {
                    self.unlink(id);
                    let old = self.nodes.remove(&id).expect("checked");
                    for child in old.children {
                        if let Some(node) = self.nodes.get_mut(&child) {
                            node.parent = None;
                        }
                    }
                }
                self.nodes.insert(
                    id,
                    RefNode {
                        element_type,
                        parent: None,
                        children: Vec::new(),
                        text: None,
                        style: None,
                        listeners: BTreeSet::new(),
                        props: BTreeMap::new(),
                        auto_focus: false,
                        test_id: None,
                    },
                );
            }
            RefOp::Destroy(id) => {
                if !self.nodes.contains_key(&id) {
                    return;
                }
                self.unlink(id);
                let mut order = Vec::new();
                let mut stack = vec![id];
                while let Some(current) = stack.pop() {
                    order.push(current);
                    if let Some(node) = self.nodes.get(&current) {
                        stack.extend(node.children.iter().rev().copied());
                    }
                }
                for current in order {
                    self.nodes.remove(&current);
                    if self.root == Some(current) {
                        self.root = None;
                    }
                    destroyed.push(current);
                }
            }
            RefOp::Append(parent, child) => self.reparent(parent, child, None),
            RefOp::InsertBefore(parent, child, before) => {
                self.reparent(parent, child, Some(before))
            }
            RefOp::SetStyle(id, style) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.style = Some(style);
                }
            }
            RefOp::SetText(id, text) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.text = Some(text);
                }
            }
            RefOp::SetEventListener(id, event, has) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    if has {
                        node.listeners.insert(event);
                    } else {
                        node.listeners.remove(&event);
                    }
                }
            }
            RefOp::SetRoot(id) => {
                if self.nodes.contains_key(&id) {
                    self.root = Some(id);
                }
            }
            RefOp::SetKeyboardFocusDim(enabled) => self.focus_dim = enabled,
            RefOp::SetCustomProp(id, key, value) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    match key.as_str() {
                        "autoFocus" => node.auto_focus = value.as_bool().unwrap_or(false),
                        "testId" => node.test_id = value.as_str().map(str::to_owned),
                        _ if value.is_null() => {
                            node.props.remove(&key);
                        }
                        _ => {
                            node.props.insert(key, value);
                        }
                    }
                }
            }
        }
    }

    fn reparent(&mut self, parent: u64, child: u64, before: Option<u64>) {
        if !self.nodes.contains_key(&child) {
            return;
        }
        if !self.nodes.contains_key(&parent) {
            self.unlink(child);
            return;
        }
        // Never make an element its own ancestor.
        let mut cursor = Some(parent);
        while let Some(current) = cursor {
            if current == child {
                return;
            }
            cursor = self.nodes.get(&current).and_then(|n| n.parent);
        }
        self.unlink(child);
        let parent_node = self.nodes.get_mut(&parent).expect("checked");
        let position = before
            .and_then(|sibling| parent_node.children.iter().position(|&c| c == sibling))
            .unwrap_or(parent_node.children.len());
        parent_node.children.insert(position, child);
        self.nodes.get_mut(&child).expect("checked").parent = Some(parent);
    }

    /// Checks that `tree` is identical to this reference build: same root, focus-dim flag,
    /// nodes, types, parent links, child order, text, style content, listeners, properties,
    /// autoFocus, testId, and independently recomputed focusable/animated/anchored indexes.
    pub fn check_equivalent(&self, tree: &HostTree) -> Result<(), String> {
        if tree.len() != self.nodes.len() {
            return Err(format!("node count {} != {}", tree.len(), self.nodes.len()));
        }
        if tree.root() != self.root {
            return Err(format!("root {:?} != {:?}", tree.root(), self.root));
        }
        if tree.keyboard_focus_dim() != self.focus_dim {
            return Err("keyboard focus dim differs".into());
        }
        let mut focusable = BTreeSet::new();
        let mut animated = BTreeSet::new();
        let mut anchored = 0;
        for (&id, expected) in &self.nodes {
            let node = tree.node(id).ok_or_else(|| format!("node {id} missing"))?;
            let fail = |what: &str| Err(format!("node {id}: {what} differs"));
            if node.element_type() != expected.element_type {
                return fail("type");
            }
            if node.parent() != expected.parent {
                return fail("parent");
            }
            if !node.children().eq(expected.children.iter().copied()) {
                return fail("children");
            }
            if node.text() != expected.text.as_deref() {
                return fail("text");
            }
            match (node.style(), &expected.style) {
                (None, None) => {}
                (Some(actual), Some(expected)) if content_eq(actual, expected) => {}
                _ => return fail("style"),
            }
            if !node
                .listeners()
                .eq(expected.listeners.iter().map(String::as_str))
            {
                return fail("listeners");
            }
            let props: Vec<_> = node.props().collect();
            if props.len() != expected.props.len()
                || !props
                    .iter()
                    .all(|(k, v)| expected.props.get(*k).is_some_and(|e| content_eq(v, e)))
            {
                return fail("props");
            }
            if node.auto_focus() != expected.auto_focus
                || node.test_id() != expected.test_id.as_deref()
            {
                return fail("autoFocus/testId");
            }
            if expected.element_type == "anchored" {
                anchored += 1;
            }
            let tab_index = matches!(
                expected.props.get("tabIndex"),
                Some(Value::Number(n)) if n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0)
            );
            let key_listener = ["keyDown", "keyUp", "focus", "blur"]
                .iter()
                .any(|name| expected.listeners.contains(*name));
            if matches!(expected.element_type.as_str(), "input" | "textarea")
                || tab_index
                || key_listener
            {
                focusable.insert(id);
            }
            if expected.props.contains_key("motion") {
                animated.insert(id);
            }
        }
        if !tree.focusable_ids().eq(focusable) {
            return Err("focusable index differs".into());
        }
        if !tree.animated_ids().eq(animated) {
            return Err("animated index differs".into());
        }
        if tree.anchored_count() != anchored {
            return Err("anchored count differs".into());
        }
        Ok(())
    }
}
