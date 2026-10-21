//! CMP-004 A4, A5 (native): hover cards through the production offscreen test host.

use std::{cell::Cell, cell::RefCell, rc::Rc};
use vespertine::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, div, px,
};
use vespertine_components::floating::{EscapeOutcome, register_layer};
use vespertine_components::tooltip::card::{CardPlacement, HoverCardHost};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

/// Triggers: 0 at (20, 20), 1 at (140, 20), 2 at (20, 120); each 80 x 30.
const TRIGGERS: [(f32, f32); 3] = [(20., 20.), (140., 20.), (20., 120.)];

struct Root {
    host: Entity<HoverCardHost<usize>>,
    mounted: Rc<Cell<[bool; 3]>>,
    shift: Rc<Cell<f32>>,
    reconcile_in_render: Rc<Cell<bool>>,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mounted = self.mounted.get();
        if self.reconcile_in_render.get() {
            // R13: the container reconciles at every render.
            self.host.update(cx, |cards, cx| {
                cards.reconcile(|key| mounted[*key], false, false, cx)
            });
        }
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
    reconcile_in_render: Rc<Cell<bool>>,
}

fn fixture(interactive: bool, placement: CardPlacement) -> Fixture {
    let out: Rc<RefCell<Option<Entity<HoverCardHost<usize>>>>> = Rc::new(RefCell::new(None));
    let sink = out.clone();
    let root_out: Rc<RefCell<Option<Entity<Root>>>> = Rc::new(RefCell::new(None));
    let root_sink = root_out.clone();
    let mounted = Rc::new(Cell::new([true; 3]));
    let shift = Rc::new(Cell::new(0.));
    let reconcile_in_render = Rc::new(Cell::new(false));
    let (m, s, r) = (mounted.clone(), shift.clone(), reconcile_in_render.clone());
    let host = TestHost::with_view(HostOptions::with_size(400.0, 300.0), move |_, cx| {
        let cards = cx.new(|cx| {
            HoverCardHost::new(
                |key: &usize, _, _| {
                    div()
                        .w(px(100.))
                        .h(px(40.))
                        .child(format!("card {key}"))
                        .into_any_element()
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
            reconcile_in_render: r,
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
        reconcile_in_render,
    }
}

impl Fixture {
    fn shown(&mut self) -> Option<usize> {
        let cards = self.cards.clone();
        self.host.app().update(|cx| cards.read(cx).shown().copied())
    }

    fn target(&mut self) -> Option<usize> {
        let cards = self.cards.clone();
        self.host
            .app()
            .update(|cx| cards.read(cx).target().copied())
    }

    fn at(&mut self, ix: usize) {
        let (x, y) = TRIGGERS[ix];
        self.move_to(f64::from(x + 40.), f64::from(y + 15.));
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.host.mouse_move(x, y, None, "").unwrap();
        self.host.flush();
    }

    /// The container re-renders (its state changed). The fixture's `mounted` and `shift` cells
    /// are not observed by the window: a change to them is only drawn when the container is
    /// notified, exactly as with any state kept outside an entity.
    fn rerender(&mut self) {
        let root = self.root.clone();
        self.host
            .app()
            .update(|cx| root.update(cx, |_, cx| cx.notify()));
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
    assert_eq!(
        f.shown(),
        Some(1),
        "a neighbor shows at once while a card is shown"
    );
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
fn escape_hides_a_shown_card_before_the_layer_below_it() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    // A layer that is already open below the card (a menu or a dialog).
    let below = Rc::new(Cell::new(0));
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, {
            let below = below.clone();
            move |_, window, cx| {
                register_layer(
                    window,
                    cx,
                    Rc::new(move |_, _| {
                        below.set(below.get() + 1);
                        EscapeOutcome::Consumed
                    }),
                );
            }
        })
        .unwrap();
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    f.host.key_down("escape", false, None).unwrap();
    f.host.flush();
    assert_eq!(f.shown(), None, "Escape hides the card");
    assert_eq!(below.get(), 0, "the layer below did not see it");
    f.host.key_down("escape", false, None).unwrap();
    f.host.flush();
    assert_eq!(below.get(), 1, "the next Escape reaches the layer below");
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
        cards.update(cx, |cards, cx| {
            cards.reconcile(|key| *key != 0, false, false, cx)
        })
    });
    f.host.flush();
    assert_eq!(
        f.shown(),
        None,
        "the card of an unmounted trigger is dismissed"
    );
}

/// The sequence the predecessor had to split: unmount the pointed-at trigger, reconcile from
/// outside any window update, mount it again, then leave and re-enter it and move it away under a
/// stationary pointer. A trigger that is not mounted cannot hear the pointer leave (that is why
/// R13 reconciles from the container), and a container whose own state lives outside an entity
/// must be notified to draw it again; with both done every leave and enter is delivered.
#[test]
fn a_reconcile_outside_a_window_update_leaves_every_later_leave_and_enter_working() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    // The container drops the trigger; the reconcile's own notification draws the window
    // without it.
    f.mounted.set([false, true, true]);
    let cards = f.cards.clone();
    f.host.app().update(|cx| {
        cards.update(cx, |cards, cx| {
            cards.reconcile(|key| *key != 0, false, false, cx)
        })
    });
    f.host.flush();
    assert_eq!(
        f.shown(),
        None,
        "the card of an unmounted trigger is dismissed"
    );
    assert!(!f.host.painted_text().iter().any(|t| t == "trigger 0"));
    // Mounting it again under the stationary pointer is drawn once the container is notified,
    // and the paint-time hit test starts a fresh dwell: the cold dismissal cleared hidden-at.
    f.mounted.set([true; 3]);
    f.rerender();
    assert!(f.host.painted_text().iter().any(|t| t == "trigger 0"));
    assert_eq!(f.target(), Some(0));
    assert_eq!(f.shown(), None);
    // The pointer leaves: the pending dwell is cancelled and never settles.
    f.move_to(300., 250.);
    assert_eq!(f.target(), None, "the leave was delivered");
    f.pass(600.);
    assert_eq!(f.shown(), None);
    // Entering again waits the dwell, shows, and leaving hides the card.
    f.at(0);
    assert_eq!(f.target(), Some(0));
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    f.move_to(300., 250.);
    assert_eq!(f.shown(), None, "leaving hides the shown card");
    // A pending dwell whose trigger moves away under a stationary pointer never settles.
    f.pass(500.);
    f.at(0);
    f.pass(300.);
    f.shift.set(200.);
    f.rerender();
    f.pass(300.);
    assert_eq!(f.shown(), None);
    // The moved trigger is hovered at its new place (the warm window has passed, so it dwells).
    f.pass(500.);
    f.move_to(260., 35.);
    assert_eq!(f.target(), Some(0));
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
}

#[test]
fn a_container_reconciling_in_its_render_dismisses_the_card_and_keeps_hover_events_flowing() {
    require_backend!();
    let mut f = fixture(false, CardPlacement::Below);
    f.reconcile_in_render.set(true);
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    f.mounted.set([false, true, true]);
    f.rerender();
    f.host.flush();
    assert_eq!(
        f.shown(),
        None,
        "the render-time reconcile dismissed the card"
    );
    f.mounted.set([true; 3]);
    f.rerender();
    f.move_to(300., 250.);
    f.at(0);
    assert_eq!(
        f.target(),
        Some(0),
        "the leave and the enter were both delivered"
    );
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    f.move_to(300., 250.);
    assert_eq!(f.shown(), None);
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
    assert_eq!(
        f.shown(),
        None,
        "the moved trigger cancelled the dwell before it settled"
    );
}

#[test]
fn an_interactive_card_survives_a_diagonal_travel_and_closes_after_the_grace() {
    require_backend!();
    let mut f = fixture(
        true,
        CardPlacement::Beside {
            left: false,
            clearance: px(30.),
        },
    );
    f.mounted.set([true, false, false]);
    f.host.flush();
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    let card = {
        let cards = f.cards.clone();
        f.host
            .app()
            .update(|cx| cards.read(cx).card_bounds())
            .expect("card painted")
    };
    assert!(
        f32::from(card.left()) >= 129.0,
        "the card sits 30 px beside the trigger"
    );
    // Leave the trigger heading for the card and keep moving forward in steps of 3 px.
    f.move_to(101.0, 36.0);
    for step in 0..8 {
        f.pass(50.);
        f.move_to(104.0 + 3.0 * f64::from(step), 37.0);
    }
    assert_eq!(f.shown(), Some(0), "forward moves keep the card open");
    // Entering the card keeps it open however long the pointer rests there.
    let center = card.center();
    f.move_to(
        f64::from(f32::from(center.x)),
        f64::from(f32::from(center.y)),
    );
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
    let mut f = fixture(
        true,
        CardPlacement::Beside {
            left: false,
            clearance: px(30.),
        },
    );
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

/// The pointer can jump from the trigger into the card in one move (a fast flick, or a card that
/// touches the trigger): the card's hover is delivered before the trigger's leave, so the leave
/// must not start a grace that nothing cancels.
#[test]
fn an_interactive_card_entered_in_one_move_stays_open() {
    require_backend!();
    let mut f = fixture(
        true,
        CardPlacement::Beside {
            left: false,
            clearance: px(30.),
        },
    );
    f.mounted.set([true, false, false]);
    f.rerender();
    f.at(0);
    f.pass(550.);
    assert_eq!(f.shown(), Some(0));
    let card = {
        let cards = f.cards.clone();
        f.host
            .app()
            .update(|cx| cards.read(cx).card_bounds())
            .expect("card painted")
    };
    let center = card.center();
    f.move_to(
        f64::from(f32::from(center.x)),
        f64::from(f32::from(center.y)),
    );
    f.pass(500.);
    assert_eq!(f.shown(), Some(0), "the card keeps the pointer");
    f.move_to(300.0, 250.0);
    f.pass(80.);
    assert_eq!(
        f.shown(),
        None,
        "leaving the card closes it after the grace"
    );
}
