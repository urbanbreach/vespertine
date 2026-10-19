//! DEV-011: embedded launch on the test platform.

use crate::{
    AppContext as _, Application, BackgroundExecutor, ForegroundExecutor, TestDispatcher,
    TestPlatform,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

struct Harness {
    dispatcher: TestDispatcher,
    platform: Rc<TestPlatform>,
}

impl Harness {
    fn new() -> Self {
        let dispatcher = TestDispatcher::new(0);
        let arc_dispatcher = Arc::new(dispatcher.clone());
        let platform = TestPlatform::new(
            BackgroundExecutor::new(arc_dispatcher.clone()),
            ForegroundExecutor::new(arc_dispatcher),
        );
        Self {
            dispatcher,
            platform,
        }
    }
}

#[test]
fn dev_011_a1_embedded_launch_runs_callback_once_and_handle_updates() {
    let Harness { platform, .. } = Harness::new();
    let launches = Rc::new(Cell::new(0));
    let handle = Application::with_platform(platform).run_embedded({
        let launches = launches.clone();
        move |_cx| launches.set(launches.get() + 1)
    });
    assert_eq!(launches.get(), 1, "launch callback runs exactly once");

    let value = handle.update(|cx| {
        cx.set_global(Marker(7));
        cx.global::<Marker>().0
    });
    assert_eq!(value, 7);
    assert_eq!(handle.update(|cx| cx.global::<Marker>().0), 7);
    assert_eq!(launches.get(), 1, "updating does not relaunch");
    drop(handle);
    assert_eq!(launches.get(), 1);
}

struct Marker(u32);
impl crate::Global for Marker {}

#[test]
fn dev_011_a2_no_queued_task_runs_until_the_dispatcher_is_driven() {
    let harness = Harness::new();
    let handle = Application::with_platform(harness.platform.clone()).run_embedded(|_cx| {});
    let ran = Rc::new(RefCell::new(Vec::new()));
    handle.update(|cx| {
        let ran = ran.clone();
        cx.spawn(async move |_| ran.borrow_mut().push("foreground"))
            .detach();
        cx.background_spawn(async {}).detach();
    });
    assert!(
        ran.borrow().is_empty(),
        "the test platform must not run tasks by itself"
    );
    harness.dispatcher.run_until_parked();
    assert_eq!(*ran.borrow(), ["foreground"]);
}

#[test]
fn dev_011_a3_async_context_does_not_keep_the_app_alive() {
    let harness = Harness::new();
    let handle = Application::with_platform(harness.platform.clone()).run_embedded(|_cx| {});
    let async_cx = handle.to_async();
    assert!(async_cx.app.upgrade().is_some(), "alive while held");
    drop(handle);
    harness.dispatcher.run_until_parked();
    assert!(
        async_cx.app.upgrade().is_none(),
        "the async context must not keep the app alive"
    );
}
