//! CMP-022 A2 (scripted part): asynchronous startup rules seen through the surface, and A7: close
//! and presentation behaviour.

#![cfg(target_os = "linux")]

mod support;

use std::time::Duration;
use support::*;
use vespertine::TestAppContext;
use vespertine_components::browser::{BodyKind, Presentation};
use vespertine_webview::testing::{ScriptedStartup, wait_until};

#[vespertine::test]
fn latest_address_wins_during_startup(cx: &mut TestAppContext) {
    let startup = ScriptedStartup::gated();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    for input in ["one.test", "two.test", "three.test"] {
        let started = std::time::Instant::now();
        assert!(fixture.navigate(cx, input));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "navigate must not block"
        );
    }
    // Startup is pending: loading, no native page, the "starting" placeholder.
    fixture.read(cx, |surface| {
        assert!(surface.state.loading);
        assert_eq!(surface.state.url.as_deref(), Some("https://three.test/"));
    });
    cx.update(|_, app| {
        let page = fixture.surface.read(app).page().read(app);
        assert!(page.startup_pending() && !page.has_native_page());
    });
    cx.update(|_, app| assert_eq!(fixture.surface.read(app).body_kind(app), BodyKind::Starting));
    startup.release();
    assert!(pump_until(cx, |_| !startup.all_named("load").is_empty()));
    settle(cx);
    // Exactly one load, carrying the latest address.
    let loads = startup.all_named("load");
    assert_eq!(loads.len(), 1, "{loads:?}");
    assert_eq!(loads[0].2["url"], "https://three.test/");
    assert_eq!(startup.peers().len(), 1);
    cx.update(|_, app| assert_eq!(fixture.surface.read(app).body_kind(app), BodyKind::Page));
}

#[vespertine::test]
fn closing_during_startup_never_revives_the_surface(cx: &mut TestAppContext) {
    let startup = ScriptedStartup::gated();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    assert!(fixture.navigate(cx, "one.test"));
    let started = std::time::Instant::now();
    fixture.call(cx, |surface, window, cx| surface.close(window, cx));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "close must not block"
    );
    startup.release();
    // The page the late startup created is closed on the helper.
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        !startup.all_named("close").is_empty()
    }));
    settle(cx);
    cx.update(|_, app| {
        let page = fixture.surface.read(app).page().read(app);
        assert!(!page.has_native_page() && !page.startup_pending());
    });
    assert!(
        startup.all_named("load").is_empty(),
        "the abandoned address must never load"
    );
    // A later navigation starts afresh and loads only its own address.
    assert!(fixture.navigate(cx, "two.test"));
    assert!(pump_until(cx, |_| !startup.all_named("load").is_empty()));
    settle(cx);
    let loads = startup.all_named("load");
    assert_eq!(loads.len(), 1, "{loads:?}");
    assert_eq!(loads[0].2["url"], "https://two.test/");
}

#[vespertine::test]
fn startup_failure_is_retryable_through_navigate(cx: &mut TestAppContext) {
    let startup = scripted();
    startup.set_failure(Some("Synthetic storage initialization failure"));
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    assert!(fixture.navigate(cx, "one.test"));
    assert!(pump_until(cx, |cx| fixture
        .read(cx, |surface| surface.state.error.is_some())));
    fixture.read(cx, |surface| {
        let error = surface.state.error.clone().unwrap();
        assert!(
            error.contains("Synthetic storage initialization failure"),
            "{error}"
        );
        assert!(!surface.state.loading);
    });
    cx.update(|_, app| {
        let surface = fixture.surface.read(app);
        let page = surface.page().read(app);
        assert!(!page.has_native_page() && !page.startup_pending());
        assert_eq!(surface.body_kind(app), BodyKind::Error);
    });
    startup.set_failure(None);
    assert!(fixture.navigate(cx, "one.test"));
    assert!(pump_until(cx, |_| !startup.all_named("load").is_empty()));
    settle(cx);
    fixture.read(cx, |surface| assert!(surface.state.error.is_none()));
    cx.update(|_, app| assert!(fixture.surface.read(app).page().read(app).has_native_page()));
}

#[vespertine::test]
fn startup_failure_is_retryable_through_the_retry_button(cx: &mut TestAppContext) {
    let startup = scripted();
    startup.set_failure(Some("Synthetic storage initialization failure"));
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    assert!(fixture.navigate(cx, "one.test"));
    assert!(pump_until(cx, |cx| fixture
        .read(cx, |surface| surface.state.error.is_some())));
    redraw(cx);
    cx.update(|_, app| {
        let placeholder = fixture.surface.read(app).placeholder(app).unwrap();
        assert_eq!(placeholder.title, "Couldn’t load this page");
        assert!(
            placeholder
                .description
                .contains("Synthetic storage initialization failure")
        );
    });
    // Remove the failure condition and press "Try again": startup restarts with the address.
    startup.set_failure(None);
    fixture.click(cx, "browser-placeholder-action");
    assert!(pump_until(cx, |_| !startup.all_named("load").is_empty()));
    settle(cx);
    let loads = startup.all_named("load");
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0].2["url"], "https://one.test/");
    fixture.read(cx, |surface| assert!(surface.state.error.is_none()));
}

