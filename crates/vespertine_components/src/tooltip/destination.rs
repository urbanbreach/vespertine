//! The link destination card (CMP-004 R15-R17).
//!
//! The card shows the destination string exactly as written in the document (after the safe-link
//! rules of TXT-008) and nothing else: it never fetches, resolves or previews the remote
//! resource. The display string inserts U+200B between every grapheme cluster so that even one
//! very long segment wraps; the display string is presentation only. The text is a native
//! [`RichText`] run carrying a display map, so selecting and copying from the card yields the
//! original destination exactly.

use crate::tokens::ComponentTokens;
use vespertine::{
    InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Size,
    StatefulInteractiveElement, Styled, TextRun, Window, div, px, size,
};
use vespertine_richtext::links::{
    DISCLOSURE_LINE_HEIGHT, DISCLOSURE_PADDING_X, DISCLOSURE_PADDING_Y, DISCLOSURE_TEXT_SIZE,
    DisclosureSize,
};
use vespertine_richtext::{
    OriginalText, RichText, RunKey, SelectionRequest, TextService, text_layer,
    with_break_opportunities,
};

/// Horizontal chrome of the card: 6 px padding and a 1 px border on each side.
pub const HORIZONTAL_CHROME: f32 = 14.0;
/// Vertical chrome of the card: 4 px padding and a 1 px border on each side.
pub const VERTICAL_CHROME: f32 = 10.0;

/// The run key of the card's text: the card's whole text is addressable for programmatic
/// selection ([`DestinationCard::select_all`]).
fn run_key() -> RunKey {
    RunKey::new("destination-card-text", 0)
}

/// The measured destination card.
#[derive(Clone, Debug)]
pub struct DestinationCard {
    original: OriginalText,
    limits: DisclosureSize,
    bounds: Size<Pixels>,
    content_height: Pixels,
    tokens: ComponentTokens,
}

/// Builds the card for `destination` in a window of `viewport` size and measures it with the
/// real font: the width is the shaped natural width of the display string, rounded up to a whole
/// pixel, plus the chrome, never above the maximum width; the height follows the wrapped lines
/// and never exceeds the maximum height (the content scrolls beyond it).
pub fn destination_card(
    destination: &str,
    viewport: Size<Pixels>,
    tokens: ComponentTokens,
    window: &mut Window,
) -> DestinationCard {
    let (displayed, map) = with_break_opportunities(destination);
    let font = window.text_style().font();
    let run = |len: usize| TextRun {
        len,
        font: font.clone(),
        color: tokens.text,
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: Pixels::ZERO,
    };
    // Hover surfaces are measured with minimum-content constraints, so the natural width comes
    // from the destination itself (the break opportunities are zero-width).
    let natural = window
        .text_system()
        .shape_line(
            SharedString::from(destination.to_string()),
            px(DISCLOSURE_TEXT_SIZE),
            &[run(destination.len())],
            None,
        )
        .width;
    let limits = DisclosureSize::compute(viewport, f32::from(natural));
    let wrap_width = (limits.width - HORIZONTAL_CHROME).max(1.0);
    let rows: f32 = window
        .text_system()
        .shape_text(
            SharedString::from(displayed.clone()),
            px(DISCLOSURE_TEXT_SIZE),
            &[run(displayed.len())],
            Some(px(wrap_width)),
            None,
        )
        .map(|lines| {
            lines
                .iter()
                .map(|line| line.wrap_boundaries().len() + 1)
                .sum::<usize>() as f32
        })
        .unwrap_or(1.0);
    let content = rows * DISCLOSURE_LINE_HEIGHT + VERTICAL_CHROME;
    DestinationCard {
        original: OriginalText::plain(destination).present(displayed, map),
        limits,
        bounds: size(px(limits.width), px(content.min(limits.max_height))),
        content_height: px(content),
        tokens,
    }
}

impl DestinationCard {
    /// The measured size of the card (the bounds the caller places).
    pub fn bounds(&self) -> Size<Pixels> {
        self.bounds
    }

    /// The full height of the wrapped text with its chrome, before the maximum height applies.
    pub fn content_height(&self) -> Pixels {
        self.content_height
    }

    /// Whether the wrapped text is taller than the card, so the content scrolls vertically.
    pub fn scrolls(&self) -> bool {
        self.content_height > self.bounds.height
    }

    /// The width and height limits that applied.
    pub fn limits(&self) -> DisclosureSize {
        self.limits
    }

    /// The destination exactly as written.
    pub fn destination(&self) -> &str {
        self.original.source()
    }

    /// The display string, with a zero-width space between every grapheme cluster.
    pub fn display_text(&self) -> &str {
        self.original.displayed()
    }

    /// The text a selection of the whole card copies: the original destination.
    pub fn copied_text(&self) -> &str {
        self.original.copy(0..self.original.displayed().len())
    }

    /// Selects the whole text of the card while it is painted, so a copy (Ctrl-C) yields the
    /// destination exactly. The selection is applied when the current frame finishes painting,
    /// so the card must paint in that frame; the window must be rooted in
    /// [`vespertine_richtext::text_root`].
    pub fn select_all(&self, service: &TextService) {
        let end = self.original.displayed().len();
        service.select_runs(SelectionRequest {
            from: (run_key(), 0),
            to: (run_key(), end),
        });
    }

    /// The card element: raised surface, strong border, 4 px radius, 11 px text on 14 px lines;
    /// the content scrolls vertically beyond the maximum height. The text joins the window's text
    /// service even inside a deferred layer, so it can be selected and copied.
    pub fn element(&self) -> impl IntoElement + use<> {
        div()
            .id("destination-card")
            .debug_selector(|| "destination-card".to_string())
            .w(px(self.limits.width))
            .max_w(px(self.limits.max_width))
            .max_h(px(self.limits.max_height))
            .overflow_y_scroll()
            .px(px(DISCLOSURE_PADDING_X))
            .py(px(DISCLOSURE_PADDING_Y))
            .bg(self.tokens.surface_raised)
            .border_1()
            .border_color(self.tokens.border_strong)
            .rounded(px(4.))
            .text_size(px(DISCLOSURE_TEXT_SIZE))
            .line_height(px(DISCLOSURE_LINE_HEIGHT))
            .text_color(self.tokens.text)
            .child(text_layer(
                RichText::with_original("destination-card-text", self.original.clone())
                    .key(run_key()),
            ))
    }
}

/// The maximum width and height of the card for a viewport (A7): width
/// `clamp(viewport width - 24, 1, 360)`, height `clamp(viewport height - 80, 1, 160)`.
pub fn card_limits(viewport: Size<Pixels>) -> (f32, f32) {
    let limits = DisclosureSize::compute(viewport, 0.0);
    (limits.max_width, limits.max_height)
}
