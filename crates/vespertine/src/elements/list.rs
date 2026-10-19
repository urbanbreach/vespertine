//! A list element that can be used to render a large number of differently sized elements
//! efficiently. Clients of this API need to ensure that elements outside of the scrolled
//! area do not change their height for this element to function correctly. If your elements
//! do change height, notify the list element via [`ListState::splice`] or [`ListState::reset`].
//! In order to minimize re-renders, this element's state is stored intrusively
//! on your own views, so that your code can coordinate directly with the list element's cached state.
//!
//! If all of your elements are the same height, see [`crate::UniformList`] for a simpler API
//!
//! # Model
//!
//! A list operates on a *logical dataset* of `N` rows. By default `N` is the number of rows
//! that were mounted with [`ListState::new`], [`ListState::splice`] and friends. In *windowed
//! mode* (a positive finite [estimate](ListState::set_estimated_item_height) together with a
//! [logical item count](ListState::set_logical_item_count)) `N` is the configured count and
//! the mounted rows form a contiguous window of the dataset starting at
//! [`ListState::window_start`]. Unmounted rows occupy a placeholder height but are never
//! rendered. Scroll anchors ([`ListOffset`]) and the row indices passed to the render callback
//! are always logical indices; ranges passed to splice and remeasure operations are relative
//! to the mounted rows.

use crate::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, DispatchPhase, Edges, Element, EntityId,
    FocusHandle, FocusId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    Overflow, Pixels, Point, ScrollDelta, ScrollWheelEvent, Size, Style, StyleRefinement, Styled,
    Window, point, px, size,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    hash::Hash,
    ops::Range,
    rc::Rc,
};
use vespertine_collections::VecDeque;
use vespertine_refineable::Refineable as _;
use vespertine_sum_tree::{Bias, SumTree};

type RenderItemFn = dyn FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static;
type ScrollHandler = Rc<RefCell<dyn FnMut(&ListScrollEvent, &mut Window, &mut App)>>;

/// Minimum height of an unmounted placeholder row, in logical pixels.
const PLACEHOLDER_MIN_HEIGHT: Pixels = px(1.);
/// Distance from the bottom within which a stopped tail-follow list resumes following.
const TAIL_RESUME_TOLERANCE: Pixels = px(1.);
/// Upper bound on retained measured heights of rows that left the mounted window.
const MAX_UNMOUNTED_HINTS: usize = 2048;

/// Construct a new list element
pub fn list(
    state: ListState,
    render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
) -> List {
    List {
        state,
        render_item: Box::new(render_item),
        style: StyleRefinement::default(),
        sizing_behavior: ListSizingBehavior::default(),
        a11y: crate::Interactivity::default(),
    }
}

/// A list element
pub struct List {
    state: ListState,
    render_item: Box<RenderItemFn>,
    style: StyleRefinement,
    sizing_behavior: ListSizingBehavior,
    pub(crate) a11y: crate::Interactivity,
}

impl List {
    /// Set the sizing behavior for the list.
    pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self {
        self.sizing_behavior = behavior;
        self
    }
}

/// The list state that views must hold on behalf of the list element.
#[derive(Clone)]
pub struct ListState(Rc<RefCell<StateInner>>);

impl std::fmt::Debug for ListState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ListState")
    }
}

/// A reserved suffix of the list (LAY-002): rows from `start` on are padded, inside the last
/// row's layout box, up to the viewport height minus `inset`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TrailingReservation {
    start: usize,
    inset: Pixels,
}

struct StateInner {
    last_layout_bounds: Option<Bounds<Pixels>>,
    last_padding: Option<Edges<Pixels>>,
    /// The content width (list width minus horizontal padding) of the last layout.
    last_content_width: Option<Pixels>,
    /// The mounted rows. In windowed mode row `i` is logical row `window_start + i`.
    items: SumTree<ListItem>,
    logical_scroll_top: Option<ListOffset>,
    alignment: ListAlignment,
    overdraw: Pixels,
    reset: bool,
    scroll_handler: Option<ScrollHandler>,
    last_visible_range: Option<Range<usize>>,
    scrollbar_drag_start_height: Option<Pixels>,
    measuring_behavior: ListMeasuringBehavior,
    pending_scroll: Option<PendingScroll>,
    follow_state: FollowState,
    /// Uniform height estimate, only ever `Some` for strictly positive finite values.
    estimate: Option<Pixels>,
    /// Configured logical item count; windowed mode needs it together with `estimate`.
    configured_count: Option<usize>,
    window_start: usize,
    /// Retained heights of rows that left the mounted window, by logical index.
    unmounted_hints: BTreeMap<usize, Pixels>,
    reservation: Option<TrailingReservation>,
    /// The reservation extra currently included in the last row's stored height.
    reservation_extra: Pixels,
    /// The focus handle of the row that held focus at the last layout.
    last_focus_id: Option<FocusId>,
}

/// Deferred scroll adjustment applied after the scroll-top item has been remeasured.
///
/// An absolute pending scroll preserves the same pixel offset into the item, which keeps
/// visible text stable while content is appended to or removed from that item. A
/// proportional pending scroll preserves the same fractional position within the item,
/// which is useful when the whole list is being resized and each item scales similarly.
#[derive(Clone)]
enum PendingScroll {
    /// Preserve the same pixel offset into the item after it is remeasured.
    Absolute { item_ix: usize, offset: Pixels },
    /// Preserve the same fractional offset into the item after it is remeasured.
    Proportional(PendingScrollFraction),
}

impl PendingScroll {
    fn item_ix(&self) -> usize {
        match self {
            PendingScroll::Absolute { item_ix, .. } => *item_ix,
            PendingScroll::Proportional(fraction) => fraction.item_ix,
        }
    }

    fn set_item_ix(&mut self, ix: usize) {
        match self {
            PendingScroll::Absolute { item_ix, .. } => *item_ix = ix,
            PendingScroll::Proportional(fraction) => fraction.item_ix = ix,
        }
    }
}

/// Keeps track of a fractional scroll position within an item for restoration
/// after remeasurement.
#[derive(Clone)]
struct PendingScrollFraction {
    /// The index of the item to scroll within.
    item_ix: usize,
    /// Fractional offset (0.0 to 1.0) within the item's height.
    fraction: f32,
}

/// Determines how remeasurement preserves the scroll position when the scroll-top item
/// changes height.
enum ScrollAnchor {
    /// Preserve the same pixel offset into the scroll-top item.
    Absolute,
    /// Preserve the same fractional position within the scroll-top item.
    Proportional,
}

/// Controls whether the list automatically follows new content at the end.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FollowMode {
    /// Normal scrolling — no automatic following.
    #[default]
    Normal,
    /// The list should auto-scroll along with the tail, when scrolled to bottom.
    Tail,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FollowState {
    #[default]
    Normal,
    Tail {
        is_following: bool,
    },
}

impl FollowState {
    fn is_following(&self) -> bool {
        matches!(self, FollowState::Tail { is_following: true })
    }

    fn has_stopped_following(&self) -> bool {
        matches!(
            self,
            FollowState::Tail {
                is_following: false
            }
        )
    }

    fn start_following(&mut self) {
        if let FollowState::Tail {
            is_following: false,
        } = self
        {
            *self = FollowState::Tail { is_following: true };
        }
    }

    fn stop_following(&mut self) {
        if let FollowState::Tail { is_following: true } = self {
            *self = FollowState::Tail {
                is_following: false,
            };
        }
    }
}

/// Whether the list is scrolling from top to bottom or bottom to top.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListAlignment {
    /// The list is scrolling from top to bottom, like most lists.
    Top,
    /// The list is scrolling from bottom to top, like a chat log.
    Bottom,
}

/// A scroll event that has been converted to be in terms of the list's items.
pub struct ListScrollEvent {
    /// The range of items currently visible in the list, after applying the scroll event.
    /// The start is inclusive and the end exclusive, in logical row indices.
    pub visible_range: Range<usize>,

    /// The number of items that are currently in the list's logical dataset.
    pub count: usize,

    /// Whether the list has been scrolled.
    pub is_scrolled: bool,

    /// Whether the list is currently in follow-tail mode (auto-scrolling to end).
    pub is_following_tail: bool,

    /// Whether the event comes from wheel or touch input (CMP-029 R2). Layout-triggered
    /// visible-range notifications and programmatic scrolling (`scroll_by`, `scroll_to`,
    /// follow motion) are never user input.
    pub user_initiated: bool,
}

/// The sizing behavior to apply during layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ListSizingBehavior {
    /// The list should calculate its size based on the size of its items.
    Infer,
    /// The list should not calculate a fixed size.
    #[default]
    Auto,
}

/// The measuring behavior to apply during layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ListMeasuringBehavior {
    /// Measure all items in the list.
    /// Note: This can be expensive for the first frame in a large list.
    Measure(bool),
    /// Only measure visible items
    #[default]
    Visible,
}

impl ListMeasuringBehavior {
    fn reset(&mut self) {
        match self {
            ListMeasuringBehavior::Measure(has_measured) => *has_measured = false,
            ListMeasuringBehavior::Visible => {}
        }
    }
}

/// The horizontal sizing behavior to apply during layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ListHorizontalSizingBehavior {
    /// List items' width can never exceed the width of the list.
    #[default]
    FitList,
    /// List items' width may go over the width of the list, if any item is wider.
    Unconstrained,
}

struct LayoutItemsResponse {
    max_item_width: Pixels,
    scroll_top: ListOffset,
    item_layouts: VecDeque<ItemLayout>,
    /// A newly focused row that is outside the rendered span and has to be scrolled into view.
    reveal_focus: Option<usize>,
}

struct ItemLayout {
    index: usize,
    element: AnyElement,
}

/// Frame state used by the [List] element after layout.
pub struct ListPrepaintState {
    hitbox: Hitbox,
    layout: LayoutItemsResponse,
}

#[derive(Clone)]
enum ListItem {
    Unmeasured {
        size_hint: Option<Size<Pixels>>,
        /// Height used while there is neither a measurement nor a hint (zero without an
        /// active uniform estimate).
        estimate: Pixels,
        focus_handle: Option<FocusHandle>,
    },
    Measured {
        size: Size<Pixels>,
        focus_handle: Option<FocusHandle>,
    },
}

impl ListItem {
    fn size(&self) -> Option<Size<Pixels>> {
        if let ListItem::Measured { size, .. } = self {
            Some(*size)
        } else {
            None
        }
    }

    fn size_hint(&self) -> Option<Size<Pixels>> {
        match self {
            ListItem::Measured { size, .. } => Some(*size),
            ListItem::Unmeasured { size_hint, .. } => *size_hint,
        }
    }

    /// The row height per ELM-001 R3: measurement, then hint, then estimate.
    fn height(&self) -> Pixels {
        match self {
            ListItem::Measured { size, .. } => size.height,
            ListItem::Unmeasured {
                size_hint,
                estimate,
                ..
            } => size_hint.map_or(*estimate, |size| size.height),
        }
    }

    fn focus_handle(&self) -> Option<FocusHandle> {
        match self {
            ListItem::Unmeasured { focus_handle, .. } | ListItem::Measured { focus_handle, .. } => {
                focus_handle.clone()
            }
        }
    }

