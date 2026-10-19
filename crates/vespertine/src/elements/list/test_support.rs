//! Shared headless harness for the list tests (ELM-001, LAY-002).

use super::*;
use crate::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, TestAppContext, VisualTestContext, canvas, div,
};
use std::{cell::Cell, collections::HashMap};

/// How a test row is built.
#[derive(Clone, Copy, Debug)]
pub(super) enum Row {
    /// A row with a fixed height and the full available width.
    Fixed(f32),
    /// A row whose height is its width divided by the ratio (stands in for wrapped text).
    Ratio(f32),
}

pub(super) type RowFn = Rc<dyn Fn(usize) -> Row>;

pub(super) struct TestView {
    state: ListState,
    row: RowFn,
    padding: [f32; 4],
    rendered: Rc<RefCell<Vec<usize>>>,
    bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    focus: Option<Rc<Vec<FocusHandle>>>,
    keys: Rc<Cell<usize>>,
    /// When set, the list is wrapped in a box of this size (for views that are a window's root).
    viewport: Option<(f32, f32)>,
}

impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = self.row.clone();
        let rendered = self.rendered.clone();
        let bounds = self.bounds.clone();
        let [left, top, right, bottom] = self.padding;
        let focus = self.focus.clone();
        let keys = self.keys.clone();
        let viewport = self.viewport;
        let list = list(self.state.clone(), move |ix, _, _| {
            rendered.borrow_mut().push(ix);
            let log = bounds.clone();
            let element = match row(ix) {
                Row::Fixed(height) => div().h(px(height)).w_full(),
                Row::Ratio(ratio) => div().w_full().aspect_ratio(ratio),
            };
            let element = match focus.as_ref().and_then(|handles| handles.get(ix)) {
                Some(handle) => {
                    let keys = keys.clone();
                    element
                        .track_focus(handle)
                        .on_key_down(move |_, _, _| keys.set(keys.get() + 1))
                }
                None => element,
            };
            element
                .child(
                    canvas(
                        move |bounds, _, _| {
                            log.borrow_mut().insert(ix, bounds);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .into_any()
        })
        .pl(px(left))
        .pt(px(top))
        .pr(px(right))
        .pb(px(bottom))
        .w_full()
        .h_full();
        match viewport {
            Some((width, height)) => div()
                .w(px(width))
                .h(px(height))
                .child(list)
                .into_any_element(),
            None => list.into_any_element(),
        }
    }
}

/// A list state mounted in a headless window.
pub(super) struct Harness {
    view: Entity<TestView>,
    rendered: Rc<RefCell<Vec<usize>>>,
    bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
}

impl Harness {
    pub fn new(
        cx: &mut VisualTestContext,
        state: &ListState,
        row: impl Fn(usize) -> Row + 'static,
    ) -> Self {
        let rendered = Rc::new(RefCell::new(Vec::new()));
        let bounds = Rc::new(RefCell::new(HashMap::new()));
        let view = cx.update(|_, cx| {
            cx.new(|_| TestView {
                state: state.clone(),
                row: Rc::new(row),
                padding: [0.; 4],
                rendered: rendered.clone(),
                bounds: bounds.clone(),
                focus: None,
                keys: Rc::new(Cell::new(0)),
                viewport: None,
            })
        });
        Self {
            view,
            rendered,
            bounds,
        }
    }

    /// A harness whose view is the root of a real window with the list in a `width` by
    /// `height` box, as needed for tests that dispatch input events (those redraw the window's
    /// root view).
    pub fn in_window<'a>(
        cx: &'a mut TestAppContext,
        state: &ListState,
        row: impl Fn(usize) -> Row + 'static,
        viewport: (f32, f32),
    ) -> (Self, &'a mut VisualTestContext) {
        let rendered = Rc::new(RefCell::new(Vec::new()));
        let bounds = Rc::new(RefCell::new(HashMap::new()));
        let (view, cx) = cx.add_window_view(|_, _| TestView {
            state: state.clone(),
            row: Rc::new(row),
            padding: [0.; 4],
            rendered: rendered.clone(),
            bounds: bounds.clone(),
            focus: None,
            keys: Rc::new(Cell::new(0)),
            viewport: Some(viewport),
        });
        let harness = Self {
            view,
            rendered,
            bounds,
        };
        (harness, cx)
    }

    /// Redraw the window root (for harnesses created with [`Self::in_window`]).
    pub fn redraw(&self, cx: &mut VisualTestContext) {
        self.rendered.borrow_mut().clear();
        self.bounds.borrow_mut().clear();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    /// Make row `i` track the focus handle `handles[i]` and count key presses it receives.
    pub fn track_focus(&self, cx: &mut VisualTestContext, handles: Vec<FocusHandle>) {
        self.view
            .update(cx, |view, _| view.focus = Some(Rc::new(handles)));
    }

    /// The number of key-down events delivered to focus-tracking rows.
    pub fn key_presses(&self, cx: &mut VisualTestContext) -> usize {
        self.view.read_with(cx, |view, _| view.keys.get())
    }

    /// Fixed-height rows.
    pub fn fixed(
        cx: &mut VisualTestContext,
        state: &ListState,
        height: impl Fn(usize) -> f32 + 'static,
    ) -> Self {
        Self::new(cx, state, move |ix| Row::Fixed(height(ix)))
    }

    pub fn set_row(&self, cx: &mut VisualTestContext, row: impl Fn(usize) -> Row + 'static) {
        self.view.update(cx, |view, _| view.row = Rc::new(row));
    }

    pub fn set_padding(&self, cx: &mut VisualTestContext, padding: [f32; 4]) {
        self.view.update(cx, |view, _| view.padding = padding);
    }

    /// Draw one frame; the logs of rendered rows and painted bounds describe only this frame.
    pub fn draw(&self, cx: &mut VisualTestContext, width: f32, height: f32) {
        self.rendered.borrow_mut().clear();
        self.bounds.borrow_mut().clear();
        let view = self.view.clone();
        cx.draw(
            point(px(0.), px(0.)),
            size(px(width), px(height)),
            |_, _| view.into_any_element(),
        );
    }

    /// The logical indices passed to the render callback during the last frame.
    pub fn rendered(&self) -> Vec<usize> {
        self.rendered.borrow().clone()
    }

    /// The window bounds of a row painted during the last frame.
    pub fn bounds(&self, ix: usize) -> Option<Bounds<Pixels>> {
        self.bounds.borrow().get(&ix).copied()
    }

    pub fn painted(&self) -> Vec<usize> {
        let mut painted: Vec<usize> = self.bounds.borrow().keys().copied().collect();
        painted.sort_unstable();
        painted
    }
}

/// Replace the mounted rows' records with measured rows of the given heights.
pub(super) fn set_measured(state: &ListState, heights: &[Option<f32>]) {
    let mut inner = state.0.borrow_mut();
    let estimate = inner.estimate_px();
    inner.items = SumTree::from_iter(
        heights.iter().map(|height| match height {
            Some(height) => ListItem::Measured {
                size: size(px(10.), px(*height)),
                focus_handle: None,
            },
            None => ListItem::Unmeasured {
                size_hint: None,
                estimate,
                focus_handle: None,
            },
        }),
        (),
    );
}

pub(super) fn anchor(state: &ListState) -> (usize, f32) {
    let top = state.logical_scroll_top();
    (top.item_ix, top.offset_in_item.as_f32())
}

pub(super) fn at(item_ix: usize, offset: f32) -> ListOffset {
    ListOffset {
        item_ix,
        offset_in_item: px(offset),
    }
}
