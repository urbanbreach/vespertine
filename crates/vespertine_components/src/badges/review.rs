//! Review cards, the bounded draft and the add affordance (CMP-028 R10-R15).
//!
//! The components are plain: owner callbacks, owner-supplied string ids, no domain state.

use std::rc::Rc;

use vespertine::{
    AnyView, App, ElementId, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px, svg,
};

use super::common::{Handler, action_button, text_tooltip};
use super::icons::{ICON_COMMENT, ICON_EDIT, ICON_PLUS, ICON_REMOVE};
use super::tokens::BadgeTokens;

/// Vertical padding of a card (10 above and below).
pub const CARD_PADDING_V: f32 = 20.0;
/// Header row height.
pub const HEADER_HEIGHT: f32 = 22.0;
/// Body line height.
pub const LINE_HEIGHT: f32 = 18.0;
/// Gap between the header and the body.
pub const GAP: f32 = 6.0;
/// Characters per wrapped body line in the analytic height.
pub const WRAP_COLUMNS: usize = 64;
/// Most body lines counted by the analytic height.
pub const MAX_LINES: usize = 8;
/// The fixed height of an open draft.
pub const DRAFT_HEIGHT: f32 = 116.0;
/// The accent bar width.
pub const BAR_WIDTH: f32 = 3.0;
/// The add control side.
pub const ADD_SIZE: f32 = 16.0;

/// The number of body lines the analytic height counts: the sum over the newline-separated
/// lines of `max(1, ceil(characters / 64))`, clamped to `[1, 8]` (R11).
pub fn card_body_lines(body: &str) -> usize {
    body.split('\n')
        .map(|line| line.chars().count().div_ceil(WRAP_COLUMNS).max(1))
        .sum::<usize>()
        .clamp(1, MAX_LINES)
}

/// The card height: `20 + 22 + lines x 18 + 6`, analytic so folds and tweens never desync from
/// measurement (R11).
pub fn card_height(body: &str) -> f32 {
    CARD_PADDING_V + HEADER_HEIGHT + card_body_lines(body) as f32 * LINE_HEIGHT + GAP
}

/// The content column of a document: cards align their content with it while their background
/// stays full width (R12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    /// Maximum content width.
    pub max_width: f32,
    /// Gutter on both sides of the content.
    pub gutter: f32,
}

/// The content of a review comment, supplied by the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentContent {
    /// The owner's string id.
    pub id: SharedString,
    /// The location text (for example a file and line).
    pub location: SharedString,
    /// The comment body.
    pub body: SharedString,
}

/// The owner callbacks of a card: `edit(id)` and `remove(id)`.
#[derive(Clone)]
pub struct CardCallbacks {
    /// Invoked with the comment id when the edit action is activated.
    pub edit: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    /// Invoked with the comment id when the remove action is activated.
    pub remove: Rc<dyn Fn(&str, &mut Window, &mut App)>,
}

impl CardCallbacks {
    /// Callbacks from two closures.
    pub fn new(
        edit: impl Fn(&str, &mut Window, &mut App) + 'static,
        remove: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            edit: Rc::new(edit),
            remove: Rc::new(remove),
        }
    }
}

/// A review card (R10): a left accent bar, a header with the location and the revealed edit and
/// remove actions, and the clipped body. Its height is [`card_height`].
#[derive(IntoElement)]
pub struct ReviewCard {
    content: CommentContent,
    callbacks: CardCallbacks,
    column: Option<Column>,
    tokens: BadgeTokens,
}

/// Renders a card for `content` with the owner's callbacks.
pub fn render_card(
    content: CommentContent,
    callbacks: CardCallbacks,
    column: Option<Column>,
    tokens: &BadgeTokens,
) -> ReviewCard {
    ReviewCard {
        content,
        callbacks,
        column,
        tokens: tokens.clone(),
    }
}

/// The focus handles of a card's actions and the subscriptions that repaint on focus changes.
struct ActionFocus {
    edit: FocusHandle,
    remove: FocusHandle,
    _subscriptions: [[vespertine::Subscription; 2]; 2],
}

/// The accent bar, positioned absolutely at the left edge.
fn accent_bar(color: vespertine::Hsla) -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(px(BAR_WIDTH))
        .bg(color)
}

/// Wraps content in the optional document column: centered, capped at the maximum width with
/// the gutter as padding; without a column the large spacing token pads it.
fn column_content(
    content: impl IntoElement,
    column: Option<Column>,
    tokens: &BadgeTokens,
) -> impl IntoElement {
    match column {
        Some(column) => div()
            .size_full()
            .flex()
            .flex_col()
            .max_w(px(column.max_width))
            .mx_auto()
            .px(px(column.gutter))
            .child(content),
        None => div()
            .size_full()
            .flex()
            .flex_col()
            .px(tokens.spacing_large)
            .child(content),
    }
}

