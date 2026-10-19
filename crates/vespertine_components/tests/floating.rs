//! CMP-002 native acceptance: floating menus, dismissal layers, trigger press tracking,
//! exit phase, contained placement, focus, hover and keyboard (A2-A8, A13), run through the
//! production offscreen test host.

use std::{cell::Cell, cell::RefCell, rc::Rc};
use vespertine::{
    AppContext as _, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Pixels, Point, Render, StatefulInteractiveElement, Styled, Window, div, point, px, size,
};
use vespertine_components::floating::{
    BoundsObserver, ContainedAlign, EscapeOutcome, MenuPart, Placement, PopupMenu, PopupMenuItem,
    layers, register_layer, top_layer, unregister_layer,
};
use vespertine_test_host::{HostOptions, PaintLog, TestHost, require_backend};

const SURFACE: u64 = 1;
const TRIGGER: u64 = 0;

/// Paint-log identity of a menu part: menu `k` owns ids `1000 * (k + 1) ..`.
fn part_id(menu: usize, part: MenuPart) -> u64 {
    let base = 1000 * (menu as u64 + 1);
    base + match part {
        MenuPart::Trigger => TRIGGER,
        MenuPart::Surface => SURFACE,
        MenuPart::Row(ix) => 10 + ix as u64,
    }
}

fn observer(menu: usize) -> BoundsObserver {
    Rc::new(move |part, bounds, cx| PaintLog::record_bounds(cx, part_id(menu, part), bounds))
}

struct MenuSpec {
    at: (f32, f32),
    size: (f32, f32),
    items: Vec<PopupMenuItem>,
    placement: Placement,
    row_gap: f32,
    selected: Option<usize>,
}

impl MenuSpec {
    fn new(at: (f32, f32), items: Vec<PopupMenuItem>) -> Self {
        Self {
            at,
            size: (180., 36.),
            items,
            placement: Placement::Below {
                align: ContainedAlign::Start,
                gap: px(4.),
            },
            row_gap: 0.,
            selected: None,
        }
    }
}

fn three_rows() -> Vec<PopupMenuItem> {
    vec![
        PopupMenuItem::new("Alpha"),
        PopupMenuItem::new("Beta").disabled(),
        PopupMenuItem::new("Gamma"),
    ]
}

#[derive(Clone, Default)]
struct Shared {
    menus: Rc<RefCell<Vec<Entity<PopupMenu>>>>,
    background_clicks: Rc<Cell<u32>>,
    root_keys: Rc<Cell<u32>>,
    selections: Rc<RefCell<Vec<(usize, usize)>>>,
    /// Wrap every trigger in a clipping ancestor of the trigger's own size.
    clip: Rc<Cell<bool>>,
}

struct Root {
    shared: Shared,
    positions: Vec<(f32, f32)>,
    /// The background button: a region under the menus that counts clicks.
    background: Bounds<Pixels>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.shared.background_clicks.clone();
        let keys = self.shared.root_keys.clone();
        let mut root = div()
            .size_full()
            .relative()
            .on_key_down(move |_, _, _| keys.set(keys.get() + 1))
            .child(
                div()
                    .id("background-button")
                    .absolute()
                    .left(self.background.origin.x)
                    .top(self.background.origin.y)
                    .w(self.background.size.width)
                    .h(self.background.size.height)
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            );
        let _ = cx;
        let clip = self.shared.clip.get();
        for (menu, (x, y)) in self.shared.menus.borrow().iter().zip(&self.positions) {
            let mut holder = div().absolute().left(px(*x)).top(px(*y));
            if clip {
                holder = holder.overflow_hidden().w(px(180.)).h(px(36.));
            }
            root = root.child(holder.child(menu.clone()));
        }
        root
    }
}

struct Fixture {
    host: TestHost,
    shared: Shared,
}

fn fixture(window: (f64, f64), background: Bounds<Pixels>, specs: Vec<MenuSpec>) -> Fixture {
    build_fixture(Shared::default(), window, background, specs)
}

fn build_fixture(
    shared: Shared,
    window: (f64, f64),
    background: Bounds<Pixels>,
    specs: Vec<MenuSpec>,
) -> Fixture {
    let build = shared.clone();
    let host = TestHost::with_view(HostOptions::with_size(window.0, window.1), move |_, cx| {
        let positions: Vec<_> = specs.iter().map(|spec| spec.at).collect();
        let count = specs.len();
        let triggers: Vec<_> = (0..count)
            .map(|_| vespertine_components::floating::TriggerBounds::new())
            .collect();
        for (ix, spec) in specs.into_iter().enumerate() {
            let selections = build.selections.clone();
            let others: Vec<_> = triggers
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != ix)
                .map(|(_, t)| t.clone())
                .collect();
            let tracker = triggers[ix].clone();
            let menu = cx.new(|cx| {
                PopupMenu::new(format!("menu-{ix}"), format!("Menu {ix}"), spec.items, cx)
                    .placement(spec.placement)
                    .size(px(spec.size.0), px(spec.size.1))
                    .row_gap(px(spec.row_gap))
                    .selected(spec.selected)
                    .trigger_tracker(tracker)
                    .exempt_triggers(others)
                    .observe_bounds(observer(ix))
                    .on_select(move |row, _, _| selections.borrow_mut().push((ix, row)))
            });
            build.menus.borrow_mut().push(menu);
        }
        cx.new(|_| Root {
            shared: build.clone(),
            positions,
            background,
        })
    })
    .expect("host");
    Fixture { host, shared }
}

