//! CMP-004 A1 (focus-opened tooltips), A6 (no focus change) and A9 (Escape ownership), through
//! the production offscreen test host.

use std::{cell::RefCell, rc::Rc};
use vespertine::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    Render, Styled, Window, div, px,
};
use vespertine_components::floating::{PopupMenu, PopupMenuItem};
use vespertine_components::tooltip::trigger::TooltipTrigger;
use vespertine_test_host::{HostOptions, TestHost, require_backend};

struct Root {
    trigger: Entity<TooltipTrigger>,
    menu: Entity<PopupMenu>,
    button: FocusHandle,
    sibling: FocusHandle,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .child(div().absolute().left(px(20.)).top(px(100.)).child(self.trigger.clone()))
            .child(
                div()
                    .id("sibling")
                    .absolute()
                    .left(px(20.))
                    .top(px(150.))
                    .w(px(100.))
                    .h(px(30.))
                    .track_focus(&self.sibling)
                    .child("sibling"),
            )
            .child(div().absolute().left(px(200.)).top(px(20.)).child(self.menu.clone()))
    }
}

struct Fixture {
    host: TestHost,
    root: Entity<Root>,
}

fn fixture() -> Fixture {
    let slot: Rc<RefCell<Option<Entity<Root>>>> = Rc::new(RefCell::new(None));
    let out = slot.clone();
    let host = TestHost::with_view(HostOptions::with_size(400.0, 300.0), move |window, cx| {
        let button = cx.focus_handle().tab_stop(true);
        let sibling = cx.focus_handle().tab_stop(true);
        let inner = button.clone();
        let trigger = cx.new(|cx| {
            TooltipTrigger::new(
                "save-trigger",
                "Save the document",
                move |_, _| {
                    div()
                        .id("save")
                        .w(px(80.))
                        .h(px(30.))
                        .track_focus(&inner)
                        .child("Save")
                        .into_any_element()
                },
                window,
                cx,
            )
        });
        let menu = cx.new(|cx| {
            PopupMenu::new(
                "menu",
                "Menu",
                vec![PopupMenuItem::new("One"), PopupMenuItem::new("Two")],
                cx,
            )
        });
        let root = cx.new(|_| Root {
            trigger,
            menu,
            button,
            sibling,
        });
        *out.borrow_mut() = Some(root.clone());
        root
    })
    .expect("host");
    let root = slot.borrow().clone().expect("root");
    Fixture { host, root }
}

impl Fixture {
    fn shown(&self) -> bool {
        self.host
            .painted_text()
            .iter()
            .any(|text| text.contains("Save the document"))
    }

    fn trigger_open(&mut self) -> bool {
        let root = self.root.clone();
        let trigger = self.host.app().update(|cx| root.read(cx).trigger.clone());
        self.host.app().update(|cx| trigger.read(cx).is_open())
    }

    fn focused_is(&mut self, which: &str) -> bool {
        let root = self.root.clone();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                let root = root.read(cx);
                let handle = if which == "button" { &root.button } else { &root.sibling };
                handle.is_focused(window)
            })
            .unwrap()
    }

    fn tab(&mut self) {
        // A key press makes the modality keyboard; the traversal itself is the window's.
        self.host.key_down("tab", false, None).unwrap();
        self.host.focus_next().unwrap();
        self.host.key_up("tab", None).unwrap();
        self.settle();
    }

    fn escape(&mut self) {
        self.host.simulate_keystrokes("escape", None).unwrap();
        self.settle();
    }

    /// Focus listeners run when the window's effects flush, after the frame that moved focus;
    /// the state they change draws in the next frame.
    fn settle(&mut self) {
        self.host.flush();
        self.host.app().update(|_| ());
        self.host.flush();
    }
}

#[test]
fn a_trigger_focused_by_tab_opens_at_once_and_closes_on_blur() {
    require_backend!();
    let mut f = fixture();
    assert!(!f.shown());
    f.tab();
    assert!(f.focused_is("button"), "tab focuses the wrapped button");
    assert!(f.trigger_open(), "a keyboard-focused trigger opens with no delay");
    assert!(f.shown(), "the surface is painted");
    f.tab();
    assert!(f.focused_is("sibling"));
    assert!(!f.trigger_open(), "blur closes it");
    assert!(!f.shown());
}

#[test]
fn a_trigger_focused_by_a_mouse_press_does_not_stay_open() {
    require_backend!();
    let mut f = fixture();
    // A press on the trigger focuses its button by pointer.
    f.host.click(40.0, 115.0, 0, "").unwrap();
    f.settle();
    assert!(f.focused_is("button"), "the press focused the button");
    assert!(!f.trigger_open());
    assert!(!f.shown());
}

#[test]
fn escape_and_a_press_close_a_focus_opened_surface_without_changing_focus() {
    require_backend!();
    let mut f = fixture();
    f.tab();
    assert!(f.shown());
    f.escape();
    assert!(!f.trigger_open(), "Escape closes it");
    assert!(f.focused_is("button"), "the focus owner is unchanged by hiding");

    // Re-open by keyboard and close by a press on the trigger (A6: still focused afterwards).
    f.tab();
    f.tab();
    f.tab();
    assert!(f.focused_is("button") || f.focused_is("sibling"));
    f.host.click(40.0, 115.0, 0, "").unwrap();
    f.settle();
    assert!(!f.trigger_open(), "a pointer press closes it");
    assert!(f.focused_is("button"));
}

#[test]
fn escape_reaches_the_tooltip_before_the_menu_below_it() {
    require_backend!();
    let mut f = fixture();
    let menu = f.host.app().update(|cx| f.root.read(cx).menu.clone());
    let trigger = f.host.app().update(|cx| f.root.read(cx).trigger.clone());
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            menu.update(cx, |menu, cx| menu.open(window, cx));
            trigger.update(cx, |trigger, cx| trigger.show(window, cx));
        })
        .unwrap();
    f.host.flush();
    assert!(f.trigger_open());
    assert!(f.host.app().update(|cx| menu.read(cx).is_open()));
    f.escape();
    assert!(!f.trigger_open(), "the first Escape closes the tooltip");
    assert!(f.host.app().update(|cx| menu.read(cx).is_open()), "the menu is still open");
    f.escape();
    assert!(!f.host.app().update(|cx| menu.read(cx).is_open()), "the second closes the menu");
}

#[test]
fn passive_native_tooltips_do_not_take_escape_from_the_menu() {
    require_backend!();
    let mut f = fixture();
    let menu = f.host.app().update(|cx| f.root.read(cx).menu.clone());
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| menu.update(cx, |menu, cx| menu.open(window, cx)))
        .unwrap();
    f.host.flush();
    // Hover the trigger long enough for the base text tooltip to show.
    f.host.mouse_move(40.0, 115.0, None, "").unwrap();
    f.host.advance_executor_time(600.0).unwrap();
    f.host.flush();
    assert!(f.shown(), "the passive hover tooltip is visible");
    f.escape();
    assert!(!f.host.app().update(|cx| menu.read(cx).is_open()), "Escape went to the menu");
}
