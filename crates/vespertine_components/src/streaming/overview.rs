//! The overview rail (CMP-029 R17-R21): logical markers mapped to list rows, collapsing tick
//! buckets, the active marker, hover previews and navigation that works for rows that were never
//! measured.

use std::{cell::Cell, ops::Range, rc::Rc};

use vespertine::{
    AnyElement, App, InteractiveElement as _, IntoElement as _, ListState, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, deferred, div,
    hsla, px,
};

use super::controller::FollowController;

/// The container is at least this wide for the rail to show (R17).
pub const RAIL_MIN_WIDTH: f32 = 768.0;
/// The rail shows only with at least this many markers (R17).
pub const RAIL_MIN_MARKERS: usize = 2;
/// Left offset of the column.
pub const RAIL_LEFT: f32 = 16.0;
/// Column width.
pub const RAIL_COLUMN_WIDTH: f32 = 26.0;
/// Height of one tick slot.
pub const TICK_SLOT: f32 = 10.0;
/// Gap between tick slots.
pub const TICK_GAP: f32 = 3.0;
/// Vertical margin (top and bottom).
pub const RAIL_VERTICAL_MARGIN: f32 = 24.0;
/// The most slots the column shows.
pub const MAX_SLOTS: usize = 12;
/// The assumed height before the viewport is measured.
pub const UNMEASURED_HEIGHT: f32 = 600.0;
/// Preview card width.
pub const PREVIEW_WIDTH: f32 = 280.0;
/// Preview prompt cap (characters).
pub const PREVIEW_PROMPT_CAP: usize = 160;
/// Preview reply cap (characters).
pub const PREVIEW_REPLY_CAP: usize = 200;

/// A logical marker (one per section or prompt), identified by item identity, not row index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    /// The item identity.
    pub id: SharedString,
    /// The list row the marker maps to.
    pub row: usize,
    /// The prompt text.
    pub prompt: SharedString,
    /// The opening of the reply, when present.
    pub reply: Option<SharedString>,
}

/// Extracts markers from `(identity, is marker, prompt, reply)` items in row order, mapping each
/// to its row (R17). `rows` are the items' row indices (identity-stable, so a re-ordered or
/// partially loaded list maps correctly).
pub fn extract_markers<I>(
    items: impl IntoIterator<Item = (usize, I)>,
    mut as_marker: impl FnMut(&I) -> Option<(SharedString, SharedString, Option<SharedString>)>,
) -> Vec<Marker> {
    items
        .into_iter()
        .filter_map(|(row, item)| {
            as_marker(&item).map(|(id, prompt, reply)| Marker {
                id,
                row,
                prompt,
                reply,
            })
        })
        .collect()
}

/// Whether the rail shows: the container is at least 768 px wide and there are at least two
/// markers (R17).
pub fn rail_visible(width: f32, markers: usize) -> bool {
    width >= RAIL_MIN_WIDTH && markers >= RAIL_MIN_MARKERS
}

/// The number of tick slots that fit in `height`: `max(1, floor((max(height - 48, 10) + 3) /
/// 13))`, capped at 12 (R18).
pub fn rail_slots(height: f32) -> usize {
    let fit = (((height - 2.0 * RAIL_VERTICAL_MARGIN).max(10.0) + TICK_GAP)
        / (TICK_SLOT + TICK_GAP))
        .floor() as usize;
    fit.clamp(1, MAX_SLOTS)
}

/// The marker ranges of the tick buckets (R18): with `m` markers and `c = clamp(capacity, 1, m)`,
/// bucket `k` covers `[floor(k m / c), floor((k + 1) m / c))`; with `m <= c` every bucket is one
/// marker.
pub fn tick_buckets(markers: usize, capacity: usize) -> Vec<Range<usize>> {
    if markers == 0 {
        return Vec::new();
    }
    let c = capacity.clamp(1, markers);
    (0..c)
        .map(|k| k * markers / c..(k + 1) * markers / c)
        .collect()
}

/// The bucket containing `marker`.
pub fn bucket_of(markers: usize, capacity: usize, marker: usize) -> Option<usize> {
    tick_buckets(markers, capacity)
        .iter()
        .position(|range| range.contains(&marker))
}

/// The active marker (R19): the last marker whose row is at or above the reading row, else the
/// first marker. `marker_rows` ascend.
pub fn active_marker(marker_rows: &[usize], reading_row: usize) -> usize {
    marker_rows
        .iter()
        .rposition(|row| *row <= reading_row)
        .unwrap_or(0)
}

/// The reading row (R19): from the logical scroll top row, walking forward over rows whose
/// measured top is at or above the reading line (viewport top + top chrome inset + 0.5 px),
/// stopping at the first unmeasured row.
pub fn reading_row(list: &ListState, top_inset: f32) -> usize {
    let viewport = list.viewport_bounds();
    let line = f32::from(viewport.top()) + top_inset + 0.5;
    let mut row = list.logical_scroll_top().item_ix;
    while let Some(bounds) = list.bounds_for_item(row + 1) {
        if f32::from(bounds.top()) <= line {
            row += 1;
        } else {
            break;
        }
    }
    row
}