impl Fixture {
    fn rect(&self, menu: usize, part: MenuPart) -> Option<(f64, f64, f64, f64)> {
        self.host
            .element_bounds(part_id(menu, part))
            .map(|r| (r.x, r.y, r.width, r.height))
    }

    fn menu(&self, ix: usize) -> Entity<PopupMenu> {
        self.shared.menus.borrow()[ix].clone()
    }

    fn open(&mut self, ix: usize) {
        let menu = self.menu(ix);
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                menu.update(cx, |menu, cx| menu.open(window, cx))
            })
            .unwrap();
        self.host.flush();
    }

    fn close(&mut self, ix: usize) {
        let menu = self.menu(ix);
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                menu.update(cx, |menu, cx| menu.close(window, cx))
            })
            .unwrap();
        self.host.flush();
    }

    fn is_open(&mut self, ix: usize) -> bool {
        let menu = self.menu(ix);
        self.host.app().update(|cx| menu.read(cx).is_open())
    }

    fn is_mounted(&mut self, ix: usize) -> bool {
        let menu = self.menu(ix);
        self.host.app().update(|cx| menu.read(cx).is_mounted())
    }

    fn active(&mut self, ix: usize) -> Option<usize> {
        let menu = self.menu(ix);
        self.host.app().update(|cx| menu.read(cx).active())
    }

    /// Lets the 140 ms entrance finish on the frozen motion clock.
    fn settle(&mut self) {
        self.host.motion_fast_forward(200.).unwrap();
        self.host.flush();
    }

    fn click(&mut self, x: f64, y: f64) {
        self.host.click(x, y, 0, "").unwrap();
        self.host.flush();
    }

    fn press(&mut self, x: f64, y: f64) {
        self.host.mouse_down(x, y, 0, "").unwrap();
        self.host.flush();
    }

    fn release(&mut self, x: f64, y: f64) {
        self.host.mouse_up(x, y, 0, "").unwrap();
        self.host.flush();
    }

    fn focused_is(&mut self, ix: usize, layer: bool) -> bool {
        let menu = self.menu(ix);
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                let menu = menu.read(cx);
                let handle = if layer {
                    menu.layer_focus().clone()
                } else {
                    menu.trigger_focus().clone()
                };
                handle.is_focused(window)
            })
            .unwrap()
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

fn assert_rect(got: Option<(f64, f64, f64, f64)>, want: (f64, f64, f64, f64), what: &str) {
    let got = got.unwrap_or_else(|| panic!("{what}: not painted"));
    assert!(
        near(got.0, want.0) && near(got.1, want.1) && near(got.2, want.2) && near(got.3, want.3),
        "{what}: got {got:?}, want {want:?}"
    );
}

fn window_bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

/// One menu at (12, 12) in a 400 x 300 window with the background button under the lower part.
fn a5_fixture() -> Fixture {
    let mut fixture = fixture(
        (400., 300.),
        window_bounds(0., 100., 400., 200.),
        vec![MenuSpec::new((12., 12.), three_rows())],
    );
    fixture.host.motion_pause();
    fixture
}

// ---- A5 -------------------------------------------------------------------------------------

#[test]
fn a5_popup_geometry_and_disabled_row_opacity() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    assert_rect(
        f.rect(0, MenuPart::Trigger),
        (12., 12., 180., 36.),
        "trigger",
    );
    assert!(
        f.rect(0, MenuPart::Surface).is_none(),
        "closed menu paints nothing"
    );
    f.click(100., 30.);
    assert!(f.is_open(0));
    f.settle();
    assert_rect(
        f.rect(0, MenuPart::Surface),
        (12., 52., 180., 104.),
        "popup",
    );
    assert_rect(f.rect(0, MenuPart::Row(0)), (16., 56., 172., 32.), "row 0");
    assert_rect(f.rect(0, MenuPart::Row(1)), (16., 88., 172., 32.), "row 1");
    assert_rect(f.rect(0, MenuPart::Row(2)), (16., 120., 172., 32.), "row 2");

    // Opacity 0.4 of the disabled row, measured on the painted pixels: its text is the enabled
    // text blended with the card fill (#1A1A1A).
    let image = f.host.screenshot_image().unwrap();
    let scale = 2.;
    let brightest = |y0: f64, y1: f64| {
        let mut best = 0u8;
        for y in (y0 * scale) as u32..(y1 * scale) as u32 {
            for x in (16. * scale) as u32..(188. * scale) as u32 {
                best = best.max(image.get_pixel(x, y).0[0]);
            }
        }
        f64::from(best)
    };
    let enabled = brightest(56., 88.);
    let disabled = brightest(88., 120.);
    let fill = f64::from(0x1Au8);
    assert!(enabled > 150., "enabled text is bright: {enabled}");
    let expected = fill + 0.4 * (enabled - fill);
    assert!(
        (disabled - expected).abs() < 8.,
        "disabled text {disabled} should be about {expected} (opacity 0.4)"
    );
}

