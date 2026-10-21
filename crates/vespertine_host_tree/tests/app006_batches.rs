//! APP-006 acceptance: atomic retained host-tree mutations.

mod common;

use std::sync::Arc;

use common::{apply, apply_err, apply_raw, children, fingerprint};
use serde_json::{Value, json};
use vespertine_host_tree::{BatchErrorKind, HostTree, TreeOptions};

fn three_level_tree() -> HostTree {
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
        ]),
    );
    tree
}

// ---------------------------------------------------------------------- A1

#[test]
fn a1_create_and_append_builds_ordered_tree() {
    let mut tree = HostTree::new();
    assert!(tree.is_empty());
    assert_eq!(tree.root(), None);
    assert!(tree.keyboard_focus_dim());
    let result = apply(
        &mut tree,
        json!([
            ["createElement", 10, "div"],
            ["createElement", 11, "span"],
            ["createElement", 12, "span"],
            ["appendChild", 10, 11],
            ["appendChild", 10, 12],
            ["setRoot", 10],
        ]),
    );
    assert!(result.destroyed.is_empty());
    assert!(result.redraw);
    assert_eq!(tree.root(), Some(10));
    assert_eq!(children(&tree, 10), [11, 12]);
    let node = tree.node(11).unwrap();
    assert_eq!(node.element_type(), "span");
    assert_eq!(node.parent(), Some(10));
    assert_eq!(node.text(), None);
    assert!(node.style().is_none());
    assert_eq!(node.listeners().count(), 0);
    assert_eq!(node.props().count(), 0);
    assert!(children(&tree, 11).is_empty());
    assert_eq!(tree.node(10).unwrap().parent(), None);
    // An empty batch is valid and idle.
    let result = apply_raw(&mut tree, "[]");
    assert!(!result.redraw);
}

#[test]
fn a1_reparent_moves_child_out_of_old_parent() {
    let mut tree = HostTree::new();
    apply(
        &mut tree,
        json!([
            ["createElement", 1, "div"],
            ["createElement", 2, "div"],
            ["createElement", 3, "div"],
            ["appendChild", 1, 3],
        ]),
    );
    assert_eq!(children(&tree, 1), [3]);
    apply(&mut tree, json!([["appendChild", 2, 3]]));
    assert!(children(&tree, 1).is_empty());
    assert_eq!(children(&tree, 2), [3]);
    assert_eq!(tree.node(3).unwrap().parent(), Some(2));
    // Reorder inside one parent: appending an earlier child moves it to the end, once.
    apply(
        &mut tree,
        json!([
            ["createElement", 4, "div"],
            ["appendChild", 2, 4],
            ["appendChild", 2, 3],
        ]),
    );
    assert_eq!(children(&tree, 2), [4, 3]);
    // Missing parent: the child is only unlinked.
    apply(&mut tree, json!([["appendChild", 999, 3]]));
    assert_eq!(children(&tree, 2), [4]);
    assert_eq!(tree.node(3).unwrap().parent(), None);
}

#[test]
fn a1_insert_before_orders_and_missing_sibling_appends() {
    let mut tree = HostTree::new();
    apply(
        &mut tree,
        json!([
            ["createElement", 1, "div"],
            ["createElement", 2, "div"],
            ["createElement", 3, "div"],
            ["createElement", 4, "div"],
            ["createElement", 5, "div"],
            ["appendChild", 1, 2],
            ["appendChild", 1, 3],
            ["insertBefore", 1, 4, 3],
        ]),
    );
    assert_eq!(children(&tree, 1), [2, 4, 3]);
    // Sibling that does not exist: appended at the end.
    apply(&mut tree, json!([["insertBefore", 1, 5, 12345]]));
    assert_eq!(children(&tree, 1), [2, 4, 3, 5]);
    // Reorder within the same parent.
    apply(&mut tree, json!([["insertBefore", 1, 5, 2]]));
    assert_eq!(children(&tree, 1), [5, 2, 4, 3]);
    // Sibling that is a child of another parent: appended.
    apply(
        &mut tree,
        json!([
            ["createElement", 6, "div"],
            ["createElement", 7, "div"],
            ["appendChild", 6, 7],
            ["insertBefore", 1, 6, 7]
        ]),
    );
    assert_eq!(children(&tree, 1), [5, 2, 4, 3, 6]);
    // Moving between parents leaves the old list.
    apply(&mut tree, json!([["insertBefore", 7, 2, 12345]]));
    assert_eq!(children(&tree, 1), [5, 4, 3, 6]);
    assert_eq!(children(&tree, 7), [2]);
    // Missing child: nothing changes and no dangling identifier appears.
    let before = fingerprint(&tree);
    apply(
        &mut tree,
        json!([["insertBefore", 1, 555, 5], ["appendChild", 1, 556]]),
    );
    assert_eq!(fingerprint(&tree), before);
}

