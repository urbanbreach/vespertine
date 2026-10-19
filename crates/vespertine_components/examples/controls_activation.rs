//! CMP-001 A1: one activation per physical press.
//!
//! A native button counts its activations. The script presses Enter once and delivers a
//! synthesized second key-down that is not flagged as held (the X11 repeat case), a flagged repeat
//! and the key release: exactly one activation results. Space activates only on release. A
//! disabled button (focusable, so the keys really reach it) never activates by key or click.
//!
//! Without arguments the script runs offscreen on the production test host and the process exits 0
//! when every check holds (1 otherwise). With `--interactive` the same controls open in a window:
//! Tab to a button and press Enter or Space.

mod controls_common;

use std::{cell::Cell, rc::Rc};

use controls_common::{Build, Checks, run_window, scripted_host};
use vespertine::{IntoElement, ParentElement, Styled, div, px};
use vespertine_components::controls::Button;

#[derive(Default)]
struct Counts {
    activate: Cell<u32>,
    disabled: Cell<u32>,
}

fn build(counts: &Rc<Counts>) -> Build {
    let counts = counts.clone();
    Rc::new(move |_, _| {
        let (on, off) = (counts.clone(), counts.clone());
        div()
            .p(px(24.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .child(
                Button::new("activate")
                    .label("Activate")
                    .w(px(160.))
                    .on_click(move |_, window, _| {
                        on.activate.set(on.activate.get() + 1);
                        window.refresh();
                    }),
            )
            .child(
                Button::new("disabled")
                    .label("Disabled")
                    .w(px(160.))
                    .disabled(true)
                    .focusable_when_disabled(true)
                    .on_click(move |_, window, _| {
                        off.disabled.set(off.disabled.get() + 1);
                        window.refresh();
                    }),
            )
            .child(div().text_sm().child(format!(
                "activations: {}   disabled activations: {}",
                counts.activate.get(),
                counts.disabled.get()
            )))
            .into_any_element()
    })
}

fn main() {
    let counts = Rc::new(Counts::default());
    if std::env::args().any(|argument| argument == "--interactive") {
        run_window(320.0, 200.0, build(&counts));
        return;
    }

    let mut host = match scripted_host(320.0, 200.0, build(&counts)) {
        Ok(host) => host,
        Err(reason) => {
            println!(
                "controls_activation: no offscreen host here ({reason}); run with --interactive"
            );
            return;
        }
    };
    let mut checks = Checks::default();
    host.simulate_keystrokes("tab", None).unwrap(); // focus "Activate"
    let n = || counts.activate.get();

    host.key_down("enter", false, None).unwrap();
    checks.check(n() == 1, "Enter: the first key-down activates once");
    host.key_down("enter", false, None).unwrap();
    checks.check(
        n() == 1,
        "a synthesized second key-down (not flagged as held) is ignored",
    );
    host.key_down("enter", true, None).unwrap();
    checks.check(n() == 1, "a key-down flagged as held is ignored");
    host.key_up("enter", None).unwrap();
    checks.check(
        n() == 1,
        "the key release does not activate again: one activation per press",
    );
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    checks.check(n() == 2, "the next physical Enter press activates again");

    host.key_down("space", false, None).unwrap();
    checks.check(n() == 2, "Space: nothing on key-down");
    host.key_down("space", true, None).unwrap();
    checks.check(n() == 2, "Space: nothing on a repeat");
    host.key_up("space", None).unwrap();
    checks.check(n() == 3, "Space: activates on release");

    // The disabled button: click focuses it (pointer press), then both keys and a click.
    host.click(60.0, 84.0, 0, "").unwrap();
    for key in ["enter", "space"] {
        host.key_down(key, false, None).unwrap();
        host.key_up(key, None).unwrap();
    }
    host.click(60.0, 84.0, 0, "").unwrap();
    checks.check(
        counts.disabled.get() == 0,
        "a disabled button never activates (keys and clicks)",
    );
    checks.check(
        n() == 3,
        "the enabled button was not activated by those events",
    );
    checks.finish("controls_activation");
}