#[test]
fn entrance_starts_two_pixels_toward_the_trigger_and_settles() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    f.click(100., 30.);
    // The motion clock is frozen at the opening instant: progress 0, 2 px up, fully faded.
    assert_rect(
        f.rect(0, MenuPart::Surface),
        (12., 50., 180., 104.),
        "entrance start",
    );
    f.host.motion_fast_forward(20.).unwrap();
    f.host.flush();
    let mid = f.rect(0, MenuPart::Surface).unwrap();
    assert!(mid.1 > 50. && mid.1 < 52., "settling: {mid:?}");
    f.settle();
    assert_rect(f.rect(0, MenuPart::Surface), (12., 52., 180., 104.), "rest");
}

// ---- A3 -------------------------------------------------------------------------------------

#[test]
fn a3_click_on_menu_content_does_not_reach_the_button_underneath() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    f.click(100., 30.);
    f.settle();
    // Row 2 (y 120..152) overlaps the background button (y >= 100).
    f.click(100., 136.);
    assert_eq!(f.shared.background_clicks.get(), 0, "no click-through");
    assert_eq!(*f.shared.selections.borrow(), vec![(0, 2)]);
    assert!(!f.is_open(0));

    // A click on a disabled row neither selects nor reaches underneath.
    f.host.advance_executor_time(300.).unwrap();
    f.click(100., 30.);
    f.settle();
    f.click(100., 104.);
    assert_eq!(f.shared.background_clicks.get(), 0);
    assert_eq!(f.shared.selections.borrow().len(), 1);
    assert!(f.is_open(0), "a disabled row keeps the menu open");
}

#[test]
fn a3_outside_press_dismisses_without_activating_the_control_below() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    f.click(100., 30.);
    f.settle();
    assert!(f.is_open(0));
    // The press lands on the background button, outside the menu.
    f.click(300., 250.);
    assert!(!f.is_open(0), "dismissed");
    assert_eq!(
        f.shared.background_clicks.get(),
        0,
        "the press was consumed"
    );
    // Without a menu the same click activates it.
    f.host.motion_fast_forward(300.).unwrap();
    f.host.advance_executor_time(300.).unwrap();
    f.click(300., 250.);
    assert_eq!(f.shared.background_clicks.get(), 1);
}

// ---- A4 -------------------------------------------------------------------------------------

#[test]
fn a4_press_and_click_on_the_trigger_of_an_open_popup_leaves_it_closed() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    f.click(100., 30.);
    f.settle();
    assert!(f.is_open(0));
    f.press(100., 30.);
    assert!(!f.is_open(0), "the press closed it");
    f.release(100., 30.);
    assert!(!f.is_open(0), "the click did not reopen it");
    // Again, while the first exit is still running: still leaves it closed.
    f.host.advance_executor_time(300.).unwrap();
    f.click(100., 30.);
    assert!(f.is_open(0));
    f.click(100., 30.);
    assert!(!f.is_open(0));
    f.click(100., 30.);
    assert!(
        !f.is_open(0),
        "a press during the exit phase leaves it closed"
    );
    // After the exit it opens again.
    f.host.advance_executor_time(300.).unwrap();
    f.click(100., 30.);
    assert!(f.is_open(0));
}

#[test]
fn a4_pressing_a_second_trigger_switches_in_one_click() {
    require_backend!();
    let mut f = fixture(
        (400., 300.),
        window_bounds(0., 200., 400., 100.),
        vec![
            MenuSpec::new((12., 12.), three_rows()),
            MenuSpec::new((212., 12.), three_rows()),
        ],
    );
    f.host.motion_pause();
    f.host.flush();
    f.click(100., 30.);
    f.settle();
    assert!(f.is_open(0));
    f.click(300., 30.);
    assert!(!f.is_open(0), "the first popup closed");
    assert!(f.is_open(1), "the second opened with the same click");
    assert_eq!(f.shared.background_clicks.get(), 0);
    // The first trigger pressed during its own exit phase still counts as pressing an open
    // popup: it stays closed.
    f.click(100., 30.);
    assert!(!f.is_open(0));
    // After the exit, back again in one click.
    f.host.advance_executor_time(300.).unwrap();
    f.click(100., 30.);
    assert!(f.is_open(0) && !f.is_open(1));
}

