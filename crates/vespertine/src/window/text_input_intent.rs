//! Tests for the portable text-input intent signal and the insets query
//! (PLT-006). The signal itself is [`Window::request_text_input`].

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use crate::{
        Context, Edges, InteractiveElement as _, IntoElement, Modifiers, MouseButton,
        ParentElement as _, Render, Styled as _, TestAppContext, Window, WindowInsets, div, point,
        px,
    };

    struct GestureView {
        /// `(requests seen inside the callback, requests seen right after the
        /// signal returned)` for every pointer-down.
        observed: Rc<RefCell<Vec<(usize, usize)>>>,
    }

    impl Render for GestureView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let observed = self.observed.clone();
            div().size_full().child(div().size(px(50.)).on_mouse_down(
                MouseButton::Left,
                move |_, window, _| {
                    let before = window.test_text_input_requests();
                    window.request_text_input();
                    let after = window.test_text_input_requests();
                    observed.borrow_mut().push((before, after));
                },
            ))
        }
    }

    impl Window {
        fn test_text_input_requests(&mut self) -> usize {
            self.platform_window
                .as_test()
                .expect("test window")
                .text_input_request_count()
        }
    }

    #[crate::test]
    fn request_reaches_the_platform_before_the_pointer_callback_returns(cx: &mut TestAppContext) {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let (_view, cx) = cx.add_window_view({
            let observed = observed.clone();
            move |_, _| GestureView { observed }
        });
        let focus_before = cx.update(|window, _| window.focus);

        cx.simulate_mouse_down(point(px(5.), px(5.)), MouseButton::Left, Modifiers::none());

        assert_eq!(*observed.borrow(), [(0, 1)]);
        cx.update(|window, _| {
            assert_eq!(window.test_text_input_requests(), 1);
            // The signal is not a focus change.
            assert_eq!(window.focus, focus_before);
        });
    }

    #[crate::test]
    fn request_does_not_touch_text_input_configuration_or_state(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| crate::Empty);
        window
            .update(cx, |_, window, _| {
                let test = window.platform_window.as_test().unwrap();
                let configurations = test.text_input_configurations();
                let changes = test.text_input_state_changes();
                window.request_text_input();
                window.request_text_input();
                let test = window.platform_window.as_test().unwrap();
                assert_eq!(test.text_input_request_count(), 2);
                assert_eq!(test.text_input_configurations(), configurations);
                assert_eq!(test.text_input_state_changes(), changes);
                // No input handler is installed or removed by the signal.
                assert!(window.platform_window.take_input_handler().is_none());
            })
            .unwrap();
    }

    #[crate::test]
    fn insets_are_queried_from_the_platform_at_call_time(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| crate::Empty);
        let test_window = cx.test_window(window.into());
        window
            .update(cx, |_, window, _| {
                assert_eq!(window.insets(), WindowInsets::default());
            })
            .unwrap();

        let reported = WindowInsets {
            safe_area: Edges {
                top: px(24.),
                ..Default::default()
            },
            ime: Edges {
                bottom: px(300.),
                ..Default::default()
            },
        };
        test_window.simulate_insets_change(reported.clone());
        window
            .update(cx, |_, window, _| assert_eq!(window.insets(), reported))
            .unwrap();

        // A later platform change is observed by the next query.
        test_window.simulate_insets_change(WindowInsets::default());
        window
            .update(cx, |_, window, _| {
                assert_eq!(window.insets(), WindowInsets::default())
            })
            .unwrap();
    }
}
