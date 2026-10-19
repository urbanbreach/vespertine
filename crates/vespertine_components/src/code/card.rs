//! The Markdown fenced-block card (CMP-014 R15) with its header and controls (R8).
//!
//! A [`CodeCard`] is a rounded, bordered, clipped card around a [`CodeBlock`]: an optional header
//! (shown only when a language label or at least one action exists) with the language label on
//! the left and right-aligned actions, and a body with the code typography of the theme. The
//! label and every control are chrome: not selectable and not searchable.

use std::rc::Rc;
use vespertine::{
    AnyElement, App, ClickEvent, CursorStyle, ElementId, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    px,
};
use vespertine_richtext::RichText;
use vespertine_theme::document::DocumentTheme;

use super::block::CodeBlock;
use super::indicator::CodeScrollState;
use super::widgets::{Icon, icon, tooltip};
use crate::doc_theme::{alpha, hsla, ink_alpha};

type CopyFn = Rc<dyn Fn(usize, &str, &mut Window, &mut App)>;
type ToggleFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// An owner-supplied header action (for example the diagram source toggle of CMP-013).
pub struct CardAction {
    /// The label (accessible name and tooltip).
    pub label: SharedString,
    /// The icon.
    pub icon: Icon,
    /// Whether the action is in its active (pressed) state.
    pub active: bool,
    /// The click handler.
    pub on_click: ToggleFn,
}

/// The copy action: the block index and exact code text go to the handler; while the owner's
/// copied index equals the block index the control shows a check and "Copied".
pub struct CopyOptions {
    /// The index the owner marked as just copied, if any.
    pub copied_index: Option<usize>,
    /// Called with the block index and the exact code text.
    pub on_copy: CopyFn,
}

/// The wrap toggle: wrapped fit-to-width rows or nowrap horizontal scrolling.
pub struct WrapToggle {
    /// Whether the block is currently wrapped.
    pub wrapped: bool,
    /// Called when the toggle is activated.
    pub on_toggle: ToggleFn,
}

/// A fenced-block card. Construct with [`CodeCard::new`].
#[derive(IntoElement)]
pub struct CodeCard {
    id: ElementId,
    index: usize,
    code: String,
    language: Option<String>,
    label: Option<SharedString>,
    theme: DocumentTheme,
    copy: Option<CopyOptions>,
    wrap: Option<WrapToggle>,
    actions: Vec<CardAction>,
    notice: Option<SharedString>,
    scroll: Option<CodeScrollState>,
    indicator: bool,
    highlight: Option<std::sync::Arc<vespertine_richtext::syntax::HighlightedDocument>>,
    has_highlight: bool,
    key_owner: Option<u64>,
    body: Option<AnyElement>,
}

