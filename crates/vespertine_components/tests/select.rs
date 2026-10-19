//! CMP-005 native acceptance (A6-A9, A10 support): the select through the production offscreen
//! test host.

use std::{cell::Cell, cell::RefCell, rc::Rc};
use vespertine::{
    AppContext as _, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, StatefulInteractiveElement, Styled, Window, div, point, px, size,
};
use vespertine_components::floating::{BoundsObserver, MenuPart};
use vespertine_components::select::{Select, SelectOption};
use vespertine_test_host::{HostOptions, PaintLog, TestHost, require_backend};

const SURFACE: u64 = 1;

fn part_id(part: MenuPart) -> u64 {
    match part {
        MenuPart::Trigger => 2000,
        MenuPart::Surface => 2000 + SURFACE,
        MenuPart::Row(ix) => 2010 + ix as u64,
    }
}

fn observer() -> BoundsObserver {
    Rc::new(|part, bounds, cx| PaintLog::record_bounds(cx, part_id(part), bounds))
}

fn options(count: usize) -> Vec<SelectOption> {
    (0..count)
        .map(|ix| SelectOption::new(format!("opt{ix}"), format!("Option {ix}")))
        .collect()
}

#[derive(Default, Clone)]
struct Shared {
    selections: Rc<RefCell<Vec<String>>>,
    background_clicks: Rc<Cell<u32>>,
}

struct Root {
    select: Entity<Select>,
    at: (f32, f32),
    shared: Shared,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.shared.background_clicks.clone();
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("background")
                    .absolute()
                    .size_full()
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(self.at.0))
                    .top(px(self.at.1))
                    .child(self.select.clone()),
            )
    }
}

struct Spec {
    window: (f64, f64),
    at: (f32, f32),
    options: Vec<SelectOption>,
    selected: Option<&'static str>,
    width: Option<f32>,
    compact: bool,
    heading: Option<&'static str>,
    rightward: bool,
    disabled: bool,
    limits: Option<Bounds<Pixels>>,
}

impl Spec {
    fn new(options: Vec<SelectOption>) -> Self {
        Self {
            window: (480., 360.),
            at: (240., 20.),
            options,
            selected: None,
            width: Some(160.),
            compact: false,
            heading: None,
            rightward: false,
            disabled: false,
            limits: None,
        }
    }
}

struct Fixture {
    host: TestHost,
    select: Entity<Select>,
    shared: Shared,
}

fn fixture(spec: Spec) -> Fixture {
    let shared = Shared::default();
    let selections = shared.selections.clone();
    let out: Rc<RefCell<Option<Entity<Select>>>> = Rc::new(RefCell::new(None));
    let sink = out.clone();
    let build = shared.clone();
    let host = TestHost::with_view(
        HostOptions::with_size(spec.window.0, spec.window.1),
        move |_, cx| {
            let select = cx.new(|cx| {
                let mut select = Select::new("select", "Letters", spec.options, cx)
                    .selected(spec.selected.map(|id| id.to_string().into()))
                    .placeholder("Pick one")
                    .compact(spec.compact)
                    .open_rightward(spec.rightward)
                    .disabled(spec.disabled)
                    .observe_bounds(observer())
                    .on_select(move |id, _, cx| {
                        selections.borrow_mut().push(id.to_string());
                        let _ = cx;
                    });
                if let Some(width) = spec.width {
                    select = select.width(px(width));
                }
                if let Some(heading) = spec.heading {
                    select = select.heading(heading);
                }
                if let Some(limits) = spec.limits {
                    select = select.limits(limits);
                }
                select
            });
            *sink.borrow_mut() = Some(select.clone());
            cx.new(|_| Root {
                select,
                at: spec.at,
                shared: build,
            })
        },
    )
    .expect("host");
    let select = out.borrow().clone().expect("select");
    Fixture {
        host,
        select,
        shared,
    }
}

impl Fixture {
    fn rect(&self, part: MenuPart) -> Option<(f64, f64, f64, f64)> {
        self.host
            .element_bounds(part_id(part))
            .map(|r| (r.x, r.y, r.width, r.height))
    }

