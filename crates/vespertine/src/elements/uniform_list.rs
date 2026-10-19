//! A scrollable list of elements with uniform height, optimized for large lists.
//! Rather than use the full taffy layout system, uniform_list simply measures
//! the first element and then lays out all remaining elements in a line based on that
//! measurement. This is much faster than the full layout system, but only works for
//! elements with uniform height.

use crate::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Element, ElementId, Entity,
    GlobalElementId, Hitbox, InspectorElementId, InteractiveElement, Interactivity, IntoElement,
    IsZero, LayoutId, ListSizingBehavior, Overflow, Pixels, Point, ScrollHandle, Size,
    StyleRefinement, Styled, Window, point, px, size,
    window::{
        read_tracker::ViewKey,
        retained::Placement,
        rows::{
            EXTENSION_BUDGET_VIEWPORTS, MeasureSample, OVERSCAN_VIEWPORTS, RowGeometry, row_offset,
        },
    },
};
use smallvec::SmallVec;
use std::{any::TypeId, cell::RefCell, cmp, collections::BTreeMap, ops::Range, rc::Rc};

use super::ListHorizontalSizingBehavior;

/// uniform_list provides lazy rendering for a set of items that are of uniform height.
/// When rendered into a container with overflow-y: hidden and a fixed (or max) height,
/// uniform_list will only render the visible subset of items.
#[track_caller]
pub fn uniform_list<R>(
    id: impl Into<ElementId>,
    item_count: usize,
    f: impl 'static + Fn(Range<usize>, &mut Window, &mut App) -> Vec<R>,
) -> UniformList
where
    R: IntoElement,
{
    let id = id.into();
    let mut base_style = StyleRefinement::default();
    base_style.overflow.y = Some(Overflow::Scroll);
    let render_type = type_of(&f);

    let render_range = move |range: Range<usize>, window: &mut Window, cx: &mut App| {
        f(range, window, cx)
            .into_iter()
            .map(|component| component.into_any_element())
            .collect()
    };

    UniformList {
        item_count,
        item_to_measure_index: 0,
        render_type,
        render_items: Box::new(render_range),
        decorations: Vec::new(),
        interactivity: Interactivity {
            element_id: Some(id),
            base_style: Box::new(base_style),
            ..Interactivity::new()
        },
        scroll_handle: None,
        sizing_behavior: ListSizingBehavior::default(),
        horizontal_sizing_behavior: ListHorizontalSizingBehavior::default(),
    }
}

/// A list element for efficiently laying out and displaying a list of uniform-height elements.
pub struct UniformList {
    item_count: usize,
    item_to_measure_index: usize,
    /// The type of the row callback: a list whose callback is a different closure than
    /// last frame cannot reuse the rows the old one rendered (PRF-005 R5).
    render_type: TypeId,
    render_items: Box<
        dyn for<'a> Fn(Range<usize>, &'a mut Window, &'a mut App) -> SmallVec<[AnyElement; 64]>,
    >,
    decorations: Vec<Box<dyn UniformListDecoration>>,
    interactivity: Interactivity,
    scroll_handle: Option<UniformListScrollHandle>,
    sizing_behavior: ListSizingBehavior,
    horizontal_sizing_behavior: ListHorizontalSizingBehavior,
}

/// Frame state used by the [UniformList].
pub struct UniformListFrameState {
    items: SmallVec<[AnyElement; 32]>,
    decorations: SmallVec<[AnyElement; 2]>,
    /// Retained rows of this frame, in index order (PRF-005).
    rows: Vec<RowPlan>,
    row_paint: Option<RowPaint>,
}

type RenderItems =
    dyn for<'a> Fn(Range<usize>, &'a mut Window, &'a mut App) -> SmallVec<[AnyElement; 64]>;

/// The parts of a list its rows are planned from; borrowed separately from the list's
/// interactivity, which is in use while the rows are prepared.
struct RowSource<'a> {
    item_count: usize,
    render_type: TypeId,
    render_items: &'a RenderItems,
}

fn type_of<T: 'static>(_: &T) -> TypeId {
    TypeId::of::<T>()
}

/// What a frame does with one retained row.
struct RowPlan {
    ix: usize,
    key: ViewKey,
    /// The element of a row built in this frame; `None` for a replayed row.
    element: Option<AnyElement>,
    placement: Placement,
    /// The row reaches the visible viewport.
    shown: bool,
}

/// What painting the rows needs from prepaint.
struct RowPaint {
    extent: ContentMask<Pixels>,
}

/// The measurements of one frame of row planning.
struct RowFrame {
    list: GlobalElementId,
    /// The same row geometry the direct path computes, so both place rows identically.
    padded_origin_y: Pixels,
    row_extent: f64,
    scroll_top: f64,
    extent: ContentMask<Pixels>,
    clip: Bounds<Pixels>,
    scale: f32,
    content_origin: Point<Pixels>,
    item_height: Pixels,
    available_width: Pixels,
}

/// A handle for controlling the scroll position of a uniform list.
/// This should be stored in your view and passed to the uniform_list on each frame.
#[derive(Clone, Debug, Default)]
pub struct UniformListScrollHandle(pub Rc<RefCell<UniformListScrollState>>);

