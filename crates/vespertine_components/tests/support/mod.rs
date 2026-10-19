//! Shared fixture of the browser surface tests: a window hosting a surface whose helper is the
//! in-process scripted peer of `vespertine_webview::testing`.

#![allow(dead_code)]

use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use vespertine::{
    AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, Modifiers, MouseButton,
    NavigationDirection, ParentElement, Pixels, Point, Render, Styled, TestAppContext,
    VisualTestContext, Window, div, point,
};
use vespertine_components::browser::{BrowserSurface, Presentation};
use vespertine_webview::protocol::kind;
use vespertine_webview::testing::{Peer, PeerScript, ScriptedStartup, wait_until};

/// The window content: the surface below an ancestor that counts mouse back/forward presses
/// (what a shell-level history handler would see).
pub struct Root {
    pub surface: Entity<BrowserSurface>,
    pub navigation_presses: Rc<Cell<usize>>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let back = self.navigation_presses.clone();
        let forward = self.navigation_presses.clone();
        div()
            .size_full()
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                move |_, _, _| back.set(back.get() + 1),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                move |_, _, _| forward.set(forward.get() + 1),
            )
            .child(self.surface.clone())
    }
}

pub struct Fixture {
    pub surface: Entity<BrowserSurface>,
    pub startup: Arc<ScriptedStartup>,
    pub navigation_presses: Rc<Cell<usize>>,
}

/// A scripted startup whose pages answer `load` with state messages. (Frames are large in a
/// test window and slow in debug builds; tests that need them script their own startup.)
pub fn scripted() -> Arc<ScriptedStartup> {
    ScriptedStartup::with(
        false,
        PeerScript {
            echo_load: true,
            frame_color: None,
            answer_pings: false,
        },
    )
}

/// A window with one surface of `startup`'s context at the given presentation.
pub fn setup<'a>(
    cx: &'a mut TestAppContext,
    startup: &Arc<ScriptedStartup>,
    presentation: Presentation,
) -> (Fixture, &'a mut VisualTestContext) {
    cx.executor().allow_parking();
    let context = startup.context();
    let navigation_presses = Rc::new(Cell::new(0));
    let counter = navigation_presses.clone();
    let (root, vcx) = cx.add_window_view(move |window, cx| {
        let surface = cx.new(|cx| BrowserSurface::new(context, window, cx));
        Root {
            surface,
            navigation_presses: counter,
        }
    });
    let surface = root.read_with(vcx, |root, _| root.surface.clone());
    vcx.update(|window, app| {
        window.activate_window();
        surface.update(app, |surface, cx| {
            surface.set_presentation(presentation, window, cx)
        });
    });
    redraw(vcx);
    (
        Fixture {
            surface,
            startup: startup.clone(),
            navigation_presses,
        },
        vcx,
    )
}

/// Runs the foreground executor until `condition` holds (helper messages arrive through
/// tasks) or five seconds passed.
pub fn pump_until(
    cx: &mut VisualTestContext,
    mut condition: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    wait_until(Duration::from_secs(20), || {
        cx.run_until_parked();
        condition(cx)
    })
}

/// Draws a frame so focus and state changes reach their listeners.
pub fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// Lets in-flight helper messages arrive and then draws.
pub fn settle(cx: &mut VisualTestContext) {
    std::thread::sleep(Duration::from_millis(100));
    redraw(cx);
}

impl Fixture {
    /// Navigates and returns whether the input was accepted.
    pub fn navigate(&self, cx: &mut VisualTestContext, input: &str) -> bool {
        let accepted = cx.update(|window, app| {
            self.surface
                .update(app, |surface, cx| surface.navigate(input, window, cx))
        });
        redraw(cx);
        accepted
    }

    /// Navigates and waits until the helper received the load; returns its peer.
    pub fn open(&self, cx: &mut VisualTestContext, input: &str) -> Arc<Peer> {
        assert!(self.navigate(cx, input), "{input} was rejected");
        assert!(pump_until(cx, |_| !self
            .startup
            .all_named("load")
            .is_empty()));
        let peer = self.startup.peers()[0].clone();
        self.wait_loaded(cx);
        peer
    }

    /// Waits until the scripted helper's answer to the last load (the state with the title
    /// "Scripted <url>") arrived, so that no late answer overwrites states the test injects.
    pub fn wait_loaded(&self, cx: &mut VisualTestContext) {
        assert!(
            pump_until(cx, |cx| self.read(cx, |s| s
                .state
                .title
                .starts_with("Scripted ")
                && !s.state.loading)),
            "the scripted page never finished loading"
        );
        redraw(cx);
    }

    pub fn call<R>(
        &self,
        cx: &mut VisualTestContext,
        f: impl FnOnce(&mut BrowserSurface, &mut Window, &mut Context<BrowserSurface>) -> R,
    ) -> R {
        let result = cx.update(|window, app| {
            self.surface
                .update(app, |surface, cx| f(surface, window, cx))
        });
        redraw(cx);
        result
    }

    pub fn read<R>(&self, cx: &mut VisualTestContext, f: impl FnOnce(&BrowserSurface) -> R) -> R {
        self.surface.read_with(cx, |surface, _| f(surface))
    }

    pub fn address_text(&self, cx: &mut VisualTestContext) -> String {
        cx.update(|_, app| self.surface.read(app).address_text(app).to_owned())
    }

    /// Whether the address field has keyboard focus.
    pub fn address_focused(&self, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, app| {
            self.surface
                .read(app)
                .address()
                .read(app)
                .is_focused(window)
        })
    }

    /// Whether the page target has keyboard focus.
    pub fn page_focused(&self, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, app| {
            self.surface
                .read(app)
                .page()
                .read(app)
                .focus_handle()
                .is_focused(window)
        })
    }

    /// The centre of the element recorded under `selector`.
    pub fn center(&self, cx: &mut VisualTestContext, selector: &'static str) -> Point<Pixels> {
        let bounds: Bounds<Pixels> = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("no element {selector} in the last frame"));
        bounds.center()
    }

    pub fn click(&self, cx: &mut VisualTestContext, selector: &'static str) {
        let position = self.center(cx, selector);
        cx.simulate_click(position, Modifiers::none());
        cx.run_until_parked();
    }
}

/// A complete state snapshot message.
pub fn state_json(
    url: &str,
    title: &str,
    error: Option<&str>,
    loading: bool,
    can_back: bool,
    can_forward: bool,
) -> Value {
    json!({
        "url": url, "title": title, "error": error, "loading": loading,
        "can_back": can_back, "can_forward": can_forward,
    })
}

pub fn send_state(peer: &Peer, value: Value) {
    peer.send_json(kind::STATE, 1, value);
}

/// A menu message at (x, y) of page 1.
pub fn send_menu(peer: &Peer, x: f64, y: f64, items: Value) {
    peer.send_json(kind::MENU, 1, json!({"x": x, "y": y, "items": items}));
}

pub fn item(label: &str, action: &str, enabled: bool, selected: bool) -> Value {
    json!({"label": label, "action": action, "enabled": enabled, "selected": selected})
}

/// The point over the page area far from any menu.
pub fn page_point() -> Point<Pixels> {
    point(vespertine::px(700.), vespertine::px(500.))
}
