//! APP-005 R9: a batch that changes anything a frame would draw queues a repaint request;
//! one that changes nothing does not; batches before initialization and after the loop
//! stopped are still applied.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use vespertine_js::{HostError, HostRenderer, UiLink, UiTask};

const MOUNT: &str = r#"[["createElement",1,"div"],["setStyle",1,{"width":10}],["setRoot",1]]"#;

fn running() -> (HostRenderer, async_channel::Receiver<UiTask>) {
    let renderer = HostRenderer::new(None);
    let (link, tasks) = UiLink::<UiTask>::channel();
    renderer
        .attach_link(link, Arc::new(AtomicBool::new(true)))
        .unwrap();
    (renderer, tasks)
}

#[test]
fn a_changing_batch_queues_one_repaint_and_an_equivalent_resend_queues_none() {
    let (renderer, tasks) = running();
    let outcome = renderer.apply_batch(MOUNT).unwrap();
    assert!(outcome.repainted);
    assert_eq!(tasks.len(), 1);

    let resend = r#"[["setStyle",1,{"width":10}],["setRoot",1]]"#;
    let outcome = renderer.apply_batch(resend).unwrap();
    assert!(!outcome.repainted);
    assert_eq!(tasks.len(), 1, "an idle commit must not wake the window");

    assert!(!renderer.apply_batch("[]").unwrap().repainted);
    assert_eq!(tasks.len(), 1);
}

#[test]
fn a_rejected_batch_changes_nothing_and_queues_nothing() {
    let (renderer, tasks) = running();
    let error = renderer
        .apply_batch(r#"[["createElement",1,"div"],["bogus",2]]"#)
        .unwrap_err();
    assert!(matches!(error, HostError::Batch(_)));
    assert!(error.to_string().contains("Batch op 1"), "{error}");
    assert_eq!(renderer.with_tree(|tree| tree.len()), 0);
    assert_eq!(tasks.len(), 0);
}

#[test]
fn destroyed_identities_are_reported_in_preorder() {
    let (renderer, _tasks) = running();
    renderer
        .apply_batch(
            r#"[["createElement",1,"div"],["createElement",2,"div"],["createElement",3,"div"],
                ["appendChild",1,2],["appendChild",1,3],["setRoot",1]]"#,
        )
        .unwrap();
    let outcome = renderer
        .apply_batch(r#"[["destroyElement",1],["destroyElement",9]]"#)
        .unwrap();
    assert_eq!(outcome.destroyed, vec![1, 2, 3]);
}

#[test]
fn a_batch_before_initialization_is_applied_and_the_repaint_request_fails() {
    let renderer = HostRenderer::new(None);
    let error = renderer.apply_batch(MOUNT).unwrap_err();
    assert_eq!(
        error,
        HostError::RepaintFailed(Box::new(HostError::NotInitialized))
    );
    assert_eq!(renderer.with_tree(|tree| tree.root()), Some(1));
}

#[test]
fn a_batch_after_the_loop_stopped_is_applied_and_the_repaint_request_fails_as_not_running() {
    let (renderer, tasks) = running();
    drop(tasks);
    let error = renderer.apply_batch(MOUNT).unwrap_err();
    assert_eq!(
        error,
        HostError::RepaintFailed(Box::new(HostError::NotRunning))
    );
    assert_eq!(renderer.with_tree(|tree| tree.root()), Some(1));
    // A batch that changes nothing needs no repaint, so it succeeds even now.
    assert!(!renderer.apply_batch("[]").unwrap().repainted);
}
