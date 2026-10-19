//! CMP-022 A8: page-provided menus (R14) against the scripted helper.

mod support;

use serde_json::json;
use support::*;
use vespertine::{Modifiers, MouseButton, TestAppContext};
use vespertine_components::browser::{BodyKind, Presentation};
use vespertine_webview::testing::Peer;

fn labels(
    fixture: &Fixture,
    cx: &mut vespertine::VisualTestContext,
) -> Vec<(String, String, bool)> {
    fixture.read(cx, |surface| {
        surface
            .menu()
            .expect("a menu is open")
            .items()
            .iter()
            .map(|item| (item.label.clone(), item.action.clone(), item.enabled))
            .collect()
    })
}

fn open_menu(
    fixture: &Fixture,
    cx: &mut vespertine::VisualTestContext,
    peer: &Peer,
    items: serde_json::Value,
) {
    send_menu(peer, 12., 20., items);
    assert!(pump_until(cx, |cx| fixture
        .read(cx, |surface| surface.menu().is_some())));
    redraw(cx);
}

#[vespertine::test]
fn context_menu_rows_come_from_the_helper_message_in_order(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    // A link under the pointer.
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([
            item("Open link in new tab", "open-link", true, false),
            item("Copy link address", "copy-link", true, false),
            item("Back", "back", false, false),
            item("Forward", "forward", false, false),
            item("Reload", "reload", true, false),
        ]),
    );
    let rows = labels(&fixture, cx);
    assert_eq!(
        rows[0],
        ("Open link in new tab".into(), "open-link".into(), true)
    );
    assert_eq!(
        rows[1],
        ("Copy link address".into(), "copy-link".into(), true)
    );
    assert_eq!(rows.len(), 5);
    assert!(cx.debug_bounds("browser-menu").is_some());
    fixture.read(cx, |surface| {
        assert_eq!(surface.menu().unwrap().active(), 0)
    });
    // A selection in an editable target.
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([
            item("Copy", "copy", true, false),
            item("Paste", "text", true, false),
            item("Select all", "select-all", true, false),
            item("Back", "back", true, false),
            item("Forward", "forward", false, false),
            item("Reload", "reload", true, false),
        ]),
    );
    let rows = labels(&fixture, cx);
    assert_eq!(
        rows.iter().map(|row| row.1.as_str()).collect::<Vec<_>>(),
        ["copy", "text", "select-all", "back", "forward", "reload"]
    );
    assert!(
        !rows[4].2,
        "forward is disabled when history does not allow it"
    );
}

#[vespertine::test]
fn the_initial_row_follows_selected_and_down_skips_disabled_rows_with_wrap(
    cx: &mut TestAppContext,
) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([
            item("One", "option:0", true, false),
            item("Two", "option:1", false, false),
            item("Three", "option:2", true, true),
        ]),
    );
    fixture.read(cx, |surface| {
        assert_eq!(surface.menu().unwrap().active(), 2)
    });
    cx.simulate_keystrokes("down");
    fixture.read(cx, |surface| {
        assert_eq!(surface.menu().unwrap().active(), 0)
    });
    cx.simulate_keystrokes("down");
    fixture.read(cx, |surface| {
        assert_eq!(surface.menu().unwrap().active(), 2)
    });
    cx.simulate_keystrokes("up");
    fixture.read(cx, |surface| {
        assert_eq!(surface.menu().unwrap().active(), 0)
    });
    // Other keys are swallowed while the menu is open: nothing reaches the page.
    cx.simulate_keystrokes("a tab");
    settle(cx);
    assert!(peer.named("key_down").is_empty() && peer.named("commit").is_empty());
}

#[vespertine::test]
fn enter_sends_the_action_then_dismiss_menu_and_escape_only_dismisses(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([
            item("Copy", "copy", true, false),
            item("Select all", "select-all", true, false),
        ]),
    );
    cx.simulate_keystrokes("down enter");
    let select_all = peer.wait_named("select-all", 1);
    assert_eq!(select_all.len(), 1);
    peer.wait_named("dismiss-menu", 1);
    // The action comes before the dismissal.
    let order: Vec<String> = peer
        .commands()
        .iter()
        .map(|(_, command)| command["cmd"].as_str().unwrap().to_owned())
        .filter(|name| name == "select-all" || name == "dismiss-menu")
        .collect();
    assert_eq!(order, ["select-all", "dismiss-menu"]);
    fixture.read(cx, |surface| assert!(surface.menu().is_none()));
    // Escape dismisses without sending an action.
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([item("Copy", "copy", true, false)]),
    );
    cx.simulate_keystrokes("escape");
    fixture.read(cx, |surface| assert!(surface.menu().is_none()));
    peer.wait_named("dismiss-menu", 2);
    assert!(peer.named("copy").is_empty());
    // Enter on a disabled row does nothing.
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([item("Forward", "forward", false, false)]),
    );
    cx.simulate_keystrokes("enter");
    fixture.read(cx, |surface| assert!(surface.menu().is_some()));
    assert!(peer.named("forward").is_empty());
}