/// Cuts preview text (R20): flattens every whitespace run to a single space; text longer than
/// `max` keeps `max - 1` characters, trimmed of trailing whitespace, followed by an ellipsis.
pub fn truncate_preview(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let kept: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// The content of a bucket's preview card (R20).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    /// The prompt text, cut to 160 characters.
    pub prompt: String,
    /// The opening of the reply, cut to 200 characters, when present.
    pub reply: Option<String>,
    /// "N prompts" when the bucket stands for more than one marker.
    pub count: Option<String>,
}

/// The preview of `bucket`: the representative marker is the active marker when inside the
/// bucket, otherwise the bucket's first marker.
pub fn bucket_preview(markers: &[Marker], bucket: Range<usize>, active: usize) -> Option<Preview> {
    let representative = if bucket.contains(&active) {
        active
    } else {
        bucket.start
    };
    let marker = markers.get(representative)?;
    Some(Preview {
        prompt: truncate_preview(&marker.prompt, PREVIEW_PROMPT_CAP),
        reply: marker
            .reply
            .as_ref()
            .map(|reply| truncate_preview(reply, PREVIEW_REPLY_CAP)),
        count: (bucket.len() > 1).then(|| format!("{} prompts", bucket.len())),
    })
}

/// The hover state of the rail: the hovered bucket. One per rail, owned by the view.
#[derive(Clone, Default)]
pub struct OverviewState {
    hovered: Rc<Cell<Option<usize>>>,
}

impl OverviewState {
    /// A fresh state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The hovered bucket.
    pub fn hovered(&self) -> Option<usize> {
        self.hovered.get()
    }
}