    fn focus_id_if_focused(&self, window: &Window, cx: &App) -> Option<FocusId> {
        match self {
            ListItem::Unmeasured { focus_handle, .. } | ListItem::Measured { focus_handle, .. } => {
                focus_handle
                    .as_ref()
                    .filter(|handle| handle.contains_focused(window, cx))
                    .map(|handle| handle.id)
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ListItemSummary {
    count: usize,
    measured_count: usize,
    height: Pixels,
    measured_height: Pixels,
    has_focus_handles: bool,
    has_unknown_height: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Count(usize);

#[derive(Clone, Debug, Default)]
struct Height(Pixels);

/// Reservation bookkeeping for one layout pass (LAY-002 R3, R5).
struct PassReservation {
    /// Whether the pass reached the last row and computed an extra.
    computed: bool,
    /// The extra currently included in the pass's rendered height.
    applied: Pixels,
}

/// The current scroll position of a list together with its total content extent.
#[derive(Clone, Copy, Debug)]
pub struct ListScrollTop {
    /// The scroll anchor: item index and signed pixel offset within the item.
    pub offset: ListOffset,
    /// The total scrollable content extent in pixels (row heights per ELM-001 R3 plus vertical
    /// padding and any trailing reservation).
    pub content_height: Pixels,
}

impl ListState {
    /// Construct a new list state, for storage on a view.
    ///
    /// The overdraw parameter controls how much extra space is rendered
    /// above and below the visible area. Elements within this area will
    /// be measured even though they are not visible. This can help ensure
    /// that the list doesn't flicker or pop in when scrolling. Negative overdraw
    /// is clamped to zero.
    pub fn new(item_count: usize, alignment: ListAlignment, overdraw: Pixels) -> Self {
        let this = Self(Rc::new(RefCell::new(StateInner {
            last_layout_bounds: None,
            last_padding: None,
            last_content_width: None,
            items: SumTree::default(),
            logical_scroll_top: None,
            alignment,
            overdraw: overdraw.max(px(0.)),
            scroll_handler: None,
            last_visible_range: None,
            reset: false,
            scrollbar_drag_start_height: None,
            measuring_behavior: ListMeasuringBehavior::default(),
            pending_scroll: None,
            follow_state: FollowState::default(),
            estimate: None,
            configured_count: None,
            window_start: 0,
            unmounted_hints: BTreeMap::new(),
            reservation: None,
            reservation_extra: px(0.),
            last_focus_id: None,
        })));
        this.splice(0..0, item_count);
        this
    }

    /// Set the list to measure all items in the list in the first layout phase.
    ///
    /// This is useful for ensuring that the scrollbar size is correct instead of based on only rendered elements.
    pub fn measure_all(self) -> Self {
        self.0.borrow_mut().measuring_behavior = ListMeasuringBehavior::Measure(false);
        self
    }

    /// Pre-populate every unmeasured item with a uniform height hint so the scrollbar thumb
    /// is correctly sized from the first frame, without measuring all items up front.
    ///
    /// As items are actually rendered their real heights replace the hint, so the scrollbar
    /// converges to the exact size over time. This is a cheaper alternative to [`Self::measure_all`]
    /// for lists where items have roughly uniform heights (e.g. table rows). The hint survives
    /// the list's first layout.
    pub fn with_uniform_item_height(self, height: Pixels) -> Self {
        self.apply_uniform_item_height(height);
        self
    }

    /// Reset this instantiation of the list state.
    ///
    /// Note that this will cause scroll events to be dropped until the next paint.
    pub fn reset(&self, element_count: usize) {
        let old_count = {
            let state = &mut *self.0.borrow_mut();
            state.reset = true;
            state.measuring_behavior.reset();
            state.logical_scroll_top = None;
            state.pending_scroll = None;
            state.scrollbar_drag_start_height = None;
            state.unmounted_hints.clear();
            state.items.summary().count
        };

        self.splice(0..old_count, element_count);
    }

    /// Reset the list to `element_count` items, pre-populating every item with a
    /// uniform height hint so the scrollbar thumb is correctly sized from the first
    /// frame even for off-screen items.
    pub fn reset_with_uniform_height(&self, element_count: usize, height: Pixels) {
        self.reset(element_count);
        self.apply_uniform_item_height(height);
    }

    fn apply_uniform_item_height(&self, height: Pixels) {
        let mut state = self.0.borrow_mut();
        let size_hint = Size {
            width: px(0.),
            height,
        };
        if !state.items.iter().any(|item| item.size_hint().is_none()) {
            return;
        }
        let new_items = state
            .items
            .iter()
            .map(|item| match item {
                ListItem::Unmeasured {
                    size_hint: None,
                    estimate,
                    focus_handle,
                } => ListItem::Unmeasured {
                    size_hint: Some(size_hint),
                    estimate: *estimate,
                    focus_handle: focus_handle.clone(),
                },
                other => other.clone(),
            })
            .collect::<Vec<_>>();
        state.items = SumTree::from_iter(new_items, ());
    }

    /// Remeasure all items while preserving proportional scroll position.
    ///
    /// Use this when item heights may have changed (e.g., font size changes)
    /// but the number and identity of items remains the same.
    pub fn remeasure(&self) {
        let count = self.0.borrow().items.summary().count;
        self.remeasure_items_with_scroll_anchor(0..count, ScrollAnchor::Proportional);
    }

    /// Mark items in `range` (mounted-row indices) as needing remeasurement while preserving
    /// the current scroll position. Unlike [`Self::splice`], this does
    /// not change the number of items or blow away `logical_scroll_top`.
    ///
    /// Use this when an item's content has changed and its rendered
    /// height may be different (e.g., streaming text, tool results
    /// loading), but the item itself still exists at the same index.
    /// Remeasured rows keep their previous height as a hint until they are laid out again.
    pub fn remeasure_items(&self, range: Range<usize>) {
        self.remeasure_items_with_scroll_anchor(range, ScrollAnchor::Absolute);
    }

    fn remeasure_items_with_scroll_anchor(&self, range: Range<usize>, scroll_anchor: ScrollAnchor) {
        let state = &mut *self.0.borrow_mut();
        let window_start = state.mounted_range().start;
        let logical_range = window_start + range.start..window_start + range.end;

        if let Some(scroll_top) = state.logical_scroll_top {
            if logical_range.contains(&scroll_top.item_ix) {
                state.pending_scroll = match scroll_anchor {
                    ScrollAnchor::Absolute => Some(PendingScroll::Absolute {
                        item_ix: scroll_top.item_ix,
                        offset: scroll_top.offset_in_item,
                    }),
                    // A negative offset places the item below the viewport top; it is not a
                    // position within the item, so keep it as a pixel offset.
                    ScrollAnchor::Proportional if scroll_top.offset_in_item < px(0.) => {
                        Some(PendingScroll::Absolute {
                            item_ix: scroll_top.item_ix,
                            offset: scroll_top.offset_in_item,
                        })
                    }
                    ScrollAnchor::Proportional => {
                        // If the scroll-top item falls within the remeasured range,
                        // store a fractional offset so the layout can restore the
                        // proportional scroll position after the item is re-rendered
                        // at its new height.
                        state
                            .mounted_item(scroll_top.item_ix)
                            .and_then(|item| {
                                item.size().map(|size| {
                                    let fraction = if size.height.0 > 0.0 {
                                        (scroll_top.offset_in_item.0 / size.height.0)
                                            .clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };

                                    PendingScroll::Proportional(PendingScrollFraction {
                                        item_ix: scroll_top.item_ix,
                                        fraction,
                                    })
                                })
                            })
                            .or_else(|| state.pending_scroll.clone())
                    }
                };
            }
        }

        // Rebuild the tree, replacing items in the range with
        // Unmeasured copies that keep their focus handles and their old height as a hint.
        let estimate = state.estimate_px();
        let new_items = {
            let mut cursor = state.items.cursor::<Count>(());
            let mut new_items = cursor.slice(&Count(range.start), Bias::Right);
            let invalidated = cursor.slice(&Count(range.end), Bias::Right);
            new_items.extend(
                invalidated.iter().map(|item| ListItem::Unmeasured {
                    size_hint: item.size_hint(),
                    estimate,
                    focus_handle: item.focus_handle(),
                }),
                (),
            );
            new_items.append(cursor.suffix(), ());
            new_items
        };
        state.items = new_items;
        state.measuring_behavior.reset();
    }

    /// The number of rows in the list's logical dataset: the configured logical item count in
    /// windowed mode, otherwise the number of mounted rows.
    pub fn item_count(&self) -> usize {
        self.0.borrow().logical_count()
    }

    /// The number of rows that are currently mounted (supplied through splices).
    pub fn mounted_item_count(&self) -> usize {
        self.0.borrow().items.summary().count
    }

    /// Whether the list is scrolled to the end, or `None` if the list is
    /// not scrollable or the total content height is not yet known.
    pub fn is_scrolled_to_end(&self) -> Option<bool> {
        let state = self.0.borrow();
        let bounds = state.last_layout_bounds?;
        if state.has_unknown_height() {
            return None;
        }
        let padding = state.last_padding.unwrap_or_default();
        let content_height = state.total_height() + padding.top + padding.bottom;
        let scroll_max = (content_height - bounds.size.height).max(px(0.));
        if scroll_max <= px(0.) {
            return None;
        }
        let scroll_top = state.scroll_px(&state.logical_scroll_top());
        Some(scroll_top >= scroll_max)
    }

    /// Inform the list state that the items in `old_range` (mounted-row indices) have been
    /// replaced by `count` new items that must be recalculated.
    pub fn splice(&self, old_range: Range<usize>, count: usize) {
        self.splice_focusable(old_range, (0..count).map(|_| None))
    }

    /// Register with the list state that the items in `old_range` have been replaced
    /// by new items. As opposed to [`Self::splice`], this method allows an iterator of optional focus handles
    /// to be supplied to properly integrate with items in the list that can be focused. If a focused item
    /// is scrolled out of view, the list will continue to render it to allow keyboard interaction.
    ///
    /// Rows outside the range keep their measurements, hints, focus handles and identity; rows
    /// in the range become unmeasured without hints. The anchor follows ELM-001 R11 and R12.
    pub fn splice_focusable(
        &self,
        old_range: Range<usize>,
        focus_handles: impl IntoIterator<Item = Option<FocusHandle>>,
    ) {
        let state = &mut *self.0.borrow_mut();
        state.strip_reservation_extra();
        let old_mounted_len = state.items.summary().count;
        let estimate = state.estimate_px();

        let mut old_items = state.items.cursor::<Count>(());
        let mut new_items = old_items.slice(&Count(old_range.start), Bias::Right);
        old_items.seek_forward(&Count(old_range.end), Bias::Right);

        let mut spliced_count = 0;
        new_items.extend(
            focus_handles.into_iter().map(|focus_handle| {
                spliced_count += 1;
                ListItem::Unmeasured {
                    size_hint: None,
                    estimate,
                    focus_handle,
                }
            }),
            (),
        );
        new_items.append(old_items.suffix(), ());
        drop(old_items);
        state.items = new_items;

        state.adjust_anchor_after_splice(old_range, spliced_count, old_mounted_len);
    }

    /// Replace the ordered sequence of mounted rows identified by `old_keys` with the sequence
    /// identified by `new_keys` (insert, delete, filter, reorder, asynchronous data changes).
    ///
    /// Rows whose keys survive keep their measured heights, hints and focus handles; new keys
    /// are unmeasured without hints. The scroll anchor is captured as (key of the anchor row,
    /// offset) and restored at the row's new index; when the anchor key no longer exists the
    /// surviving row that now occupies the index at which the anchor row stood (clamped to the
    /// last row) becomes the anchor with offset zero, and an empty result has no anchor. In
    /// windowed mode the logical dataset is fixed, so the logical anchor is kept as is.
    /// Identical sequences leave the state untouched.
    pub fn reconcile_keys<K: Eq + Hash>(&self, old_keys: &[K], new_keys: &[K]) {
        self.reconcile_keys_with(old_keys, new_keys, None, |_| None);
    }

    /// Like [`Self::reconcile_keys`] for a windowed list whose window moves to `window_start` at
    /// the same time: the surviving keys keep their records, rows that leave the window keep
    /// their height as a hint for the placeholders and the logical anchor is preserved.
    pub fn reconcile_keys_in_window<K: Eq + Hash>(
        &self,
        window_start: usize,
        old_keys: &[K],
        new_keys: &[K],
    ) {
        self.reconcile_keys_with(old_keys, new_keys, Some(window_start), |_| None);
    }

    /// The general form of [`Self::reconcile_keys`]: an optional new window start and a function
    /// supplying the focus handle (if any) of each newly inserted row by its index in
    /// `new_keys`.
    pub fn reconcile_keys_with<K: Eq + Hash>(
        &self,
        old_keys: &[K],
        new_keys: &[K],
        window_start: Option<usize>,
        new_focus_handle: impl FnMut(usize) -> Option<FocusHandle>,
    ) {
        let state = &mut *self.0.borrow_mut();
        let window_start_before = state.mounted_range().start;
        if old_keys == new_keys && window_start.is_none_or(|start| start == window_start_before) {
            return;
        }
        let new_start = match window_start {
            Some(start) if state.is_windowed() => start.min(state.logical_count()),
            _ => window_start_before,
        };
        let window_start = window_start_before;
        let old_mounted_len = state.items.summary().count;
        let anchor = state.logical_scroll_top;

        let mut lookup: HashMap<&K, usize> = HashMap::with_capacity(old_keys.len());
        for (ix, key) in old_keys.iter().enumerate().take(old_mounted_len) {
            lookup.entry(key).or_insert(ix);
        }
        let sources: Vec<Option<usize>> = new_keys.iter().map(|key| lookup.remove(key)).collect();

        // Where the anchor row stood among the previously mounted rows.
        let anchor_old_ix = anchor.and_then(|top| {
            let relative = top.item_ix.checked_sub(window_start)?;
            (relative < old_keys.len().min(old_mounted_len)).then_some(relative)
        });
        let old_len = old_keys.len().min(old_mounted_len);
        let anchored_to_end = anchor.is_some_and(|top| top.item_ix >= window_start + old_len);

        state.remount(new_start, &sources, new_focus_handle);

        if state.is_windowed() {
            return;
        }
        let Some(top) = anchor else { return };
        let new_len = new_keys.len();
        let mut pending = state.pending_scroll.take();
        let new_anchor = if anchored_to_end {
            Some(ListOffset {
                item_ix: new_len,
                offset_in_item: top.offset_in_item,
            })
        } else if let Some(old_ix) = anchor_old_ix {
            match sources.iter().position(|source| *source == Some(old_ix)) {
                Some(new_ix) => {
                    if let Some(pending) = pending.as_mut() {
                        if pending.item_ix() == top.item_ix {
                            pending.set_item_ix(new_ix);
                        }
                    }
                    Some(ListOffset {
                        item_ix: new_ix,
                        offset_in_item: top.offset_in_item,
                    })
                }
                None => {
                    pending = None;
                    (new_len > 0).then(|| ListOffset {
                        item_ix: old_ix.min(new_len - 1),
                        offset_in_item: px(0.),
                    })
                }
            }
        } else {
            Some(top)
        };
        state.pending_scroll = pending
            .filter(|pending| new_anchor.is_some_and(|anchor| pending.item_ix() == anchor.item_ix));
        state.logical_scroll_top = new_anchor;
    }

    /// Set a handler that will be called when the list is scrolled and whenever the visible
    /// logical range changes after a layout (inclusive start, exclusive end).
    ///
    /// Handlers triggered by a layout run after the frame's draw phase.
    pub fn set_scroll_handler(
        &self,
        handler: impl FnMut(&ListScrollEvent, &mut Window, &mut App) + 'static,
    ) {
        self.0.borrow_mut().scroll_handler = Some(Rc::new(RefCell::new(handler)));
    }

    /// Get the current scroll offset, in terms of the list's items.
    pub fn logical_scroll_top(&self) -> ListOffset {
        self.0.borrow().logical_scroll_top()
    }

    /// Get the current scroll offset together with the total content extent in pixels.
    pub fn logical_scroll_top_and_extent(&self) -> ListScrollTop {
        let state = self.0.borrow();
        let padding = state.last_padding.unwrap_or_default();
        ListScrollTop {
            offset: state.logical_scroll_top(),
            content_height: state.total_height() + padding.top + padding.bottom,
        }
    }

    /// Scroll the list by the given offset
    pub fn scroll_by(&self, distance: Pixels) {
        if distance == px(0.) {
            return;
        }

        let current_offset = self.logical_scroll_top();
        let state = &mut *self.0.borrow_mut();

        if distance < px(0.) {
            state.follow_state.stop_following();
        }

        // The old viewport coordinate is the start of the anchor row plus the signed offset;
        // the target may precede the anchor row when that offset is negative, so it is
        // always resolved with a search from the beginning of the sequence.
        let start_pixel_offset = state.scroll_px(&current_offset);
        let new_pixel_offset = (start_pixel_offset + distance).max(px(0.));
        let (item_ix, item_top) = state.locate(new_pixel_offset);

        let scroll_top = ListOffset {
            item_ix,
            offset_in_item: new_pixel_offset - item_top,
        };
        state.rebase_pending_scroll(scroll_top);
        state.logical_scroll_top = Some(scroll_top);
    }

    /// Scroll the list to the very end (past the last item).
    ///
    /// Unlike [`scroll_to_reveal_item`], this uses the total item count as the
    /// anchor, so the list's layout pass will walk backwards from the end and
    /// always show the bottom of the last item — even when that item is still
    /// growing (e.g. during streaming).
    pub fn scroll_to_end(&self) {
        let state = &mut *self.0.borrow_mut();
        let item_count = state.logical_count();
        state.pending_scroll = None;
        state.logical_scroll_top = Some(ListOffset {
            item_ix: item_count,
            offset_in_item: px(0.),
        });
    }

    /// Set the follow mode for the list. In `Tail` mode, the list
    /// will auto-scroll to the end and re-engage after the user
    /// scrolls back to the bottom. In `Normal` mode, no automatic
    /// following occurs.
    pub fn set_follow_mode(&self, mode: FollowMode) {
        let state = &mut *self.0.borrow_mut();

        match mode {
            FollowMode::Normal => {
                state.follow_state = FollowState::Normal;
            }
            FollowMode::Tail => {
                state.follow_state = FollowState::Tail { is_following: true };
                let item_count = state.logical_count();
                state.logical_scroll_top = Some(ListOffset {
                    item_ix: item_count,
                    offset_in_item: px(0.),
                });
            }
        }
    }

    /// Enable or disable tail following. Enabling while already in tail mode (following or
    /// paused) and disabling while not in tail mode are no-ops, so a rerender with identical
    /// configuration keeps the follow state.
    pub fn set_follow_tail(&self, follow: bool) {
        let in_tail_mode = matches!(self.0.borrow().follow_state, FollowState::Tail { .. });
        if follow != in_tail_mode {
            self.set_follow_mode(if follow {
                FollowMode::Tail
            } else {
                FollowMode::Normal
            });
        }
    }

    /// Pause tail-following, freezing the list at its current scroll
    /// position. Unlike [`Self::set_follow_mode`] with [`FollowMode::Normal`],
    /// this keeps the list in `Tail` mode, so it will resume following
    /// automatically once the view returns to the bottom. No-op when the list
    /// isn't currently following.
    ///
    /// Useful when something other than the user grows an item (e.g. zooming a
    /// diagram) and the current position should stay put rather than snapping
    /// to the end.
    pub fn pause_following_tail(&self) {
        self.0.borrow_mut().follow_state.stop_following();
    }

    /// Returns whether the list is currently actively following the
    /// tail (snapping to the end on each layout).
    pub fn is_following_tail(&self) -> bool {
        matches!(
            self.0.borrow().follow_state,
            FollowState::Tail { is_following: true }
        )
    }

    /// Scroll the list to the given anchor: a logical row index and a signed pixel offset. A
    /// negative offset places the row below the viewport top and is resolved during layout.
    /// An index at or beyond the row count normalizes to the end of the list with offset zero.
    pub fn scroll_to(&self, mut scroll_top: ListOffset) {
        let state = &mut *self.0.borrow_mut();
        let item_count = state.logical_count();
        if scroll_top.item_ix >= item_count {
            scroll_top.item_ix = item_count;
            scroll_top.offset_in_item = px(0.);
        }

        if scroll_top.item_ix < item_count {
            state.follow_state.stop_following();
        }

        state.rebase_pending_scroll(scroll_top);
        state.logical_scroll_top = Some(scroll_top);
    }

    /// Scroll the list to the given item, such that the item is fully visible, moving as
    /// little as possible.
    pub fn scroll_to_reveal_item(&self, ix: usize) {
        let state = &mut *self.0.borrow_mut();
        if state.logical_count() == 0 {
            return;
        }
        let height = state
            .last_layout_bounds
            .map_or(px(0.), |bounds| bounds.size.height);
        let padding = state.last_padding.unwrap_or_default();
        let scroll_top = state.reveal_offset(ix, height, &padding);

        state.rebase_pending_scroll(scroll_top);
        state.logical_scroll_top = Some(scroll_top);
    }

    /// Get the bounds for the given item in window coordinates, if its height is known
    /// (it has been measured and lies at or after the scroll anchor). Does not move the list.
    pub fn bounds_for_item(&self, ix: usize) -> Option<Bounds<Pixels>> {
        let state = &*self.0.borrow();

        let bounds = state.last_layout_bounds.unwrap_or_default();
        let padding = state.last_padding.unwrap_or_default();
        let scroll_top = state.logical_scroll_top();
        if ix < scroll_top.item_ix {
            return None;
        }

        let size = state.mounted_item(ix)?.size()?;
        let top = bounds.top() + padding.top + state.row_top(ix) - state.scroll_px(&scroll_top);
        let left = bounds.left() + padding.left;
        Some(Bounds::from_corners(
            point(left, top),
            point(left + size.width.max(px(0.)), top + size.height),
        ))
    }

    /// Call this method when the user starts dragging the scrollbar.
    ///
    /// This will prevent the height reported to the scrollbar from changing during the drag
    /// as items in the overdraw get measured, and help offset scroll position changes accordingly.
    pub fn scrollbar_drag_started(&self) {
        let mut state = self.0.borrow_mut();
        state.scrollbar_drag_start_height = Some(state.total_height());
    }

    /// Called when the user stops dragging the scrollbar.
    ///
    /// See `scrollbar_drag_started`.
    pub fn scrollbar_drag_ended(&self) {
        self.0.borrow_mut().scrollbar_drag_start_height.take();
    }

    /// Returns `true` if the scrollbar is currently being dragged.
    ///
    /// This is set between [`scrollbar_drag_started`](Self::scrollbar_drag_started)
    /// and [`scrollbar_drag_ended`](Self::scrollbar_drag_ended) calls. Useful for
    /// consumers that need to distinguish scrollbar drags from wheel/trackpad scrolls,
    /// e.g. to suppress auto-scroll behavior during manual positioning.
    pub fn is_scrollbar_dragging(&self) -> bool {
        self.0.borrow().scrollbar_drag_start_height.is_some()
    }

    /// Set the offset from the scrollbar
    pub fn set_offset_from_scrollbar(&self, point: Point<Pixels>) {
        self.0.borrow_mut().set_offset_from_scrollbar(point);
    }

    /// Returns the maximum scroll offset according to the items we have measured.
    /// This value remains constant while dragging to prevent the scrollbar from moving away unexpectedly.
    pub fn max_offset_for_scrollbar(&self) -> Point<Pixels> {
        let state = self.0.borrow();
        point(Pixels::ZERO, state.max_scroll_offset())
    }

    /// Returns the current scroll offset adjusted for the scrollbar.
    ///
    /// The returned offset has a negative `y` component representing
    /// how far the content has scrolled.
    pub fn scroll_px_offset_for_scrollbar(&self) -> Point<Pixels> {
        let state = &self.0.borrow();

        if state.logical_scroll_top.is_none() && state.alignment == ListAlignment::Bottom {
            return Point::new(px(0.), -state.max_scroll_offset());
        }

        Point::new(px(0.), -state.scroll_px(&state.logical_scroll_top()))
    }

    /// Measured-average scroll extents for scrollbars (ELM-001 R17): the maximum scroll distance
    /// and the current scroll distance, both nonnegative, estimated from measured rows only
    /// (unmeasured rows, placeholders and hint-only rows are valued at the average measured
    /// height). `None` until at least one row is measured. Never measures or moves anything.
    pub fn measured_average_scroll_extents(&self) -> Option<(Pixels, Pixels)> {
        let state = self.0.borrow();
        let mounted = state.mounted_range();
        let measured = state.tree_prefix(mounted.len());
        if measured.measured_count == 0 {
            return None;
        }
        let padding = state.last_padding.unwrap_or_default();
        let viewport = state.last_layout_bounds.map_or(px(0.), |b| b.size.height);
        let count = state.logical_count();
        let average = measured.measured_height / measured.measured_count as f32;
        let unmeasured = count - measured.measured_count;
        let estimated_total = measured.measured_height + average * unmeasured as f32;
        let max = (estimated_total + padding.top + padding.bottom - viewport).max(px(0.));

        if state.logical_scroll_top.is_none() && state.alignment == ListAlignment::Bottom {
            return Some((max, max));
        }
        let anchor = state.logical_scroll_top();
        let anchor_ix = anchor.item_ix.min(count);
        let before = state.tree_prefix(anchor_ix.saturating_sub(mounted.start).min(mounted.len()));
        let unmeasured_before = anchor_ix - before.measured_count;
        let current = before.measured_height
            + average * unmeasured_before as f32
            + anchor.offset_in_item.max(px(0.))
            + padding.top;
        Some((max, current.min(max)))
    }

    /// The average height of the rows measured so far, or `None` before any row is measured.
    /// Hosts feed it back through [`Self::set_estimated_item_height`] so unmeasured rows are
    /// valued sensibly while scrolling far through a long list.
    pub fn average_measured_height(&self) -> Option<Pixels> {
        let state = self.0.borrow();
        let mounted = state.mounted_range();
        let measured = state.tree_prefix(mounted.len());
        (measured.measured_count > 0)
            .then(|| measured.measured_height / measured.measured_count as f32)
    }

    /// The scroll extents a scrollbar reports for this list (CMP-031 R2): the maximum scroll
    /// distance and the current scroll distance, both nonnegative.
    ///
    /// Unseen rows are valued at the average measured row height; before any row is measured
    /// the list's own totals stand. While a scrollbar drag is in progress
    /// ([`Self::scrollbar_drag_started`]) the content height stays constant, so the thumb does
    /// not run away as new rows get measured.
    pub fn scrollbar_extents(&self) -> (Pixels, Pixels) {
        if !self.is_scrollbar_dragging()
            && let Some(extents) = self.measured_average_scroll_extents()
        {
            return extents;
        }
        let max = self.max_offset_for_scrollbar().y.max(px(0.));
        let position = (-self.scroll_px_offset_for_scrollbar().y).clamp(px(0.), max);
        (max, position)
    }

    /// Return the bounds of the viewport in pixels.
    pub fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().last_layout_bounds.unwrap_or_default()
    }

    /// Returns whether the item is entirely above the viewport, or `None` if
    /// the list has not measured enough layout to know.
    ///
    /// A zero-height viewport still yields a definitive answer: callers may
    /// size sibling UI based on this query (potentially squeezing the list
    /// itself to zero height), so returning `None` in that case would make
    /// the answer oscillate from frame to frame.
    pub fn item_is_above_viewport(&self, ix: usize) -> Option<bool> {
        let viewport_bounds = self.0.borrow().last_layout_bounds?;

        let scroll_top = self.logical_scroll_top();
        if ix < scroll_top.item_ix {
            // Rows before the logical scroll top have no item bounds, but
            // their position relative to the viewport is known from scroll state.
            return Some(true);
        }

        let item_bounds = self.bounds_for_item(ix)?;
        Some(item_bounds.bottom() <= viewport_bounds.top())
    }

    /// Returns whether the item is entirely below the viewport, or `None` if
    /// the list has not measured enough layout to know.
    ///
    /// See [`Self::item_is_above_viewport`] for why a zero-height viewport
    /// still yields a definitive answer.
    pub fn item_is_below_viewport(&self, ix: usize) -> Option<bool> {
        let viewport_bounds = self.0.borrow().last_layout_bounds?;

        let scroll_top = self.logical_scroll_top();
        if ix < scroll_top.item_ix {
            // Rows before the logical scroll top have no item bounds, but
            // their position relative to the viewport is known from scroll state.
            return Some(false);
        }

        let item_bounds = self.bounds_for_item(ix)?;
        Some(item_bounds.top() >= viewport_bounds.bottom())
    }

    // Configuration. Every setter is a no-op when given the current value and none of them moves
    // the logical scroll anchor (except entering tail following, see `set_follow_tail`).

    /// The list's alignment.
    pub fn alignment(&self) -> ListAlignment {
        self.0.borrow().alignment
    }

    /// Change the alignment, preserving the logical scroll anchor.
    pub fn set_alignment(&self, alignment: ListAlignment) {
        let state = &mut *self.0.borrow_mut();
        if state.alignment == alignment {
            return;
        }
        // An absent anchor means "top" or "end" depending on the alignment; make it explicit so
        // the viewport keeps showing the same position.
        state.logical_scroll_top = Some(state.logical_scroll_top());
        state.alignment = alignment;
    }

    /// The overdraw distance.
    pub fn overdraw(&self) -> Pixels {
        self.0.borrow().overdraw
    }

    /// Change the overdraw distance (clamped to at least zero).
    pub fn set_overdraw(&self, overdraw: Pixels) {
        let overdraw = overdraw.max(px(0.));
        self.0.borrow_mut().overdraw = overdraw;
    }

    /// Set the uniform row height estimate. Only a strictly positive finite value activates it;
    /// zero, negative, NaN and infinite values (and `None`) clear it. Rows that have neither a
    /// measurement nor a hint use the estimate.
    pub fn set_estimated_item_height(&self, estimate: Option<Pixels>) {
        let estimate = estimate.filter(|height| height.0.is_finite() && height.0 > 0.);
        let state = &mut *self.0.borrow_mut();
        if state.estimate == estimate {
            return;
        }
        state.estimate = estimate;
        let estimate = state.estimate_px();
        let new_items = state.items.iter().map(|item| match item {
            ListItem::Unmeasured {
                size_hint,
                focus_handle,
                ..
            } => ListItem::Unmeasured {
                size_hint: *size_hint,
                estimate,
                focus_handle: focus_handle.clone(),
            },
            other => other.clone(),
        });
        state.items = SumTree::from_iter(new_items.collect::<Vec<_>>(), ());
    }

    /// Configure the logical item count. A nonnegative finite number is truncated to an
    /// integer; negative, NaN, infinite values and `None` mean "not configured". The count only
    /// takes effect together with a positive [estimate](Self::set_estimated_item_height)
    /// (windowed mode); otherwise the logical count is the number of mounted rows.
    pub fn set_logical_item_count(&self, count: Option<f64>) {
        let count = count
            .filter(|count| count.is_finite() && *count >= 0.)
            .map(|count| count as usize);
        self.0.borrow_mut().configured_count = count;
    }

    /// Whether the list is in windowed mode (see [`Self::set_logical_item_count`]).
    pub fn is_windowed(&self) -> bool {
        self.0.borrow().is_windowed()
    }

    /// The logical index of the first mounted row, clamped to the logical row count. Zero when
    /// the list is not windowed.
    pub fn window_start(&self) -> usize {
        self.0.borrow().mounted_range().start
    }

    /// Set the logical index of the first mounted row without touching the mounted rows: row
    /// `i` of the mounted rows is reinterpreted as logical row `start + i`, and the logical
    /// scroll anchor is preserved. Use this to declare where freshly spliced rows start; when
    /// the mounted rows change at the same time use [`Self::reconcile_keys_in_window`] or
    /// [`Self::slide_window`], which keep the measurements of the rows that stay mounted.
    pub fn set_window_start(&self, start: usize) {
        self.0.borrow_mut().window_start = start;
    }

    /// Slide the mounted window to `count` rows starting at logical row `start`, preserving the
    /// logical scroll anchor. Rows that stay mounted keep their measurements, hints and focus
    /// handles; rows entering the window are unmeasured (using a retained hint if the row
    /// was measured before it left the window) and rows leaving it retain their height as a
    /// hint for the scroll geometry of placeholders.
    pub fn slide_window(&self, start: usize, count: usize) {
        let state = &mut *self.0.borrow_mut();
        let old = state.mounted_range();
        let new_start = if state.is_windowed() {
            start.min(state.logical_count())
        } else {
            0
        };
        let sources: Vec<Option<usize>> = (new_start..new_start + count)
            .map(|logical| old.contains(&logical).then(|| logical - old.start))
            .collect();
        state.remount(new_start, &sources, |_| None);
    }

    // Trailing viewport reservation (LAY-002).

    /// Reserve the trailing viewport: rows from the zero-based `start` index on are padded, in
    /// the last row's layout box, until their combined height reaches the viewport height minus
    /// the `inset` (a negative inset reserves more than a viewport). `None` removes the
    /// reservation. Setting an equal reservation is a no-op; a changed one removes the extra
    /// height applied so far (the last row is remeasured by the next layout).
    pub fn set_trailing_reservation(&self, reservation: Option<(usize, Pixels)>) {
        let reservation = reservation.map(|(start, inset)| TrailingReservation { start, inset });
        let state = &mut *self.0.borrow_mut();
        if state.reservation == reservation {
            return;
        }
        state.strip_reservation_extra();
        state.reservation = reservation;
    }

    /// The cumulative estimated height of all rows before `ix` (measured heights, then retained
    /// hints; rows with neither contribute zero unless a uniform estimate is active), including
    /// the applied reservation extra when `ix` lies after the last row. An index beyond the row
    /// count reports the total. Does not move the viewport.
    pub fn item_offset(&self, ix: usize) -> Pixels {
        self.0.borrow().row_top(ix)
    }

    /// Whether the natural height of the reserved suffix reaches the reservation's minimum
    /// height (computed from the last laid-out viewport height). `false` without a reservation.
    pub fn trailing_reservation_filled(&self) -> bool {
        let state = self.0.borrow();
        let Some(reservation) = state.reservation else {
            return false;
        };
        let viewport = state.last_layout_bounds.map_or(px(0.), |b| b.size.height);
        let minimum = (viewport - reservation.inset).max(px(0.));
        let natural =
            state.total_height() - state.reservation_extra - state.row_top(reservation.start);
        natural >= minimum
    }
}

impl StateInner {
    // Geometry over the logical dataset.

    fn estimate_px(&self) -> Pixels {
        self.estimate.unwrap_or(Pixels::ZERO)
    }

    fn is_windowed(&self) -> bool {
        self.estimate.is_some() && self.configured_count.is_some()
    }

    /// The number of rows `N` in the logical dataset.
    fn logical_count(&self) -> usize {
        match (self.estimate, self.configured_count) {
            (Some(_), Some(count)) => count,
            _ => self.items.summary().count,
        }
    }

    /// The logical indices of the mounted rows that take part in geometry and painting.
    fn mounted_range(&self) -> Range<usize> {
        let mounted = self.items.summary().count;
        match (self.estimate, self.configured_count) {
            (Some(_), Some(count)) => {
                let start = self.window_start.min(count);
                start..start + mounted.min(count - start)
            }
            _ => 0..mounted,
        }
    }

    /// The mounted row with the given logical index.
    fn mounted_item(&self, ix: usize) -> Option<ListItem> {
        let range = self.mounted_range();
        if !range.contains(&ix) {
            return None;
        }
        self.items
            .find::<ListItemSummary, _>((), &Count(ix - range.start), Bias::Right)
            .2
            .cloned()
    }

    /// The summary of the first `count` mounted rows.
    fn tree_prefix(&self, count: usize) -> ListItemSummary {
        let mut cursor = self.items.cursor::<ListItemSummary>(());
        cursor.summary(&Count(count), Bias::Right)
    }

    fn has_unknown_height(&self) -> bool {
        let mounted = self.mounted_range();
        if mounted.len() == self.items.summary().count {
            self.items.summary().has_unknown_height
        } else {
            self.tree_prefix(mounted.len()).has_unknown_height
        }
    }

    fn placeholder_base(&self) -> Pixels {
        self.estimate_px().max(PLACEHOLDER_MIN_HEIGHT)
    }

    fn placeholder_height(&self, ix: usize) -> Pixels {
        if ix >= self.logical_count() {
            return px(0.);
        }
        self.unmounted_hints
            .get(&ix)
            .copied()
            .unwrap_or_else(|| self.placeholder_base())
    }

    /// The summed height of the placeholder rows in `range`.
    fn placeholder_span(&self, range: Range<usize>) -> Pixels {
        if range.is_empty() {
            return px(0.);
        }
        let base = self.placeholder_base();
        let mut total = base * range.len() as f32;
        for hint in self.unmounted_hints.range(range).map(|(_, hint)| *hint) {
            total += hint - base;
        }
        total
    }

    /// The cumulative height of all rows before `ix` (R3 heights); an index beyond the row
    /// count reports the total.
    fn row_top(&self, ix: usize) -> Pixels {
        let ix = ix.min(self.logical_count());
        if !self.is_windowed() {
            return self.tree_prefix(ix).height;
        }
        let mounted = self.mounted_range();
        if ix <= mounted.start {
            self.placeholder_span(0..ix)
        } else if ix <= mounted.end {
            self.placeholder_span(0..mounted.start) + self.tree_prefix(ix - mounted.start).height
        } else {
            self.placeholder_span(0..mounted.start)
                + self.tree_prefix(mounted.len()).height
                + self.placeholder_span(mounted.end..ix)
        }
    }

    fn total_height(&self) -> Pixels {
        self.row_top(usize::MAX)
    }

    /// The row containing the content coordinate `y` and that row's top (right-biased at row
    /// boundaries). Coordinates at or beyond the end resolve to the row count and the total.
    fn locate(&self, y: Pixels) -> (usize, Pixels) {
        let y = y.max(px(0.));
        if !self.is_windowed() {
            let (start, ..) = self
                .items
                .find::<ListItemSummary, _>((), &Height(y), Bias::Right);
            return (start.count, start.height);
        }
        let mounted = self.mounted_range();
        let before = self.placeholder_span(0..mounted.start);
        if y < before {
            return self.locate_placeholder(0..mounted.start, px(0.), y);
        }
        let mounted_height = self.tree_prefix(mounted.len()).height;
        if y < before + mounted_height {
            let (start, ..) =
                self.items
                    .find::<ListItemSummary, _>((), &Height(y - before), Bias::Right);
            return (mounted.start + start.count, before + start.height);
        }
        self.locate_placeholder(
            mounted.end..self.logical_count(),
            before + mounted_height,
            y,
        )
    }

    /// Locate `y` among the placeholder rows of `range`, whose first row starts at `top`.
    fn locate_placeholder(
        &self,
        range: Range<usize>,
        mut top: Pixels,
        y: Pixels,
    ) -> (usize, Pixels) {
        let base = self.placeholder_base();
        let mut ix = range.start;
        let in_run = |ix: usize, len: usize, top: Pixels, y: Pixels| {
            let skipped = (((y - top) / base).floor().max(0.) as usize).min(len - 1);
            (ix + skipped, top + base * skipped as f32)
        };
        for (&hint_ix, &hint) in self.unmounted_hints.range(range.clone()) {
            let run = hint_ix - ix;
            if run > 0 {
                if y < top + base * run as f32 {
                    return in_run(ix, run, top, y);
                }
                top += base * run as f32;
            }
            if y < top + hint {
                return (hint_ix, top);
            }
            top += hint;
            ix = hint_ix + 1;
        }
        let run = range.end.saturating_sub(ix);
        if run > 0 && y < top + base * run as f32 {
            return in_run(ix, run, top, y);
        }
        (range.end, top + base * run as f32)
    }

    /// The content coordinate of the viewport top for the given anchor.
    fn scroll_px(&self, scroll_top: &ListOffset) -> Pixels {
        self.row_top(scroll_top.item_ix) + scroll_top.offset_in_item
    }

    // Reservation (LAY-002).

    /// Remove the applied reservation extra from the last row: its retained size hint loses the
    /// extra (clamped to zero) and its measurement is invalidated, keeping its focus handle.
    fn strip_reservation_extra(&mut self) {
        let extra = std::mem::replace(&mut self.reservation_extra, px(0.));
        let count = self.logical_count();
        let mounted = self.mounted_range();
        if extra <= px(0.) || count == 0 || mounted.end != count || mounted.is_empty() {
            return;
        }
        let Some(last) = self.mounted_item(count - 1) else {
            return;
        };
        let Some(hint) = last.size_hint() else {
            return;
        };
        let estimate = self.estimate_px();
        let stripped = ListItem::Unmeasured {
            size_hint: Some(size(hint.width, (hint.height - extra).max(px(0.)))),
            estimate,
            focus_handle: last.focus_handle(),
        };
        self.replace_mounted_item(mounted.len() - 1, stripped);
    }

    fn replace_mounted_item(&mut self, mounted_ix: usize, item: ListItem) {
        let mut cursor = self.items.cursor::<Count>(());
        let mut new_items = cursor.slice(&Count(mounted_ix), Bias::Right);
        cursor.seek_forward(&Count(mounted_ix + 1), Bias::Right);
        new_items.push(item, ());
        new_items.append(cursor.suffix(), ());
        drop(cursor);
        self.items = new_items;
    }

    /// The reservation extra for a layout pass, or `None` when the pass did not reach the last
    /// row (so the extra applied earlier stays). `measured` holds the natural sizes of the
    /// contiguous rows measured in the pass, starting at logical row `first_ix`.
    fn pass_reservation_extra(
        &self,
        first_ix: usize,
        measured: &VecDeque<ListItem>,
        viewport: Pixels,
    ) -> Option<Pixels> {
        let reservation = self.reservation?;
        let count = self.logical_count();
        if reservation.start >= count || measured.is_empty() || first_ix + measured.len() != count {
            return None;
        }
        let mut natural = px(0.);
        if reservation.start < first_ix {
            // Rows between the reservation start and the measured sequence count with their
            // measured height or hint; unknown rows defer the reservation for this pass.
            let mounted = self.mounted_range();
            let from = reservation.start.clamp(mounted.start, mounted.end) - mounted.start;
            let to = first_ix.clamp(mounted.start, mounted.end) - mounted.start;
            let mut cursor = self.items.cursor::<ListItemSummary>(());
            cursor.seek(&Count(from), Bias::Right);
            let between: ListItemSummary = cursor.summary(&Count(to), Bias::Right);
            if between.has_unknown_height {
                return Some(px(0.));
            }
            natural += self.row_top(first_ix) - self.row_top(reservation.start);
        }
        let skip = reservation.start.saturating_sub(first_ix);
        natural += measured
            .iter()
            .skip(skip)
            .fold(px(0.), |sum, item| sum + item.height());
        let minimum = (viewport - reservation.inset).max(px(0.));
        Some((minimum - natural).max(px(0.)))
    }

    fn update_pass_reservation(
        &self,
        first_ix: usize,
        measured: &VecDeque<ListItem>,
        viewport: Pixels,
        rendered_height: &mut Pixels,
        pass: &mut PassReservation,
    ) {
        if let Some(extra) = self.pass_reservation_extra(first_ix, measured, viewport) {
            *rendered_height += extra - pass.applied;
            pass.applied = extra;
            pass.computed = true;
        }
    }

    // Anchors.

    /// Keep the anchor on the same row across a splice of `range` (mounted-row indices) that
    /// replaced those rows by `new_len` rows (ELM-001 R11, R12).
    fn adjust_anchor_after_splice(
        &mut self,
        range: Range<usize>,
        new_len: usize,
        old_mounted_len: usize,
    ) {
        let Some(top) = self.logical_scroll_top else {
            return;
        };
        let windowed = self.is_windowed();
        let window_start = self.mounted_range().start;
        let (start, end) = (window_start + range.start, window_start + range.end);
        let old_mounted_end = window_start + old_mounted_len;

        if range.start == 0
            && range.is_empty()
            && top.item_ix == 0
            && top.offset_in_item == px(0.)
            && !self.follow_state.is_following()
        {
            // Prepending at the very top leaves the top of the list in view.
            return;
        }

        let mut item_ix = top.item_ix;
        let mut offset_in_item = top.offset_in_item;
        let mut replaced = false;
        if item_ix >= start && item_ix < end {
            item_ix = start;
            offset_in_item = px(0.);
            replaced = true;
        } else if item_ix >= end && (!windowed || item_ix < old_mounted_end) {
            item_ix = item_ix - (end - start) + new_len;
        }

        if replaced {
            let mounted_end = window_start + self.items.summary().count;
            if item_ix >= mounted_end {
                if mounted_end > window_start {
                    item_ix = mounted_end - 1;
                } else if !windowed {
                    self.logical_scroll_top = None;
                    self.pending_scroll = None;
                    return;
                }
            }
        }

        if let Some(pending) = self.pending_scroll.as_mut() {
            if replaced {
                self.pending_scroll = None;
            } else if pending.item_ix() == top.item_ix {
                pending.set_item_ix(item_ix);
            }
        }
        self.logical_scroll_top = Some(ListOffset {
            item_ix,
            offset_in_item,
        });
    }

    /// Re-anchor a pending scroll adjustment from a remeasure onto a newly set
    /// scroll position, so it clamps to the remeasured item's new height on
    /// the next layout instead of reverting the scroll.
    fn rebase_pending_scroll(&mut self, scroll_top: ListOffset) {
        let Some(pending) = self.pending_scroll.take() else {
            return;
        };
        if scroll_top.item_ix >= self.logical_count() {
            return;
        }

        self.pending_scroll = match pending {
            PendingScroll::Absolute { .. } => Some(PendingScroll::Absolute {
                item_ix: scroll_top.item_ix,
                offset: scroll_top.offset_in_item,
            }),
            PendingScroll::Proportional(_) => self
                .mounted_item(scroll_top.item_ix)
                .and_then(|item| item.size_hint())
                .filter(|size| size.height.0 > 0.0 && scroll_top.offset_in_item >= px(0.))
                .map(|size| {
                    PendingScroll::Proportional(PendingScrollFraction {
                        item_ix: scroll_top.item_ix,
                        fraction: (scroll_top.offset_in_item.0 / size.height.0).clamp(0.0, 1.0),
                    })
                }),
        };
    }

    /// The anchor as stored (an anchor past the end of a scroll-by keeps its excess offset;
    /// layout clamps it).
    fn logical_scroll_top(&self) -> ListOffset {
        self.logical_scroll_top
            .unwrap_or_else(|| match self.alignment {
                ListAlignment::Top => ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                },
                ListAlignment::Bottom => ListOffset {
                    item_ix: self.logical_count(),
                    offset_in_item: px(0.),
                },
            })
    }

    /// The anchor with positions at or past the end normalized to the end of the list.
    fn normalized_scroll_top(&self) -> ListOffset {
        let top = self.logical_scroll_top();
        let count = self.logical_count();
        if top.item_ix >= count {
            ListOffset {
                item_ix: count,
                offset_in_item: px(0.),
            }
        } else {
            top
        }
    }

    /// The anchor that reveals row `ix` fully with minimal movement.
    fn reveal_offset(&self, ix: usize, height: Pixels, padding: &Edges<Pixels>) -> ListOffset {
        let count = self.logical_count();
        let current = self.logical_scroll_top();
        if count == 0 {
            return current;
        }
        let ix = ix.min(count - 1);
        if ix < current.item_ix || (ix == current.item_ix && current.offset_in_item > px(0.)) {
            return ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            };
        }
        let bottom = self.row_top(ix + 1) + padding.top;
        let goal_top = px(0.).max(bottom - height + padding.bottom);
        if goal_top <= self.scroll_px(&current) {
            return current;
        }
        let (item_ix, item_top) = self.locate(goal_top);
        ListOffset {
            item_ix,
            offset_in_item: goal_top - item_top,
        }
    }

    // Windowing.

    /// Replace the mounted rows by `sources.len()` rows starting at logical row `new_start`;
    /// `sources[j]` is the previously mounted row (by mounted index) that becomes new row `j`,
    /// or `None` for a new row. Surviving rows keep their records; with windowing, rows that
    /// leave keep their measured height as a hint for placeholders and rows that enter use
    /// such a hint when there is one.
    fn remount(
        &mut self,
        new_start: usize,
        sources: &[Option<usize>],
        mut new_focus_handle: impl FnMut(usize) -> Option<FocusHandle>,
    ) {
        self.strip_reservation_extra();
        let windowed = self.is_windowed();
        let count = self.logical_count();
        let old_start = self.mounted_range().start;
        let estimate = self.estimate_px();
        let old_items: Vec<ListItem> = self.items.iter().cloned().collect();
        let mut consumed = vec![false; old_items.len()];
        let mut new_items = Vec::with_capacity(sources.len());
        for (ix, source) in sources.iter().enumerate() {
            let survivor =
                source.and_then(|old_ix| old_items.get(old_ix).map(|item| (old_ix, item)));
            new_items.push(match survivor {
                Some((old_ix, item)) => {
                    consumed[old_ix] = true;
                    item.clone()
                }
                None => {
                    let hint = if windowed {
                        self.unmounted_hints.remove(&(new_start + ix))
                    } else {
                        None
                    };
                    ListItem::Unmeasured {
                        size_hint: hint.map(|height| size(px(0.), height)),
                        estimate,
                        focus_handle: new_focus_handle(ix),
                    }
                }
            });
        }
        if windowed {
            let new_range = new_start..new_start + sources.len();
            for (ix, item) in old_items.iter().enumerate() {
                let logical = old_start + ix;
                if consumed[ix] || logical >= count || new_range.contains(&logical) {
                    continue;
                }
                if item.size_hint().is_some() {
                    self.unmounted_hints.insert(logical, item.height());
                }
            }
            while self.unmounted_hints.len() > MAX_UNMOUNTED_HINTS {
                let first = *self.unmounted_hints.keys().next().unwrap();
                let last = *self.unmounted_hints.keys().next_back().unwrap();
                if new_start.abs_diff(first) >= new_start.abs_diff(last) {
                    self.unmounted_hints.remove(&first);
                } else {
                    self.unmounted_hints.remove(&last);
                }
            }
        }
        self.items = SumTree::from_iter(new_items, ());
        self.window_start = new_start;
        self.measuring_behavior.reset();
    }

    /// Replace every measurement by a hint of the same size. Called when the content width
    /// changes (and at the first layout); existing hints are kept.
    fn invalidate_measurements(&mut self) {
        if self.items.summary().measured_count > 0 {
            let estimate = self.estimate_px();
            let new_items = self
                .items
                .iter()
                .map(|item| match item {
                    ListItem::Measured { size, focus_handle } => ListItem::Unmeasured {
                        size_hint: Some(*size),
                        estimate,
                        focus_handle: focus_handle.clone(),
                    },
                    other => other.clone(),
                })
                .collect::<Vec<_>>();
            self.items = SumTree::from_iter(new_items, ());
        }
        self.measuring_behavior.reset();
    }

    fn max_scroll_offset(&self) -> Pixels {
        let bounds = self.last_layout_bounds.unwrap_or_default();
        let height = self
            .scrollbar_drag_start_height
            .unwrap_or_else(|| self.total_height());
        (height - bounds.size.height).max(px(0.))
    }

    /// The logical rows intersecting the viewport: inclusive start, exclusive end.
    fn visible_range(&self, height: Pixels, scroll_top: &ListOffset) -> Range<usize> {
        let count = self.logical_count();
        let start = scroll_top.item_ix.min(count);
        let start_y = self.scroll_px(scroll_top);
        let end_y = start_y + height;
        // The last visible row is the one containing the viewport bottom, resolving a bottom
        // that coincides with a row boundary to the row above it.
        let last = if self.is_windowed() {
            let (ix, top) = self.locate(end_y);
            if top == end_y && ix > 0 { ix - 1 } else { ix }
        } else {
            self.items
                .find::<ListItemSummary, _>((), &Height(end_y), Bias::Left)
                .0
                .count
        };
        start..(last + 1).min(count).max(start)
    }

    /// Record the visible range after a layout; returns the notification to deliver when the
    /// range changed and a handler is registered.
    fn visible_range_notification(
        &mut self,
        height: Pixels,
        scroll_top: &ListOffset,
    ) -> Option<(ScrollHandler, ListScrollEvent)> {
        let visible_range = self.visible_range(height, scroll_top);
        if self.last_visible_range.as_ref() == Some(&visible_range) {
            return None;
        }
        self.last_visible_range = Some(visible_range.clone());
        let handler = self.scroll_handler.clone()?;
        Some((
            handler,
            ListScrollEvent {
                visible_range,
                count: self.logical_count(),
                is_scrolled: self.logical_scroll_top.is_some(),
                is_following_tail: self.follow_state.is_following(),
                user_initiated: false,
            },
        ))
    }

    fn scroll(
        &mut self,
        scroll_top: &ListOffset,
        height: Pixels,
        delta: Point<Pixels>,
        current_view: EntityId,
        cx: &mut App,
    ) -> Option<(ScrollHandler, ListScrollEvent)> {
        // Drop scroll events after a reset, since we can't calculate
        // the new logical scroll top without the item heights
        if self.reset {
            return None;
        }

        let padding = self.last_padding.unwrap_or_default();
        let scroll_max = (self.total_height() + padding.top + padding.bottom - height).max(px(0.));
        let new_scroll_top = (self.scroll_px(scroll_top) - delta.y)
            .max(px(0.))
            .min(scroll_max);

        if self.alignment == ListAlignment::Bottom && new_scroll_top == scroll_max {
            self.pending_scroll = None;
            self.logical_scroll_top = None;
        } else {
            let (item_ix, item_top) = self.locate(new_scroll_top);
            let scroll_top = ListOffset {
                item_ix,
                offset_in_item: new_scroll_top - item_top,
            };
            // The user's scroll supersedes the position stashed by a
            // remeasure; re-anchor the pending adjustment so it doesn't revert
            // this scroll on the next layout.
            self.rebase_pending_scroll(scroll_top);
            self.logical_scroll_top = Some(scroll_top);
        }

        if delta.y > px(0.) {
            self.follow_state.stop_following();
        }

        cx.notify(current_view);

        let handler = self.scroll_handler.clone()?;
        let visible_range = self.visible_range(height, scroll_top);
        self.last_visible_range = Some(visible_range.clone());
        Some((
            handler,
            ListScrollEvent {
                visible_range,
                count: self.logical_count(),
                is_scrolled: self.logical_scroll_top.is_some(),
                is_following_tail: self.follow_state.is_following(),
                user_initiated: true,
            },
        ))
    }

    // Layout.

    /// The height of row `ix` for walks that must not guess: a measured mounted row keeps its
    /// size, any other mounted row is rendered and measured, placeholders use their height.
    fn walk_row_height(
        &self,
        ix: usize,
        space: Size<AvailableSpace>,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) -> Pixels {
        match self.mounted_item(ix) {
            Some(item) => item.size().map_or_else(
                || {
                    let mut element = render_item(ix, window, cx);
                    window
                        .with_layout_item_index(ix, |window| {
                            element.layout_as_root(space, window, cx)
                        })
                        .height
                },
                |size| size.height,
            ),
            None => self.placeholder_height(ix),
        }
    }

    fn layout_all_items(
        &mut self,
        available_width: Pixels,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) {
        match &mut self.measuring_behavior {
            ListMeasuringBehavior::Visible => {
                return;
            }
            ListMeasuringBehavior::Measure(has_measured) => {
                if *has_measured {
                    return;
                }
                *has_measured = true;
            }
        }

        let mounted = self.mounted_range();
        let available_item_space = size(
            AvailableSpace::Definite(available_width),
            AvailableSpace::MinContent,
        );

        let mut measured_items = Vec::with_capacity(self.items.summary().count);
        for (ix, item) in self.items.iter().enumerate() {
            if ix >= mounted.len() {
                measured_items.push(item.clone());
                continue;
            }
            let size = item.size().unwrap_or_else(|| {
                let item_index = mounted.start + ix;
                let mut element = render_item(item_index, window, cx);
                window.with_layout_item_index(item_index, |window| {
                    element.layout_as_root(available_item_space, window, cx)
                })
            });

            measured_items.push(ListItem::Measured {
                size,
                focus_handle: item.focus_handle(),
            });
        }

        self.items = SumTree::from_iter(measured_items, ());
    }

    fn layout_items(
        &mut self,
        available_width: Option<Pixels>,
        available_height: Pixels,
        padding: &Edges<Pixels>,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) -> LayoutItemsResponse {
        let count = self.logical_count();
        let mounted = self.mounted_range();
        let old_items = self.items.clone();
        let overdraw = self.overdraw;
        let mut measured_items: VecDeque<ListItem> = VecDeque::new();
        // The logical index of `measured_items[0]`.
        let mut measured_start = 0;
        let mut item_layouts = VecDeque::new();
        let mut rendered_height = padding.top;
        let mut max_item_width = px(0.);
        let mut scroll_top = self.normalized_scroll_top();
        let reservation_active = self.reservation.is_some_and(|r| r.start < count);
        let mut pass_reservation = PassReservation {
            computed: false,
            applied: px(0.),
        };

        if self.follow_state.is_following() {
            scroll_top = ListOffset {
                item_ix: count,
                offset_in_item: px(0.),
            };
            self.logical_scroll_top = Some(scroll_top);
        }

        let mut rendered_focus_id = None;

        let available_item_space = size(
            available_width.map_or(AvailableSpace::MaxContent, |width| {
                AvailableSpace::Definite(width)
            }),
            AvailableSpace::MinContent,
        );

        // A pending adjustment (from a remeasure) needs the remeasured anchor row first.
        let mut prerendered: Option<(usize, AnyElement, Size<Pixels>)> = None;
        if mounted.contains(&scroll_top.item_ix) {
            if let Some(pending_scroll) = self.pending_scroll.take() {
                let item_ix = scroll_top.item_ix;
                let mut element = render_item(item_ix, window, cx);
                let element_size = window.with_layout_item_index(item_ix, |window| {
                    element.layout_as_root(available_item_space, window, cx)
                });
                match pending_scroll {
                    PendingScroll::Absolute {
                        item_ix: ix,
                        offset,
                    } if ix == item_ix => {
                        scroll_top.offset_in_item = offset.min(element_size.height);
                        self.logical_scroll_top = Some(scroll_top);
                    }
                    PendingScroll::Proportional(pending) if pending.item_ix == item_ix => {
                        // Ensuring proportional scroll position is
                        // maintained after re-measuring.
                        scroll_top.offset_in_item =
                            Pixels(pending.fraction * element_size.height.0);
                        self.logical_scroll_top = Some(scroll_top);
                    }
                    _ => {}
                }
                prerendered = Some((item_ix, element, element_size));
            }
        }

        // A negative offset places the anchor row below the viewport top. Resolve it now,
        // from measured predecessors, into the row holding the viewport top (ELM-001 R9).
        if scroll_top.offset_in_item < px(0.) {
            let mut remaining = -scroll_top.offset_in_item;
            let mut ix = scroll_top.item_ix;
            loop {
                if ix == 0 {
                    scroll_top = ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.),
                    };
                    break;
                }
                ix -= 1;
                let height =
                    self.walk_row_height(ix, available_item_space, render_item, window, cx);
                if remaining <= height {
                    scroll_top = ListOffset {
                        item_ix: ix,
                        offset_in_item: height - remaining,
                    };
                    break;
                }
                remaining -= height;
            }
            self.logical_scroll_top = Some(scroll_top);
        }

        // Render items after the scroll top, including those in the trailing overdraw
        let mut ix = scroll_top.item_ix;
        while ix < count {
            let visible_height = rendered_height - scroll_top.offset_in_item;
            if visible_height >= available_height + overdraw {
                break;
            }

            if let Some(item) = self.mounted_item(ix) {
                // Use the previously cached height and focus handle if available
                let mut size = item.size();
                // Under an active reservation the last row is always remeasured so its stored
                // size never accumulates the extra.
                let remeasure_last = reservation_active && ix + 1 == count;

                // If we're within the visible area or the height wasn't cached, render and measure the item's element
                if visible_height < available_height || size.is_none() || remeasure_last {
                    let (element, element_size) =
                        match prerendered.take_if(|prerendered| prerendered.0 == ix) {
                            Some((_, element, element_size)) => (element, element_size),
                            None => {
                                let mut element = render_item(ix, window, cx);
                                let element_size = window.with_layout_item_index(ix, |window| {
                                    element.layout_as_root(available_item_space, window, cx)
                                });
                                (element, element_size)
                            }
                        };
                    size = Some(element_size);

                    if visible_height < available_height {
                        item_layouts.push_back(ItemLayout { index: ix, element });
                        if let Some(focus_id) = item.focus_id_if_focused(window, cx) {
                            rendered_focus_id = Some(focus_id);
                        }
                    }
                }

                let size = size.unwrap();
                rendered_height += size.height;
                max_item_width = max_item_width.max(size.width);
                if measured_items.is_empty() {
                    measured_start = ix;
                }
                measured_items.push_back(ListItem::Measured {
                    size,
                    focus_handle: item.focus_handle(),
                });
            } else {
                rendered_height += self.placeholder_height(ix);
            }
            ix += 1;
        }
        rendered_height += padding.bottom;
        self.update_pass_reservation(
            measured_start,
            &measured_items,
            available_height,
            &mut rendered_height,
            &mut pass_reservation,
        );

        // The topmost row reached so far.
        let mut top_ix = scroll_top.item_ix;

        // If the rendered items do not fill the visible region, then adjust
        // the scroll top upward.
        if rendered_height - scroll_top.offset_in_item < available_height {
            while rendered_height < available_height && top_ix > 0 {
                let prev_ix = top_ix - 1;
                if let Some(item) = self.mounted_item(prev_ix) {
                    let mut element = render_item(prev_ix, window, cx);
                    let element_size = window.with_layout_item_index(prev_ix, |window| {
                        element.layout_as_root(available_item_space, window, cx)
                    });
                    rendered_height += element_size.height;
                    measured_items.push_front(ListItem::Measured {
                        size: element_size,
                        focus_handle: item.focus_handle(),
                    });
                    measured_start = prev_ix;
                    item_layouts.push_front(ItemLayout {
                        index: prev_ix,
                        element,
                    });
                    if let Some(focus_id) = item.focus_id_if_focused(window, cx) {
                        rendered_focus_id = Some(focus_id);
                    }
                } else {
                    rendered_height += self.placeholder_height(prev_ix);
                }
                top_ix = prev_ix;
                self.update_pass_reservation(
                    measured_start,
                    &measured_items,
                    available_height,
                    &mut rendered_height,
                    &mut pass_reservation,
                );
            }

            scroll_top = ListOffset {
                item_ix: top_ix,
                offset_in_item: rendered_height - available_height,
            };

            match self.alignment {
                ListAlignment::Top => {
                    scroll_top.offset_in_item = scroll_top.offset_in_item.max(px(0.));
                    self.logical_scroll_top = Some(scroll_top);
                }
                ListAlignment::Bottom => {
                    self.logical_scroll_top = None;
                }
            };
        }

        // Measure items in the leading overdraw
        let mut leading_overdraw = scroll_top.offset_in_item;
        while leading_overdraw < overdraw && top_ix > 0 {
            let prev_ix = top_ix - 1;
            leading_overdraw += if let Some(item) = self.mounted_item(prev_ix) {
                let size = item.size().unwrap_or_else(|| {
                    let mut element = render_item(prev_ix, window, cx);
                    window.with_layout_item_index(prev_ix, |window| {
                        element.layout_as_root(available_item_space, window, cx)
                    })
                });
                measured_items.push_front(ListItem::Measured {
                    size,
                    focus_handle: item.focus_handle(),
                });
                measured_start = prev_ix;
                size.height
            } else {
                self.placeholder_height(prev_ix)
            };
            top_ix = prev_ix;
        }

        // The reservation was computed from every row measured in this pass; the extra joins
        // the last row's layout box.
        self.update_pass_reservation(
            measured_start,
            &measured_items,
            available_height,
            &mut rendered_height,
            &mut pass_reservation,
        );
        if pass_reservation.computed {
            if let Some(ListItem::Measured { size, .. }) = measured_items.back_mut() {
                size.height += pass_reservation.applied;
            }
            self.reservation_extra = pass_reservation.applied;
        }

        if !measured_items.is_empty() {
            let start = measured_start - mounted.start;
            let end = start + measured_items.len();
            let mut cursor = old_items.cursor::<Count>(());
            let mut new_items = cursor.slice(&Count(start), Bias::Right);
            new_items.extend(measured_items, ());
            cursor.seek(&Count(end), Bias::Right);
            new_items.append(cursor.suffix(), ());
            self.items = new_items;
        }

        // If follow_tail mode is on but the user scrolled away
        // (is_following is false), check whether the current scroll
        // position has returned to the bottom.
        if self.follow_state.has_stopped_following() {
            let total_height = self.total_height() + padding.top + padding.bottom;
            let scroll_offset = self.scroll_px(&scroll_top);
            if scroll_offset + available_height >= total_height - TAIL_RESUME_TOLERANCE {
                self.follow_state.start_following();
            }
        }

        // If none of the visible items are focused, check if an off-screen item is focused
        // and include it to be rendered after the visible items so keyboard interaction continues
        // to work for it. A newly focused off-screen item is scrolled into view.
        let mut reveal_focus = None;
        if rendered_focus_id.is_none() {
            let mut found = None;
            {
                let mut cursor = self
                    .items
                    .filter::<_, Count>((), |summary| summary.has_focus_handles);
                cursor.next();
                while let Some(item) = cursor.item() {
                    let item_index = mounted.start + cursor.start().0;
                    if item_index >= mounted.end {
                        break;
                    }
                    if let Some(focus_id) = item.focus_id_if_focused(window, cx) {
                        found = Some((item_index, focus_id, item.focus_handle()));
                        break;
                    }
                    cursor.next();
                }
            }
            if let Some((item_index, focus_id, focus_handle)) = found {
                let mut element = render_item(item_index, window, cx);
                let element_size = window.with_layout_item_index(item_index, |window| {
                    element.layout_as_root(available_item_space, window, cx)
                });
                item_layouts.push_back(ItemLayout {
                    index: item_index,
                    element,
                });
                // Its height is now known to the scroll geometry (the reserved last row keeps
                // the stored size that includes its extra).
                if item_index + 1 != count || self.reservation_extra == px(0.) {
                    self.replace_mounted_item(
                        item_index - mounted.start,
                        ListItem::Measured {
                            size: element_size,
                            focus_handle,
                        },
                    );
                }
                if self.last_focus_id != Some(focus_id) && !self.follow_state.is_following() {
                    reveal_focus = Some(item_index);
                }
                rendered_focus_id = Some(focus_id);
            }
        }
        self.last_focus_id = rendered_focus_id;

        LayoutItemsResponse {
            max_item_width,
            scroll_top,
            item_layouts,
            reveal_focus,
        }
    }

