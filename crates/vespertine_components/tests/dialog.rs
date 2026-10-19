//! CMP-003 native acceptance (A4-A8, R3-R15): the dialog through the production offscreen test
//! host. (A1-A2 are the JavaScript adapters' suites; A3 is the `dialog_modal_editor` example.)

mod controls_support;

use controls_support::{center, next_frame, node, nodes, tab};
use std::{cell::Cell, cell::RefCell, rc::Rc};
use vespertine::{
    AppContext as _, Context, Entity, EntityInputHandler as _, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, Role,
    StatefulInteractiveElement, Styled, Window, actions, div, px, rgb, size,
};
use vespertine_components::dialog::{
    ActionKind, Dialog, DialogAction, DialogModel, DialogPart, DismissalPolicy, limits_following,
};
use vespertine_components::floating::{FocusTarget, MenuPart, TriggerBounds, layers};
use vespertine_components::select::{Select, SelectOption};
use vespertine_components::text::TextField;
use vespertine_test_host::{HostOptions, PaintLog, TestHost, require_backend};

actions!(dialog_test, [Tab, TabPrev, PageShortcut]);

const PORTAL: u64 = 3001;
const BACKDROP: u64 = 3002;
const POPUP: u64 = 3003;
const ACTION: u64 = 3010;

fn dialog_part_id(part: DialogPart) -> u64 {
    match part {
        DialogPart::Portal => PORTAL,
        DialogPart::Backdrop => BACKDROP,
        DialogPart::Popup => POPUP,
        DialogPart::Action(ix) => ACTION + ix as u64,
    }
}

fn menu_part_id(part: MenuPart) -> u64 {
    match part {
        MenuPart::Trigger => 2000,
        MenuPart::Surface => 2001,
        MenuPart::Row(ix) => 2010 + ix as u64,
    }
}

#[derive(Default, Clone)]
struct Shared {
    log: Rc<RefCell<Vec<String>>>,
    page_clicks: Rc<Cell<u32>>,
    page_keys: Rc<Cell<u32>>,
    page_shortcuts: Rc<Cell<u32>>,
    doc_present: Rc<Cell<bool>>,
    field: Rc<RefCell<Option<Entity<TextField>>>>,
}

struct Root {
    dialog: Entity<Dialog>,
    doc: FocusHandle,
    trigger: FocusHandle,
    shared: Shared,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.shared.page_clicks.clone();
        let keys = self.shared.page_keys.clone();
        let shortcuts = self.shared.page_shortcuts.clone();
        let dialog = self.dialog.clone();
        div()
            .id("app")
            .size_full()
            .relative()
            .bg(rgb(0x2040C0))
            .on_action(cx.listener(|_, _: &Tab, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &TabPrev, window, cx| window.focus_prev(cx)))
            .on_action(move |_: &PageShortcut, _, _| shortcuts.set(shortcuts.get() + 1))
            .on_key_down(move |_, _, _| keys.set(keys.get() + 1))
            .child(
                div()
                    .id("page")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
            .child(
                div()
                    .id("trigger")
                    .absolute()
                    .left(px(10.))
                    .top(px(10.))
                    .w(px(80.))
                    .h(px(24.))
                    .track_focus(&self.trigger)
                    .role(Role::Button)
                    .aria_label("Open")
                    .child("Open")
                    .on_click(move |_, window, cx| {
                        dialog.update(cx, |dialog, cx| dialog.open(window, cx))
                    }),
            )
            .when(self.shared.doc_present.get(), |app| {
                app.child(
                    div()
                        .id("doc")
                        .absolute()
                        .left(px(10.))
                        .top(px(50.))
                        .w(px(80.))
                        .h(px(24.))
                        .track_focus(&self.doc)
                        .role(Role::Button)
                        .aria_label("Document")
                        .child("Document"),
                )
            })
            .child(self.dialog.clone())
    }
}

use vespertine::prelude::FluentBuilder as _;

struct Spec {
    modal: bool,
    policy: DismissalPolicy,
    actions: Vec<DialogAction>,
    with_select: bool,
    with_field: bool,
    with_trigger: bool,
    initial: Focus,
    last: Focus,
}

