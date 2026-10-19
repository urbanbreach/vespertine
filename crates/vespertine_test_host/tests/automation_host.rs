//! DEV-007 end to end: one scenario against the in-process offscreen host and against a launched
//! application speaking the protocol over standard input and output.

use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc, time::Duration};
use vespertine::{
    Bounds, Context, IntoElement, MotionClock, ParentElement, Render, Styled, Window, canvas, div,
    fill, px, rgb, size,
};
use vespertine_test_host::{
    HostOptions, RetainedHost, TestHost,
    automation::{Automation, DragOptions, ErrorCode, LaunchOptions, SessionOptions},
    fixture::{FixtureTree, scenario_batch},
    require_backend,
};

fn fixture_host(echo: Option<String>) -> Rc<RefCell<TestHost>> {
    let mut tree = FixtureTree::new();
    tree.apply_batch(&scenario_batch(echo).to_string()).unwrap();
    Rc::new(RefCell::new(
        TestHost::with_tree(HostOptions::with_size(400.0, 300.0), tree).unwrap(),
    ))
}

fn launch(env: &[(&str, &str)], cwd: Option<std::path::PathBuf>) -> Automation {
    Automation::launch(
        &LaunchOptions {
            command: env!("CARGO_BIN_EXE_vespertine_automation_fixture").to_owned(),
            args: Vec::new(),
            cwd,
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        },
        SessionOptions::default(),
    )
    .expect("launch the fixture application")
}

fn tree_json(app: &Automation) -> Value {
    app.tree().unwrap().expect("a tree").to_json()
}

/// The scenario of acceptance A1: render a tree, locate, click, fill, drag, wheel, read the
/// tree, set the clock. Everything observable that is not process-specific goes in the transcript.
fn scenario(app: &Automation) -> Vec<Value> {
    let mut transcript = Vec::new();
    let info = app.info();
    transcript.push(json!({
        "protocolVersion": info.protocol_version,
        "capabilities": info.capabilities.iter().map(|cap| format!("{cap:?}")).collect::<Vec<_>>(),
        "viewport": [info.width, info.height],
    }));
    transcript.push(tree_json(app));

    // Locate and measure.
    for test_id in ["title", "name", "save", "list", "drag"] {
        let locator = app.get_by_test_id(test_id);
        transcript.push(json!({ "center": test_id, "at": locator.center().unwrap() }));
    }
    assert_eq!(
        app.get_by_test_id("ghost").bounds().unwrap_err().code,
        ErrorCode::NotFound,
        "a hidden element has no painted bounds"
    );

    // Fill an input, click a button, wheel a list, drag in an area.
    app.get_by_test_id("name").fill("Ada Lovelace").unwrap();
    app.get_by_test_id("save").click().unwrap();
    app.get_by_test_id("list").wheel(0.0, -45.0).unwrap();
    transcript.push(json!({ "listOffset": app.scroll_offset(5).unwrap() }));
    app.get_by_test_id("drag")
        .drag_by(
            60.0,
            10.0,
            &DragOptions {
                steps: 3.0,
                modifiers: "shift".into(),
                ..DragOptions::default()
            },
        )
        .unwrap();
    transcript.push(tree_json(app));
    transcript.push(json!({ "painted": app.painted_text().unwrap() }));
    transcript.push(json!({ "allText": app.all_text().unwrap() }));
    transcript.push(
        json!({ "selection": app.selected_text().map_err(|e| format!("{:?}", e.code)).err() }),
    );

    // The motion clock.
    transcript.push(json!({
        "set": app.clock().set(2000.0).unwrap(),
        "fast": app.clock().fast_forward(500.0).unwrap(),
        "negative": app.clock().set(-1.0).unwrap_err().code.as_str(),
    }));
    transcript
}

#[test]
fn dev_007_a1_one_scenario_runs_in_process_and_over_stdio_with_equal_results() {
    require_backend!();
    let host = fixture_host(None);
    let in_process = Automation::connect_in_process(host).unwrap();
    let local = scenario(&in_process);

    let launched = launch(&[], None);
    assert_ne!(launched.info().pid, u64::from(std::process::id()));
    let remote = scenario(&launched);
    launched.close();

    assert_eq!(local.len(), remote.len());
    for (index, (local, remote)) in local.iter().zip(&remote).enumerate() {
        assert_eq!(
            local, remote,
            "transcript entry {index} differs between backends"
        );
    }
    // The scenario actually did something: the typed value, the clicks and the drag landed.
    let final_tree = &local[local.len() - 5];
    let rendered = final_tree.to_string();
    assert!(rendered.contains("Ada Lovelace"), "{rendered}");
    assert!(
        rendered.contains("\"received\":[\"mousedown\",\"click\",\"mouseup\"]"),
        "{rendered}"
    );
    assert!(rendered.contains("mousemove"), "{rendered}");
    assert_eq!(local[local.len() - 1]["fast"], 2500.0);
}

#[test]
fn dev_007_a3_fill_with_the_empty_string_selects_all_and_backspaces_the_input_clear() {
    require_backend!();
    let host = fixture_host(None);
    let app = Automation::connect_in_process(host).unwrap();
    let name = app.get_by_test_id("name");
    let value = |app: &Automation| {
        app.get_by_test_id("name").element().unwrap().custom_props["value"].clone()
    };
    name.fill("hello world").unwrap();
    assert_eq!(value(&app), json!("hello world"));
    // Typing again replaces (select all, then type).
    name.fill("again").unwrap();
    assert_eq!(value(&app), json!("again"));
    eprintln!("before clear: {:?}", app.painted_text().unwrap());
    name.fill("").unwrap();
    eprintln!("after clear: {:?}", app.painted_text().unwrap());
    assert_eq!(value(&app), json!(""));
    assert!(!app.painted_text().unwrap().contains(&"again".to_owned()));
    // Press sends keystrokes through the native keyboard path to exactly one element.
    name.press("x y").unwrap();
    assert_eq!(value(&app), json!("xy"));
}

