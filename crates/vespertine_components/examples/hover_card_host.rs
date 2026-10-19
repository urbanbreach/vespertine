//! CMP-004 A4 and A5 (native): the hover-card host with dwell, warm window, an interactive card
//! reached through the pointer corridor, and cold dismissal.
//!
//! Three triggers (A, B, C) share one non-interactive host: the first card opens after the 550 ms
//! dwell, a neighbor opens at once while a card is shown or within 400 ms of the last hide (and
//! waits the dwell again 401 ms after), and a click or a wheel scroll dismisses cold so the card
//! does not chase the pointer straight back. Trigger D has an interactive, scrollable card beside
//! it: the pointer may travel the corridor toward the card (forward moves of at least 2 px keep it
//! open, a 2 px reversal closes it), resting on the card keeps it open and leaving the card closes
//! it after 80 ms.
//!
//! Without arguments the example runs offscreen on the production test host with real pointer
//! events and a fake clock, and exits 0 when every check holds (1 otherwise). With `--interactive`
//! the same page opens in a window.

mod controls_common;

use std::{cell::RefCell, rc::Rc};

use controls_common::{Build, Checks, run_window, scripted_host};
use vespertine::{
    AppContext as _, Entity, InteractiveElement, IntoElement, ParentElement, Styled, div, px, rgb,
};
use vespertine_components::tooltip::card::{CardPlacement, HoverCardHost};
use vespertine_test_host::TestHost;

/// Plain triggers A, B, C: left, top; each 120 x 30.
const PLAIN: [(f32, f32); 3] = [(40., 40.), (200., 40.), (40., 100.)];
/// The interactive trigger D.
const RICH: (f32, f32) = (40., 200.);
const NAMES: [&str; 3] = ["A", "B", "C"];

struct Hosts {
    plain: Entity<HoverCardHost<usize>>,
    rich: Entity<HoverCardHost<usize>>,
}

fn build(hosts: &Rc<RefCell<Option<Rc<Hosts>>>>) -> Build {
    let slot = hosts.clone();
    Rc::new(move |_, cx| {
        let hosts = slot
            .borrow_mut()
            .get_or_insert_with(|| {
                let plain = cx.new(|cx| {
                    HoverCardHost::new(
                        |key: &usize, _, _| {
                            div()
                                .w(px(120.))
                                .child(format!("card {}", NAMES[*key]))
                                .into_any_element()
                        },
                        cx,
                    )
                });
                let rich = cx.new(|cx| {
                    HoverCardHost::new(
                        |_: &usize, _, _| {
                            div()
                                .w(px(140.))
                                .flex()
                                .flex_col()
                                .children((1..=24).map(|row| format!("detail line {row}")))
                                .into_any_element()
                        },
                        cx,
                    )
                    .interactive(true)
                    .placement(CardPlacement::Beside {
                        left: false,
                        clearance: px(30.),
                    })
                    .max_height(px(120.))
                });
                Rc::new(Hosts { plain, rich })
            })
            .clone();
        let mut page = div().size_full().relative();
        for (ix, (x, y)) in PLAIN.iter().enumerate() {
            let trigger = div()
                .id(("trigger", ix))
                .absolute()
                .left(px(*x))
                .top(px(*y))
                .w(px(120.))
                .h(px(30.))
                .rounded(px(6.))
                .bg(rgb(0x2B3038))
                .child(format!("trigger {}", NAMES[ix]));
            page = page.child(HoverCardHost::trigger(&hosts.plain, ix, trigger, cx));
        }
        let rich = div()
            .id("trigger-d")
            .absolute()
            .left(px(RICH.0))
            .top(px(RICH.1))
            .w(px(120.))
            .h(px(30.))
            .rounded(px(6.))
            .bg(rgb(0x2B3038))
            .child("trigger D");
        page.child(HoverCardHost::trigger(&hosts.rich, 0, rich, cx))
            .child(hosts.plain.clone())
            .child(hosts.rich.clone())
            .into_any_element()
    })
}

fn pass(host: &mut TestHost, ms: f64) {
    host.advance_executor_time(ms).unwrap();
    host.flush();
}

fn move_to(host: &mut TestHost, x: f64, y: f64) {
    host.mouse_move(x, y, None, "").unwrap();
    host.flush();
}

fn at(host: &mut TestHost, ix: usize) {
    let (x, y) = PLAIN[ix];
    move_to(host, f64::from(x + 60.), f64::from(y + 15.));
}

