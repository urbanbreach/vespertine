//! PLT-014 lifecycle tests against the scripted synthetic helper: handshake validation, startup
//! rules, failure, hang detection, shutdown and restart.

mod common;

use common::{Synth, process_exists, wait_until};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};
use vespertine_webview::protocol::{Command, Direction, encode_message, kind, write_all};
use vespertine_webview::state::{ERROR_HELPER_HUNG, ERROR_HELPER_STOPPED};
use vespertine_webview::{BrowserContext, Helper, HelperConfig, PageEvent};

fn start(context: &BrowserContext) -> vespertine_webview::NativePage {
    let receiver = context.start_page();
    receiver
        .recv_blocking()
        .expect("startup delivers a result")
        .expect("startup succeeds")
}

fn attach_script(script: impl FnOnce(UnixStream) + Send + 'static) -> Result<Helper, String> {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let thread = std::thread::spawn(move || script(theirs));
    let result = Helper::attach(ours, None, Instant::now(), HelperConfig::default());
    let _ = thread.join();
    result
}

fn error_of(result: Result<Helper, String>) -> String {
    match result {
        Ok(_) => panic!("startup unexpectedly succeeded"),
        Err(message) => message,
    }
}

fn send(socket: &UnixStream, kind: u8, page: u32, payload: &[u8]) {
    let bytes = encode_message(Direction::ToApplication, kind, page, payload).unwrap();
    write_all(socket.as_raw_fd(), &bytes).unwrap();
}

// ---- PLT-014 A5: startup response validation --------------------------------------------------