/// Renders the overview rail, or `None` below 768 px of container width or with fewer than two
/// markers (R17). The column is absolutely positioned 16 px from the left edge, 26 px wide and
/// vertically centered; ticks are stacked with 3 px gaps; hovering a bucket shows its preview
/// card; activating a tick scrolls the list so the marker's row sits at the viewport top (R21),
/// also for rows that are not resident or not yet measured.
#[allow(clippy::too_many_arguments)]
pub fn render_overview_rail(
    id: &'static str,
    markers: Rc<Vec<Marker>>,
    list: &ListState,
    controller: &FollowController,
    state: &OverviewState,
    container_width: f32,
    top_inset: f32,
    cx: &App,
) -> Option<AnyElement> {
    if !rail_visible(container_width, markers.len()) {
        return None;
    }
    let measured = f32::from(list.viewport_bounds().size.height);
    let height = if measured <= 0.0 {
        UNMEASURED_HEIGHT
    } else {
        measured
    };
    let capacity = rail_slots(height);
    let buckets = tick_buckets(markers.len(), capacity);
    let rows: Vec<usize> = markers.iter().map(|marker| marker.row).collect();
    let active = active_marker(&rows, reading_row(list, top_inset));
    let active_bucket = bucket_of(markers.len(), capacity, active);

    let theme = vespertine_theme::appearance::try_theme_of(cx);
    let (text, overlay, border, muted) = theme.map_or(
        (
            hsla(0.0, 0.0, 0.93, 1.0),
            hsla(0.0, 0.0, 0.14, 1.0),
            hsla(0.0, 0.0, 0.3, 1.0),
            hsla(0.0, 0.0, 0.65, 1.0),
        ),
        |theme| {
            (
                theme.colors.text.to_hsla(),
                theme.colors.overlay.to_hsla(),
                theme.colors.border.to_hsla(),
                theme.colors.text_muted.to_hsla(),
            )
        },
    );
    let mut foreground = text;
    foreground.a *= 0.8;
    let mut wash = text;
    wash.a *= 0.16;

    let hovered = state.hovered();
    let mut column = div()
        .id(id)
        .absolute()
        .left(px(RAIL_LEFT))
        .top_0()
        .bottom_0()
        .w(px(RAIL_COLUMN_WIDTH))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(TICK_GAP));
    for (index, bucket) in buckets.iter().enumerate() {
        let is_hovered = hovered == Some(index);
        let is_active = active_bucket == Some(index);
        let mark = div()
            .h(px(2.0))
            .w(px(if is_hovered { 20.0 } else { 12.0 }))
            .rounded(px(1.0))
            .bg(if is_active || is_hovered {
                foreground
            } else {
                wash
            });
        let target_row = markers.get(bucket.start).map(|marker| marker.row);
        let hover_state = state.hovered.clone();
        let controller = controller.clone();
        column = column.child(
            div()
                .id((id, index))
                .w_full()
                .h(px(TICK_SLOT))
                .items_center()
                .flex()
                .cursor_pointer()
                .child(mark)
                .on_hover(move |entered, window, _| {
                    if *entered {
                        hover_state.set(Some(index));
                    } else if hover_state.get() == Some(index) {
                        hover_state.set(None);
                    }
                    window.refresh();
                })
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    if let Some(row) = target_row {
                        controller.scroll_to_row(row, window, cx);
                    }
                }),
        );
    }

    let preview = hovered
        .and_then(|index| buckets.get(index).cloned())
        .and_then(|bucket| bucket_preview(&markers, bucket, active));
    if let Some(preview) = preview {
        let mut card = div()
            .absolute()
            .left(px(RAIL_COLUMN_WIDTH))
            .w(px(PREVIEW_WIDTH))
            .p_2()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(border)
            .bg(overlay)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text)
                    .child(SharedString::from(preview.prompt)),
            );
        if let Some(reply) = preview.reply {
            card = card.child(
                div()
                    .text_size(px(11.0))
                    .text_color(muted)
                    .child(SharedString::from(reply)),
            );
        }
        if let Some(count) = preview.count {
            card = card.child(
                div()
                    .text_size(px(10.0))
                    .text_color(muted)
                    .child(SharedString::from(count)),
            );
        }
        column = column.child(deferred(card).with_priority(2));
    }
    Some(column.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a7_slot_capacity() {
        assert_eq!(rail_slots(880.0), MAX_SLOTS);
        assert_eq!(rail_slots(0.0), 1);
        assert_eq!(rail_slots(48.0), 1);
        assert_eq!(rail_slots(100.0), 4);
        // (880 - 48 + 3) / 13 = 64 before the cap.
        assert_eq!(((880.0 - 48.0 + 3.0) / 13.0) as usize, 64);
    }

    #[test]
    fn a7_buckets_are_contiguous_with_sizes_twelve_or_thirteen() {
        let buckets = tick_buckets(100, 8);
        assert_eq!(buckets.len(), 8);
        assert_eq!(buckets[0].start, 0);
        assert_eq!(buckets[7].end, 100);
        for pair in buckets.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        assert!(buckets.iter().all(|b| b.len() == 12 || b.len() == 13));
        // 5 markers into 64 slots: identity.
        let identity = tick_buckets(5, 64);
        assert_eq!(identity.len(), 5);
        assert!(identity.iter().enumerate().all(|(i, b)| *b == (i..i + 1)));
        assert!(tick_buckets(0, 4).is_empty());
        assert_eq!(bucket_of(100, 8, 99), Some(7));
        assert_eq!(bucket_of(100, 8, 0), Some(0));
    }

    #[test]
    fn a7_the_active_marker_defaults_to_the_first() {
        let rows = [3, 10, 20];
        assert_eq!(active_marker(&rows, 0), 0);
        assert_eq!(active_marker(&rows, 3), 0);
        assert_eq!(active_marker(&rows, 12), 1);
        assert_eq!(active_marker(&rows, 99), 2);
        assert_eq!(active_marker(&[], 5), 0);
    }

    #[test]
    fn a7_previews_flatten_whitespace_and_cut_with_an_ellipsis() {
        assert_eq!(truncate_preview("a \n\t b   c", 160), "a b c");
        let long = "word ".repeat(100);
        let cut = truncate_preview(&long, PREVIEW_PROMPT_CAP);
        assert_eq!(cut.chars().count(), 160);
        assert!(cut.ends_with('\u{2026}'));
        assert!(!cut.trim_end_matches('\u{2026}').ends_with(' '));
        let exact = "x".repeat(200);
        assert_eq!(truncate_preview(&exact, 200), exact);
        let over = "x".repeat(201);
        let cut = truncate_preview(&over, 200);
        assert_eq!(cut.chars().count(), 200);
        assert!(cut.ends_with('\u{2026}'));
    }

    #[test]
    fn a7_the_rail_is_hidden_when_narrow_or_with_fewer_than_two_markers() {
        assert!(rail_visible(768.0, 2));
        assert!(!rail_visible(767.9, 5));
        assert!(!rail_visible(1200.0, 1));
        assert!(!rail_visible(1200.0, 0));
    }

    #[test]
    fn previews_name_the_bucket_and_use_the_active_representative() {
        let markers: Vec<Marker> = (0..6)
            .map(|i| Marker {
                id: format!("m{i}").into(),
                row: i * 4,
                prompt: format!("prompt {i}").into(),
                reply: (i % 2 == 0).then(|| "a reply".into()),
            })
            .collect();
        let preview = bucket_preview(&markers, 0..3, 2).unwrap();
        assert_eq!(preview.prompt, "prompt 2");
        assert_eq!(preview.count.as_deref(), Some("3 prompts"));
        let preview = bucket_preview(&markers, 3..4, 0).unwrap();
        assert_eq!(preview.prompt, "prompt 3");
        assert_eq!(preview.count, None);
        assert_eq!(preview.reply, None);
    }
}
