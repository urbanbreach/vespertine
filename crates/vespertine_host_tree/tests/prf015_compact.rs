//! PRF-015: compact retained state and no-op commit suppression.

mod common;
#[path = "support/mod.rs"]
mod support;

use std::fmt::Write;
use std::sync::Arc;

use common::{apply, apply_raw, fingerprint};
use serde_json::json;
use support::workload::{self, Workload};
use vespertine_host_tree::{HostTree, TreeOptions};

fn base_tree() -> HostTree {
    let mut tree = HostTree::new();
    apply(
        &mut tree,
        json!([
            ["createElement", 0, "div"],
            ["createElement", 1, "div"],
            ["createElement", 2, "text"],
            ["appendChild", 0, 1],
            ["appendChild", 1, 2],
            ["setRoot", 0],
            ["setText", 2, "hello"],
            ["setStyle", 1, {"width": 10, "margin": [1, 2], "box": {"a": 1, "b": 2}}],
            ["setEventListener", 1, "click", true],
            ["setCustomProp", 1, "data", {"k": [1, 2], "j": null}],
            ["setCustomProp", 1, "autoFocus", true],
            ["setCustomProp", 1, "testId", "one"],
        ]),
    );
    tree
}

// ---------------------------------------------------------------------- A1

#[test]
fn a1_equal_commit_causes_no_redraw_layout_search_or_revision_change() {
    let mut tree = base_tree();
    let snapshot = fingerprint(&tree);
    let stats = tree.stats();
    let equal_commits = [
        // The same operations again (apart from creation, which is not a resend).
        r#"[["appendChild",0,1],["setRoot",0],["setText",2,"hello"],["setEventListener",1,"click",true]]"#,
        // Equivalent style with different key order, equal attributes and properties.
        r#"[["setStyle",1,{"box":{"b":2,"a":1},"margin":[1,2],"width":10}],
            ["setCustomProp",1,"data",{"j":null,"k":[1,2]}],
            ["setCustomProp",1,"autoFocus",true],["setCustomProp",1,"testId","one"]]"#,
        // Removing what is absent, listener already absent, same focus dim.
        r#"[["setCustomProp",1,"absent",null],["setEventListener",1,"hover",false],["setKeyboardFocusDim",true],["destroyElement",404]]"#,
        "[]",
    ];
    for commit in equal_commits {
        let result = apply_raw(&mut tree, commit);
        assert!(!result.redraw, "{commit}");
        assert!(!result.searchable_changed, "{commit}");
        assert!(result.destroyed.is_empty());
    }
    assert_eq!(fingerprint(&tree), snapshot);
    let after = tree.stats();
    assert_eq!(after.redraw_commits, stats.redraw_commits);
    assert_eq!(after.searchable_revision, stats.searchable_revision);
    assert_eq!(
        after.idle_commits,
        stats.idle_commits + equal_commits.len() as u64
    );
}

#[test]
fn a1_effective_operations_among_equal_ones_invalidate_exactly_what_they_require() {
    let mut tree = base_tree();
    // One effective render-only operation: redraw, but no searchable change (R4).
    let searchable = tree.searchable_revision();
    let result = apply_raw(
        &mut tree,
        r#"[["setStyle",1,{"width":11}],["setText",2,"hello"],["setRoot",0]]"#,
    );
    assert!(result.redraw);
    assert!(!result.searchable_changed);
    assert_eq!(tree.searchable_revision(), searchable);
    // A text change bumps both.
    let result = apply_raw(
        &mut tree,
        r#"[["setStyle",1,{"width":11}],["setText",2,"hello!"]]"#,
    );
    assert!(result.redraw);
    assert!(result.searchable_changed);
    assert_eq!(tree.searchable_revision(), searchable + 1);
    // Focus dim and listener presence changes redraw without touching search.
    for commit in [
        r#"[["setKeyboardFocusDim",false]]"#,
        r#"[["setEventListener",1,"click",false]]"#,
        r#"[["setCustomProp",1,"testId","two"]]"#,
    ] {
        let result = apply_raw(&mut tree, commit);
        assert!(result.redraw, "{commit}");
        assert!(!result.searchable_changed, "{commit}");
    }
}

#[test]
fn a1_render_only_commits_never_bump_the_searchable_revision() {
    let mut tree = base_tree();
    let searchable = tree.searchable_revision();
    let node_search = tree.node(2).unwrap().search_revision();
    for k in 0..20 {
        let batch =
            json!([["setStyle", 2, {"width": k}], ["setCustomProp", 1, "paint", k]]).to_string();
        let result = apply_raw(&mut tree, &batch);
        assert!(result.redraw);
        assert!(!result.searchable_changed);
    }
    assert_eq!(tree.searchable_revision(), searchable);
    assert_eq!(tree.node(2).unwrap().search_revision(), node_search);
}