    fn prepaint_items(
        &mut self,
        bounds: Bounds<Pixels>,
        padding: Edges<Pixels>,
        autoscroll: bool,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<LayoutItemsResponse, ListOffset> {
        window.transact(|window| {
            // Rows are laid out in the list's width minus its horizontal padding.
            let content_width = (bounds.size.width - padding.left - padding.right).max(px(0.));
            if let ListMeasuringBehavior::Measure(false) = self.measuring_behavior {
                self.layout_all_items(content_width, render_item, window, cx);
            }

            let mut layout_response = self.layout_items(
                Some(content_width),
                bounds.size.height,
                &padding,
                render_item,
                window,
                cx,
            );

            // Avoid honoring autoscroll requests from elements other than our children.
            window.take_autoscroll();

            if let Some(ix) = layout_response.reveal_focus.take() {
                return Err(self.reveal_offset(ix, bounds.size.height, &padding));
            }

            // Only paint the visible items, if there is actually any space for them (taking padding into account)
            if bounds.size.height > padding.top + padding.bottom {
                let item_space = size(content_width.into(), AvailableSpace::MinContent);
                let content_origin = bounds.origin + Point::new(padding.left, padding.top);
                let scroll_px = self.scroll_px(&layout_response.scroll_top);
                for item in &mut layout_response.item_layouts {
                    let item_origin = point(
                        content_origin.x,
                        content_origin.y + self.row_top(item.index) - scroll_px,
                    );
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        item.element.prepaint_at(item_origin, window, cx);
                    });

                    if let Some(autoscroll_bounds) = window.take_autoscroll()
                        && autoscroll
                    {
                        if autoscroll_bounds.top() < bounds.top() {
                            let mut item_ix = item.index;
                            let mut offset_in_item = autoscroll_bounds.top() - item_origin.y;

                            // The requested top can sit above this item's own
                            // top. Walk into earlier items so the offset stays
                            // non-negative and no blank space appears above the
                            // list.
                            while offset_in_item < Pixels::ZERO {
                                if item_ix == 0 {
                                    offset_in_item = Pixels::ZERO;
                                    break;
                                }
                                item_ix -= 1;
                                offset_in_item += self.walk_row_height(
                                    item_ix,
                                    item_space,
                                    render_item,
                                    window,
                                    cx,
                                );
                            }

                            return Err(ListOffset {
                                item_ix,
                                offset_in_item,
                            });
                        } else if autoscroll_bounds.bottom() > bounds.bottom() {
                            let mut item_ix = item.index;
                            let mut height = bounds.size.height - padding.top - padding.bottom;

                            // Account for the height of the element down until the autoscroll bottom.
                            height -= autoscroll_bounds.bottom() - item_origin.y;

                            // Keep decreasing the scroll top until we fill all the available space.
                            while height > Pixels::ZERO && item_ix > 0 {
                                item_ix -= 1;
                                height -= self.walk_row_height(
                                    item_ix,
                                    item_space,
                                    render_item,
                                    window,
                                    cx,
                                );
                            }

                            return Err(ListOffset {
                                item_ix,
                                offset_in_item: if height < Pixels::ZERO {
                                    -height
                                } else {
                                    Pixels::ZERO
                                },
                            });
                        }
                    }
                }
            } else {
                layout_response.item_layouts.clear();
            }

            Ok(layout_response)
        })
    }

    // Scrollbar support

    fn set_offset_from_scrollbar(&mut self, point: Point<Pixels>) {
        let Some(bounds) = self.last_layout_bounds else {
            return;
        };
        let height = bounds.size.height;

        let padding = self.last_padding.unwrap_or_default();
        // Scrollbar drag positions are computed from the content height
        // captured at drag start, so map them back using the same height.
        let content_height = self
            .scrollbar_drag_start_height
            .unwrap_or_else(|| self.total_height());
        let scroll_max = (content_height + padding.top + padding.bottom - height).max(px(0.));
        let new_scroll_top = (-point.y).max(px(0.)).min(scroll_max);

        // If content grew during the drag, the frozen bottom is below the
        // live bottom. Treat dragging to the frozen end as resuming tail follow.
        let dragged_to_end =
            scroll_max > px(0.) && new_scroll_top >= (scroll_max - px(1.0)).max(px(0.));
        if dragged_to_end && matches!(self.follow_state, FollowState::Tail { .. }) {
            self.follow_state = FollowState::Tail { is_following: true };
            let item_count = self.logical_count();
            self.pending_scroll = None;
            self.logical_scroll_top = Some(ListOffset {
                item_ix: item_count,
                offset_in_item: px(0.),
            });
            return;
        }

        self.follow_state.stop_following();

        if self.alignment == ListAlignment::Bottom && new_scroll_top == scroll_max {
            self.pending_scroll = None;
            self.logical_scroll_top = None;
        } else {
            let (item_ix, item_top) = self.locate(new_scroll_top);
            let scroll_top = ListOffset {
                item_ix,
                offset_in_item: new_scroll_top - item_top,
            };
            self.rebase_pending_scroll(scroll_top);
            self.logical_scroll_top = Some(scroll_top);
        }
    }
}

