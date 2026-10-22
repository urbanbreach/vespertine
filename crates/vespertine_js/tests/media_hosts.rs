//! Image and vector-image hosts keep their node's accessibility properties, style, motion and
//! event subscriptions when they render as media leaves.

use std::{cell::RefCell, rc::Rc, sync::Arc, sync::atomic::AtomicU64};

use serde_json::{Value, json};
use vespertine_js::HostTreeHost;
use vespertine_test_host::{HostOptions, NativeEvent, TestHost, require_backend};

type EventLog = Rc<RefCell<Vec<NativeEvent>>>;

const MARKUP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;

fn media_host() -> (TestHost, EventLog) {
    let events: EventLog = Rc::default();
    let sink = events.clone();
    let mut host = TestHost::with_tree(
        HostOptions {
            event_dispatcher: Some(Box::new(move |event| sink.borrow_mut().push(event))),
            ..HostOptions::with_size(200.0, 200.0)
        },
        HostTreeHost::new(Arc::new(AtomicU64::new(0))),
    )
    .expect("host");
    let mut batch = vec![
        json!(["createElement", 1, "div"]),
        json!(["setStyle", 1, {"width": 200, "height": 200, "flexDirection": "column"}]),
    ];
    for (id, kind, label) in [(2, "img", "alpha"), (3, "svg", "beta")] {
        batch.extend([
            json!(["createElement", id, kind]),
            json!(["setStyle", id, {"width": 40, "height": 40}]),
            json!(["setCustomProp", id, "testId", kind]),
            json!(["setCustomProp", id, "role", "button"]),
            json!(["setCustomProp", id, "aria-label", label]),
            json!(["setEventListener", id, "click", true]),
            json!(["appendChild", 1, id]),
        ]);
    }
    batch.extend([
        json!(["setCustomProp", 2, "alt", "Lorem ipsum"]),
        json!(["setCustomProp", 3, "source", MARKUP]),
        json!(["setRoot", 1]),
    ]);
    host.apply_batch(&Value::Array(batch).to_string())
        .expect("the batch is valid");
    host.flush();
    (host, events)
}

/// The accessibility nodes with role `role`, as their serialized JSON.
fn nodes_with_role(tree: &Value, role: &str) -> Vec<Value> {
    tree["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|entry| entry[1].clone())
        .filter(|node| node["role"] == json!(role))
        .collect()
}

#[test]
fn media_hosts_expose_their_accessibility_properties_and_receive_clicks() {
    require_backend!();
    let (mut host, events) = media_host();

    let tree = host
        .accessibility_tree()
        .unwrap()
        .expect("accessibility is enabled");
    let mut labels = nodes_with_role(&tree, "button")
        .iter()
        .map(|node| node["properties"]["label"].clone())
        .collect::<Vec<_>>();
    labels.sort_by_key(|label| label.to_string());
    assert_eq!(
        labels,
        [json!("alpha"), json!("beta")],
        "explicit role and label of both media hosts: {tree}"
    );

    for (kind, id) in [("img", 2), ("svg", 3)] {
        let nodes = host.elements_by_test_id(kind).expect("tree");
        let (x, y) = nodes[0].bounds.expect("painted").center();
        events.borrow_mut().clear();
        host.click(x, y, 0, "").unwrap();
        let clicks = events
            .borrow()
            .iter()
            .filter(|event| event.event_type == "click")
            .map(|event| event.element_id)
            .collect::<Vec<_>>();
        assert_eq!(clicks, [id], "{kind} receives its click");
    }
}

#[test]
fn media_hosts_take_their_motion_style_like_a_div_host() {
    require_backend!();
    let (mut host, _) = media_host();
    let motion = json!({"initial": false, "animate": {"width": 80, "height": 24}});
    host.apply_batch(
        &json!([
            ["createElement", 4, "div"],
            ["setStyle", 4, {"width": 40, "height": 40}],
            ["setCustomProp", 4, "testId", "div"],
            ["appendChild", 1, 4],
            ["setCustomProp", 2, "motion", motion],
            ["setCustomProp", 3, "motion", motion],
            ["setCustomProp", 4, "motion", motion],
        ])
        .to_string(),
    )
    .expect("the batch is valid");
    host.flush();

    for kind in ["div", "img", "svg"] {
        let nodes = host.elements_by_test_id(kind).expect("tree");
        let bounds = nodes[0].bounds.expect("painted");
        assert_eq!(
            (bounds.width, bounds.height),
            (80.0, 24.0),
            "{kind} takes the size its motion settles at"
        );
    }
}