#[test]
fn startup_responses_are_validated() {
    // Shorter than a header.
    let message = error_of(attach_script(|mut socket| {
        socket.write_all(&[b'R', 0, 0, 0, 0]).unwrap();
    }));
    assert_eq!(message, "Invalid browser startup response");
    // Nonzero page identifier.
    let message = error_of(attach_script(|socket| send(&socket, kind::READY, 3, b"")));
    assert_eq!(message, "Invalid browser startup response");
    // Payload length 0xFFFFFFFF.
    let message = error_of(attach_script(|mut socket| {
        socket
            .write_all(&[b'R', 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF])
            .unwrap();
    }));
    assert_eq!(message, "Invalid browser startup response");
    // Neither ready nor error.
    let message = error_of(attach_script(|socket| send(&socket, kind::PONG, 0, b"{}")));
    assert_eq!(message, "Invalid browser startup response");
    // A ready payload that is not valid, and one with a newer protocol.
    let message = error_of(attach_script(|socket| {
        send(&socket, kind::READY, 0, b"[1]")
    }));
    assert_eq!(message, "Invalid browser startup response");
    let message = error_of(attach_script(|socket| {
        send(&socket, kind::READY, 0, br#"{"protocol":2}"#)
    }));
    assert_eq!(
        message,
        "Browser helper protocol version 2 is not supported"
    );
    // An error message yields exactly its text.
    let message = error_of(attach_script(|socket| {
        send(&socket, kind::ERROR, 0, b"locked")
    }));
    assert_eq!(message, "locked");
    // Closing before the first message.
    let message = error_of(attach_script(|_socket| {}));
    assert!(
        message.starts_with("Browser helper could not start: ")
            && message.ends_with("Check the WebKitGTK 4.1 runtime and display."),
        "{message}"
    );
    // An empty ready is accepted as protocol 1 with inline transfer only.
    let helper = attach_script(|socket| {
        send(&socket, kind::READY, 0, b"");
        // Keep the channel open until the application is done with it.
        let mut buffer = [0u8; 1];
        let _ = std::io::Read::read(&mut &socket, &mut buffer);
    })
    .unwrap();
    assert_eq!(helper.ready().protocol, 1);
    assert!(helper.ready().capabilities.is_empty());
    assert_eq!(
        helper.transfer(),
        vespertine_webview::protocol::Transfer::Inline
    );
}

#[test]
fn a_silent_helper_is_killed_and_reaped_after_the_deadline() {
    let synth = Synth::new().with("VSP_SYNTH_MODE", "silent");
    let context = synth.context();
    let started = Instant::now();
    let result = context.start_page().recv_blocking().unwrap();
    let message = match result {
        Ok(_) => panic!("a silent helper must not start"),
        Err(message) => message,
    };
    assert!(
        message.starts_with("Browser startup did not complete"),
        "{message}"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(10) && started.elapsed() < Duration::from_secs(14)
    );
    let pid = synth.started_pids()[0];
    assert!(!process_exists(pid), "the silent helper was reaped");
}

#[test]
fn an_error_message_at_startup_is_reported_verbatim_and_retry_succeeds() {
    let synth = Synth::new();
    let fail_file = synth.dir.path().join("fail");
    std::fs::write(&fail_file, b"x").unwrap();
    let synth = synth.with("VSP_SYNTH_FAIL", fail_file.display().to_string());
    let context = synth.context();
    let error = match context.start_page().recv_blocking().unwrap() {
        Ok(_) => panic!("must fail"),
        Err(error) => error,
    };
    assert_eq!(error, "Synthetic storage initialization failure");
    std::fs::remove_file(&fail_file).unwrap();
    let page = start(&context);
    assert!(page.id() >= 1);
    // The failed helper process is not reused: a new helper was spawned.
    assert_eq!(synth.started_pids().len(), 2);
}

#[test]
fn spawn_failure_names_the_missing_runtime() {
    let config = HelperConfig {
        helper_path: Some("/nonexistent/helper".into()),
        ..HelperConfig::default()
    };
    let message = BrowserContext::with_config(config)
        .start_page()
        .recv_blocking()
        .unwrap()
        .err()
        .expect("spawn fails");
    assert!(
        message.starts_with("Could not start WebKitGTK: "),
        "{message}"
    );
    assert!(message.ends_with("Install the WebKitGTK 4.1 runtime for your distribution."));
}

// ---- PLT-014 R11, R15, R16: sharing, pages, close --------------------------------------------

#[test]
fn pages_of_one_context_share_one_helper_and_get_increasing_ids() {
    let synth = Synth::new();
    let context = synth.context();
    let first = start(&context);
    let second = start(&context);
    assert_eq!(first.helper().pid(), second.helper().pid());
    assert_eq!((first.id(), second.id()), (1, 2));
    let first_id = first.id();
    drop(first);
    let third = start(&context);
    // Identifiers are never reused during the life of one helper.
    assert_eq!(third.id(), 3);
    assert_ne!(third.id(), first_id);
    assert_eq!(synth.started_pids().len(), 1);
    assert!(wait_until(Duration::from_secs(5), || {
        synth
            .commands()
            .iter()
            .any(|(page, cmd)| *page == first_id && cmd["cmd"] == "close")
    }));
    drop(second);
    drop(third);
    // The helper stays alive between tabs while the context lives.
    assert!(context.helper_pid().is_some());
    let pid = context.helper_pid().unwrap();
    assert!(process_exists(pid));
    drop(context);
    assert!(wait_until(Duration::from_secs(5), || !process_exists(pid)));
}

#[test]
fn abandoned_startup_closes_the_created_page() {
    let synth = Synth::new().gated();
    let context = synth.context();
    let receiver = context.start_page();
    let began = Instant::now();
    drop(receiver);
    assert!(began.elapsed() < Duration::from_secs(1));
    synth.release();
    assert!(wait_until(Duration::from_secs(5), || {
        let commands = synth.commands();
        commands
            .iter()
            .any(|(page, cmd)| *page == 1 && cmd["cmd"] == "create")
            && commands
                .iter()
                .any(|(page, cmd)| *page == 1 && cmd["cmd"] == "close")
    }));
}

#[test]
fn startups_in_two_threads_share_a_gated_helper() {
    let synth = Synth::new().gated();
    let context = synth.context();
    let a = context.start_page();
    let b = context.start_page();
    std::thread::sleep(Duration::from_millis(200));
    assert!(a.try_recv().is_err() && b.try_recv().is_err());
    synth.release();
    let a = a.recv_blocking().unwrap().unwrap();
    let b = b.recv_blocking().unwrap().unwrap();
    assert_eq!(a.helper().pid(), b.helper().pid());
    assert_eq!(synth.started_pids().len(), 1);
}

#[test]
fn commands_reach_the_helper_in_order_and_state_comes_back() {
    let synth = Synth::new();
    let page = start(&synth.context());
    let events = page.events();
    page.present(vespertine_webview::Presentation::Live);
    page.load("http://example.test/").unwrap();
    // Wait for the final state.
    assert!(wait_until(Duration::from_secs(5), || {
        let state = page.state();
        !state.loading && state.title == "Synthetic http://example.test/"
    }));
    assert!(matches!(events.try_recv(), Ok(PageEvent::StateChanged)));
    let commands: Vec<String> = synth
        .commands()
        .into_iter()
        .map(|(_, cmd)| cmd["cmd"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(commands[..2], ["configure".to_owned(), "create".to_owned()]);
    assert!(commands.contains(&"load".to_owned()));
}

#[test]
fn resize_is_sent_only_when_the_device_size_or_scale_changes() {
    let synth = Synth::new();
    let page = start(&synth.context());
    assert!(page.sync(640.0, 360.0, 1.5));
    assert!(!page.sync(640.0, 360.0, 1.5));
    assert!(page.sync(640.0, 360.0, 1.5000001));
    assert!(page.sync(640.0, 361.0, 1.5000001));
    assert!(page.sync(640.0, 361.0, 2.0));
    assert!(!page.sync(640.0, 361.0, 2.0));
    // The exact wire example: a 640 x 360 logical page at scale 1.5 asks for 960 x 540.
    // Consecutive queued resizes of one page collapse into the latest, so the helper sees at most
    // four and always ends with the final size.
    assert!(wait_until(Duration::from_secs(5), || {
        synth
            .commands()
            .iter()
            .any(|(_, c)| c["cmd"] == "resize" && c["width"] == 1280)
    }));
    let resizes: Vec<_> = synth
        .commands()
        .into_iter()
        .filter(|(_, c)| c["cmd"] == "resize")
        .map(|(_, c)| c)
        .collect();
    assert!(resizes.len() <= 4);
    let last = resizes.last().unwrap();
    assert_eq!(
        (last["width"].as_u64(), last["height"].as_u64()),
        (Some(1280), Some(722))
    );
    assert_eq!(last["scale"], 2.0);
}

#[test]
fn visible_is_sent_only_when_the_hidden_status_changes() {
    use vespertine_webview::Presentation::*;
    let synth = Synth::new();
    let page = start(&synth.context());
    page.present(Live);
    page.present(Passthrough);
    page.present(Hidden);
    page.present(Hidden);
    page.present(Live);
    assert!(wait_until(Duration::from_secs(5), || {
        synth
            .commands()
            .iter()
            .filter(|(_, c)| c["cmd"] == "visible")
            .count()
            == 2
    }));
    std::thread::sleep(Duration::from_millis(100));
    let values: Vec<_> = synth
        .commands()
        .into_iter()
        .filter(|(_, c)| c["cmd"] == "visible")
        .map(|(_, c)| c["value"].as_i64().unwrap())
        .collect();
    assert_eq!(values, [0, 1]);
}

// ---- PLT-014 A9: shutdown ---------------------------------------------------------------------

#[test]
fn dropping_the_last_handle_closes_the_channel_and_the_helper_exits() {
    let synth = Synth::new();
    let context = synth.context();
    let page = start(&context);
    let pid = page.helper().pid();
    drop(page);
    drop(context);
    assert!(wait_until(Duration::from_secs(5), || !process_exists(pid)));
}

#[test]
fn a_helper_that_ignores_end_of_file_is_killed_after_the_grace_period() {
    let synth = Synth::new().with("VSP_SYNTH_MODE", "ignore-eof");
    let context = synth.context();
    let page = start(&context);
    let pid = page.helper().pid();
    drop(page);
    let closed = Instant::now();
    drop(context);
    // Still alive shortly after the channel closed...
    std::thread::sleep(Duration::from_millis(1500));
    assert!(process_exists(pid));
    // ...and gone after the 3 second grace period.
    assert!(wait_until(Duration::from_secs(5), || !process_exists(pid)));
    assert!(
        closed.elapsed() >= Duration::from_millis(2900),
        "{:?}",
        closed.elapsed()
    );
}

// ---- PLT-014 A10: failure ---------------------------------------------------------------------

#[test]
fn killing_the_helper_fails_every_page_and_retry_spawns_a_new_helper() {
    let synth = Synth::new();
    let context = synth.context();
    let first = start(&context);
    let second = start(&context);
    let first_events = first.events();
    let second_events = second.events();
    let old_pid = first.helper().pid();
    first.helper().kill_for_tests();
    for (page, events) in [(&first, &first_events), (&second, &second_events)] {
        assert!(wait_until(Duration::from_secs(5), || {
            page.state().error.as_deref() == Some(ERROR_HELPER_STOPPED)
        }));
        assert!(!page.state().loading);
        assert!(
            events.try_recv().is_ok(),
            "a state-changed event is emitted"
        );
    }
    assert!(first.command(Command::Reload).is_err());
    // Retrying one page: a new helper and a new page, loading its address.
    drop(first);
    let retried = start(&context);
    assert_ne!(retried.helper().pid(), old_pid);
    retried.load("http://example.test/a").unwrap();
    assert!(wait_until(Duration::from_secs(5), || retried
        .state()
        .title
        .contains("example.test/a")));
    // The other page is unaffected by that retry until it is retried too.
    assert_eq!(second.state().error.as_deref(), Some(ERROR_HELPER_STOPPED));
    assert_eq!(second.helper().pid(), old_pid);
    assert_eq!(synth.started_pids().len(), 2);
}

// ---- PLT-014 A11: hang detection --------------------------------------------------------------

#[test]
fn an_unresponsive_helper_is_detected_with_default_timings() {
    let synth = Synth::new().with("VSP_SYNTH_MODE", "no-pong");
    let context = synth.context();
    let page = start(&context);
    let pid = page.helper().pid();
    let began = Instant::now();
    assert!(wait_until(Duration::from_secs(12), || {
        page.state().error.as_deref() == Some(ERROR_HELPER_HUNG)
    }));
    let elapsed = began.elapsed();
    assert!(
        elapsed >= Duration::from_secs(7) && elapsed <= Duration::from_secs(10),
        "{elapsed:?}"
    );
    assert!(!page.state().loading);
    assert!(
        wait_until(Duration::from_secs(2), || !process_exists(pid)),
        "the hung helper was killed"
    );
}

#[test]
fn a_helper_that_answers_pings_is_never_declared_hung() {
    let synth = Synth::new();
    let mut config = synth.config();
    config.timings.ping_interval = Duration::from_millis(50);
    config.timings.hang_deadline = Duration::from_millis(400);
    let context = BrowserContext::with_config(config);
    let page = start(&context);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(page.state().error.is_none());
    let pings = synth
        .commands()
        .iter()
        .filter(|(_, c)| c["cmd"] == "ping")
        .count();
    assert!(pings >= 10, "{pings} pings");
    // Nonces increase.
    let nonces: Vec<_> = synth
        .commands()
        .into_iter()
        .filter(|(_, c)| c["cmd"] == "ping")
        .map(|(_, c)| c["nonce"].as_u64().unwrap())
        .collect();
    assert!(nonces.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn writes_never_block_the_caller_even_when_the_helper_is_stopped() {
    let synth = Synth::new();
    let context = synth.context();
    let page = start(&context);
    let pid = page.helper().pid();
    // SAFETY: stopping a child process we spawned.
    unsafe { libc::kill(pid as i32, libc::SIGSTOP) };
    let text = "x".repeat(200_000);
    let began = Instant::now();
    let mut failed = false;
    for _ in 0..60 {
        let call = Instant::now();
        let result = page.command(Command::Text(text.clone()));
        assert!(
            call.elapsed() < Duration::from_millis(500),
            "a command blocked the caller for {:?}",
            call.elapsed()
        );
        if result.is_err() {
            failed = true;
            break;
        }
    }
    assert!(
        failed,
        "the bounded queue overflowed and the helper was declared hung"
    );
    assert!(began.elapsed() < Duration::from_secs(10));
    assert_eq!(page.state().error.as_deref(), Some(ERROR_HELPER_HUNG));
    // SAFETY: continue/kill our own child so it can be reaped.
    unsafe { libc::kill(pid as i32, libc::SIGCONT) };
    assert!(wait_until(Duration::from_secs(5), || !process_exists(pid)));
}

#[test]
fn the_helper_started_for_a_profile_is_shared_and_replaced_when_it_dies() {
    let synth = Synth::new();
    let data = tempfile::tempdir().unwrap();
    let profile = vespertine_webview::BrowserProfile::new(data.path(), &["restart"]).unwrap();
    let context = BrowserContext::for_profile_with_config(&profile, synth.config());
    let same = BrowserContext::for_profile_with_config(&profile, synth.config());
    assert!(context.same_context(&same));
    let page = start(&context);
    let first_pid = page.helper().pid();
    let other = start(&same);
    assert_eq!(other.helper().pid(), first_pid);
    // The registered helper's child exits: the next request spawns a fresh helper.
    page.helper().kill_for_tests();
    assert!(wait_until(Duration::from_secs(5), || page
        .state()
        .error
        .is_some()));
    let replacement = start(&context);
    assert_ne!(replacement.helper().pid(), first_pid);
}

#[test]
fn a_640_by_360_page_at_scale_1_5_resizes_to_960_by_540() {
    let synth = Synth::new();
    let page = start(&synth.context());
    page.sync(640.0, 360.0, 1.5);
    assert!(wait_until(Duration::from_secs(5), || {
        synth.commands().iter().any(|(_, c)| {
            c["cmd"] == "resize" && c["width"] == 960 && c["height"] == 540 && c["scale"] == 1.5
        })
    }));
}