// ---------------------------------------------------------------------- A2

fn intern_count(styles: &[&str], options: TreeOptions) -> (usize, HostTree) {
    let mut tree = HostTree::with_options(options);
    let mut ops = Vec::new();
    for (k, style) in styles.iter().enumerate() {
        ops.push(format!(r#"["createElement",{k},"div"]"#));
        ops.push(format!(r#"["setStyle",{k},{style}]"#));
    }
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    (tree.style_table_stats().interned, tree)
}

#[test]
fn a2_styles_differing_only_in_key_order_intern_to_one_entry() {
    let (count, tree) = intern_count(
        &[
            r#"{"a":1,"b":{"x":1,"y":[1,2]},"c":"s"}"#,
            r#"{"c":"s","b":{"y":[1,2],"x":1},"a":1}"#,
            r#"  {"b":{"x":1,"y":[1,2]},"a":1,"c":"s"}  "#,
            r#"{"a":1.0,"b":{"x":1,"y":[1,2]},"c":"s"}"#,
        ],
        TreeOptions::default(),
    );
    assert_eq!(count, 1);
    let first = tree.node(0).unwrap().style().unwrap();
    for k in 1..4 {
        assert!(Arc::ptr_eq(first, tree.node(k).unwrap().style().unwrap()));
    }
}

#[test]
fn a2_styles_differing_in_value_array_order_or_missing_key_intern_to_two_entries() {
    let base = r#"{"a":1,"b":[1,2],"c":"s"}"#;
    for other in [
        r#"{"a":2,"b":[1,2],"c":"s"}"#,
        r#"{"a":1,"b":[2,1],"c":"s"}"#,
        r#"{"a":1,"b":[1,2]}"#,
        r#"{"a":1,"b":[1,2],"c":"s","d":null}"#,
        r#"{"a":"1","b":[1,2],"c":"s"}"#,
        r#"{"a":1,"b":[1,2,2],"c":"s"}"#,
    ] {
        let (count, tree) = intern_count(&[base, other], TreeOptions::default());
        assert_eq!(count, 2, "{other}");
        assert_ne!(tree.node(0).unwrap().style(), tree.node(1).unwrap().style());
    }
}

#[test]
fn a2_forced_hash_collision_keeps_distinct_contents_distinct_and_equal_contents_shared() {
    let styles = [
        r#"{"a":1,"b":2}"#,
        r#"{"a":2,"b":2}"#,
        r#"{"b":2,"a":1}"#,
        r#"{"a":1}"#,
        r#"{"a":2,"b":2}"#,
        r#"{"a":[1,2]}"#,
        r#"{"a":[2,1]}"#,
    ];
    let (count, tree) = intern_count(
        &styles,
        TreeOptions {
            forced_style_hash: Some(0),
            ..TreeOptions::default()
        },
    );
    assert_eq!(count, 5);
    let style = |k| tree.node(k).unwrap().style().unwrap().clone();
    assert!(Arc::ptr_eq(&style(0), &style(2)));
    assert!(Arc::ptr_eq(&style(1), &style(4)));
    for (x, y) in [(0, 1), (0, 3), (0, 5), (1, 3), (5, 6), (3, 6)] {
        assert!(!Arc::ptr_eq(&style(x), &style(y)), "{x} vs {y}");
    }
    assert_eq!(*style(3), json!({"a": 1}));
    assert_eq!(*style(6), json!({"a": [2, 1]}));
    // The table agrees with the unforced build.
    let (unforced, _) = intern_count(&styles, TreeOptions::default());
    assert_eq!(unforced, count);
    assert_eq!(tree.style_table_stats().interned, 5);
}

// ---------------------------------------------------------------------- A3

#[test]
fn a3_nodes_without_listeners_or_properties_hold_no_maps() {
    let mut tree = HostTree::new();
    apply(
        &mut tree,
        json!([["createElement", 1, "div"], ["setStyle", 1, {"a": 1}], ["setText", 1, "t"]]),
    );
    let node = tree.node(1).unwrap();
    assert!(!node.listener_map_allocated());
    assert!(!node.prop_map_allocated());

    // Adding then removing the last listener releases the map.
    apply(
        &mut tree,
        json!([
            ["setEventListener", 1, "click", true],
            ["setEventListener", 1, "keyDown", true]
        ]),
    );
    assert!(tree.node(1).unwrap().listener_map_allocated());
    apply(&mut tree, json!([["setEventListener", 1, "click", false]]));
    assert!(tree.node(1).unwrap().listener_map_allocated());
    apply(
        &mut tree,
        json!([["setEventListener", 1, "keyDown", false]]),
    );
    assert!(!tree.node(1).unwrap().listener_map_allocated());
    // Removing a listener that was never present does not allocate.
    apply(&mut tree, json!([["setEventListener", 1, "hover", false]]));
    assert!(!tree.node(1).unwrap().listener_map_allocated());

    // Properties likewise; the lifted autoFocus and testId never need the map.
    apply(
        &mut tree,
        json!([
            ["setCustomProp", 1, "autoFocus", true],
            ["setCustomProp", 1, "testId", "x"],
            ["setCustomProp", 1, "gone", null]
        ]),
    );
    assert!(!tree.node(1).unwrap().prop_map_allocated());
    apply(
        &mut tree,
        json!([["setCustomProp", 1, "p", 1], ["setCustomProp", 1, "q", 2]]),
    );
    assert!(tree.node(1).unwrap().prop_map_allocated());
    apply(&mut tree, json!([["setCustomProp", 1, "p", null]]));
    assert!(tree.node(1).unwrap().prop_map_allocated());
    apply(&mut tree, json!([["setCustomProp", 1, "q", null]]));
    assert!(!tree.node(1).unwrap().prop_map_allocated());
}

// ---------------------------------------------------------------------- A5

fn restyle_all(tree: &mut HostTree, from: usize, to: usize, tag: &str) {
    let ops: Vec<String> = (from..to)
        .map(|k| {
            format!(r#"["createElement",{k},"div"],["setStyle",{k},{{"tag":"{tag}","n":{k}}}]"#)
        })
        .collect();
    apply_raw(tree, &format!("[{}]", ops.join(",")));
}

#[test]
fn a5_destroying_nodes_crosses_the_sweep_trigger_and_the_next_threshold_follows_the_remaining_count()
 {
    let mut tree = HostTree::new();
    // 130 live elements with distinct styles: 130 >= max(2 * 0, 64), a sweep removes nothing.
    restyle_all(&mut tree, 0, 130, "a");
    let stats = tree.style_table_stats();
    assert_eq!(
        (stats.interned, stats.sweeps, stats.after_last_sweep),
        (130, 1, 130)
    );

    // Destroy 100: 30 live, 130 > max(2 * 30, 64) = 64. The sweep reclaims the 100
    // unreferenced styles and keeps the 30 referenced ones.
    let ops: Vec<String> = (30..130)
        .map(|k| format!(r#"["destroyElement",{k}]"#))
        .collect();
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    let stats = tree.style_table_stats();
    assert_eq!(
        (stats.interned, stats.sweeps, stats.after_last_sweep),
        (30, 2, 30)
    );
    for k in 0..30 {
        assert_eq!(
            tree.node(k).unwrap().style().map(|s| (**s).clone()),
            Some(json!({"tag": "a", "n": k}))
        );
    }

    // The next threshold is max(2 * 30, 64) = 64 interned entries: 63 do not sweep ...
    restyle_all(&mut tree, 1_000, 1_033, "b");
    let stats = tree.style_table_stats();
    assert_eq!((stats.interned, stats.sweeps), (63, 2));
    // ... the 64th does (all referenced, so nothing is removed) and moves the
    // threshold to 2 * 64 = 128.
    restyle_all(&mut tree, 1_033, 1_034, "b");
    let stats = tree.style_table_stats();
    assert_eq!(
        (stats.interned, stats.sweeps, stats.after_last_sweep),
        (64, 3, 64)
    );
    restyle_all(&mut tree, 1_034, 1_097, "b");
    let stats = tree.style_table_stats();
    assert_eq!((stats.interned, stats.sweeps), (127, 3));
    restyle_all(&mut tree, 1_097, 1_098, "b");
    let stats = tree.style_table_stats();
    assert_eq!(
        (stats.interned, stats.sweeps, stats.after_last_sweep),
        (128, 4, 128)
    );
}

#[test]
fn a5_the_table_never_keeps_a_style_alive_by_itself() {
    let mut tree = HostTree::new();
    restyle_all(&mut tree, 0, 70, "x");
    let probe = Arc::downgrade(tree.node(5).unwrap().style().unwrap());
    assert!(probe.upgrade().is_some());
    let ops: Vec<String> = (0..70)
        .map(|k| format!(r#"["destroyElement",{k}]"#))
        .collect();
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    // Every node is gone: the sweep removed every entry whose only holder was the table.
    assert_eq!(tree.len(), 0);
    assert_eq!(tree.style_table_stats().interned, 0);
    assert!(probe.upgrade().is_none());
}

// ---------------------------------------------------------------------- A6

#[test]
fn a6_focus_and_animation_work_visits_only_the_indexes() {
    const HOSTS: usize = 100_000;
    let mut batch = String::from(r#"[["createElement",0,"div"],["setRoot",0]"#);
    let mut focusable = Vec::new();
    let mut animated = Vec::new();
    for i in 1..HOSTS {
        let element_type = if i % 2_000 == 0 { "input" } else { "div" };
        write!(
            batch,
            r#",["createElement",{i},"{element_type}"],["appendChild",0,{i}]"#
        )
        .unwrap();
        if element_type == "input" {
            focusable.push(i as u64);
        }
        if i % 3_300 == 0 {
            write!(batch, r#",["setCustomProp",{i},"motion",{{"to":{i}}}]"#).unwrap();
            animated.push(i as u64);
        }
        if i % 7_001 == 0 {
            write!(batch, r#",["setEventListener",{i},"keyDown",true]"#).unwrap();
            focusable.push(i as u64);
        }
    }
    batch.push(']');
    focusable.sort_unstable();
    let mut tree = HostTree::new();
    apply_raw(&mut tree, &batch);
    assert_eq!(tree.len(), HOSTS);
    assert_eq!(tree.focusable_ids().collect::<Vec<_>>(), focusable);
    assert_eq!(tree.animated_ids().collect::<Vec<_>>(), animated);
    assert!(!focusable.is_empty() && !animated.is_empty());

    tree.reset_visit_counters();
    let mut seen = Vec::new();
    tree.for_each_focusable(|node| seen.push(node.id()));
    assert_eq!(seen, focusable);
    assert_eq!(tree.stats().focus_visits, focusable.len() as u64);

    // Moving focus visits exactly the focusable index, forward, backward and wrapping.
    tree.reset_visit_counters();
    assert_eq!(tree.move_focus(None, true), focusable.first().copied());
    assert_eq!(tree.move_focus(None, false), focusable.last().copied());
    assert_eq!(
        tree.move_focus(focusable.last().copied(), true),
        focusable.first().copied()
    );
    assert_eq!(
        tree.move_focus(focusable.first().copied(), false),
        focusable.last().copied()
    );
    assert_eq!(
        tree.move_focus(Some(focusable[1]), true),
        Some(focusable[2])
    );
    assert_eq!(
        tree.move_focus(Some(focusable[1]), false),
        Some(focusable[0])
    );
    assert_eq!(tree.stats().focus_visits, 6 * focusable.len() as u64);
    assert!((tree.stats().focus_visits as usize) < HOSTS);

    tree.reset_visit_counters();
    let mut advanced = Vec::new();
    tree.advance_animations(|id, motion| {
        assert_eq!(*motion, json!({"to": id}));
        advanced.push(id);
    });
    assert_eq!(advanced, animated);
    assert_eq!(tree.stats().animation_visits, animated.len() as u64);
    assert!((tree.stats().animation_visits as usize) < HOSTS / 100);

    // The index follows changes: destroying a focusable host shrinks the work.
    apply(&mut tree, json!([["destroyElement", focusable[0]]]));
    tree.reset_visit_counters();
    tree.for_each_focusable(|_| {});
    assert_eq!(tree.stats().focus_visits, focusable.len() as u64 - 1);
}

// ---------------------------------------------------------------------- A7 support

#[test]
fn a7_workload_parameters_match_the_specification() {
    let full = Workload::new(workload::FULL_HOSTS);
    assert_eq!(full.text_hosts(), 20_000);
    assert_eq!(full.mount_logical_ops(), 220_000);
    assert_eq!(full.mount_wire_ops(), 320_000);
    for j in 0..8 {
        let forward = full.style_payload(j, false);
        let reverse = full.style_payload(j, true);
        assert!(
            (1_300..=1_500).contains(&forward.len()),
            "S_{j}: {} bytes",
            forward.len()
        );
        assert_eq!(forward.len(), reverse.len());
        assert_ne!(
            forward, reverse,
            "the two serializations are not byte-identical"
        );
        let (a, b): (serde_json::Value, serde_json::Value) = (
            serde_json::from_str(forward).unwrap(),
            serde_json::from_str(reverse).unwrap(),
        );
        assert!(vespertine_host_tree::content_eq(&a, &b));
        let properties = a.as_object().unwrap().len();
        assert!((38..=44).contains(&properties), "{properties} properties");
    }
    assert_eq!(full.destroy_targets(101).len(), 9_000);
    assert_eq!(full.destroy_targets(110).len(), 9_000);
    assert_eq!(full.resend_batch(1).matches("setStyle").count(), 1_000);
    // Frame 100 is clamped to the last row (host 99,999).
    assert_eq!(full.resend_batch(100).matches("setStyle").count(), 999);
}

#[test]
fn a7_workload_builds_the_same_tree_as_the_reference_build_and_resends_stay_idle() {
    let hosts = 5_000;
    let report =
        workload::verify_against_reference(hosts).expect("tree equals the reference build");
    assert_eq!(report.resend_redraw_commits, 0);
    assert_eq!(report.destroyed, 4_500);
    assert_eq!(report.final_live_elements, 500);
    // 8 row styles and the container style were interned; the floor of 64 means no sweep
    // runs for such a small table.
    assert_eq!(report.final_interned, 9);
}
