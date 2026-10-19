//! The collapsible status section (CMP-023 R8-R12): a checklist header and body with
//! remembered expansion, folding of long lists, dismissal and item presentation.

use std::rc::Rc;
use std::time::Duration;

use vespertine::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, ElementId, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, LineCap, LineJoin, ParentElement, PathBuilder, PathStyle,
    Pixels, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement as _, StrokeOptions,
    Styled, Window, canvas, div, edge_faded, point, px,
};

use super::button::{PressHandler, button};
use super::checklist::{Item, ItemStatus, Row, SectionState, SectionStates, Side, rows, summary};
use super::indicators::{capped_label, mini_glyph_spinner};
use super::motion::circle_points;
use super::theme::StatusColors;

/// Header height.
pub const HEADER_HEIGHT: f32 = 32.0;
/// Width of the glyph slot of an item row.
pub const GLYPH_SLOT: f32 = 14.0;
/// Item text size.
pub const TEXT_SIZE: f32 = 12.5;
/// Item line height.
pub const LINE_HEIGHT: f32 = 17.0;
/// Minimum row height.
pub const MIN_ROW_HEIGHT: f32 = 26.0;
/// Fade-in duration replayed when the section is toggled.
pub const FADE_IN: Duration = Duration::from_millis(160);
/// Count cap of the progress count label.
pub const COUNT_CAP: u32 = 99;

type ToggleHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type FoldHandler = Rc<dyn Fn(Side, bool, &mut Window, &mut App)>;

/// The status section. State (expanded choice, folds, dismissal, settled flag) is kept per
/// `owner` key in memory for the lifetime of the process.
#[derive(IntoElement)]
pub struct StatusSection {
    owner: SharedString,
    items: Vec<Item>,
    live: bool,
    name: SharedString,
    finished_text: SharedString,
    show_count: bool,
    on_toggle: Option<ToggleHandler>,
    on_fold_toggle: Option<FoldHandler>,
    on_dismiss: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    max_body_height: f32,
}

/// A status section for the owner's `items`; `live` says whether work is running.
pub fn status_section(
    owner: impl Into<SharedString>,
    items: Vec<Item>,
    live: bool,
) -> StatusSection {
    StatusSection {
        owner: owner.into(),
        items,
        live,
        name: "status".into(),
        finished_text: "All done".into(),
        show_count: true,
        on_toggle: None,
        on_fold_toggle: None,
        on_dismiss: None,
        max_body_height: 156.0,
    }
}

