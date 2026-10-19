//! The checklist/status panel of CMP-028 R16-R21.
//!
//! The pure model (summary, focus window, rows, signature, expanded-state policy, dismissal) is
//! the checklist model of the status components (CMP-023 R8-R11), re-exported here. This module
//! adds the source cache of R20 and the panel with the CMP-028 layout defaults.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use vespertine::{
    Animation, AnimationExt as _, App, ElementId, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, RenderOnce, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Window, div, edge_faded, px, svg,
};

pub use crate::status::checklist::{
    FOLD_THRESHOLD, Item, ItemStatus, Row, SectionState, SectionStates, Side, Summary, WINDOW,
    focus_window, rows, signature, summary,
};

use super::count::count_label;
use super::icons::ICON_CHECK;
use super::tokens::BadgeTokens;

/// Header height (R21).
pub const HEADER_HEIGHT: f32 = 32.0;
/// Row horizontal padding.
pub const ROW_PADDING_X: f32 = 8.0;
/// Row text size.
pub const ROW_TEXT_SIZE: f32 = 12.5;
/// Row line height.
pub const ROW_LINE_HEIGHT: f32 = 17.0;
/// Width of the status glyph slot.
pub const GLYPH_SLOT: f32 = 14.0;
/// Side of an action button.
pub const ACTION_SIZE: f32 = 24.0;
/// Side inset: one step narrower than the surface the panel docks above.
pub const SIDE_INSET: f32 = 16.0;
/// Height beyond which an expanded list scrolls with fading edges.
pub const MAX_LIST_HEIGHT: f32 = 160.0;
/// The finished-state headline.
pub const FINISHED_TEXT: &str = "All done";

/// The cache of the latest list per source (R20): a list is recomputed only when the source's
/// revision or length changes.
#[derive(Default)]
pub struct ListCache {
    entries: HashMap<SharedString, (u64, usize, Rc<[Item]>)>,
    builds: usize,
}

impl ListCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The list of `source` at `revision` with `len` items: the cached one when the key
    /// `(source, revision, len)` is unchanged, else the result of `build`.
    pub fn get_or_build(
        &mut self,
        source: &SharedString,
        revision: u64,
        len: usize,
        build: impl FnOnce() -> Vec<Item>,
    ) -> Rc<[Item]> {
        if let Some((cached_revision, cached_len, items)) = self.entries.get(source)
            && *cached_revision == revision
            && *cached_len == len
        {
            return items.clone();
        }
        let items: Rc<[Item]> = build().into();
        self.builds += 1;
        self.entries
            .insert(source.clone(), (revision, len, items.clone()));
        items
    }

    /// How many times a list was (re)built.
    pub fn builds(&self) -> usize {
        self.builds
    }
}

/// The checklist panel. Presentation state (expanded choice, fold toggles, dismissal, settled
/// flag, epoch) is kept per `owner` in memory only, in the application's [`SectionStates`].
#[derive(IntoElement)]
pub struct ChecklistPanel {
    owner: SharedString,
    name: SharedString,
    items: Rc<[Item]>,
    live: bool,
    finished_text: SharedString,
    tokens: BadgeTokens,
}

impl ChecklistPanel {
    /// A panel for the items of `owner` (an owner key such as a conversation id). `name` is the
    /// section name used in the toggle's label ("Collapse tasks"); `live` says whether work is
    /// running.
    pub fn new(
        owner: impl Into<SharedString>,
        name: impl Into<SharedString>,
        items: Rc<[Item]>,
        live: bool,
        tokens: &BadgeTokens,
    ) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
            items,
            live,
            finished_text: FINISHED_TEXT.into(),
            tokens: tokens.clone(),
        }
    }

    /// The headline shown when the list is finished (default `All done`).
    pub fn finished_text(mut self, text: impl Into<SharedString>) -> Self {
        self.finished_text = text.into();
        self
    }
}