// ---- A8 -------------------------------------------------------------------------------------

#[test]
fn a8_exit_phase_paints_swallows_clicks_and_lets_keys_fall_through() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    f.click(100., 30.);
    f.settle();
    // While open, stepping keys are consumed by the menu.
    let before = f.shared.root_keys.get();
    f.host.key_down("down", false, None).unwrap();
    f.host.flush();
    assert_eq!(f.shared.root_keys.get(), before, "open menu consumes Down");
    assert_eq!(f.active(0), Some(0));

    f.close(0);
    assert!(!f.is_open(0));
    assert!(f.is_mounted(0), "still mounted during the exit");
    f.host.motion_fast_forward(30.).unwrap();
    f.host.flush();
    assert!(
        f.rect(0, MenuPart::Surface).is_some(),
        "still painted mid-exit"
    );
    // A click on a row does nothing and does not reach what is underneath.
    f.click(100., 136.);
    assert!(f.shared.selections.borrow().is_empty());
    assert_eq!(f.shared.background_clicks.get(), 0);
    // Key events fall through to the rest of the window.
    let before = f.shared.root_keys.get();
    f.host.key_down("down", false, None).unwrap();
    f.host.flush();
    assert!(
        f.shared.root_keys.get() > before,
        "closing menu does not take keys"
    );
    // The reap removes it.
    f.host.motion_fast_forward(200.).unwrap();
    f.host.advance_executor_time(200.).unwrap();
    assert!(!f.is_mounted(0));
    assert!(f.rect(0, MenuPart::Surface).is_none());
}

#[test]
fn a8_reopen_mid_exit_survives_the_stale_reap() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    f.open(0);
    f.settle();
    f.close(0);
    f.host.advance_executor_time(50.).unwrap();
    assert!(f.is_mounted(0) && !f.is_open(0));
    f.open(0);
    assert!(f.is_open(0));
    f.host.advance_executor_time(500.).unwrap();
    assert!(f.is_open(0), "the stale reap left the reopened popup alone");
    // Re-close: only the second close's reap drops it.
    f.close(0);
    f.host.advance_executor_time(60.).unwrap();
    assert!(f.is_mounted(0));
    f.host.advance_executor_time(100.).unwrap();
    assert!(!f.is_mounted(0));
}

#[test]
fn a8_reduced_motion_unmounts_without_waiting() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.app().update(|cx| cx.set_reduce_motion(true));
    f.host.flush();
    f.open(0);
    // No entrance: at rest in the first frame.
    assert_rect(
        f.rect(0, MenuPart::Surface),
        (12., 52., 180., 104.),
        "no settle travel",
    );
    f.close(0);
    f.host.flush();
    assert!(!f.is_mounted(0), "unmounted with no exit wait");
    assert!(f.rect(0, MenuPart::Surface).is_none());
}

// ---- A2: Escape ownership ---------------------------------------------------------------------

#[test]
fn a2_escape_goes_to_the_top_layer_only_and_is_consumed() {
    require_backend!();
    let mut f = fixture(
        (400., 300.),
        window_bounds(0., 200., 400., 100.),
        vec![
            MenuSpec::new((12., 12.), three_rows()),
            MenuSpec::new((212., 12.), three_rows()),
        ],
    );
    f.host.motion_pause();
    f.host.flush();
    // Two menus open at once (the second opened from inside the first, as a nested popup would).
    f.open(0);
    f.open(1);
    let window = f.host.window();
    let stack = f
        .host
        .app()
        .update_window(window, |_, window, cx| layers(window, cx))
        .unwrap();
    assert_eq!(stack.len(), 2, "registration order: {stack:?}");

    // A tooltip-like layer registered above both receives Escape first.
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let tooltip = {
        let log = log.clone();
        f.host
            .app()
            .update_window(window, |_, window, cx| {
                register_layer(
                    window,
                    cx,
                    Rc::new(move |window, cx| {
                        log.borrow_mut().push("tooltip");
                        let id = top_layer(window, cx).expect("registered");
                        unregister_layer(window, cx, id);
                        EscapeOutcome::Dismissed
                    }),
                )
            })
            .unwrap()
    };
    let before = f.shared.root_keys.get();
    f.host.key_down("escape", false, None).unwrap();
    f.host.flush();
    assert_eq!(*log.borrow(), vec!["tooltip"]);
    assert!(f.is_open(0) && f.is_open(1), "the menus did not see it");
    let _ = tooltip;

    f.host.key_down("escape", false, None).unwrap();
    f.host.flush();
    assert!(
        f.is_open(0) && !f.is_open(1),
        "second Escape closes the top menu only"
    );
    f.host.key_down("escape", false, None).unwrap();
    f.host.flush();
    assert!(!f.is_open(0) && !f.is_open(1));
    assert_eq!(
        f.shared.root_keys.get(),
        before,
        "no window-root listener saw an Escape that a layer consumed"
    );
    // With no layer left, Escape is an ordinary key again.
    f.host.key_down("escape", false, None).unwrap();
    assert_eq!(f.shared.root_keys.get(), before + 1);
}

