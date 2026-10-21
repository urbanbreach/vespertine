//! Themed diagram media with source fallback (CMP-013).
//!
//! A fenced block written in Mermaid notation is offered to the document's optional
//! [`DiagramHandler`] with a stable frame identifier (`<row key>-mermaid-<block index>`) and the
//! active theme. The handler answers with a [`DiagramView`]: the ordinary source fence (nothing
//! available yet), the source fence flagged with a failure reason, or a rendered diagram with a
//! show-source flag and a toggle callback. The document renders the source fence itself (CMP-014);
//! this module only decides *what* to show.
//!
//! * [`adapter`] renders Mermaid to SVG with `merman`: input limits, a palette derived from the
//!   theme, panic containment and the post-processing of the SVG.
//! * [`media`] rasterizes the SVG for a view without resolving any external resource.
//! * [`cache`] is the bounded cache: lazy requests, forgetting of scrolled-away requests, stale
//!   theme discarding, eviction and per-frame source toggles.
//! * [`host`] drives the cache with a background executor and implements the handler.
//!
//! A document uses it like this:
//!
//! ```ignore
//! let diagrams = DiagramHost::new();
//! diagrams.on_update(|rows, cx| /* remeasure the rows */);
//! options.diagram_handler = Some(diagrams.handler());
//! // every render of the document, before the rows are built:
//! diagrams.begin_pass(&theme, View { width: column_width, scale }, window, cx);
//! // and when the set of rows changes:
//! diagrams.retain_rows(|row_key| rows.contains(row_key));
//! ```

// The renderer, rasterizer, cache and host need `merman` and `resvg`; the view and handler types
// below are the contract of the document's hook and stay available without the feature, where
// Mermaid fences render as ordinary source blocks.
#[cfg(feature = "diagram")]
pub mod adapter;
#[cfg(feature = "diagram")]
pub mod cache;
#[cfg(feature = "diagram")]
pub mod host;
#[cfg(feature = "diagram")]
pub mod media;
#[cfg(feature = "diagram")]
mod postprocess;

use std::rc::Rc;
use vespertine::{AnyElement, App, SharedString, Window};
use vespertine_theme::document::DocumentTheme;

#[cfg(feature = "diagram")]
pub use adapter::{
    COMPLEXITY_LIMIT, DiagramRenderer, Family, MAX_LINES, MAX_SOURCE_BYTES, MAX_SVG_BYTES,
    MAX_TOKENS, MermanRenderer, OUTPUT_LIMIT, Palette, PaletteTokens, RENDER_PANIC, check_limits,
    count_tokens, detect_family, plate_color, render, render_with,
};
#[cfg(feature = "diagram")]
pub use cache::{DiagramCache, Job, Lookup, MAX_BYTES, MAX_ENTRIES, Status, Update, row_key};
#[cfg(feature = "diagram")]
pub use host::DiagramHost;
#[cfg(feature = "diagram")]
pub use media::{DiagramMedia, View};
#[cfg(feature = "diagram")]
pub use postprocess::{DEFAULT_GRID_STROKE, restyle};

/// What a document shows for a diagram fence.
pub enum DiagramView {
    /// The ordinary source fence, unchanged (no diagram is available yet).
    Source,
    /// The source fence flagged with a failure reason (a passive warning marker whose tooltip is
    /// the reason); the source stays visible.
    Failed(SharedString),
    /// A rendered diagram.
    Diagram {
        /// The diagram body.
        body: AnyElement,
        /// Whether the source view is shown instead of the body (per frame identifier).
        show_source: bool,
        /// Switches between the diagram and its source.
        toggle: Rc<dyn Fn(&mut Window, &mut App)>,
    },
}

/// The owner's optional diagram handler: source text, frame identifier
/// (`"<row key>-mermaid-<block index>"`, plus `"-<element index>"` for a fence nested after the
/// first element of its top-level block) and the active theme in; the view to show out.
pub type DiagramHandler =
    Rc<dyn Fn(&str, &str, &DocumentTheme, &mut Window, &mut App) -> DiagramView>;
