//! Native file prompts (open files, open folder, multiple selection, save) and external
//! destinations (reveal in the file manager, open a file, open a URL).
//!
//! Click into the "input" box, press a prompt button and cancel the native dialog: the
//! example checks that the receiver reported cancellation, that the input value and preview
//! did not change, and that keyboard focus is back on the element that had it. Choosing a file
//! then enables "Reveal selection in file manager". Run it on Linux with a desktop portal:
//!
//! ```sh
//! cargo run -p vespertine --example native_prompts
//! ```

#[path = "example_support/fonts.rs"]
mod example_support;

use std::path::PathBuf;
use vespertine::{
    App, Bounds, Context, Div, FocusHandle, Focusable, KeyDownEvent, PathPromptFilter,
    PathPromptOptions, SharedString, Stateful, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use vespertine_platform::application;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prompt {
    OpenFile,
    OpenFolder,
    OpenMultiple,
    Save,
}

struct NativePrompts {
    input: FocusHandle,
    preview_focus: FocusHandle,
    value: String,
    preview: SharedString,
    selection: Vec<PathBuf>,
    status: SharedString,
}

impl NativePrompts {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            input: cx.focus_handle(),
            preview_focus: cx.focus_handle(),
            value: "draft text".to_owned(),
            preview: "preview of: draft text".into(),
            selection: Vec::new(),
            status: "Focus the input, press a prompt button, then cancel the dialog.".into(),
        }
    }

    fn focus_name(&self, window: &Window) -> &'static str {
        if self.input.is_focused(window) {
            "input"
        } else if self.preview_focus.is_focused(window) {
            "preview"
        } else {
            "nothing"
        }
    }

    fn prompt(&mut self, prompt: Prompt, window: &mut Window, cx: &mut Context<Self>) {
        let value_before = self.value.clone();
        let preview_before = self.preview.clone();
        let focus_before = self.focus_name(window);
        let images = vec![PathPromptFilter::new("Images", ["png", "jpg", "JPEG"])];

        let (paths, save) = match prompt {
            Prompt::Save => (None, {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(std::env::temp_dir);
                Some(window.prompt_for_new_path(&home, Some("untitled.png"), &images, cx))
            }),
            _ => (
                Some(window.prompt_for_paths(
                    PathPromptOptions {
                        files: prompt != Prompt::OpenFolder,
                        directories: prompt == Prompt::OpenFolder,
                        multiple: prompt == Prompt::OpenMultiple,
                        prompt: Some("Choose".into()),
                        filters: if prompt == Prompt::OpenFolder {
                            Vec::new()
                        } else {
                            images.clone()
                        },
                    },
                    cx,
                )),
                None,
            ),
        };

        self.status = format!("{prompt:?}: waiting for the native dialog...").into();
        cx.spawn_in(window, async move |this, cx| {
            let outcome: anyhow::Result<Option<Vec<PathBuf>>> = match (paths, save) {
                (Some(receiver), _) => receiver
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("the prompt ended without an answer"))),
                (_, Some(receiver)) => receiver
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("the prompt ended without an answer")))
                    .map(|path| path.map(|path| vec![path])),
                _ => unreachable!("exactly one prompt was opened"),
            };
            this.update_in(cx, |this, window, cx| {
                let unchanged = this.value == value_before && this.preview == preview_before;
                let focus_after = this.focus_name(window);
                this.status = match outcome {
                    Ok(None) => format!(
                        "{prompt:?}: cancelled (no selection). input/preview unchanged: {unchanged}; \
                         focus before: {focus_before}, after: {focus_after} (same: {})",
                        focus_before == focus_after
                    )
                    .into(),
                    Ok(Some(paths)) => {
                        let shown = paths
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        this.selection = paths;
                        format!("{prompt:?}: selected {shown}").into()
                    }
                    Err(error) => format!("{prompt:?}: failed: {error:#}").into(),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Focusable for NativePrompts {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.input.clone()
    }
}

impl Render for NativePrompts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus_name(window);
        let prompt_button = |id: &'static str, label: &'static str, prompt: Prompt| {
            button(id, label).on_click(cx.listener(move |this, _, window, cx| {
                this.prompt(prompt, window, cx);
            }))
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_3()
            .p_6()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5))
            .child(div().text_2xl().child("VESPERTINE native prompts"))
            .child(
                div()
                    .id("input")
                    .track_focus(&self.input)
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x52525b))
                    .focus(|style| style.border_color(rgb(0x60a5fa)))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if let Some(text) = &event.keystroke.key_char {
                            this.value.push_str(text);
                        } else if event.keystroke.key == "backspace" {
                            this.value.pop();
                        } else {
                            return;
                        }
                        this.preview = format!("preview of: {}", this.value).into();
                        cx.notify();
                    }))
                    .child(format!("input: {}", self.value)),
            )
            .child(
                div()
                    .id("preview")
                    .track_focus(&self.preview_focus)
                    .p_2()
                    .rounded_md()
                    .bg(rgb(0x27272a))
                    .focus(|style| style.border_1().border_color(rgb(0x60a5fa)))
                    .child(self.preview.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(prompt_button("open-file", "Open file", Prompt::OpenFile))
                    .child(prompt_button(
                        "open-folder",
                        "Open folder",
                        Prompt::OpenFolder,
                    ))
                    .child(prompt_button(
                        "open-multiple",
                        "Open multiple",
                        Prompt::OpenMultiple,
                    ))
                    .child(prompt_button("save", "Save as", Prompt::Save)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("reveal", "Reveal selection in file manager").on_click(cx.listener(
                            |this, _, _, cx| {
                                match this.selection.first() {
                                    Some(path) => {
                                        cx.reveal_path(path);
                                        this.status =
                                            format!("Revealing {}", path.display()).into();
                                    }
                                    None => {
                                        this.status =
                                            "Choose a file or folder first, then reveal it.".into()
                                    }
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        button("open-selection", "Open selection").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(path) = this.selection.first() {
                                    cx.open_with_system(path);
                                }
                            },
                        )),
                    ),
            )
            .child(
                button("open-docs", "Open documentation link").on_click(cx.listener(
                    |_, _, _, cx| {
                        cx.open_url("https://flatpak.github.io/xdg-desktop-portal/docs/");
                    },
                )),
            )
            .child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0x27272a))
                    .text_sm()
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x71717a))
                    .child(format!("focused element: {focus}")),
            )
    }
}

fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(0x3f3f46))
        .hover(|style| style.bg(rgb(0x52525b)))
        .active(|style| style.bg(rgb(0x71717a)))
        .cursor_pointer()
        .child(label)
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(760.), px(460.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(vespertine::TitlebarOptions {
                    title: Some("Native prompts".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(NativePrompts::new);
                window.focus(&view.focus_handle(cx), cx);
                view
            },
        )
        .expect("failed to open the native prompts window");
        cx.activate(true);
    });
}