impl CodeCard {
    /// A card for block number `index` of `code`. `id` is the stable identity of the card.
    pub fn new(id: impl Into<ElementId>, index: usize, code: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            index,
            code: code.into(),
            language: None,
            label: None,
            theme: DocumentTheme::dark(),
            copy: None,
            wrap: None,
            actions: Vec::new(),
            notice: None,
            scroll: None,
            indicator: false,
            highlight: None,
            has_highlight: false,
            key_owner: None,
            body: None,
        }
    }

    /// Replaces the code body with another element (a rendered diagram) inside the same card:
    /// the header, its actions and the card chrome stay.
    pub fn body_override(mut self, body: AnyElement) -> Self {
        self.body = Some(body);
        self
    }

    /// The language tag used for coloration and, unless [`CodeCard::label`] is set, the header
    /// label.
    pub fn language(mut self, language: impl Into<String>) -> Self {
        let language = language.into();
        if !language.is_empty() {
            self.language = Some(language);
        }
        self
    }

    /// The header label (defaults to the language).
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The theme.
    pub fn theme(mut self, theme: DocumentTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Adds the copy action.
    pub fn copy(mut self, copy: CopyOptions) -> Self {
        self.copy = Some(copy);
        self
    }

    /// Adds the wrap toggle.
    pub fn wrap_toggle(mut self, toggle: WrapToggle) -> Self {
        self.wrap = Some(toggle);
        self
    }

    /// Adds an owner action.
    pub fn action(mut self, action: CardAction) -> Self {
        self.actions.push(action);
        self
    }

    /// A passive warning marker among the actions with `message` as its tooltip.
    pub fn notice(mut self, message: impl Into<SharedString>) -> Self {
        self.notice = Some(message.into());
        self
    }

    /// Owner-controlled scroll state of the block.
    pub fn scroll_state(mut self, state: CodeScrollState) -> Self {
        self.scroll = Some(state);
        self
    }

    /// Shows the horizontal scroll indicator.
    pub fn scroll_indicator(mut self, show: bool) -> Self {
        self.indicator = show;
        self
    }

    /// An owner-supplied highlight for the code (`None` renders plain).
    pub fn highlight(
        mut self,
        document: Option<std::sync::Arc<vespertine_richtext::syntax::HighlightedDocument>>,
    ) -> Self {
        self.highlight = document;
        self.has_highlight = true;
        self
    }

    /// Stable owner of the line selection identities (see [`CodeBlock::key_owner`]).
    pub fn key_owner(mut self, owner: impl std::hash::Hash) -> Self {
        self.key_owner = Some(vespertine_richtext::RunKey::new(owner, 0).owner);
        self
    }

    fn action_button(
        &self,
        id: impl Into<ElementId>,
        label: SharedString,
        icon_kind: Icon,
        active: bool,
        on_click: ToggleFn,
    ) -> AnyElement {
        let muted = hsla(self.theme.text_muted);
        let hover = ink_alpha(&self.theme, 0.08);
        let rest = if active {
            ink_alpha(&self.theme, 0.09)
        } else {
            alpha(muted, 0.)
        };
        let hover = if active {
            ink_alpha(&self.theme, 0.13)
        } else {
            hover
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .flex_none()
            .rounded(px(6.))
            .bg(rest)
            .hover(move |style| style.bg(hover))
            .cursor(CursorStyle::PointingHand)
            .role(Role::Button)
            .aria_label(label.clone())
            .tooltip(tooltip(label, &self.theme))
            .on_click(move |_: &ClickEvent, window, cx| {
                cx.stop_propagation();
                on_click(window, cx)
            })
            .child(icon(icon_kind, px(13.), muted))
            .into_any_element()
    }
}

impl RenderOnce for CodeCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let theme = self.theme.clone();
        let metrics = theme.metrics;
        let muted = hsla(theme.text_muted);
        let border = hsla(theme.border);
        let label: Option<SharedString> = self
            .label
            .clone()
            .or_else(|| self.language.clone().map(SharedString::from));
        let has_actions = self.copy.is_some()
            || self.wrap.is_some()
            || !self.actions.is_empty()
            || self.notice.is_some();

        let mut actions: Vec<AnyElement> = Vec::new();
        if let Some(notice) = &self.notice {
            actions.push(
                div()
                    .id((self.id.clone(), "notice"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .flex_none()
                    .tooltip(tooltip(notice.clone(), &theme))
                    .child(icon(Icon::Warning, px(13.), hsla(theme.diff_del)))
                    .into_any_element(),
            );
        }
        for (index, action) in self.actions.iter().enumerate() {
            actions.push(self.action_button(
                (self.id.clone(), format!("action-{index}")),
                action.label.clone(),
                action.icon,
                action.active,
                action.on_click.clone(),
            ));
        }
        if let Some(wrap) = &self.wrap {
            let tooltip_text = if wrap.wrapped {
                "Use horizontal scrolling"
            } else {
                "Fit content"
            };
            actions.push(self.action_button(
                (self.id.clone(), "wrap"),
                tooltip_text.into(),
                Icon::Wrap,
                wrap.wrapped,
                wrap.on_toggle.clone(),
            ));
        }
        if let Some(copy) = &self.copy {
            let copied = copy.copied_index == Some(self.index);
            let on_copy = copy.on_copy.clone();
            let code = self.code.clone();
            let index = self.index;
            let hover = ink_alpha(&theme, 0.08);
            let mut button = div()
                .id((self.id.clone(), "copy"))
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(22.))
                .px(px(6.))
                .flex_none()
                .rounded(px(5.))
                .hover(move |style| style.bg(hover))
                .cursor(CursorStyle::PointingHand)
                .role(Role::Button)
                .aria_label(if copied { "Copied" } else { "Copy code" })
                .tooltip(tooltip("Copy code", &theme))
                .on_click(move |_: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    on_copy(index, &code, window, cx)
                })
                .child(icon(
                    if copied { Icon::Check } else { Icon::Copy },
                    px(12.),
                    muted,
                ));
            if copied {
                button = button.child(
                    RichText::new((self.id.clone(), "copied"), "Copied")
                        .chrome()
                        .into_element(),
                );
                button = button.text_size(px(10.5)).text_color(muted);
            }
            actions.push(button.into_any_element());
        }

        let mut card = div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .w_full()
            .rounded(px(metrics.md_code_radius))
            .border_1()
            .border_color(border)
            .bg(ink_alpha(&theme, 0.035))
            .overflow_hidden();

        if label.is_some() || has_actions {
            let mut header = div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .flex_none()
                .bg(ink_alpha(&theme, 0.02))
                .border_b_1()
                .border_color(border)
                .pl(px(12.))
                .pr(px(if has_actions { 5. } else { 12. }));
            header = if has_actions {
                header.h(px(28.))
            } else {
                header.py(px(metrics.md_code_header_padding_y))
            };
            header = header.child(
                div()
                    .text_size(px(metrics.md_code_header_text_size))
                    .line_height(px(metrics.md_code_header_text_size + 5.))
                    .text_color(muted)
                    .children(label.map(|label| {
                        RichText::new((self.id.clone(), "label"), label.to_string()).chrome()
                    })),
            );
            if has_actions {
                header = header.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.))
                        .children(actions),
                );
            }
            card = card.child(header);
        }

        let wrapped = self.wrap.as_ref().is_some_and(|wrap| wrap.wrapped);
        let mut block = CodeBlock::new((self.id.clone(), "body"), self.code.clone())
            .theme(theme)
            .wrap(wrapped)
            .scroll_indicator(self.indicator)
            .px(px(metrics.md_code_padding_x))
            .py(px(metrics.md_code_padding_y));
        if let Some(language) = &self.language {
            block = block.language(language.clone());
        }
        if self.has_highlight {
            block = block.highlight(self.highlight.clone());
        }
        if let Some(scroll) = self.scroll.clone() {
            block = block.scroll_state(scroll);
        }
        if let Some(owner) = self.key_owner {
            block = block.key_owner(owner);
        }
        match self.body {
            Some(body) => card.child(div().w_full().p(px(metrics.md_code_padding_y)).child(body)),
            None => card.child(block),
        }
    }
}