/// Which element an explicit focus target names.
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Default,
    Suppress,
    Document,
    Select,
}

impl Focus {
    fn target(
        self,
        doc: &FocusHandle,
        select: Option<&Entity<Select>>,
        cx: &vespertine::App,
    ) -> FocusTarget {
        match self {
            Focus::Default => FocusTarget::Default,
            Focus::Suppress => FocusTarget::Suppress,
            Focus::Document => FocusTarget::Handle(doc.clone()),
            Focus::Select => {
                FocusTarget::Handle(select.expect("a select").read(cx).trigger_focus().clone())
            }
        }
    }
}

impl Spec {
    fn new() -> Self {
        Self {
            modal: true,
            policy: DismissalPolicy::default(),
            actions: vec![
                DialogAction::new("cancel", "Cancel", ActionKind::Secondary),
                DialogAction::new("save", "Save", ActionKind::Primary),
            ],
            with_select: false,
            with_field: false,
            with_trigger: true,
            initial: Focus::Default,
            last: Focus::Default,
        }
    }
}

struct Fixture {
    host: TestHost,
    dialog: Entity<Dialog>,
    select: Option<Entity<Select>>,
    doc: FocusHandle,
    trigger: FocusHandle,
    shared: Shared,
}

fn options(count: usize) -> Vec<SelectOption> {
    (0..count)
        .map(|ix| SelectOption::new(format!("opt{ix}"), format!("Option {ix}")))
        .collect()
}

fn fixture(spec: Spec) -> Fixture {
    let shared = Shared::default();
    shared.doc_present.set(true);
    let slot: Rc<
        RefCell<
            Option<(
                Entity<Dialog>,
                Option<Entity<Select>>,
                FocusHandle,
                FocusHandle,
            )>,
        >,
    > = Rc::default();
    let out = slot.clone();
    let build_shared = shared.clone();
    let mut host = TestHost::with_view(HostOptions::with_size(480., 360.), move |window, cx| {
        let doc = cx.focus_handle().tab_stop(true);
        let trigger = cx.focus_handle().tab_stop(true);
        let tracker = TriggerBounds::new();
        let field = spec
            .with_field
            .then(|| cx.new(|cx| TextField::new(window, cx).value("draft")));
        *build_shared.field.borrow_mut() = field.clone();
        let select = spec.with_select.then(|| {
            let tracker = tracker.clone();
            cx.new(|cx| {
                Select::new("choice", "Choice", options(8), cx)
                    .width(px(160.))
                    .limits_provider(limits_following(&tracker))
                    .observe_bounds(Rc::new(|part, bounds, cx| {
                        PaintLog::record_bounds(cx, menu_part_id(part), bounds)
                    }))
            })
        });
        let log = build_shared.log.clone();
        let (confirm_log, cancel_log, change_log, action_log) =
            (log.clone(), log.clone(), log.clone(), log);
        let initial = spec.initial.target(&doc, select.as_ref(), cx);
        let last = spec.last.target(&doc, select.as_ref(), cx);
        let dialog = cx.new(|cx| {
            let mut dialog = Dialog::new("dialog", "Save changes?", cx)
                .body("The document has unsaved changes.")
                .model(DialogModel::new(spec.actions).policy(spec.policy))
                .modal(spec.modal)
                .popup_tracker(tracker)
                .initial_focus(initial)
                .final_focus(last)
                .observe_bounds(Rc::new(|part, bounds, cx| {
                    PaintLog::record_bounds(cx, dialog_part_id(part), bounds)
                }))
                .on_confirm(move |_, _| confirm_log.borrow_mut().push("confirm".into()))
                .on_cancel(move |_, _| cancel_log.borrow_mut().push("cancel".into()))
                .on_open_change(move |open, _, _| {
                    change_log.borrow_mut().push(format!("open={open}"))
                })
                .on_action(move |activation, _, _| {
                    action_log
                        .borrow_mut()
                        .push(format!("action:{}", activation.id))
                });
            if spec.with_trigger {
                dialog = dialog.trigger(trigger.clone());
            }
            if let Some(select) = &select {
                dialog = dialog.child(select.clone());
            }
            if let Some(field) = &field {
                dialog = dialog
                    .child(field.clone())
                    .default_action_field(field, window, cx);
            }
            dialog
        });
        *out.borrow_mut() = Some((dialog.clone(), select, doc.clone(), trigger.clone()));
        cx.new(|_| Root {
            dialog,
            doc,
            trigger,
            shared: build_shared,
        })
    })
    .expect("host");
    host.app().update(|cx| {
        cx.bind_keys([
            KeyBinding::new("tab", Tab, None),
            KeyBinding::new("shift-tab", TabPrev, None),
            KeyBinding::new("ctrl-j", PageShortcut, None),
        ])
    });
    let (dialog, select, doc, trigger) = slot.borrow_mut().take().expect("entities");
    Fixture {
        host,
        dialog,
        select,
        doc,
        trigger,
        shared,
    }
}