impl std::fmt::Debug for ListItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unmeasured { .. } => write!(f, "Unrendered"),
            Self::Measured { size, .. } => f.debug_struct("Rendered").field("size", size).finish(),
        }
    }
}

/// An offset into the list's items, in terms of the item index and the number
/// of pixels off the top left of the item. The offset is signed: a negative offset places the
/// item's top below the viewport top by that many pixels and is resolved during layout.
#[derive(Debug, Clone, Copy, Default)]
pub struct ListOffset {
    /// The index of an item in the list
    pub item_ix: usize,
    /// The number of pixels to offset from the item index.
    pub offset_in_item: Pixels,
}

impl Element for List {
    type RequestLayoutState = ();
    type PrepaintState = ListPrepaintState;

    fn id(&self) -> Option<crate::ElementId> {
        self.a11y.element_id.clone()
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.a11y.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.a11y.write_a11y_info(node);
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        let layout_id = match self.sizing_behavior {
            ListSizingBehavior::Infer => {
                let mut style = Style::default();
                style.overflow.y = Overflow::Scroll;
                style.refine(&self.style);
                window.with_text_style(style.text_style().cloned(), |window| {
                    let state = &mut *self.state.0.borrow_mut();

                    let available_height = if let Some(last_bounds) = state.last_layout_bounds {
                        last_bounds.size.height
                    } else {
                        // If we don't have the last layout bounds (first render),
                        // we might just use the overdraw value as the available height to layout enough items.
                        state.overdraw
                    };
                    let padding = style.padding.to_pixels(
                        state.last_layout_bounds.unwrap_or_default().size.into(),
                        window.rem_size(),
                    );

                    let layout_response = state.layout_items(
                        None,
                        available_height,
                        &padding,
                        &mut self.render_item,
                        window,
                        cx,
                    );
                    let max_element_width = layout_response.max_item_width;

                    let total_height = state.total_height();

                    window.request_measured_layout(
                        style,
                        move |known_dimensions, available_space, _window, _cx| {
                            let width =
                                known_dimensions
                                    .width
                                    .unwrap_or(match available_space.width {
                                        AvailableSpace::Definite(x) => x,
                                        AvailableSpace::MinContent | AvailableSpace::MaxContent => {
                                            max_element_width
                                        }
                                    });
                            let height = match available_space.height {
                                AvailableSpace::Definite(height) => total_height.min(height),
                                AvailableSpace::MinContent | AvailableSpace::MaxContent => {
                                    total_height
                                }
                            };
                            size(width, height)
                        },
                    )
                })
            }
            ListSizingBehavior::Auto => {
                let mut style = Style::default();
                style.refine(&self.style);
                window.with_text_style(style.text_style().cloned(), |window| {
                    window.request_layout(style, None, cx)
                })
            }
        };
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> ListPrepaintState {
        let mut style = Style::default();
        style.refine(&self.style);

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let padding = style
            .padding
            .to_pixels(bounds.size.into(), window.rem_size());
        let content_width = (bounds.size.width - padding.left - padding.right).max(px(0.));

        let (layout, notification) = {
            let state = &mut *self.state.0.borrow_mut();
            state.reset = false;

            // If the content width of the list has changed (its width or its horizontal
            // padding), every measured height becomes a hint; hints themselves survive.
            if state.last_content_width != Some(content_width) {
                state.invalidate_measurements();
                state.last_content_width = Some(content_width);
            }

            let layout = match state.prepaint_items(
                bounds,
                padding,
                true,
                &mut self.render_item,
                window,
                cx,
            ) {
                Ok(layout) => layout,
                Err(autoscroll_request) => {
                    state.logical_scroll_top = Some(autoscroll_request);
                    state
                        .prepaint_items(bounds, padding, false, &mut self.render_item, window, cx)
                        .unwrap()
                }
            };

            state.last_layout_bounds = Some(bounds);
            state.last_padding = Some(padding);
            let notification =
                state.visible_range_notification(bounds.size.height, &layout.scroll_top);
            (layout, notification)
        };

        if let Some((handler, event)) = notification {
            window.defer(cx, move |window, cx| {
                (handler.borrow_mut())(&event, window, cx);
            });
        }

        ListPrepaintState { hitbox, layout }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<crate::Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let current_view = window.current_view();

        // Register the scroll listener before painting children so that, in
        // the bubble phase (which runs in reverse registration order),
        // children's scroll-wheel handlers run first and can stop propagation
        // to prevent the list from scrolling. This matches the ordering of
        // div-based scroll containers.
        let list_state = self.state.clone();
        let height = bounds.size.height;
        let scroll_top = prepaint.layout.scroll_top;
        let hitbox_id = prepaint.hitbox.id;
        let mut accumulated_scroll_delta = ScrollDelta::default();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox_id.should_handle_scroll(window) {
                accumulated_scroll_delta = accumulated_scroll_delta.coalesce(event.delta);
                let pixel_delta = accumulated_scroll_delta.pixel_delta(px(20.));
                let notification = list_state.0.borrow_mut().scroll(
                    &scroll_top,
                    height,
                    pixel_delta,
                    current_view,
                    cx,
                );
                if let Some((handler, event)) = notification {
                    (handler.borrow_mut())(&event, window, cx);
                }
            }
        });

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for item in &mut prepaint.layout.item_layouts {
                item.element.paint(window, cx);
            }
        });

        self.paint_classic_scrollbar(bounds, window);
    }
}