/// Where to place the element scrolled to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollStrategy {
    /// Place the element at the top of the list's viewport.
    Top,
    /// Attempt to place the element in the middle of the list's viewport.
    /// May not be possible if there's not enough list items above the item scrolled to:
    /// in this case, the element will be placed at the closest possible position.
    Center,
    /// Attempt to place the element at the bottom of the list's viewport.
    /// May not be possible if there's not enough list items above the item scrolled to:
    /// in this case, the element will be placed at the closest possible position.
    Bottom,
    /// If the element is not visible attempt to place it at:
    /// - The top of the list's viewport if the target element is above currently visible elements.
    /// - The bottom of the list's viewport if the target element is above currently visible elements.
    Nearest,
}

#[derive(Clone, Copy, Debug)]
#[allow(missing_docs)]
pub struct DeferredScrollToItem {
    /// The item index to scroll to
    pub item_index: usize,
    /// The scroll strategy to use
    pub strategy: ScrollStrategy,
    /// The offset in number of items
    pub offset: usize,
    pub scroll_strict: bool,
}

#[derive(Clone, Debug, Default)]
#[allow(missing_docs)]
pub struct UniformListScrollState {
    pub base_handle: ScrollHandle,
    pub deferred_scroll_to_item: Option<DeferredScrollToItem>,
    /// Size of the item, captured during last layout.
    pub last_item_size: Option<ItemSize>,
    /// Whether the list was vertically flipped during last layout.
    pub y_flipped: bool,
}

#[derive(Copy, Clone, Debug, Default)]
/// The size of the item and its contents.
pub struct ItemSize {
    /// The size of the item.
    pub item: Size<Pixels>,
    /// The size of the item's contents, which may be larger than the item itself,
    /// if the item was bounded by a parent element.
    pub contents: Size<Pixels>,
}

impl UniformListScrollHandle {
    /// Create a new scroll handle to bind to a uniform list.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(UniformListScrollState {
            base_handle: ScrollHandle::new(),
            deferred_scroll_to_item: None,
            last_item_size: None,
            y_flipped: false,
        })))
    }

    /// Scroll the list so that the given item index is visible.
    ///
    /// This uses non-strict scrolling: if the item is already fully visible, no scrolling occurs.
    /// If the item is out of view, it scrolls the minimum amount to bring it into view according
    /// to the strategy.
    pub fn scroll_to_item(&self, ix: usize, strategy: ScrollStrategy) {
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset: 0,
            scroll_strict: false,
        });
    }

    /// Scroll the list so that the given item index is at scroll strategy position.
    ///
    /// This uses strict scrolling: the item will always be scrolled to match the strategy position,
    /// even if it's already visible. Use this when you need precise positioning.
    pub fn scroll_to_item_strict(&self, ix: usize, strategy: ScrollStrategy) {
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset: 0,
            scroll_strict: true,
        });
    }

    /// Scroll the list to the given item index with an offset in number of items.
    ///
    /// This uses non-strict scrolling: if the item is already visible within the offset region,
    /// no scrolling occurs.
    ///
    /// The offset parameter shrinks the effective viewport by the specified number of items
    /// from the corresponding edge, then applies the scroll strategy within that reduced viewport:
    /// - `ScrollStrategy::Top`: Shrinks from top, positions item at the new top
    /// - `ScrollStrategy::Center`: Shrinks from top, centers item in the reduced viewport
    /// - `ScrollStrategy::Bottom`: Shrinks from bottom, positions item at the new bottom
    pub fn scroll_to_item_with_offset(&self, ix: usize, strategy: ScrollStrategy, offset: usize) {
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset,
            scroll_strict: false,
        });
    }

    /// Scroll the list so that the given item index is at the exact scroll strategy position with an offset.
    ///
    /// This uses strict scrolling: the item will always be scrolled to match the strategy position,
    /// even if it's already visible.
    ///
    /// The offset parameter shrinks the effective viewport by the specified number of items
    /// from the corresponding edge, then applies the scroll strategy within that reduced viewport:
    /// - `ScrollStrategy::Top`: Shrinks from top, positions item at the new top
    /// - `ScrollStrategy::Center`: Shrinks from top, centers item in the reduced viewport
    /// - `ScrollStrategy::Bottom`: Shrinks from bottom, positions item at the new bottom
    pub fn scroll_to_item_strict_with_offset(
        &self,
        ix: usize,
        strategy: ScrollStrategy,
        offset: usize,
    ) {
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset,
            scroll_strict: true,
        });
    }

    /// Check if the list is flipped vertically.
    pub fn y_flipped(&self) -> bool {
        self.0.borrow().y_flipped
    }

    /// Get the index of the topmost visible child.
    #[cfg(any(test, feature = "test-support"))]
    pub fn logical_scroll_top_index(&self) -> usize {
        let this = self.0.borrow();
        this.deferred_scroll_to_item
            .as_ref()
            .map(|deferred| deferred.item_index)
            .unwrap_or_else(|| this.base_handle.logical_scroll_top().0)
    }

    /// Checks if the list can be scrolled vertically.
    pub fn is_scrollable(&self) -> bool {
        if let Some(size) = self.0.borrow().last_item_size {
            size.contents.height > size.item.height
        } else {
            false
        }
    }

    /// Whether the list is scrolled to the end, or `None` if the list is
    /// not scrollable.
    pub fn is_scrolled_to_end(&self) -> Option<bool> {
        let state = self.0.borrow();
        let max_offset = state.base_handle.max_offset();
        if max_offset.y <= px(0.) {
            return None;
        }
        let offset = state.base_handle.offset();
        Some(-offset.y >= max_offset.y)
    }

    /// Scroll to the bottom of the list.
    pub fn scroll_to_bottom(&self) {
        self.scroll_to_item(usize::MAX, ScrollStrategy::Bottom);
    }
}

