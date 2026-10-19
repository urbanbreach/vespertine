//! Shaped rich text with source-aware geometry (TXT-006), bounded cached syntax coloration
//! (TXT-007), document-wide selectable text and safe links (TXT-008), and search washes (TXT-009).

pub mod runs;
pub mod syntax;

pub use runs::{floor_char_boundary, plain_run, runs_from_spans};
pub use syntax::{
    CodeLine, CodeView, HighlightCache, HighlightError, HighlightRequest, HighlightedDocument,
    HighlighterAdapter, Language, LanguageHint, RetainedHighlight, Span, SyntaxColors, TokenKind,
};
