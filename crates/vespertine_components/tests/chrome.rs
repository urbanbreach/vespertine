//! CMP-022 A4 (external open), A5 (chrome), A6 (placeholders), A10 (labels)
//! (accessibility) against the scripted helper.

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use support::*;
use vespertine::{EntityInputHandler, Modifiers, MouseButton, TestAppContext};
use vespertine_components::browser::{BodyKind, BrowserEvent, Presentation};

fn events(
    cx: &mut vespertine::VisualTestContext,
    fixture: &Fixture,
) -> (Rc<RefCell<Vec<BrowserEvent>>>, vespertine::Subscription) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let subscription = cx.update(|_, app| {
        app.subscribe(&fixture.surface, move |_, event: &BrowserEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
    });
    (events, subscription)
}

#[vespertine::test]
fn external_open_is_disabled_for_a_file_address_and_opens_an_https_address_once(
    cx: &mut TestAppContext,
) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let opened = Rc::new(RefCell::new(Vec::<String>::new()));
    let sink = opened.clone();
    fixture.call(cx, |surface, _, _| {
        surface.set_external_opener(move |url, _| sink.borrow_mut().push(url.to_owned()));
    });
    let peer = fixture.open(cx, "example.test/page");
    // The helper reports a file address: the button is disabled and does nothing.
    send_state(
        &peer,
        state_json("file:///tmp/a", "", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s
        .state
        .url
        .as_deref()
        == Some("file:///tmp/a"))));
    redraw(cx);
    fixture.read(cx, |surface| assert!(!surface.can_open_externally()));
    fixture.click(cx, "browser-external");
    assert!(opened.borrow().is_empty());
    // An https address is opened exactly once.
    send_state(
        &peer,
        state_json("https://example.test/page", "", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.can_open_externally())));
    redraw(cx);
    fixture.click(cx, "browser-external");
    assert_eq!(*opened.borrow(), ["https://example.test/page"]);
    // Credentials in the page address disable it again.
    send_state(
        &peer,
        state_json("https://user@example.test/", "", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| !s.can_open_externally())));
    redraw(cx);
    fixture.click(cx, "browser-external");
    assert_eq!(opened.borrow().len(), 1);
}

#[vespertine::test]
fn history_buttons_are_enabled_only_when_the_page_allows_them(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    // The scripted peer reports can_back after the load; reset to a page without history.
    send_state(
        &peer,
        state_json("https://example.test/", "", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| !s.can_go_back())));
    fixture.click(cx, "browser-back");
    fixture.click(cx, "browser-forward");
    settle(cx);
    assert!(peer.named("back").is_empty() && peer.named("forward").is_empty());
    send_state(
        &peer,
        state_json("https://example.test/", "", None, false, true, true),
    );
    assert!(pump_until(cx, |cx| fixture
        .read(cx, |s| s.can_go_back() && s.can_go_forward())));
    redraw(cx);
    fixture.click(cx, "browser-back");
    fixture.click(cx, "browser-forward");
    peer.wait_named("back", 1);
    peer.wait_named("forward", 1);
}

#[vespertine::test]
fn reload_after_an_error_loads_the_address_again_and_otherwise_reloads(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    send_state(
        &peer,
        state_json("https://example.test/", "T", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.state.title == "T")));
    redraw(cx);
    fixture.click(cx, "browser-reload");
    peer.wait_named("reload", 1);
    assert_eq!(peer.named("load").len(), 1);
    // With an error the same button re-issues the load of the address.
    send_state(
        &peer,
        state_json(
            "https://example.test/",
            "",
            Some("boom"),
            false,
            false,
            false,
        ),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.state.error.is_some())));
    redraw(cx);
    fixture.click(cx, "browser-reload");
    peer.wait_named("load", 2);
    assert_eq!(
        peer.named("reload").len(),
        1,
        "an error must not send reload"
    );
    assert_eq!(peer.named("load")[1]["url"], "https://example.test/");
    fixture.read(cx, |s| assert!(s.state.error.is_none()));
}