    fn read<R>(&mut self, f: impl FnOnce(&Select) -> R) -> R {
        let select = self.select.clone();
        self.host.app().update(|cx| f(select.read(cx)))
    }

    fn focus_trigger(&mut self) {
        let select = self.select.clone();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                select.read(cx).trigger_focus().clone().focus(window, cx)
            })
            .unwrap();
        self.host.flush();
    }

    fn key(&mut self, key: &str) {
        self.host.key_down(key, false, None).unwrap();
        self.host.key_up(key, None).unwrap();
        self.host.flush();
    }

    fn key_down(&mut self, key: &str) {
        self.host.key_down(key, false, None).unwrap();
        self.host.flush();
    }

    fn key_up(&mut self, key: &str) {
        self.host.key_up(key, None).unwrap();
        self.host.flush();
    }

    fn settle(&mut self) {
        self.host.motion_fast_forward(300.).unwrap();
        self.host.advance_executor_time(300.).unwrap();
        self.host.flush();
    }

    fn open_state(&mut self) -> bool {
        self.read(|select| select.is_open())
    }

    fn highlighted(&mut self) -> Option<String> {
        self.read(|select| select.highlighted().map(|id| id.to_string()))
    }

    fn selections(&self) -> Vec<String> {
        self.shared.selections.borrow().clone()
    }
}

#[test]
fn keyboard_opens_on_the_selected_option_steps_wraps_and_commits_the_stable_id() {
    require_backend!();
    let mut opts = options(5);
    opts[1] = opts[1].clone().disabled(true);
    let mut f = fixture(Spec {
        selected: Some("opt2"),
        ..Spec::new(opts)
    });
    f.focus_trigger();
    f.key("down");
    assert!(f.open_state(), "Down on a closed select opens it");
    assert_eq!(f.highlighted().as_deref(), Some("opt2"), "without advancing");
    f.key("down");
    assert_eq!(f.highlighted().as_deref(), Some("opt3"));
    f.key("down");
    f.key("down");
    assert_eq!(f.highlighted().as_deref(), Some("opt0"), "wraps past the end");
    f.key("up");
    assert_eq!(f.highlighted().as_deref(), Some("opt4"), "wraps past the start");
    f.key("home");
    assert_eq!(f.highlighted().as_deref(), Some("opt0"));
    f.key("down");
    assert_eq!(f.highlighted().as_deref(), Some("opt2"), "the disabled option is skipped");
    f.key("end");
    assert_eq!(f.highlighted().as_deref(), Some("opt4"));

    // The owner reorders the options while the menu is open; commit still reports the id.
    let select = f.select.clone();
    f.host.app().update(|cx| {
        select.update(cx, |select, cx| {
            let mut reversed = select.options().to_vec();
            reversed.reverse();
            select.set_options(reversed, cx);
        })
    });
    f.host.flush();
    assert_eq!(f.highlighted().as_deref(), Some("opt4"), "the highlight follows its id");
    f.key("enter");
    assert_eq!(f.selections(), vec!["opt4".to_string()]);
    assert!(!f.open_state(), "the commit closes the menu");
}

#[test]
fn a_held_enter_neither_reopens_nor_commits_twice() {
    require_backend!();
    let mut f = fixture(Spec {
        selected: Some("opt1"),
        ..Spec::new(options(4))
    });
    f.focus_trigger();
    f.key_down("enter");
    assert!(f.open_state());
    // A repeat of the same press, not flagged as held (X11), does nothing.
    f.key_down("enter");
    assert!(f.open_state());
    assert!(f.selections().is_empty());
    f.key_up("enter");
    f.key_down("enter");
    assert_eq!(f.selections(), vec!["opt1".to_string()]);
    f.key_down("enter");
    assert!(!f.open_state(), "the held key does not reopen the menu");
    assert_eq!(f.selections().len(), 1);
    f.key_up("enter");
    f.key("space");
    assert!(f.open_state(), "Space opens");
    f.key("escape");
    f.settle();
    assert!(!f.open_state(), "Escape closes");
}