impl Styled for UniformList {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl Element for UniformList {
    type RequestLayoutState = UniformListFrameState;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.interactivity.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.interactivity.write_a11y_info(node);
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let max_items = self.item_count;
        let item_size = self.measure_item_retained(global_id, window, cx);
        let layout_id = self.interactivity.request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |style, window, cx| match self.sizing_behavior {
                ListSizingBehavior::Infer => {
                    window.with_text_style(style.text_style().cloned(), |window| {
                        window.request_measured_layout(
                            style,
                            move |known_dimensions, available_space, _window, _cx| {
                                let desired_height = item_size.height * max_items;
                                let width = known_dimensions.width.unwrap_or(match available_space
                                    .width
                                {
                                    AvailableSpace::Definite(x) => x,
                                    AvailableSpace::MinContent | AvailableSpace::MaxContent => {
                                        item_size.width
                                    }
                                });
                                let height = match available_space.height {
                                    AvailableSpace::Definite(height) => desired_height.min(height),
                                    AvailableSpace::MinContent | AvailableSpace::MaxContent => {
                                        desired_height
                                    }
                                };
                                size(width, height)
                            },
                        )
                    })
                }
                ListSizingBehavior::Auto => window
                    .with_text_style(style.text_style().cloned(), |window| {
                        window.request_layout(style, None, cx)
                    }),
            },
        );

        (
            layout_id,
            UniformListFrameState {
                items: SmallVec::new(),
                decorations: SmallVec::new(),
                rows: Vec::new(),
                row_paint: None,
            },
        )
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        frame_state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        let style = self
            .interactivity
            .compute_style(global_id, None, window, cx);
        let border = style.border_widths.to_pixels(window.rem_size());
        let padding = style
            .padding
            .to_pixels(bounds.size.into(), window.rem_size());

        let padded_bounds = Bounds::from_corners(
            bounds.origin + point(border.left + padding.left, border.top + padding.top),
            bounds.bottom_right()
                - point(border.right + padding.right, border.bottom + padding.bottom),
        );

        let can_scroll_horizontally = matches!(
            self.horizontal_sizing_behavior,
            ListHorizontalSizingBehavior::Unconstrained
        );

        let longest_item_size = self.measure_item_retained(global_id, window, cx);
        let content_width = if can_scroll_horizontally {
            padded_bounds.size.width.max(longest_item_size.width)
        } else {
            padded_bounds.size.width
        };
        let content_size = Size {
            width: content_width,
            height: longest_item_size.height * self.item_count,
        };

