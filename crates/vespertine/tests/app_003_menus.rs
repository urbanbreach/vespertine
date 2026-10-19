//! Acceptance tests for APP-003: action-backed application menus.

#[allow(dead_code)]
#[path = "../examples/example_support/menus_in_window.rs"]
mod menus_in_window;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use menus_in_window::{
    ActivationLog, Copy, Cut, MenuDemo, MenuEntry, NewDocument, Paste, Quit, Redo, SelectAll,
    Settings, Undo, bind_shortcuts, displayed_equivalent, install, menu_entries, set_menus,
};
use vespertine::{
    Action, AnyWindowHandle, App, Context, FocusHandle, Focusable, IntoElement, KeyBinding, Menu,
    MenuItem, OsAction, OwnedMenu, OwnedMenuItem, Render, SystemMenuType, TestAppContext,
    VisualTestContext, Window, actions, div, prelude::*,
};

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn open_menu_demo(
    cx: &mut TestAppContext,
) -> (vespertine::Entity<MenuDemo>, &mut VisualTestContext) {
    cx.update(install);
    cx.add_window_view(MenuDemo::new)
}

fn log(cx: &VisualTestContext) -> Vec<String> {
    cx.read_global(|log: &ActivationLog, _| log.entries.clone())
}

fn edit_items(cx: &mut VisualTestContext) -> Vec<(String, bool, Option<String>)> {
    cx.update(|window, cx| {
        let menus = cx.get_menus().unwrap();
        let edit = menus.iter().find(|menu| menu.name == "Edit").unwrap();
        menu_entries(edit, window, cx)
            .into_iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => {
                    Some((item.label.to_string(), item.enabled, item.equivalent))
                }
                _ => None,
            })
            .collect()
    })
}

fn enabled(items: &[(String, bool, Option<String>)], label: &str) -> bool {
    items.iter().find(|item| item.0 == label).unwrap().1
}

fn click_selector(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("`{selector}` is not rendered"));
    cx.simulate_click(bounds.center(), Default::default());
}

// ---------------------------------------------------------------------------------------------
// A1: in-window menus rendered from retrieved descriptors; mouse and shortcut reach one handler.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn app_003_a1_in_window_menu_activates_the_same_handler_as_the_shortcut(cx: &mut TestAppContext) {
    let (demo, cx) = open_menu_demo(cx);

    // The bar is rendered from `get_menus()`: one title per installed top-level menu.
    for title in ["menu-title-App", "menu-title-Edit", "menu-title-Window"] {
        assert!(cx.debug_bounds(title).is_some(), "{title} is rendered");
    }
    assert!(cx.debug_bounds("menu-item-Copy").is_none(), "no popup yet");

    // Open the Edit menu and activate Copy with the mouse.
    click_selector(cx, "menu-title-Edit");
    assert!(cx.debug_bounds("menu-item-Copy").is_some());
    click_selector(cx, "menu-item-Copy");
    assert_eq!(log(cx), vec!["editor: copy"]);
    assert!(
        cx.debug_bounds("menu-item-Copy").is_none(),
        "choosing an item closes the popup"
    );

    // The menu click did not move focus away from the editor, so the same action bound to a
    // shortcut reaches the very same handler.
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(log(cx), vec!["editor: copy", "editor: copy"]);
    let editor_focused =
        cx.update(|window, cx| demo.read(cx).editor.focus_handle(cx).is_focused(window));
    assert!(editor_focused);

    // Application group: a displayed equivalent, a separator and a disabled item.
    click_selector(cx, "menu-title-App");
    let (separators, settings_enabled, new_document, quit) = cx.update(|window, cx| {
        let menus = cx.get_menus().unwrap();
        let app = menus.iter().find(|menu| menu.name == "App").unwrap();
        let entries = menu_entries(app, window, cx);
        let separators = entries
            .iter()
            .filter(|entry| matches!(entry, MenuEntry::Separator))
            .count();
        let find = |label: &str| {
            entries.iter().find_map(|entry| match entry {
                MenuEntry::Item(item) if item.label == label => {
                    Some((item.enabled, item.equivalent.clone()))
                }
                _ => None,
            })
        };
        (
            separators,
            find("Settings").unwrap().0,
            find("New Document").unwrap(),
            find("Quit").unwrap(),
        )
    });
    assert_eq!(separators, 2);
    assert!(
        !settings_enabled,
        "the descriptor's disabled flag disables the item"
    );
    assert_eq!(new_document, (true, Some("ctrl-N".to_string())));
    assert_eq!(quit, (true, Some("ctrl-Q".to_string())));

    // Clicking the disabled item does nothing and leaves the popup open.
    click_selector(cx, "menu-item-Settings");
    assert_eq!(log(cx).len(), 2);
    assert!(cx.debug_bounds("menu-item-Settings").is_some());

    // New Document: mouse, then shortcut, same handler on the window root.
    click_selector(cx, "menu-item-New Document");
    cx.simulate_keystrokes("ctrl-n");
    assert_eq!(&log(cx)[2..], ["new document", "new document"]);

    // Quit is a global handler: reached by the menu and by the shortcut alike.
    click_selector(cx, "menu-title-App");
    click_selector(cx, "menu-item-Quit");
    cx.simulate_keystrokes("ctrl-q");
    assert_eq!(&log(cx)[4..], ["quit", "quit"]);
    assert_eq!(cx.quit_requests(), 2);
}

