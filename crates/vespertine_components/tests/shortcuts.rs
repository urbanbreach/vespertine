//! CMP-022 A9: actions, key bindings and mouse back/forward buttons.

#![cfg(target_os = "linux")]

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use support::*;
use vespertine::{
    KeyBinding, Modifiers, MouseButton, NavigationDirection, TestAppContext, actions, point, px,
};
use vespertine_components::browser::{
    BrowserEvent, BrowserShortcuts, Presentation, bind_keys, bind_shortcuts,
};

actions!(embedder, [EmbedderNewTab]);

fn chords_of(app: &vespertine::App, action_name: &str) -> Vec<String> {
    app.key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| binding.action().name() == action_name)
        .map(|binding| {
            binding
                .keystrokes()
                .iter()
                .map(|stroke| stroke.inner().unparse())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[vespertine::test]
fn the_default_chords_are_registered_and_run_their_actions(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    cx.update(|_, app| bind_keys(app));
    cx.update(|_, app| {
        assert_eq!(chords_of(app, "browser::FocusAddress"), ["ctrl-l"]);
        assert_eq!(chords_of(app, "browser::NewTab"), ["ctrl-t"]);
        assert_eq!(chords_of(app, "browser::CloseTab"), ["ctrl-w"]);
        assert_eq!(chords_of(app, "browser::Back"), ["ctrl-["]);
        assert_eq!(chords_of(app, "browser::Forward"), ["ctrl-]"]);
        assert!(
            chords_of(app, "browser::Reload").is_empty(),
            "reload has no built-in chord"
        );
    });
    // Registering again is a no-op.
    cx.update(|_, app| bind_keys(app));
    cx.update(|_, app| assert_eq!(chords_of(app, "browser::NewTab").len(), 1));

    let peer = fixture.open(cx, "example.test");
    send_state(
        &peer,
        state_json("https://example.test/", "", None, false, true, true),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.can_go_back())));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let _subscription = cx.update(|_, app| {
        app.subscribe(&fixture.surface, move |_, event: &BrowserEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
    });
    // The page has focus after navigating; the chords still reach the surface first.
    assert!(fixture.page_focused(cx));
    cx.simulate_keystrokes("ctrl-t");
    cx.simulate_keystrokes("ctrl-w");
    assert!(events.borrow().contains(&BrowserEvent::NewTab(None)));
    assert!(events.borrow().contains(&BrowserEvent::Close));
    cx.simulate_keystrokes("ctrl-[");
    cx.simulate_keystrokes("ctrl-]");
    peer.wait_named("back", 1);
    peer.wait_named("forward", 1);
    // None of the chords leaked to the page as keys.
    settle(cx);
    assert!(
        peer.named("key_down").is_empty(),
        "{:?}",
        peer.named("key_down")
    );
    cx.simulate_keystrokes("ctrl-l");
    assert!(fixture.address_focused(cx));
    // The chords also work while the address field has focus.
    cx.simulate_keystrokes("ctrl-t");
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|e| **e == BrowserEvent::NewTab(None))
            .count(),
        2
    );
}

#[vespertine::test]
fn a_chord_the_embedder_already_assigned_is_not_registered(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    cx.update(|_, app| {
        app.bind_keys([KeyBinding::new("ctrl-t", EmbedderNewTab, None)]);
        bind_keys(app);
    });
    cx.update(|_, app| {
        assert!(
            chords_of(app, "browser::NewTab").is_empty(),
            "the embedder's chord wins"
        );
        assert_eq!(chords_of(app, "embedder::EmbedderNewTab"), ["ctrl-t"]);
        // The other chords are still registered.
        assert_eq!(chords_of(app, "browser::CloseTab"), ["ctrl-w"]);
        assert_eq!(chords_of(app, "browser::FocusAddress"), ["ctrl-l"]);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let _subscription = cx.update(|_, app| {
        app.subscribe(&fixture.surface, move |_, event: &BrowserEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
    });
    fixture.open(cx, "example.test");
    cx.simulate_keystrokes("ctrl-t");
    assert!(
        events
            .borrow()
            .iter()
            .all(|event| *event != BrowserEvent::NewTab(None))
    );
}

#[vespertine::test]
fn reload_is_registered_only_when_a_configured_chord_parses(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    cx.update(|_, app| bind_keys(app));
    cx.update(|_, app| assert!(chords_of(app, "browser::Reload").is_empty()));
    // A chord that does not parse is ignored.
    cx.update(|_, app| {
        bind_shortcuts(
            app,
            &BrowserShortcuts {
                reload: Some("ctrl-a-b".into()),
                ..BrowserShortcuts::default()
            },
        )
    });
    cx.update(|_, app| assert!(chords_of(app, "browser::Reload").is_empty()));
    // A configured chord is registered, also through the surface.
    fixture.call(cx, |surface, _, cx| {
        surface.set_shortcuts(
            BrowserShortcuts {
                reload: Some("ctrl-r".into()),
                focus_address: Some("alt-d".into()),
                ..BrowserShortcuts::default()
            },
            cx,
        )
    });
    cx.update(|_, app| {
        assert_eq!(chords_of(app, "browser::Reload"), ["ctrl-r"]);
        assert!(chords_of(app, "browser::FocusAddress").contains(&"alt-d".to_owned()));
    });
    // The hint follows the configured focus-address chord.
    let hint = cx.update(|_, app| {
        fixture
            .surface
            .read(app)
            .placeholder(app)
            .and_then(|p| p.hint)
    });
    assert_eq!(hint.as_deref(), Some("Alt D"));
    let peer = fixture.open(cx, "example.test");
    cx.simulate_keystrokes("ctrl-r");
    peer.wait_named("reload", 1);
}

#[vespertine::test]
fn mouse_back_and_forward_buttons_over_the_surface_do_nothing(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    send_state(
        &peer,
        state_json("https://example.test/", "", None, false, true, true),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.can_go_back())));
    let over_toolbar = point(px(300.), px(20.));
    let over_page = page_point();
    for position in [over_toolbar, over_page] {
        for direction in [NavigationDirection::Back, NavigationDirection::Forward] {
            cx.simulate_mouse_down(
                position,
                MouseButton::Navigate(direction),
                Modifiers::none(),
            );
            cx.simulate_mouse_up(
                position,
                MouseButton::Navigate(direction),
                Modifiers::none(),
            );
        }
    }
    settle(cx);
    assert_eq!(
        fixture.navigation_presses.get(),
        0,
        "no ancestor (shell history) sees them"
    );
    assert!(peer.named("back").is_empty() && peer.named("forward").is_empty());
    assert!(peer.named("down").is_empty() && peer.named("up").is_empty());
    // Ordinary buttons still behave: a left press over the page reaches the helper.
    cx.simulate_mouse_down(over_page, MouseButton::Left, Modifiers::none());
    peer.wait_named("down", 1);
}