        let shared_scroll_offset = self.interactivity.scroll_offset.clone().unwrap();
        let item_height = longest_item_size.height;
        let shared_scroll_to_item = self.scroll_handle.as_mut().and_then(|handle| {
            let mut handle = handle.0.borrow_mut();
            handle.last_item_size = Some(ItemSize {
                item: padded_bounds.size,
                contents: content_size,
            });
            handle.deferred_scroll_to_item.take()
        });

        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            content_size,
            window,
            cx,
            |_style, mut scroll_offset, hitbox, window, cx| {
                let y_flipped = if let Some(scroll_handle) = &self.scroll_handle {
                    let scroll_state = scroll_handle.0.borrow();
                    scroll_state.y_flipped
                } else {
                    false
                };

                if self.item_count > 0 {
                    let content_height = item_height * self.item_count;

                    let is_scrolled_vertically = !scroll_offset.y.is_zero();
                    let max_scroll_offset = padded_bounds.size.height - content_height;

                    if is_scrolled_vertically && scroll_offset.y < max_scroll_offset {
                        shared_scroll_offset.borrow_mut().y = max_scroll_offset;
                        scroll_offset.y = max_scroll_offset;
                    }

                    let content_width = content_size.width + padding.left + padding.right;
                    let is_scrolled_horizontally =
                        can_scroll_horizontally && !scroll_offset.x.is_zero();
                    if is_scrolled_horizontally && content_width <= padded_bounds.size.width {
                        shared_scroll_offset.borrow_mut().x = Pixels::ZERO;
                        scroll_offset.x = Pixels::ZERO;
                    }

                    if let Some(DeferredScrollToItem {
                        mut item_index,
                        mut strategy,
                        offset,
                        scroll_strict,
                    }) = shared_scroll_to_item
                    {
                        if y_flipped {
                            item_index = self.item_count.saturating_sub(item_index + 1);
                        }
                        let list_height = padded_bounds.size.height;
                        let mut updated_scroll_offset = shared_scroll_offset.borrow_mut();
                        let item_top = item_height * item_index;
                        let item_bottom = item_top + item_height;
                        let scroll_top = -updated_scroll_offset.y;
                        let offset_pixels = item_height * offset;

                        // is the selected item above/below currently visible items
                        let is_above = item_top < scroll_top + offset_pixels;
                        let is_below = item_bottom > scroll_top + list_height;

                        if scroll_strict || is_above || is_below {
                            if strategy == ScrollStrategy::Nearest {
                                if is_above {
                                    strategy = ScrollStrategy::Top;
                                } else if is_below {
                                    strategy = ScrollStrategy::Bottom;
                                }
                            }

                            let max_scroll_offset =
                                (content_height - list_height).max(Pixels::ZERO);
                            match strategy {
                                ScrollStrategy::Top => {
                                    updated_scroll_offset.y = -(item_top - offset_pixels)
                                        .clamp(Pixels::ZERO, max_scroll_offset);
                                }
                                ScrollStrategy::Center => {
                                    let item_center = item_top + item_height / 2.0;

                                    let viewport_height = list_height - offset_pixels;
                                    let viewport_center = offset_pixels + viewport_height / 2.0;
                                    let target_scroll_top = item_center - viewport_center;
                                    updated_scroll_offset.y =
                                        -target_scroll_top.clamp(Pixels::ZERO, max_scroll_offset);
                                }
                                ScrollStrategy::Bottom => {
                                    updated_scroll_offset.y = -(item_bottom - list_height)
                                        .clamp(Pixels::ZERO, max_scroll_offset);
                                }
                                ScrollStrategy::Nearest => {
                                    // Nearest, but the item is visible -> no scroll is required
                                }
                            }
                        }
                        scroll_offset = *updated_scroll_offset
                    }

                    // Scroll offsets place content on the device pixel grid (PRF-004 R3); the
                    // stored offset keeps its sub-pixel motion.
                    let snapped_offset = point(
                        window.pixel_snap(scroll_offset.x),
                        window.pixel_snap(scroll_offset.y),
                    );
                    let scroll_top = -snapped_offset.y.to_f64();
                    let item_height_f64 = item_height.to_f64();
                    let first_visible_element_ix =
                        ((scroll_top - padding.top.to_f64()) / item_height_f64).floor() as usize;
                    let last_visible_element_ix = ((scroll_top
                        + padded_bounds.size.height.to_f64())
                        / item_height_f64)
                        .ceil() as usize;

                    let visible_range = first_visible_element_ix
                        ..cmp::min(last_visible_element_ix, self.item_count);

                    let content_mask = ContentMask { bounds };
                    let retained_rows = !y_flipped && global_id.is_some() && window.rows_enabled();
                    if retained_rows {
                        let visible = match global_id {
                            Some(list) => prepaint_rows(
                                &RowSource {
                                    item_count: self.item_count,
                                    render_type: self.render_type,
                                    render_items: &*self.render_items,
                                },
                                list,
                                bounds,
                                padded_bounds,
                                padding.top,
                                scroll_offset,
                                item_height,
                                can_scroll_horizontally,
                                frame_state,
                                window,
                                cx,
                            ),
                            None => visible_range.clone(),
                        };
                        window.with_content_mask(Some(content_mask), |window| {
                            prepaint_decorations(
                                &self.decorations,
                                self.item_count,
                                visible,
                                padded_bounds,
                                scroll_offset,
                                item_height,
                                frame_state,
                                window,
                                cx,
                            );
                        });
                    } else {
                        if let Some(list) = global_id {
                            window.row_store_clear(list);
                        }
                        let items = if y_flipped {
                            let flipped_range = self.item_count.saturating_sub(visible_range.end)
                                ..self.item_count.saturating_sub(visible_range.start);
                            let mut items = (self.render_items)(flipped_range, window, cx);
                            items.reverse();
                            items
                        } else {
                            (self.render_items)(visible_range.clone(), window, cx)
                        };

                        window.with_content_mask(Some(content_mask), |window| {
                            for (mut item, ix) in items.into_iter().zip(visible_range.clone()) {
                                let item_top = item_height_f64 * ix as f64 - scroll_top;
                                let item_origin = padded_bounds.origin
                                    + point(snapped_offset.x, Pixels::from(item_top));

                                let available_width = if can_scroll_horizontally {
                                    padded_bounds.size.width + scroll_offset.x.abs()
                                } else {
                                    padded_bounds.size.width
                                };
                                let available_space = size(
                                    AvailableSpace::Definite(available_width),
                                    AvailableSpace::Definite(item_height),
                                );
                                window.with_layout_item_index(ix, |window| {
                                    item.layout_as_root(available_space, window, cx)
                                });
                                item.prepaint_at(item_origin, window, cx);
                                frame_state.items.push(item);
                            }

                            prepaint_decorations(
                                &self.decorations,
                                self.item_count,
                                visible_range.clone(),
                                padded_bounds,
                                scroll_offset,
                                item_height,
                                frame_state,
                                window,
                                cx,
                            );
                        });
                    }
                }

                hitbox
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<crate::Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.interactivity.paint(
            global_id,
            inspector_id,
            bounds,
            hitbox.as_ref(),
            window,
            cx,
            |_, window, cx| {
                for item in &mut request_layout.items {
                    item.paint(window, cx);
                }
                paint_rows(request_layout, window, cx);
                for decoration in &mut request_layout.decorations {
                    decoration.paint(window, cx);
                }
            },
        )
    }
}