impl List {
    /// The classic vertical thumb of CMP-031, painted after the rows from the extents of this
    /// frame's layout.
    fn paint_classic_scrollbar(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let Some(color) = self.style.scrollbar_color else {
            return;
        };
        if !crate::is_valid_scrollbar_color(color) {
            return;
        }
        let thickness = self
            .style
            .scrollbar_width
            .map_or(crate::CLASSIC_SCROLLBAR_THICKNESS, |width| {
                f32::from(width.to_pixels(window.rem_size()))
            });
        let (max, position) = self.state.scrollbar_extents();
        let thumbs = crate::classic_thumb_rects(
            bounds,
            point(px(0.), max),
            point(px(0.), -position),
            thickness,
            false,
            true,
        );
        crate::paint_classic_thumbs(&thumbs, thickness, color, window);
    }
}

impl IntoElement for List {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for List {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl vespertine_sum_tree::Item for ListItem {
    type Summary = ListItemSummary;

    fn summary(&self, _: ()) -> Self::Summary {
        match self {
            ListItem::Unmeasured {
                size_hint,
                estimate,
                focus_handle,
            } => ListItemSummary {
                count: 1,
                measured_count: 0,
                height: size_hint.map_or(*estimate, |size| size.height),
                measured_height: px(0.),
                has_focus_handles: focus_handle.is_some(),
                has_unknown_height: size_hint.is_none() && *estimate == px(0.),
            },
            ListItem::Measured {
                size, focus_handle, ..
            } => ListItemSummary {
                count: 1,
                measured_count: 1,
                height: size.height,
                measured_height: size.height,
                has_focus_handles: focus_handle.is_some(),
                has_unknown_height: false,
            },
        }
    }
}

impl vespertine_sum_tree::ContextLessSummary for ListItemSummary {
    fn zero() -> Self {
        Default::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.count += summary.count;
        self.measured_count += summary.measured_count;
        self.height += summary.height;
        self.measured_height += summary.measured_height;
        self.has_focus_handles |= summary.has_focus_handles;
        self.has_unknown_height |= summary.has_unknown_height;
    }
}

impl<'a> vespertine_sum_tree::Dimension<'a, ListItemSummary> for Count {
    fn zero(_cx: ()) -> Self {
        Default::default()
    }