#[vespertine::test]
fn focusing_the_address_selects_all_text(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    fixture.open(cx, "example.test/a");
    assert!(fixture.page_focused(cx));
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    assert!(fixture.address_focused(cx) && !fixture.page_focused(cx));
    let (text, selection) = cx.update(|_, app| {
        let address = fixture.surface.read(app).address().read(app);
        (address.text().to_owned(), address.selection())
    });
    assert_eq!(text, "https://example.test/a");
    assert_eq!(selection, 0..text.len());
    // Clicking the field focuses it and selects everything as well.
    fixture.call(cx, |surface, window, cx| {
        let page = surface.page().read(cx).focus_handle().clone();
        window.focus(&page, cx);
    });
    assert!(!fixture.address_focused(cx));
    let position = fixture.center(cx, "browser-address-field");
    cx.simulate_click(position, Modifiers::none());
    assert!(fixture.address_focused(cx));
    let selection = cx.update(|_, app| fixture.surface.read(app).address().read(app).selection());
    assert_eq!(selection, 0..text.len());
}

#[vespertine::test]
fn enter_submits_and_escape_restores_the_page_address_and_returns_focus(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    let (events, _subscription) = events(cx, &fixture);
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    cx.simulate_input("other.test");
    assert_eq!(fixture.address_text(cx), "other.test");
    fixture.read(cx, |s| assert!(s.is_edited()));
    // Keys never reach the page while the field has focus.
    settle(cx);
    assert!(peer.named("key_down").is_empty() && peer.named("commit").is_empty());
    // Escape restores the page address, clears the edit state and returns focus to the page.
    cx.simulate_keystrokes("escape");
    assert_eq!(fixture.address_text(cx), "https://example.test/");
    fixture.read(cx, |s| assert!(!s.is_edited() && s.validation().is_none()));
    assert!(fixture.page_focused(cx) && !fixture.address_focused(cx));
    // Enter submits.
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    cx.simulate_input("other.test");
    cx.simulate_keystrokes("enter");
    peer.wait_named("load", 2);
    assert_eq!(peer.named("load")[1]["url"], "https://other.test/");
    fixture.wait_loaded(cx);
    assert_eq!(fixture.address_text(cx), "https://other.test/");
    assert!(fixture.page_focused(cx));
    assert!(events.borrow().contains(&BrowserEvent::Changed));
}

#[vespertine::test]
fn a_rejected_input_shows_its_message_and_keeps_the_field_text(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    cx.simulate_input("two words");
    cx.simulate_keystrokes("enter");
    fixture.read(cx, |s| {
        assert_eq!(
            s.validation(),
            Some("Enter a valid website or localhost address.")
        );
    });
    assert_eq!(fixture.address_text(cx), "two words");
    assert!(cx.debug_bounds("browser-validation").is_some());
    assert!(fixture.address_focused(cx));
    settle(cx);
    assert_eq!(peer.named("load").len(), 1, "nothing is navigated");
    // Editing clears the message.
    cx.simulate_input("x");
    fixture.read(cx, |s| assert_eq!(s.validation(), None));
    // The public API reports the same message for an empty input.
    assert!(!fixture.call(cx, |s, w, cx| s.navigate("   ", w, cx)));
    fixture.read(cx, |s| {
        assert_eq!(
            s.validation(),
            Some("Enter a website or localhost address.")
        )
    });
}

#[vespertine::test]
fn the_edited_flag_tracks_differences_from_the_page_address(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    fixture.open(cx, "example.test");
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    // The go button appears while the field is focused.
    assert!(cx.debug_bounds("browser-go").is_some());
    fixture.read(cx, |s| assert!(!s.is_edited()));
    cx.simulate_keystrokes("end");
    cx.simulate_input("x");
    fixture.read(cx, |s| assert!(s.is_edited()));
    cx.simulate_keystrokes("backspace");
    fixture.read(cx, |s| assert!(!s.is_edited()));
    // Unfocused and unedited: no go button.
    cx.simulate_keystrokes("escape");
    redraw(cx);
    assert!(cx.debug_bounds("browser-go").is_none());
}

#[vespertine::test]
fn the_go_button_submits_without_taking_focus_from_the_field(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    cx.simulate_input("go.test");
    redraw(cx);
    let position = fixture.center(cx, "browser-go");
    cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
    assert!(
        fixture.address_focused(cx),
        "mouse down must not take focus from the field"
    );
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    peer.wait_named("load", 2);
    assert_eq!(peer.named("load")[1]["url"], "https://go.test/");
}