impl IntoElement for UniformList {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A decoration for a [`UniformList`]. This can be used for various things,
/// such as rendering indent guides, or other visual effects.
pub trait UniformListDecoration {
    /// Compute the decoration element, given the visible range of list items,
    /// the bounds of the list, and the height of each item.
    fn compute(
        &self,
        visible_range: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}

impl<T: UniformListDecoration + 'static> UniformListDecoration for Entity<T> {
    fn compute(
        &self,
        visible_range: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.update(cx, |inner, cx| {
            inner.compute(
                visible_range,
                bounds,
                scroll_offset,
                item_height,
                item_count,
                window,
                cx,
            )
        })
    }
}

impl UniformList {
    /// Selects a specific list item for measurement.
    pub fn with_width_from_item(mut self, item_index: Option<usize>) -> Self {
        self.item_to_measure_index = item_index.unwrap_or(0);
        self
    }

    /// Sets the sizing behavior, similar to the `List` element.
    pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self {
        self.sizing_behavior = behavior;
        self
    }

    /// Sets the horizontal sizing behavior, controlling the way list items laid out horizontally.
    /// With [`ListHorizontalSizingBehavior::Unconstrained`] behavior, every item and the list itself will
    /// have the size of the widest item and lay out pushing the `end_slot` to the right end.
    pub fn with_horizontal_sizing_behavior(
        mut self,
        behavior: ListHorizontalSizingBehavior,
    ) -> Self {
        self.horizontal_sizing_behavior = behavior;
        match behavior {
            ListHorizontalSizingBehavior::FitList => {
                self.interactivity.base_style.overflow.x = None;
            }
            ListHorizontalSizingBehavior::Unconstrained => {
                self.interactivity.base_style.overflow.x = Some(Overflow::Scroll);
            }
        }
        self
    }

    /// Adds a decoration element to the list.
    pub fn with_decoration(mut self, decoration: impl UniformListDecoration + 'static) -> Self {
        self.decorations.push(Box::new(decoration));
        self
    }

    /// Measures the item that gives the list its size, reusing the sample of an earlier frame
    /// while the content, the item count and the constraints it was measured under are
    /// unchanged (PRF-005 R7).
    fn measure_item_retained(
        &self,
        list: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        let Some(list) = list.filter(|_| self.item_count > 0 && window.rows_enabled()) else {
            return self.measure_item(None, window, cx);
        };
        let ambiguous = window.rows_owner_ambiguous();
        let cached = window
            .row_store(list)
            .measure
            .as_ref()
            .filter(|sample| sample.item_count == self.item_count && sample.width.is_none())
            .map(|sample| (sample.key, sample.size));
        if let Some((key, size)) = cached
            && !ambiguous
            && window.row_sample_valid(key)
        {
            window.row_touch(key);
            return size;
        }
        let key = window.row_record(list, usize::MAX);
        window.row_begin(key);
        let size = self.measure_item(None, window, cx);
        window.row_layout_done(key);
        window.row_finish_measure(key);
        window.row_store(list).measure = Some(MeasureSample {
            key,
            item_count: self.item_count,
            width: None,
            size,
        });
        size
    }

    fn measure_item(
        &self,
        list_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        if self.item_count == 0 {
            return Size::default();
        }

        let item_ix = cmp::min(self.item_to_measure_index, self.item_count - 1);
        let mut items = (self.render_items)(item_ix..item_ix + 1, window, cx);
        let Some(mut item_to_measure) = items.pop() else {
            return Size::default();
        };
        let available_space = size(
            list_width.map_or(AvailableSpace::MaxContent, |width| {
                AvailableSpace::Definite(width)
            }),
            AvailableSpace::MinContent,
        );
        item_to_measure.layout_as_root(available_space, window, cx)
    }

    /// Track and render scroll state of this list with reference to the given scroll handle.
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.interactivity.tracked_scroll_handle = Some(handle.0.borrow().base_handle.clone());
        self.scroll_handle = Some(handle.clone());
        self
    }

    /// Sets whether the list is flipped vertically, such that item 0 appears at the bottom.
    pub fn y_flipped(mut self, y_flipped: bool) -> Self {
        if let Some(ref scroll_handle) = self.scroll_handle {
            let mut scroll_state = scroll_handle.0.borrow_mut();
            let mut base_handle = &scroll_state.base_handle;
            let offset = base_handle.offset();
            match scroll_state.last_item_size {
                Some(last_size) if scroll_state.y_flipped != y_flipped => {
                    let new_y_offset =
                        -(offset.y + last_size.contents.height - last_size.item.height);
                    base_handle.set_offset(point(offset.x, new_y_offset));
                    scroll_state.y_flipped = y_flipped;
                }
                // Handle case where list is initially flipped.
                None if y_flipped => {
                    base_handle.set_offset(point(offset.x, Pixels::MIN));
                    scroll_state.y_flipped = y_flipped;
                }
                _ => {}
            }
        }
        self
    }
}

impl InteractiveElement for UniformList {
    fn interactivity(&mut self) -> &mut crate::Interactivity {
        &mut self.interactivity
    }
}

#[cfg(test)]
mod test {
    use crate::TestAppContext;