#[test]
fn escape_step_back_keeps_the_layer_and_hides_focus_indicators() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    let window = f.host.window();
    let depth = Rc::new(Cell::new(2u32));
    let id = {
        f.host
            .app()
            .update_window(window, |_, window, cx| {
                register_layer(
                    window,
                    cx,
                    Rc::new(move |window, cx| {
                        if depth.get() > 0 {
                            depth.set(depth.get() - 1);
                            EscapeOutcome::SteppedBack
                        } else {
                            let top = top_layer(window, cx).unwrap();
                            unregister_layer(window, cx, top);
                            EscapeOutcome::Dismissed
                        }
                    }),
                )
            })
            .unwrap()
    };
    f.host.key_down("escape", false, None).unwrap();
    f.host.flush();
    let (still, keyboard) = f
        .host
        .app()
        .update_window(window, |_, window, cx| {
            (
                layers(window, cx).contains(&id),
                window.last_input_was_keyboard(),
            )
        })
        .unwrap();
    assert!(still, "a step-back keeps the layer");
    assert!(
        !keyboard,
        "Escape that merely steps back shows no focus indicators"
    );
    // The next navigation key brings them back.
    f.host.key_down("down", false, None).unwrap();
    let keyboard = f
        .host
        .app()
        .update_window(window, |_, window, _| window.last_input_was_keyboard())
        .unwrap();
    assert!(keyboard);
    f.host.key_down("escape", false, None).unwrap();
    f.host.key_down("escape", false, None).unwrap();
    let gone = f
        .host
        .app()
        .update_window(window, |_, window, cx| !layers(window, cx).contains(&id))
        .unwrap();
    assert!(gone);
}

// ---- Focus (R9, R10) --------------------------------------------------------------------------

#[test]
fn opening_focus_moves_after_attach_and_closing_restores_the_prior_owner() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    let menu = f.menu(0);
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            menu.read(cx).trigger_focus().clone().focus(window, cx)
        })
        .unwrap();
    f.host.flush();
    assert!(f.focused_is(0, false));
    // Open without letting frames run: the layer's subtree is not attached yet, focus has not
    // moved.
    let menu2 = f.menu(0);
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            menu2.update(cx, |menu, cx| menu.open(window, cx))
        })
        .unwrap();
    assert!(
        f.focused_is(0, false),
        "focus stays until the layer is attached"
    );
    f.host.flush();
    assert!(
        f.focused_is(0, true),
        "the layer takes focus after attaching"
    );
    // Closing returns focus to the captured owner.
    f.close(0);
    assert!(f.focused_is(0, false), "focus returned to the trigger");
}

#[test]
fn closing_does_not_steal_focus_that_moved_elsewhere() {
    require_backend!();
    let mut f = fixture(
        (400., 300.),
        window_bounds(0., 200., 400., 100.),
        vec![
            MenuSpec::new((12., 12.), three_rows()),
            MenuSpec::new((212., 12.), three_rows()),
        ],
    );
    f.host.motion_pause();
    f.host.flush();
    let window = f.host.window();
    let (a, b) = (f.menu(0), f.menu(1));
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            a.read(cx).trigger_focus().clone().focus(window, cx)
        })
        .unwrap();
    f.open(0);
    assert!(f.focused_is(0, true));
    // Something else obtains focus before the layer closes (an outside press on a control).
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            b.read(cx).trigger_focus().clone().focus(window, cx)
        })
        .unwrap();
    f.close(0);
    assert!(
        f.focused_is(1, false),
        "the focus the press obtained is kept"
    );
}

#[test]
fn switching_triggers_with_one_click_focuses_the_new_trigger() {
    require_backend!();
    let mut f = fixture(
        (400., 300.),
        window_bounds(0., 200., 400., 100.),
        vec![
            MenuSpec::new((12., 12.), three_rows()),
            MenuSpec::new((212., 12.), three_rows()),
        ],
    );
    f.host.motion_pause();
    f.host.flush();
    f.click(100., 30.);
    f.settle();
    f.click(300., 30.);
    assert!(f.is_open(1));
    assert!(
        f.focused_is(1, true) || f.focused_is(1, false),
        "focus belongs to the second menu"
    );
    assert!(!f.focused_is(0, true));
}

// ---- Keyboard and pointer highlight (R22-R24) ----------------------------------------------------

