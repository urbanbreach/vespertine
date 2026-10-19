//! CMP-004 A10 (and the focus-opened half of A1): native text tooltips on the base show/hide
//! mechanism.
//!
//! Four triggers: "Save" has a plain tooltip (default 500 ms show delay, hides the moment the
//! pointer leaves); "Details" has a hoverable tooltip (stays while the pointer is on the trigger or
//! on the tooltip, hides 500 ms after leaving); "Pin" is a small control whose tooltip opens above
//! the pointer with a 20 px clearance; "Focus me" is a keyboard-focusable trigger whose tooltip opens
//! at once when Tab focuses it (and not for a mouse press). A press or a wheel scroll away from an
//! open tooltip hides it.
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
use vespertine_components::tooltip::text::{TextTooltipExt, TooltipSpec};
use vespertine_components::tooltip::trigger::TooltipTrigger;
use vespertine_test_host::TestHost;

const SAVE: (f32, f32) = (40., 40.);
const DETAILS: (f32, f32) = (40., 110.);
const PIN: (f32, f32) = (40., 180.);
const FOCUS_ME: (f32, f32) = (40., 240.);

fn chip(
    id: &'static str,
    label: &'static str,
    at: (f32, f32),
    w: f32,
    h: f32,
) -> vespertine::Stateful<vespertine::Div> {
    div()
        .id(id)
        .absolute()
        .left(px(at.0))
        .top(px(at.1))
        .w(px(w))
        .h(px(h))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(rgb(0x2B3038))
        .child(label)
}

fn build(trigger: &Rc<RefCell<Option<Entity<TooltipTrigger>>>>) -> Build {
    let slot = trigger.clone();
    Rc::new(move |window, cx| {
        let focus_trigger = slot
            .borrow_mut()
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let button = cx.focus_handle().tab_stop(true);
                    TooltipTrigger::new(
                        "focus-me",
                        "Opens when Tab focuses it",
                        move |_, _| {
                            div()
                                .id("focus-me-button")
                                .track_focus(&button)
                                .w(px(120.))
                                .h(px(32.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .bg(rgb(0x2B3038))
                                .child("Focus me")
                                .into_any_element()
                        },
                        window,
                        cx,
                    )
                })
            })
            .clone();
        div()
            .size_full()
            .relative()
            .child(
                chip("save", "Save", SAVE, 120., 32.)
                    .tooltip_spec(TooltipSpec::new("Save the document")),
            )
            .child(
                chip("details", "Details", DETAILS, 120., 32.)
                    .tooltip_spec(TooltipSpec::new("More detail about this item").hoverable()),
            )
            .child(
                chip("pin", "Pin", PIN, 28., 28.)
                    .tooltip_spec(TooltipSpec::new("Pin to the sidebar").above_pointer()),
            )
            .child(
                div()
                    .absolute()
                    .left(px(FOCUS_ME.0))
                    .top(px(FOCUS_ME.1))
                    .child(focus_trigger),
            )
            .into_any_element()
    })
}

fn shown(host: &TestHost, text: &str) -> bool {
    host.painted_text().iter().any(|painted| painted == text)
}

fn pass(host: &mut TestHost, ms: f64) {
    host.advance_executor_time(ms).unwrap();
    host.flush();
}

fn hover(host: &mut TestHost, x: f32, y: f32) {
    host.mouse_move(f64::from(x), f64::from(y), None, "")
        .unwrap();
    host.flush();
}

/// Focus listeners run when the window's effects flush, after the frame that moved focus; the state
/// they change draws in the next frame.
fn settle(host: &mut TestHost) {
    host.flush();
    host.app().update(|_| ());
    host.flush();
}