#[vespertine::test]
fn the_field_text_is_not_overwritten_while_focused_but_follows_the_page_when_unfocused(
    cx: &mut TestAppContext,
) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    cx.simulate_input("typing");
    send_state(
        &peer,
        state_json("https://b.test/", "B", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s
        .state
        .url
        .as_deref()
        == Some("https://b.test/"))));
    assert_eq!(
        fixture.address_text(cx),
        "typing",
        "a focused field is never overwritten"
    );
    fixture.read(cx, |s| assert!(s.is_edited()));
    // Unfocused: the next notification replaces the text and clears the edited flag.
    fixture.call(cx, |surface, window, cx| {
        let page = surface.page().read(cx).focus_handle().clone();
        window.focus(&page, cx);
    });
    send_state(
        &peer,
        state_json("https://b.test/", "B2", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.address_text(cx) == "https://b.test/"));
    fixture.read(cx, |s| assert!(!s.is_edited()));
    // A new page address while unfocused replaces the text.
    send_state(
        &peer,
        state_json("https://c.test/", "C", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.address_text(cx) == "https://c.test/"));
}

#[vespertine::test]
fn the_address_field_edits_text_with_selection_clipboard_and_composition(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    cx.simulate_input("hello world");
    assert_eq!(fixture.address_text(cx), "hello world");
    // Movement and selection keys.
    cx.simulate_keystrokes("home shift-right shift-right");
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(
        cx.update(|_, app| app.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("he")
    );
    cx.simulate_keystrokes("ctrl-x");
    assert_eq!(fixture.address_text(cx), "llo world");
    cx.simulate_keystrokes("end ctrl-v");
    assert_eq!(fixture.address_text(cx), "llo worldhe");
    cx.simulate_keystrokes("ctrl-left");
    cx.simulate_keystrokes("ctrl-backspace");
    assert_eq!(fixture.address_text(cx), "worldhe");
    cx.simulate_keystrokes("ctrl-a delete");
    assert_eq!(fixture.address_text(cx), "");
    // Input-method composition: preedit then commit of non-Latin text.
    let address = cx.update(|_, app| fixture.surface.read(app).address().clone());
    cx.update(|window, app| {
        address.update(app, |address, cx| {
            address.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
            assert_eq!(address.marked_text_range(window, cx), Some(0..2));
            address.replace_and_mark_text_in_range(None, "日本語", None, window, cx);
            assert_eq!(address.marked_text_range(window, cx), Some(0..3));
            address.replace_text_in_range(None, "日本語", window, cx);
            assert!(address.marked_text_range(window, cx).is_none());
        })
    });
    assert_eq!(fixture.address_text(cx), "日本語");
    // Pasted line breaks never reach the single-line field.
    cx.update(|_, app| {
        app.write_to_clipboard(vespertine::ClipboardItem::new_string("a\nb".into()))
    });
    cx.simulate_keystrokes("ctrl-a ctrl-v");
    assert_eq!(fixture.address_text(cx), "ab");
}

#[vespertine::test]
fn placeholders_follow_the_surface_state_and_the_first_frame_replaces_them(
    cx: &mut TestAppContext,
) {
    use vespertine_webview::testing::ScriptedStartup;
    let startup = ScriptedStartup::with(
        true,
        vespertine_webview::testing::PeerScript {
            echo_load: true,
            frame_color: Some(0x336699),
            answer_pings: false,
        },
    );
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    // Empty: the action, its hint and the neutral texts.
    let empty = cx
        .update(|_, app| fixture.surface.read(app).placeholder(app))
        .unwrap();
    assert_eq!(empty.kind, BodyKind::Empty);
    assert_eq!(empty.title, "Enter an address");
    assert_eq!(
        empty.description,
        "Open a website or a local web app beside your content."
    );
    assert_eq!(
        empty.action.as_ref().map(|(_, label)| label.as_str()),
        Some("Enter an address")
    );
    assert_eq!(empty.hint.as_deref(), Some("Ctrl L"));
    assert!(cx.debug_bounds("browser-placeholder").is_some());
    // The placeholder button focuses the address field.
    fixture.click(cx, "browser-placeholder-action");
    assert!(fixture.address_focused(cx));
    // Pending startup.
    fixture.navigate(cx, "example.test");
    let starting = cx
        .update(|_, app| fixture.surface.read(app).placeholder(app))
        .unwrap();
    assert_eq!(starting.kind, BodyKind::Starting);
    assert_eq!(starting.title, "Starting browser…");
    assert_eq!(
        starting.description,
        "You can keep working while your page opens."
    );
    assert!(starting.action.is_none());
    startup.release();
    // The native page replaces the placeholder and the first frame arrives.
    assert!(pump_until(cx, |cx| {
        redraw(cx);
        cx.update(|_, app| fixture.surface.read(app).page().read(app).has_frame())
    }));
    cx.update(|_, app| assert_eq!(fixture.surface.read(app).body_kind(app), BodyKind::Page));
    assert!(cx.debug_bounds("browser-placeholder").is_none());
    let peer = startup.peers()[0].clone();
    fixture.wait_loaded(cx);
    // An error replaces the page again.
    send_state(
        &peer,
        state_json(
            "https://example.test/",
            "",
            Some("net::ERR_FAILED"),
            false,
            false,
            false,
        ),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.state.error.is_some())));
    redraw(cx);
    let failed = cx
        .update(|_, app| fixture.surface.read(app).placeholder(app))
        .unwrap();
    assert_eq!(failed.kind, BodyKind::Error);
    assert_eq!(failed.title, "Couldn’t load this page");
    assert_eq!(failed.description, "net::ERR_FAILED");
    assert_eq!(
        failed.action.as_ref().map(|(_, label)| label.as_str()),
        Some("Try again")
    );
    assert!(cx.debug_bounds("browser-placeholder").is_some());
    // "Try again" reloads as in R9: the address is loaded again.
    fixture.click(cx, "browser-placeholder-action");
    peer.wait_named("load", 2);
    fixture.wait_loaded(cx);
    // A clean state shows the page again.
    send_state(
        &peer,
        state_json("https://example.test/", "ok", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.state.error.is_none())));
    redraw(cx);
    assert!(cx.debug_bounds("browser-placeholder").is_none());
}

