//! CMP-001 A10: every control operated with the keyboard only, with the accessibility output.
//!
//! A button, an icon-only button (accessible label plus tooltip text), a switch, a slider, a
//! disabled button and a busy button. The script reaches each control with Tab, operates it with
//! Enter, Space and the arrow keys, and prints the accessibility tree the way assistive technology
//! sees it: role, label, checked state, numeric value, minimum, maximum, step, value text and the
//! disabled and busy flags. The disabled button is skipped by Tab; the busy one is reachable but
//! starts nothing.
//!
//! The icon-only button's tooltip opens with Tab, with no delay.
//!
//! Without arguments the script runs offscreen on the production test host and the process exits 0
//! when every check holds (1 otherwise). With `--interactive` the same controls open in a window.

mod controls_common;

use std::{cell::Cell, rc::Rc};

use controls_common::{Build, Checks, accessibility, find, run_window, scripted_host};
use vespertine::{IntoElement, ParentElement, Styled, div, px, rgb};
use vespertine_components::controls::{Button, Slider, SliderRange, Switch};

#[derive(Default)]
struct State {
    saves: Cell<u32>,
    searches: Cell<u32>,
    busy_clicks: Cell<u32>,
    notifications: Cell<bool>,
    volume: Cell<f64>,
}

fn build(state: &Rc<State>) -> Build {
    let state = state.clone();
    Rc::new(move |_, _| {
        let (save, search, busy, notify, volume) = (
            state.clone(),
            state.clone(),
            state.clone(),
            state.clone(),
            state.clone(),
        );
        div()
            .p(px(24.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(14.))
            .child(
                Button::new("save")
                    .label("Save")
                    .w(px(140.))
                    .on_click(move |_, window, _| {
                        save.saves.set(save.saves.get() + 1);
                        window.refresh();
                    }),
            )
            .child(
                Button::icon_only(
                    "search",
                    "Search",
                    div().size(px(14.)).rounded_full().bg(rgb(0xF0F1F3)),
                )
                .tooltip_text("Find in files")
                .on_click(move |_, window, _| {
                    search.searches.set(search.searches.get() + 1);
                    window.refresh();
                }),
            )
            .child(
                Button::new("export")
                    .label("Export")
                    .w(px(140.))
                    .disabled(true),
            )
            .child(
                Button::new("sync")
                    .label("Sync")
                    .w(px(140.))
                    .busy(true)
                    .on_click(move |_, window, _| {
                        busy.busy_clicks.set(busy.busy_clicks.get() + 1);
                        window.refresh();
                    }),
            )
            .child(
                Switch::new("notifications")
                    .label("Notifications")
                    .checked(state.notifications.get())
                    .on_change(move |next, window, _| {
                        notify.notifications.set(next);
                        window.refresh();
                    }),
            )
            .child(
                Slider::new(
                    "volume",
                    SliderRange::new(0.0, 100.0)
                        .with_step(5.0)
                        .with_default(40.0),
                )
                .accessible_label("Volume")
                .value(state.volume.get())
                .value_text(|value| format!("{value:.0} %"))
                .on_change(move |value, window, _| {
                    volume.volume.set(value);
                    window.refresh();
                }),
            )
            .into_any_element()
    })
}

fn main() {
    let state = Rc::new(State::default());
    state.volume.set(40.0);
    if std::env::args().any(|argument| argument == "--interactive") {
        run_window(360.0, 420.0, build(&state));
        return;
    }

    let mut host = match scripted_host(360.0, 420.0, build(&state)) {
        Ok(host) => host,
        Err(reason) => {
            println!(
                "controls_keyboard: no offscreen host here ({reason}); run with --interactive"
            );
            return;
        }
    };
    let mut checks = Checks::default();
    let press = |host: &mut vespertine_test_host::TestHost, key: &str| {
        host.key_down(key, false, None).unwrap();
        host.key_up(key, None).unwrap();
    };

    println!("-- accessibility tree at rest");
    for node in accessibility(&mut host) {
        println!("   {}", node.describe());
    }

    // Tab order: Save, Search (the disabled Export is skipped), Sync (busy but focusable),
    // Notifications, Volume.
    host.simulate_keystrokes("tab", None).unwrap();
    press(&mut host, "enter");
    checks.check(state.saves.get() == 1, "Tab, Enter: the button activates");
    host.simulate_keystrokes("tab", None).unwrap();
    // Focus listeners run when the effects flush; the state they change draws in the next frame.
    host.flush();
    host.app().update(|_| ());
    host.flush();
    checks.check(
        host.painted_text()
            .iter()
            .any(|text| text == "Find in files"),
        "Tab: the icon-only button's CMP-004 tooltip opens at once and takes no focus",
    );
    press(&mut host, "space");
    checks.check(
        state.searches.get() == 1,
        "Tab, Space: the icon-only button activates",
    );
    host.simulate_keystrokes("tab", None).unwrap();
    press(&mut host, "enter");
    press(&mut host, "space");
    checks.check(
        state.busy_clicks.get() == 0,
        "Tab skipped the disabled button and reached the busy one, which starts nothing",
    );
    host.simulate_keystrokes("tab", None).unwrap();
    press(&mut host, "space");
    checks.check(state.notifications.get(), "Tab, Space: the switch turns on");
    press(&mut host, "enter");
    checks.check(
        !state.notifications.get(),
        "Enter: the switch turns off again",
    );
    press(&mut host, "space");
    checks.check(state.notifications.get(), "Space: on once more");
    host.simulate_keystrokes("tab", None).unwrap();
    for _ in 0..3 {
        press(&mut host, "right");
    }
    checks.check(
        state.volume.get() == 55.0,
        "Tab, Right x3: the slider moves by its step",
    );
    press(&mut host, "home");
    checks.check(
        state.volume.get() == 0.0,
        "Home: the slider goes to its minimum",
    );
    press(&mut host, "end");
    checks.check(
        state.volume.get() == 100.0,
        "End: the slider goes to its maximum",
    );
    press(&mut host, "left");
    checks.check(state.volume.get() == 95.0, "Left: one step down");

    println!("-- accessibility tree after keyboard operation");
    let nodes = accessibility(&mut host);
    for node in &nodes {
        println!("   {}", node.describe());
    }
    let save = find(&nodes, "button", "Save");
    checks.check(!save.disabled() && !save.busy(), "Save: enabled and idle");
    let search = find(&nodes, "button", "Search");
    checks.check(
        search.label() == "Search",
        "the icon-only button exposes its accessible label",
    );
    let export = find(&nodes, "button", "Export");
    checks.check(
        export.disabled(),
        "Export exposes its disabled state, role and label",
    );
    let sync = find(&nodes, "button", "Sync");
    checks.check(
        sync.busy() && !sync.disabled(),
        "Sync exposes its busy state",
    );
    let switch = find(&nodes, "switch", "Notifications");
    checks.check(
        switch.text("toggled") == Some("true"),
        "the switch exposes role switch and its checked state",
    );
    let slider = find(&nodes, "slider", "Volume");
    checks.check(
        slider.number("numericValue") == Some(95.0)
            && slider.number("minNumericValue") == Some(0.0)
            && slider.number("maxNumericValue") == Some(100.0)
            && slider.number("numericValueStep") == Some(5.0)
            && slider.text("value") == Some("95 %"),
        "the slider exposes value, minimum, maximum, step and value text",
    );
    checks.finish("controls_keyboard");
}