#[vespertine::test]
fn clicking_an_enabled_row_activates_it_and_a_disabled_row_is_inert(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([
            item("Back", "back", false, false),
            item("Reload", "reload", true, false)
        ]),
    );
    // The card is anchored at the page origin plus (12, 20); rows are 28 px high with 4 px padding.
    let card = cx.debug_bounds("browser-menu").expect("menu card");
    let body = cx.debug_bounds("browser-body").expect("body");
    assert!((card.origin.x - body.origin.x - vespertine::px(12.)).abs() < vespertine::px(1.));
    assert!((card.origin.y - body.origin.y - vespertine::px(20.)).abs() < vespertine::px(1.));
    assert!((card.size.width - vespertine::px(240.)).abs() < vespertine::px(1.));
    let disabled = card.origin + vespertine::point(vespertine::px(40.), vespertine::px(4. + 14.));
    cx.simulate_click(disabled, Modifiers::none());
    settle(cx);
    assert!(peer.named("back").is_empty());
    fixture.read(cx, |surface| assert!(surface.menu().is_some()));
    let enabled =
        card.origin + vespertine::point(vespertine::px(40.), vespertine::px(4. + 28. + 1. + 14.));
    cx.simulate_click(enabled, Modifiers::none());
    peer.wait_named("reload", 1);
    peer.wait_named("dismiss-menu", 1);
    fixture.read(cx, |surface| assert!(surface.menu().is_none()));
}

#[vespertine::test]
fn an_outside_press_dismisses_the_menu_and_never_reaches_the_page(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([item("Copy", "copy", true, false)]),
    );
    cx.simulate_mouse_down(page_point(), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(page_point(), MouseButton::Left, Modifiers::none());
    assert!(pump_until(cx, |_| !peer.named("dismiss-menu").is_empty()));
    settle(cx);
    fixture.read(cx, |surface| assert!(surface.menu().is_none()));
    assert!(
        peer.named("down").is_empty(),
        "the dismissing press must not reach the page"
    );
    assert!(peer.named("up").is_empty());
    // The next press is forwarded.
    cx.simulate_mouse_down(page_point(), MouseButton::Left, Modifiers::none());
    peer.wait_named("down", 1);
}

#[vespertine::test]
fn a_press_inside_the_card_never_reaches_the_page(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([item("Copy", "copy", false, false)]),
    );
    let card = cx.debug_bounds("browser-menu").expect("menu card");
    cx.simulate_mouse_down(card.center(), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(card.center(), MouseButton::Left, Modifiers::none());
    settle(cx);
    assert!(peer.named("down").is_empty());
}

#[vespertine::test]
fn paste_reads_the_clipboard_first_and_sends_nothing_without_text(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    let paste = json!([item("Paste", "text", true, false)]);
    // No text on the clipboard: only the dismissal is sent.
    open_menu(&fixture, cx, &peer, paste.clone());
    cx.simulate_keystrokes("enter");
    peer.wait_named("dismiss-menu", 1);
    settle(cx);
    assert!(peer.named("text").is_empty());
    // With text it is sent as a "text" command, before the dismissal.
    cx.update(|_, app| {
        app.write_to_clipboard(vespertine::ClipboardItem::new_string(
            " — café 日本語".into(),
        ))
    });
    open_menu(&fixture, cx, &peer, paste);
    cx.simulate_keystrokes("enter");
    let texts = peer.wait_named("text", 1);
    assert_eq!(texts[0]["text"], " — café 日本語");
    peer.wait_named("dismiss-menu", 2);
    let names: Vec<String> = peer
        .commands()
        .iter()
        .map(|(_, command)| command["cmd"].as_str().unwrap().to_owned())
        .filter(|name| name == "text" || name == "dismiss-menu")
        .collect();
    assert_eq!(names, ["dismiss-menu", "text", "dismiss-menu"]);
}

#[vespertine::test]
fn an_option_menu_picks_the_second_option_with_down_then_enter(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([
            item("First", "option:0", true, true),
            item("Second", "option:1", true, false),
            item("Third", "option:2", false, false),
        ]),
    );
    fixture.read(cx, |surface| {
        assert_eq!(surface.menu().unwrap().active(), 0)
    });
    cx.simulate_keystrokes("down enter");
    peer.wait_named("option:1", 1);
    peer.wait_named("dismiss-menu", 1);
    assert!(peer.named("option:0").is_empty());
}

#[vespertine::test]
fn an_empty_menu_is_dismissed_at_once_and_navigating_closes_a_menu(cx: &mut TestAppContext) {
    let startup = scripted();
    let (fixture, cx) = setup(cx, &startup, Presentation::Live);
    let peer = fixture.open(cx, "example.test");
    send_menu(&peer, 1., 1., json!([]));
    assert!(pump_until(cx, |_| !peer.named("dismiss-menu").is_empty()));
    settle(cx);
    fixture.read(cx, |surface| assert!(surface.menu().is_none()));
    open_menu(
        &fixture,
        cx,
        &peer,
        json!([item("Copy", "copy", true, false)]),
    );
    fixture.navigate(cx, "other.test");
    fixture.read(cx, |surface| assert!(surface.menu().is_none()));
    assert_eq!(
        cx.update(|_, app| fixture.surface.read(app).body_kind(app)),
        BodyKind::Page
    );
}