impl Fixture {
    fn rect(&self, id: u64) -> (f64, f64, f64, f64) {
        self.host
            .element_bounds(id)
            .map(|r| (r.x, r.y, r.width, r.height))
            .unwrap_or_else(|| panic!("element {id} was not painted"))
    }

    fn settle(&mut self) {
        self.host.flush();
        for _ in 0..3 {
            next_frame(&mut self.host);
        }
        self.host.motion_fast_forward(300.).unwrap();
        self.host.advance_executor_time(300.).unwrap();
        self.host.flush();
    }

    fn focus(&mut self, handle: &FocusHandle) {
        let window = self.host.window();
        let handle = handle.clone();
        self.host
            .app()
            .update_window(window, |_, window, cx| handle.focus(window, cx))
            .unwrap();
        self.settle();
    }

    fn open(&mut self) {
        let dialog = self.dialog.clone();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                dialog.update(cx, |dialog, cx| dialog.open(window, cx))
            })
            .unwrap();
        self.settle();
    }

    fn close(&mut self) {
        let dialog = self.dialog.clone();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                dialog.update(cx, |dialog, cx| dialog.close(window, cx))
            })
            .unwrap();
        self.settle();
    }

    fn is_open(&mut self) -> bool {
        let dialog = self.dialog.clone();
        self.host.app().update(|cx| dialog.read(cx).is_open())
    }

    fn log(&self) -> Vec<String> {
        self.shared.log.borrow().clone()
    }

    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.log.borrow_mut())
    }

    fn click(&mut self, x: f64, y: f64) {
        self.host.click(x, y, 0, "").unwrap();
        self.settle();
    }

    fn click_label(&mut self, label: &str) {
        let target = node(&mut self.host, "button", Some(label));
        let (x, y) = center(&mut self.host, &target);
        self.click(x, y);
    }

    fn key(&mut self, key: &str) {
        self.host.key_down(key, false, None).unwrap();
        self.host.key_up(key, None).unwrap();
        self.settle();
    }

    /// The accessible label of the focused node, `None` when nothing is focused.
    fn focused_label(&mut self) -> Option<String> {
        let tree = self
            .host
            .accessibility_tree()
            .unwrap()
            .expect("accessibility");
        let focus = tree["focus"].as_u64()?;
        // The root node is the default focus of an unfocused window.
        let all = nodes(&mut self.host);
        let focused = all.iter().find(|node| node.id == focus)?;
        focused
            .label()
            .map(str::to_owned)
            .or_else(|| Some(focused.role.clone()))
    }

    fn layer_count(&mut self) -> usize {
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| layers(window, cx).len())
            .unwrap()
    }
}