    fn add_summary(&mut self, summary: &'a ListItemSummary, _: ()) {
        self.0 += summary.count;
    }
}

impl<'a> vespertine_sum_tree::Dimension<'a, ListItemSummary> for Height {
    fn zero(_cx: ()) -> Self {
        Default::default()
    }

    fn add_summary(&mut self, summary: &'a ListItemSummary, _: ()) {
        self.0 += summary.height;
    }
}

impl vespertine_sum_tree::SeekTarget<'_, ListItemSummary, ListItemSummary> for Count {
    fn cmp(&self, other: &ListItemSummary, _: ()) -> std::cmp::Ordering {
        self.0.partial_cmp(&other.count).unwrap()
    }
}

impl vespertine_sum_tree::SeekTarget<'_, ListItemSummary, ListItemSummary> for Height {
    fn cmp(&self, other: &ListItemSummary, _: ()) -> std::cmp::Ordering {
        self.0.partial_cmp(&other.height).unwrap()
    }
}

#[cfg(test)]
mod scroll_by_tests;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod elm001_tests;

#[cfg(test)]
mod reservation_tests;

#[cfg(test)]
mod test {

    use std::cell::Cell;
    use std::rc::Rc;
    use vespertine::{ScrollDelta, ScrollWheelEvent};

    use crate::{
        self as vespertine, AppContext, Bounds, Context, Element, FollowMode, InteractiveElement,
        IntoElement, ListState, Render, Styled, TestAppContext, Window, canvas, div, list, point,
        px, size,
    };