fn glyph(item: &Item, live: bool, tokens: &BadgeTokens) -> impl IntoElement {
    let slot = div()
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .w(px(GLYPH_SLOT))
        .h(px(ROW_LINE_HEIGHT));
    match item.status {
        ItemStatus::Completed => slot.child(
            svg()
                .path(ICON_CHECK)
                .size(px(12.))
                .text_color(tokens.success),
        ),
        ItemStatus::InProgress => {
            let color = if live { tokens.busy } else { tokens.muted };
            let ring = div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(10.))
                .rounded_full()
                .border_1()
                .border_color(color);
            // A stopped run must not look busy forever: the dot only shows while live.
            slot.child(ring.when(live, |ring| {
                ring.child(div().size(px(4.)).rounded_full().bg(color))
            }))
        }
        ItemStatus::Pending => slot.child(
            div()
                .size(px(10.))
                .rounded_full()
                .border_1()
                .border_color(tokens.muted),
        ),
    }
}

use vespertine::prelude::FluentBuilder as _;

impl RenderOnce for ChecklistPanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let items = self.items;
        let owner = self.owner;
        let live = self.live;
        let summary = summary(&items);
        let (expanded, show_earlier, show_later, dismissed, epoch) = {
            let state = SectionStates::state(cx, &owner);
            state.observe(summary.finished && !live);
            (
                state.is_expanded(summary.finished),
                state.show_earlier(),
                state.show_later(),
                state.is_dismissed(&items),
                state.epoch(),
            )
        };
        // A section with no items, or a dismissed list, renders nothing.
        if items.is_empty() || dismissed {
            return div().id(ElementId::Name(format!("{owner}-checklist-empty").into()));
        }
        let scroll = window.use_keyed_state(
            ElementId::Name(format!("{owner}-checklist-scroll").into()),
            cx,
            |_, _| ScrollHandle::new(),
        );
        let scroll = scroll.read(cx).clone();

        let headline: SharedString = if summary.finished {
            self.finished_text.clone()
        } else {
            summary
                .headline()
                .map(|index| items[index].text.clone())
                .unwrap_or_default()
        };
        let counts: SharedString = format!(
            "{} of {}",
            count_label(summary.done),
            count_label(summary.total)
        )
        .into();
        let label = format!(
            "{} {}",
            if expanded { "Collapse" } else { "Expand" },
            self.name
        );

        let finished = summary.finished;
        let toggle_owner = owner.clone();
        let toggle = div()
            .id(ElementId::Name(format!("{owner}-checklist-toggle").into()))
            .role(accesskit::Role::Button)
            .aria_label(label)
            .aria_expanded(expanded)
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .h(px(HEADER_HEIGHT))
            .px(px(ROW_PADDING_X))
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                SectionStates::state(cx, &toggle_owner).toggle(finished);
                window.refresh();
            })
            .child(
                crate::layout::flexible_cell()
                    .truncate()
                    .text_size(px(ROW_TEXT_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tokens.text)
                    .child(headline),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(tokens.faint)
                    .child(counts),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(tokens.muted)
                    .child(if expanded { "\u{25BE}" } else { "\u{25B8}" }),
            );

        let dismiss = SectionState::can_dismiss(live).then(|| {
            let dismiss_owner = owner.clone();
            let dismiss_items = items.clone();
            div()
                .id(ElementId::Name(format!("{owner}-checklist-dismiss").into()))
                .role(accesskit::Role::Button)
                .aria_label(format!("Dismiss {}", self.name))
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .size(px(ACTION_SIZE))
                .rounded(px(6.))
                .cursor_pointer()
                .text_color(tokens.muted)
                .hover(|style| style.bg(tokens.element_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    SectionStates::state(cx, &dismiss_owner).dismiss(&dismiss_items);
                    window.refresh();
                })
                .child("\u{2715}")
        });

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(HEADER_HEIGHT))
            .child(toggle)
            .children(dismiss);

        let body = expanded.then(|| {
            let rows = rows(&items, show_earlier, show_later);
            let list = div()
                .id(ElementId::Name(format!("{owner}-checklist-rows").into()))
                .track_scroll(&scroll)
                .overflow_y_scroll()
                .max_h(px(MAX_LIST_HEIGHT))
                .flex()
                .flex_col()
                .children(rows.into_iter().map(|row| match row {
                    Row::Item(index) => {
                        let item = &items[index];
                        let (color, weight) = match item.status {
                            ItemStatus::Completed => (tokens.faint, FontWeight::NORMAL),
                            ItemStatus::InProgress => (tokens.text, FontWeight::MEDIUM),
                            ItemStatus::Pending => (tokens.muted, FontWeight::NORMAL),
                        };
                        div()
                            .id(ElementId::NamedInteger(
                                format!("{owner}-checklist-item").into(),
                                index as u64,
                            ))
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(8.))
                            .px(px(ROW_PADDING_X))
                            .child(glyph(item, live, &tokens))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(ROW_TEXT_SIZE))
                                    .line_height(px(ROW_LINE_HEIGHT))
                                    .font_weight(weight)
                                    .text_color(color)
                                    .child(item.text.clone()),
                            )
                            .into_any_element()
                    }
                    Row::Fold { side, count, open } => {
                        let fold_owner = owner.clone();
                        let text = match side {
                            Side::Earlier => format!("{count} earlier"),
                            Side::Later => format!("{count} later"),
                        };
                        div()
                            .id(ElementId::NamedInteger(
                                format!("{owner}-checklist-fold").into(),
                                side as u64,
                            ))
                            .role(accesskit::Role::Button)
                            .aria_expanded(open)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .px(px(ROW_PADDING_X))
                            .h(px(ROW_LINE_HEIGHT))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(tokens.faint)
                            .on_click(move |_, window, cx| {
                                SectionStates::state(cx, &fold_owner).toggle_fold(side);
                                window.refresh();
                            })
                            .child(div().flex_none().w(px(GLYPH_SLOT)).child(if open {
                                "\u{25BE}"
                            } else {
                                "\u{25B8}"
                            }))
                            .child(text)
                            .into_any_element()
                    }
                }));
            // Rows fade in again whenever the toggle bumps the epoch.
            let list = list.with_animation(
                ElementId::NamedInteger(format!("{owner}-checklist-fade").into(), epoch),
                Animation::new(Duration::from_millis(160)),
                |element, progress| element.opacity(progress),
            );
            edge_faded(px(12.), true, true, list).fade_overflow_y(&scroll)
        });

        div()
            .id(ElementId::Name(format!("{owner}-checklist").into()))
            .mx(px(SIDE_INSET))
            .flex()
            .flex_col()
            .child(header)
            .children(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(count: usize, headline: Option<usize>, finished: bool) -> Vec<Item> {
        (0..count)
            .map(|index| {
                let status = if finished {
                    ItemStatus::Completed
                } else {
                    match headline {
                        Some(h) if index < h => ItemStatus::Completed,
                        Some(h) if index == h => ItemStatus::InProgress,
                        _ => ItemStatus::Pending,
                    }
                };
                Item::new(format!("item {index}"), status)
            })
            .collect()
    }

    fn item_indexes(rows: &[Row]) -> Vec<usize> {
        rows.iter()
            .filter_map(|row| match row {
                Row::Item(index) => Some(*index),
                Row::Fold { .. } => None,
            })
            .collect()
    }

    #[test]
    fn six_items_never_fold() {
        for headline in [None, Some(0), Some(3), Some(5)] {
            let list = items(6, headline, false);
            let rows = rows(&list, false, false);
            assert_eq!(item_indexes(&rows), (0..6).collect::<Vec<_>>());
            assert!(rows.iter().all(|row| matches!(row, Row::Item(_))));
        }
    }

    #[test]
    fn ten_items_with_headline_seven_show_six_to_eight_between_two_folds() {
        let list = items(10, Some(7), false);
        assert_eq!(summary(&list).headline(), Some(7));
        let rows = rows(&list, false, false);
        assert_eq!(
            rows,
            vec![
                Row::Fold {
                    side: Side::Earlier,
                    count: 6,
                    open: false
                },
                Row::Item(6),
                Row::Item(7),
                Row::Item(8),
                Row::Fold {
                    side: Side::Later,
                    count: 1,
                    open: false
                },
            ]
        );
        // Open folds list the hidden items at the outer edges, in the producer's order.
        let open = super::rows(&list, true, true);
        assert_eq!(item_indexes(&open), (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn a_finished_ten_item_list_shows_the_last_three() {
        let list = items(10, None, true);
        let rows = rows(&list, false, false);
        assert_eq!(
            rows,
            vec![
                Row::Fold {
                    side: Side::Earlier,
                    count: 7,
                    open: false
                },
                Row::Item(7),
                Row::Item(8),
                Row::Item(9),
            ]
        );
    }

    #[test]
    fn dismissal_holds_for_the_same_list_and_ends_when_an_item_changes() {
        let mut state = SectionState::new();
        let mut list = items(4, Some(1), false);
        assert!(
            SectionState::can_dismiss(false),
            "idle lists can be dismissed, finished or not"
        );
        assert!(!SectionState::can_dismiss(true));
        state.dismiss(&list);
        assert!(state.is_dismissed(&list));
        assert!(state.is_dismissed(&list.clone()));
        list[2].status = ItemStatus::InProgress;
        assert!(!state.is_dismissed(&list));
        list[2].status = ItemStatus::Pending;
        assert!(
            state.is_dismissed(&list),
            "the signature is of the content, in order"
        );
        list[3].text = "changed".into();
        assert!(!state.is_dismissed(&list));
    }

    #[test]
    fn the_settled_transition_clears_an_explicit_choice_exactly_once() {
        let mut state = SectionState::new();
        state.toggle(false);
        assert_eq!(
            state.explicit(),
            Some(false),
            "the user collapsed an unfinished list"
        );
        state.observe(false);
        assert_eq!(state.explicit(), Some(false));
        state.observe(true);
        assert_eq!(state.explicit(), None, "tidied on the first settled frame");
        // A later manual expand is respected, also across further settled frames.
        state.toggle(true);
        assert_eq!(state.explicit(), Some(true));
        state.observe(true);
        assert_eq!(state.explicit(), Some(true));
        // A finished list stays compact when a new task starts.
        let mut fresh = SectionState::new();
        fresh.observe(true);
        assert!(!fresh.is_expanded(true));
        fresh.observe(false);
        assert!(!fresh.is_expanded(true));
        assert!(fresh.is_expanded(false));
    }

    #[test]
    fn toggling_bumps_the_epoch() {
        let mut state = SectionState::new();
        let before = state.epoch();
        state.toggle(false);
        assert_eq!(state.epoch(), before + 1);
    }

    #[test]
    fn the_cache_rebuilds_only_when_revision_or_length_change() {
        let mut cache = ListCache::new();
        let source: SharedString = "conversation-1".into();
        let first = cache.get_or_build(&source, 1, 2, || items(2, Some(0), false));
        let again = cache.get_or_build(&source, 1, 2, || unreachable!("cached"));
        assert!(Rc::ptr_eq(&first, &again));
        assert_eq!(cache.builds(), 1);
        cache.get_or_build(&source, 2, 2, || items(2, Some(1), false));
        cache.get_or_build(&source, 2, 3, || items(3, Some(1), false));
        assert_eq!(cache.builds(), 3);
        let other: SharedString = "conversation-2".into();
        cache.get_or_build(&other, 1, 2, || items(2, None, true));
        assert_eq!(cache.builds(), 4);
    }

    #[test]
    fn counts_use_the_capped_label_only_for_display() {
        let list = items(120, None, true);
        let summary = summary(&list);
        assert_eq!((summary.done, summary.total), (120, 120));
        assert_eq!(count_label(summary.total), "99+");
    }
}
