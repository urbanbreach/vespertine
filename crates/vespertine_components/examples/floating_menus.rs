//! CMP-002 A13: popup menus at all four corners of the viewport.
//!
//! Each corner holds a trigger; its menu opens below it (or, near the bottom edge, snaps inside
//! the 8 px window margin). Clicking another corner's trigger while a menu is open switches to
//! that menu in one click, and no click reaches the "background" button that fills the middle of
//! the window (its click counter stays at 0 unless you click it while no menu is open).
//! Escape closes the open menu; Up/Down/Home/End/Enter step and accept; resize the window to
//! watch the menus follow their corners.
//!
//! Without arguments the example runs interactively. With `FLOATING_MENUS_SCRIPT=1` it runs the
//! acceptance script in the offscreen test host (no window needed) and exits 0 when every check
//! passes, 1 otherwise.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use vespertine::{
    App, AppContext as _, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use vespertine_components::floating::{
    ContainedAlign, Placement, PopupMenu, PopupMenuItem, TRIGGER_CLEARANCE, TriggerBounds,
};
use vespertine_platform::application;

const CORNERS: [&str; 4] = ["Top left", "Top right", "Bottom left", "Bottom right"];

struct CornerMenus {
    menus: Vec<Entity<PopupMenu>>,
    background_clicks: Rc<Cell<u32>>,
    last_selection: Rc<Cell<Option<(usize, usize)>>>,
}

impl CornerMenus {
    fn new(cx: &mut Context<Self>) -> Self {
        let background_clicks = Rc::new(Cell::new(0));
        let last_selection = Rc::new(Cell::new(None));
        let triggers: Vec<TriggerBounds> = (0..4).map(|_| TriggerBounds::new()).collect();
        let menus = (0..4)
            .map(|ix| {
                let others = triggers
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != ix)
                    .map(|(_, tracker)| tracker.clone())
                    .collect();
                let selection = last_selection.clone();
                let tracker = triggers[ix].clone();
                let notify = cx.entity();
                cx.new(|cx| {
                    PopupMenu::new(
                        format!("corner-{ix}"),
                        CORNERS[ix],
                        vec![
                            PopupMenuItem::new("Open"),
                            PopupMenuItem::new("Rename").disabled(),
                            PopupMenuItem::new("Duplicate"),
                            PopupMenuItem::new("Delete"),
                        ],
                        cx,
                    )
                    .placement(Placement::Below {
                        align: if ix % 2 == 0 {
                            ContainedAlign::Start
                        } else {
                            ContainedAlign::End
                        },
                        gap: TRIGGER_CLEARANCE,
                    })
                    .trigger_tracker(tracker)
                    .exempt_triggers(others)
                    .on_select(move |row, _, cx| {
                        selection.set(Some((ix, row)));
                        notify.update(cx, |_, cx| cx.notify());
                    })
                })
            })
            .collect();
        Self {
            menus,
            background_clicks,
            last_selection,
        }
    }
}

impl Render for CornerMenus {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.background_clicks.clone();
        let status = match self.last_selection.get() {
            Some((menu, row)) => format!("last selection: {} / row {row}", CORNERS[menu]),
            None => "last selection: none".to_owned(),
        };
        let m = &self.menus;
        div()
            .size_full()
            .relative()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .child(
                div()
                    .id("background-button")
                    .absolute()
                    .top(px(64.))
                    .bottom(px(64.))
                    .left(px(16.))
                    .right(px(16.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .bg(rgb(0x262A31))
                    .on_click({
                        let clicks = self.background_clicks.clone();
                        move |_, _, _| clicks.set(clicks.get() + 1)
                    })
                    .child("Background button (no menu click ever reaches it)")
                    .child(format!("background clicks: {}", clicks.get()))
                    .child(status),
            )
            .child(
                div()
                    .absolute()
                    .top(px(12.))
                    .left(px(12.))
                    .child(m[0].clone()),
            )
            .child(
                div()
                    .absolute()
                    .top(px(12.))
                    .right(px(12.))
                    .child(m[1].clone()),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(12.))
                    .left(px(12.))
                    .child(m[2].clone()),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(12.))
                    .right(px(12.))
                    .child(m[3].clone()),
            )
    }
}

/// The acceptance script of A13 in the offscreen host.
fn run_script() -> bool {
    use vespertine_test_host::{HostOptions, TestHost, is_available, unavailable_reason};
    if !is_available() {
        eprintln!(
            "test host unavailable: {}",
            unavailable_reason().unwrap_or_default()
        );
        return false;
    }
    let slot = Rc::new(RefCell::new(None::<Entity<CornerMenus>>));
    let capture = slot.clone();
    let mut host = TestHost::with_view(HostOptions::with_size(640., 420.), move |_, cx| {
        let view = cx.new(CornerMenus::new);
        *capture.borrow_mut() = Some(view.clone());
        view
    })
    .expect("host");
    let view = slot.borrow().clone().expect("view");
    host.motion_pause();
    host.flush();
    let mut ok = true;
    let mut check = |what: &str, passed: bool| {
        println!("{} {what}", if passed { "PASS" } else { "FAIL" });
        ok &= passed;
    };
    // Trigger centres of the four corners, and a point of the background button that the open
    // menus cover.
    let triggers = [(102., 30.), (538., 30.), (102., 390.), (538., 390.)];
    let open_menus = |host: &mut TestHost| -> Vec<bool> {
        let view = view.clone();
        host.app().update(|cx| {
            view.read(cx)
                .menus
                .iter()
                .map(|menu| menu.read(cx).is_open())
                .collect()
        })
    };
    let clicks = |host: &mut TestHost| -> u32 {
        let view = view.clone();
        host.app()
            .update(|cx| view.read(cx).background_clicks.get())
    };
    for (ix, (x, y)) in triggers.iter().enumerate() {
        host.advance_executor_time(300.).unwrap();
        host.click(*x, *y, 0, "").unwrap();
        host.flush();
        host.motion_fast_forward(200.).unwrap();
        host.flush();
        let open = open_menus(&mut host);
        check(
            &format!("corner {ix} opens exactly its own menu"),
            open.iter()
                .enumerate()
                .all(|(other, open)| *open == (other == ix)),
        );
    }
    // Switching takes one click: top left, then bottom right.
    host.advance_executor_time(300.).unwrap();
    host.click(102., 30., 0, "").unwrap();
    host.flush();
    host.motion_fast_forward(200.).unwrap();
    // Let the previously closed menu finish its exit: pressing a trigger whose popup is still
    // in its exit phase leaves it closed (R13).
    host.advance_executor_time(300.).unwrap();
    host.click(538., 390., 0, "").unwrap();
    host.flush();
    host.motion_fast_forward(200.).unwrap();
    let open = open_menus(&mut host);
    check(
        "one click switches from the first menu to the last",
        open == [false, false, false, true],
    );
    // A click on the open menu's row lies over the background button: it must not click through.
    host.click(538., 330., 0, "").unwrap();
    host.flush();
    check(
        "no click reaches the background button",
        clicks(&mut host) == 0,
    );
    ok
}

fn main() {
    if std::env::var_os("FLOATING_MENUS_SCRIPT").is_some_and(|value| value == "1") {
        std::process::exit(if run_script() { 0 } else { 1 });
    }
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(720.), px(480.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(CornerMenus::new),
        )
        .expect("open the window");
        cx.activate(true);
    });
}
