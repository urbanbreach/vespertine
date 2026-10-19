//! Selectable code blocks and code editors (CMP-014).
//!
//! * [`CodeBlock`] is the read-only no-wrap host: fixed-height rows, an analytic line-number
//!   gutter, syntax colors that never change metrics, and an internal horizontal scroller.
//! * [`CodeCard`] is the Markdown fenced-block card (R15) with its header and controls (R8).
//! * [`SpanHighlighter`] is the owner-supplied highlighter of the editor (R11).
//! * [`CodeEditor`] is the editable surface built on the multiline editor (R10-R14).

pub mod block;
pub mod card;
pub mod indicator;
pub mod lines;
pub mod spans;
pub mod typography;
pub mod widgets;

pub use block::{CodeBlock, GutterLine, LineBounds, LineClick};
pub use card::{CardAction, CodeCard, CopyOptions, WrapToggle};
pub use indicator::{CodeScrollState, ThumbMetrics};
pub use lines::{digits, gutter_width, line_ranges, normalize};
pub use spans::{CodeHighlighter, SpanHighlighter, StyledRun, TextEdit};
pub use typography::{CodeTypography, TypographyOverrides};
