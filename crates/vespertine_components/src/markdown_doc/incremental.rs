//! The block-incremental parser (CMP-011 R16, R17): appended text re-parses only from the last
//! top-level block (snapped back to a line start), the canonical tree after any sequence of
//! appends equals the tree of a full parse, and the display tree repairs the hanging inline
//! syntax of the last block without touching the stored source.

use super::mend::mend;
use super::parse::{Parsed, parse_slice};
use super::tree::{Block, BlockKind};

/// The display tree: the canonical blocks before the repaired tail, then the repaired tail.
#[derive(Clone, Copy, Debug)]
pub struct DisplayTree<'a> {
    prefix: &'a [Block],
    tail: &'a [Block],
}

impl<'a> DisplayTree<'a> {
    /// The blocks in order.
    pub fn iter(&self) -> impl Iterator<Item = &'a Block> + use<'a> {
        self.prefix.iter().chain(self.tail.iter())
    }

    /// Number of blocks.
    pub fn len(&self) -> usize {
        self.prefix.len() + self.tail.len()
    }

    /// Whether there are no blocks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The blocks as a vector (cloned).
    pub fn to_vec(&self) -> Vec<Block> {
        self.iter().cloned().collect()
    }
}

/// A block-incremental Markdown parser.
#[derive(Debug, Default)]
pub struct IncrementalParser {
    source: String,
    blocks: Vec<Block>,
    /// Number of leading blocks of `blocks` that the repaired tail does not replace.
    mended_from: usize,
    mended_tail: Vec<Block>,
    last_parse_bytes: usize,
    stable_prefix: usize,
    /// A link reference definition was seen: every parse is a full parse.
    force_full: bool,
    parses: u64,
}

impl IncrementalParser {
    /// An empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored source: exactly the appended text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The canonical tree.
    pub fn tree(&self) -> &[Block] {
        &self.blocks
    }

    /// The display tree (the canonical tree with the last block's hanging syntax repaired).
    pub fn display_tree(&self) -> DisplayTree<'_> {
        if self.mended_tail.is_empty() {
            DisplayTree {
                prefix: &self.blocks,
                tail: &[],
            }
        } else {
            DisplayTree {
                prefix: &self.blocks[..self.mended_from],
                tail: &self.mended_tail,
            }
        }
    }

    /// Bytes the last append or replace parsed.
    pub fn last_parse_bytes(&self) -> usize {
        self.last_parse_bytes
    }

    /// Number of leading blocks guaranteed unchanged by the last append (render caches for them
    /// stay valid).
    pub fn stable_prefix_blocks(&self) -> usize {
        self.stable_prefix
    }

    /// How many canonical parses ran so far (a theme or paint change never adds one).
    pub fn parse_count(&self) -> u64 {
        self.parses
    }

    /// Whether a link reference definition forces full reparses.
    pub fn forces_full_parse(&self) -> bool {
        self.force_full
    }

    /// Replaces the whole source.
    pub fn replace(&mut self, source: &str) {
        self.source.clear();
        self.source.push_str(source);
        self.force_full = false;
        self.full_parse();
        self.stable_prefix = 0;
    }

    /// Clears the document.
    pub fn reset(&mut self) {
        self.replace("");
    }

    /// Appends text and re-parses incrementally.
    pub fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.source.push_str(text);
        if self.force_full {
            self.full_parse();
            self.stable_prefix = 0;
            return;
        }
        let tail_start = self
            .blocks
            .last()
            .map_or(0, |block| block.range.start.min(self.source.len()));
        let tail_start = self.source[..tail_start].rfind('\n').map_or(0, |i| i + 1);
        let mut tail_start = tail_start;
        let mut kept = self
            .blocks
            .partition_point(|block| block.range.start < tail_start);
        // A block directly followed by the tail (no blank line between) can still absorb it:
        // a table takes a following row, a paragraph or list item a lazy continuation line, so
        // such blocks are reparsed with their follower. Headings, rules and closed fences never
        // change.
        while kept > 0
            && self.blocks[kept - 1].range.end >= tail_start
            && can_absorb(&self.blocks[kept - 1].kind)
        {
            let start = self.blocks[kept - 1].range.start.min(self.source.len());
            tail_start = self.source[..start].rfind('\n').map_or(0, |i| i + 1);
            // Pieces of a split paragraph share their start: all of them are reparsed.
            kept = self
                .blocks
                .partition_point(|block| block.range.start < tail_start);
        }
        let Parsed {
            blocks,
            has_reference_definitions,
        } = parse_slice(&self.source, tail_start);
        self.parses += 1;
        if has_reference_definitions {
            self.force_full = true;
            self.full_parse();
            self.stable_prefix = 0;
            return;
        }
        self.blocks.truncate(kept);
        self.blocks.extend(blocks);
        self.last_parse_bytes = self.source.len() - tail_start;
        self.stable_prefix = kept;
        self.mend_tail(tail_start);
    }

    fn full_parse(&mut self) {
        let parsed = parse_slice(&self.source, 0);
        self.parses += 1;
        self.force_full |= parsed.has_reference_definitions;
        self.blocks = parsed.blocks;
        self.last_parse_bytes = self.source.len();
        let tail_start = self
            .blocks
            .last()
            .map_or(0, |block| block.range.start.min(self.source.len()));
        let tail_start = self.source[..tail_start].rfind('\n').map_or(0, |i| i + 1);
        self.mend_tail(tail_start);
    }

    /// Repairs the last top-level block group for display: the source from its start with the
    /// hanging syntax closed, parsed on its own.
    fn mend_tail(&mut self, tail_start: usize) {
        self.mended_tail.clear();
        self.mended_from = self.blocks.len();
        let Some(last) = self.blocks.last() else {
            return;
        };
        if matches!(last.kind, BlockKind::Code { .. }) {
            return;
        }
        let from = self
            .blocks
            .partition_point(|block| block.range.start < tail_start);
        let Some(mended) = mend(&self.source[tail_start..]) else {
            return;
        };
        let mut text = String::with_capacity(tail_start + mended.len());
        // Parse the repaired text at its real offsets: pad is avoided by parsing the slice and
        // shifting, so build a string whose tail is the mended block.
        text.push_str(&self.source[..tail_start]);
        text.push_str(&mended);
        let parsed = parse_slice(&text, tail_start);
        self.mended_from = from;
        self.mended_tail = parsed.blocks;
        let source_len = self.source.len();
        for block in &mut self.mended_tail {
            clamp_ranges(block, source_len);
        }
    }
}

/// Whether a block that is followed directly by more text can still change because of it.
fn can_absorb(kind: &BlockKind) -> bool {
    !matches!(
        kind,
        BlockKind::Heading { .. }
            | BlockKind::Rule
            | BlockKind::DisplayMath(_)
            | BlockKind::Code { fenced: true, .. }
    )
}

/// Clamps block ranges to the stored source (the repair appends text that is not in it).
fn clamp_ranges(block: &mut Block, len: usize) {
    block.range.start = block.range.start.min(len);
    block.range.end = block.range.end.min(len);
    match &mut block.kind {
        BlockKind::Quote(children) => children.iter_mut().for_each(|b| clamp_ranges(b, len)),
        BlockKind::List { items, .. } => {
            for item in items {
                item.range.start = item.range.start.min(len);
                item.range.end = item.range.end.min(len);
                item.blocks.iter_mut().for_each(|b| clamp_ranges(b, len));
            }
        }
        _ => {}
    }
}
