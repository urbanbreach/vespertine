//! A docked workspace with hidden and visible subscribers and streaming quotes (R4).

use super::quotes::{format_volume, price_at, symbol};
use super::{BORDER, GREEN, INK, RED, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use vespertine::{
    AnyView, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div, px,
};

const SYMBOLS: usize = 24;
const UPDATES_PER_FRAME: usize = 6;
const HIDDEN_PANELS: usize = 6;
const SYMBOLS_PER_HIDDEN: usize = 4;
const BOTTOM_DOCK_TOGGLE: u64 = 50;

struct SymbolQuote {
    row: usize,
    version: u64,
    previous: f64,
}

impl SymbolQuote {
    fn price(&self) -> f64 {
        price_at(self.row, self.version)
    }

    fn change(&self) -> f64 {
        (self.price() - self.previous) / self.previous * 100.0
    }
}

/// A visible table row of the center region.
struct WatchRow {
    quote: Entity<SymbolQuote>,
}

impl Render for WatchRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let quote = self.quote.read(cx);
        let change = quote.change();
        div()
            .h(px(22.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(div().w(px(70.0)).child(symbol(quote.row)))
            .child(div().w(px(70.0)).child(format!("{:.2}", quote.price())))
            .child(
                div()
                    .w(px(70.0))
                    .text_color(hex(if change >= 0.0 { GREEN } else { RED }))
                    .child(format!("{change:+.2}%")),
            )
            .child(div().child(format_volume(
                1000 * (quote.row as u64 + 1) + 17 * quote.version,
            )))
    }
}

/// A compact visible subscriber in the right dock.
struct TickerRow {
    quote: Entity<SymbolQuote>,
}

impl Render for TickerRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let quote = self.quote.read(cx);
        div()
            .h(px(20.0))
            .flex()
            .justify_between()
            .child(symbol(quote.row))
            .child(format!("{:.2}", quote.price()))
    }
}

/// A subscriber whose panel is hidden: it counts the updates of its symbols and notifies itself,
/// but no view of it is in the window.
struct HiddenPanel {
    updates: u64,
    _subscriptions: Vec<Subscription>,
}

struct Dock {
    title: &'static str,
    rows: Vec<AnyView>,
    lines: Vec<String>,
    width: Option<f32>,
}

impl Render for Dock {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dock = div()
            .flex()
            .flex_col()
            .bg(hex(SURFACE))
            .border_1()
            .border_color(hex(BORDER))
            .p(px(6.0))
            .child(div().font_weight(FontWeight::BOLD).child(self.title))
            .children(self.lines.iter().cloned())
            .children(self.rows.iter().cloned());
        match self.width {
            Some(width) => dock.w(px(width)).h_full(),
            None => dock.w_full().h(px(110.0)),
        }
    }
}

struct Workspace {
    left: Entity<Dock>,
    center: Vec<Entity<WatchRow>>,
    right: Entity<Dock>,
    bottom: Entity<Dock>,
    bottom_visible: bool,
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex_col()
            .flex()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(13.0))
            .text_color(hex(INK))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .child(self.left.clone())
                    .child(
                        div()
                            .flex_col()
                            .flex()
                            .flex_1()
                            .children(self.center.iter().cloned()),
                    )
                    .child(self.right.clone()),
            )
            .children(self.bottom_visible.then(|| self.bottom.clone()))
    }
}

struct WorkspaceDriver {
    workspace: Entity<Workspace>,
    /// Hidden subscribers stay alive for the whole run.
    _hidden: Vec<Entity<HiddenPanel>>,
    quotes: Vec<Entity<SymbolQuote>>,
    bottom_visible: bool,
}

impl Driver for WorkspaceDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let toggle = frame % BOTTOM_DOCK_TOGGLE == BOTTOM_DOCK_TOGGLE - 1;
        if toggle {
            self.bottom_visible = !self.bottom_visible;
        }
        let (quotes, workspace, visible) = (&self.quotes, &self.workspace, self.bottom_visible);
        stage.mutate(|app| {
            for slot in 0..UPDATES_PER_FRAME {
                let row = (frame as usize * 5 + slot * 4) % SYMBOLS;
                quotes[row].update(app, |quote, cx| {
                    quote.previous = quote.price();
                    quote.version = frame + 1;
                    cx.notify();
                });
            }
            if toggle {
                workspace.update(app, |workspace, cx| {
                    workspace.bottom_visible = visible;
                    cx.notify();
                });
            }
        });
    }
}

fn setup(harness: &mut dyn Host) -> Result<Box<dyn Driver>> {
    let (mut quotes, mut workspace, mut hidden_panels) = (Vec::new(), None, Vec::new());
    harness.open_window(|_, cx| {
        quotes = (0..SYMBOLS)
            .map(|row| {
                cx.new(|_| SymbolQuote {
                    row,
                    version: 0,
                    previous: price_at(row, 0),
                })
            })
            .collect();
        // Hidden subscribers: observed through the model layer only.
        hidden_panels = (0..HIDDEN_PANELS)
            .map(|panel| {
                let watched: Vec<Entity<SymbolQuote>> = (0..SYMBOLS_PER_HIDDEN)
                    .map(|slot| quotes[(panel * SYMBOLS_PER_HIDDEN + slot) % SYMBOLS].clone())
                    .collect();
                cx.new(|cx| HiddenPanel {
                    updates: 0,
                    _subscriptions: watched
                        .iter()
                        .map(|quote| {
                            cx.observe(quote, |panel: &mut HiddenPanel, _, cx| {
                                panel.updates += 1;
                                cx.notify();
                            })
                        })
                        .collect(),
                })
            })
            .collect();
        let center: Vec<Entity<WatchRow>> = quotes
            .iter()
            .map(|quote| {
                cx.new(|_| WatchRow {
                    quote: quote.clone(),
                })
            })
            .collect();
        let ticker_rows: Vec<AnyView> = quotes[..8]
            .iter()
            .map(|quote| {
                AnyView::from(cx.new(|_| TickerRow {
                    quote: quote.clone(),
                }))
            })
            .collect();
        let left = cx.new(|_| Dock {
            title: "Explorer",
            rows: Vec::new(),
            lines: (0..14)
                .map(|file| format!("src/module_{file}.rs"))
                .collect(),
            width: Some(150.0),
        });
        let right = cx.new(|_| Dock {
            title: "Ticker",
            rows: ticker_rows,
            lines: Vec::new(),
            width: Some(170.0),
        });
        let bottom = cx.new(|_| Dock {
            title: "Output",
            rows: Vec::new(),
            lines: (0..3)
                .map(|line| format!("build step {line} finished"))
                .collect(),
            width: None,
        });
        let entity = cx.new(|_| Workspace {
            left,
            center,
            right,
            bottom,
            bottom_visible: true,
        });
        workspace = Some(entity.clone());
        AnyView::from(entity)
    })?;
    Ok(Box::new(WorkspaceDriver {
        workspace: workspace.expect("the window built the workspace"),
        _hidden: hidden_panels,
        quotes,
        bottom_visible: true,
    }))
}

/// The docked-workspace workload.
pub fn workloads() -> Vec<Workload> {
    vec![Workload {
        name: "workspace-docked",
        family: Family::Workspace,
        description: "six streaming quote updates per frame reach visible rows and hidden subscribers; the bottom dock toggles every 50 frames",
        warmup: 30,
        frames: 200,
        setup,
    }]
}
