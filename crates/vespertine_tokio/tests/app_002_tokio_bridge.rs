//! APP-002 A6: the external executor bridge stores a Tokio handle as a global, tasks spawned on
//! Tokio are awaited from the UI executor.

use std::thread::{self, ThreadId};

use vespertine::TestAppContext;
use vespertine_tokio::Tokio;

#[vespertine::test]
async fn app_002_a6_tokio_bridge_awaits_a_tokio_task_from_the_ui_executor(cx: &mut TestAppContext) {
    // The Tokio worker threads complete outside the deterministic test scheduler.
    cx.executor().allow_parking();
    cx.update(vespertine_tokio::init);
    assert!(cx.update(
        |cx| Tokio::handle(cx).runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    ));

    let ui_thread = thread::current().id();

    // Awaited directly from the test's UI task.
    let task = cx.update(|cx| Tokio::spawn(cx, async { (thread::current().id(), 5u32) }));
    let (tokio_thread, value): (ThreadId, u32) = task.await.unwrap();
    assert_eq!(value, 5);
    assert_ne!(tokio_thread, ui_thread, "the future ran on a Tokio worker");

    // Awaited from a task spawned on the foreground executor with an async app context.
    let from_foreground = cx.spawn(async move |cx| {
        let task = cx.update(|cx| {
            Tokio::spawn(cx, async {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                11u32
            })
        });
        task.await.unwrap()
    });
    assert_eq!(from_foreground.await, 11);

    // Errors from the Tokio side come back to the UI executor as errors.
    let failing = cx.update(|cx| {
        Tokio::spawn_result(cx, async {
            Err::<(), _>(anyhow::anyhow!("tokio side failed"))
        })
    });
    assert_eq!(failing.await.unwrap_err().to_string(), "tokio side failed");
}

#[vespertine::test]
async fn app_002_a6_tokio_bridge_can_use_an_existing_runtime_handle(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();
    cx.update(|cx| vespertine_tokio::init_from_handle(cx, handle));
    let task = cx.update(|cx| Tokio::spawn(cx, async { 3u32 + 4 }));
    assert_eq!(task.await.unwrap(), 7);
    drop(runtime);
}