impl RenderOnce for ReviewCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let content = self.content;
        let group: SharedString = format!("review-card-{}", content.id).into();
        let focus = window.use_keyed_state(
            ElementId::Name(format!("{group}-focus").into()),
            cx,
            |window, cx| {
                // Both actions follow the focus of either: repaint when it moves.
                let (edit, remove) = (
                    cx.focus_handle().tab_stop(true),
                    cx.focus_handle().tab_stop(true),
                );
                let view = window.current_view();
                let subscriptions = [&edit, &remove].map(|handle| {
                    [
                        window.on_focus_in(handle, cx, move |_, cx| cx.notify(view)),
                        window.on_focus_out(handle, cx, move |_, _, cx| cx.notify(view)),
                    ]
                });
                ActionFocus {
                    edit,
                    remove,
                    _subscriptions: subscriptions,
                }
            },
        );
        let (edit_focus, remove_focus) = {
            let state = focus.read(cx);
            (state.edit.clone(), state.remove.clone())
        };
        let focused = edit_focus.is_focused(window) || remove_focus.is_focused(window);
        let height = card_height(&content.body);
        let body_height = card_body_lines(&content.body) as f32 * LINE_HEIGHT;

        let action = |name: &str,
                      label: &'static str,
                      icon: &'static str,
                      handle: FocusHandle,
                      callback: Rc<dyn Fn(&str, &mut Window, &mut App)>|
         -> vespertine::Stateful<vespertine::Div> {
            let id = content.id.clone();
            let handler: Handler = Rc::new(move |_, window, cx| callback(&id, window, cx));
            let muted = tokens.muted;
            let hover = tokens.element_hover;
            let button = div()
                .id(ElementId::Name(format!("{group}-{name}").into()))
                .track_focus(&handle)
                .tab_stop(true)
                .role(accesskit::Role::Button)
                .aria_label(label)
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .size(px(16.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                // Hidden actions keep their layout space and stay in the accessibility tree.
                .opacity(if focused { 1.0 } else { 0.0 })
                .group_hover(group.clone(), |style| style.opacity(1.0))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    handler(event, window, cx);
                })
                .child(svg().path(icon).size(px(12.)).flex_none().text_color(muted));
            button.tooltip(text_tooltip(label.into(), tokens.clone()))
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .gap(px(6.))
            .child(
                svg()
                    .path(ICON_COMMENT)
                    .size(px(12.))
                    .flex_none()
                    .text_color(tokens.faint),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(tokens.mono_family.clone())
                    .text_size(px(11.))
                    .text_color(tokens.faint)
                    .child(content.location.clone()),
            )
            .child(action(
                "edit",
                "Edit comment",
                ICON_EDIT,
                edit_focus,
                self.callbacks.edit.clone(),
            ))
            .child(action(
                "remove",
                "Remove comment",
                ICON_REMOVE,
                remove_focus,
                self.callbacks.remove.clone(),
            ));
        let body = div()
            .flex_none()
            .h(px(body_height))
            .overflow_hidden()
            .text_size(px(12.))
            .line_height(px(LINE_HEIGHT))
            .text_color(tokens.dim)
            .child(content.body.clone());
        let inner = div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .py(px(CARD_PADDING_V / 2.0))
            .child(header)
            .child(body);
        div()
            .id(ElementId::Name(group.clone()))
            .group(group.clone())
            .relative()
            .w_full()
            .flex_none()
            .h(px(height))
            .overflow_hidden()
            .bg(tokens.wash(0.05))
            .child(accent_bar(tokens.accent.opacity(0.35)))
            .child(column_content(inner, self.column, &tokens))
    }
}

/// The bounded editable draft (R14): a fixed 116 px tall, with a header showing the location, a
/// 46 px overflow-hidden input region holding the owner's embedded multiline editor and an
/// action row with Cancel and the commit action.
#[derive(IntoElement)]
pub struct ReviewDraft {
    id: SharedString,
    location: SharedString,
    input: AnyView,
    editing: bool,
    commit_label: SharedString,
    cancel: Rc<dyn Fn(&mut Window, &mut App)>,
    commit: Rc<dyn Fn(&mut Window, &mut App)>,
    column: Option<Column>,
    tokens: BadgeTokens,
}

/// Renders a draft: `render_draft(location, input entity, editing flag, cancel, commit,
/// optional column)`. `input` is the owner's multiline editor view (CMP-008); `id` scopes the
/// draft's element identities.
#[allow(clippy::too_many_arguments)]
pub fn render_draft(
    id: impl Into<SharedString>,
    location: impl Into<SharedString>,
    input: impl Into<AnyView>,
    editing: bool,
    cancel: impl Fn(&mut Window, &mut App) + 'static,
    commit: impl Fn(&mut Window, &mut App) + 'static,
    column: Option<Column>,
    tokens: &BadgeTokens,
) -> ReviewDraft {
    ReviewDraft {
        id: id.into(),
        location: location.into(),
        input: input.into(),
        editing,
        commit_label: "Comment".into(),
        cancel: Rc::new(cancel),
        commit: Rc::new(commit),
        column,
        tokens: tokens.clone(),
    }
}