#[test]
fn keyboard_stepping_wraps_skips_disabled_rows_and_hover_shares_one_highlight() {
    require_backend!();
    let mut f = a5_fixture();
    f.host.flush();
    f.open(0);
    f.settle();
    assert_eq!(f.active(0), None);
    f.host.key_down("down", false, None).unwrap();
    assert_eq!(f.active(0), Some(0));
    f.host.key_down("down", false, None).unwrap();
    assert_eq!(f.active(0), Some(2), "the disabled row is skipped");
    f.host.key_down("down", false, None).unwrap();
    assert_eq!(f.active(0), Some(0), "wraps");
    f.host.key_down("up", false, None).unwrap();
    assert_eq!(f.active(0), Some(2), "wraps backward");
    f.host.key_down("home", false, None).unwrap();
    assert_eq!(f.active(0), Some(0));
    f.host.key_down("end", false, None).unwrap();
    assert_eq!(f.active(0), Some(2));
    f.host.simulate_keystrokes("ctrl-p", None).unwrap();
    assert_eq!(f.active(0), Some(0), "Ctrl-P is Up");
    f.host.simulate_keystrokes("ctrl-n", None).unwrap();
    assert_eq!(f.active(0), Some(2), "Ctrl-N is Down");

    // Pointer motion takes the highlight; a disabled row does not.
    f.host.mouse_move(100., 70., None, "").unwrap();
    f.host.flush();
    assert_eq!(
        f.active(0),
        Some(0),
        "one highlight: hover replaced the keyboard row"
    );
    f.host.mouse_move(100., 104., None, "").unwrap();
    f.host.flush();
    assert_eq!(
        f.active(0),
        Some(0),
        "a disabled row never takes the highlight"
    );
    // Enter accepts the highlighted row.
    f.host.key_down("end", false, None).unwrap();
    f.host.key_down("enter", false, None).unwrap();
    f.host.flush();
    assert_eq!(*f.shared.selections.borrow(), vec![(0, 2)]);
    assert!(!f.is_open(0));
}

// ---- Placement variants (R20) and A13 ------------------------------------------------------------

fn placed(placement: Placement, at: (f32, f32), window: (f64, f64)) -> Fixture {
    let mut spec = MenuSpec::new(at, three_rows());
    spec.placement = placement;
    spec.size = (100., 30.);
    let mut f = fixture(window, window_bounds(0., 0., 1., 1.), vec![spec]);
    f.host.motion_pause();
    f.host.flush();
    f.open(0);
    f.settle();
    f
}

#[test]
fn below_end_above_and_beside_placements() {
    require_backend!();
    // Trigger 100 x 30 at (150, 120); the card is 100 wide (trigger width), 104 high.
    let end = placed(
        Placement::Below {
            align: ContainedAlign::End,
            gap: px(6.),
        },
        (150., 120.),
        (500., 400.),
    );
    assert_rect(
        end.rect(0, MenuPart::Surface),
        (150., 156., 100., 104.),
        "below end",
    );
    let above = placed(
        Placement::Above {
            align: ContainedAlign::Start,
            gap: px(6.),
        },
        (150., 220.),
        (500., 400.),
    );
    assert_rect(
        above.rect(0, MenuPart::Surface),
        (150., 110., 100., 104.),
        "above",
    );
    let right = placed(
        Placement::Beside {
            left: false,
            offset: px(6.),
        },
        (150., 120.),
        (500., 400.),
    );
    assert_rect(
        right.rect(0, MenuPart::Surface),
        (256., 120., 100., 104.),
        "beside right",
    );
    let left = placed(
        Placement::Beside {
            left: true,
            offset: px(6.),
        },
        (250., 120.),
        (500., 400.),
    );
    assert_rect(
        left.rect(0, MenuPart::Surface),
        (144., 120., 100., 104.),
        "beside left",
    );
    let at = placed(
        Placement::At(point(px(40.), px(50.))),
        (150., 120.),
        (500., 400.),
    );
    assert_rect(
        at.rect(0, MenuPart::Surface),
        (40., 50., 100., 104.),
        "at a point",
    );
}

#[test]
fn menus_snap_to_the_window_with_an_eight_pixel_margin() {
    require_backend!();
    // Bottom-right corner trigger: below does not fit, the surface snaps inside the margin.
    let f = placed(
        Placement::Below {
            align: ContainedAlign::Start,
            gap: px(6.),
        },
        (390., 280.),
        (400., 300.),
    );
    let (x, y, w, h) = f.rect(0, MenuPart::Surface).unwrap();
    assert!(near(x + w, 400. - 8.), "right margin: {x} {w}");
    assert!(near(y + h, 300. - 8.), "bottom margin: {y} {h}");
}

