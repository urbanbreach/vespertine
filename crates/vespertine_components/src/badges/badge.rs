//! Inline badges with optional hover details (CMP-028 R1-R3).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use vespertine::{
    App, AppContext as _, ClickEvent, ElementId, Entity, FontWeight, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, svg,
};

use super::tokens::BadgeTokens;
use crate::tooltip::card::HoverCardHost;

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
/// Pointer rest time before the detail card is shown (R3): the hover-card dwell of the badges.
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

/// The hover-card facility of the badges of one container (CMP-004 through CMP-028 R3): one
/// [`HoverCardHost`] whose cards are the detail rows of the badge being hovered, so the dwell
/// (280 ms), the warm window between neighboring badges and the cold dismissal are the hover
/// card rules. The container creates it once, passes it to every badge with
/// [`InlineBadge::cards`] and renders [`BadgeCards::layer`] once.
#[derive(Clone)]
pub struct BadgeCards {
    host: Entity<HoverCardHost<ElementId>>,
    rows: Rc<RefCell<HashMap<ElementId, Vec<DetailRow>>>>,
}

impl BadgeCards {
    /// The card facility for badges drawn with `tokens`.
    pub fn new(tokens: &BadgeTokens, cx: &mut App) -> Self {
        let rows: Rc<RefCell<HashMap<ElementId, Vec<DetailRow>>>> = Rc::default();
        let host = cx.new(|cx| {
            let rows = rows.clone();
            let tokens = tokens.clone();
            HoverCardHost::new(
                move |key: &ElementId, _, _| {
                    let rows = rows.borrow().get(key).cloned().unwrap_or_default();
                    detail_card(rows, tokens.clone()).into_any_element()
                },
                cx,
            )
            .dwell(HOVER_DELAY)
            .bare(true)
        });
        Self { host, rows }
    }

    /// The host entity to render once in the container: it paints the shown card on its own
    /// layer.
    pub fn layer(&self) -> Entity<HoverCardHost<ElementId>> {
        self.host.clone()
    }
}

/// A badge pill (R1). `id` is the stable identity supplied by the owner; it scopes the hover
/// card.
#[derive(IntoElement)]
pub struct InlineBadge {
    id: ElementId,
    descriptor: BadgeDescriptor,
    tokens: BadgeTokens,
    cards: Option<BadgeCards>,
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
        cards: None,
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

    /// Shows this badge's detail rows through the container's shared hover-card facility, so
    /// neighboring badges share the warm window. Without it a badge with details brings its own
    /// facility (one card per badge, the same 280 ms dwell).
    pub fn cards(mut self, cards: &BadgeCards) -> Self {
        self.cards = Some(cards.clone());
        self
    }
}

impl RenderOnce for InlineBadge {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let descriptor = self.descriptor;
        let id = self.id;
        let trailing = descriptor.trailing.as_ref().map(|trailing| match trailing {
            Trailing::Count(count) => SharedString::from(super::count::count_label(*count)),
            Trailing::Status(status) => status.clone(),
        });
        let mut badge = div()
            .id(id.clone())
            .relative()
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
        let (cards, own_layer) = match self.cards {
            Some(cards) => (cards, false),
            None if descriptor.details.is_empty() => return badge.into_any_element(),
            None => {
                let state = window.use_keyed_state((id.clone(), "badge-cards"), cx, |_, cx| {
                    BadgeCards::new(&tokens, cx)
                });
                (state.read(cx).clone(), true)
            }
        };
        if descriptor.details.is_empty() {
            cards.rows.borrow_mut().remove(&id);
            return badge.into_any_element();
        }
        cards
            .rows
            .borrow_mut()
            .insert(id.clone(), descriptor.details);
        let badge = HoverCardHost::trigger(&cards.host, id, badge, cx);
        if own_layer {
            // The badge's own host paints its card from inside the badge; the zero-size
            // absolute wrapper keeps it out of the badge's flow and gap.
            badge
                .child(div().absolute().size_0().child(cards.layer()))
                .into_any_element()
        } else {
            badge.into_any_element()
        }
    }
}

/// The hover card: width 320, padding 6, rows separated by 4, popover surface style. The hover
/// card host paints it on its own layer above all ordinary content, so equal draw orders never
/// let the underlying text show through (R3).
#[derive(IntoElement)]
pub struct DetailCard {
    rows: Vec<DetailRow>,
    tokens: BadgeTokens,
}

impl RenderOnce for DetailCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
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

/// The hover-card content for `rows`, for hosts that show it through their own
/// [`HoverCardHost`].
pub fn detail_card(rows: Vec<DetailRow>, tokens: BadgeTokens) -> DetailCard {
    DetailCard { rows, tokens }
}
