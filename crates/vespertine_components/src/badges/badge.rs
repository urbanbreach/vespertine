//! Inline badges with optional hover details (CMP-028 R1-R3).

use std::rc::Rc;
use std::time::Duration;

use vespertine::{
    AnyView, App, AppContext as _, ClickEvent, Context, ElementId, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, svg,
};

use super::tokens::BadgeTokens;

/// Fixed badge height: the surrounding strip sizes itself arithmetically, so the height never
/// depends on measurement (R1).
pub const BADGE_HEIGHT: f32 = 24.0;
/// Icon size.
pub const ICON_SIZE: f32 = 12.0;
/// Horizontal padding.
pub const PADDING_X: f32 = 8.0;
/// Gap between the icon, label and trailing item.
pub const GAP: f32 = 6.0;
/// Corner radius.
pub const RADIUS: f32 = 8.0;
/// Label text size.
pub const TEXT_SIZE: f32 = 12.0;
/// Neutral wash opacity of the background.
pub const WASH_OPACITY: f32 = 0.06;
/// Icon opacity.
pub const ICON_OPACITY: f32 = 0.7;
/// Delay before the detail card is shown (R3).
pub const HOVER_DELAY: Duration = Duration::from_millis(280);
/// Width of the detail card.
pub const CARD_WIDTH: f32 = 320.0;

/// One detail row of the hover card (R3): location, optional tag, body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailRow {
    /// Location text: monospace, 10 px, faint, single line, truncating.
    pub location: SharedString,
    /// Optional tag chip.
    pub tag: Option<SharedString>,
    /// Body text: 12 px on a 16 px line, wrapping.
    pub body: SharedString,
}

impl DetailRow {
    /// A row without a tag.
    pub fn new(location: impl Into<SharedString>, body: impl Into<SharedString>) -> Self {
        Self {
            location: location.into(),
            tag: None,
            body: body.into(),
        }
    }

    /// The same row with a tag chip.
    pub fn tag(mut self, tag: impl Into<SharedString>) -> Self {
        self.tag = Some(tag.into());
        self
    }
}

/// The trailing item of a badge: a count or a status word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trailing {
    /// A count, shown through [`count_label`](super::count::count_label).
    Count(usize),
    /// A status text.
    Status(SharedString),
}

/// What a badge shows: icon, label, optional count or status and ordered detail rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadgeDescriptor {
    /// The icon asset path (host supplied).
    pub icon: SharedString,
    /// The label; truncates with an ellipsis.
    pub label: SharedString,
    /// The optional count or status.
    pub trailing: Option<Trailing>,
    /// Detail rows; none means the badge carries no card.
    pub details: Vec<DetailRow>,
}

