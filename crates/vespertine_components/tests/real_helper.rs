//! Tests against the real WebKitGTK helper (CMP-022 A2 profile isolation, A8 option menus).
//!
//! They need the helper's runtime (WebKitGTK 4.1) and a display of the session (Wayland or X11),
//! like the example does. A loopback server of the test process serves the pages, since files are
//! rejected by the scheme policy.

mod support;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::rc::Rc;
use std::time::{Duration, Instant};
use support::*;
use vespertine::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use vespertine_components::browser::{
    BodyKind, BrowserContext, BrowserProfile, BrowserSurface, Presentation,
};
use vespertine_webview::HelperConfig;
use vespertine_webview::extract::{EMBEDDED_HELPER, extract_into};
use vespertine_webview::protocol::Command;
use vespertine_webview::{WebPage, WebPageEvent};

const PROFILE_PAGE: &str = "<!doctype html><html><head><meta charset=utf-8>\
    <title>profile page</title></head><body>profile</body></html>";

const SELECT_PAGE: &str = "<!doctype html><html><head><meta charset=utf-8>\
    <title>select page</title></head><body style=\"margin:0\">\
    <select id=\"s\" style=\"font-size:20px;width:200px\">\
    <option value=\"one\">One</option><option value=\"two\">Two</option>\
    <option value=\"three\">Three</option></select></body></html>";

/// Serves `html` for every request on a loopback port until the process exits.
fn serve(html: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let address = listener.local_addr().expect("local address");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Connection: close\r\nContent-Length: {}\r\n\r\n{html}",
                    html.len()
                );
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    address
}

