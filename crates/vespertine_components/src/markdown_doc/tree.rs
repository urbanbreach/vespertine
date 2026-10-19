//! The Markdown block tree (CMP-011 R1): blocks with byte ranges, runs with style flags, link
//! destinations, inline formulas, task markers and image info.

use std::ops::Range;

/// The destination of a link whose target has not streamed in yet (R17): styled as a link, never
/// activatable.
pub const PENDING_LINK: &str = "pending-link:";

/// Style flags of an inline run; they compose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    /// Strong emphasis.
    pub bold: bool,
    /// Emphasis.
    pub italic: bool,
    /// Strikethrough.
    pub strike: bool,
    /// Inline code.
    pub code: bool,
}

impl Style {
    /// Whether no flag is set.
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// An inline formula (CMP-012): the TeX source and whether it is a display formula.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formula {
    /// The formula source, without delimiters.
    pub tex: String,
    /// Display (true) or inline (false).
    pub display: bool,
}

/// The marker of a task item: the byte range of its bracket text ("[ ]", "[x]" or "[X]") in the
/// stored source and its checked state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskMarker {
    /// Byte range of the bracket text in the source.
    pub range: Range<usize>,
    /// Whether the task is checked.
    pub checked: bool,
}

/// Image markup: destination, alt text and title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    /// The destination.
    pub dest: String,
    /// The alt text.
    pub alt: String,
    /// The title (empty when none).
    pub title: String,
}

/// One inline run of uniform style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The displayed text. A formula run holds its canonical delimited source (`$tex$` or
    /// `$$tex$$`); a task marker run holds the literal marker and a space; an image run holds its
    /// label.
    pub text: String,
    /// Style flags.
    pub style: Style,
    /// Link destination, if the run is (part of) a link.
    pub link: Option<String>,
    /// The formula, if this run is one.
    pub formula: Option<Formula>,
    /// The task marker, if this run is the marker of a task item.
    pub task: Option<TaskMarker>,
    /// Image info, if this run is image markup.
    pub image: Option<ImageInfo>,
}

impl Run {
    /// A plain text run.
    pub fn text(text: impl Into<String>) -> Self {
        Self::styled(text, Style::default(), None)
    }

    /// A text run with a style and an optional link.
    pub fn styled(text: impl Into<String>, style: Style, link: Option<String>) -> Self {
        Self {
            text: text.into(),
            style,
            link,
            formula: None,
            task: None,
            image: None,
        }
    }

    /// Whether this run is plain text that may merge with neighbours of the same style.
    pub fn is_mergeable(&self) -> bool {
        self.formula.is_none() && self.task.is_none() && self.image.is_none()
    }
}

/// Column alignment of a table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Left (also unspecified).
    #[default]
    Left,
    /// Centered.
    Center,
    /// Right.
    Right,
}

/// A list item: its blocks and, for a task item, the marker (also the first run of the first
/// paragraph).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Source range of the item.
    pub range: Range<usize>,
    /// The item's blocks.
    pub blocks: Vec<Block>,
}

/// The kinds of block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// A paragraph.
    Paragraph(Vec<Run>),
    /// A heading of level 1 to 6.
    Heading {
        /// The level.
        level: u8,
        /// The heading's runs.
        runs: Vec<Run>,
    },
    /// Fenced or indented code.
    Code {
        /// The info string of a fence (trimmed), empty when absent.
        info: String,
        /// Whether the block is fenced.
        fenced: bool,
        /// The code text exactly, without the final line terminator.
        code: String,
    },
    /// A block quote.
    Quote(Vec<Block>),
    /// A list.
    List {
        /// Whether the list is ordered.
        ordered: bool,
        /// The number of the first item.
        start: u64,
        /// The items.
        items: Vec<Item>,
    },
    /// A thematic break.
    Rule,
    /// A pipe table.
    Table {
        /// Alignment per column.
        aligns: Vec<Align>,
        /// Header cells.
        header: Vec<Vec<Run>>,
        /// Body rows of cells (a row may hold fewer cells than there are columns).
        rows: Vec<Vec<Vec<Run>>>,
    },
    /// A display formula that is a block of its own.
    DisplayMath(Formula),
}

/// A block with its source range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Byte range of the block in the source. Pieces of one split paragraph share the range.
    pub range: Range<usize>,
    /// What the block is.
    pub kind: BlockKind,
}

impl BlockKind {
    /// The first line of the block's byte range, for source-line navigation helpers.
    pub fn is_container(&self) -> bool {
        matches!(self, Self::Quote(_) | Self::List { .. })
    }
}

/// The 1-based source line of a byte offset.
pub fn source_line(source: &str, offset: usize) -> usize {
    let end = offset.min(source.len());
    source.as_bytes()[..end]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}
