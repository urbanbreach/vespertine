//! APP-007 R8 / DEV-007 end to end: a Node process started with piped standard input creates a
//! native renderer, opens a real window, and serves the automation protocol over standard
//! input and output; this test drives it as a client (`Automation::launch`) and observes the
//! window the way a user would. It needs a display server (Wayland or X11) like the lifecycle
//! example does.

#[path = "../harness/mod.rs"]
mod harness;

use std::sync::{Arc, Mutex};

use serde_json::json;
use vespertine_test_host::automation::{
    Automation, Capability, ErrorCode, LaunchOptions, SessionOptions,
};

const WAIT_MS: Option<f64> = Some(15_000.0);

fn launch_app(stderr: &Arc<Mutex<Vec<String>>>) -> Automation {
    assert!(
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some(),
        "live automation opens a real window: set WAYLAND_DISPLAY or DISPLAY"
    );
    let addon = harness::build_addon(false);
    harness::install_dependencies();
    let lines = stderr.clone();
    Automation::launch(
        &LaunchOptions {
            command: "node".to_owned(),
            args: vec!["examples/automation_app.mjs".to_owned()],
            cwd: Some(harness::package_dir()),
            env: vec![(
                "VESPERTINE_NATIVE_ADDON".to_owned(),
                addon.to_string_lossy().into_owned(),
            )],
        },
        SessionOptions {
            on_stderr: Some(Arc::new(move |line| lines.lock().unwrap().push(line))),
            ..SessionOptions::default()
        },
    )
    .unwrap_or_else(|error| {
        panic!(
            "launching the automation app failed: {error}\nstderr: {:#?}",
            stderr.lock().unwrap()
        )
    })
}

#[test]
fn app_007_r8_a_piped_javascript_process_serves_live_automation_over_stdio() {
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let app = launch_app(&stderr);

    // initialize: process identity, capabilities, the window's real viewport.
    let info = app.info().clone();
    assert_eq!(info.protocol_version, 1);
    assert_ne!(info.pid, 0);
    assert_ne!(
        info.pid,
        u64::from(std::process::id()),
        "the app is another process"
    );
    let mut offered = info.capabilities.clone();
    offered.sort_by_key(|capability| capability.as_str());
    assert_eq!(
        offered,
        [Capability::Clock, Capability::Input, Capability::Tree]
    );
    assert!(info.width > 0.0 && info.height > 0.0, "{info:?}");

    // The tree the JavaScript application mounted, with the painted bounds of the real frame.
    let title = app.get_by_test_id("title").wait_for(WAIT_MS).unwrap();
    assert_eq!(title.text.as_deref(), Some("Automation"));
    let tree = app.tree().unwrap().expect("a mounted tree");
    assert_eq!(tree.id, 1);
    let root = tree.bounds.expect("the root was painted");
    assert_eq!(
        (root.x, root.y, root.width, root.height),
        (0.0, 0.0, 400.0, 300.0)
    );
    let save = app.get_by_test_id("save").bounds().unwrap();
    assert_eq!((save.width, save.height), (80.0, 30.0));
    assert!(save.y >= 28.0, "{save:?}");
    assert_eq!(app.get_by_test_id("list").bounds().unwrap().height, 90.0);
    let window = app.get_by_test_id("window").element().unwrap();
    assert_eq!(
        window.text.as_deref(),
        Some(format!("window: {}x{}", info.width, info.height).as_str()),
        "the viewport matches what the renderer reports to JavaScript"
    );
    let serve = app.get_by_test_id("serve").element().unwrap();
    assert!(
        serve
            .text
            .as_deref()
            .is_some_and(|text| text.contains("already being served")),
        "{serve:?}"
    );
    let all = app.all_text().unwrap();
    assert!(all.iter().any(|text| text == "Item 9"), "{all:?}");
    let painted = app.painted_text().unwrap();
    assert!(painted.iter().any(|text| text == "Save"), "{painted:?}");
    assert_eq!(app.selected_text().unwrap(), None);

    // A click on the button reaches the JavaScript handler, which mutates the tree.
    app.get_by_test_id("save").click().unwrap();
    app.get_by_text("clicks: 1").wait_for(WAIT_MS).unwrap();
    app.get_by_test_id("save").click().unwrap();
    app.get_by_text("clicks: 2").wait_for(WAIT_MS).unwrap();

    // Keystrokes focus the input and reach its key handler; blur removes focus.
    app.get_by_test_id("name").press("a b").unwrap();
    app.get_by_text("keys: a b").wait_for(WAIT_MS).unwrap();
    app.get_by_text("focus: name").wait_for(WAIT_MS).unwrap();
    app.blur().unwrap();
    app.get_by_text("focus: none").wait_for(WAIT_MS).unwrap();
    app.focus(3).unwrap();
    app.get_by_text("focus: name").wait_for(WAIT_MS).unwrap();

    // Scrolling: wheel input and absolute offsets.
    assert_eq!(app.scroll_offset(5).unwrap(), Some((0.0, 0.0)));
    app.get_by_test_id("list").wheel(0.0, -30.0).unwrap();
    assert_eq!(app.scroll_offset(5).unwrap(), Some((0.0, 30.0)));
    app.scroll_to(5, 0.0, 60.0).unwrap();
    assert_eq!(app.scroll_offset(5).unwrap(), Some((0.0, 60.0)));
    assert_eq!(app.scroll_offset(2).unwrap(), None);

    // The motion clock is the process's; a live window cannot read its frame back.
    assert_eq!(app.clock().set(2000.0).unwrap(), 2000.0);
    assert_eq!(app.clock().fast_forward(500.0).unwrap(), 2500.0);
    assert_eq!(
        app.request(
            "screenshot",
            json!({ "path": "/tmp/vespertine-live-automation.png" })
        )
        .unwrap_err()
        .code,
        ErrorCode::Unsupported
    );

    // Closing the session terminates the process.
    app.close();
    assert_eq!(app.tree().unwrap_err().code, ErrorCode::Closed);
}