#[test]
fn the_modal_portal_fills_the_window_follows_a_resize_and_centers_the_popup() {
    require_backend!();
    let mut f = fixture(Spec::new());
    assert!(
        f.host.element_bounds(PORTAL).is_none(),
        "mounted only while open"
    );
    f.open();
    assert_eq!(f.rect(PORTAL), (0., 0., 480., 360.));
    assert_eq!(
        f.rect(BACKDROP),
        (0., 0., 480., 360.),
        "inset 0 on all four edges"
    );
    let popup = f.rect(POPUP);
    assert!(
        ((popup.0 + popup.2 / 2.) - 240.).abs() < 1.
            && ((popup.1 + popup.3 / 2.) - 180.).abs() < 1.,
        "centered by flex layout: {popup:?}"
    );

    // A7: the portal follows the viewport in the same frame the window changed.
    let window = f.host.window();
    f.host
        .app()
        .simulate_window_resize(window, size(px(600.), px(420.)))
        .unwrap();
    f.host.flush();
    assert_eq!(f.rect(PORTAL), (0., 0., 600., 420.));
    assert_eq!(f.rect(BACKDROP), (0., 0., 600., 420.));
    let popup = f.rect(POPUP);
    assert!(
        ((popup.0 + popup.2 / 2.) - 300.).abs() < 1.,
        "still centered: {popup:?}"
    );

    f.close();
    assert!(
        f.host.element_bounds(PORTAL).is_none(),
        "unmounted when closed"
    );
}

#[test]
fn the_scrim_dims_the_page_and_a_press_inside_the_popup_does_not_reach_the_backdrop() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    // Neutral scrim: black at 50 % over the page color 0x2040C0.
    let scale = f64::from(f.host.scale_factor());
    let image = f.host.screenshot_image().unwrap();
    let pixel = image.get_pixel((470. * scale) as u32, (350. * scale) as u32);
    assert!(
        (i32::from(pixel[2]) - 0x60).abs() <= 3 && (i32::from(pixel[0]) - 0x10).abs() <= 3,
        "page color halved by the scrim: {pixel:?}"
    );

    // A press inside the popup neither closes the dialog nor reaches the page.
    let popup = f.rect(POPUP);
    f.click(popup.0 + 20., popup.1 + 20.);
    assert!(f.is_open());
    assert_eq!(f.shared.page_clicks.get(), 0);
    // A press on the backdrop closes it (default policy) and the page never sees the press.
    f.click(470., 350.);
    assert!(!f.is_open());
    assert_eq!(f.shared.page_clicks.get(), 0);
    assert_eq!(f.take_log(), vec!["open=true", "cancel", "open=false"]);
}

#[test]
fn tab_cycles_inside_a_modal_popup_and_never_reaches_the_page() {
    require_backend!();
    let mut f = fixture(Spec {
        with_select: true,
        ..Spec::new()
    });
    f.focus(&f.doc.clone());
    f.open();
    let popup = f.dialog.clone();
    let popup_focused = |f: &mut Fixture| {
        let handle = f.dialog.clone();
        let window = f.host.window();
        f.host
            .app()
            .update_window(window, |_, window, cx| {
                handle.read(cx).popup_focus().is_focused(window)
            })
            .unwrap()
    };
    let _ = popup;
    assert!(popup_focused(&mut f), "initial focus is the popup itself");
    // The first Tab enters the first focusable descendant.
    tab(&mut f.host);
    assert_eq!(f.focused_label().as_deref(), Some("Choice: "));
    tab(&mut f.host);
    assert_eq!(f.focused_label().as_deref(), Some("Cancel"));
    tab(&mut f.host);
    assert_eq!(f.focused_label().as_deref(), Some("Save"));
    // Wraps last to first, never to the page's tab stops.
    tab(&mut f.host);
    assert_eq!(f.focused_label().as_deref(), Some("Choice: "));
    // Shift-Tab wraps first to last.
    f.host.simulate_keystrokes("shift-tab", None).unwrap();
    assert_eq!(f.focused_label().as_deref(), Some("Save"));
    f.host.simulate_keystrokes("shift-tab", None).unwrap();
    assert_eq!(f.focused_label().as_deref(), Some("Cancel"));
}

