//! Framework-independent core of search washes (TXT-009): declarations, literal matching,
//! resolution over logical groups, paint-time numbering and the text-search
//! helper. Painting, layout and the inspection logs belong to the caller.

pub mod find;
pub mod matcher;
pub mod numbering;
pub mod resolve;
pub mod spec;

pub use find::{FindCursor, find_ranges};
pub use matcher::{GroupMatcher, Query, is_word_char, scan_count};
pub use numbering::Numbering;
pub use resolve::{GroupInput, MatchKind, Resolution, ResolvedMatch, Resolver, Segment};
pub use spec::{DEFAULT_RADIUS, HighlightDeclaration, HighlightSpec, default_hex_color};