    #[vespertine::test]
    fn test_scroll_strategy_nearest(cx: &mut TestAppContext) {
        use crate::{
            Context, FocusHandle, ScrollStrategy, UniformListScrollHandle, Window, div, prelude::*,
            px, uniform_list,
        };
        use std::ops::Range;

        actions!(example, [SelectNext, SelectPrev]);

        struct TestView {
            index: usize,
            length: usize,
            scroll_handle: UniformListScrollHandle,
            focus_handle: FocusHandle,
        }

        impl TestView {
            /// The rows the list shows, read from its scroll offset: retained rows render only
            /// the rows that changed, so the row callback no longer reports the visible range
            /// (PRF-005).
            fn visible(&self) -> Range<usize> {
                let top = (-self.scroll_handle.0.borrow().base_handle.offset().y / px(20.0))
                    .floor()
                    .max(0.0) as usize;
                top..(top + 10).min(self.length)
            }

            pub fn select_next(
                &mut self,
                _: &SelectNext,
                window: &mut Window,
                _: &mut Context<Self>,
            ) {
                if self.index + 1 == self.length {
                    self.index = 0
                } else {
                    self.index += 1;
                }
                self.scroll_handle
                    .scroll_to_item(self.index, ScrollStrategy::Nearest);
                window.refresh();
            }

            pub fn select_previous(
                &mut self,
                _: &SelectPrev,
                window: &mut Window,
                _: &mut Context<Self>,
            ) {
                if self.index == 0 {
                    self.index = self.length - 1
                } else {
                    self.index -= 1;
                }
                self.scroll_handle
                    .scroll_to_item(self.index, ScrollStrategy::Nearest);
                window.refresh();
            }
        }

        impl Render for TestView {
            fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .id("list-example")
                    .track_focus(&self.focus_handle)
                    .on_action(cx.listener(Self::select_next))
                    .on_action(cx.listener(Self::select_previous))
                    .size_full()
                    .child(
                        uniform_list(
                            "entries",
                            self.length,
                            cx.processor(|_this, range: Range<usize>, _window, _cx| {
                                range
                                    .map(|ix| div().id(ix).h(px(20.0)).child(format!("Item {ix}")))
                                    .collect()
                            }),
                        )
                        .track_scroll(&self.scroll_handle)
                        .h(px(200.0)),
                    )
            }
        }

        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            window.focus(&focus_handle, cx);
            TestView {
                scroll_handle: UniformListScrollHandle::new(),
                index: 0,
                focus_handle,
                length: 47,
            }
        });

        // 10 out of 47 items are visible

        // First 9 times selecting next item does not scroll
        for ix in 1..10 {
            cx.dispatch_action(SelectNext);
            view.read_with(cx, |view, _| {
                assert_eq!(view.index, ix);
                assert_eq!(view.visible(), 0..10);
            })
        }

        // Now each time the list scrolls down by 1
        for ix in 10..47 {
            cx.dispatch_action(SelectNext);
            view.read_with(cx, |view, _| {
                assert_eq!(view.index, ix);
                assert_eq!(view.visible(), ix - 9..ix + 1);
            })
        }

        // After the last item we move back to the start
        cx.dispatch_action(SelectNext);
        view.read_with(cx, |view, _| {
            assert_eq!(view.index, 0);
            assert_eq!(view.visible(), 0..10);
        });

        // Return to the last element
        cx.dispatch_action(SelectPrev);
        view.read_with(cx, |view, _| {
            assert_eq!(view.index, 46);
            assert_eq!(view.visible(), 37..47);
        });

        // First 9 times selecting previous does not scroll
        for ix in (37..46).rev() {
            cx.dispatch_action(SelectPrev);
            view.read_with(cx, |view, _| {
                assert_eq!(view.index, ix);
                assert_eq!(view.visible(), 37..47);
            })
        }

        // Now each time the list scrolls up by 1
        for ix in (0..37).rev() {
            cx.dispatch_action(SelectPrev);
            view.read_with(cx, |view, _| {
                assert_eq!(view.index, ix);
                assert_eq!(view.visible(), ix..ix + 10);
            })
        }
    }

    #[vespertine::test]
    fn test_item_positions_far_down_long_list(cx: &mut TestAppContext) {
        use crate::{
            Bounds, Context, Pixels, UniformListScrollHandle, Window, canvas, point, prelude::*,
            px, uniform_list,
        };
        use std::{cell::RefCell, ops::Range, rc::Rc};

        struct TestView {
            scroll_handle: UniformListScrollHandle,
            item_bounds: Rc<RefCell<Vec<(usize, Bounds<Pixels>)>>>,
        }

        impl Render for TestView {
            fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let item_bounds = self.item_bounds.clone();
                uniform_list(
                    "entries",
                    10_000_000,
                    cx.processor(move |_, range: Range<usize>, _window, _cx| {
                        range
                            .map(|ix| {
                                let item_bounds = item_bounds.clone();
                                canvas(
                                    move |bounds, _, _| item_bounds.borrow_mut().push((ix, bounds)),
                                    |_, _, _, _| {},
                                )
                                .w_full()
                                .h(px(24.0))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll_handle)
                .h(px(200.0))
            }
        }

        let item_bounds = Rc::new(RefCell::new(Vec::new()));
        let (view, cx) = cx.add_window_view(|_, _| TestView {
            scroll_handle: UniformListScrollHandle::new(),
            item_bounds: item_bounds.clone(),
        });

        // Past 2^27px, f32 can only represent multiples of 16px. This offset is 16px into item
        // 9_000_002, and the items must still be laid out 24px apart from that point.
        item_bounds.borrow_mut().clear();
        view.update(cx, |view, cx| {
            view.scroll_handle
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.0), px(-216_000_064.0)));
            cx.notify();
        });
        cx.run_until_parked();

        let item_tops = item_bounds
            .borrow()
            .iter()
            .map(|(ix, bounds)| (*ix, bounds.origin.y))
            .collect::<Vec<_>>();
        let expected_tops = (9_000_002..9_000_011)
            .map(|ix| (ix, px(-16.0 + 24.0 * (ix - 9_000_002) as f32)))
            .collect::<Vec<_>>();
        assert_eq!(item_tops, expected_tops);
    }
}

