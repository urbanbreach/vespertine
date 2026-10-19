#![allow(dead_code)]

use std::fmt::Write;

use serde_json::Value;
use vespertine_host_tree::{BatchError, BatchResult, HostTree};

pub fn apply(tree: &mut HostTree, ops: Value) -> BatchResult {
    tree.apply_batch(&ops.to_string())
        .unwrap_or_else(|error| panic!("batch should apply: {error}"))
}

pub fn apply_raw(tree: &mut HostTree, batch: &str) -> BatchResult {
    tree.apply_batch(batch)
        .unwrap_or_else(|error| panic!("batch should apply: {error}"))
}

pub fn apply_err(tree: &mut HostTree, batch: &str) -> BatchError {
    match tree.apply_batch(batch) {
        Ok(_) => panic!("batch should be rejected: {batch}"),
        Err(error) => error,
    }
}

pub fn children(tree: &HostTree, id: u64) -> Vec<u64> {
    tree.node(id).expect("node exists").children().collect()
}

/// Everything observable about the tree, including revisions, as one string.
pub fn fingerprint(tree: &HostTree) -> String {
    let mut ids: Vec<u64> = tree.node_ids().collect();
    ids.sort_unstable();
    let mut out = format!(
        "root={:?} dim={} len={} rev={} searchable={} focusable={:?} animated={:?} anchored={}\n",
        tree.root(),
        tree.keyboard_focus_dim(),
        tree.len(),
        tree.revision_counter(),
        tree.searchable_revision(),
        tree.focusable_ids().collect::<Vec<_>>(),
        tree.animated_ids().collect::<Vec<_>>(),
        tree.anchored_count(),
    );
    for id in ids {
        let node = tree.node(id).unwrap();
        let _ = writeln!(
            out,
            "[{id} {} parent={:?} children={:?} text={:?} style={:?} listeners={:?} props={:?} \
             autoFocus={} testId={:?} sub={} search={}]",
            node.element_type(),
            node.parent(),
            node.children().collect::<Vec<_>>(),
            node.text(),
            node.style().map(|s| s.to_string()),
            node.listeners().collect::<Vec<_>>(),
            node.props()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>(),
            node.auto_focus(),
            node.test_id(),
            node.subtree_revision(),
            node.search_revision(),
        );
    }
    out
}
