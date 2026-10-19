//! Nested layout changes (R4).

use super::{BORDER, INK, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use vespertine::{
    AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px,
};

const DEPTH: usize = 6;
const LEAVES: usize = 3;

/// One level of a nested tree of panels: a header, leaf boxes and the next level in a
/// flex row whose first child's basis the step changes.
struct Level {
    depth: usize,
    basis: f32,
    inner: Option<Entity<Level>>,
}

impl Render for Level {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(6.0))
            .flex()
            .flex_row()
            .gap(px(6.0))
            .border_1()
            .border_color(hex(BORDER))
            .bg(hex(if self.depth.is_multiple_of(2) {
                WHITE
            } else {
                SURFACE
            }))
            .child(
                div()
                    .w(px(self.basis))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(format!("Level {} width {:.0}", self.depth, self.basis))
                    .children((0..LEAVES).map(|leaf| {
                        div()
                            .h(px(22.0))
                            .px(px(4.0))
                            .border_1()
                            .border_color(hex(BORDER))
                            .child(format!("Leaf {leaf} of level {}", self.depth))
                    })),
            )
            .children(
                self.inner
                    .clone()
                    .map(|inner| div().flex_1().h_full().child(inner)),
            )
    }
}

struct NestedRoot {
    outer: Entity<Level>,
}

impl Render for NestedRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .font_family(SANS)
            .text_size(px(13.0))
            .text_color(hex(INK))
            .child(self.outer.clone())
    }
}

struct NestedDriver {
    levels: Vec<Entity<Level>>,
}

impl Driver for NestedDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let depth = frame as usize % DEPTH;
        let basis = 90.0 + ((frame * 13) % 7) as f32 * 8.0;
        let level = &self.levels[depth];
        stage.mutate(|app| {
            level.update(app, |level, cx| {
                level.basis = basis;
                cx.notify();
            });
        });
    }
}

fn setup(harness: &mut dyn Host) -> Result<Box<dyn Driver>> {
    let mut levels: Vec<Entity<Level>> = Vec::new();
    harness.open_window(|_, cx| {
        let mut inner = None;
        for depth in (0..DEPTH).rev() {
            let entity = cx.new(|_| Level {
                depth,
                basis: 100.0,
                inner: inner.take(),
            });
            levels.push(entity.clone());
            inner = Some(entity);
        }
        levels.reverse();
        let outer = inner.expect("at least one level");
        AnyView::from(cx.new(|_| NestedRoot { outer }))
    })?;
    Ok(Box::new(NestedDriver { levels }))
}

/// The nested-layout workload.
pub fn workloads() -> Vec<Workload> {
    vec![Workload {
        name: "nested-layout",
        family: Family::NestedLayout,
        description: "one level of a six-deep nested flex tree changes its basis every frame",
        warmup: 30,
        frames: 200,
        setup,
    }]
}
