//! Rich Markdown documents (CMP-011).

pub mod cache;
pub mod flatten;
pub mod incremental;
pub mod mend;
pub mod options;
pub mod parse;
pub mod render;
pub mod scan;
pub mod table;
pub mod tree;
pub mod view;

pub use flatten::StreamingFade;
pub use incremental::{DisplayTree, IncrementalParser};
pub use options::{
    CodeOptions, CodeUi, DiagramHandler, DiagramView, ImageResolver, MarkdownOptions, MediaOptions,
    TaskOptions,
};
pub use parse::{parse, parse_slice};
pub use table::TableGeometry;
pub use tree::*;
pub use view::{Markdown, MarkdownDocument};
