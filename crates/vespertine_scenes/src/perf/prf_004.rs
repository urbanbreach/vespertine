//! PERF-PRF-004 and PERF-PRF-004-2: scrolling, single-section updates and broad refreshes of a
//! 12,000 px tall scroll viewport (spec PRF-004).
//!
//! Both variants show the same content and follow the same per-frame driver. In PERF-PRF-004 the
//! scroll viewport is the root of an entity-backed view of its own embedded in the scene's view
//! (so the runner's per-frame animation request of the outer view does not concern the block); in
//! PERF-PRF-004-2 the viewport and its sections are plain elements built by the scene's view.
//!
//! Frames 0..29 are unchanged; frames 30..179 scroll by -40 px per frame, frames 180..329 by
//! +40; frames 330..389 update the section `(f - 330) mod 100`; from frame 390 on, every tenth
//! frame refreshes all sections. Wheel events go through the window's input path (queued after
//! the frame) with the pointer at (800, 240), over content but over no button.

use crate::common::{BORDER, SANS, SURFACE, WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, AppContext as _, Context, Entity, InputEvent as _, InteractiveElement,
    IntoElement, Modifiers, ParentElement, Render, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement, Styled, TouchPhase, Window, div, point, px,
};

const SECTIONS: usize = 100;
const SECTION_HEIGHT: f32 = 120.0;
const POINTER: (f32, f32) = (800.0, 240.0);
const WHEEL_STEP: f32 = 40.0;

/// The revision of every section, as the per-frame driver last wrote it.
#[derive(Clone)]
struct Revisions([u32; SECTIONS]);

/// What the driver does in frame `f`: the wheel delta to queue, and the sections to refresh with
/// their new revision.
struct Plan {
    wheel: f32,
    updates: Vec<(usize, u32)>,
}

fn plan(f: u64) -> Plan {
    let mut plan = Plan {
        wheel: 0.0,
        updates: Vec::new(),
    };
    match f {
        0..30 => {}
        30..180 => plan.wheel = -WHEEL_STEP,
        180..330 => plan.wheel = WHEEL_STEP,
        330..390 => plan
            .updates
            .push((((f - 330) % SECTIONS as u64) as usize, (f % 100) as u32)),
        _ => {
            if (f - 390).is_multiple_of(10) {
                let revision = (((f - 390) / 10) % 100) as u32;
                plan.updates = (0..SECTIONS).map(|k| (k, revision)).collect();
            }
        }
    }
    plan
}

fn button(section: usize, index: usize) -> impl IntoElement {
    div()
        .id(("button", section * 3 + index))
        .absolute()
        .left(px(16.0 + 88.0 * index as f32))
        .top(px(92.0))
        .w(px(80.0))
        .h(px(24.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(hex(BORDER))
        .bg(hex(SURFACE))
        .hover(|style| style.bg(hex(0xeaeef2)))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.0))
        .text_color(hex(0x202020))
        .child(format!("Action {}", index + 1))
}

fn body_line(section: usize, line: usize, revision: u32) -> impl IntoElement {
    div()
        .absolute()
        .left(px(16.0))
        .top(px(30.0 + 20.0 * (line - 1) as f32))
        .h(px(20.0))
        .line_height(px(20.0))
        .text_size(px(14.0))
        .text_color(hex(0x4a4a4a))
        .child(format!(
            "Section {section}, line {line}: the quick brown fox jumps over the lazy dog, rev {revision:02}"
        ))
}

fn section(index: usize, revision: u32) -> impl IntoElement {
    let fill = if index.is_multiple_of(2) {
        WHITE
    } else {
        0xfafafa
    };
    div()
        .id(("section", index))
        .relative()
        .flex_none()
        .w_full()
        .h(px(SECTION_HEIGHT))
        .bg(hex(fill))
        .border_b(px(1.0))
        .border_color(hex(0xe5e5e5))
        .child(
            div()
                .absolute()
                .left(px(16.0))
                .top(px(8.0))
                .h(px(20.0))
                .line_height(px(20.0))
                .text_size(px(14.0))
                .text_color(hex(0x202020))
                .child(format!("Section {index}")),
        )
        .child(body_line(index, 1, revision))
        .child(body_line(index, 2, revision))
        .child(body_line(index, 3, revision))
        .child(button(index, 0))
        .child(button(index, 1))
        .child(button(index, 2))
}

fn viewport(revisions: &Revisions) -> impl IntoElement {
    div()
        .id("viewport")
        .size_full()
        .bg(hex(WHITE))
        .overflow_y_scroll()
        .children(revisions.0.iter().enumerate().map(|(k, r)| section(k, *r)))
}

/// The scroll viewport as a view of its own (PERF-PRF-004).
struct ViewportView {
    revisions: Revisions,
}

impl Render for ViewportView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        viewport(&self.revisions)
    }
}

pub struct Sections {
    nested: bool,
    /// Scroll layers are on (the default); the `-NOLAYERS` variants switch them off for the
    /// comparison with everything else equal.
    layers: bool,
    configured: bool,
    revisions: Revisions,
    view: Option<Entity<ViewportView>>,
}

impl Sections {
    pub fn nested() -> Self {
        Self {
            nested: true,
            layers: true,
            configured: false,
            revisions: Revisions([0; SECTIONS]),
            view: None,
        }
    }

    pub fn plain() -> Self {
        Self {
            nested: false,
            layers: true,
            configured: false,
            revisions: Revisions([0; SECTIONS]),
            view: None,
        }
    }
}

impl Scene for Sections {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        if !self.configured {
            window.set_scroll_layers_enabled(self.layers);
            self.configured = true;
        }
        let plan = plan(f);
        if plan.wheel != 0.0 {
            let delta = plan.wheel;
            window.defer(cx, move |window, cx| {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position: point(px(POINTER.0), px(POINTER.1)),
                        delta: ScrollDelta::Pixels(point(px(0.0), px(delta))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                    }
                    .to_platform_input(),
                    cx,
                );
            });
        }
        for (index, revision) in &plan.updates {
            self.revisions.0[*index] = *revision;
        }
        let content: AnyElement = if self.nested {
            let revisions = self.revisions.clone();
            let view = self
                .view
                .get_or_insert_with(|| {
                    let initial = revisions.clone();
                    cx.new(|_| ViewportView { revisions: initial })
                })
                .clone();
            if !plan.updates.is_empty() {
                view.update(cx, |view, cx| {
                    view.revisions = revisions;
                    cx.notify();
                });
            }
            view.into_any_element()
        } else {
            viewport(&self.revisions).into_any_element()
        };
        div()
            .size_full()
            .relative()
            .bg(hex(0xeeeeee))
            .font_family(SANS)
            .child(
                div()
                    .absolute()
                    .left(px(40.0))
                    .top(px(40.0))
                    .w(px(800.0))
                    .h(px(400.0))
                    .child(content),
            )
            .into_any_element()
    }
}

register_scene!("PERF-PRF-004", Sections::nested);
register_scene!("PERF-PRF-004-2", Sections::plain);
register_scene!("PERF-PRF-004-NOLAYERS", || Sections {
    layers: false,
    ..Sections::nested()
});
register_scene!("PERF-PRF-004-2-NOLAYERS", || Sections {
    layers: false,
    ..Sections::plain()
});
