//! Uniform-height and variable-height list scrolling and live updates (R4).

use super::{BORDER, INK, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use std::{cell::RefCell, rc::Rc};
use vespertine::{
    AnyView, AppContext as _, Context, Entity, IntoElement, ListAlignment, ListState,
    ParentElement as _, Render, SharedString, Styled as _, UniformListScrollHandle, Window, div,
    list, point, px, uniform_list,
};

const UNIFORM_ROWS: usize = 5_000;
const VARIABLE_ROWS: usize = 1_000;
const UNIFORM_HEIGHT: f32 = 24.0;
/// Rows whose content changes each frame in the live variants.
const LIVE_ROWS: usize = 3;
const SCROLL_STEP: f32 = 29.0;

struct Row {
    index: usize,
    revision: u64,
}

fn row_text(index: usize, revision: u64) -> SharedString {
    format!("Row {index}: lorem ipsum dolor sit amet, revision {revision}").into()
}

/// A variable-height body: the number of sentences depends on the row.
fn variable_body(index: usize, revision: u64) -> String {
    let sentences = 1 + (index * 7 + 3) % 5;
    (0..sentences)
        .map(|sentence| {
            format!(
                "Sentence {sentence} of row {index} revision {revision} keeps going for a while so that it wraps. "
            )
        })
        .collect()
}

/// Uniform rows are plain elements: a uniform list measures its first item on its own, and the
/// rows read their revision from the shared table the steps write.
fn uniform_row(index: usize, revision: u64) -> impl IntoElement {
    let base = div()
        .h(px(UNIFORM_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .px(px(12.0));
    (if index % 2 == 1 {
        base.bg(hex(SURFACE))
    } else {
        base
    })
    .whitespace_nowrap()
    .child(row_text(index, revision))
}

struct VariableRow(Row);

impl Render for VariableRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = &self.0;
        div()
            .w_full()
            .p(px(12.0))
            .border_b_1()
            .border_color(hex(BORDER))
            .child(
                div()
                    .text_size(px(13.0))
                    .child(format!("Row {}", row.index)),
            )
            .child(
                div()
                    .line_height(px(20.0))
                    .child(variable_body(row.index, row.revision)),
            )
    }
}

struct UniformFeed {
    revisions: Rc<RefCell<Vec<u64>>>,
    scroll: UniformListScrollHandle,
}

impl Render for UniformFeed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let revisions = self.revisions.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(
                uniform_list("uniform-feed", UNIFORM_ROWS, move |range, _, _| {
                    let revisions = revisions.borrow();
                    range
                        .map(|ix| uniform_row(ix, revisions[ix]))
                        .collect::<Vec<_>>()
                })
                .track_scroll(&self.scroll)
                .flex_1(),
            )
    }
}

struct VariableFeed {
    rows: Vec<Entity<VariableRow>>,
    state: ListState,
}

impl Render for VariableFeed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        div()
            .size_full()
            .flex_col()
            .flex()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(
                list(self.state.clone(), move |ix, _, _| {
                    rows[ix].clone().into_any_element()
                })
                .flex_1(),
            )
    }
}

/// Scrolls (and optionally edits rows) of a uniform list.
struct UniformDriver {
    feed: Entity<UniformFeed>,
    revisions: Rc<RefCell<Vec<u64>>>,
    scroll: UniformListScrollHandle,
    live: bool,
}

impl Driver for UniformDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let offset = SCROLL_STEP * (frame + 1) as f32;
        let live = self.live;
        let (feed, revisions, scroll) = (&self.feed, &self.revisions, &self.scroll);
        stage.mutate(|app| {
            scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.0), px(-offset)));
            if live {
                // The rows under the viewport at this offset.
                let first = (offset / UNIFORM_HEIGHT) as usize;
                for row in first..first + LIVE_ROWS {
                    revisions.borrow_mut()[row % UNIFORM_ROWS] = frame + 1;
                }
            }
            feed.update(app, |_, cx| cx.notify());
        });
    }
}

struct VariableDriver {
    feed: Entity<VariableFeed>,
    rows: Vec<Entity<VariableRow>>,
    state: ListState,
    live: bool,
}

impl Driver for VariableDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let live = self.live;
        let (feed, rows, state) = (&self.feed, &self.rows, &self.state);
        stage.mutate(|app| {
            state.scroll_by(px(SCROLL_STEP));
            if live {
                let visible = state.logical_scroll_top().item_ix;
                for row in visible..visible + LIVE_ROWS {
                    let row = row % VARIABLE_ROWS;
                    rows[row].update(app, |row, cx| {
                        row.0.revision = frame + 1;
                        cx.notify();
                    });
                    state.remeasure_items(row..row + 1);
                }
            }
            feed.update(app, |_, cx| cx.notify());
        });
    }
}

fn uniform_setup(live: bool) -> impl Fn(&mut dyn Host) -> Result<Box<dyn Driver>> {
    move |harness| {
        let scroll = UniformListScrollHandle::new();
        let revisions = Rc::new(RefCell::new(vec![0; UNIFORM_ROWS]));
        let mut feed = None;
        harness.open_window(|_, cx| {
            let entity = cx.new(|_| UniformFeed {
                revisions: revisions.clone(),
                scroll: scroll.clone(),
            });
            feed = Some(entity.clone());
            AnyView::from(entity)
        })?;
        Ok(Box::new(UniformDriver {
            feed: feed.expect("the window built the feed"),
            revisions,
            scroll,
            live,
        }))
    }
}

fn variable_setup(live: bool) -> impl Fn(&mut dyn Host) -> Result<Box<dyn Driver>> {
    move |harness| {
        let state = ListState::new(VARIABLE_ROWS, ListAlignment::Top, px(300.0));
        let (mut rows, mut feed) = (Vec::new(), None);
        harness.open_window(|_, cx| {
            rows = (0..VARIABLE_ROWS)
                .map(|index| cx.new(|_| VariableRow(Row { index, revision: 0 })))
                .collect();
            let entity = cx.new(|_| VariableFeed {
                rows: rows.clone(),
                state: state.clone(),
            });
            feed = Some(entity.clone());
            AnyView::from(entity)
        })?;
        Ok(Box::new(VariableDriver {
            feed: feed.expect("the window built the feed"),
            rows,
            state,
            live,
        }))
    }
}

/// The list workloads.
pub fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "list-uniform-scroll",
            family: Family::Lists,
            description: "a uniform-height list scrolled 29 px per frame",
            warmup: 30,
            frames: 200,
            setup: |harness| uniform_setup(false)(harness),
        },
        Workload {
            name: "list-uniform-live",
            family: Family::Lists,
            description: "a uniform-height list scrolled while three visible rows change per frame",
            warmup: 30,
            frames: 200,
            setup: |harness| uniform_setup(true)(harness),
        },
        Workload {
            name: "list-variable-scroll",
            family: Family::Lists,
            description: "a variable-height list scrolled 29 px per frame",
            warmup: 30,
            frames: 200,
            setup: |harness| variable_setup(false)(harness),
        },
        Workload {
            name: "list-variable-live",
            family: Family::Lists,
            description: "a variable-height list scrolled while three visible rows change per frame",
            warmup: 30,
            frames: 200,
            setup: |harness| variable_setup(true)(harness),
        },
    ]
}