/// The decorations of the visible range, prepainted over the rows.
#[allow(clippy::too_many_arguments)]
fn prepaint_decorations(
    decorations: &[Box<dyn UniformListDecoration>],
    item_count: usize,
    visible_range: Range<usize>,
    padded_bounds: Bounds<Pixels>,
    scroll_offset: Point<Pixels>,
    item_height: Pixels,
    frame_state: &mut UniformListFrameState,
    window: &mut Window,
    cx: &mut App,
) {
    let bounds = Bounds::new(padded_bounds.origin + scroll_offset, padded_bounds.size);
    for decoration in decorations {
        let mut decoration = decoration.as_ref().compute(
            visible_range.clone(),
            bounds,
            scroll_offset,
            item_height,
            item_count,
            window,
            cx,
        );
        let available_space = size(
            AvailableSpace::Definite(bounds.size.width),
            AvailableSpace::Definite(bounds.size.height),
        );
        decoration.layout_as_root(available_space, window, cx);
        decoration.prepaint_at(bounds.origin, window, cx);
        frame_state.decorations.push(decoration);
    }
}

/// Plans and prepares the retained rows of the frame: replays the rows that did not change,
/// builds the missing and changed visible rows, then fills the overscan within the frame's
/// budget (PRF-005 R1-R7). Returns the visible rows.
#[allow(clippy::too_many_arguments)]
fn prepaint_rows(
    source: &RowSource,
    list: &GlobalElementId,
    bounds: Bounds<Pixels>,
    padded_bounds: Bounds<Pixels>,
    padding_top: Pixels,
    scroll_offset: Point<Pixels>,
    item_height: Pixels,
    can_scroll_horizontally: bool,
    frame_state: &mut UniformListFrameState,
    window: &mut Window,
    cx: &mut App,
) -> Range<usize> {
    let scale = window.scale_factor();
    let item_count = source.item_count;
    let snapped = point(
        window.pixel_snap(scroll_offset.x),
        window.pixel_snap(scroll_offset.y),
    );
    let content_origin = padded_bounds.origin + snapped;
    let viewport = padded_bounds.size.height.to_f64();
    let scroll_top = (-snapped.y).to_f64();
    let row_extent = item_height.to_f64();
    let first = (((scroll_top - padding_top.to_f64()) / row_extent)
        .floor()
        .max(0.0) as usize)
        .min(item_count);
    let last = (((scroll_top + viewport) / row_extent).ceil() as usize).min(item_count);
    let visible = first.min(last)..last;
    let available_width = if can_scroll_horizontally {
        padded_bounds.size.width + scroll_offset.x.abs()
    } else {
        padded_bounds.size.width
    };
    let geometry = RowGeometry {
        item_count,
        item_height,
        bounds: padded_bounds,
        available_width,
        scale_factor: scale,
    };
    if !window.retention_reuse_allowed()
        || window.row_store(list).geometry.as_ref() != Some(&geometry)
    {
        window.row_store_clear(list);
    }
    let ambiguous = window.rows_owner_ambiguous()
        || window.row_store(list).render_type != Some(source.render_type);
    let (mut rows, constructed, last_top, was_moving_down) = {
        let store = window.row_store(list);
        store.geometry = Some(geometry);
        store.render_type = Some(source.render_type);
        (
            std::mem::take(&mut store.rows),
            store.constructed,
            store.last_scroll_top,
            store.moving_down,
        )
    };
    let moving_down = if scroll_top > last_top {
        true
    } else if scroll_top < last_top {
        false
    } else {
        was_moving_down
    };
    if ambiguous {
        for key in rows.values() {
            window.row_mark_suspect(*key);
        }
    }

    let frame = RowFrame {
        list: list.clone(),
        padded_origin_y: padded_bounds.origin.y,
        row_extent,
        scroll_top,
        extent: ContentMask {
            bounds: Bounds {
                origin: bounds.origin
                    - point(bounds.size.width, bounds.size.height * 3.0 + item_height),
                size: size(
                    bounds.size.width * 3.0,
                    bounds.size.height * 7.0 + item_height * 2.0,
                ),
            },
        },
        clip: window.content_mask().bounds,
        scale,
        content_origin,
        item_height,
        available_width,
    };

    // The retained region: the viewport and two viewports on each side.
    let overscan = viewport * f64::from(OVERSCAN_VIEWPORTS);
    let region_start =
        (((scroll_top - overscan) / row_extent).floor().max(0.0) as usize).min(item_count);
    let region_end =
        (((scroll_top + viewport + overscan) / row_extent).ceil() as usize).min(item_count);
    // Rows leave in batches of at least a quarter of a viewport.
    let batch = (((0.25 * viewport) / row_extent).floor() as usize).max(1);
    let keep = region_start.saturating_sub(batch)..region_end + batch;
    let departed: Vec<usize> = rows
        .keys()
        .copied()
        .filter(|ix| !keep.contains(ix))
        .collect();
    for ix in departed {
        if let Some(key) = rows.remove(&ix) {
            window.row_drop(key);
        }
    }

    let mut plans: Vec<RowPlan> = Vec::new();
    let mut built_visible = 0usize;
    for ix in visible.clone() {
        built_visible += usize::from(prepare_row(
            source, &frame, ix, true, &mut rows, &mut plans, window, cx,
        ));
    }
    // Rows retained outside the viewport keep their output alive in the frame.
    let others: Vec<usize> = rows
        .keys()
        .copied()
        .filter(|ix| !visible.contains(ix))
        .collect();
    for ix in others {
        prepare_row(source, &frame, ix, false, &mut rows, &mut plans, window, cx);
    }
    // Fill the overscan: everything on the first frame, afterwards within the budget, three
    // quarters toward the scroll direction and one quarter against it.
    let above: Vec<usize> = (region_start..visible.start)
        .rev()
        .filter(|ix| !rows.contains_key(ix))
        .collect();
    let below: Vec<usize> = (visible.end..region_end)
        .filter(|ix| !rows.contains_key(ix))
        .collect();
    let (toward, against) = if moving_down {
        (below, above)
    } else {
        (above, below)
    };
    let fill: Vec<usize> = if constructed {
        let budget = (((f64::from(EXTENSION_BUDGET_VIEWPORTS) * viewport) / row_extent).floor()
            as usize)
            .max(1);
        let remaining = budget.saturating_sub(built_visible);
        let against_share = remaining / 4;
        toward
            .into_iter()
            .take(remaining - against_share)
            .chain(against.into_iter().take(against_share))
            .collect()
    } else {
        toward.into_iter().chain(against).collect()
    };
    for ix in fill {
        prepare_row(source, &frame, ix, false, &mut rows, &mut plans, window, cx);
    }
    plans.sort_by_key(|plan| plan.ix);
    frame_state.rows = plans;
    frame_state.row_paint = Some(RowPaint {
        extent: frame.extent,
    });

    let store = window.row_store(list);
    store.rows = rows;
    store.constructed = true;
    store.last_scroll_top = scroll_top;
    store.moving_down = moving_down;
    visible
}