    #[vespertine::test]
    fn test_autoscroll_above_item_top_renders_items_above(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.));
        state.scroll_to(vespertine::ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |ix, _, _| {
                    if ix == 2 {
                        // Request an autoscroll whose top sits 30px above item 2's
                        // own top, mimicking a scroll-margin overshoot.
                        canvas(
                            |bounds, window, _| {
                                window.request_autoscroll(Bounds::from_corners(
                                    point(bounds.left(), bounds.top() - px(30.)),
                                    point(bounds.right(), bounds.top() + px(5.)),
                                ));
                            },
                            |_, _, _, _| {},
                        )
                        .h(px(20.))
                        .w_full()
                        .into_any()
                    } else {
                        div().h(px(20.)).w_full().into_any()
                    }
                })
                .w_full()
                .h_full()
            }
        }

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(60.)), |_, cx| {
            cx.new(|_| TestView(state.clone())).into_any_element()
        });

        // 30px above item 2's top, with 20px items, lands 10px into item 0.
        let scroll_top = state.logical_scroll_top();
        assert!(
            scroll_top.offset_in_item >= px(0.),
            "offset_in_item must never be negative (would leave blank space above), got {:?}",
            scroll_top.offset_in_item,
        );
        assert_eq!(scroll_top.item_ix, 0);
        assert_eq!(scroll_top.offset_in_item, px(10.));
    }

    #[vespertine::test]
    fn test_reset_after_paint_before_scroll(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.));

        // Ensure that the list is scrolled to the top
        state.scroll_to(vespertine::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(10.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        // Paint
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
            cx.new(|_| TestView(state.clone())).into_any_element()
        });

        // Reset
        state.reset(5);

        // And then receive a scroll event _before_ the next paint
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(1.), px(1.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-500.))),
            ..Default::default()
        });

        // Scroll position should stay at the top of the list
        assert_eq!(state.logical_scroll_top().item_ix, 0);
        assert_eq!(state.logical_scroll_top().offset_in_item, px(0.));
    }

    #[vespertine::test]
    fn test_scroll_by_positive_and_negative_distance(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.));

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(20.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        // Paint
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, cx| {
            cx.new(|_| TestView(state.clone())).into_any_element()
        });

        // Test positive distance: start at item 1, move down 30px
        state.scroll_by(px(30.));

        // Should move to item 2
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 1);
        assert_eq!(offset.offset_in_item, px(10.));

        // Test negative distance: start at item 2, move up 30px
        state.scroll_by(px(-30.));

        // Should move back to item 1
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 0);
        assert_eq!(offset.offset_in_item, px(0.));

        // Test zero distance
        state.scroll_by(px(0.));
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 0);
        assert_eq!(offset.offset_in_item, px(0.));
    }

    #[vespertine::test]
    fn test_child_scroll_handler_can_stop_list_scroll(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.));
        let child_saw_event = Rc::new(Cell::new(false));

        struct TestView {
            state: ListState,
            child_saw_event: Rc<Cell<bool>>,
        }
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let child_saw_event = self.child_saw_event.clone();
                list(self.state.clone(), move |_, _, _| {
                    let child_saw_event = child_saw_event.clone();
                    div()
                        .h(px(20.))
                        .w_full()
                        .on_scroll_wheel(move |_, _, cx| {
                            child_saw_event.set(true);
                            cx.stop_propagation();
                        })
                        .into_any()
                })
                .w_full()
                .h_full()
            }
        }

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, cx| {
            cx.new(|_| TestView {
                state: state.clone(),
                child_saw_event: child_saw_event.clone(),
            })
            .into_any_element()
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
            ..Default::default()
        });

        assert!(
            child_saw_event.get(),
            "the child's scroll-wheel handler should run"
        );
        // The child stopped propagation, so the list must not have scrolled.
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 0);
        assert_eq!(offset.offset_in_item, px(0.));
    }

    struct TestListView(ListState);
    impl Render for TestListView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            list(self.0.clone(), |_, _, _| {
                div().h(px(20.)).w_full().into_any()
            })
            .w_full()
            .h_full()
        }
    }

    #[vespertine::test]
    fn test_item_viewport_queries_return_none_before_layout(_cx: &mut TestAppContext) {
        let state = ListState::new(5, crate::ListAlignment::Top, px(10.)).measure_all();

        assert_eq!(state.item_is_above_viewport(0), None);
        assert_eq!(state.item_is_below_viewport(0), None);
    }

    #[vespertine::test]
    fn test_item_viewport_queries_before_logical_scroll_top(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.)).measure_all();

        state.scroll_to(vespertine::ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
            cx.new(|_| TestListView(state.clone())).into_any_element()
        });

        assert_eq!(state.item_is_above_viewport(1), Some(true));
        assert_eq!(state.item_is_below_viewport(1), Some(false));
    }

    #[vespertine::test]
    fn test_item_viewport_queries_measured_item_inside_viewport(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.)).measure_all();

        state.scroll_to(vespertine::ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
            cx.new(|_| TestListView(state.clone())).into_any_element()
        });

        assert_eq!(state.item_is_above_viewport(2), Some(false));
        assert_eq!(state.item_is_below_viewport(2), Some(false));
    }

    #[vespertine::test]
    fn test_item_viewport_queries_measured_item_above_viewport(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.)).measure_all();

        state.scroll_to(vespertine::ListOffset {
            item_ix: 2,
            offset_in_item: px(20.),
        });
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
            cx.new(|_| TestListView(state.clone())).into_any_element()
        });

        assert_eq!(state.item_is_above_viewport(2), Some(true));
        assert_eq!(state.item_is_below_viewport(2), Some(false));
    }

    #[vespertine::test]
    fn test_item_viewport_queries_measured_item_below_viewport(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.)).measure_all();

        state.scroll_to(vespertine::ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
            cx.new(|_| TestListView(state.clone())).into_any_element()
        });

        assert_eq!(state.item_is_above_viewport(3), Some(false));
        assert_eq!(state.item_is_below_viewport(3), Some(true));
    }

    #[vespertine::test]
    fn test_item_viewport_queries_remain_stable_with_zero_height_viewport(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.)).measure_all();

        state.scroll_to(vespertine::ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
            cx.new(|_| TestListView(state.clone())).into_any_element()
        });

        assert_eq!(state.item_is_above_viewport(3), Some(false));
        assert_eq!(state.item_is_below_viewport(3), Some(true));

        // Squeeze the list to zero height, e.g. because a sibling element
        // (sized based on the queries above) consumed all the space. The
        // answers must remain definitive rather than becoming `None`,
        // otherwise the sibling's size can oscillate between frames.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(0.)), |_, cx| {
            cx.new(|_| TestListView(state.clone())).into_any_element()
        });

        assert_eq!(state.item_is_above_viewport(1), Some(true));
        assert_eq!(state.item_is_below_viewport(1), Some(false));
        assert_eq!(state.item_is_above_viewport(3), Some(false));
        assert_eq!(state.item_is_below_viewport(3), Some(true));
    }

    #[vespertine::test]
    fn test_item_viewport_queries_after_scroll_to_end_before_layout(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(5, crate::ListAlignment::Top, px(10.)).measure_all();

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(20.)), |_, cx| {
            cx.new(|_| TestListView(state.clone())).into_any_element()
        });

        state.scroll_to_end();

        assert_eq!(state.logical_scroll_top().item_ix, state.item_count());
        assert_eq!(state.item_is_above_viewport(0), Some(true));
        assert_eq!(state.item_is_below_viewport(0), Some(false));
    }

    #[vespertine::test]
    fn test_measure_all_after_width_change(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(10, crate::ListAlignment::Top, px(0.)).measure_all();

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));

        // First draw at width 100: all 10 items measured (total 500px).
        // Viewport is 200px, so max scroll offset should be 300px.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert_eq!(state.max_offset_for_scrollbar().y, px(300.));

        // Second draw at a different width: items get invalidated.
        // Without the fix, max_offset would drop because unmeasured items
        // contribute 0 height.
        cx.draw(point(px(0.), px(0.)), size(px(200.), px(200.)), |_, _| {
            view.into_any_element()
        });
        assert_eq!(state.max_offset_for_scrollbar().y, px(300.));
    }

    #[vespertine::test]
    fn test_remeasure(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // Create a list with 10 items, each 100px tall. We'll keep a reference
        // to the item height so we can later change the height and assert how
        // `ListState` handles it.
        let item_height = Rc::new(Cell::new(100usize));
        let state = ListState::new(10, crate::ListAlignment::Top, px(10.));

        struct TestView {
            state: ListState,
            item_height: Rc<Cell<usize>>,
        }

        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let height = self.item_height.get();
                list(self.state.clone(), move |_, _, _| {
                    div().h(px(height as f32)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let state_clone = state.clone();
        let item_height_clone = item_height.clone();
        let view = cx.update(|_, cx| {
            cx.new(|_| TestView {
                state: state_clone,
                item_height: item_height_clone,
            })
        });

        // Simulate scrolling 40px inside the element with index 2. Since the
        // original item height is 100px, this equates to 40% inside the item.
        state.scroll_to(vespertine::ListOffset {
            item_ix: 2,
            offset_in_item: px(40.),
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 2);
        assert_eq!(offset.offset_in_item, px(40.));

        // Update the `item_height` to be 50px instead of 100px so we can assert
        // that the scroll position is proportionally preserved, that is,
        // instead of 40px from the top of item 2, it should be 20px, since the
        // item's height has been halved.
        item_height.set(50);
        state.remeasure();

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 2);
        assert_eq!(offset.offset_in_item, px(20.));
    }

    #[vespertine::test]
    fn test_remeasure_item_preserves_scroll_offset(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let item_height = Rc::new(Cell::new(100usize));
        let state = ListState::new(20, crate::ListAlignment::Top, px(10.));

        struct TestView {
            state: ListState,
            item_height: Rc<Cell<usize>>,
        }

        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let height = self.item_height.get();
                list(self.state.clone(), move |index, _, _| {
                    let height = if index == 5 { height } else { 100 };
                    div().h(px(height as f32)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let state_clone = state.clone();
        let item_height_clone = item_height.clone();
        let view = cx.update(|_, cx| {
            cx.new(|_| TestView {
                state: state_clone,
                item_height: item_height_clone,
            })
        });

        state.scroll_to(vespertine::ListOffset {
            item_ix: 5,
            offset_in_item: px(40.),
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        item_height.set(200);
        state.remeasure_items(5..6);

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 5);
        assert_eq!(offset.offset_in_item, px(40.));
    }

    #[vespertine::test]
    fn test_remeasure_then_scroll_does_not_revert_scroll_position(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let state = ListState::new(20, crate::ListAlignment::Top, px(10.));

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(100.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = {
            let state = state.clone();
            cx.update(|_, cx| cx.new(|_| TestView(state)))
        };

        state.scroll_to(vespertine::ListOffset {
            item_ix: 5,
            offset_in_item: px(40.),
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        state.remeasure_items(5..6);

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
            ..Default::default()
        });

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 5);
        assert_eq!(offset.offset_in_item, px(70.));

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 5);
        assert_eq!(
            offset.offset_in_item,
            px(70.),
            "scrolling after a remeasure should not be reverted by the stale pending scroll"
        );
    }

    #[vespertine::test]
    fn test_scroll_after_remeasure_clamps_to_shrunk_item_height(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let item_height = Rc::new(Cell::new(100usize));
        let state = ListState::new(20, crate::ListAlignment::Top, px(10.));

        struct TestView {
            state: ListState,
            item_height: Rc<Cell<usize>>,
        }

        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let height = self.item_height.get();
                list(self.state.clone(), move |index, _, _| {
                    let height = if index == 5 { height } else { 100 };
                    div().h(px(height as f32)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = {
            let state = state.clone();
            let item_height = item_height.clone();
            cx.update(|_, cx| cx.new(|_| TestView { state, item_height }))
        };

        state.scroll_to(vespertine::ListOffset {
            item_ix: 5,
            offset_in_item: px(40.),
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        // Item 5 shrinks from 100px to 50px and is remeasured...
        item_height.set(50);
        state.remeasure_items(5..6);

        // ...and then the user scrolls down by 30px before the next frame,
        // landing at offset 70.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
            ..Default::default()
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        // The rebased pending scroll clamps the user's offset to the item's
        // new height instead of leaving it pointing past the end of the item.
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 5);
        assert_eq!(offset.offset_in_item, px(50.));
    }

    #[vespertine::test]
    fn test_follow_tail_stays_at_bottom_as_items_grow(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items, each 50px tall → 500px total content, 200px viewport.
        // With follow-tail on, the list should always show the bottom.
        let item_height = Rc::new(Cell::new(50usize));
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.));

        struct TestView {
            state: ListState,
            item_height: Rc<Cell<usize>>,
        }
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let height = self.item_height.get();
                list(self.state.clone(), move |_, _, _| {
                    div().h(px(height as f32)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let state_clone = state.clone();
        let item_height_clone = item_height.clone();
        let view = cx.update(|_, cx| {
            cx.new(|_| TestView {
                state: state_clone,
                item_height: item_height_clone,
            })
        });

        state.set_follow_mode(FollowMode::Tail);

        // First paint — items are 50px, total 500px, viewport 200px.
        // Follow-tail should anchor to the end.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        // The scroll should be at the bottom: the last visible items fill the
        // 200px viewport from the end of 500px of content (offset 300px).
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 6);
        assert_eq!(offset.offset_in_item, px(0.));
        assert!(state.is_following_tail());

        // Simulate items growing (e.g. streaming content makes each item taller).
        // 10 items × 80px = 800px total.
        item_height.set(80);
        state.remeasure();

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        // After growth, follow-tail should have re-anchored to the new end.
        // 800px total − 200px viewport = 600px offset → item 7 at offset 40px,
        // but follow-tail anchors to item_count (10), and layout walks back to
        // fill 200px, landing at item 7 (7 × 80 = 560, 800 − 560 = 240 > 200,
        // so item 8: 8 × 80 = 640, 800 − 640 = 160 < 200 → keeps walking →
        // item 7: offset = 800 − 200 = 600, item_ix = 600/80 = 7, remainder 40).
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 7);
        assert_eq!(offset.offset_in_item, px(40.));
        assert!(state.is_following_tail());
    }

    #[vespertine::test]
    fn test_pause_following_tail_reengages_when_still_at_bottom(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items × 50px = 500px total, 200px viewport.
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.));

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));
        state.set_follow_mode(FollowMode::Tail);

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(state.is_following_tail());

        // Pausing while the view is still at the bottom (e.g. a no-op zoom)
        // must not strand follow-tail: the next layout has to re-engage so the
        // invariant "at the bottom + new content => visible" is preserved.
        state.pause_following_tail();
        assert!(!state.is_following_tail());

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });
        assert!(
            state.is_following_tail(),
            "pausing while at the bottom must re-engage follow-tail on the next layout"
        );
    }

    #[vespertine::test]
    fn test_pause_following_tail_freezes_off_bottom(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items, 200px viewport. Item height is adjustable to simulate a
        // diagram block growing/shrinking on zoom.
        let item_height = Rc::new(Cell::new(50usize));
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.));

        struct TestView {
            state: ListState,
            item_height: Rc<Cell<usize>>,
        }
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let height = self.item_height.get();
                list(self.state.clone(), move |_, _, _| {
                    div().h(px(height as f32)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| {
            cx.new(|_| TestView {
                state: state.clone(),
                item_height: item_height.clone(),
            })
        });
        state.set_follow_mode(FollowMode::Tail);

        // At the bottom: 500px content, 200px viewport → top at item 6.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert_eq!(state.logical_scroll_top().item_ix, 6);
        assert!(state.is_following_tail());

        // Pause, then grow items (a zoom-in that pushes content below the fold).
        // The frozen top must stay put rather than snapping to the new end, and
        // following stays paused since we're no longer at the bottom.
        state.pause_following_tail();
        item_height.set(80);
        state.remeasure();
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 6);
        assert_eq!(offset.offset_in_item, px(0.));
        assert!(
            !state.is_following_tail(),
            "a paused list must not re-engage while the frozen top is off the bottom"
        );

        // Shrink back (zoom-out) so the frozen top once again reaches the
        // bottom: follow-tail must re-engage on its own.
        item_height.set(50);
        state.remeasure();
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });
        assert!(
            state.is_following_tail(),
            "returning to the bottom must restore follow-tail"
        );
    }

    #[vespertine::test]
    fn test_follow_tail_disengages_on_user_scroll(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items × 50px = 500px total, 200px viewport.
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.));

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        state.set_follow_mode(FollowMode::Tail);

        // Paint with follow-tail — scroll anchored to the bottom.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, cx| {
            cx.new(|_| TestView(state.clone())).into_any_element()
        });
        assert!(state.is_following_tail());

        // Simulate the user scrolling up.
        // This should disengage follow-tail.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(100.))),
            ..Default::default()
        });

        assert!(
            !state.is_following_tail(),
            "follow-tail should disengage when the user scrolls toward the start"
        );
    }

    #[vespertine::test]
    fn test_follow_tail_disengages_on_scrollbar_reposition(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items × 50px = 500px total, 200px viewport.
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.)).measure_all();

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));

        state.set_follow_mode(FollowMode::Tail);

        // Paint with follow-tail — scroll anchored to the bottom.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(state.is_following_tail());

        // Simulate the scrollbar moving the viewport to the middle.
        state.set_offset_from_scrollbar(point(px(0.), px(-150.)));

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 3);
        assert_eq!(offset.offset_in_item, px(0.));
        assert!(
            !state.is_following_tail(),
            "follow-tail should disengage when the scrollbar manually repositions the list"
        );

        // A subsequent draw should preserve the user's manual position instead
        // of snapping back to the end.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 3);
        assert_eq!(offset.offset_in_item, px(0.));
    }

    #[vespertine::test]
    fn test_scrollbar_drag_with_growing_content(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        let last_item_height = Rc::new(Cell::new(50usize));
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.)).measure_all();

        struct TestView {
            state: ListState,
            last_item_height: Rc<Cell<usize>>,
        }
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let last_item_height = self.last_item_height.clone();
                list(self.state.clone(), move |index, _, _| {
                    let height = if index == 9 {
                        last_item_height.get()
                    } else {
                        50
                    };
                    div().h(px(height as f32)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| {
            cx.new(|_| TestView {
                state: state.clone(),
                last_item_height: last_item_height.clone(),
            })
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        state.scrollbar_drag_started();

        state.set_offset_from_scrollbar(point(px(0.), px(-150.)));
        let scrollbar_offset_before_growth = state.scroll_px_offset_for_scrollbar();

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 3);
        assert_eq!(offset.offset_in_item, px(0.));

        last_item_height.set(550);
        state.remeasure_items(9..10);
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        assert_eq!(state.max_offset_for_scrollbar().y, px(300.));
        assert_eq!(
            state.scroll_px_offset_for_scrollbar(),
            scrollbar_offset_before_growth
        );

        state.set_offset_from_scrollbar(point(px(0.), px(-150.)));
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 3);
        assert_eq!(offset.offset_in_item, px(0.));
    }

    #[vespertine::test]
    fn test_set_follow_tail_snaps_to_bottom(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items × 50px = 500px total, 200px viewport.
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.));

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));

        // Scroll to the middle of the list (item 3).
        state.scroll_to(vespertine::ListOffset {
            item_ix: 3,
            offset_in_item: px(0.),
        });

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 3);
        assert_eq!(offset.offset_in_item, px(0.));
        assert!(!state.is_following_tail());

        // Enable follow-tail — this should immediately snap the scroll anchor
        // to the end, like the user just sent a prompt.
        state.set_follow_mode(FollowMode::Tail);

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        // After paint, scroll should be at the bottom.
        // 500px total − 200px viewport = 300px offset → item 6, offset 0.
        let offset = state.logical_scroll_top();
        assert_eq!(offset.item_ix, 6);
        assert_eq!(offset.offset_in_item, px(0.));
        assert!(state.is_following_tail());
    }

    #[vespertine::test]
    fn test_bottom_aligned_scrollbar_offset_at_end(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        const ITEMS: usize = 10;
        const ITEM_SIZE: f32 = 50.0;

        let state = ListState::new(
            ITEMS,
            crate::ListAlignment::Bottom,
            px(ITEMS as f32 * ITEM_SIZE),
        );

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(ITEM_SIZE)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, cx| {
            cx.new(|_| TestView(state.clone())).into_any_element()
        });

        // Bottom-aligned lists start pinned to the end: logical_scroll_top returns
        // item_ix == item_count, meaning no explicit scroll position has been set.
        assert_eq!(state.logical_scroll_top().item_ix, ITEMS);

        let max_offset = state.max_offset_for_scrollbar();
        let scroll_offset = state.scroll_px_offset_for_scrollbar();

        assert_eq!(
            -scroll_offset.y, max_offset.y,
            "scrollbar offset ({}) should equal max offset ({}) when list is pinned to bottom",
            -scroll_offset.y, max_offset.y,
        );
    }

    /// When the user scrolls away from the bottom during follow_tail,
    /// follow_tail suspends. If they scroll back to the bottom, the
    /// next paint should re-engage follow_tail using fresh measurements.
    #[vespertine::test]
    fn test_follow_tail_reengages_when_scrolled_back_to_bottom(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items × 50px = 500px total, 200px viewport.
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.));

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));

        state.set_follow_mode(FollowMode::Tail);

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(state.is_following_tail());

        // Scroll up — follow_tail should suspend (not fully disengage).
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(50.))),
            ..Default::default()
        });
        assert!(!state.is_following_tail());

        // Scroll back down to the bottom.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-10000.))),
            ..Default::default()
        });

        // After a paint, follow_tail should re-engage because the
        // layout confirmed we're at the true bottom.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(
            state.is_following_tail(),
            "follow_tail should re-engage after scrolling back to the bottom"
        );
    }

    /// When an item is spliced to unmeasured (0px) while follow_tail
    /// is suspended, the re-engagement check should still work correctly
    #[vespertine::test]
    fn test_follow_tail_reengagement_not_fooled_by_unmeasured_items(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 20 items × 50px = 1000px total, 200px viewport, 1000px
        // overdraw so all items get measured during the follow_tail
        // paint (matching realistic production settings).
        let state = ListState::new(20, crate::ListAlignment::Top, px(1000.));

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));

        state.set_follow_mode(FollowMode::Tail);

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(state.is_following_tail());

        // Scroll up a meaningful amount — suspends follow_tail.
        // 20 items × 50px = 1000px. viewport 200px. scroll_max = 800px.
        // Scrolling up 200px puts us at 600px, clearly not at bottom.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(50.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(200.))),
            ..Default::default()
        });
        assert!(!state.is_following_tail());

        // Invalidate the last item (simulates EntryUpdated calling
        // remeasure_items). This makes items.summary().height
        // temporarily wrong (0px for the invalidated item).
        state.remeasure_items(19..20);

        // Paint — layout re-measures the invalidated item with its true
        // height. The re-engagement check uses these fresh measurements.
        // Since we scrolled 200px up from the 800px max, we're at
        // ~600px — NOT at the bottom, so follow_tail should NOT
        // re-engage.
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(
            !state.is_following_tail(),
            "follow_tail should not falsely re-engage due to an unmeasured item \
             reducing items.summary().height"
        );
    }

    #[vespertine::test]
    fn test_follow_tail_reengages_after_scrollbar_disengagement(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();

        // 10 items × 50px = 500px total, 200px viewport, scroll_max = 300px.
        let state = ListState::new(10, crate::ListAlignment::Top, px(0.)).measure_all();

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));

        state.set_follow_mode(FollowMode::Tail);
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(state.is_following_tail());

        // Drag the scrollbar up to the middle — follow_tail should suspend.
        state.set_offset_from_scrollbar(point(px(0.), px(-150.)));
        assert!(!state.is_following_tail());

        // Drag the scrollbar back to the bottom — follow_tail should re-engage
        // on the next paint.
        state.set_offset_from_scrollbar(point(px(0.), px(-300.)));
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });
        assert!(
            state.is_following_tail(),
            "follow_tail should re-engage after scrolling back to the bottom via the scrollbar"
        );
    }

    #[vespertine::test]
    fn test_follow_tail_reengages_after_scrollbar_drag_to_bottom_while_growing(
        cx: &mut TestAppContext,
    ) {
        let cx = cx.add_empty_window();

        let state = ListState::new(10, crate::ListAlignment::Top, px(0.)).measure_all();

        struct TestView(ListState);
        impl Render for TestView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.0.clone(), |_, _, _| {
                    div().h(px(50.)).w_full().into_any()
                })
                .w_full()
                .h_full()
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| TestView(state.clone())));

        state.set_follow_mode(FollowMode::Tail);
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });
        assert!(state.is_following_tail());

        state.scrollbar_drag_started();

        state.splice(10..10, 10);
        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.clone().into_any_element()
        });

        state.set_offset_from_scrollbar(point(px(0.), px(-300.)));
        state.scrollbar_drag_ended();

        cx.draw(point(px(0.), px(0.)), size(px(100.), px(200.)), |_, _| {
            view.into_any_element()
        });

        assert!(
            state.is_following_tail(),
            "follow_tail should re-engage when the user drags the scrollbar to \
             the bottom of its track, even when content has grown during the drag \
             (so frozen_bottom < live_bottom)"
        );
    }
}
