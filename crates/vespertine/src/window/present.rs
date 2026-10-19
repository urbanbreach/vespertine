//! Presentation-synchronized callbacks (PLT-002).
//!
//! Elements register callbacks during paint that run only when the scene they
//! were recorded in actually reaches the platform's presentation. Native
//! surfaces use them to move, resize, show and hide exactly together with the
//! pixels that were presented.

use std::{cell::RefCell, rc::Rc};

use crate::{App, Window};

/// A repeatable callback stored with the frame that registered it. It is shared
/// (not moved) when cached paint is replayed into a later frame.
pub(crate) type PresentCallback = Rc<RefCell<dyn FnMut(&mut Window, &mut App)>>;

impl Window {
    /// Registers a callback that runs each time the frame being painted is
    /// presented, immediately before the platform draws it.
    ///
    /// Unlike [`Window::on_next_frame`], which is one-shot and runs after the next
    /// frame is requested, a present callback is repeatable and bound to a single
    /// painted frame. It does not run for hit-test-only paints, discarded draws or
    /// intermediate frames that never reach presentation, so state it captures
    /// always matches the scene being shown. If paint is replayed from the previous
    /// frame by cached-paint reuse, the callbacks recorded in the replayed range are
    /// carried over to the new frame in their original relative order.
    ///
    /// Callbacks run in registration order on the application thread. They must not
    /// draw the window: a redraw requested from inside a callback (for example
    /// [`Window::refresh`]) is scheduled for the next frame.
    ///
    /// This method can only be called during paint; calling it at any other time is
    /// a programming error.
    #[track_caller]
    pub fn on_present(&mut self, callback: impl FnMut(&mut Window, &mut App) + 'static) {
        self.invalidator.debug_assert_paint();
        self.next_frame
            .present_callbacks
            .push(Rc::new(RefCell::new(callback)));
    }

