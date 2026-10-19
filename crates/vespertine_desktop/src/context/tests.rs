//! PLT-013 context query tests (A1 selection cases, A2, A3).

use super::{
    AccessibilityContext, AccessibilitySnapshot, ContextBudgets, ContextQuery, ContextRequest,
    ContextUnavailable,
    fake::{FakeBus, FakeNodeSpec, FakeRequest, ManualClock, NodeId},
};
use futures::future::join;
use std::{
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

const PID: u32 = 4242;
const TITLE: &str = "Doc";

struct Fixture {
    clock: ManualClock,
    bus: FakeBus,
    app: NodeId,
    window: NodeId,
}

fn fixture() -> Fixture {
    let clock = ManualClock::new();
    let bus = FakeBus::new(Arc::new(clock.clone()));
    let app = bus.add_application(":1.7", PID, "Editor");
    let window = bus.add_window(app, TITLE, true);
    Fixture {
        clock,
        bus,
        app,
        window,
    }
}

fn query(fx: &Fixture, request: ContextRequest) -> ContextQuery {
    ContextQuery::with_services(
        Arc::new(fx.bus.clone()),
        Arc::new(fx.clock.clone()),
        request,
    )
}

fn run(fx: &Fixture, request: ContextRequest) -> Result<AccessibilityContext, ContextUnavailable> {
    fx.clock.run(query(fx, request))
}

fn outline(fx: &Fixture) -> AccessibilitySnapshot {
    run(fx, ContextRequest::new(PID, TITLE))
        .expect("query should match")
        .snapshot
}

fn roles_requested(fx: &Fixture) -> usize {
    fx.bus
        .requests()
        .iter()
        .filter(|request| matches!(request, FakeRequest::Role(_)))
        .count()
}

// ---- A1 selection rules --------------------------------------------------

#[test]
fn single_active_window_is_enriched() {
    let fx = fixture();
    fx.bus
        .add_node(fx.window, FakeNodeSpec::new("push button").name("Save"));
    let context = run(&fx, ContextRequest::new(PID, TITLE)).unwrap();
    assert_eq!(context.application_name, "Editor");
    assert_eq!(context.snapshot.version, 1);
    assert_eq!(
        context.snapshot.content,
        "frame: Doc\n  push button: Save\n"
    );
    assert!(!context.snapshot.truncated);
}

#[test]
fn metadata_and_selection_failures_are_screenshot_only() {
    let fx = fixture();
    assert_eq!(
        run(&fx, ContextRequest::new(0, TITLE)),
        Err(ContextUnavailable::NoMatch)
    );
    assert_eq!(
        run(&fx, ContextRequest::new(PID, "")),
        Err(ContextUnavailable::NoMatch)
    );
    assert_eq!(
        run(&fx, ContextRequest::new(PID, " \t\n")),
        Err(ContextUnavailable::NoMatch)
    );
    // Blank metadata never touches the bus.
    assert!(fx.bus.requests().is_empty());

    // Exact, case-sensitive, untrimmed comparison.
    assert_eq!(
        run(&fx, ContextRequest::new(PID, "doc")),
        Err(ContextUnavailable::NoMatch)
    );
    assert_eq!(
        run(&fx, ContextRequest::new(PID, "Doc ")),
        Err(ContextUnavailable::NoMatch)
    );
    // Another process id.
    assert_eq!(
        run(&fx, ContextRequest::new(PID + 1, TITLE)),
        Err(ContextUnavailable::NoMatch)
    );
    for reason in [
        ContextUnavailable::NoMatch,
        ContextUnavailable::Ambiguous,
        ContextUnavailable::Expired,
        ContextUnavailable::Cancelled,
        ContextUnavailable::UnavailableBus,
    ] {
        assert_eq!(reason.screenshot_only(), AccessibilitySnapshot::empty());
    }
    assert_eq!(
        AccessibilitySnapshot::empty(),
        AccessibilitySnapshot {
            version: 1,
            content: String::new(),
            truncated: false
        }
    );
}

#[test]
fn duplicate_titles_are_ambiguous() {
    // Two windows with the same title in one application.
    let fx = fixture();
    fx.bus.add_window(fx.app, TITLE, true);
    assert_eq!(
        run(&fx, ContextRequest::new(PID, TITLE)),
        Err(ContextUnavailable::Ambiguous)
    );

    // Several applications of the same process.
    let fx = fixture();
    let second = fx.bus.add_application(":1.9", PID, "Second");
    fx.bus.add_window(second, TITLE, true);
    assert_eq!(
        run(&fx, ContextRequest::new(PID, TITLE)),
        Err(ContextUnavailable::Ambiguous)
    );

    // The same title in another process does not count.
    let fx = fixture();
    let other = fx.bus.add_application(":1.10", PID + 1, "Other");
    fx.bus.add_window(other, TITLE, true);
    assert!(run(&fx, ContextRequest::new(PID, TITLE)).is_ok());
}

#[test]
fn inactive_window_is_screenshot_only_and_focused_counts() {
    let fx = fixture();
    fx.bus.set_active(fx.window, false);
    assert_eq!(
        run(&fx, ContextRequest::new(PID, TITLE)),
        Err(ContextUnavailable::NoMatch)
    );

    let clock = ManualClock::new();
    let bus = FakeBus::new(Arc::new(clock.clone()));
    let app = bus.add_application(":1.7", PID, "Editor");
    let window = bus.add_node(app, FakeNodeSpec::new("frame").name(TITLE).focused());
    let fx = Fixture {
        clock,
        bus,
        app,
        window,
    };
    assert!(run(&fx, ContextRequest::new(PID, TITLE)).is_ok());
}

#[test]
fn missing_accessibility_bus_is_a_distinct_outcome() {
    let fx = fixture();
    fx.bus.set_connect_fails(true);
    assert_eq!(
        run(&fx, ContextRequest::new(PID, TITLE)),
        Err(ContextUnavailable::UnavailableBus)
    );
}

#[test]
fn process_identity_comes_from_the_bus_connection() {
    let fx = fixture();
    // An application whose display name looks like the pid does not match.
    let app = fx.bus.add_application(":1.9", 7, "4242");
    fx.bus.add_window(app, "Spoof", true);
    assert_eq!(
        run(&fx, ContextRequest::new(PID, "Spoof")),
        Err(ContextUnavailable::NoMatch)
    );
    assert!(
        fx.bus
            .requests()
            .contains(&FakeRequest::ProcessId(":1.9".to_owned()))
    );
}

// ---- A2 traversal limits and line format -------------------------------------

#[test]
fn depth_is_limited_to_24_and_marks_truncation() {
    let fx = fixture();
    let mut parent = fx.window;
    for depth in 1..=30 {
        parent = fx
            .bus
            .add_node(parent, FakeNodeSpec::new("panel").name(format!("d{depth}")));
    }
    let snapshot = outline(&fx);
    assert!(snapshot.truncated);
    let last = snapshot.content.lines().last().unwrap();
    assert_eq!(last, format!("{}panel: d24", "  ".repeat(24)));
    assert!(!snapshot.content.contains("d25"));
    assert_eq!(snapshot.content.lines().count(), 25);
    // Depth 25 nodes are refused rather than visited: window + 24 levels.
    assert_eq!(roles_requested(&fx), 25);
}

#[test]
fn tree_of_exactly_depth_24_is_not_truncated() {
    let fx = fixture();
    let mut parent = fx.window;
    for depth in 1..=24 {
        parent = fx
            .bus
            .add_node(parent, FakeNodeSpec::new("panel").name(format!("d{depth}")));
    }
    let snapshot = outline(&fx);
    assert!(!snapshot.truncated);
    assert_eq!(snapshot.content.lines().count(), 25);
}

#[test]
fn node_limit_counts_visited_nodes() {
    let fx = fixture();
    for index in 0..2000 {
        fx.bus.add_node(
            fx.window,
            FakeNodeSpec::new("label").name(format!("c{index}")),
        );
    }
    let snapshot = outline(&fx);
    assert!(snapshot.truncated);
    assert_eq!(roles_requested(&fx), 1500);
    // The window plus 1499 children produced lines, in order.
    assert_eq!(snapshot.content.lines().count(), 1500);
    assert!(snapshot.content.contains("c1498\n"));
    assert!(!snapshot.content.contains("c1499\n"));
}

#[test]
fn nodes_whose_properties_fail_still_count_as_visited() {
    let fx = fixture();
    for _ in 0..1600 {
        fx.bus
            .add_node(fx.window, FakeNodeSpec::new("label").name("x").failing());
    }
    let snapshot = outline(&fx);
    assert!(snapshot.truncated);
    assert_eq!(roles_requested(&fx), 1500);
    assert_eq!(snapshot.content, "frame: Doc\n");
}

#[test]
fn output_stops_before_the_line_that_would_overflow() {
    let fx = fixture();
    let long = "x".repeat(4000);
    for _ in 0..40 {
        fx.bus
            .add_node(fx.window, FakeNodeSpec::new("label").name(long.clone()));
    }
    let snapshot = outline(&fx);
    let limit = 96 * 1024;
    assert!(snapshot.truncated);
    assert!(snapshot.content.len() <= limit);
    let child_line = 2 + "label: ".len() + 4000 + 1;
    assert!(snapshot.content.len() + child_line > limit);
    assert!(snapshot.content.ends_with('\n'));
    assert!(
        snapshot
            .content
            .lines()
            .skip(1)
            .all(|line| line.len() == child_line - 1)
    );
}

#[test]
fn large_text_is_read_to_4096_characters() {
    let fx = fixture();
    let text = "word ".repeat(2000); // 10,000 characters
    assert_eq!(text.chars().count(), 10_000);
    let node = fx
        .bus
        .add_node(fx.window, FakeNodeSpec::new("text").text(text));
    let snapshot = outline(&fx);
    let object = fx.bus.object(node);
    assert!(fx.bus.requests().contains(&FakeRequest::Text {
        node: object,
        start: 0,
        end: 4096,
    }));
    let line = snapshot.content.lines().nth(1).unwrap();
    let field = line.strip_prefix("  text: ").unwrap();
    assert!(field.chars().count() <= 4096);
    assert_eq!(field.chars().count(), 4096);
}

#[test]
fn password_text_is_never_requested() {
    let fx = fixture();
    let node = fx.bus.add_node(
        fx.window,
        FakeNodeSpec::new("entry")
            .name("Password")
            .text("hunter2")
            .password(),
    );
    let snapshot = outline(&fx);
    assert_eq!(snapshot.content, "frame: Doc\n  password text: Password\n");
    let object = fx.bus.object(node);
    assert!(!snapshot.content.contains("hunter2"));
    for request in fx.bus.requests() {
        match request {
            FakeRequest::Text { node, .. } | FakeRequest::CharacterCount(node) => {
                assert_ne!(node, object, "password text was requested");
            }
            _ => {}
        }
    }
}

#[test]
fn whitespace_runs_collapse_and_fields_follow_the_format() {
    let fx = fixture();
    fx.bus.add_node(
        fx.window,
        FakeNodeSpec::new("push button")
            .name("  Save \t\n  file ")
            .description("Writes\n the   file")
            .text("Save file"),
    );
    // Description equal to the name when trimmed and text equal to the name
    // are omitted.
    fx.bus.add_node(
        fx.window,
        FakeNodeSpec::new("label")
            .name("Same")
            .description(" Same ")
            .text("Same"),
    );
    let snapshot = outline(&fx);
    assert_eq!(
        snapshot.content,
        "frame: Doc\n  push button: Save file | Writes the file\n  label: Same\n"
    );
}

#[test]
fn text_field_is_emitted_when_it_differs_from_the_name() {
    let fx = fixture();
    fx.bus.add_node(
        fx.window,
        FakeNodeSpec::new("entry")
            .name("Query")
            .description("Search box")
            .text("a   b"),
    );
    assert_eq!(
        outline(&fx).content,
        "frame: Doc\n  entry: Query | Search box | a b\n"
    );
}

#[test]
fn nameless_node_emits_no_line_but_children_are_indented() {
    let fx = fixture();
    let panel = fx.bus.add_node(fx.window, FakeNodeSpec::new("panel"));
    fx.bus
        .add_node(panel, FakeNodeSpec::new("push button").name("OK"));
    fx.bus
        .add_node(panel, FakeNodeSpec::new("push button").name("Cancel"));
    assert_eq!(
        outline(&fx).content,
        "frame: Doc\n    push button: OK\n    push button: Cancel\n"
    );
}

#[test]
fn children_are_visited_breadth_first_in_order() {
    let fx = fixture();
    let a = fx
        .bus
        .add_node(fx.window, FakeNodeSpec::new("panel").name("a"));
    fx.bus
        .add_node(fx.window, FakeNodeSpec::new("panel").name("b"));
    fx.bus.add_node(a, FakeNodeSpec::new("label").name("a1"));
    assert_eq!(
        outline(&fx).content,
        "frame: Doc\n  panel: a\n  panel: b\n    label: a1\n"
    );
}

#[test]
fn custom_budgets_apply() {
    let fx = fixture();
    for index in 0..10 {
        fx.bus.add_node(
            fx.window,
            FakeNodeSpec::new("label").name(format!("c{index}")),
        );
    }
    let budgets = ContextBudgets {
        max_nodes: 3,
        ..ContextBudgets::default()
    };
    let snapshot = run(&fx, ContextRequest::new(PID, TITLE).with_budgets(budgets))
        .unwrap()
        .snapshot;
    assert!(snapshot.truncated);
    assert_eq!(snapshot.content, "frame: Doc\n  label: c0\n  label: c1\n");
}

// ---- A3 deadline, cancellation and identity ---------------------------------

#[test]
fn slow_bus_connection_expires_within_the_budget_and_capture_is_not_delayed() {
    let fx = fixture();
    fx.bus.set_connect_latency(Duration::from_secs(5));
    let clock = fx.clock.clone();
    let capture_clock = fx.clock.clone();
    let (capture_time, result) = fx.clock.run(async {
        let capture = async move { capture_clock.elapsed() };
        join(capture, query(&fx, ContextRequest::new(PID, TITLE))).await
    });
    assert_eq!(result, Err(ContextUnavailable::Expired));
    assert_eq!(
        capture_time,
        Duration::ZERO,
        "capture delivered without waiting"
    );
    assert_eq!(clock.elapsed(), Duration::from_millis(900));
    assert_eq!(fx.bus.in_flight(), 0);
}

#[test]
fn slow_requests_expire_and_traversal_is_dropped_at_the_deadline() {
    let fx = fixture();
    fx.bus.set_request_latency(Duration::from_millis(100));
    let mut parent = fx.window;
    for depth in 0..30 {
        parent = fx
            .bus
            .add_node(parent, FakeNodeSpec::new("panel").name(format!("d{depth}")));
    }
    let result = run(&fx, ContextRequest::new(PID, TITLE));
    assert_eq!(result, Err(ContextUnavailable::Expired));
    assert!(fx.clock.elapsed() <= Duration::from_millis(900));
    assert_eq!(fx.bus.in_flight(), 0);
    assert!(fx.bus.is_unlocked());
}

#[test]
fn deadline_budget_is_configurable() {
    let fx = fixture();
    fx.bus.set_connect_latency(Duration::from_millis(200));
    let budgets = ContextBudgets {
        deadline: Duration::from_millis(100),
        ..ContextBudgets::default()
    };
    let result = run(&fx, ContextRequest::new(PID, TITLE).with_budgets(budgets));
    assert_eq!(result, Err(ContextUnavailable::Expired));
    assert_eq!(fx.clock.elapsed(), Duration::from_millis(100));
}

#[test]
fn fast_bus_finishes_within_budget() {
    let fx = fixture();
    fx.bus.set_request_latency(Duration::from_millis(10));
    fx.bus
        .add_node(fx.window, FakeNodeSpec::new("label").name("ok"));
    let result = run(&fx, ContextRequest::new(PID, TITLE)).unwrap();
    assert_eq!(result.snapshot.content, "frame: Doc\n  label: ok\n");
    assert!(fx.clock.elapsed() < Duration::from_millis(900));
}

#[test]
fn hanging_connection_expires_on_the_real_clock() {
    let clock = ManualClock::new();
    let bus = FakeBus::new(Arc::new(clock));
    bus.set_connect_hangs(true);
    let budgets = ContextBudgets {
        deadline: Duration::from_millis(50),
        ..ContextBudgets::default()
    };
    let started = std::time::Instant::now();
    let result = smol::block_on(ContextQuery::with_services(
        Arc::new(bus.clone()),
        Arc::new(super::SystemClock),
        ContextRequest::new(PID, TITLE).with_budgets(budgets),
    ));
    assert_eq!(result, Err(ContextUnavailable::Expired));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(bus.in_flight(), 0);
}

#[test]
fn cancelling_releases_the_query_immediately() {
    let fx = fixture();
    fx.bus.set_connect_hangs(true);
    let mut pending = Box::pin(query(&fx, ContextRequest::new(PID, TITLE)));
    let handle = pending.cancel_handle();
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(pending.as_mut().poll(&mut cx), Poll::Pending));
    assert_eq!(fx.bus.in_flight(), 1);
    assert!(fx.bus.is_unlocked());

    handle.cancel();
    assert!(handle.is_cancelled());
    assert_eq!(
        pending.as_mut().poll(&mut cx),
        Poll::Ready(Err(ContextUnavailable::Cancelled))
    );
    // The work was dropped at cancellation, before the query was dropped.
    assert_eq!(fx.bus.in_flight(), 0);
    drop(pending);
    assert_eq!(fx.clock.pending_timers(), 0);
}