// ---------------------------------------------------------------------------------------------
// A2: set_menus / get_menus round trip and replacement.
// ---------------------------------------------------------------------------------------------

actions!(app_003_round_trip, [First, Second, Third]);

fn describe(menus: &[OwnedMenu]) -> Vec<String> {
    fn menu(menu: &OwnedMenu, depth: usize, out: &mut Vec<String>) {
        let pad = "  ".repeat(depth);
        out.push(format!(
            "{pad}menu {} disabled={}",
            menu.name, menu.disabled
        ));
        for item in &menu.items {
            match item {
                OwnedMenuItem::Separator => out.push(format!("{pad}  separator")),
                OwnedMenuItem::Submenu(submenu) => menu_into(submenu, depth + 1, out),
                OwnedMenuItem::SystemMenu(os) => out.push(format!(
                    "{pad}  system {} services={}",
                    os.name,
                    os.menu_type == SystemMenuType::Services
                )),
                OwnedMenuItem::Action {
                    name,
                    action,
                    os_action,
                    checked,
                    disabled,
                } => out.push(format!(
                    "{pad}  action {name} -> {} role={} checked={checked} disabled={disabled}",
                    action.name(),
                    match os_action {
                        None => "none",
                        Some(OsAction::Cut) => "cut",
                        Some(OsAction::Copy) => "copy",
                        Some(OsAction::Paste) => "paste",
                        Some(OsAction::SelectAll) => "select-all",
                        Some(OsAction::Undo) => "undo",
                        Some(OsAction::Redo) => "redo",
                    }
                )),
            }
        }
    }
    fn menu_into(m: &OwnedMenu, depth: usize, out: &mut Vec<String>) {
        menu(m, depth, out)
    }
    let mut out = Vec::new();
    for top in menus {
        menu(top, 0, &mut out);
    }
    out
}

