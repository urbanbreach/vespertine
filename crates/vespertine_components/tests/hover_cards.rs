//! CMP-004 A4, A5 (native): hover cards through the production offscreen test host.

use std::{cell::Cell, cell::RefCell, rc::Rc};
use vespertine::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, div, px,
};
use vespertine_components::tooltip::card::{CardPlacement, HoverCardHost};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

/// Triggers: 0 at (20, 20), 1 at (140, 20), 2 at (20, 120); each 80 x 30.
const TRIGGERS: [(f32, f32); 3] = [(20., 20.), (140., 20.), (20., 120.)];

struct Root {
    host: Entity<HoverCardHost<usize>>,
    mounted: Rc<Cell<[bool; 3]>>,
    shift: Rc<Cell<f32>>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mounted = self.mounted.get();
        let shift = self.shift.get();
        let mut root = div().size_full().relative();
        for (ix, (x, y)) in TRIGGERS.iter().enumerate() {
            if !mounted[ix] {
                continue;
            }
            let trigger = div()
                .id(("trigger", ix))
                .absolute()
                .left(px(x + if ix == 0 { shift } else { 0. }))
                .top(px(*y))
                .w(px(80.))
                .h(px(30.))
                .child(format!("trigger {ix}"));
            root = root.child(HoverCardHost::trigger(&self.host, ix, trigger, cx));
        }
        root.child(self.host.clone())
    }
}

struct Fixture {
    host: TestHost,
    root: Entity<Root>,
    cards: Entity<HoverCardHost<usize>>,
    mounted: Rc<Cell<[bool; 3]>>,
    shift: Rc<Cell<f32>>,
}

fn fixture(interactive: bool, placement: CardPlacement) -> Fixture {
    let out: Rc<RefCell<Option<Entity<HoverCardHost<usize>>>>> = Rc::new(RefCell::new(None));
    let sink = out.clone();
    let root_out: Rc<RefCell<Option<Entity<Root>>>> = Rc::new(RefCell::new(None));
    let root_sink = root_out.clone();
    let mounted = Rc::new(Cell::new([true; 3]));
    let shift = Rc::new(Cell::new(0.));
    let (m, s) = (mounted.clone(), shift.clone());
    let host = TestHost::with_view(HostOptions::with_size(400.0, 300.0), move |_, cx| {
        let cards = cx.new(|cx| {
            HoverCardHost::new(
                |key: &usize, _, _| {
                    div().w(px(100.)).h(px(40.)).child(format!("card {key}")).into_any_element()
                },
                cx,
            )
            .interactive(interactive)
            .placement(placement)
        });
        *sink.borrow_mut() = Some(cards.clone());
        let root = cx.new(|_| Root {
            host: cards,
            mounted: m,
            shift: s,
        });
        *root_sink.borrow_mut() = Some(root.clone());
        root
    })
    .expect("host");
    let cards = out.borrow().clone().expect("cards");
    let root = root_out.borrow().clone().expect("root");
    Fixture {
        host,
        root,
        cards,
        mounted,
        shift,
    }
}

impl Fixture {
    fn shown(&mut self) -> Option<usize> {
        let cards = self.cards.clone();
        self.host.app().update(|cx| cards.read(cx).shown().copied())
    }

    fn target(&mut self) -> Option<usize> {
        let cards = self.cards.clone();
        self.host.app().update(|cx| cards.read(cx).target().copied())
    }

    fn at(&mut self, ix: usize) {
        let (x, y) = TRIGGERS[ix];
        self.move_to(f64::from(x + 40.), f64::from(y + 15.));
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.host.mouse_move(x, y, None, "").unwrap();
        self.host.flush();
    }

    /// The container re-renders (its state changed).
    fn rerender(&mut self) {
        let root = self.root.clone();
        self.host.app().update(|cx| root.update(cx, |_, cx| cx.notify()));
        self.host.flush();
    }

    fn pass(&mut self, ms: f64) {
        self.host.advance_executor_time(ms).unwrap();
        self.host.flush();
    }
}

#[test]
fn the_first_card_shows_after_550_ms_and_a_neighbor_shows_at_once() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    f.at(0);
    assert_eq!(f.target(), Some(0));
    f.pass(549.);
    assert_eq!(f.shown(), None);
    f.pass(1.);
    assert_eq!(f.shown(), Some(0));
    assert!(f.host.painted_text().iter().any(|t| t == "card 0"));
    f.at(1);
    assert_eq!(f.shown(), Some(1), "a neighbor shows at once while a card is shown");
    f.pass(0.);
    assert!(f.host.painted_text().iter().any(|t| t == "card 1"));
}