fn main() {
    let hosts = Rc::new(RefCell::new(None));
    if std::env::args().any(|argument| argument == "--interactive") {
        run_window(400.0, 320.0, build(&hosts));
        return;
    }
    let mut host = match scripted_host(400.0, 320.0, build(&hosts)) {
        Ok(host) => host,
        Err(reason) => {
            println!("hover_card_host: no offscreen host here ({reason}); run with --interactive");
            return;
        }
    };
    host.flush();
    let hosts = hosts.borrow().clone().expect("the card hosts");
    let mut checks = Checks::default();
    let plain_shown = |host: &mut TestHost| {
        let plain = hosts.plain.clone();
        host.app().update(|cx| plain.read(cx).shown().copied())
    };
    let rich_shown = |host: &mut TestHost| {
        let rich = hosts.rich.clone();
        host.app().update(|cx| rich.read(cx).shown().copied())
    };

    // Dwell: the first card needs the pointer to rest 550 ms; a neighbor shows at once.
    at(&mut host, 0);
    pass(&mut host, 549.);
    checks.check(plain_shown(&mut host).is_none(), "A: nothing at 549 ms");
    pass(&mut host, 1.);
    checks.check(
        plain_shown(&mut host) == Some(0),
        "A: the card opens at 550 ms",
    );
    at(&mut host, 1);
    checks.check(
        plain_shown(&mut host) == Some(1),
        "B: a neighbor opens at once while a card is shown",
    );

    // Warm window: 400 ms after the hide is still warm, 401 ms waits the dwell again.
    move_to(&mut host, 380., 300.);
    checks.check(plain_shown(&mut host).is_none(), "leaving hides the card");
    pass(&mut host, 400.);
    at(&mut host, 2);
    checks.check(
        plain_shown(&mut host) == Some(2),
        "C: 400 ms after the hide the card opens at once",
    );
    move_to(&mut host, 380., 300.);
    pass(&mut host, 401.);
    at(&mut host, 0);
    checks.check(
        plain_shown(&mut host).is_none(),
        "A: 401 ms after the hide the dwell applies again",
    );
    pass(&mut host, 550.);
    checks.check(
        plain_shown(&mut host) == Some(0),
        "A: opens after the dwell",
    );

    // Cold dismissal: a click hides, and a re-enter within 400 ms waits the dwell.
    host.click(100., 55., 0, "").unwrap();
    host.flush();
    checks.check(
        plain_shown(&mut host).is_none(),
        "a click dismisses the card",
    );
    move_to(&mut host, 380., 300.);
    pass(&mut host, 100.);
    at(&mut host, 0);
    checks.check(
        plain_shown(&mut host).is_none(),
        "the card does not chase the pointer straight back after a click",
    );
    pass(&mut host, 550.);
    checks.check(
        plain_shown(&mut host) == Some(0),
        "A: opens after the dwell",
    );
    host.scroll_wheel(100., 55., 0.0, -30.0, "").unwrap();
    host.flush();
    checks.check(
        plain_shown(&mut host).is_none(),
        "a wheel scroll dismisses the card",
    );
    move_to(&mut host, 380., 300.);

    // Interactive card: the corridor toward the card keeps it open.
    let (rx, ry) = RICH;
    move_to(&mut host, f64::from(rx + 60.), f64::from(ry + 15.));
    pass(&mut host, 550.);
    checks.check(
        rich_shown(&mut host) == Some(0),
        "D: the interactive card opens",
    );
    let card = {
        let rich = hosts.rich.clone();
        host.app().update(|cx| rich.read(cx).card_bounds())
    }
    .expect("the card is painted");
    checks.check(
        f32::from(card.left()) >= rx + 120. + 29.,
        "D: the card sits 30 px beside the trigger",
    );
    move_to(&mut host, f64::from(rx + 121.), f64::from(ry + 16.));
    for step in 0..8 {
        pass(&mut host, 50.);
        move_to(
            &mut host,
            f64::from(rx + 124.) + 3.0 * f64::from(step),
            f64::from(ry + 17.),
        );
    }
    checks.check(
        rich_shown(&mut host) == Some(0),
        "D: forward moves through the corridor keep the card open",
    );
    let center = card.center();
    move_to(
        &mut host,
        f64::from(f32::from(center.x)),
        f64::from(f32::from(center.y)),
    );
    pass(&mut host, 500.);
    checks.check(
        rich_shown(&mut host) == Some(0),
        "D: resting on the card keeps it open",
    );
    // A wheel over the card scrolls it instead of dismissing it.
    host.scroll_wheel(
        f64::from(f32::from(center.x)),
        f64::from(f32::from(center.y)),
        0.0,
        -30.0,
        "",
    )
    .unwrap();
    host.flush();
    checks.check(
        rich_shown(&mut host) == Some(0),
        "D: a wheel over the card scrolls it and keeps it open",
    );
    move_to(&mut host, 380., 300.);
    pass(&mut host, 79.);
    checks.check(
        rich_shown(&mut host) == Some(0),
        "D: still open 79 ms after leaving the card",
    );
    pass(&mut host, 1.);
    checks.check(
        rich_shown(&mut host).is_none(),
        "D: closed 80 ms after leaving the card",
    );

    // A reversal inside the corridor closes the card at once.
    pass(&mut host, 500.);
    move_to(&mut host, f64::from(rx + 60.), f64::from(ry + 15.));
    pass(&mut host, 550.);
    move_to(&mut host, f64::from(rx + 121.), f64::from(ry + 16.));
    move_to(&mut host, f64::from(rx + 130.), f64::from(ry + 17.));
    checks.check(
        rich_shown(&mut host) == Some(0),
        "D: moving toward the card keeps it",
    );
    move_to(&mut host, f64::from(rx + 126.), f64::from(ry + 17.));
    checks.check(
        rich_shown(&mut host).is_none(),
        "D: a move of 2 px or more away closes it at once",
    );
    checks.finish("hover_card_host");
}