#[vespertine::test]
fn app_003_a2_set_menus_get_menus_round_trip_and_replacement(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_menus([
            Menu::new("File").items([
                MenuItem::action("First", First),
                MenuItem::separator(),
                MenuItem::action("Second", Second)
                    .checked(true)
                    .disabled(true),
                MenuItem::submenu(Menu::new("Recent").disabled(true).items([
                    MenuItem::os_action("Paste", Third, OsAction::Paste),
                    MenuItem::separator(),
                    MenuItem::submenu(Menu::new("Nested").items([MenuItem::action("Deep", First)])),
                ])),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
            ]),
            Menu::new("Empty"),
            Menu::new("Edit").items([
                MenuItem::os_action("Undo", First, OsAction::Undo),
                MenuItem::os_action("Redo", Second, OsAction::Redo),
                MenuItem::os_action("Cut", Third, OsAction::Cut),
                MenuItem::os_action("Copy", First, OsAction::Copy),
                MenuItem::os_action("Select All", Second, OsAction::SelectAll),
            ]),
        ])
    });
    let installed = cx.read(|cx| cx.get_menus()).unwrap();
    assert_eq!(
        describe(&installed),
        vec![
            "menu File disabled=false",
            "  action First -> app_003_round_trip::First role=none checked=false disabled=false",
            "  separator",
            "  action Second -> app_003_round_trip::Second role=none checked=true disabled=true",
            "  menu Recent disabled=true",
            "    action Paste -> app_003_round_trip::Third role=paste checked=false disabled=false",
            "    separator",
            "    menu Nested disabled=false",
            "      action Deep -> app_003_round_trip::First role=none checked=false disabled=false",
            "  system Services services=true",
            "menu Empty disabled=false",
            "menu Edit disabled=false",
            "  action Undo -> app_003_round_trip::First role=undo checked=false disabled=false",
            "  action Redo -> app_003_round_trip::Second role=redo checked=false disabled=false",
            "  action Cut -> app_003_round_trip::Third role=cut checked=false disabled=false",
            "  action Copy -> app_003_round_trip::First role=copy checked=false disabled=false",
            "  action Select All -> app_003_round_trip::Second role=select-all checked=false disabled=false",
        ]
    );

    // The retrieved descriptors are owned: they stay valid and keep their action identities
    // after the application replaces the installed menus.
    cx.update(|cx| cx.set_menus([Menu::new("Only").items([MenuItem::action("Third", Third)])]));
    let replaced = cx.read(|cx| cx.get_menus()).unwrap();
    assert_eq!(
        describe(&replaced),
        vec![
            "menu Only disabled=false",
            "  action Third -> app_003_round_trip::Third role=none checked=false disabled=false",
        ]
    );
    assert_eq!(describe(&installed).len(), 17);
    let OwnedMenuItem::Action { action, .. } = &installed[0].items[0] else {
        panic!("first item is an action");
    };
    assert!(action.partial_eq(&First));
    assert!(!action.partial_eq(&Second));

    // Installing an empty list clears everything.
    cx.update(|cx| cx.set_menus([]));
    assert!(cx.read(|cx| cx.get_menus()).unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// A3: edit-action enablement follows focus.
// ---------------------------------------------------------------------------------------------

const EDIT_LABELS: [&str; 6] = ["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"];

#[vespertine::test]
fn app_003_a3_edit_action_enablement_follows_focus(cx: &mut TestAppContext) {
    let (demo, cx) = open_menu_demo(cx);

    // An editor is focused: every edit item is available.
    let items = edit_items(cx);
    for label in EDIT_LABELS {
        assert!(
            enabled(&items, label),
            "{label} is available for the focused editor"
        );
    }

    // A read-only viewer only handles copy and select all.
    let viewer = demo.read_with(cx, |demo, cx| demo.viewer.focus_handle(cx));
    cx.update(|window, cx| window.focus(&viewer, cx));
    cx.run_until_parked();
    let items = edit_items(cx);
    for label in ["Undo", "Redo", "Cut", "Paste"] {
        assert!(
            !enabled(&items, label),
            "{label} is unavailable for the viewer"
        );
    }
    for label in ["Copy", "Select All"] {
        assert!(
            enabled(&items, label),
            "{label} is available for the viewer"
        );
    }
    // Unavailable items show no equivalent in this context either; available ones keep theirs.
    let equivalent = |label: &str| {
        items
            .iter()
            .find(|item| item.0 == label)
            .and_then(|item| item.2.clone())
    };
    assert_eq!(equivalent("Cut"), None);
    assert_eq!(equivalent("Copy"), Some("ctrl-C".to_string()));

    // Shortcut and menu both refuse to reach the unfocused editor.
    cx.simulate_keystrokes("ctrl-x");
    click_selector(cx, "menu-title-Edit");
    click_selector(cx, "menu-item-Cut");
    assert!(
        log(cx).is_empty(),
        "cut is not dispatched to the background editor"
    );
    click_selector(cx, "menu-item-Copy");
    assert_eq!(log(cx), vec!["viewer: copy"]);

    // Another editor takes focus: available again, and it is the one that receives actions.
    let other = demo.read_with(cx, |demo, cx| demo.other_editor.focus_handle(cx));
    cx.update(|window, cx| window.focus(&other, cx));
    cx.run_until_parked();
    let items = edit_items(cx);
    for label in EDIT_LABELS {
        assert!(
            enabled(&items, label),
            "{label} is available for the second editor"
        );
    }
    cx.simulate_keystrokes("ctrl-x");
    assert_eq!(log(cx), vec!["viewer: copy", "other editor: cut"]);

    // A surface without handlers: nothing is available, and nothing is dispatched.
    cx.update(|window, cx| window.blur(cx));
    cx.run_until_parked();
    let items = edit_items(cx);
    for label in EDIT_LABELS {
        assert!(
            !enabled(&items, label),
            "{label} is unavailable without a focused handler"
        );
    }
    cx.simulate_keystrokes("ctrl-c ctrl-v");
    assert_eq!(log(cx).len(), 2);

    // The same availability is exposed through the window and application queries.
    cx.update(|window, cx| {
        assert!(!window.is_action_available(&Cut, cx));
        window.focus(&other, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(window.is_action_available(&Cut, cx));
        assert!(window.is_action_available(&Paste, cx));
        assert!(!window.is_action_available(&Settings, cx));
        // Quit has a global handler, so it is available regardless of focus.
        assert!(menus_in_window::is_available(&Quit, window, cx));
    });
    // The application-wide query inspects the active window, which must not be borrowed, so ask
    // it at the end of the effect cycle.
    let answers = Rc::new(RefCell::new(Vec::new()));
    cx.update(|window, cx| {
        window.activate_window();
        let answers = answers.clone();
        cx.defer(move |cx| {
            answers.borrow_mut().push((
                cx.is_action_available(&Quit),
                cx.is_action_available(&Cut),
                cx.is_action_available(&Settings),
            ));
        });
    });
    cx.run_until_parked();
    assert_eq!(
        *answers.borrow(),
        vec![(true, true, false)],
        "a global handler or a handler on the active window's focus path makes an action available"
    );

    // With the viewer focused, the application-wide answer follows the focus too.
    cx.update(|window, cx| {
        window.focus(&viewer, cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let answers = answers.clone();
        cx.defer(move |cx| {
            answers.borrow_mut().push((
                true,
                cx.is_action_available(&Cut),
                cx.is_action_available(&Copy),
            ));
        });
    });
    cx.run_until_parked();
    assert_eq!(answers.borrow()[1], (true, false, true));
}

// ---------------------------------------------------------------------------------------------
// A4: vetoable quit decided after the dispatch returns.
// ---------------------------------------------------------------------------------------------

actions!(app_003_quit, [QuitApp]);

struct Document {
    saved: bool,
    focus: FocusHandle,
}

impl Render for Document {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().track_focus(&self.focus)
    }
}

fn open_document(cx: &mut TestAppContext, saved: bool) -> AnyWindowHandle {
    let window = cx.add_window(move |window, cx| {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Document { saved, focus }
    });
    cx.run_until_parked();
    window.into()
}

#[vespertine::test]
fn app_003_a4_quit_is_vetoable_and_decided_after_the_dispatch_returns(cx: &mut TestAppContext) {
    let order = Rc::new(RefCell::new(Vec::<String>::new()));
    cx.update(|cx| {
        let order = order.clone();
        cx.on_action(move |_: &QuitApp, cx| {
            order.borrow_mut().push("handler start".into());
            let order_in_decision = order.clone();
            // The decision is deferred to the end of the effect cycle. By then the window that
            // dispatched the action has been returned to the application, so every window root
            // can be inspected.
            cx.defer(move |cx| {
                let mut asked = 0;
                let mut all_ready = true;
                for window in cx.windows() {
                    let document = window.downcast::<Document>().expect("document root");
                    let saved = document.read(cx).expect("window is readable").saved;
                    asked += 1;
                    all_ready &= saved;
                }
                order_in_decision
                    .borrow_mut()
                    .push(format!("decision asked={asked} ready={all_ready}"));
                if all_ready {
                    cx.quit();
                }
            });
            order.borrow_mut().push("handler end".into());
        });
    });

    let first = open_document(cx, true);
    let second = open_document(cx, false);
    assert_eq!(cx.windows().len(), 2);

    // One window is not ready: the quit is vetoed, the app keeps running, both windows stay.
    cx.dispatch_action(first, QuitApp);
    cx.run_until_parked();
    assert_eq!(
        *order.borrow(),
        vec![
            "handler start",
            "handler end",
            "decision asked=2 ready=false"
        ],
        "the decision runs after the handler returned"
    );
    assert_eq!(cx.quit_requests(), 0);
    assert_eq!(cx.windows().len(), 2);

    // The veto is per request: the other window can ask as well and is still vetoed.
    cx.dispatch_action(second, QuitApp);
    cx.run_until_parked();
    assert_eq!(cx.quit_requests(), 0);

    // Once the second window reports ready the application quits.
    cx.update(|cx| {
        second
            .downcast::<Document>()
            .unwrap()
            .update(cx, |document, _, _| document.saved = true)
            .unwrap();
    });
    order.borrow_mut().clear();
    cx.dispatch_action(first, QuitApp);
    cx.run_until_parked();
    assert_eq!(
        *order.borrow(),
        vec![
            "handler start",
            "handler end",
            "decision asked=2 ready=true"
        ]
    );
    assert_eq!(cx.quit_requests(), 1);
}

// ---------------------------------------------------------------------------------------------
// A5: should-close veto, and a close action consumed by the focused surface.
// ---------------------------------------------------------------------------------------------

actions!(app_003_close, [CloseItem]);

struct TabbedWindow {
    tabs: usize,
    focus: FocusHandle,
    gate: Rc<dyn Fn() -> bool>,
}

impl Render for TabbedWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gate = self.gate.clone();
        div()
            .size_full()
            // The window-level handler: only reached when the focused surface declines.
            .on_action(cx.listener(move |_, _: &CloseItem, window, _| {
                if gate() {
                    window.remove_window();
                }
            }))
            .child(
                div()
                    .size_full()
                    .track_focus(&self.focus)
                    // The focused surface closes its active tab and consumes the action; with no
                    // tab left it declines and lets the action bubble up.
                    .on_action(cx.listener(|this, _: &CloseItem, _, cx| {
                        if this.tabs > 0 {
                            this.tabs -= 1;
                            cx.notify();
                        } else {
                            cx.propagate();
                        }
                    })),
            )
    }
}