#[test]
fn a_nonmodal_dialog_leaves_the_page_interactive_and_does_not_trap_tab() {
    require_backend!();
    let mut f = fixture(Spec {
        modal: false,
        policy: DismissalPolicy {
            outside_press: false,
            escape: true,
        },
        ..Spec::new()
    });
    f.open();
    assert!(f.host.element_bounds(BACKDROP).is_none(), "no scrim");
    assert_eq!(f.rect(PORTAL), (0., 0., 480., 360.));
    // The page around the popup still receives presses.
    f.click(470., 350.);
    assert_eq!(f.shared.page_clicks.get(), 1, "pointer pass-through");
    assert!(f.is_open(), "outside press dismissal is off");
    // A press inside the popup is its own.
    let popup = f.rect(POPUP);
    f.click(popup.0 + 20., popup.1 + 20.);
    assert_eq!(f.shared.page_clicks.get(), 1);
    // Tab is not trapped: from the last action it continues to the page's tab stops.
    let mut reached_save = false;
    for _ in 0..8 {
        tab(&mut f.host);
        if f.focused_label().as_deref() == Some("Save") {
            reached_save = true;
            break;
        }
    }
    assert!(reached_save, "Tab reaches the last action");
    tab(&mut f.host);
    let label = f.focused_label();
    assert!(
        matches!(label.as_deref(), Some("Document") | Some("Open")),
        "Tab continues to the page instead of wrapping inside the popup: {label:?}"
    );
}

#[test]
fn a_nonmodal_outside_press_dismisses_when_allowed_without_consuming_the_press() {
    require_backend!();
    let mut f = fixture(Spec {
        modal: false,
        ..Spec::new()
    });
    f.open();
    f.click(470., 350.);
    assert!(!f.is_open());
    assert_eq!(
        f.shared.page_clicks.get(),
        1,
        "the press still reached the page"
    );
    assert_eq!(f.take_log(), vec!["open=true", "cancel", "open=false"]);
}

#[test]
fn a_destructive_confirmation_closes_only_through_its_explicit_buttons() {
    require_backend!();
    let mut f = fixture(Spec {
        policy: DismissalPolicy::explicit_only(),
        actions: vec![
            DialogAction::new("cancel", "Keep", ActionKind::Secondary),
            DialogAction::new("delete", "Delete", ActionKind::Destructive),
        ],
        ..Spec::new()
    });
    f.open();
    f.take_log();
    let keys_before = f.shared.page_keys.get();
    f.key("escape");
    assert!(f.is_open(), "Escape does nothing visible");
    assert_eq!(
        f.shared.page_keys.get(),
        keys_before,
        "and is consumed: the page never sees it"
    );
    assert!(f.log().is_empty(), "no cancel callback: {:?}", f.log());
    f.click(470., 350.);
    assert!(f.is_open(), "an outside press does nothing either");
    assert_eq!(f.shared.page_clicks.get(), 0);
    f.click_label("Delete");
    assert!(!f.is_open(), "only an explicit action closes it");
    assert_eq!(f.take_log(), vec!["action:delete", "open=false"]);
}

#[test]
fn escape_cancels_without_confirming_when_allowed() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    f.take_log();
    let keys_before = f.shared.page_keys.get();
    f.key("escape");
    assert!(!f.is_open());
    assert_eq!(
        f.take_log(),
        vec!["cancel", "open=false"],
        "cancel, never confirm"
    );
    assert_eq!(f.shared.page_keys.get(), keys_before, "consumed");
}