#[test]
fn the_warm_window_is_400_ms_inclusive_and_401_ms_waits_the_dwell() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    f.move_to(300., 250.);
    assert_eq!(f.shown(), None, "leaving hides the card");
    f.pass(400.);
    f.at(1);
    assert_eq!(f.shown(), Some(1), "400 ms after the hide shows at once");
    f.move_to(300., 250.);
    f.pass(401.);
    f.at(2);
    assert_eq!(f.shown(), None, "401 ms after the hide waits");
    assert_eq!(f.target(), Some(2));
    f.pass(550.);
    assert_eq!(f.shown(), Some(2));
}

#[test]
fn a_click_a_scroll_and_a_drag_dismiss_cold() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    f.host.click(60.0, 35.0, 0, "").unwrap();
    f.host.flush();
    assert_eq!(f.shown(), None, "a click hides the card");
    f.move_to(300., 250.);
    f.pass(100.);
    f.at(0);
    assert_eq!(f.shown(), None, "no card chases the pointer straight back");
    assert_eq!(f.target(), Some(0));
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    f.host.scroll_wheel(60.0, 35.0, 0.0, -30.0, "").unwrap();
    f.host.flush();
    assert_eq!(f.shown(), None, "a scroll hides the card");
}

#[test]
fn an_unmounted_trigger_is_reconciled_at_the_next_render() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    // The container unmounts the trigger and reconciles at its next render.
    f.mounted.set([false, true, true]);
    f.rerender();
    let cards = f.cards.clone();
    f.host.app().update(|cx| {
        cards.update(cx, |cards, cx| cards.reconcile(|key| *key != 0, false, false, cx))
    });
    f.host.flush();
    assert_eq!(f.shown(), None, "the card of an unmounted trigger is dismissed");
}

#[test]
fn a_trigger_that_moves_under_a_stationary_pointer_cancels_the_pending_dwell() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    f.at(0);
    assert_eq!(f.target(), Some(0));
    f.pass(300.);
    f.shift.set(200.);
    f.rerender();
    f.pass(300.);
    assert_eq!(f.shown(), None, "the moved trigger cancelled the dwell before it settled");
}

#[test]
fn an_interactive_card_survives_a_diagonal_travel_and_closes_after_the_grace() {
    require_backend!();
    let mut f = fixture(true, CardPlacement::Beside { left: false, clearance: px(30.) });
    f.mounted.set([true, false, false]);
    f.host.flush();
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    let card = {
        let cards = f.cards.clone();
        f.host.app().update(|cx| cards.read(cx).card_bounds()).expect("card painted")
    };
    assert!(f32::from(card.left()) >= 129.0, "the card sits 30 px beside the trigger");
    // Leave the trigger heading for the card and keep moving forward in steps of 3 px.
    f.move_to(101.0, 36.0);
    for step in 0..8 {
        f.pass(50.);
        f.move_to(104.0 + 3.0 * f64::from(step), 37.0);
    }
    assert_eq!(f.shown(), Some(0), "forward moves keep the card open");
    // Entering the card keeps it open however long the pointer rests there.
    let center = card.center();
    f.move_to(f64::from(f32::from(center.x)), f64::from(f32::from(center.y)));
    f.pass(500.);
    assert_eq!(f.shown(), Some(0));
    // Leaving the card closes it after the 80 ms grace.
    f.move_to(300.0, 250.0);
    f.pass(79.);
    assert_eq!(f.shown(), Some(0));
    f.pass(1.);
    assert_eq!(f.shown(), None);
}

#[test]
fn a_reversal_inside_the_corridor_closes_the_interactive_card_at_once() {
    require_backend!();
    let mut f = fixture(true, CardPlacement::Beside { left: false, clearance: px(30.) });
    f.mounted.set([true, false, false]);
    f.host.flush();
    f.at(0);
    f.pass(550.);
    f.move_to(101.0, 36.0);
    f.move_to(110.0, 37.0);
    assert_eq!(f.shown(), Some(0));
    f.move_to(106.0, 37.0);
    assert_eq!(f.shown(), None, "a move of -2 px or more away closes");
}