#[test]
fn a_selection_outside_the_viewport_is_brought_to_the_center_and_stepping_reveals() {
    require_backend!();
    let mut f = fixture(Spec {
        selected: Some("opt24"),
        ..Spec::new(options(30))
    });
    f.focus_trigger();
    f.key("down");
    f.settle();
    let card = f.rect(MenuPart::Surface).expect("menu painted");
    let row = f.rect(MenuPart::Row(24)).expect("selected row painted");
    let center = card.1 + card.3 / 2.0;
    assert!(
        (row.1 + row.3 / 2.0 - center).abs() < 20.0,
        "selected row {row:?} is near the middle of the card {card:?}"
    );
    f.key("home");
    f.settle();
    let first = f.rect(MenuPart::Row(0)).expect("first row");
    assert!(first.1 >= card.1 - 0.5, "Home reveals the first row: {first:?} in {card:?}");
}

#[test]
fn a_select_near_the_bottom_flips_above_and_stays_inside_the_limits() {
    require_backend!();
    let limits = Bounds::new(point(px(8.), px(8.)), size(px(464.), px(344.)));
    let mut f = fixture(Spec {
        at: (240., 300.),
        limits: Some(limits),
        ..Spec::new(options(30))
    });
    f.focus_trigger();
    f.key("down");
    f.settle();
    let trigger = f.rect(MenuPart::Trigger).expect("trigger");
    let card = f.rect(MenuPart::Surface).expect("card");
    assert!(card.1 + card.3 <= trigger.1 - 6.0 + 0.01, "above with >= 6 px clearance");
    assert!(card.1 >= 8.0 - 0.01 && card.1 + card.3 <= 352.0 + 0.01, "inside the limits");
    let formula = ((344.0_f64 - trigger.3) / 2.0 - 6.0).clamp(1.0, 320.0);
    assert!(card.3 <= formula + 0.01, "no taller than the formula allows: {} <= {formula}", card.3);
    // 30 rows scroll inside the card.
    let before = f.read(|select| select.scroll_offset().y);
    f.host.scroll_wheel(card.0 + 20.0, card.1 + 20.0, 0.0, -60.0, "").unwrap();
    f.host.flush();
    let after = f.read(|select| select.scroll_offset().y);
    assert_ne!(f32::from(before), f32::from(after), "the rows scroll");
}

#[test]
fn the_menu_opens_below_end_aligned_and_is_at_least_160_wide() {
    require_backend!();
    let mut f = fixture(Spec {
        width: Some(100.),
        at: (200., 20.),
        ..Spec::new(options(3))
    });
    f.focus_trigger();
    f.key("down");
    f.settle();
    let trigger = f.rect(MenuPart::Trigger).expect("trigger");
    let card = f.rect(MenuPart::Surface).expect("card");
    assert!(card.2 >= 160.0 - 0.01, "at least 160 wide: {}", card.2);
    assert!((card.0 + card.2 - (trigger.0 + trigger.2)).abs() < 0.01, "right edges align");
    assert!(card.1 >= trigger.1 + trigger.3 + 6.0 - 0.01, "below with the clearance");

    let mut g = fixture(Spec {
        width: Some(100.),
        at: (200., 20.),
        rightward: true,
        ..Spec::new(options(3))
    });
    g.focus_trigger();
    g.key("down");
    g.settle();
    let trigger = g.rect(MenuPart::Trigger).expect("trigger");
    let card = g.rect(MenuPart::Surface).expect("card");
    assert!((card.0 - trigger.0).abs() < 0.01, "start-aligned opens rightward");
}

#[test]
fn compact_selector_list_heights_follow_the_formula() {
    require_backend!();
    for (count, expected) in [(0usize, 144.0), (3, 110.0), (12, 246.0)] {
        let mut f = fixture(Spec {
            compact: true,
            at: (100., 20.),
            window: (480., 560.),
            ..Spec::new(options(count))
        });
        f.focus_trigger();
        f.key("down");
        f.settle();
        let card = f.rect(MenuPart::Surface).expect("card");
        assert!((card.3 - expected).abs() < 0.51, "{count} options: {} vs {expected}", card.3);
        assert!((card.2 - 256.0).abs() < 0.51, "256 px wide");
    }
}