fn pump_for(
    cx: &mut VisualTestContext,
    seconds: u64,
    mut condition: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        cx.run_until_parked();
        if condition(cx) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A window with one surface per context, side by side.
struct Pair {
    surfaces: Vec<Entity<BrowserSurface>>,
}

impl vespertine::Render for Pair {
    fn render(
        &mut self,
        _: &mut vespertine::Window,
        _: &mut vespertine::Context<Self>,
    ) -> impl vespertine::IntoElement {
        use vespertine::{ParentElement, Styled, div};
        div().size_full().flex().children(
            self.surfaces
                .iter()
                .map(|surface| div().flex_1().h_full().child(surface.clone())),
        )
    }
}

struct Session {
    surface: Entity<BrowserSurface>,
    results: Rc<RefCell<Vec<String>>>,
    _subscription: vespertine::Subscription,
}

fn open_sessions(
    cx: &mut TestAppContext,
    contexts: Vec<BrowserContext>,
) -> (Vec<Session>, &mut VisualTestContext) {
    cx.executor().allow_parking();
    let (root, vcx) = cx.add_window_view(move |window, cx| Pair {
        surfaces: contexts
            .into_iter()
            .map(|context| {
                vespertine::AppContext::new(cx, |cx| BrowserSurface::new(context, window, cx))
            })
            .collect(),
    });
    let surfaces = root.read_with(vcx, |root, _| root.surfaces.clone());
    let mut sessions = Vec::new();
    for surface in surfaces {
        vcx.update(|window, app| {
            window.activate_window();
            surface.update(app, |surface, cx| {
                surface.set_presentation(Presentation::Live, window, cx)
            });
        });
        let results = Rc::new(RefCell::new(Vec::new()));
        let sink = results.clone();
        let page: Entity<WebPage> = vcx.update(|_, app| surface.read(app).page().clone());
        let subscription = vcx.update(|_, app| {
            app.subscribe(&page, move |_, event: &WebPageEvent, _| {
                if let WebPageEvent::EvalResult(result) = event {
                    sink.borrow_mut().push(result.clone());
                }
            })
        });
        sessions.push(Session {
            surface,
            results,
            _subscription: subscription,
        });
    }
    (sessions, vcx)
}

impl Session {
    fn navigate(&self, cx: &mut VisualTestContext, input: &str) {
        let input = input.to_owned();
        cx.update(|window, app| {
            self.surface
                .update(app, |surface, cx| surface.navigate(&input, window, cx))
        });
        redraw(cx);
    }

    fn wait_loaded(&self, cx: &mut VisualTestContext, title: &str) -> bool {
        pump_for(cx, 40, |cx| {
            redraw(cx);
            self.surface.read_with(cx, |surface, _| {
                surface.state.title == title && !surface.state.loading
            })
        })
    }

    /// Evaluates `script` in the page and returns its JSON result.
    fn eval(&self, cx: &mut VisualTestContext, script: &str) -> String {
        let before = self.results.borrow().len();
        let page = cx.update(|_, app| self.surface.read(app).page().clone());
        let sent = cx.update(|_, app| {
            page.read(app).send_command(Command::Eval {
                script: script.to_owned(),
            })
        });
        assert!(sent, "the eval command was not accepted");
        assert!(
            pump_for(cx, 30, |_| self.results.borrow().len() > before),
            "no evaluation result for {script}"
        );
        self.results.borrow()[before].clone()
    }
}

/// The embedded helper extracted into a self-deleting directory and a configuration naming it,
/// so that the tests leave no files in the user's cache.
fn helper_config() -> (tempfile::TempDir, HelperConfig) {
    let dir = tempfile::tempdir().expect("temporary helper directory");
    let path = extract_into(dir.path(), EMBEDDED_HELPER).expect("extract the helper");
    let config = HelperConfig {
        helper_path: Some(path),
        ..HelperConfig::default()
    };
    (dir, config)
}

const READ: &str = "document.cookie + '|' + localStorage.getItem('k')";

#[vespertine::test]
fn profile_data_is_isolated_between_profiles_and_survives_closing_every_tab(
    cx: &mut TestAppContext,
) {
    let address = serve(PROFILE_PAGE);
    let site = format!("127.0.0.1:{}", address.port());
    let data = tempfile::tempdir().expect("temporary data directory");
    let (_helper_dir, config) = helper_config();
    let profile_a = BrowserProfile::new(data.path(), &["profile-a"]).expect("profile a");
    let profile_b = BrowserProfile::new(data.path(), &["profile-b"]).expect("profile b");
    assert_ne!(profile_a.root(), profile_b.root());

    {
        // Both profiles run at the same time, each with its own helper.
        let (sessions, cx_both) = open_sessions(
            cx,
            vec![
                BrowserContext::for_profile_with_config(&profile_a, config.clone()),
                BrowserContext::for_profile_with_config(&profile_b, config.clone()),
            ],
        );
        let (a, b) = (&sessions[0], &sessions[1]);
        a.navigate(cx_both, &site);
        b.navigate(cx_both, &site);
        assert!(
            a.wait_loaded(cx_both, "profile page"),
            "profile a did not load"
        );
        assert!(
            b.wait_loaded(cx_both, "profile page"),
            "profile b did not load"
        );
        let written = a.eval(
            cx_both,
            "document.cookie = 'k=v; max-age=3600; path=/'; localStorage.setItem('k', 'v'); 'written'",
        );
        assert_eq!(written, "\"written\"");
        assert_eq!(a.eval(cx_both, READ), "\"k=v|v\"");
        // Profile B sees neither the cookie nor the localStorage item of profile A.
        assert_eq!(b.eval(cx_both, READ), "\"|null\"");
        // Closing the surfaces closes only their pages; removing the window drops them.
        for session in &sessions {
            cx_both.update(|window, app| {
                session
                    .surface
                    .update(app, |surface, cx| surface.close(window, cx))
            });
        }
        drop(sessions);
        cx_both.update(|window, _| window.remove_window());
        cx_both.run_until_parked();
    }

    // After every tab of profile A closed (and its last handle dropped) the helper exits and
    // releases the profile lock; reopening the profile recovers the data. Wait for the lock.
    let (mut again, cx_again) = open_sessions(
        cx,
        vec![BrowserContext::for_profile_with_config(&profile_a, config)],
    );
    let again = again.remove(0);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        again.navigate(cx_again, &site);
        if again.wait_loaded(cx_again, "profile page") {
            break;
        }
        let error = again
            .surface
            .read_with(cx_again, |surface, _| surface.state.error.clone());
        assert!(
            Instant::now() < deadline,
            "profile a could not be reopened: {error:?}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    assert_eq!(
        again.eval(cx_again, READ),
        "\"k=v|v\"",
        "profile a data must be back"
    );
}

#[vespertine::test]
fn an_html_select_opens_a_menu_and_down_enter_picks_the_second_option(cx: &mut TestAppContext) {
    let address = serve(SELECT_PAGE);
    let site = format!("127.0.0.1:{}", address.port());
    let (_helper_dir, config) = helper_config();
    let (mut sessions, cx) = open_sessions(cx, vec![BrowserContext::with_config(config)]);
    let session = sessions.remove(0);
    session.navigate(cx, &site);
    assert!(
        session.wait_loaded(cx, "select page"),
        "the select page did not load"
    );
    // Wait for the first frame so the page has been laid out at its real size.
    assert!(pump_for(cx, 30, |cx| {
        redraw(cx);
        cx.update(|_, app| session.surface.read(app).page().read(app).has_frame())
    }));
    assert_eq!(
        session.eval(cx, "document.getElementById('s').value"),
        "\"one\""
    );
    let body = cx.debug_bounds("browser-body").expect("page body");
    let click = body.origin + point(px(60.), px(14.));
    // The select sits at the page's top left corner; a press and release opens its menu.
    cx.simulate_mouse_move(click, None, Modifiers::none());
    cx.simulate_mouse_down(click, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(click, MouseButton::Left, Modifiers::none());
    assert!(
        pump_for(cx, 20, |cx| session
            .surface
            .read_with(cx, |surface, _| surface.menu().is_some())),
        "the page did not provide a menu"
    );
    let labels: Vec<String> = session.surface.read_with(cx, |surface, _| {
        surface
            .menu()
            .unwrap()
            .items()
            .iter()
            .map(|item| item.label.clone())
            .collect()
    });
    assert_eq!(labels, ["One", "Two", "Three"]);
    redraw(cx);
    cx.simulate_keystrokes("down enter");
    assert!(
        pump_for(cx, 20, |cx| session
            .surface
            .read_with(cx, |surface, _| surface.menu().is_none())),
        "the menu did not close"
    );
    let picked = pump_for(cx, 20, |cx| {
        session.eval(cx, "document.getElementById('s').value") == "\"two\""
    });
    assert!(picked, "the select's value did not change");
    cx.update(|_, app| assert_eq!(session.surface.read(app).body_kind(app), BodyKind::Page));
}