impl StatusSection {
    /// The section name used in the accessible label ("Collapse <name>" / "Expand <name>").
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
        self
    }

    /// The header text of a finished list (default "All done").
    pub fn finished_text(mut self, text: impl Into<SharedString>) -> Self {
        self.finished_text = text.into();
        self
    }

    /// Shows or hides the `done / total` count in the header (default shown).
    pub fn show_count(mut self, show: bool) -> Self {
        self.show_count = show;
        self
    }

    /// Called after the header toggle changes the expansion, with the new effective value.
    pub fn on_toggle(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    /// Called after a fold row toggles: the side and whether it is now open.
    pub fn on_fold_toggle(
        mut self,
        handler: impl Fn(Side, bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_fold_toggle = Some(Rc::new(handler));
        self
    }

    /// Called after the section was dismissed.
    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// Maximum body height before the list scrolls inside the section with fading edges.
    pub fn max_body_height(mut self, height: f32) -> Self {
        self.max_body_height = height;
        self
    }
}

fn stroke_path(
    window: &mut Window,
    origin: vespertine::Point<Pixels>,
    points: &[(f32, f32)],
    width: f32,
    color: Hsla,
) {
    let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(
        StrokeOptions::default()
            .with_line_width(width)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round),
    ));
    for (index, (x, y)) in points.iter().enumerate() {
        let at = point(origin.x + px(*x), origin.y + px(*y));
        if index == 0 {
            builder.move_to(at);
        } else {
            builder.line_to(at);
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// Which glyph an item shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// A check mark in the success color.
    Check,
    /// The mini glyph spinner (in progress while work is live).
    Spinner,
    /// A still ring in the text color (in progress while work is not live).
    StillRing,
    /// An empty ring in the muted color (pending).
    EmptyRing,
}

/// The glyph of an item (R12).
pub fn glyph_for(status: ItemStatus, live: bool) -> Glyph {
    match (status, live) {
        (ItemStatus::Completed, _) => Glyph::Check,
        (ItemStatus::InProgress, true) => Glyph::Spinner,
        (ItemStatus::InProgress, false) => Glyph::StillRing,
        (ItemStatus::Pending, _) => Glyph::EmptyRing,
    }
}

fn glyph_slot(
    owner: &SharedString,
    index: usize,
    glyph: Glyph,
    colors: &StatusColors,
) -> AnyElement {
    let slot = div()
        .w(px(GLYPH_SLOT))
        .h(px(LINE_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match glyph {
        Glyph::Spinner => slot
            .child(mini_glyph_spinner((owner.clone(), index), 2.5).tint(colors.accent))
            .into_any_element(),
        Glyph::Check | Glyph::StillRing | Glyph::EmptyRing => {
            let color = match glyph {
                Glyph::Check => colors.success,
                Glyph::StillRing => colors.text,
                _ => colors.muted,
            };
            slot.child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        if glyph == Glyph::Check {
                            let (cx, cy) = (w / 2.0, h / 2.0);
                            stroke_path(
                                window,
                                bounds.origin,
                                &[(cx - 3.5, cy), (cx - 1.0, cy + 2.8), (cx + 3.5, cy - 3.0)],
                                1.6,
                                color,
                            );
                        } else {
                            let ring = circle_points(w / 2.0, h / 2.0, 4.2);
                            stroke_path(window, bounds.origin, &ring, 1.4, color);
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
        }
    }
}

fn chevron(open: bool, color: Hsla) -> AnyElement {
    div()
        .w(px(12.0))
        .h(px(12.0))
        .flex_none()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                    let points: [(f32, f32); 3] = if open {
                        [(2.5, 4.0), (6.0, 7.5), (9.5, 4.0)]
                    } else {
                        [(4.0, 2.5), (7.5, 6.0), (4.0, 9.5)]
                    };
                    stroke_path(window, bounds.origin, &points, 1.4, color);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn item_row(
    owner: &SharedString,
    index: usize,
    item: &Item,
    live: bool,
    colors: &StatusColors,
) -> AnyElement {
    let (color, weight) = match item.status {
        ItemStatus::Completed => (colors.faint, FontWeight::NORMAL),
        ItemStatus::InProgress => (colors.text, FontWeight::MEDIUM),
        ItemStatus::Pending => (colors.muted, FontWeight::NORMAL),
    };
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px((MIN_ROW_HEIGHT - LINE_HEIGHT) / 2.0))
        .min_h(px(MIN_ROW_HEIGHT))
        .child(glyph_slot(
            owner,
            index,
            glyph_for(item.status, live),
            colors,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(TEXT_SIZE))
                .line_height(px(LINE_HEIGHT))
                .text_color(color)
                .font_weight(weight)
                .child(item.text.clone()),
        )
        .into_any_element()
}

fn sub(owner: &SharedString, name: &'static str) -> ElementId {
    ElementId::from((ElementId::from(owner.clone()), name))
}

/// The accessible label of the header toggle: the action plus the section name.
pub fn toggle_label(expanded: bool, name: &str) -> String {
    format!("{} {name}", if expanded { "Collapse" } else { "Expand" })
}

impl RenderOnce for StatusSection {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = StatusColors::active(cx);
        let view = window.current_view();
        let summary = summary(&self.items);
        let settled = summary.finished && !self.live;
        let owner = self.owner.clone();

        let state: SectionState = {
            let state = SectionStates::state(cx, &owner);
            state.observe(settled);
            state.clone()
        };
        if self.items.is_empty() || state.is_dismissed(&self.items) {
            return div().into_any_element();
        }
        let expanded = state.is_expanded(summary.finished);

        let headline = if summary.finished {
            Some(self.finished_text.clone())
        } else {
            summary
                .headline()
                .map(|index| self.items[index].text.clone())
        };

        // Header toggle.
        let toggle_owner = owner.clone();
        let finished = summary.finished;
        let on_toggle = self.on_toggle.clone();
        let toggle_press: PressHandler = Rc::new(move |window, cx| {
            let now_expanded = {
                let state = SectionStates::state(cx, &toggle_owner);
                state.toggle(finished);
                state.is_expanded(finished)
            };
            if let Some(handler) = &on_toggle {
                handler(now_expanded, window, cx);
            }
            cx.notify(view);
        });
        let label = toggle_label(expanded, &self.name);
        let tooltip_label: SharedString = label.clone().into();
        let mut toggle = button(
            sub(&owner, "toggle"),
            label,
            Some((tooltip_label, colors.hover, colors.text)),
            colors.accent,
            toggle_press,
        )
        .flex_1()
        .min_w(px(0.0))
        .h(px(HEADER_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .child(chevron(expanded, colors.muted));
        if let Some(headline) = headline {
            toggle = toggle.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(TEXT_SIZE))
                    .line_height(px(LINE_HEIGHT))
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(headline),
            );
        }
        if self.show_count {
            toggle = toggle.child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(colors.faint)
                    .child(format!(
                        "{} / {}",
                        capped_label(summary.done as u32, COUNT_CAP),
                        capped_label(summary.total as u32, COUNT_CAP)
                    )),
            );
        }
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .child(toggle);

        // Dismissal, available only while work is not live.
        if SectionState::can_dismiss(self.live) {
            let dismiss_owner = owner.clone();
            let items = self.items.clone();
            let on_dismiss = self.on_dismiss.clone();
            let press: PressHandler = Rc::new(move |window, cx| {
                SectionStates::state(cx, &dismiss_owner).dismiss(&items);
                if let Some(handler) = &on_dismiss {
                    handler(window, cx);
                }
                cx.notify(view);
            });
            let name = self.name.clone();
            header = header.child(
                button(
                    sub(&owner, "dismiss"),
                    format!("Dismiss {name}"),
                    None,
                    colors.accent,
                    press,
                )
                .w(px(24.0))
                .h(px(24.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .text_color(colors.muted)
                .child(close_glyph(colors.muted)),
            );
        }

        let mut root = div()
            .flex()
            .flex_col()
            .w_full()
            .text_size(px(TEXT_SIZE))
            .child(header);

        if expanded {
            let scroll = window
                .use_keyed_state(sub(&owner, "scroll"), cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone();
            let mut body = div()
                .id(sub(&owner, "body"))
                .flex()
                .flex_col()
                .max_h(px(self.max_body_height))
                .overflow_y_scroll()
                .track_scroll(&scroll);
            for row in rows(&self.items, state.show_earlier(), state.show_later()) {
                match row {
                    Row::Item(index) => {
                        body = body.child(item_row(
                            &owner,
                            index,
                            &self.items[index],
                            self.live,
                            &colors,
                        ));
                    }
                    Row::Fold { side, count, open } => {
                        let fold_owner = owner.clone();
                        let on_fold = self.on_fold_toggle.clone();
                        let press: PressHandler = Rc::new(move |window, cx| {
                            let now_open = {
                                let state = SectionStates::state(cx, &fold_owner);
                                state.toggle_fold(side);
                                match side {
                                    Side::Earlier => state.show_earlier(),
                                    Side::Later => state.show_later(),
                                }
                            };
                            if let Some(handler) = &on_fold {
                                handler(side, now_open, window, cx);
                            }
                            cx.notify(view);
                        });
                        let text = fold_label(side, count);
                        body = body.child(
                            button(
                                sub(
                                    &owner,
                                    match side {
                                        Side::Earlier => "fold-earlier",
                                        Side::Later => "fold-later",
                                    },
                                ),
                                text.clone(),
                                None,
                                colors.accent,
                                press,
                            )
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(8.0))
                            .min_h(px(MIN_ROW_HEIGHT))
                            .text_size(px(11.0))
                            .text_color(colors.muted)
                            .child(
                                div()
                                    .w(px(GLYPH_SLOT))
                                    .flex_none()
                                    .child(chevron(open, colors.muted)),
                            )
                            .child(text),
                        );
                    }
                }
            }
            let faded = edge_faded(px(12.0), true, true, body).fade_overflow_y(&scroll);
            let epoch = state.epoch();
            root = root.child(div().w_full().child(faded).with_animation(
                ElementId::from((owner.clone(), epoch as usize)),
                Animation::new(FADE_IN),
                |element, delta| element.opacity(delta),
            ));
        }
        root.into_any_element()
    }
}

/// The label of a fold row.
pub fn fold_label(side: Side, count: usize) -> String {
    match side {
        Side::Earlier => format!("{count} earlier"),
        Side::Later => format!("{count} later"),
    }
}

fn close_glyph(color: Hsla) -> AnyElement {
    div()
        .w(px(10.0))
        .h(px(10.0))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                    stroke_path(window, bounds.origin, &[(1.5, 1.5), (8.5, 8.5)], 1.3, color);
                    stroke_path(window, bounds.origin, &[(8.5, 1.5), (1.5, 8.5)], 1.3, color);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_follow_status_and_liveness() {
        assert_eq!(glyph_for(ItemStatus::Completed, true), Glyph::Check);
        assert_eq!(glyph_for(ItemStatus::Completed, false), Glyph::Check);
        assert_eq!(glyph_for(ItemStatus::InProgress, true), Glyph::Spinner);
        // A stopped run must not look busy forever.
        assert_eq!(glyph_for(ItemStatus::InProgress, false), Glyph::StillRing);
        assert_eq!(glyph_for(ItemStatus::Pending, true), Glyph::EmptyRing);
    }

    #[test]
    fn labels_state_the_action() {
        assert_eq!(toggle_label(true, "plan"), "Collapse plan");
        assert_eq!(toggle_label(false, "plan"), "Expand plan");
        assert_eq!(fold_label(Side::Earlier, 4), "4 earlier");
        assert_eq!(fold_label(Side::Later, 3), "3 later");
    }
}