#[vespertine::test]
fn app_003_a5_should_close_veto_and_close_action_consumed_by_focused_surface(
    cx: &mut TestAppContext,
) {
    let gate_calls = Rc::new(Cell::new(0));
    let allow_close = Rc::new(Cell::new(false));
    let gate: Rc<dyn Fn() -> bool> = {
        let (gate_calls, allow_close) = (gate_calls.clone(), allow_close.clone());
        Rc::new(move || {
            gate_calls.set(gate_calls.get() + 1);
            allow_close.get()
        })
    };

    let (view, cx) = cx.add_window_view({
        let gate = gate.clone();
        move |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            let for_platform = gate.clone();
            window.on_window_should_close(cx, move |_, _| for_platform());
            TabbedWindow {
                tabs: 2,
                focus,
                gate,
            }
        }
    });
    assert_eq!(cx.windows().len(), 1);

    // A close request (titlebar control or compositor) is vetoed by the should-close callback.
    assert!(!cx.simulate_close());
    assert_eq!(gate_calls.get(), 1);
    assert_eq!(cx.windows().len(), 1, "the veto leaves the window open");

    // Two close actions are consumed by the focused surface; the gate is not consulted.
    cx.dispatch_action(CloseItem);
    cx.dispatch_action(CloseItem);
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.tabs), 0);
    assert_eq!(
        gate_calls.get(),
        1,
        "a consumed close action never reaches the window gate"
    );
    assert_eq!(cx.windows().len(), 1);

    // With nothing left to close the surface declines: the action falls through to the gate,
    // which still vetoes.
    cx.dispatch_action(CloseItem);
    cx.run_until_parked();
    assert_eq!(gate_calls.get(), 2);
    assert_eq!(cx.windows().len(), 1);

    // Once the gate allows it the window closes, from the action and from a close request.
    allow_close.set(true);
    assert!(cx.simulate_close());
    assert_eq!(gate_calls.get(), 3);
    cx.dispatch_action(CloseItem);
    cx.run_until_parked();
    assert_eq!(gate_calls.get(), 4);
    assert!(cx.windows().is_empty());
}