#[test]
fn cancel_wakes_the_task() {
    let fx = fixture();
    fx.bus.set_connect_hangs(true);
    let pending = query(&fx, ContextRequest::new(PID, TITLE));
    let handle = pending.cancel_handle();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        handle.cancel();
    });
    assert_eq!(smol::block_on(pending), Err(ContextUnavailable::Cancelled));
    canceller.join().unwrap();
}

#[test]
fn dropping_the_query_cancels_everything() {
    let fx = fixture();
    fx.bus.set_request_latency(Duration::from_secs(10));
    let mut pending = Box::pin(query(&fx, ContextRequest::new(PID, TITLE)));
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(pending.as_mut().poll(&mut cx).is_pending());
    assert!(fx.bus.in_flight() > 0);
    assert!(fx.clock.pending_timers() > 0);
    drop(pending);
    assert_eq!(fx.bus.in_flight(), 0);
    assert_eq!(fx.clock.pending_timers(), 0);
    assert!(fx.bus.is_unlocked());
}

#[test]
fn window_identity_change_between_selection_and_reselection_yields_no_context() {
    // The window is renamed while the tree is being read.
    let fx = fixture();
    fx.bus
        .add_node(fx.window, FakeNodeSpec::new("label").name("body"));
    let window = fx.window;
    fx.bus.set_request_hook(move |bus, request| {
        if matches!(request, FakeRequest::Description(_)) {
            bus.set_name(window, "Renamed");
        }
    });
    assert_eq!(
        run(&fx, ContextRequest::new(PID, TITLE)),
        Err(ContextUnavailable::NoMatch)
    );

    // The window stops being active while the tree is being read.
    let fx = fixture();
    fx.bus
        .add_node(fx.window, FakeNodeSpec::new("label").name("body"));
    let window = fx.window;
    fx.bus.set_request_hook(move |bus, request| {
        if matches!(request, FakeRequest::Description(_)) {
            bus.set_active(window, false);
        }
    });
    assert_eq!(
        run(&fx, ContextRequest::new(PID, TITLE)),
        Err(ContextUnavailable::NoMatch)
    );
}