#[vespertine::test]
fn two_surfaces_in_two_windows_load_independent_addresses_with_one_helper(cx: &mut TestAppContext) {
    use vespertine::AppContext;
    use vespertine_components::browser::BrowserSurface;
    cx.executor().allow_parking();
    let startup = scripted();
    let context = startup.context();
    let mut windows = Vec::new();
    for address in ["left.test", "right.test"] {
        let context = context.clone();
        let window = cx.add_window(move |window, cx| {
            let surface = cx.new(|cx| BrowserSurface::new(context, window, cx));
            support::Root {
                surface,
                navigation_presses: Default::default(),
            }
        });
        window
            .update(cx, |root, window, cx| {
                root.surface.update(cx, |surface, cx| {
                    surface.set_presentation(Presentation::Live, window, cx);
                    assert!(surface.navigate(address, window, cx));
                });
            })
            .unwrap();
        windows.push(window);
    }
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        startup.all_named("load").len() >= 2
    }));
    let loads = startup.all_named("load");
    let mut urls: Vec<_> = loads
        .iter()
        .map(|(_, _, command)| command["url"].as_str().unwrap().to_owned())
        .collect();
    urls.sort();
    assert_eq!(urls, ["https://left.test/", "https://right.test/"]);
    // One helper serves both windows; each page has its own id.
    assert_eq!(startup.peers().len(), 1);
    let mut pages: Vec<_> = loads.iter().map(|(_, page, _)| *page).collect();
    pages.sort();
    pages.dedup();
    assert_eq!(pages.len(), 2);
    // Each surface reports its own address.
    let mut seen = Vec::new();
    for window in &windows {
        window
            .read_with(cx, |root, cx| {
                seen.push(root.surface.read(cx).state.url.clone().unwrap());
            })
            .unwrap();
    }
    assert_eq!(seen, ["https://left.test/", "https://right.test/"]);
}

#[vespertine::test]
fn close_leaves_no_native_page_and_no_pending_startup(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "one.test");
    cx.update(|_, app| assert!(fixture.surface.read(app).page().read(app).has_native_page()));
    fixture.call(cx, |surface, window, cx| surface.close(window, cx));
    peer.wait_named("close", 1);
    cx.update(|_, app| {
        let surface = fixture.surface.read(app);
        let page = surface.page().read(app);
        assert!(!page.has_native_page() && !page.startup_pending() && !page.has_frame());
        assert_eq!(surface.presentation(app), Presentation::Hidden);
    });
    // A later navigation starts again.
    fixture.navigate(cx, "again.test");
    assert!(pump_until(cx, |_| startup.all_named("load").len() == 2));
    assert_eq!(startup.all_named("load")[1].2["url"], "https://again.test/");
}

#[vespertine::test]
fn menus_cannot_appear_in_non_live_presentations(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "one.test");
    for presentation in [Presentation::Passthrough, Presentation::Hidden] {
        fixture.call(cx, |surface, window, cx| {
            surface.set_presentation(presentation, window, cx)
        });
        let dismissals = peer.named("dismiss-menu").len();
        send_menu(
            &peer,
            10.,
            10.,
            serde_json::json!([item("Copy", "copy", true, false)]),
        );
        assert!(pump_until(cx, |_| peer.named("dismiss-menu").len() > dismissals));
        settle(cx);
        fixture.read(cx, |surface| {
            assert!(surface.menu().is_none(), "{presentation:?}")
        });
    }
    // The visible command is sent only when the hidden status changes.
    peer.wait_named("visible", 1);
    // A pending menu is dismissed when the surface stops being live.
    fixture.call(cx, |surface, window, cx| {
        surface.set_presentation(Presentation::Live, window, cx)
    });
    send_menu(
        &peer,
        10.,
        10.,
        serde_json::json!([item("Copy", "copy", true, false)]),
    );
    assert!(pump_until(cx, |cx| fixture
        .read(cx, |surface| surface.menu().is_some())));
    let dismissals = peer.named("dismiss-menu").len();
    fixture.call(cx, |surface, window, cx| {
        surface.set_presentation(Presentation::Hidden, window, cx)
    });
    fixture.read(cx, |surface| assert!(surface.menu().is_none()));
    assert!(pump_until(cx, |_| peer.named("dismiss-menu").len() > dismissals));
}

#[vespertine::test]
fn page_new_tab_requests_are_re_emitted_only_while_live(cx: &mut TestAppContext) {
    use vespertine_components::browser::BrowserEvent;
    use vespertine_webview::protocol::kind;
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "one.test");
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = events.clone();
    let _subscription = cx.update(|_, app| {
        app.subscribe(&fixture.surface, move |_, event: &BrowserEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
    });
    peer.send(kind::NEW_TAB, 1, b"http://example.test/");
    assert!(pump_until(cx, |_| {
        events
            .borrow()
            .contains(&BrowserEvent::NewTab(Some("http://example.test/".into())))
    }));
    events.borrow_mut().clear();
    fixture.call(cx, |surface, window, cx| {
        surface.set_presentation(Presentation::Passthrough, window, cx)
    });
    peer.send(kind::NEW_TAB, 1, b"http://other.test/");
    settle(cx);
    assert!(
        !events
            .borrow()
            .iter()
            .any(|event| matches!(event, BrowserEvent::NewTab(_)))
    );
}