// ---------------------------------------------------------------------------------------------
// A6: displayed equivalents track the current key bindings.
// ---------------------------------------------------------------------------------------------

fn equivalents(cx: &mut VisualTestContext) -> Vec<(&'static str, Option<String>)> {
    cx.update(|window, cx| {
        vec![
            ("cut", displayed_equivalent(&Cut, window, cx)),
            ("redo", displayed_equivalent(&Redo, window, cx)),
            ("quit", displayed_equivalent(&Quit, window, cx)),
            ("new", displayed_equivalent(&NewDocument, window, cx)),
            ("select all", displayed_equivalent(&SelectAll, window, cx)),
            ("settings", displayed_equivalent(&Settings, window, cx)),
            ("undo", displayed_equivalent(&Undo, window, cx)),
        ]
    })
}

#[vespertine::test]
fn app_003_a6_displayed_equivalents_track_current_key_bindings(cx: &mut TestAppContext) {
    let (_demo, cx) = open_menu_demo(cx);
    let get = |all: &[(&'static str, Option<String>)], name: &str| {
        all.iter().find(|entry| entry.0 == name).unwrap().1.clone()
    };

    let initial = equivalents(cx);
    assert_eq!(get(&initial, "cut").as_deref(), Some("ctrl-X"));
    assert_eq!(get(&initial, "redo").as_deref(), Some("ctrl-shift-Z"));
    assert_eq!(get(&initial, "quit").as_deref(), Some("ctrl-Q"));
    assert_eq!(get(&initial, "new").as_deref(), Some("ctrl-N"));
    // An action with no binding shows no equivalent.
    assert_eq!(get(&initial, "settings"), None);

    // Rebinding changes the text: the later binding has the highest precedence.
    cx.update(|_, cx| {
        cx.bind_keys([KeyBinding::new("ctrl-alt-x", Cut, Some("Editor"))]);
        cx.bind_keys([KeyBinding::new("ctrl-k", Quit, None)]);
        set_menus(cx);
    });
    cx.run_until_parked();
    let rebound = equivalents(cx);
    assert_eq!(get(&rebound, "cut").as_deref(), Some("ctrl-alt-X"));
    assert_eq!(get(&rebound, "quit").as_deref(), Some("ctrl-K"));
    assert_eq!(get(&rebound, "new").as_deref(), Some("ctrl-N"));
    // The menu displays the same text, and the new shortcut really triggers the action.
    let shown = edit_items(cx);
    assert_eq!(
        shown
            .iter()
            .find(|item| item.0 == "Cut")
            .unwrap()
            .2
            .as_deref(),
        Some("ctrl-alt-X")
    );
    cx.simulate_keystrokes("ctrl-alt-x");
    assert_eq!(log(cx), vec!["editor: cut"]);

    // Clearing every binding removes every equivalent. Installing menus again keeps the
    // descriptors retrievable.
    cx.update(|_, cx| {
        cx.clear_key_bindings();
        set_menus(cx);
    });
    cx.run_until_parked();
    let cleared = equivalents(cx);
    assert!(cleared.iter().all(|entry| entry.1.is_none()), "{cleared:?}");
    assert!(edit_items(cx).iter().all(|item| item.2.is_none()));
    assert_eq!(cx.read(|cx| cx.get_menus()).unwrap().len(), 3);
    cx.simulate_keystrokes("ctrl-x");
    assert_eq!(
        log(cx),
        vec!["editor: cut"],
        "a cleared shortcut no longer dispatches"
    );

    // Re-installing the fixed application shortcuts and the menus restores them.
    cx.update(|_, cx| {
        bind_shortcuts(cx);
        set_menus(cx);
    });
    cx.run_until_parked();
    let restored = equivalents(cx);
    assert_eq!(restored, initial);
    cx.simulate_keystrokes("ctrl-x");
    assert_eq!(log(cx), vec!["editor: cut", "editor: cut"]);
}

#[allow(dead_code)]
fn _keep_imports(_: &dyn Action, _: &App, _: Undo, _: Paste, _: Copy) {}
