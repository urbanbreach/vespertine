use crate::{App, PathPromptFilter, PathPromptOptions, Window};
use anyhow::{Result, anyhow};
use futures::{
    channel::oneshot,
    future::{Either, select},
};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

/// The error a window-bound prompt resolves with when its window closes before the user answers.
pub const NATIVE_PROMPT_WINDOW_CLOSED: &str = "the window was closed before the prompt completed";

impl Window {
    /// Shows a native open prompt on behalf of this window and returns a receiver that resolves
    /// exactly once with the selection, `None` when the user cancelled, or an error.
    ///
    /// Unlike [`App::prompt_for_paths`], the prompt is bound to this window: keyboard focus goes
    /// back to the element that was focused when the prompt opened (the window's deactivation
    /// while the prompt is modal does not clear it), whether the user confirms or cancels, and
    /// the receiver resolves with an error instead of never resolving when the window is closed
    /// first. Selecting a path is the only way the prompt changes application state.
    pub fn prompt_for_paths(
        &mut self,
        options: PathPromptOptions,
        cx: &mut App,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let prompt = cx.prompt_for_paths(options);
        self.bind_native_prompt(prompt, cx)
    }

    /// Shows a native save prompt on behalf of this window; see [`Window::prompt_for_paths`] for
    /// the focus and window-closing behavior. `filters` are ordered extension filters, the first
    /// being the default; empty means all files.
    pub fn prompt_for_new_path(
        &mut self,
        directory: &Path,
        suggested_name: Option<&str>,
        filters: &[PathPromptFilter],
        cx: &mut App,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let prompt = cx.prompt_for_new_path(directory, suggested_name, filters);
        self.bind_native_prompt(prompt, cx)
    }

    fn bind_native_prompt<T: 'static>(
        &mut self,
        prompt: oneshot::Receiver<Result<T>>,
        cx: &mut App,
    ) -> oneshot::Receiver<Result<T>> {
        let previous_focus = self.focused(cx);
        let window_id = self.handle.id;

        let (closed_tx, closed_rx) = oneshot::channel::<()>();
        let closed_tx = Rc::new(RefCell::new(Some(closed_tx)));
        let closed_subscription = cx.on_window_closed(move |_, closed| {
            if closed == window_id
                && let Some(sender) = closed_tx.borrow_mut().take()
            {
                sender.send(()).ok();
            }
        });

        let (result_tx, result_rx) = oneshot::channel();
        self.spawn(cx, async move |cx| {
            let _closed_subscription = closed_subscription;
            let result = match select(prompt, closed_rx).await {
                Either::Left((answer, _)) => {
                    answer.unwrap_or_else(|_| Err(anyhow!("the prompt ended without an answer")))
                }
                Either::Right(_) => {
                    result_tx
                        .send(Err(anyhow!(NATIVE_PROMPT_WINDOW_CLOSED)))
                        .ok();
                    return;
                }
            };
            cx.update(|window, cx| {
                if let Some(previous) = &previous_focus {
                    window.focus(previous, cx);
                }
            })
            .ok();
            result_tx.send(result).ok();
        })
        .detach();
        result_rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _,
        Render, TestAppContext, div,
    };

    struct Form {
        input: FocusHandle,
        preview: FocusHandle,
        value: String,
    }

    impl Render for Form {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(div().id("input").track_focus(&self.input))
                .child(div().id("preview").track_focus(&self.preview))
        }
    }

    impl Focusable for Form {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.input.clone()
        }
    }

    fn open_form(cx: &mut TestAppContext) -> (crate::Entity<Form>, &mut crate::VisualTestContext) {
        let (form, window) = cx.add_window_view(|_, cx| Form {
            input: cx.focus_handle(),
            preview: cx.focus_handle(),
            value: "draft".to_owned(),
        });
        window.update(|window, cx| {
            form.update(cx, |form, cx| window.focus(&form.input.clone(), cx));
        });
        (form, window)
    }

    fn options() -> PathPromptOptions {
        PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
            filters: Vec::new(),
        }
    }

    #[crate::test]
    async fn cancelled_prompt_reports_none_and_returns_focus_to_the_same_element(
        cx: &mut TestAppContext,
    ) {
        let (form, window) = open_form(cx);
        let receiver = window.update(|window, cx| window.prompt_for_paths(options(), cx));
        assert!(window.did_prompt_for_paths());

        // The focus moves away and the window deactivates while the prompt is modal.
        window.update(|window, cx| {
            form.update(cx, |form, cx| window.focus(&form.preview.clone(), cx));
        });
        window.deactivate_window();

        window.simulate_path_prompt_response(|_| None);
        let result = receiver.await.unwrap().unwrap();
        assert_eq!(result, None);
        window.run_until_parked();

        window.update(|window, cx| {
            let form = form.read(cx);
            assert!(form.input.is_focused(window), "focus did not return");
            assert_eq!(form.value, "draft");
        });
    }

    #[crate::test]
    async fn deactivation_while_the_prompt_is_open_keeps_focus(cx: &mut TestAppContext) {
        let (form, window) = open_form(cx);
        let _receiver = window.update(|window, cx| window.prompt_for_paths(options(), cx));
        window.deactivate_window();
        window.update(|window, cx| {
            assert!(!window.is_window_active());
            assert!(form.read(cx).input.is_focused(window));
        });
    }

    #[crate::test]
    async fn confirmed_prompt_returns_the_selection_and_restores_focus(cx: &mut TestAppContext) {
        let (form, window) = open_form(cx);
        let receiver = window.update(|window, cx| window.prompt_for_paths(options(), cx));
        window.update(|window, cx| {
            form.update(cx, |form, cx| window.focus(&form.preview.clone(), cx));
        });
        window.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/a.png")]));
        let result = receiver.await.unwrap().unwrap();
        assert_eq!(result, Some(vec![PathBuf::from("/tmp/a.png")]));
        window.update(|window, cx| assert!(form.read(cx).input.is_focused(window)));
    }

    #[crate::test]
    async fn save_prompt_cancel_restores_focus(cx: &mut TestAppContext) {
        let (form, window) = open_form(cx);
        let receiver = window.update(|window, cx| {
            window.prompt_for_new_path(
                Path::new("/tmp"),
                Some("out.png"),
                &[PathPromptFilter::new("PNG", ["png"])],
                cx,
            )
        });
        assert!(window.did_prompt_for_new_path());
        window.update(|window, cx| {
            form.update(cx, |form, cx| window.focus(&form.preview.clone(), cx));
        });
        window.simulate_new_path_selection(|_| None);
        assert_eq!(receiver.await.unwrap().unwrap(), None);
        window.update(|window, cx| assert!(form.read(cx).input.is_focused(window)));
    }

    #[crate::test]
    async fn closing_the_window_first_resolves_with_an_error_not_a_hang(cx: &mut TestAppContext) {
        let (_form, window) = open_form(cx);
        let receiver = window.update(|window, cx| window.prompt_for_paths(options(), cx));
        window.update(|window, _| window.remove_window());
        window.run_until_parked();
        let error = receiver.await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), NATIVE_PROMPT_WINDOW_CLOSED);
    }
}
