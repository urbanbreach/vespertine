//! Severity-tinted notice chips with selectable, copyable messages (CMP-028 R6-R9).

use std::rc::Rc;

use vespertine::{
    App, ClickEvent, ClipboardItem, Div, FontWeight, Hsla, InteractiveElement, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, px, svg,
};
use vespertine_richtext::RichText;

use super::common::{action_button, icon_button, text_tooltip};
use super::icons::{ICON_COPY, ICON_WARNING};
use super::tokens::BadgeTokens;

/// The semantic severity of a notice (R6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Severity {
    /// The danger accent, muted danger for text.
    Error,
    /// The warning accent.
    Warning,
    /// A host-mapped severity (info, success): the accent and its muted text companion.
    Custom {
        /// The accent tone.
        accent: Hsla,
        /// The muted companion used for text.
        muted: Hsla,
    },
}

impl Severity {
    /// `(accent, muted text)` of the severity.
    pub fn tones(self, tokens: &BadgeTokens) -> (Hsla, Hsla) {
        match self {
            Severity::Error => (tokens.danger, tokens.danger_muted),
            Severity::Warning => (tokens.warning, tokens.warning_muted),
            Severity::Custom { accent, muted } => (accent, muted),
        }
    }
}

/// The two metric variants of the chip (R7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeVariant {
    /// Header icon 14 px; inset and radius 12 px; text scaled with the UI font scale.
    Plain,
    /// A 20 px square tile holding the icon; inset and radius 10 px; fixed 12 px text.
    Tile,
}

/// The optional action of a notice (R9), for example Retry.
#[derive(Clone)]
pub struct NoticeAction {
    /// The button label.
    pub label: SharedString,
    /// Invoked when the button is clicked.
    pub on_click: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

impl NoticeAction {
    /// An action button.
    pub fn new(
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Rc::new(on_click),
        }
    }
}

/// Metrics of a variant: `(inset and radius, text size)` before the UI font scale.
const PLAIN_INSET: f32 = 12.0;
const TILE_INSET: f32 = 10.0;

/// A notice chip: a rounded container with a 1 px border in the severity accent at 16% over a
/// wash of the accent at 5%, a header row (icon, medium-weight label, spacer, copy button) and
/// the message below. `key` identifies the chip (and scopes its message text); the returned
/// element is a normal stateful div the caller chains (dismiss on click, width, overflow).
///
/// The message wraps and never truncates; it is selectable text (TXT-008) when the window has a
/// text root, and the copy button writes the full message to the clipboard without letting the
/// click reach a surrounding handler. Notice strings are plain data: they never alter the
/// chip's structure, metrics or markup.
pub fn notice_chip(
    key: impl Into<SharedString>,
    tokens: &BadgeTokens,
    severity: Severity,
    label: impl Into<SharedString>,
    message: impl Into<SharedString>,
    variant: NoticeVariant,
    action: Option<NoticeAction>,
) -> Stateful<Div> {
    let key: SharedString = key.into();
    let label: SharedString = label.into();
    let message: SharedString = message.into();
    let (accent, muted) = severity.tones(tokens);
    let (inset, radius) = match variant {
        NoticeVariant::Plain => (PLAIN_INSET, PLAIN_INSET),
        NoticeVariant::Tile => (TILE_INSET, TILE_INSET),
    };
    let (text_size, line_height) = match variant {
        NoticeVariant::Plain => (12.0 * tokens.ui_scale, 16.0 * tokens.ui_scale),
        NoticeVariant::Tile => (12.0, 16.0),
    };
    let (label_color, message_color) = match variant {
        NoticeVariant::Plain => (muted.opacity(0.9), muted.opacity(0.9)),
        NoticeVariant::Tile => (muted.opacity(0.8), tokens.text.opacity(0.8)),
    };
    let icon = match variant {
        NoticeVariant::Plain => div().flex_none().child(
            svg()
                .path(ICON_WARNING)
                .size(px(14.))
                .text_color(muted.opacity(0.9)),
        ),
        NoticeVariant::Tile => div()
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .size(px(20.))
            .rounded(px(6.))
            .bg(accent.opacity(0.12))
            .child(
                svg()
                    .path(ICON_WARNING)
                    .size(px(12.))
                    .text_color(muted.opacity(0.8)),
            ),
    };
    let copied = message.clone();
    let copy = icon_button(
        SharedString::from(format!("{key}-copy")),
        ICON_COPY,
        20.0,
        12.0,
        6.0,
        muted.opacity(0.8),
        accent.opacity(0.12),
        Rc::new(move |_, _, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(copied.to_string()));
        }),
    )
    .tooltip(text_tooltip("Copy message".into(), tokens.clone()));
    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(icon)
        .child(
            div()
                .min_w_0()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(text_size))
                .text_color(label_color)
                .child(label),
        )
        .child(div().flex_1())
        .child(copy);
    let body = div()
        .w_full()
        .min_w_0()
        .text_size(px(text_size))
        .line_height(px(line_height))
        .text_color(message_color)
        .child(RichText::new(
            vespertine::ElementId::Name(SharedString::from(format!("{key}-message"))),
            message.to_string(),
        ));
    let mut chip = div()
        .id(vespertine::ElementId::Name(key.clone()))
        .flex()
        .flex_col()
        .gap(px(6.))
        .py(px(8.))
        .px(px(inset))
        .rounded(px(radius))
        .border_1()
        .border_color(accent.opacity(0.16))
        .bg(accent.opacity(0.05))
        .child(header)
        .child(body);
    if let Some(action) = action {
        chip = chip.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(6.))
                .child(action_button(
                    SharedString::from(format!("{key}-action")),
                    action.label,
                    tokens,
                    false,
                    action.on_click,
                )),
        );
    }
    chip
}