struct Bar;

impl Render for Bar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0xffffff)).child(
            canvas(
                |_, _, _| (),
                |bounds, _, window, cx| {
                    // Motion time drives the painted width: 1 px per 10 ms, up to the full width.
                    let width = (MotionClock::now_ms(cx) / 10.0).min(200.0) as f32;
                    window.paint_quad(fill(
                        Bounds {
                            origin: bounds.origin,
                            size: size(px(width), px(20.0)),
                        },
                        rgb(0xff0000),
                    ));
                },
            )
            .size_full(),
        )
    }
}

#[test]
fn dev_007_a5_motion_clock_semantics_and_deterministic_frame_capture() {
    require_backend!();
    let host = Rc::new(RefCell::new(
        TestHost::with_view(HostOptions::with_size(220.0, 30.0), |_, cx| {
            vespertine::AppContext::new(cx, |_| Bar)
        })
        .unwrap(),
    ));
    let app = Automation::connect_in_process(host.clone()).unwrap();

    let paused = app.clock().pause().unwrap();
    assert!(paused >= 0.0);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        app.clock().pause().unwrap(),
        paused,
        "a frozen clock does not move"
    );
    assert_eq!(app.clock().set(2000.0).unwrap(), 2000.0);
    assert_eq!(app.clock().fast_forward(500.0).unwrap(), 2500.0);
    // Wall time spent paused is not included when the clock resumes.
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(app.clock().resume().unwrap(), 2500.0);
    let later = app.clock().pause().unwrap();
    assert!(
        (2500.0..2880.0).contains(&later),
        "resumed from 2500, got {later}"
    );
    // Every clock operation requests a new frame (the test platform draws it at once).
    let frames = host.borrow().paint_log().frames();
    app.clock().set(1234.0).unwrap();
    assert!(
        host.borrow().paint_log().frames() > frames,
        "the clock change repainted"
    );

    let dir = tempfile::tempdir().unwrap();
    let times = [0.0, 250.0, 500.0];
    let first = app.capture_frames(dir.path().join("a"), &times).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        ["t0.png", "t250.png", "t500.png"]
    );
    let images: Vec<_> = first
        .iter()
        .map(|path| image::open(path).unwrap().to_rgba8())
        .collect();
    assert_eq!(images[0].dimensions(), (440, 60));
    assert_ne!(
        images[0], images[1],
        "an animated element differs between 0 and 250 ms"
    );
    assert_ne!(images[1], images[2], "and between 250 and 500 ms");
    // 250 ms paints a 25 px (50 device px) wide bar.
    assert_eq!(images[1].get_pixel(40, 10).0[..3], [0xff, 0x00, 0x00]);
    assert_eq!(images[1].get_pixel(60, 10).0[..3], [0xff, 0xff, 0xff]);

    // Capturing the same list twice yields identical files.
    let second = app.capture_frames(dir.path().join("b"), &times).unwrap();
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(
            std::fs::read(a).unwrap(),
            std::fs::read(b).unwrap(),
            "{a:?} vs {b:?}"
        );
    }
    assert!(first.iter().all(|path| path.exists()));
}

#[test]
fn dev_007_a6_launch_merges_environment_runs_in_the_directory_and_close_terminates() {
    require_backend!();
    // A variable supplied by the launcher and one inherited from this process both arrive.
    let app = launch(
        &[
            ("FIXTURE_ECHO_ENV", "VESPERTINE_MERGED"),
            ("VESPERTINE_MERGED", "merged-value"),
        ],
        None,
    );
    assert_eq!(
        app.get_by_test_id("env").element().unwrap().text.as_deref(),
        Some("merged-value")
    );
    let info = app.info().clone();
    assert_eq!((info.width, info.height), (400.0, 300.0));
    assert_eq!(info.protocol_version, 1);
    assert!(info.capabilities.len() == 4);

    let path = std::env::var("PATH").unwrap();
    let inherited = launch(&[("FIXTURE_ECHO_ENV", "PATH")], None);
    assert_eq!(
        inherited
            .get_by_test_id("env")
            .element()
            .unwrap()
            .text
            .as_deref(),
        Some(path.as_str())
    );
    inherited.close();

    let directory = tempfile::tempdir().unwrap();
    let in_directory = launch(
        &[("FIXTURE_ECHO_CWD", "1")],
        Some(directory.path().to_owned()),
    );
    let echoed = in_directory
        .get_by_test_id("env")
        .element()
        .unwrap()
        .text
        .unwrap();
    assert_eq!(
        std::fs::canonicalize(&echoed).unwrap(),
        std::fs::canonicalize(directory.path()).unwrap()
    );
    in_directory.close();

    // Closing terminates the owned process and further calls fail with Closed.
    let pid = info.pid;
    app.close();
    assert_eq!(app.tree().unwrap_err().code, ErrorCode::Closed);
    #[cfg(target_os = "linux")]
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "the launched process {pid} must be gone after close"
    );
    let _ = pid;
}