/// Prepares row `ix` of the frame: replays it when its record is reusable, otherwise builds
/// it again (when it is shown, or when it is missing and is being filled). Returns whether
/// the row was built.
#[allow(clippy::too_many_arguments)]
fn prepare_row(
    source: &RowSource,
    frame: &RowFrame,
    ix: usize,
    shown: bool,
    rows: &mut BTreeMap<usize, ViewKey>,
    plans: &mut Vec<RowPlan>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if let Some(key) = rows.get(&ix).copied() {
        let placement = Placement {
            offset: row_offset(window.row_anchor(key), frame.content_origin),
            clip: frame.clip,
            keep_states: shown,
            scale_factor: frame.scale,
        };
        if window.row_reusable(key) && !window.row_hover_changed(key, &placement) {
            window.row_replay_prepaint(key, &placement, cx);
            plans.push(RowPlan {
                ix,
                key,
                element: None,
                placement,
                shown,
            });
            return false;
        }
        if !shown {
            // A row that changed outside the viewport is built when it is shown.
            rows.remove(&ix);
            window.row_drop(key);
            return false;
        }
    }
    let key = match rows.get(&ix).copied() {
        Some(key) => key,
        None => {
            let key = window.row_record(&frame.list, ix);
            rows.insert(ix, key);
            key
        }
    };
    window.row_begin(key);
    let mut item = (source.render_items)(ix..ix + 1, window, cx)
        .pop()
        .unwrap_or_else(|| crate::Empty.into_any_element());
    let available_space = size(
        AvailableSpace::Definite(frame.available_width),
        AvailableSpace::Definite(frame.item_height),
    );
    window.with_layout_item_index(ix, |window| {
        item.layout_as_root(available_space, window, cx)
    });
    window.row_layout_done(key);
    let origin = frame.content_origin + point(px(0.), frame.item_height * ix);
    let origin = point(
        origin.x,
        frame.padded_origin_y + Pixels::from(frame.row_extent * ix as f64 - frame.scroll_top),
    );
    window.row_prepaint(
        key,
        frame.extent,
        frame.clip,
        frame.content_origin,
        |window| item.prepaint_at(origin, window, cx),
    );
    plans.push(RowPlan {
        ix,
        key,
        element: Some(item),
        placement: Placement {
            offset: Point::default(),
            clip: frame.clip,
            keep_states: true,
            scale_factor: frame.scale,
        },
        shown,
    });
    true
}

/// Paints the rows of the frame: rows built in it are painted into scenes of their own,
/// replayed rows reuse theirs; the rows that reach the viewport are shown, moved and clipped.
fn paint_rows(frame_state: &mut UniformListFrameState, window: &mut Window, cx: &mut App) {
    let Some(row_paint) = frame_state.row_paint.as_ref() else {
        return;
    };
    let extent = row_paint.extent;
    for plan in &mut frame_state.rows {
        match plan.element.as_mut() {
            Some(element) => {
                window.row_paint(plan.key, extent, |window| element.paint(window, cx));
            }
            None => window.row_replay_paint(plan.key, &plan.placement, cx),
        }
        if plan.shown {
            window.row_show(plan.key, &plan.placement);
        }
    }
}