#[test]
fn a_disabled_action_does_nothing_and_a_busy_primary_keeps_the_dialog_open() {
    require_backend!();
    let mut f = fixture(Spec {
        actions: vec![
            DialogAction::new("cancel", "Cancel", ActionKind::Secondary),
            DialogAction::new("save", "Save", ActionKind::Primary).disabled(true),
        ],
        ..Spec::new()
    });
    f.open();
    f.take_log();
    // Click on the disabled action; the default-action key does nothing either.
    f.click_label("Save");
    assert!(f.is_open());
    let dialog = f.dialog.clone();
    let window = f.host.window();
    let ran = f
        .host
        .app()
        .update_window(window, |_, window, cx| {
            dialog.update(cx, |dialog, cx| dialog.activate_default(window, cx))
        })
        .unwrap();
    assert!(!ran, "no enabled primary action: nothing happens");
    assert!(f.log().is_empty(), "{:?}", f.log());
    assert!(node(&mut f.host, "button", Some("Save")).disabled());

    // Enable it; the owner marks it busy from the confirm callback (kept in the model).
    f.host.app().update(|cx| {
        dialog.update(cx, |dialog, cx| {
            dialog.set_action_disabled("save", false, cx)
        })
    });
    f.settle();
    f.click_label("Save");
    assert_eq!(f.take_log(), vec!["confirm", "action:save", "open=false"]);

    // Busy: the owner sets the busy flag before the activation finishes.
    let mut f = fixture(Spec::new());
    f.open();
    f.take_log();
    let dialog = f.dialog.clone();
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.set_action_busy("save", true, cx);
                assert!(
                    !dialog.activate("save", window, cx),
                    "a busy action cannot start"
                );
            })
        })
        .unwrap();
    f.settle();
    assert!(f.is_open());
    assert!(f.log().is_empty());
    assert!(
        node(&mut f.host, "button", Some("Save")).busy(),
        "busy is exposed"
    );
    f.click_label("Save");
    assert!(f.log().is_empty(), "no second activation while busy");

    // A primary action that turns busy during its own activation keeps the dialog open.
    let mut f = fixture(Spec::new());
    f.open();
    f.take_log();
    let dialog = f.dialog.clone();
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.set_actions(
                    vec![DialogAction::new("save", "Save", ActionKind::Primary).keeps_open(true)],
                    cx,
                );
                assert!(dialog.activate("save", window, cx));
            })
        })
        .unwrap();
    f.settle();
    assert!(f.is_open());
    assert_eq!(f.take_log(), vec!["confirm", "action:save"]);
}

#[test]
fn a_select_inside_the_dialog_is_contained_selects_without_closing_and_escapes_first() {
    require_backend!();
    let mut f = fixture(Spec {
        with_select: true,
        ..Spec::new()
    });
    f.open();
    let popup = f.rect(POPUP);
    let trigger = f.rect(menu_part_id(MenuPart::Trigger));
    f.click(trigger.0 + 20., trigger.1 + 10.);
    let card = f.rect(menu_part_id(MenuPart::Surface));
    // Contained placement inside the dialog's border box inset by 8 px (R11).
    assert!(card.0 >= popup.0 + 8. - 0.5, "left: {card:?} in {popup:?}");
    assert!(card.1 >= popup.1 + 8. - 0.5, "top: {card:?} in {popup:?}");
    assert!(
        card.0 + card.2 <= popup.0 + popup.2 - 8. + 0.5,
        "right: {card:?} in {popup:?}"
    );
    assert!(
        card.1 + card.3 <= popup.1 + popup.3 - 8. + 0.5,
        "bottom: {card:?} in {popup:?}"
    );

    // A7: the priority-1 menu paints above the portal; the portal above the page. The menu fill
    // is opaque surface color, the page around is dimmed.
    let scale = f64::from(f.host.scale_factor());
    let image = f.host.screenshot_image().unwrap();
    let at = |x: f64, y: f64| *image.get_pixel((x * scale) as u32, (y * scale) as u32);
    let menu_pixel = at(card.0 + card.2 - 4., card.1 + card.3 - 4.);
    assert!(
        menu_pixel[2] < 0x30 && menu_pixel[0] == menu_pixel[1],
        "the menu paints its neutral surface fill above the dialog: {menu_pixel:?}"
    );
    let page_pixel = at(470., 350.);
    assert!(
        page_pixel[2] < 0x70,
        "the page is dimmed by the portal: {page_pixel:?}"
    );

    // Escape closes the select first, then the next Escape closes the dialog.
    let layers_open = f.layer_count();
    assert_eq!(layers_open, 2);
    f.key("escape");
    assert!(f.is_open(), "the dialog stays open");
    assert_eq!(f.layer_count(), 1, "only the select layer closed");
    f.key("escape");
    assert!(!f.is_open());

    // Pressing a row selects it without closing the dialog or counting as a backdrop press.
    f.open();
    let trigger = f.rect(menu_part_id(MenuPart::Trigger));
    f.click(trigger.0 + 20., trigger.1 + 10.);
    let row = f.rect(menu_part_id(MenuPart::Row(0)));
    f.click(row.0 + 10., row.1 + 6.);
    assert!(f.is_open(), "selecting a row does not dismiss the dialog");
    let select = f.select.clone().unwrap();
    let chosen = f
        .host
        .app()
        .update(|cx| select.read(cx).selected_id().map(|id| id.to_string()));
    assert_eq!(chosen.as_deref(), Some("opt0"));
}