fn main() {
    let trigger = Rc::new(RefCell::new(None));
    if std::env::args().any(|argument| argument == "--interactive") {
        run_window(360.0, 320.0, build(&trigger));
        return;
    }
    let mut host = match scripted_host(360.0, 320.0, build(&trigger)) {
        Ok(host) => host,
        Err(reason) => {
            println!("native_tooltips: no offscreen host here ({reason}); run with --interactive");
            return;
        }
    };
    let mut checks = Checks::default();
    host.flush();

    // A non-hoverable tooltip: default 500 ms, hides as soon as the pointer leaves.
    let save = (SAVE.0 + 60., SAVE.1 + 16.);
    hover(&mut host, save.0, save.1);
    pass(&mut host, 499.);
    checks.check(
        !shown(&host, "Save the document"),
        "Save: not shown at 499 ms",
    );
    pass(&mut host, 1.);
    checks.check(shown(&host, "Save the document"), "Save: shown at 500 ms");
    hover(&mut host, 300., 300.);
    checks.check(
        !shown(&host, "Save the document"),
        "Save: a non-hoverable tooltip hides the moment the pointer leaves",
    );

    // A press away hides an open tooltip.
    hover(&mut host, save.0, save.1);
    pass(&mut host, 500.);
    checks.check(shown(&host, "Save the document"), "Save: shown again");
    host.click(300., 300., 0, "").unwrap();
    host.flush();
    checks.check(
        !shown(&host, "Save the document"),
        "Save: a press away hides it",
    );

    // A wheel scroll that is not over the tooltip hides it.
    hover(&mut host, 300., 300.);
    hover(&mut host, save.0, save.1);
    pass(&mut host, 500.);
    checks.check(shown(&host, "Save the document"), "Save: shown once more");
    host.scroll_wheel(f64::from(save.0), f64::from(save.1), 0.0, -20.0, "")
        .unwrap();
    host.flush();
    checks.check(
        !shown(&host, "Save the document"),
        "Save: a wheel scroll away from the tooltip hides it",
    );

    // A hoverable tooltip stays while the pointer is on it and hides 500 ms after leaving.
    let (dx, dy) = (DETAILS.0 + 115., DETAILS.1 + 10.);
    hover(&mut host, 300., 300.);
    hover(&mut host, dx, dy);
    pass(&mut host, 500.);
    checks.check(
        shown(&host, "More detail about this item"),
        "Details: shown at 500 ms",
    );
    // The tooltip opens just below and right of the pointer: move onto it, off the trigger.
    hover(&mut host, dx + 14., dy + 12.);
    pass(&mut host, 2000.);
    checks.check(
        shown(&host, "More detail about this item"),
        "Details: stays while the pointer is on the tooltip",
    );
    hover(&mut host, 300., 300.);
    pass(&mut host, 499.);
    checks.check(
        shown(&host, "More detail about this item"),
        "Details: still shown 499 ms after leaving",
    );
    pass(&mut host, 1.);
    checks.check(
        !shown(&host, "More detail about this item"),
        "Details: hidden 500 ms after leaving",
    );

    // The above-pointer placement for a small control.
    hover(&mut host, PIN.0 + 14., PIN.1 + 14.);
    pass(&mut host, 500.);
    checks.check(
        shown(&host, "Pin to the sidebar"),
        "Pin: the small control's tooltip shows above the pointer",
    );
    hover(&mut host, 300., 300.);

    // A keyboard-focusable trigger opens at once on Tab; a mouse press does not leave it open.
    let focus_trigger = trigger.borrow().clone().expect("the focus trigger");
    let is_open = |host: &mut TestHost| {
        let focus_trigger = focus_trigger.clone();
        host.app().update(|cx| focus_trigger.read(cx).is_open())
    };
    host.simulate_keystrokes("tab", None).unwrap();
    settle(&mut host);
    checks.check(
        is_open(&mut host) && shown(&host, "Opens when Tab focuses it"),
        "Focus me: Tab opens the tooltip with no delay",
    );
    host.simulate_keystrokes("escape", None).unwrap();
    settle(&mut host);
    checks.check(!is_open(&mut host), "Focus me: Escape closes it");
    host.blur().unwrap();
    settle(&mut host);
    host.click(FOCUS_ME.0 as f64 + 60., FOCUS_ME.1 as f64 + 16., 0, "")
        .unwrap();
    settle(&mut host);
    checks.check(
        !is_open(&mut host),
        "Focus me: a mouse press that focuses it does not leave the tooltip open",
    );
    checks.finish("native_tooltips");
}