#[test]
fn a1_destroy_subtree_returns_preorder_and_clears_root() {
    let mut tree = HostTree::new();
    apply(
        &mut tree,
        json!([
            ["createElement", 1, "div"],
            ["createElement", 2, "div"],
            ["createElement", 3, "div"],
            ["createElement", 4, "div"],
            ["createElement", 5, "div"],
            ["createElement", 6, "div"],
            ["appendChild", 1, 2],
            ["appendChild", 1, 5],
            ["appendChild", 2, 3],
            ["appendChild", 2, 4],
            ["appendChild", 5, 6],
            ["setRoot", 1],
        ]),
    );
    let result = apply(&mut tree, json!([["destroyElement", 2]]));
    assert_eq!(result.destroyed, [2, 3, 4]);
    assert_eq!(children(&tree, 1), [5]);
    assert_eq!(tree.len(), 3);
    assert_eq!(tree.root(), Some(1));
    // The batch returns the concatenation of its destroy operations' lists.
    let result = apply(
        &mut tree,
        json!([
            ["destroyElement", 6],
            ["destroyElement", 1],
            ["destroyElement", 1]
        ]),
    );
    assert_eq!(result.destroyed, [6, 1, 5]);
    assert_eq!(tree.root(), None);
    assert!(tree.is_empty());
}

#[test]
fn a1_destroy_missing_id_returns_nothing() {
    let mut tree = three_level_tree();
    let before = fingerprint(&tree);
    let result = apply(&mut tree, json!([["destroyElement", 77]]));
    assert!(result.destroyed.is_empty());
    assert!(!result.redraw);
    assert_eq!(fingerprint(&tree), before);
}

#[test]
fn a1_repeated_equivalent_commits_leave_dirty_flag_clear() {
    let mut tree = three_level_tree();
    let commit = json!([
        ["setStyle", 1, {"width": 10, "height": 20}],
        ["setText", 2, "hello"],
        ["setEventListener", 2, "click", true],
        ["setCustomProp", 2, "data", {"a": [1, 2]}],
        ["setCustomProp", 2, "autoFocus", true],
        ["setCustomProp", 2, "testId", "t"],
        ["setRoot", 0],
        ["setKeyboardFocusDim", true],
        ["appendChild", 0, 1],
    ])
    .to_string();
    let first = apply_raw(&mut tree, &commit);
    assert!(first.redraw);
    let snapshot = fingerprint(&tree);
    let redraws = tree.stats().redraw_commits;
    for _ in 0..3 {
        let again = apply_raw(&mut tree, &commit);
        assert!(!again.redraw);
        assert!(!again.searchable_changed);
    }
    assert_eq!(fingerprint(&tree), snapshot);
    assert_eq!(tree.stats().redraw_commits, redraws);
}

// ---------------------------------------------------------------------- A2

fn schema_validator(style: &Value) -> Result<(), String> {
    let Some(map) = style.as_object() else {
        return Err("style must be a JSON object".into());
    };
    match map.get("width") {
        None | Some(Value::Number(_)) | Some(Value::String(_)) => Ok(()),
        Some(other) => Err(format!("invalid type for width: {other}")),
    }
}

fn schema_tree() -> HostTree {
    HostTree::with_options(TreeOptions {
        style_validator: schema_validator,
        ..TreeOptions::default()
    })
}

#[test]
fn a2_invalid_final_operation_leaves_earlier_operations_unapplied() {
    let invalid_finals = [
        r#"["frobnicate",1]"#,
        r#"["destroyElement",-1]"#,
        r#"["appendChild",1]"#,
        r#"["setEventListener",1,"click",1]"#,
        r#"["setEventListener",1,"click","true"]"#,
        r#"["setEventListener",1,"click"]"#,
        r#"["setKeyboardFocusDim","yes"]"#,
        r#"["setText",1,5]"#,
        r#"["createElement",1]"#,
        r#"["createElement",1,7]"#,
        r#"["setCustomProp",1,"key"]"#,
        r#"["setCustomProp",1,5,1]"#,
        r#"[]"#,
        r#"[1,2]"#,
        r#"7"#,
        r#"{"op":"createElement"}"#,
    ];
    for last in invalid_finals {
        let mut tree = three_level_tree();
        let before = fingerprint(&tree);
        let batch = format!(
            r#"[["setKeyboardFocusDim",false],["createElement",50,"div"],["appendChild",0,50],["setText",2,"x"],["setRoot",1],{last}]"#
        );
        let error = apply_err(&mut tree, &batch);
        assert_eq!(error.kind(), BatchErrorKind::Decode, "{last}");
        assert_eq!(error.op_index(), Some(5), "{last}: {error}");
        assert!(
            error.message().starts_with("failed to parse batch"),
            "{error}"
        );
        assert!(error.message().contains("Batch op 5"), "{error}");
        assert_eq!(fingerprint(&tree), before, "{last}");
        assert!(tree.keyboard_focus_dim());
        assert_eq!(tree.root(), Some(0));
        assert!(!tree.contains(50));
    }
}