#[test]
fn focus_returns_to_the_trigger_then_the_prior_owner_then_the_window_default() {
    require_backend!();
    // The trigger is mounted: it wins over the prior owner.
    let mut f = fixture(Spec::new());
    f.focus(&f.doc.clone());
    f.open();
    f.key("escape");
    assert!(!f.is_open());
    assert_eq!(f.focused_label().as_deref(), Some("Open"), "the trigger");

    // No trigger: the previously focused element.
    let mut f = fixture(Spec {
        with_trigger: false,
        ..Spec::new()
    });
    f.focus(&f.doc.clone());
    f.open();
    f.key("escape");
    assert_eq!(
        f.focused_label().as_deref(),
        Some("Document"),
        "the prior owner"
    );

    // A backdrop press that closes the dialog restores focus as Escape does.
    let mut f = fixture(Spec {
        with_trigger: false,
        ..Spec::new()
    });
    f.focus(&f.doc.clone());
    f.open();
    f.click(470., 350.);
    assert!(!f.is_open());
    assert_eq!(
        f.focused_label().as_deref(),
        Some("Document"),
        "a backdrop press restores the prior owner"
    );

    // The captured owner was removed while the dialog was open: the trigger if mounted...
    let mut f = fixture(Spec::new());
    f.focus(&f.doc.clone());
    f.open();
    f.shared.doc_present.set(false);
    f.host.invalidate();
    f.settle();
    f.key("escape");
    assert_eq!(
        f.focused_label().as_deref(),
        Some("Open"),
        "the trigger when the prior is gone"
    );

    // ...else the window default: nothing is focused.
    let mut f = fixture(Spec {
        with_trigger: false,
        ..Spec::new()
    });
    f.focus(&f.doc.clone());
    f.open();
    f.shared.doc_present.set(false);
    f.host.invalidate();
    f.settle();
    f.key("escape");
    let label = f.focused_label();
    assert!(
        !matches!(label.as_deref(), Some("Document") | Some("Open")),
        "nothing from the page is focused: {label:?}"
    );
}

#[test]
fn explicit_initial_and_final_focus_targets_win_and_suppression_leaves_focus_alone() {
    require_backend!();
    // Explicit initial focus: the select's trigger instead of the popup.
    let mut f = fixture(Spec {
        with_select: true,
        initial: Focus::Select,
        ..Spec::new()
    });
    f.open();
    assert_eq!(f.focused_label().as_deref(), Some("Choice: "));

    // Explicit final focus beats the trigger and the prior owner.
    let mut f = fixture(Spec {
        last: Focus::Document,
        ..Spec::new()
    });
    f.focus(&f.trigger.clone());
    f.open();
    f.key("escape");
    assert_eq!(f.focused_label().as_deref(), Some("Document"));

    // A suppressed final focus moves nothing: neither the trigger nor the prior owner.
    let mut f = fixture(Spec {
        last: Focus::Suppress,
        ..Spec::new()
    });
    f.focus(&f.doc.clone());
    f.open();
    f.key("escape");
    let label = f.focused_label();
    assert!(
        !matches!(label.as_deref(), Some("Document") | Some("Open")),
        "focus was left alone: {label:?}"
    );

    // A suppressed initial focus leaves focus where it was.
    let mut f = fixture(Spec {
        initial: Focus::Suppress,
        ..Spec::new()
    });
    f.focus(&f.doc.clone());
    f.open();
    assert_eq!(f.focused_label().as_deref(), Some("Document"));
}