#[test]
fn a_second_matching_window_appearing_during_traversal_is_discarded() {
    let fx = fixture();
    let app = fx.app;
    fx.bus
        .add_node(fx.window, FakeNodeSpec::new("label").name("body"));
    let bus_for_hook = fx.bus.clone();
    let added = std::sync::atomic::AtomicBool::new(false);
    fx.bus.set_request_hook(move |_, request| {
        if matches!(request, FakeRequest::Description(_))
            && !added.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            bus_for_hook.add_window(app, TITLE, true);
        }
    });
    assert_eq!(
        run(&fx, ContextRequest::new(PID, TITLE)),
        Err(ContextUnavailable::Ambiguous)
    );
}

#[test]
fn unchanged_identity_returns_the_context() {
    let fx = fixture();
    fx.bus
        .add_node(fx.window, FakeNodeSpec::new("label").name("body"));
    let result = run(&fx, ContextRequest::new(PID, TITLE)).unwrap();
    assert_eq!(result.snapshot.content, "frame: Doc\n  label: body\n");
    // Selection ran twice: before and after the traversal.
    let applications = fx
        .bus
        .requests()
        .iter()
        .filter(|request| matches!(request, FakeRequest::Applications))
        .count();
    assert_eq!(applications, 2);
    assert_eq!(fx.bus.in_flight(), 0);
}
