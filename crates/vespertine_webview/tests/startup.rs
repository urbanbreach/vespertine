//! PLT-014 R15: asynchronous startup of surfaces (latest address wins, close before ready never
//! revives, failure then retry, independent windows) against scripted in-process helpers.

use std::time::{Duration, Instant};
use vespertine::{
    AnyWindowHandle, AppContext, Context, Entity, IntoElement, Render, TestAppContext, Window, div,
};
use vespertine_webview::testing::{ScriptedStartup, wait_until};
use vespertine_webview::{BrowserContext, WebPage};

struct Host {
    page: Entity<WebPage>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A test window with one page, and the handle to run code in it.
#[derive(Clone)]
struct Win {
    page: Entity<WebPage>,
    handle: AnyWindowHandle,
}

fn window(cx: &mut TestAppContext, context: BrowserContext) -> Win {
    let host = cx.add_window(|window, cx| Host {
        page: cx.new(|cx| WebPage::new(context, window, cx)),
    });
    let page = host.read_with(cx, |host, _| host.page.clone()).unwrap();
    Win {
        page,
        handle: host.into(),
    }
}

impl Win {
    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut WebPage, &mut Window, &mut Context<WebPage>) -> R,
    ) -> R {
        let page = self.page.clone();
        cx.update_window(self.handle, move |_, window, app| {
            page.update(app, |page, cx| f(page, window, cx))
        })
        .unwrap()
    }

    fn navigate(&self, cx: &mut TestAppContext, url: &str) -> Duration {
        let started = Instant::now();
        self.update(cx, |page, window, cx| {
            page.navigate_to(url.to_owned(), window, cx)
        });
        started.elapsed()
    }

    fn close(&self, cx: &mut TestAppContext) {
        self.update(cx, |page, window, cx| page.close(window, cx));
    }

    fn reload(&self, cx: &mut TestAppContext) {
        self.update(cx, |page, window, cx| page.reload(window, cx));
    }

    fn snapshot(&self, cx: &mut TestAppContext) -> (bool, Option<String>, bool, bool) {
        self.page.read_with(cx, |page, _| {
            (
                page.state().loading,
                page.state().error.clone(),
                page.has_native_page(),
                page.startup_pending(),
            )
        })
    }
}

fn pump_until(
    cx: &mut TestAppContext,
    mut condition: impl FnMut(&mut TestAppContext) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        cx.run_until_parked();
        if condition(cx) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn loads(startup: &ScriptedStartup) -> Vec<(usize, u32, String)> {
    startup
        .all_named("load")
        .into_iter()
        .map(|(index, page, command)| (index, page, command["url"].as_str().unwrap().to_owned()))
        .collect()
}

#[vespertine::test]
fn two_windows_start_while_the_helper_is_withheld_and_both_load_after_release(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::gated();
    let context = startup.context();
    let a = window(cx, context.clone());
    let b = window(cx, context);
    assert!(a.navigate(cx, "http://a.test/") < Duration::from_secs(1));
    assert!(b.navigate(cx, "http://b.test/") < Duration::from_secs(1));
    // The UI keeps running: the windows still process work while startup is pending.
    cx.run_until_parked();
    assert_eq!(a.snapshot(cx), (true, None, false, true));
    assert_eq!(b.snapshot(cx), (true, None, false, true));
    startup.release();
    assert!(pump_until(cx, |cx| {
        let (a, b) = (a.snapshot(cx), b.snapshot(cx));
        a.2 && !a.3 && b.2 && !b.3
    }));
    assert!(wait_until(Duration::from_secs(5), || loads(&startup).len() == 2));
    let urls: Vec<String> = loads(&startup).into_iter().map(|(_, _, url)| url).collect();
    assert!(
        urls.contains(&"http://a.test/".to_owned()) && urls.contains(&"http://b.test/".to_owned())
    );
    // One helper served both windows.
    assert_eq!(startup.peers().len(), 1);
}

#[vespertine::test]
fn only_the_latest_requested_address_is_loaded_once(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::gated();
    let win = window(cx, startup.context());
    win.navigate(cx, "http://one.test/");
    win.navigate(cx, "http://two.test/");
    win.navigate(cx, "http://three.test/");
    let requested = win
        .page
        .read_with(cx, |p, _| p.requested_address().map(str::to_owned));
    assert_eq!(requested.as_deref(), Some("http://three.test/"));
    startup.release();
    assert!(pump_until(cx, |cx| win.snapshot(cx).2));
    assert!(wait_until(Duration::from_secs(5), || !loads(&startup).is_empty()));
    std::thread::sleep(Duration::from_millis(200));
    let found = loads(&startup);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].2, "http://three.test/");
    assert_eq!(startup.peers().len(), 1, "no second startup began");
}

#[vespertine::test]
fn two_windows_with_other_addresses_log_exactly_their_final_addresses(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::gated();
    let context = startup.context();
    let a = window(cx, context.clone());
    let b = window(cx, context);
    a.navigate(cx, "http://one.test/");
    b.navigate(cx, "http://other.test/");
    a.navigate(cx, "http://two.test/");
    a.navigate(cx, "http://three.test/");
    startup.release();
    assert!(pump_until(cx, |cx| a.snapshot(cx).2 && b.snapshot(cx).2));
    assert!(wait_until(Duration::from_secs(5), || loads(&startup).len() == 2));
    std::thread::sleep(Duration::from_millis(200));
    let mut urls: Vec<String> = loads(&startup).into_iter().map(|(_, _, url)| url).collect();
    urls.sort();
    assert_eq!(urls, ["http://other.test/", "http://three.test/"]);
}

