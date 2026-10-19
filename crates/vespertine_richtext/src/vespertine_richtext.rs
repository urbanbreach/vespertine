//! Shaped rich text with source-aware geometry (TXT-006), bounded cached syntax coloration
//! (TXT-007), document-wide selectable text and safe links (TXT-008), and search washes (TXT-009).
//!
//! # Overview
//!
//! - [`RichText`] is a shaped text run. It paints run backgrounds, then search washes, then the
//!   selection wash, then the glyphs, and registers itself with the window's [`TextService`] while
//!   it paints (document order is paint order). Text that is displayed differently from its source
//!   (shortened labels, reserved inline-object slots, break opportunities) carries an
//!   [`OriginalText`]/[`DisplayMap`] so that hit testing, selection and copy resolve to the source.
//! - [`text_root`] is the root of a document: it resets the per-frame registry and installs the
//!   window-level press, move, release and copy listeners. [`surface`], [`user_select`] and
//!   [`selection_color`] scope state to a subtree.
//! - [`highlight`] declares search washes (a literal query and/or explicit UTF-16 ranges) for a
//!   subtree; [`search`] holds the framework-independent matcher, numbering and find helper.
//! - [`RichLayout`] answers the geometry questions of carets, link hitboxes, washes and overlays;
//!   [`range_rects`] gives one rectangle per wrapped row for any byte range.
//! - [`links`] holds the link rules (activation, menu, disclosure, file-destination anchors) and
//!   [`link_layer`] wires them to focusable hit regions.
//!
//! A selection requires the app to focus-route key events as usual: the copy chord is handled by
//! a root-level listener, which runs when the focused element (or no element) lets the key bubble
//! to the root. An application's Tab binding (`window.focus_next`) moves between link tab stops.

pub mod autoscroll;
pub mod display_map;
pub mod geometry;
pub mod highlight;
pub mod host_text;
pub mod label;
pub mod link_layer;
pub mod links;
pub mod registry;
pub mod rich_text;
pub mod runs;
pub mod search;
pub mod selection;
pub mod service;
pub mod slot;
pub mod syntax;

pub use display_map::{DisplayMap, OriginalText, Replacement, with_break_opportunities};
pub use geometry::{IndexHit, MAX_RANGE_ROWS, RichLayout, RowGeometry, RowInfo, range_rects};
pub use highlight::{Highlight, highlight};
pub use host_text::{RichHostText, host_run_key};
pub use label::{ELLIPSIS, FlatText, Shortened, fit_labels};
pub use link_layer::{LinkActivation, LinkLayer, LinkOptions};
pub use links::{
    Column, ColumnUnit, FileLink, Link, LinkActionKind, LinkMenu, parse_file_destination,
};
pub use registry::{
    PaintedHighlight, PaintedText, PressTarget, RegisteredRun, Registry, RunGeometry, RunKey,
    StartRegion, SurfaceId,
};
pub use rich_text::{RichText, UnderWash};
pub use selection::{Outcome, Selection, Span, resolve_spans, word_range};
pub use service::{
    SELECTION_ALPHA, Scope, SelectionRequest, TextService, TextTheme, UserSelect, selection_color,
    surface, text_root, user_select,
};
pub use slot::{FILE_LINK_SLOT, SlottedText, insert_slots};

pub use runs::{
    CoverError, RichSpans, floor_char_boundary, plain_run, runs_from_spans, shaping_key,
};
pub use syntax::{
    CodeLine, CodeView, HighlightCache, HighlightError, HighlightRequest, HighlightedDocument,
    HighlighterAdapter, Language, LanguageHint, RetainedHighlight, SyntaxColors, TokenKind,
};
