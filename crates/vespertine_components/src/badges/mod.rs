//! Inline badges, notices and review cards (CMP-028).
//!
//! Generic content-slot components for compact status and review information inside text-heavy
//! surfaces: [`InlineBadge`] (icon, label, optional count or status, optional hover details),
//! [`ActivityPill`] and [`CornerCountBadge`] with the count formatting policy, [`notice_chip`]
//! (severity-tinted, selectable and copyable message), [`ReviewCard`] / [`ReviewDraft`] /
//! [`CommentAdder`] (located header, clipped body, bounded editable draft) and the
//! [`ChecklistPanel`]. They expose owner-provided callbacks and slots and know nothing about
//! the application's domain data. Colors come from [`BadgeTokens`] (theme roles).
//!
//! The components address icons by asset path; [`IconAssets`] serves the icons they draw
//! themselves.

mod badge;
pub mod checklist;
mod common;
mod count;
mod icons;
mod notice;
mod review;
mod tokens;

pub use badge::{
    BADGE_HEIGHT, BadgeDescriptor, CARD_WIDTH, DetailRow, HOVER_DELAY, InlineBadge, Trailing,
    detail_card, render_badge,
};
pub use checklist::{ChecklistPanel, ListCache};
pub use count::{
    ActivityPill, ActivityPillView, COUNT_CAP, CornerCountBadge, CountDisplay, activity_pill,
    corner_count_badge, count_display, count_label, count_label_capped, dot_opacity, pill_width,
};
pub use icons::{
    ICON_CHECK, ICON_COMMENT, ICON_COPY, ICON_EDIT, ICON_PLUS, ICON_REMOVE, ICON_WARNING,
    IconAssets,
};
pub use notice::{NoticeAction, NoticeVariant, Severity, notice_chip};
pub use review::{
    CardCallbacks, Column, CommentAdder, CommentContent, DRAFT_HEIGHT, ReviewCard, ReviewDraft,
    card_body_lines, card_height, render_adder, render_card, render_draft,
};
pub use tokens::BadgeTokens;