#[test]
fn a2_malformed_final_style_leaves_earlier_operations_unapplied() {
    for bad_style in [
        r#"{"width":true}"#,
        r#"{"width":[1]}"#,
        r#"[1,2]"#,
        "5",
        r#""text""#,
        "null",
    ] {
        let mut tree = schema_tree();
        apply(
            &mut tree,
            json!([["createElement", 1, "div"], ["setRoot", 1]]),
        );
        let before = fingerprint(&tree);
        let batch = format!(
            r#"[["setKeyboardFocusDim",false],["createElement",2,"div"],["setStyle",1,{{"width":1}}],["setText",1,"x"],["setStyle",1,{bad_style}]]"#
        );
        let error = apply_err(&mut tree, &batch);
        assert_eq!(error.kind(), BatchErrorKind::Style, "{bad_style}");
        assert_eq!(error.op_index(), Some(4));
        assert!(
            error
                .message()
                .starts_with("Batch op 4 setStyle parse error"),
            "{error}"
        );
        assert_eq!(fingerprint(&tree), before, "{bad_style}");
        assert!(tree.keyboard_focus_dim());
        // The style table was swept: the style interned by the failed batch did not persist.
        assert_eq!(tree.style_table_stats().interned, 0, "{bad_style}");
        assert_eq!(tree.style_table_stats().sweeps, 1);
    }
}