#[vespertine::test]
fn the_empty_state_texts_are_configurable(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    fixture.call(cx, |surface, _, cx| {
        surface.set_empty_state("Open a page", "Anything you like.", cx)
    });
    let empty = cx
        .update(|_, app| fixture.surface.read(app).placeholder(app))
        .unwrap();
    assert_eq!(
        (empty.title.as_str(), empty.description.as_str()),
        ("Open a page", "Anything you like.")
    );
}

#[vespertine::test]
fn the_page_label_is_the_title_then_the_host_then_browser(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    fixture.read(cx, |s| assert_eq!(s.title(), "Browser"));
    let peer = fixture.open(cx, "localhost:3000/path");
    send_state(
        &peer,
        state_json("http://localhost:3000/path", "", None, false, false, false),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s
        .state
        .title
        .is_empty()
        && s.title() == "localhost")));
    send_state(
        &peer,
        state_json(
            "http://localhost:3000/path",
            "Local preview",
            None,
            false,
            false,
            false,
        ),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.title() == "Local preview")));
}

/// The `(role, label)` of every node of the surface's accessibility tree in the last frame.
fn accessibility_nodes(cx: &mut vespertine::VisualTestContext) -> Vec<(String, String)> {
    cx.update(|window, _| window.set_a11y_active_for_tests(true));
    cx.run_until_parked();
    let json = cx
        .update(|window, _| window.debug_a11y_tree_json())
        .expect("the window builds an accessibility tree while it is active");
    let tree: serde_json::Value = serde_json::from_str(&json).expect("the tree is JSON");
    tree["nodes"]
        .as_object()
        .expect("nodes")
        .values()
        .filter_map(|node| {
            let aria = &node["aria"];
            Some((
                aria["role"].as_str()?.to_owned(),
                aria["label"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn has_node(nodes: &[(String, String)], role: &str, label: &str) -> bool {
    nodes.iter().any(|(r, l)| r == role && l == label)
}

#[vespertine::test]
fn the_chrome_exposes_its_accessibility_roles_and_labels(cx: &mut TestAppContext) {
    // R19, A11Y-001: the toolbar buttons are buttons with their labels, the address field is a
    // text input, the placeholder action and the go button are labelled buttons.
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let nodes = accessibility_nodes(cx);
    for label in [
        "Back",
        "Forward",
        "Reload page",
        "Open in default browser",
        "Enter an address",
    ] {
        assert!(has_node(&nodes, "Button", label), "{label}: {nodes:?}");
    }
    assert!(has_node(&nodes, "TextInput", "Address"), "{nodes:?}");
    assert!(!has_node(&nodes, "Button", "Go to address"), "{nodes:?}");

    // Editing the field shows the go button.
    fixture.call(cx, |surface, window, cx| surface.focus_address(window, cx));
    cx.simulate_input("example.test");
    redraw(cx);
    let nodes = accessibility_nodes(cx);
    assert!(has_node(&nodes, "Button", "Go to address"), "{nodes:?}");

    // A failed page shows the retry action under its own label.
    let peer = fixture.open(cx, "example.test");
    send_state(
        &peer,
        state_json(
            "https://example.test/",
            "",
            Some("net::ERR_FAILED"),
            false,
            false,
            false,
        ),
    );
    assert!(pump_until(cx, |cx| fixture.read(cx, |s| s.state.error.is_some())));
    redraw(cx);
    let nodes = accessibility_nodes(cx);
    assert!(has_node(&nodes, "Button", "Retry page"), "{nodes:?}");
    assert!(!has_node(&nodes, "Button", "Enter an address"), "{nodes:?}");
}

#[vespertine::test]
fn the_chrome_colors_are_the_semantic_tokens_of_the_active_theme(cx: &mut TestAppContext) {
    use vespertine::{Background, Hsla};
    use vespertine_components::browser::BrowserTheme;
    use vespertine_theme::{
        appearance,
        selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
    };

    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let painted_quad_with = |cx: &mut vespertine::VisualTestContext, color: Hsla| {
        let background = format!("{:?}", Background::from(color));
        cx.update(|window, _| {
            window
                .painted_primitives()
                .iter()
                .any(|primitive| primitive.contains(&background))
        })
    };
    let roles = |cx: &mut vespertine::VisualTestContext| {
        cx.update(|_, app| fixture.surface.read(app).theme(app))
    };

    // Without an installed theme the roles are the built-in neutral dark tokens.
    let neutral = BrowserTheme::default();
    assert_eq!(roles(cx), neutral);

    let mut resolved = Vec::new();
    for mode in [AppearanceMode::Dark, AppearanceMode::Light] {
        cx.update(|_, app| {
            if appearance::try_theme_of(app).is_none() {
                appearance::init(
                    app,
                    mode,
                    ThemeSelection::default(),
                    AccentSelection::ThemeDefault,
                    SurfacePreference::ThemeDefault,
                );
            } else {
                appearance::set_mode(app, mode);
            }
        });
        redraw(cx);
        let expected = cx.update(|_, app| BrowserTheme::from_theme(appearance::theme_of(app)));
        assert_eq!(roles(cx), expected, "{mode:?}");
        // The toolbar fill is the theme's `raised` role and the page backing its `background`.
        assert!(painted_quad_with(cx, expected.raised), "{mode:?} raised");
        assert!(
            painted_quad_with(cx, expected.page_background),
            "{mode:?} background"
        );
        resolved.push(expected);
    }
    assert_eq!(resolved[0], neutral);
    assert_ne!(
        resolved[0], resolved[1],
        "the light theme recolors the chrome"
    );

    // An explicit set overrides the theme until it is cleared again.
    let custom = BrowserTheme {
        raised: vespertine::rgb(0x123456).into(),
        ..resolved[1].clone()
    };
    fixture.call(cx, |surface, _, cx| surface.set_theme(custom.clone(), cx));
    redraw(cx);
    assert_eq!(roles(cx), custom);
    assert!(painted_quad_with(cx, custom.raised));
    fixture.call(cx, |surface, _, cx| {
        surface.set_theme(None::<BrowserTheme>, cx)
    });
    redraw(cx);
    assert_eq!(roles(cx), resolved[1]);
}
