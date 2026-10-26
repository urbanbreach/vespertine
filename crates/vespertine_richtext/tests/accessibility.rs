//! A11Y-003 A7: a rich text run is a label holding its displayed text.

mod support;

use std::rc::Rc;
use support::Harness;
use vespertine::{IntoElement, ParentElement as _, Role, div};
use vespertine_richtext::RichText;

#[test]
fn a_run_is_one_label_holding_its_displayed_text() {
    let mut harness = Harness::new(
        300.,
        100.,
        Rc::new(|_, _| {
            div()
                .child(RichText::new("greeting", "Hello, world"))
                .child(RichText::new("gutter", "12").chrome())
                .into_any_element()
        }),
    );
    let window = harness.window.into();
    assert!(
        harness
            .cx
            .simulate_accessibility_activation(window)
            .unwrap()
    );
    harness.draw();
    let update = harness
        .cx
        .last_accessibility_update(window)
        .unwrap()
        .expect("an accessible frame delivers a tree");
    let mut labels: Vec<&str> = update
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == Role::Label)
        .filter_map(|(_, node)| node.value())
        .collect();
    labels.sort_unstable();
    assert_eq!(
        labels,
        ["12", "Hello, world"],
        "each run is one label, its shaped text adds none"
    );
}
