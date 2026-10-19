//! DEV-003 A3: panel-local updates.
//!
//! A dashboard of eight panels. Updating one panel's model rebuilds that panel only: the other
//! seven are replayed from the previous frame, so the frame does a fraction of the work. The
//! example draws the same dashboard with retained frames on and off, prints the per-frame
//! statistics of both (views built and reused, layout nodes reused, lines shaped) and exits
//! non-zero unless retention rebuilt exactly the one updated panel while the full render
//! rebuilt every panel.
//!
//! ```sh
//! cargo run -p vespertine_bench --example panel_updates
//! ```

use std::{cell::RefCell, process::ExitCode, rc::Rc};
use vespertine::{
    AnyView, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px, rgb,
};
use vespertine_bench::{
    Driver, Harness, Meter, RetentionMode, Stage,
    harness::{Host, real_text_system},
};

const PANELS: usize = 8;

struct Panel {
    index: usize,
    reading: u32,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(200.0))
            .h(px(120.0))
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .rounded(px(8.0))
            .bg(rgb(0xf6f8fa))
            .border_1()
            .border_color(rgb(0xd0d7de))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Panel {}", self.index + 1)),
            )
            .child(format!("reading {:05}", self.reading))
            .child("Only this panel's view is rebuilt when its model changes.")
    }
}

struct Dashboard {
    tiles: Vec<Entity<Panel>>,
}

impl Render for Dashboard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(12.0))
            .flex()
            .flex_wrap()
            .gap(px(12.0))
            .bg(rgb(0xffffff))
            .text_color(rgb(0x1f2328))
            .text_size(px(13.0))
            .children(self.tiles.iter().cloned())
    }
}

struct Updates {
    panels: Vec<Entity<Panel>>,
}

impl Driver for Updates {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let panel = &self.panels[(frame as usize * 3) % PANELS];
        stage.mutate(|app| {
            panel.update(app, |panel, cx| {
                panel.reading += 17;
                cx.notify();
            });
        });
    }
}

fn setup(host: &mut dyn Host) -> anyhow::Result<Box<dyn Driver>> {
    let mut panels = Vec::new();
    host.open_window(|_, cx| {
        panels = (0..PANELS)
            .map(|index| cx.new(|_| Panel { index, reading: 0 }))
            .collect();
        AnyView::from(cx.new(|_| Dashboard {
            tiles: panels.clone(),
        }))
    })?;
    Ok(Box::new(Updates { panels }))
}

fn main() -> ExitCode {
    let meter = Rc::new(RefCell::new(Meter::new()));
    let mut ok = true;
    for mode in [RetentionMode::On, RetentionMode::Off] {
        let mut harness = Harness::new(real_text_system(), meter.clone(), false);
        let mut driver = setup(&mut harness).expect("build the dashboard");
        harness.set_retention(mode).expect("set retention");
        harness.settle().expect("first frame");
        for frame in 0..PANELS as u64 {
            harness.reset_frame_stats(false).expect("reset statistics");
            harness
                .run_frame(frame, &mut *driver)
                .expect("draw the frame");
            let stats = harness.frame_stats().expect("statistics");
            println!(
                "{:14} update of panel {}: views built {} reused {}, layout nodes reused {}, lines shaped {}",
                mode.label(),
                (frame as usize * 3) % PANELS + 1,
                stats.views_built,
                stats.views_reused,
                stats.nodes_reused,
                stats.lines_shaped
            );
            // Retained: exactly the updated panel is built. Full render: every panel, the
            // dashboard and the harness's root view are built.
            let expected = match mode {
                RetentionMode::On => 1,
                RetentionMode::Off => PANELS as u64 + 2,
            };
            if stats.views_built != expected {
                println!("FAIL: expected {expected} views built");
                ok = false;
            }
        }
    }
    if ok {
        println!("panel-local updates rebuild only the updated panel");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