#[test]
fn a_row_click_selects_it_closes_and_does_not_reach_the_content_beneath() {
    require_backend!();
    let mut f = fixture(Spec::new(options(4)));
    f.host.click(300.0, 36.0, 0, "").unwrap();
    f.host.flush();
    f.settle();
    assert!(f.open_state(), "a click on the trigger opens");
    let before = f.shared.background_clicks.get();
    let row = f.rect(MenuPart::Row(2)).expect("row 2");
    f.host.click(row.0 + 10.0, row.1 + 10.0, 0, "").unwrap();
    f.host.flush();
    assert_eq!(f.selections(), vec!["opt2".to_string()]);
    assert!(!f.open_state());
    assert_eq!(f.shared.background_clicks.get(), before, "the row click did not reach the background");
}

#[test]
fn pressing_the_open_trigger_leaves_it_closed_and_an_outside_press_dismisses() {
    require_backend!();
    let mut f = fixture(Spec::new(options(4)));
    f.host.click(300.0, 36.0, 0, "").unwrap();
    f.host.flush();
    assert!(f.open_state());
    f.host.click(300.0, 36.0, 0, "").unwrap();
    f.host.flush();
    assert!(!f.open_state(), "press and click on the trigger of an open menu leave it closed");
    f.settle();
    f.host.click(300.0, 36.0, 0, "").unwrap();
    f.host.flush();
    assert!(f.open_state());
    let before = f.shared.background_clicks.get();
    f.host.click(40.0, 300.0, 0, "").unwrap();
    f.host.flush();
    assert!(!f.open_state(), "an outside press dismisses");
    assert_eq!(f.shared.background_clicks.get(), before, "and is consumed");
}

#[test]
fn a_disabled_select_is_not_focusable_and_ignores_input() {
    require_backend!();
    let mut f = fixture(Spec {
        disabled: true,
        selected: Some("opt1"),
        ..Spec::new(options(3))
    });
    f.host.click(300.0, 36.0, 0, "").unwrap();
    f.host.flush();
    assert!(!f.open_state());
    let select = f.select.clone();
    let window = f.host.window();
    let focused = f
        .host
        .app()
        .update_window(window, |_, window, cx| {
            select.read(cx).trigger_focus().is_focused(window)
        })
        .unwrap();
    assert!(!focused, "a disabled select takes no focus");
}

#[test]
fn the_heading_stays_fixed_while_only_the_rows_scroll() {
    require_backend!();
    let mut f = fixture(Spec {
        heading: Some("Letters"),
        ..Spec::new(options(30))
    });
    f.focus_trigger();
    f.key("down");
    f.settle();
    let painted = f.host.painted_text();
    assert!(painted.iter().any(|t| t == "Letters"), "the heading is shown");
    let card = f.rect(MenuPart::Surface).expect("card");
    f.host.scroll_wheel(card.0 + 20.0, card.1 + 40.0, 0.0, -200.0, "").unwrap();
    f.host.flush();
    assert!(f.host.painted_text().iter().any(|t| t == "Letters"), "the heading did not scroll away");
}

#[test]
fn the_accessibility_tree_names_the_trigger_and_marks_one_selected_radio_row() {
    require_backend!();
    let mut f = fixture(Spec {
        selected: Some("opt1"),
        ..Spec::new(options(3))
    });
    f.focus_trigger();
    f.key("down");
    f.settle();
    let tree = f.host.accessibility_tree().unwrap().expect("a tree");
    let text = tree.to_string();
    assert!(text.contains("Letters: Option 1"), "the trigger name is `label: value`");
    assert_eq!(text.matches("menuItemRadio").count(), 3);
    assert_eq!(text.matches("\"toggled\":\"true\"").count(), 1, "one selected radio row");
}