#[vespertine::test]
fn a_startup_failure_is_shown_and_both_retry_paths_succeed(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::new();
    startup.set_failure(Some("Synthetic storage initialization failure"));
    let win = window(cx, startup.context());
    win.navigate(cx, "http://example.test/");
    assert!(pump_until(cx, |cx| !win.snapshot(cx).3));
    let (loading, error, native, pending) = win.snapshot(cx);
    assert!(!loading && !native && !pending);
    let error = error.expect("an error");
    assert!(
        error.contains("Synthetic storage initialization failure"),
        "{error}"
    );
    assert!(error.starts_with("Could not open this page: "));
    // Removing the failure condition and navigating again succeeds.
    startup.set_failure(None);
    win.navigate(cx, "http://example.test/ok");
    assert!(pump_until(cx, |cx| win.snapshot(cx).2));
    let (_, error, native, pending) = win.snapshot(cx);
    assert!(error.is_none() && native && !pending);
}

#[vespertine::test]
fn the_retry_control_restarts_a_failed_startup_with_the_current_address(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::new();
    startup.set_failure(Some("Synthetic storage initialization failure"));
    let win = window(cx, startup.context());
    win.navigate(cx, "http://example.test/again");
    assert!(pump_until(cx, |cx| !win.snapshot(cx).3));
    assert!(win.snapshot(cx).1.is_some());
    // Invoking the retry control while no native page exists starts a new startup.
    startup.set_failure(None);
    win.reload(cx);
    assert!(win.snapshot(cx).3, "a startup is pending again");
    assert!(pump_until(cx, |cx| win.snapshot(cx).2));
    let (_, error, native, pending) = win.snapshot(cx);
    assert!(error.is_none() && native && !pending);
    assert!(wait_until(Duration::from_secs(5), || {
        loads(&startup)
            .iter()
            .any(|(_, _, url)| url == "http://example.test/again")
    }));
}

#[vespertine::test]
fn closing_during_startup_never_revives_the_page(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::gated();
    let win = window(cx, startup.context());
    win.navigate(cx, "http://abandoned.test/");
    let started = Instant::now();
    win.close(cx);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "closing never waits for startup"
    );
    cx.run_until_parked();
    startup.release();
    // The helper receives a close for the page the startup thread created.
    let closed = wait_until(Duration::from_secs(5), || {
        let peers = startup.peers();
        peers
            .first()
            .is_some_and(|peer| !peer.named_for(1, "close").is_empty())
    });
    assert!(
        closed,
        "{:?}",
        startup
            .peers()
            .iter()
            .map(|p| p.commands())
            .collect::<Vec<_>>()
    );
    cx.run_until_parked();
    let (loading, _, native, pending) = win.snapshot(cx);
    assert!(
        !native && !pending && !loading,
        "the closed surface stays closed"
    );
    assert!(
        loads(&startup).is_empty(),
        "the abandoned address was never loaded"
    );
    // A later navigation starts afresh and loads only its own address.
    win.navigate(cx, "http://fresh.test/");
    assert!(pump_until(cx, |cx| win.snapshot(cx).2));
    assert!(wait_until(Duration::from_secs(5), || !loads(&startup).is_empty()));
    let found = loads(&startup);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2, "http://fresh.test/");
}

#[vespertine::test]
fn the_presentation_is_applied_before_the_address_loads(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::gated();
    let win = window(cx, startup.context());
    // A surface created without a presentation is hidden: the hidden status is applied to the
    // new page before its address loads.
    win.navigate(cx, "http://example.test/");
    startup.release();
    assert!(pump_until(cx, |cx| win.snapshot(cx).2));
    assert!(wait_until(Duration::from_secs(5), || !loads(&startup).is_empty()));
    let peer = startup.peers().remove(0);
    assert_eq!(peer.named("visible")[0]["value"], 0);
    let names: Vec<String> = peer
        .commands()
        .into_iter()
        .map(|(_, command)| command["cmd"].as_str().unwrap().to_owned())
        .collect();
    let create = names.iter().position(|n| n == "create").unwrap();
    let visible = names.iter().position(|n| n == "visible").unwrap();
    let load = names.iter().position(|n| n == "load").unwrap();
    assert!(create < visible && visible < load, "{names:?}");
    // Exactly one create for the new page.
    assert_eq!(names.iter().filter(|n| *n == "create").count(), 1);
}

#[vespertine::test]
fn a_failed_helper_is_restarted_through_the_retry_control(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let startup = ScriptedStartup::new();
    let win = window(cx, startup.context());
    win.navigate(cx, "http://example.test/");
    assert!(pump_until(cx, |cx| win.snapshot(cx).2));
    // The helper dies: the page shows the helper-stopped error and is not loading.
    startup.peers()[0].kill_channel();
    assert!(pump_until(cx, |cx| win.snapshot(cx).1.is_some()));
    let (loading, error, _, _) = win.snapshot(cx);
    assert!(!loading);
    assert!(error.unwrap().starts_with("The browser helper stopped"));
    // The retry control restarts startup with the current address and spawns a new helper.
    win.reload(cx);
    assert!(pump_until(cx, |cx| {
        let (_, error, native, pending) = win.snapshot(cx);
        error.is_none() && native && !pending
    }));
    assert_eq!(startup.peers().len(), 2);
    assert!(wait_until(Duration::from_secs(5), || {
        startup.peers()[1]
            .named("load")
            .iter()
            .any(|c| c["url"] == "http://example.test/")
    }));
}