#[test]
fn a2_decode_failures_precede_style_failures_and_report_first_failing_index() {
    let mut tree = schema_tree();
    // Phase 1 fails at op 3 even though op 1 has a bad style: decode happens first.
    let error = apply_err(
        &mut tree,
        r#"[["createElement",1,"div"],["setStyle",1,{"width":true}],["setRoot",1],["setRoot",-1]]"#,
    );
    assert_eq!(error.kind(), BatchErrorKind::Decode);
    assert_eq!(error.op_index(), Some(3));
    // Two bad styles: the first one is reported.
    let error = apply_err(
        &mut tree,
        r#"[["setStyle",1,{"width":true}],["setStyle",1,{"width":false}]]"#,
    );
    assert_eq!(error.kind(), BatchErrorKind::Style);
    assert_eq!(error.op_index(), Some(0));
    // JSON that is not an array, and broken JSON.
    for batch in [
        "{}",
        "null",
        "5",
        "[[\"setRoot\",1]",
        "[[\"setRoot\",1]] trailing",
        "",
    ] {
        let error = apply_err(&mut tree, batch);
        assert!(
            error.message().starts_with("failed to parse batch"),
            "{batch}: {error}"
        );
    }
    // An unknown operation is a hard error, never skipped.
    let error = apply_err(&mut tree, r#"[["createElement",1,"div"],["noSuchOp",1]]"#);
    assert_eq!(error.op_index(), Some(1));
    assert!(error.message().contains("unknown operation"));
    assert!(tree.is_empty());
    // Invalid UTF-8 transport bytes are rejected as a decode failure.
    assert!(tree.apply_batch_bytes(&[b'[', 0xff, b']']).is_err());
    assert_eq!(
        tree.apply_batch_bytes(br#"[["createElement",1,"div"]]"#)
            .unwrap()
            .redraw,
        true
    );
}

// ---------------------------------------------------------------------- A3

/// Every operation with an identifier; `@` marks each identifier position.
const ID_TEMPLATES: [&str; 9] = [
    r#"["createElement",@,"div"]"#,
    r#"["destroyElement",@]"#,
    r#"["appendChild",@,@]"#,
    r#"["insertBefore",@,@,@]"#,
    r#"["setStyle",@,{"a":1}]"#,
    r#"["setText",@,"t"]"#,
    r#"["setEventListener",@,"click",true]"#,
    r#"["setRoot",@]"#,
    r#"["setCustomProp",@,"k",1]"#,
];

fn id_positions(template: &str) -> usize {
    template.matches('@').count()
}

/// Substitutes `candidate` at identifier position `position` and the valid id `1` at the
/// other positions.
fn instantiate(template: &str, position: usize, candidate: &str) -> String {
    let mut out = String::new();
    for (index, part) in template.split('@').enumerate() {
        if index > 0 {
            out.push_str(if index - 1 == position {
                candidate
            } else {
                "1"
            });
        }
        out.push_str(part);
    }
    out
}

#[test]
fn a3_invalid_identifiers_rejected_in_every_position() {
    let invalid = [
        "-1",
        "1.5",
        "9007199254740992",
        "1e999",
        "\"1\"",
        "null",
        "true",
        "[]",
        "{}",
        "-0.5",
    ];
    for template in ID_TEMPLATES {
        for position in 0..id_positions(template) {
            for candidate in invalid {
                let mut tree = three_level_tree();
                let before = fingerprint(&tree);
                let op = instantiate(template, position, candidate);
                let batch = format!(r#"[{op},["createElement",900,"div"]]"#);
                let error = apply_err(&mut tree, &batch);
                assert_eq!(error.op_index(), Some(0), "{op}");
                assert!(error.message().contains("Batch op 0"), "{op}: {error}");
                assert_eq!(fingerprint(&tree), before, "{op}");
            }
        }
    }
}

#[test]
fn a3_boundary_identifiers_accepted() {
    for candidate in ["9007199254740991", "3.0", "0", "2.5e1"] {
        for template in ID_TEMPLATES {
            for position in 0..id_positions(template) {
                let mut tree = three_level_tree();
                let op = instantiate(template, position, candidate);
                tree.apply_batch(&format!("[{op}]"))
                    .unwrap_or_else(|error| panic!("{op} should be accepted: {error}"));
            }
        }
    }
    let mut tree = HostTree::new();
    apply_raw(
        &mut tree,
        r#"[["createElement",9007199254740991,"div"],["createElement",3.0,"div"],["appendChild",3.0,9007199254740991],["setRoot",3.0]]"#,
    );
    assert_eq!(tree.root(), Some(3));
    assert_eq!(children(&tree, 3), [9_007_199_254_740_991]);
}

// ---------------------------------------------------------------------- A4

#[test]
fn a4_trailing_arguments_are_ignored() {
    let mut tree = HostTree::new();
    apply(
        &mut tree,
        json!([
            ["createElement", 1, "div", "extra", {"x": 1}],
            ["createElement", 2, "div", null],
            ["appendChild", 1, 2, 3, 4],
            ["setText", 2, "hi", 9],
            ["setEventListener", 2, "click", true, "x"],
            ["setRoot", 1, []],
            ["setKeyboardFocusDim", false, 1],
            ["setCustomProp", 2, "k", 5, "tail"],
            ["setStyle", 2, {"a": 1}, {"b": 2}],
            ["destroyElement", 77, 1],
        ]),
    );
    assert_eq!(tree.root(), Some(1));
    assert_eq!(children(&tree, 1), [2]);
    assert_eq!(tree.node(2).unwrap().text(), Some("hi"));
    assert!(tree.node(2).unwrap().has_listener("click"));
    assert!(!tree.keyboard_focus_dim());
    assert_eq!(tree.node(2).unwrap().prop("k"), Some(&json!(5)));
    assert_eq!(
        tree.node(2).unwrap().style().map(|s| (**s).clone()),
        Some(json!({"a": 1}))
    );
}

#[test]
fn a4_has_handler_must_be_boolean() {
    for bad in ["1", "0", "\"true\"", "null", "[true]"] {
        let mut tree = three_level_tree();
        let before = fingerprint(&tree);
        let error = apply_err(
            &mut tree,
            &format!(r#"[["setEventListener",1,"click",{bad}]]"#),
        );
        assert_eq!(error.op_index(), Some(0));
        assert_eq!(fingerprint(&tree), before);
    }
    for bad in ["1", "\"true\"", "null"] {
        let mut tree = three_level_tree();
        apply_err(&mut tree, &format!(r#"[["setKeyboardFocusDim",{bad}]]"#));
        assert!(tree.keyboard_focus_dim());
    }
}

#[test]
fn a4_operations_on_missing_elements_are_harmless() {
    let mut tree = three_level_tree();
    let before = fingerprint(&tree);
    let result = apply(
        &mut tree,
        json!([
            ["destroyElement", 404],
            ["appendChild", 404, 1],
            ["appendChild", 1, 404],
            ["insertBefore", 404, 404, 404],
            ["setStyle", 404, {"a": 1}],
            ["setText", 404, "x"],
            ["setEventListener", 404, "click", true],
            ["setEventListener", 404, "click", false],
            ["setRoot", 404],
            ["setCustomProp", 404, "k", 1],
            ["setCustomProp", 404, "testId", "t"],
            ["setCustomProp", 404, "k", null],
        ]),
    );
    // appendChild(404, 1) moves node 1 out of the root (missing parent): that is the only
    // effect, and nothing is created.
    assert!(result.destroyed.is_empty());
    assert!(!tree.contains(404));
    assert_eq!(tree.len(), 3);
    assert_eq!(tree.root(), Some(0));
    assert_eq!(tree.node(1).unwrap().parent(), None);
    assert!(children(&tree, 0).is_empty());
    assert_ne!(fingerprint(&tree), before);
    // Without that unlink, a batch of purely missing references is entirely idle.
    let before = fingerprint(&tree);
    let result = apply(
        &mut tree,
        json!([
            ["destroyElement", 404],
            ["appendChild", 1, 404],
            ["setText", 404, "x"],
            ["setRoot", 404]
        ]),
    );
    assert!(!result.redraw);
    assert_eq!(fingerprint(&tree), before);
}

// ---------------------------------------------------------------------- A5

fn revisions(tree: &HostTree, ids: &[u64]) -> Vec<(u32, u32)> {
    ids.iter()
        .map(|&id| {
            let node = tree.node(id).unwrap();
            (node.subtree_revision(), node.search_revision())
        })
        .collect()
}

#[test]
fn a5_equivalent_style_takes_no_revision() {
    let mut tree = three_level_tree();
    apply_raw(
        &mut tree,
        r#"[["setStyle",2,{"a":1,"b":[1,2],"c":{"x":1,"y":2}}]]"#,
    );
    let before = revisions(&tree, &[0, 1, 2]);
    let counter = tree.revision_counter();
    let result = apply_raw(
        &mut tree,
        r#"[["setStyle",2,{"c":{"y":2,"x":1},"b":[1,2],"a":1}]]"#,
    );
    assert!(!result.redraw);
    assert_eq!(revisions(&tree, &[0, 1, 2]), before);
    assert_eq!(tree.revision_counter(), counter);
    // A different array order is a different style.
    let result = apply_raw(
        &mut tree,
        r#"[["setStyle",2,{"a":1,"b":[2,1],"c":{"x":1,"y":2}}]]"#,
    );
    assert!(result.redraw);
    assert_ne!(revisions(&tree, &[2]), &before[2..]);
}

#[test]
fn a5_search_revision_follows_text_and_structure_only() {
    let mut tree = three_level_tree();
    let chain = [0, 1, 2];
    let changed =
        |before: &[(u32, u32)], after: &[(u32, u32)], sub: [bool; 3], search: [bool; 3]| {
            for k in 0..3 {
                assert_eq!(before[k].0 != after[k].0, sub[k], "subtree revision of {k}");
                assert_eq!(
                    before[k].1 != after[k].1,
                    search[k],
                    "search revision of {k}"
                );
            }
        };

    // setText: the element and every ancestor.
    let before = revisions(&tree, &chain);
    apply(&mut tree, json!([["setText", 2, "abc"]]));
    changed(&before, &revisions(&tree, &chain), [true; 3], [true; 3]);
    // Equal text: nothing.
    let before = revisions(&tree, &chain);
    apply(&mut tree, json!([["setText", 2, "abc"]]));
    assert_eq!(before, revisions(&tree, &chain));

    // setStyle and a non-highlight property: subtree only.
    let before = revisions(&tree, &chain);
    let searchable = tree.searchable_revision();
    apply(&mut tree, json!([["setStyle", 2, {"width": 4}]]));
    changed(&before, &revisions(&tree, &chain), [true; 3], [false; 3]);
    let before = revisions(&tree, &chain);
    apply(&mut tree, json!([["setCustomProp", 2, "label", "x"]]));
    changed(&before, &revisions(&tree, &chain), [true; 3], [false; 3]);
    assert_eq!(tree.searchable_revision(), searchable);

    // Structure: appending marks the new parent's chain and the old parent's chain.
    apply(
        &mut tree,
        json!([["createElement", 3, "div"], ["appendChild", 0, 3]]),
    );
    let chain4 = [0, 1, 2, 3];
    let before = revisions(&tree, &chain4);
    apply(&mut tree, json!([["appendChild", 3, 2]]));
    let after = revisions(&tree, &chain4);
    assert_ne!(before[0].1, after[0].1, "root: both parents' chain");
    assert_ne!(before[1].1, after[1].1, "old parent");
    assert_ne!(before[3].1, after[3].1, "new parent");
    assert_eq!(
        before[2].1, after[2].1,
        "the moved child's own search revision is unchanged"
    );
    // Destroy marks the former parent's chain.
    let before = revisions(&tree, &[0, 3]);
    apply(&mut tree, json!([["destroyElement", 2]]));
    let after = revisions(&tree, &[0, 3]);
    assert_ne!(before[0].1, after[0].1);
    assert_ne!(before[1].1, after[1].1);
    // Creating an element takes a fresh revision for the new element only.
    let before = revisions(&tree, &[0, 1]);
    apply(&mut tree, json!([["createElement", 9, "div"]]));
    assert_eq!(before, revisions(&tree, &[0, 1]));
    let fresh = tree.node(9).unwrap();
    assert_eq!(fresh.subtree_revision(), tree.revision_counter());
    assert_eq!(fresh.search_revision(), tree.revision_counter());
}

#[test]
fn a5_highlight_declaration_presence_changes_ancestor_search_revision() {
    let mut tree = three_level_tree();
    let chain = [0, 1, 2];
    let own = |tree: &HostTree| revisions(tree, &chain);

    // Setting the declaration where absent: ancestors' search revision, not the element's.
    let before = own(&tree);
    apply(
        &mut tree,
        json!([["setCustomProp", 2, "highlight", {"color": "red"}]]),
    );
    let after = own(&tree);
    assert_eq!(before[2].1, after[2].1);
    assert_ne!(before[1].1, after[1].1);
    assert_ne!(before[0].1, after[0].1);
    assert_ne!(before[2].0, after[2].0);

    // Changing only the content: paint only.
    let before = own(&tree);
    let searchable = tree.searchable_revision();
    apply(
        &mut tree,
        json!([["setCustomProp", 2, "highlight", {"color": "blue"}]]),
    );
    let after = own(&tree);
    for k in 0..3 {
        assert_eq!(before[k].1, after[k].1);
        assert_ne!(before[k].0, after[k].0);
    }
    assert_eq!(tree.searchable_revision(), searchable);

    // Removing it: ancestors again.
    let before = own(&tree);
    apply(&mut tree, json!([["setCustomProp", 2, "highlight", null]]));
    let after = own(&tree);
    assert_eq!(before[2].1, after[2].1);
    assert_ne!(before[1].1, after[1].1);
    assert_ne!(before[0].1, after[0].1);
    assert!(tree.searchable_revision() > searchable);
}

// ---------------------------------------------------------------------- A6

#[test]
fn a6_null_custom_prop_removes_a_property() {
    let mut tree = three_level_tree();
    apply(
        &mut tree,
        json!([["setCustomProp", 2, "motion", {"to": 1}], ["setCustomProp", 2, "tabIndex", 0], ["setCustomProp", 2, "x", 1]]),
    );
    assert_eq!(tree.animated_ids().collect::<Vec<_>>(), [2]);
    assert_eq!(tree.focusable_ids().collect::<Vec<_>>(), [2]);

    let result = apply(&mut tree, json!([["setCustomProp", 2, "motion", null]]));
    assert!(result.redraw);
    assert_eq!(tree.animated_ids().count(), 0);
    assert_eq!(tree.node(2).unwrap().prop("motion"), None);
    apply(
        &mut tree,
        json!([
            ["setCustomProp", 2, "tabIndex", null],
            ["setCustomProp", 2, "x", null]
        ]),
    );
    assert_eq!(tree.focusable_ids().count(), 0);
    assert!(!tree.node(2).unwrap().prop_map_allocated());
    // Removing an absent property is unchanged.
    let result = apply(&mut tree, json!([["setCustomProp", 2, "motion", null]]));
    assert!(!result.redraw);
}

#[test]
fn a6_auto_focus_and_test_id_are_not_general_props() {
    let mut tree = three_level_tree();
    let chain = [0, 1, 2];
    let before = revisions(&tree, &chain);
    let result = apply(
        &mut tree,
        json!([
            ["setCustomProp", 2, "autoFocus", true],
            ["setCustomProp", 2, "testId", "locator"],
            ["setCustomProp", 2, "other", 1]
        ]),
    );
    assert!(result.redraw);
    let node = tree.node(2).unwrap();
    assert!(node.auto_focus());
    assert_eq!(node.test_id(), Some("locator"));
    assert_eq!(node.props().map(|(k, _)| k).collect::<Vec<_>>(), ["other"]);
    assert_eq!(node.prop("autoFocus"), None);
    assert_eq!(node.prop("testId"), None);

    // They are redraw-relevant but take no revision.
    let before_lifted = revisions(&tree, &chain);
    assert_ne!(before, before_lifted); // `other` took a revision above
    let result = apply(
        &mut tree,
        json!([
            ["setCustomProp", 2, "autoFocus", false],
            ["setCustomProp", 2, "testId", "other id"]
        ]),
    );
    assert!(result.redraw);
    assert_eq!(before_lifted, revisions(&tree, &chain));
    // Equal values are idle; non-boolean autoFocus is false; non-string testId clears.
    let result = apply(
        &mut tree,
        json!([
            ["setCustomProp", 2, "autoFocus", "yes"],
            ["setCustomProp", 2, "testId", "other id"]
        ]),
    );
    assert!(!result.redraw);
    apply(&mut tree, json!([["setCustomProp", 2, "autoFocus", true]]));
    apply(
        &mut tree,
        json!([
            ["setCustomProp", 2, "autoFocus", 1],
            ["setCustomProp", 2, "testId", 5]
        ]),
    );
    let node = tree.node(2).unwrap();
    assert!(!node.auto_focus());
    assert_eq!(node.test_id(), None);
    assert_eq!(node.props().count(), 1);
}

#[test]
fn a6_focusable_and_animated_indices_follow_listener_tabindex_and_motion_changes() {
    let mut tree = HostTree::new();
    apply(
        &mut tree,
        json!([
            ["createElement", 1, "div"],
            ["createElement", 2, "input"],
            ["createElement", 3, "textarea"],
            ["createElement", 4, "div"],
            ["createElement", 5, "anchored"],
            ["createElement", 6, "anchored"],
            ["setRoot", 1],
        ]),
    );
    let focusable = |tree: &HostTree| tree.focusable_ids().collect::<Vec<_>>();
    let animated = |tree: &HostTree| tree.animated_ids().collect::<Vec<_>>();
    assert_eq!(focusable(&tree), [2, 3]);
    assert_eq!(tree.anchored_count(), 2);

    for (event, expected) in [
        ("keyDown", true),
        ("keyUp", true),
        ("focus", true),
        ("blur", true),
        ("click", false),
        ("mouseDown", false),
    ] {
        apply(&mut tree, json!([["setEventListener", 4, event, true]]));
        assert_eq!(focusable(&tree).contains(&4), expected, "{event}");
        apply(&mut tree, json!([["setEventListener", 4, event, false]]));
        assert!(!focusable(&tree).contains(&4), "{event} removed");
    }
    // Two focus-relevant listeners: removing one keeps membership.
    apply(
        &mut tree,
        json!([
            ["setEventListener", 4, "focus", true],
            ["setEventListener", 4, "blur", true],
            ["setEventListener", 4, "focus", false]
        ]),
    );
    assert!(focusable(&tree).contains(&4));
    apply(&mut tree, json!([["setEventListener", 4, "blur", false]]));
    assert!(!focusable(&tree).contains(&4));

    // tabIndex: integers only.
    for (value, expected) in [
        (json!(0), true),
        (json!(-1), true),
        (json!(3), true),
        (json!("1"), false),
        (json!(1.5), false),
        (json!(true), false),
    ] {
        apply(
            &mut tree,
            json!([["setCustomProp", 4, "tabIndex", value.clone()]]),
        );
        assert_eq!(focusable(&tree).contains(&4), expected, "tabIndex {value}");
        apply(&mut tree, json!([["setCustomProp", 4, "tabIndex", null]]));
        assert!(!focusable(&tree).contains(&4));
    }

    // motion.
    apply(
        &mut tree,
        json!([["setCustomProp", 4, "motion", {"x": 1}], ["setCustomProp", 6, "motion", 1]]),
    );
    assert_eq!(animated(&tree), [4, 6]);
    apply(&mut tree, json!([["setCustomProp", 4, "motion", null]]));
    assert_eq!(animated(&tree), [6]);

    // Destruction removes the element from both indices and from the anchored count.
    apply(
        &mut tree,
        json!([
            ["appendChild", 1, 2],
            ["appendChild", 1, 6],
            ["setCustomProp", 2, "motion", 1]
        ]),
    );
    assert_eq!(animated(&tree), [2, 6]);
    let result = apply(&mut tree, json!([["destroyElement", 1]]));
    assert_eq!(result.destroyed, [1, 2, 6]);
    assert_eq!(focusable(&tree), [3]);
    assert!(animated(&tree).is_empty());
    assert_eq!(tree.anchored_count(), 1);
}

// ---------------------------------------------------------------------- A7

fn distinct_style(k: u64) -> String {
    format!(r#"{{"width":{k}}}"#)
}

#[test]
fn a7_collision_resolves_by_content() {
    let mut tree = HostTree::with_options(TreeOptions {
        forced_style_hash: Some(7),
        ..TreeOptions::default()
    });
    apply(
        &mut tree,
        json!([
            ["createElement", 1, "div"],
            ["createElement", 2, "div"],
            ["createElement", 3, "div"],
            ["createElement", 4, "div"]
        ]),
    );
    apply_raw(
        &mut tree,
        r#"[["setStyle",1,{"a":1,"b":2}],["setStyle",2,{"a":9}],["setStyle",3,{"b":2,"a":1}],["setStyle",4,{"a":9}]]"#,
    );
    let style = |id: u64| tree.node(id).unwrap().style().unwrap().clone();
    assert_ne!(*style(1), *style(2));
    assert_eq!(*style(1), json!({"a": 1, "b": 2}));
    assert_eq!(*style(2), json!({"a": 9}));
    assert!(Arc::ptr_eq(&style(1), &style(3)));
    assert!(Arc::ptr_eq(&style(2), &style(4)));
    assert!(!Arc::ptr_eq(&style(1), &style(2)));
    assert_eq!(tree.style_table_stats().interned, 2);
}

#[test]
fn a7_sweep_reclaims_styles_of_destroyed_and_restyled_elements() {
    let mut tree = HostTree::new();
    // 100 elements with distinct styles: 100 >= 64 triggers a sweep that removes nothing.
    let mut ops = Vec::new();
    for k in 0..100 {
        ops.push(format!(r#"["createElement",{k},"div"]"#));
        ops.push(format!(r#"["setStyle",{k},{}]"#, distinct_style(k)));
    }
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    let stats = tree.style_table_stats();
    assert_eq!(
        (stats.interned, stats.sweeps, stats.after_last_sweep),
        (100, 1, 100)
    );

    // Restyling 30 elements to one new shared style adds an entry and releases 30; the
    // count 101 is below both thresholds (max(2 * 100, 64) = 200): no sweep yet.
    let ops: Vec<String> = (0..30)
        .map(|k| format!(r#"["setStyle",{k},{{"shared":true}}]"#))
        .collect();
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    let stats = tree.style_table_stats();
    assert_eq!((stats.interned, stats.sweeps), (101, 1));

    // Destroying 60 elements leaves 40 live: 101 > max(2 * 40, 64): the sweep reclaims
    // exactly the unreferenced entries.
    let ops: Vec<String> = (30..90)
        .map(|k| format!(r#"["destroyElement",{k}]"#))
        .collect();
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    let stats = tree.style_table_stats();
    // Live elements 0..30 share one style, 90..100 have ten distinct styles.
    assert_eq!(
        (stats.interned, stats.sweeps, stats.after_last_sweep),
        (11, 2, 11)
    );
    for k in 90..100 {
        assert_eq!(
            tree.node(k).unwrap().style().map(|s| (**s).clone()),
            Some(serde_json::from_str(&distinct_style(k)).unwrap())
        );
    }
    assert!(tree.style_table_stats().interned <= tree.len());
}

#[test]
fn a7_no_sweep_below_floor_and_sweep_at_thresholds() {
    let mut tree = HostTree::new();
    apply(&mut tree, json!([["createElement", 1, "div"]]));
    // One element restyled 63 times: the table grows to 63 entries, 62 of them
    // unreferenced, without a sweep (63 < 64 and 63 <= max(2 * 1, 64)).
    for k in 0..63 {
        apply_raw(
            &mut tree,
            &format!(r#"[["setStyle",1,{}]]"#, distinct_style(k)),
        );
    }
    let stats = tree.style_table_stats();
    assert_eq!((stats.interned, stats.sweeps), (63, 0));
    // The 64th entry reaches max(2 * 0, 64): a sweep leaves only the live style.
    apply_raw(
        &mut tree,
        &format!(r#"[["setStyle",1,{}]]"#, distinct_style(63)),
    );
    let stats = tree.style_table_stats();
    assert_eq!(
        (stats.interned, stats.sweeps, stats.after_last_sweep),
        (1, 1, 1)
    );
    // Growth factor: the next sweep needs max(2 * 1, 64) = 64 entries again.
    for k in 64..64 + 62 {
        apply_raw(
            &mut tree,
            &format!(r#"[["setStyle",1,{}]]"#, distinct_style(k)),
        );
    }
    assert_eq!(tree.style_table_stats().sweeps, 1);
    assert_eq!(tree.style_table_stats().interned, 63);
    apply_raw(
        &mut tree,
        &format!(r#"[["setStyle",1,{}]]"#, distinct_style(200)),
    );
    assert_eq!(tree.style_table_stats().sweeps, 2);
    assert_eq!(tree.style_table_stats().interned, 1);
}

#[test]
fn a7_sweep_after_destroying_most_of_a_large_tree_leaves_at_most_live_count() {
    let mut tree = HostTree::new();
    let mut ops = vec![
        r#"["createElement",0,"div"]"#.to_string(),
        r#"["setRoot",0]"#.to_string(),
    ];
    for k in 1..=500 {
        ops.push(format!(r#"["createElement",{k},"div"]"#));
        ops.push(format!(r#"["appendChild",0,{k}]"#));
        ops.push(format!(r#"["setStyle",{k},{}]"#, distinct_style(k)));
    }
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    assert_eq!(tree.style_table_stats().interned, 500);
    let sweeps = tree.style_table_stats().sweeps;
    // Destroy all but 20 children of the root.
    let ops: Vec<String> = (21..=500)
        .map(|k| format!(r#"["destroyElement",{k}]"#))
        .collect();
    apply_raw(&mut tree, &format!("[{}]", ops.join(",")));
    let stats = tree.style_table_stats();
    assert_eq!(tree.len(), 21);
    assert_eq!(stats.sweeps, sweeps + 1);
    assert_eq!(stats.interned, 20);
    assert!(stats.interned <= tree.len());
}