impl ReviewDraft {
    /// The commit label when not editing an existing card (default `Comment`); editing always
    /// reads `Save`.
    pub fn commit_label(mut self, label: impl Into<SharedString>) -> Self {
        self.commit_label = label.into();
        self
    }

    /// The label the primary action shows.
    pub fn primary_label(&self) -> SharedString {
        if self.editing {
            "Save".into()
        } else {
            self.commit_label.clone()
        }
    }
}

impl RenderOnce for ReviewDraft {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens.clone();
        let primary_label = self.primary_label();
        let id = self.id.clone();
        let cancel = self.cancel.clone();
        let escape = self.cancel.clone();
        let commit = self.commit.clone();
        let inner = div()
            .size_full()
            .flex()
            .flex_col()
            .py(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(px(HEADER_HEIGHT))
                    .truncate()
                    .font_family(tokens.mono_family.clone())
                    .text_size(px(11.))
                    .text_color(tokens.faint)
                    .child(self.location),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(46.))
                    .overflow_hidden()
                    .text_size(px(12.))
                    .text_color(tokens.text)
                    .child(self.input),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .flex_none()
                    .h(px(28.))
                    .gap(px(6.))
                    .child(action_button(
                        ElementId::Name(format!("{id}-cancel").into()),
                        "Cancel".into(),
                        &tokens,
                        false,
                        Rc::new(move |_, window, cx| cancel(window, cx)),
                    ))
                    .child(action_button(
                        ElementId::Name(format!("{id}-commit").into()),
                        primary_label,
                        &tokens,
                        true,
                        Rc::new(move |_, window, cx| commit(window, cx)),
                    )),
            );
        div()
            .id(ElementId::Name(format!("{id}-draft").into()))
            .relative()
            .w_full()
            .flex_none()
            .h(px(DRAFT_HEIGHT))
            .overflow_hidden()
            .bg(tokens.wash(0.08))
            .child(accent_bar(tokens.accent.opacity(0.7)))
            .child(column_content(inner, self.column, &tokens))
            // Mouse-down inside the draft never starts a selection or a row click.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // Escape cancels the draft and stops propagating, even before the editor sees it.
            .capture_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    escape(window, cx);
                    cx.stop_propagation();
                }
            })
    }
}

/// The add affordance (R13): a 16 px control with the accessible label "Add comment".
#[derive(IntoElement)]
pub struct CommentAdder {
    id: ElementId,
    open: Handler,
    tokens: BadgeTokens,
}

/// Renders the adder; `open` is the owner's open callback.
pub fn render_adder(
    id: impl Into<ElementId>,
    open: impl Fn(&mut Window, &mut App) + 'static,
    tokens: &BadgeTokens,
) -> CommentAdder {
    CommentAdder {
        id: id.into(),
        open: Rc::new(move |_, window, cx| open(window, cx)),
        tokens: tokens.clone(),
    }
}

impl RenderOnce for CommentAdder {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let open = self.open;
        let accent = self.tokens.accent;
        div()
            .id(self.id)
            .role(accesskit::Role::Button)
            .aria_label("Add comment")
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .size(px(ADD_SIZE))
            .rounded(px(4.))
            .bg(accent)
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .hover(move |style| style.bg(accent.opacity(0.85)))
            // Mouse-down stops propagation so selection or row clicks do not start.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |event, window, cx| {
                cx.stop_propagation();
                open(event, window, cx);
            })
            .child(
                svg()
                    .path(ICON_PLUS)
                    .size(px(11.))
                    .flex_none()
                    .text_color(self.tokens.on_accent),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_height_follows_the_analytic_formula() {
        assert_eq!(card_height("x"), 20.0 + 22.0 + 18.0 + 6.0);
        assert_eq!(card_height("x"), 66.0);
        let long = "x".repeat(65);
        assert_eq!(card_body_lines(&long), 2);
        assert_eq!(card_height(&long), 20.0 + 22.0 + 36.0 + 6.0);
        assert_eq!(card_height(&long), 84.0);
        let nine = ["x"; 9].join("\n");
        assert_eq!(card_body_lines(&nine), 8);
        assert_eq!(card_height(&nine), 20.0 + 22.0 + 8.0 * 18.0 + 6.0);
        assert_eq!(card_height(&nine), 192.0);
    }

    #[test]
    fn empty_lines_count_as_one_and_wrapping_counts_characters_not_bytes() {
        assert_eq!(card_body_lines(""), 1);
        assert_eq!(card_body_lines("a\n\nb"), 3);
        assert_eq!(card_body_lines(&"é".repeat(64)), 1);
        assert_eq!(card_body_lines(&"é".repeat(65)), 2);
        assert_eq!(card_body_lines(&"x".repeat(64 * 20)), 8);
    }

    #[test]
    fn the_draft_height_adds_up() {
        assert_eq!(DRAFT_HEIGHT, 22.0 + 46.0 + 28.0 + 2.0 * 10.0);
    }
}