    /// Runs the callbacks stored in the frame about to be presented, in
    /// registration order.
    pub(crate) fn run_present_callbacks(&mut self, cx: &mut App) {
        if self.rendered_frame.present_callbacks.is_empty() {
            return;
        }
        let callbacks = self.rendered_frame.present_callbacks.clone();
        assert!(
            !self.running_present_callbacks,
            "present callbacks must not re-enter presentation"
        );
        self.running_present_callbacks = true;
        for callback in callbacks {
            (callback.borrow_mut())(self, cx);
        }
        self.running_present_callbacks = false;
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use crate::{
        AnyElement, App, AppContext as _, Context, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, Window, canvas, div, px,
    };

    type Log = Rc<RefCell<Vec<String>>>;

    /// Paints a canvas that registers a present callback logging `label` and the
    /// number of platform draws completed when the callback runs.
    fn registering_canvas(log: &Log, label: &'static str, value: u32) -> AnyElement {
        let log = log.clone();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let log = log.clone();
                window.on_present(move |window, _| {
                    let draws = window
                        .platform_window
                        .as_test()
                        .map_or(0, |test| test.draw_count());
                    log.borrow_mut()
                        .push(format!("{label}={value} (draws before: {draws})"));
                });
            },
        )
        .size(px(10.))
        .into_any_element()
    }

    struct OrderedView {
        log: Log,
        value: u32,
    }

    impl Render for OrderedView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(registering_canvas(&self.log, "first", self.value))
                .child(registering_canvas(&self.log, "second", self.value))
                .child(registering_canvas(&self.log, "third", self.value))
        }
    }

    fn setup(cx: &mut TestAppContext) -> (crate::AnyWindowHandle, Log) {
        let log: Log = Default::default();
        let window = cx.add_window({
            let log = log.clone();
            move |_, _| OrderedView { log, value: 1 }
        });
        (window.into(), log)
    }

    fn draw(cx: &mut TestAppContext, window: crate::AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn present(cx: &mut TestAppContext, window: crate::AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.present(cx))
            .unwrap();
    }

    #[crate::test]
    fn hit_test_only_paint_runs_no_callbacks(cx: &mut TestAppContext) {
        let (window, log) = setup(cx);
        draw(cx, window);
        draw(cx, window);
        assert!(log.borrow().is_empty());
    }

    #[crate::test]
    fn callbacks_run_in_registration_order_before_the_platform_draw(cx: &mut TestAppContext) {
        let (window, log) = setup(cx);
        draw(cx, window);
        present(cx, window);
        // Each callback ran before the platform's own draw call.
        assert_eq!(
            *log.borrow(),
            [
                "first=1 (draws before: 0)",
                "second=1 (draws before: 0)",
                "third=1 (draws before: 0)"
            ]
        );
        present(cx, window);
        // Repeatable: presenting the same frame again runs them again, after
        // exactly one completed platform draw.
        assert_eq!(log.borrow().len(), 6);
        assert_eq!(log.borrow()[3], "first=1 (draws before: 1)");
    }

    #[crate::test]
    fn presented_frame_runs_its_own_state_once(cx: &mut TestAppContext) {
        let (window, log) = setup(cx);
        // Hit-test paint records value 1 and is superseded without presentation.
        draw(cx, window);
        let view = window
            .downcast::<OrderedView>()
            .unwrap()
            .entity(cx)
            .unwrap();
        view.update(cx, |view, cx| {
            view.value = 2;
            cx.notify();
        });
        draw(cx, window);
        present(cx, window);
        assert_eq!(
            *log.borrow(),
            [
                "first=2 (draws before: 0)",
                "second=2 (draws before: 0)",
                "third=2 (draws before: 0)"
            ]
        );
    }

    #[crate::test]
    fn clearing_a_frame_discards_its_callbacks(cx: &mut TestAppContext) {
        let (window, _log) = setup(cx);
        draw(cx, window);
        cx.update_window(window, |_, window, _| {
            assert_eq!(window.rendered_frame.present_callbacks.len(), 3);
            window.rendered_frame.clear();
            assert!(window.rendered_frame.present_callbacks.is_empty());
        })
        .unwrap();
    }

    /// A parent with a cached child between two freshly painted canvases.
    struct CachedChild {
        log: Log,
        renders: Rc<std::cell::Cell<u32>>,
    }

    impl Render for CachedChild {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div().child(registering_canvas(&self.log, "leaf", 1))
        }
    }

    struct Parent {
        log: Log,
        child: crate::Entity<CachedChild>,
        fresh_value: u32,
    }

    impl Render for Parent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(registering_canvas(&self.log, "before", self.fresh_value))
                .child(
                    crate::AnyView::from(self.child.clone())
                        .cached(crate::StyleRefinement::default()),
                )
                .child(registering_canvas(&self.log, "after", self.fresh_value))
        }
    }

    #[crate::test]
    fn cached_paint_copies_only_the_replayed_range_in_position(cx: &mut TestAppContext) {
        let log: Log = Default::default();
        let child_renders = Rc::new(std::cell::Cell::new(0));
        let window = cx.add_window({
            let log = log.clone();
            let renders = child_renders.clone();
            move |_, cx| Parent {
                log: log.clone(),
                child: cx.new(|_| CachedChild { log, renders }),
                fresh_value: 1,
            }
        });
        let handle: crate::AnyWindowHandle = window.into();
        draw(cx, handle);
        present(cx, handle);
        let first_frame: Vec<String> = log.borrow_mut().drain(..).collect();
        assert_eq!(
            first_frame,
            [
                "before=1 (draws before: 0)",
                "leaf=1 (draws before: 0)",
                "after=1 (draws before: 0)"
            ]
        );

        // Only the parent changes: the cached child's paint is replayed.
        window
            .update(cx, |parent, _, cx| {
                parent.fresh_value = 2;
                cx.notify();
            })
            .unwrap();
        draw(cx, handle);
        present(cx, handle);
        let second_frame: Vec<String> = log.borrow_mut().drain(..).collect();
        assert_eq!(
            second_frame,
            [
                "before=2 (draws before: 1)",
                "leaf=1 (draws before: 1)",
                "after=2 (draws before: 1)"
            ]
        );
        // The child was not rendered again: its paint was replayed from the
        // previous frame, which is where the "leaf" callback came from.
        assert_eq!(child_renders.get(), 1);
        // The replayed callback is copied once, never duplicated.
        cx.update_window(handle, |_, window, _| {
            assert_eq!(window.rendered_frame.present_callbacks.len(), 3);
        })
        .unwrap();
    }

    #[crate::test]
    fn redraw_requested_from_a_callback_is_deferred(cx: &mut TestAppContext) {
        let observed: Rc<RefCell<Vec<&'static str>>> = Default::default();
        let window = cx.add_window({
            let observed = observed.clone();
            move |_, _| RefreshingView { observed }
        });
        let handle: crate::AnyWindowHandle = window.into();
        draw(cx, handle);
        present(cx, handle);
        assert_eq!(*observed.borrow(), ["callback", "still-dirty"]);
    }

    struct RefreshingView {
        observed: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for RefreshingView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let observed = self.observed.clone();
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    window.on_present(move |window, _| {
                        observed.borrow_mut().push("callback");
                        window.refresh();
                        // The redraw is scheduled, not performed synchronously.
                        if window.invalidator.is_dirty() {
                            observed.borrow_mut().push("still-dirty");
                        }
                    });
                },
            )
            .size(px(10.))
        }
    }

    #[crate::test]
    fn registering_outside_paint_is_a_programming_error(cx: &mut TestAppContext) {
        let (window, _log) = setup(cx);
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cx.update_window(window, |_, window: &mut Window, _: &mut App| {
                window.on_present(|_, _| {});
            })
            .ok();
        }))
        .expect_err("registering outside paint must be rejected");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(
            message.contains("this method can only be called during paint"),
            "unexpected panic message: {message}"
        );
    }
}