#[test]
fn a_modal_dialog_blocks_window_level_shortcuts_of_the_page_while_open() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.focus(&f.doc.clone());
    f.host.simulate_keystrokes("ctrl-j", None).unwrap();
    assert_eq!(
        f.shared.page_shortcuts.get(),
        1,
        "the page shortcut works before the dialog"
    );
    f.open();
    f.host.simulate_keystrokes("ctrl-j", None).unwrap();
    assert_eq!(
        f.shared.page_shortcuts.get(),
        1,
        "blocked while the modal is open"
    );
    f.close();
    f.focus(&f.doc.clone());
    f.host.simulate_keystrokes("ctrl-j", None).unwrap();
    assert_eq!(f.shared.page_shortcuts.get(), 2, "restored after it closes");
}

#[test]
fn the_dialog_exposes_its_role_heading_and_modal_key_scope() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    let dialog = node(&mut f.host, "dialog", Some("Save changes?"));
    assert_eq!(dialog.role, "dialog");
    let heading = node(&mut f.host, "heading", Some("Save changes?"));
    assert_eq!(heading.number("level"), Some(2.0), "title level 2");
    // The key-context identifier the popup carries is the core modal scope identifier.
    assert_eq!(vespertine::MODAL_SCOPE_CONTEXT, "ModalScope");
}

impl Fixture {
    fn field(&self) -> Entity<TextField> {
        self.shared.field.borrow().clone().expect("a field")
    }
}

#[test]
fn enter_in_a_field_runs_the_primary_action_unless_it_is_disabled() {
    require_backend!();
    let mut f = fixture(Spec {
        with_field: true,
        ..Spec::new()
    });
    f.open();
    f.take_log();
    // Focus the field and press Enter: the primary action runs and the dialog closes.
    let field = f.field();
    let handle = f.host.app().update(|cx| field.read(cx).focus_handle(cx));
    f.focus(&handle);
    f.key("enter");
    assert_eq!(f.take_log(), vec!["confirm", "action:save", "open=false"]);

    // A disabled primary action: Enter does nothing.
    let mut f = fixture(Spec {
        with_field: true,
        actions: vec![
            DialogAction::new("cancel", "Cancel", ActionKind::Secondary),
            DialogAction::new("save", "Save", ActionKind::Primary).disabled(true),
        ],
        ..Spec::new()
    });
    f.open();
    f.take_log();
    let field = f.field();
    let handle = f.host.app().update(|cx| field.read(cx).focus_handle(cx));
    f.focus(&handle);
    f.key("enter");
    assert!(f.is_open());
    assert!(f.log().is_empty(), "{:?}", f.log());
}

#[test]
fn form_children_keep_their_text_cursor_and_selection_across_rerenders() {
    require_backend!();
    let mut f = fixture(Spec {
        with_field: true,
        ..Spec::new()
    });
    f.open();
    let field = f.field();
    let handle = f.host.app().update(|cx| field.read(cx).focus_handle(cx));
    f.focus(&handle);
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            field.update(cx, |field, cx| {
                field.replace_text_in_range(None, "!", window, cx);
                field.set_selected_range(1..3, false, window, cx);
            })
        })
        .unwrap();
    f.settle();
    let state = |f: &mut Fixture| {
        let field = f.field();
        f.host.app().update(|cx| {
            let field = field.read(cx);
            (
                field.value_text().to_owned(),
                field.selected_range(),
                field.cursor_offset(),
            )
        })
    };
    let before = state(&mut f);
    assert_eq!(before.1, 1..3);
    // Re-render the dialog repeatedly (the owner changes an action, the window repaints).
    let dialog = f.dialog.clone();
    for _ in 0..3 {
        f.host.app().update(|cx| {
            dialog.update(cx, |dialog, cx| dialog.set_action_busy("cancel", false, cx))
        });
        f.host.invalidate();
        f.settle();
    }
    assert_eq!(
        state(&mut f),
        before,
        "the editor kept text, cursor and selection"
    );
    assert!(f.is_open());
}