/// A13 (test counterpart of the `floating_menus` example): menus at all four corners of the
/// viewport; switching triggers takes one click and nothing clicks through.
#[test]
fn a13_four_corner_menus_switch_in_one_click_without_click_through() {
    require_backend!();
    let (w, h) = (600., 400.);
    let corners = [(10., 10.), (410., 10.), (10., 350.), (410., 350.)];
    let specs = corners
        .iter()
        .map(|&at| {
            let mut spec = MenuSpec::new(at, three_rows());
            spec.size = (180., 36.);
            spec.row_gap = 2.;
            spec.placement = Placement::Below {
                align: ContainedAlign::Start,
                gap: px(6.),
            };
            spec
        })
        .collect();
    // A background button fills the middle of the window and must never see a click.
    let mut f = fixture((w, h), window_bounds(0., 60., 600., 280.), specs);
    f.host.motion_pause();
    f.host.flush();
    for (ix, (x, y)) in corners.iter().enumerate() {
        f.click(f64::from(*x) + 90., f64::from(*y) + 18.);
        assert!(f.is_open(ix), "corner {ix} opened");
        f.settle();
        let (sx, sy, sw, sh) = f.rect(ix, MenuPart::Surface).expect("painted");
        assert!(
            sx >= 8. - 0.01
                && sy >= 8. - 0.01
                && sx + sw <= w - 8. + 0.01
                && sy + sh <= h - 8. + 0.01,
            "corner {ix}: surface {sx} {sy} {sw} {sh} stays inside the window margin"
        );
        for other in 0..4 {
            assert_eq!(f.is_open(other), other == ix, "exactly one menu open");
        }
        // Click a row that lies over the background button: nothing clicks through.
        let row = f.rect(ix, MenuPart::Row(2)).unwrap();
        f.click(row.0 + 20., row.1 + 10.);
        assert_eq!(f.shared.background_clicks.get(), 0);
        assert_eq!(f.shared.selections.borrow().last(), Some(&(ix, 2)));
    }
    // Switching: open the first, click the last trigger.
    f.host.advance_executor_time(300.).unwrap();
    f.click(100., 28.);
    f.settle();
    f.click(500., 368.);
    assert!(!f.is_open(0) && f.is_open(3), "one click switched the menu");
    assert_eq!(
        f.shared.background_clicks.get(),
        0,
        "no click-through while switching"
    );
}

// ---- A7 / R19 / R21 contained menus ------------------------------------------------------------

fn thirty_rows() -> Vec<PopupMenuItem> {
    (0..30)
        .map(|n| PopupMenuItem::new(format!("Row {n}")))
        .collect()
}

fn contained(selected: Option<usize>) -> Fixture {
    let mut spec = MenuSpec::new((300., 300.), thirty_rows());
    spec.size = (200., 34.);
    spec.row_gap = 0.;
    spec.selected = selected;
    spec.placement = Placement::Contained {
        limits: window_bounds(100., 80., 600., 400.),
        align: ContainedAlign::Start,
    };
    let mut f = fixture((800., 600.), window_bounds(0., 0., 1., 1.), vec![spec]);
    f.host.motion_pause();
    f.host.flush();
    f
}

#[test]
fn a7_tall_contained_menu_is_capped_sits_above_and_scrolls_without_the_background() {
    require_backend!();
    let mut f = contained(None);
    f.click(400., 317.);
    f.settle();
    let (x, y, w, h) = f.rect(0, MenuPart::Surface).expect("painted");
    assert!(h <= 177.01, "at most 177 px high: {h}");
    assert!(near(h, 177.), "a 30-row menu fills the cap: {h}");
    assert!(
        y + h <= 300. - 6. + 0.01,
        "above the trigger with 6 px clearance: {y} {h}"
    );
    assert!(y >= 80. - 0.01 && x >= 100. - 0.01 && x + w <= 700. + 0.01);
    // Wheel over the rows scrolls the menu and nothing behind it.
    let menu = f.menu(0);
    let offset = |f: &mut Fixture| -> Point<Pixels> {
        f.host.app().update(|cx| menu.read(cx).scroll_offset())
    };
    assert_eq!(offset(&mut f).y, px(0.));
    f.host.scroll_wheel(x + 60., y + 60., 0., -80., "").unwrap();
    f.host.flush();
    assert!(offset(&mut f).y < px(0.), "the rows scrolled");
    // The highlight stays with the keyboard/pointer motion, not with rows moving under a resting
    // pointer.
    assert_eq!(f.active(0), None);
}

#[test]
fn contained_menu_scrolls_the_selected_row_into_view_on_open() {
    require_backend!();
    let mut f = contained(Some(25));
    f.click(400., 317.);
    f.settle();
    let (_, sy, _, sh) = f.rect(0, MenuPart::Surface).unwrap();
    let (_, ry, _, rh) = f.rect(0, MenuPart::Row(25)).expect("selected row painted");
    assert!(
        ry >= sy - 0.01 && ry + rh <= sy + sh + 0.01,
        "row 25 is visible: {ry} in {sy}+{sh}"
    );
    // Stepping with the keyboard keeps the active row visible (nearest edge).
    for _ in 0..3 {
        f.host.key_down("down", false, None).unwrap();
    }
    f.host.flush();
    let (_, ry, _, rh) = f.rect(0, MenuPart::Row(28)).expect("row 28 painted");
    assert!(
        ry >= sy - 0.01 && ry + rh <= sy + sh + 0.01,
        "row 28 is visible"
    );
}

