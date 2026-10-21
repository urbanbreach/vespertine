//! Bounded cached theme-independent syntax coloration (TXT-007).

mod cache;
mod code_line;
mod document;
mod highlighter;
mod kind;
mod language;
mod theme;

pub use cache::{
    CACHE_MAX_BYTES, CACHE_MAX_DOCUMENTS, CacheLimits, CacheStats, GRAMMAR_GENERATION,
    HighlightCache, HighlighterAdapter, LanguageHint, RetainedHighlight,
};
pub use code_line::CodeLine;
pub use document::{
    HighlightError, HighlightedDocument, Limits, MAX_RAW_SPANS, MAX_SOURCE_BYTES, Span,
};
pub use highlighter::{
    HighlightRequest, first_line, highlight, highlight_language, preload_grammars,
};
pub use kind::{TokenKind, kind_for_scope, kind_for_scope_stack, scope_matches_prefix};
pub use language::{Language, detect, resolve_alias, resolve_path, resolve_shebang};
pub use theme::{CATEGORY_KEYS, SyntaxColors};