impl BadgeDescriptor {
    /// A descriptor with an icon and a label.
    pub fn new(icon: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            icon: icon.into(),
            label: label.into(),
            trailing: None,
            details: Vec::new(),
        }
    }

    /// With a count.
    pub fn count(mut self, count: usize) -> Self {
        self.trailing = Some(Trailing::Count(count));
        self
    }

    /// With a status text.
    pub fn status(mut self, status: impl Into<SharedString>) -> Self {
        self.trailing = Some(Trailing::Status(status.into()));
        self
    }

    /// With detail rows.
    pub fn details(mut self, details: Vec<DetailRow>) -> Self {
        self.details = details;
        self
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ContextMenuHandler = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// A badge pill (R1). `id` is the stable identity supplied by the owner; it scopes the hover
/// card.
#[derive(IntoElement)]
pub struct InlineBadge {
    id: ElementId,
    descriptor: BadgeDescriptor,
    tokens: BadgeTokens,
    on_click: Option<ClickHandler>,
    on_context_menu: Option<ContextMenuHandler>,
}

/// Renders a badge: `render(id, descriptor, theme)` of the public API.
pub fn render_badge(
    id: impl Into<ElementId>,
    descriptor: BadgeDescriptor,
    tokens: &BadgeTokens,
) -> InlineBadge {
    InlineBadge {
        id: id.into(),
        descriptor,
        tokens: tokens.clone(),
        on_click: None,
        on_context_menu: None,
    }
}

impl InlineBadge {
    /// Invoked when the badge is clicked.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Invoked on a secondary-button press over the badge.
    pub fn on_context_menu(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for InlineBadge {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let descriptor = self.descriptor;
        let trailing = descriptor.trailing.as_ref().map(|trailing| match trailing {
            Trailing::Count(count) => SharedString::from(super::count::count_label(*count)),
            Trailing::Status(status) => status.clone(),
        });
        let mut badge = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .h(px(BADGE_HEIGHT))
            .px(px(PADDING_X))
            .gap(px(GAP))
            .rounded(px(RADIUS))
            .bg(tokens.wash(WASH_OPACITY))
            .text_size(px(TEXT_SIZE))
            .font_weight(FontWeight::MEDIUM)
            .text_color(tokens.muted)
            // The badge takes part in flex shrinking and never reflows its neighbors.
            .min_w_0()
            .flex_shrink_1()
            .overflow_hidden()
            .child(
                svg()
                    .path(descriptor.icon)
                    .size(px(ICON_SIZE))
                    .flex_none()
                    .text_color(tokens.muted.opacity(ICON_OPACITY)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_shrink_1()
                    .truncate()
                    .child(descriptor.label),
            )
            .children(trailing.map(|trailing| div().flex_none().child(trailing)));
        if let Some(handler) = self.on_click {
            badge = badge.on_click(move |event, window, cx| handler(event, window, cx));
        }
        if let Some(handler) = self.on_context_menu {
            badge = badge.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                handler(event, window, cx)
            });
        }
        if !descriptor.details.is_empty() {
            let rows = descriptor.details;
            let card_tokens = tokens;
            badge = badge
                .tooltip_show_delay(HOVER_DELAY)
                .hoverable_tooltip(move |_, cx| {
                    let rows = rows.clone();
                    let tokens = card_tokens.clone();
                    cx.new(|_| DetailCard { rows, tokens }).into()
                });
        }
        badge
    }
}

/// The hover card: width 320, padding 6, rows separated by 4, popover surface style. It is
/// shown through the framework's tooltip layer, which paints above all ordinary content, so
/// equal draw orders never let the underlying text show through (R3).
struct DetailCard {
    rows: Vec<DetailRow>,
    tokens: BadgeTokens,
}

impl vespertine::Render for DetailCard {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = &self.tokens;
        let rows = self.rows.iter().map(|row| {
            let location = div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(tokens.mono_family.clone())
                .text_size(px(10.))
                .text_color(tokens.faint)
                .child(row.location.clone());
            let tag = row.tag.clone().map(|tag| {
                div()
                    .flex_none()
                    .px(px(4.))
                    .rounded(px(3.))
                    .bg(tokens.wash(0.10))
                    .font_family(tokens.mono_family.clone())
                    .text_size(px(10.))
                    .text_color(tokens.faint)
                    .child(tag)
            });
            div()
                .relative()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(8.))
                .rounded(px(6.))
                .bg(tokens.wash(0.05))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(tokens.accent.opacity(0.35)),
                )
                .child(
                    crate::layout::centered_row()
                        .gap(px(8.))
                        .child(location)
                        .children(tag),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(tokens.text)
                        .child(row.body.clone()),
                )
        });
        div()
            .w(px(CARD_WIDTH))
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(tokens.popover_border)
            .bg(tokens.popover)
            .shadow_md()
            .children(rows)
    }
}

/// A hover-card view for `rows`, for hosts that attach the card to their own element through
/// the framework's tooltip facility.
pub fn detail_card(rows: Vec<DetailRow>, tokens: BadgeTokens, cx: &mut App) -> AnyView {
    cx.new(|_| DetailCard { rows, tokens }).into()
}