#[test]
fn contained_menu_below_end_alignment_stays_inside_the_limits() {
    require_backend!();
    let mut spec = MenuSpec::new((700., 100.), three_rows());
    spec.size = (100., 34.);
    spec.placement = Placement::Contained {
        limits: window_bounds(100., 80., 700., 400.),
        align: ContainedAlign::End,
    };
    spec.row_gap = 0.;
    let mut f = fixture((900., 600.), window_bounds(0., 0., 1., 1.), vec![spec]);
    f.host.motion_pause();
    f.host.flush();
    f.open(0);
    f.settle();
    let (x, y, w, h) = f.rect(0, MenuPart::Surface).unwrap();
    // Trigger right edge at 800, menu width 100 (trigger width): end alignment.
    assert!(
        near(x + w, 800.),
        "end aligned to the trigger's right edge: {x} {w}"
    );
    assert!(near(y, 134. + 6.), "below with the 6 px gap: {y}");
    assert!(y + h <= 480.01);
}

// ---- R14 nested layers -------------------------------------------------------------------------

#[test]
fn nested_layer_presses_reach_the_parent_and_presses_inside_it_do_not_dismiss_the_parent() {
    require_backend!();
    let parent = MenuSpec::new((12., 12.), three_rows());
    let mut nested = MenuSpec::new((200., 120.), three_rows());
    nested.size = (100., 30.);
    nested.placement = Placement::Beside {
        left: false,
        offset: px(6.),
    };
    let mut f = fixture(
        (500., 400.),
        window_bounds(0., 250., 500., 150.),
        vec![parent, nested],
    );
    f.host.motion_pause();
    f.host.flush();
    // The nested menu is opened from inside the parent (both open, nested above).
    f.open(0);
    f.open(1);
    f.settle();
    let (nx, ny, _, _) = f.rect(1, MenuPart::Row(2)).expect("nested row painted");

    // A press inside the nested layer is outside the parent's own bounds, yet the parent stays.
    f.click(nx + 20., ny + 10.);
    assert_eq!(*f.shared.selections.borrow(), vec![(1, 2)]);
    assert!(
        f.is_open(0),
        "a press inside a layer above does not dismiss the parent"
    );
    f.open(1);
    f.settle();
    // The nested layer's own outside press (on a parent row) reaches the parent's control.
    f.click(100., 136.);
    assert!(!f.is_open(1), "the nested layer dismissed");
    assert!(
        f.shared.selections.borrow().contains(&(0, 2)),
        "the press reached the parent's row: {:?}",
        f.shared.selections.borrow()
    );
}

#[test]
fn press_outside_every_layer_is_consumed_by_the_top_level_layer() {
    require_backend!();
    let parent = MenuSpec::new((12., 12.), three_rows());
    let mut nested = MenuSpec::new((200., 120.), three_rows());
    nested.placement = Placement::Beside {
        left: false,
        offset: px(6.),
    };
    let mut f = fixture(
        (500., 400.),
        window_bounds(0., 250., 500., 150.),
        vec![parent, nested],
    );
    f.host.motion_pause();
    f.host.flush();
    f.open(0);
    f.open(1);
    f.settle();
    f.click(300., 300.);
    assert!(!f.is_open(0) && !f.is_open(1), "both dismissed");
    assert_eq!(
        f.shared.background_clicks.get(),
        0,
        "the enclosing layer consumed the press"
    );
}

// ---- R5 ----------------------------------------------------------------------------------------

#[test]
fn floating_content_neither_reserves_flow_space_nor_clips_to_the_trigger_ancestors() {
    require_backend!();
    let shared_clip = Rc::new(Cell::new(true));
    let mut f = {
        let mut f = fixture_with_clip(shared_clip);
        f.host.motion_pause();
        f.host.flush();
        f
    };
    let before = f.rect(0, MenuPart::Trigger);
    f.click(100., 30.);
    f.settle();
    assert_eq!(
        f.rect(0, MenuPart::Trigger),
        before,
        "no flow space reserved"
    );
    assert_rect(
        f.rect(0, MenuPart::Row(2)),
        (16., 120., 172., 32.),
        "row outside the clip",
    );
    // The row lies outside the clipping ancestor (36 px tall) and still takes the click.
    f.click(100., 136.);
    assert_eq!(*f.shared.selections.borrow(), vec![(0, 2)]);
}

fn fixture_with_clip(clip: Rc<Cell<bool>>) -> Fixture {
    let shared = Shared {
        clip,
        ..Shared::default()
    };
    build_fixture(
        shared,
        (400., 300.),
        window_bounds(0., 100., 400., 200.),
        vec![MenuSpec::new((12., 12.), three_rows())],
    )
}
