//! Incremental block documents (PRF-017 R1, R2).
//!
//! A [`BlockDocument`] is the ordered list of blocks a consumer-supplied [`BlockModel`] parses
//! from a source text and measures. An update takes the new full text (or an appended chunk) and
//! re-parses and re-measures only the suffix that starts at the first block the change can have
//! affected: every settled block before it keeps its parse and layout result. The result equals a
//! full parse of the whole text.
//!
//! Blocks have stable identities ([`BlockId`]): a block that continues to correspond to the same
//! source region (including a trailing block that only grows) keeps its identity, a split keeps
//! it on the first part and a merge keeps the identity of the first merged block. Selections and
//! scroll anchors are [`BlockAnchor`]s: a block identity and an offset inside the block.

use std::{collections::HashMap, ops::Range};

/// The stable identity of a block, assigned when the block is first created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u64);

/// One block parsed from the source by [`BlockModel::parse`].
pub struct ParsedBlock<B> {
    /// The consumer's parse result.
    pub block: B,
    /// Byte offset in the source where the block ends (the next block starts here). It must be
    /// greater than the block's start, at most the source length and on a character boundary.
    pub end: usize,
    /// Whether text appended after the source could still alter this block (for example a
    /// paragraph that no blank line has closed yet). The trailing block of a growing text is
    /// open; an open block is never settled.
    pub open: bool,
}

/// The consumer's parser and layout step.
pub trait BlockModel {
    /// The result of parsing one block.
    type Block;
    /// The result of measuring one block.
    type Layout;

    /// Parses the block that starts at byte `start` of `source` (`start < source.len()`).
    ///
    /// A block reported as closed (`open == false`) must not change when text is appended to
    /// `source` or when text after `end` changes: the parser has to report a block as open
    /// whenever its result depends on anything past the end of the source.
    fn parse(&self, source: &str, start: usize) -> ParsedBlock<Self::Block>;

    /// Measures a block; `text` is the block's source text.
    fn measure(&self, block: &Self::Block, text: &str) -> Self::Layout;
}

/// A block of the document with its parse and layout result.
pub struct BlockEntry<M: BlockModel> {
    /// The block's stable identity.
    pub id: BlockId,
    /// The block's byte range in the source.
    pub range: Range<usize>,
    /// Whether the block was reported open when it was parsed.
    pub open: bool,
    /// The consumer's parse result.
    pub block: M::Block,
    /// The consumer's layout result.
    pub layout: M::Layout,
}

/// A position inside a block: survives every update that leaves the block's prefix unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockAnchor {
    /// The block.
    pub block: BlockId,
    /// Byte offset inside the block's source text.
    pub offset: usize,
}

/// A selection between two anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockSelection {
    /// Where the selection started.
    pub anchor: BlockAnchor,
    /// Where it currently ends.
    pub focus: BlockAnchor,
}

/// What one update did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateStats {
    /// Index of the first block that was re-parsed.
    pub first_affected: usize,
    /// Settled blocks kept with their parse and layout results.
    pub reused: usize,
    /// Blocks parsed by this update.
    pub parsed: usize,
    /// Blocks measured by this update.
    pub measured: usize,
}

/// Totals over the life of a document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentStats {
    /// Updates applied.
    pub updates: u64,
    /// Blocks parsed.
    pub blocks_parsed: u64,
    /// Blocks measured.
    pub blocks_measured: u64,
}

/// An ordered list of blocks with incremental updates.
pub struct BlockDocument<M: BlockModel> {
    model: M,
    source: String,
    blocks: Vec<BlockEntry<M>>,
    index: HashMap<BlockId, usize>,
    next_id: u64,
    stats: DocumentStats,
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut length = a
        .as_bytes()
        .iter()
        .zip(b.as_bytes())
        .take_while(|(x, y)| x == y)
        .count();
    while !(a.is_char_boundary(length) && b.is_char_boundary(length)) {
        length -= 1;
    }
    length
}

fn common_suffix(a: &str, b: &str, limit: usize) -> usize {
    let mut length = a
        .as_bytes()
        .iter()
        .rev()
        .zip(b.as_bytes().iter().rev())
        .take(limit)
        .take_while(|(x, y)| x == y)
        .count();
    while !(a.is_char_boundary(a.len() - length) && b.is_char_boundary(b.len() - length)) {
        length -= 1;
    }
    length
}

impl<M: BlockModel> BlockDocument<M> {
    /// An empty document.
    pub fn new(model: M) -> Self {
        Self {
            model,
            source: String::new(),
            blocks: Vec::new(),
            index: HashMap::new(),
            next_id: 0,
            stats: DocumentStats::default(),
        }
    }

    /// The model (parser and layout step).
    pub fn model(&self) -> &M {
        &self.model
    }

    /// Mutable access to the model. After changing an input of the layout step (the width, say)
    /// call [`Self::remeasure`].
    pub fn model_mut(&mut self) -> &mut M {
        &mut self.model
    }

    /// The source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The blocks in order.
    pub fn blocks(&self) -> &[BlockEntry<M>] {
        &self.blocks
    }

    /// Totals of parse and layout calls.
    pub fn stats(&self) -> DocumentStats {
        self.stats
    }

    /// The position of the block with identity `id`.
    pub fn index_of(&self, id: BlockId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// The block with identity `id`.
    pub fn block(&self, id: BlockId) -> Option<&BlockEntry<M>> {
        self.index_of(id).map(|index| &self.blocks[index])
    }

    /// The source text of a block.
    pub fn text(&self, entry: &BlockEntry<M>) -> &str {
        &self.source[entry.range.clone()]
    }

    /// Replaces the source with `new_source`, re-parsing only from the first block the change can
    /// affect: the settled blocks that end inside the unchanged prefix are kept.
    pub fn update(&mut self, new_source: &str) -> UpdateStats {
        let common = common_prefix(&self.source, new_source);
        let suffix = if common < self.source.len() {
            let limit = self.source.len().min(new_source.len()) - common;
            common_suffix(&self.source, new_source, limit)
        } else {
            0
        };
        let old_len = self.source.len();
        self.source.truncate(common);
        self.source.push_str(&new_source[common..]);
        self.reparse(common, suffix, old_len)
    }

    /// Appends `chunk` to the source. Equivalent to [`Self::update`] with the extended text but
    /// without comparing the settled prefix.
    pub fn append(&mut self, chunk: &str) -> UpdateStats {
        let old_len = self.source.len();
        self.source.push_str(chunk);
        self.reparse(old_len, 0, old_len)
    }

    /// Measures every block again (after the layout step's inputs changed); parse results are
    /// kept.
    pub fn remeasure(&mut self) -> usize {
        for entry in &mut self.blocks {
            entry.layout = self
                .model
                .measure(&entry.block, &self.source[entry.range.clone()]);
        }
        self.stats.blocks_measured += self.blocks.len() as u64;
        self.blocks.len()
    }

    /// The anchor of byte `offset` of the source: the block containing it (the last block for an
    /// offset at the very end) and the offset inside that block.
    pub fn anchor_at(&self, offset: usize) -> Option<BlockAnchor> {
        let index = self
            .blocks
            .partition_point(|entry| entry.range.end <= offset)
            .min(self.blocks.len().checked_sub(1)?);
        let entry = &self.blocks[index];
        Some(BlockAnchor {
            block: entry.id,
            offset: offset
                .saturating_sub(entry.range.start)
                .min(entry.range.len()),
        })
    }

    /// The source offset of `anchor`, or `None` once its block no longer exists. The offset is
    /// clamped to the block's length.
    pub fn resolve(&self, anchor: BlockAnchor) -> Option<usize> {
        let entry = self.block(anchor.block)?;
        Some(entry.range.start + anchor.offset.min(entry.range.len()))
    }

    /// The source range of a selection, in document order.
    pub fn resolve_selection(&self, selection: BlockSelection) -> Option<Range<usize>> {
        let a = self.resolve(selection.anchor)?;
        let b = self.resolve(selection.focus)?;
        Some(a.min(b)..a.max(b))
    }

    /// The parts of the source range `range` that lie in each block, as block-relative ranges.
    pub fn blocks_in(&self, range: Range<usize>) -> impl Iterator<Item = (BlockId, Range<usize>)> {
        let first = self
            .blocks
            .partition_point(|entry| entry.range.end <= range.start);
        self.blocks[first..]
            .iter()
            .take_while(move |entry| entry.range.start < range.end)
            .map(move |entry| {
                let start = range.start.max(entry.range.start) - entry.range.start;
                let end = range.end.min(entry.range.end) - entry.range.start;
                (entry.id, start..end)
            })
    }

    /// Re-parses from the first block affected by a change whose unchanged prefix is `common`
    /// bytes long. `suffix` is the length of the unchanged tail and `old_len` the previous
    /// source length (both only steer identity assignment).
    fn reparse(&mut self, common: usize, suffix: usize, old_len: usize) -> UpdateStats {
        // Settled blocks that end inside the unchanged prefix are kept.
        let mut keep = self
            .blocks
            .partition_point(|entry| entry.range.end <= common);
        while keep > 0 && self.blocks[keep - 1].open {
            keep -= 1;
        }

        // Identities of the dropped blocks, by the start offset they have in the new text.
        let new_len = self.source.len();
        let mut inherited: HashMap<usize, BlockId> = HashMap::new();
        for entry in self.blocks.drain(keep..) {
            self.index.remove(&entry.id);
            let start = entry.range.start;
            let mapped = if start <= common {
                Some(start)
            } else if suffix > 0 && start >= old_len - suffix {
                Some(start + new_len - old_len)
            } else {
                None
            };
            if let Some(start) = mapped {
                inherited.entry(start).or_insert(entry.id);
            }
        }

        let mut position = self.blocks.last().map_or(0, |entry| entry.range.end);
        let mut stats = UpdateStats {
            first_affected: keep,
            reused: keep,
            ..UpdateStats::default()
        };
        while position < new_len {
            let parsed = self.model.parse(&self.source, position);
            assert!(
                parsed.end > position
                    && parsed.end <= new_len
                    && self.source.is_char_boundary(parsed.end),
                "BlockModel::parse must consume part of the source: block at {position} ended at {}",
                parsed.end
            );
            let range = position..parsed.end;
            let layout = self
                .model
                .measure(&parsed.block, &self.source[range.clone()]);
            let id = inherited.remove(&position).unwrap_or_else(|| {
                self.next_id += 1;
                BlockId(self.next_id - 1)
            });
            self.index.insert(id, self.blocks.len());
            position = parsed.end;
            self.blocks.push(BlockEntry {
                id,
                range,
                open: parsed.open,
                block: parsed.block,
                layout,
            });
            stats.parsed += 1;
            stats.measured += 1;
        }
        self.stats.updates += 1;
        self.stats.blocks_parsed += stats.parsed as u64;
        self.stats.blocks_measured += stats.measured as u64;
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paragraphs separated by a blank line (`\n\n`); a paragraph is open until the blank line
    /// that closes it has arrived. The layout is the number of characters.
    struct Paragraphs;

    impl BlockModel for Paragraphs {
        type Block = String;
        type Layout = usize;

        fn parse(&self, source: &str, start: usize) -> ParsedBlock<String> {
            match source[start..].find("\n\n") {
                Some(at) => {
                    // The block owns the blank line that closes it.
                    let end = start + at + 2;
                    ParsedBlock {
                        block: source[start..start + at].to_string(),
                        end,
                        open: false,
                    }
                }
                None => ParsedBlock {
                    block: source[start..].to_string(),
                    end: source.len(),
                    open: true,
                },
            }
        }

        fn measure(&self, block: &String, _text: &str) -> usize {
            block.chars().count()
        }
    }

    fn snapshot(document: &BlockDocument<Paragraphs>) -> Vec<(Range<usize>, bool, String, usize)> {
        document
            .blocks()
            .iter()
            .map(|b| (b.range.clone(), b.open, b.block.clone(), b.layout))
            .collect()
    }

    fn full_parse(source: &str) -> Vec<(Range<usize>, bool, String, usize)> {
        let mut fresh = BlockDocument::new(Paragraphs);
        fresh.update(source);
        snapshot(&fresh)
    }

    fn corpus(blocks: usize) -> String {
        (0..blocks)
            .map(|i| format!("block number {i}\n\n"))
            .collect()
    }

    #[test]
    fn appending_reuses_every_settled_block_and_matches_a_full_parse() {
        let mut document = BlockDocument::new(Paragraphs);
        let mut text = corpus(500);
        let first = document.update(&text);
        assert_eq!(first.parsed, 500);

        // Stream a trailing paragraph chunk by chunk, then close it and start another.
        for step in 0..40 {
            let before = document.stats();
            let chunk = if step == 30 { "\n\nnext " } else { "chunk " };
            text.push_str(chunk);
            let stats = document.append(chunk);
            assert_eq!(snapshot(&document), full_parse(&text), "step {step}");
            let after = document.stats();
            // Settled blocks are never re-parsed or re-measured; at most the open tail (and the
            // block it closes into) is.
            assert!(stats.parsed <= 2, "step {step}: {stats:?}");
            assert_eq!(stats.measured, stats.parsed);
            assert_eq!(
                after.blocks_parsed - before.blocks_parsed,
                stats.parsed as u64
            );
            assert_eq!(stats.first_affected, stats.reused);
            assert!(stats.reused >= 500);
        }
        assert_eq!(document.source(), text);
    }

    #[test]
    fn updating_with_the_new_full_text_reuses_the_unchanged_prefix_only() {
        let mut document = BlockDocument::new(Paragraphs);
        let mut text = corpus(100);
        document.update(&text);
        text.push_str("tail");
        let stats = document.update(&text);
        assert_eq!((stats.reused, stats.parsed), (100, 1));

        // A change in the middle re-parses from the block it touches.
        let changed = text.replacen("block number 40\n\n", "block number 4x\n\n", 1);
        let stats = document.update(&changed);
        assert_eq!(stats.reused, 40);
        assert_eq!(stats.parsed, 61);
        assert_eq!(snapshot(&document), full_parse(&changed));

        // An identical text parses nothing but a trailing open block.
        let stats = document.update(&changed);
        assert_eq!((stats.reused, stats.parsed), (100, 1));

        // Shrinking the text discards stale blocks.
        let shorter = &changed[..changed.len() / 2];
        document.update(shorter);
        assert_eq!(snapshot(&document), full_parse(shorter));
        document.update("");
        assert!(document.blocks().is_empty());
    }

    #[test]
    fn ids_of_unchanged_and_grown_blocks_never_change() {
        let mut document = BlockDocument::new(Paragraphs);
        document.update(&corpus(20));
        document.append("growing");
        let ids: Vec<BlockId> = document.blocks().iter().map(|b| b.id).collect();
        assert_eq!(ids.len(), 21);
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());

        let anchor = document
            .anchor_at(document.blocks()[7].range.start + 5)
            .unwrap();
        let selection = BlockSelection {
            anchor,
            focus: document
                .anchor_at(document.blocks()[9].range.start + 2)
                .unwrap(),
        };
        let scroll = document
            .anchor_at(document.blocks()[3].range.start)
            .unwrap();
        let before = document.resolve_selection(selection).unwrap();

        for chunk in [" more", " and more", " and even more"] {
            document.append(chunk);
            let now: Vec<BlockId> = document.blocks().iter().map(|b| b.id).collect();
            assert_eq!(
                now, ids,
                "the trailing block that only grows keeps its identity"
            );
            assert_eq!(document.resolve_selection(selection), Some(before.clone()));
            assert_eq!(
                document.resolve(scroll),
                Some(document.blocks()[3].range.start)
            );
        }
        // Closing the trailing block and starting another creates a new identity for the new one.
        document.append("\n\nfresh");
        assert_eq!(document.blocks().len(), 22);
        assert_eq!(document.blocks()[20].id, ids[20]);
        assert!(!ids.contains(&document.blocks()[21].id));
    }

    #[test]
    fn a_split_keeps_the_identity_on_the_first_part_and_a_merge_on_the_first_block() {
        let mut document = BlockDocument::new(Paragraphs);
        document.update("alpha beta\n\ngamma\n\ndelta");
        let ids: Vec<BlockId> = document.blocks().iter().map(|b| b.id).collect();
        assert_eq!(ids.len(), 3);
        let in_second = document
            .anchor_at(document.blocks()[1].range.start + 2)
            .unwrap();
        let in_first = document.anchor_at(3).unwrap();

        // Split the first block in two by inserting a blank line inside it.
        document.update("alpha\n\nbeta\n\ngamma\n\ndelta");
        let split: Vec<BlockId> = document.blocks().iter().map(|b| b.id).collect();
        assert_eq!(split.len(), 4);
        assert_eq!(split[0], ids[0], "first part keeps the identity");
        assert!(!ids.contains(&split[1]), "second part is new");
        assert_eq!(split[2], ids[1], "later blocks keep theirs (shifted)");
        assert_eq!(split[3], ids[2]);
        assert_eq!(
            document.resolve(in_first),
            Some(3),
            "anchor in the first part survives"
        );
        let moved = document.resolve(in_second).unwrap();
        assert_eq!(
            &document.source()[moved..moved + 3],
            "mma",
            "anchor follows its block"
        );

        // Merge the first two blocks again: the merged block keeps the first identity.
        document.update("alpha\nbeta\n\ngamma\n\ndelta");
        let merged: Vec<BlockId> = document.blocks().iter().map(|b| b.id).collect();
        assert_eq!(merged, vec![split[0], split[2], split[3]]);
        assert!(document.block(split[1]).is_none());
        assert_eq!(
            snapshot(&document),
            full_parse("alpha\nbeta\n\ngamma\n\ndelta")
        );
    }

    #[test]
    fn anchors_clamp_and_disappear_with_their_block() {
        let mut document = BlockDocument::new(Paragraphs);
        document.update("one\n\ntwo\n\nthree");
        let two = document.anchor_at(7).unwrap();
        assert_eq!(two.offset, 2);
        assert_eq!(
            document.anchor_at(document.source().len()).unwrap().block,
            document.blocks()[2].id
        );
        assert_eq!(
            document.resolve(BlockAnchor {
                block: two.block,
                offset: 999
            }),
            Some(document.blocks()[1].range.end)
        );
        document.update("one\n\n");
        assert_eq!(document.resolve(two), None);
        assert!(BlockDocument::new(Paragraphs).anchor_at(0).is_none());
    }

    #[test]
    fn blocks_in_reports_block_relative_ranges() {
        let mut document = BlockDocument::new(Paragraphs);
        document.update("one\n\ntwo\n\nthree");
        let ranges: Vec<_> = document.blocks_in(2..11).collect();
        let ids: Vec<BlockId> = document.blocks().iter().map(|b| b.id).collect();
        assert_eq!(ranges, vec![(ids[0], 2..5), (ids[1], 0..5), (ids[2], 0..1)]);
    }

    #[test]
    fn remeasure_measures_every_block_without_parsing() {
        let mut document = BlockDocument::new(Paragraphs);
        document.update(&corpus(10));
        let before = document.stats();
        assert_eq!(document.remeasure(), 10);
        let after = document.stats();
        assert_eq!(after.blocks_parsed, before.blocks_parsed);
        assert_eq!(after.blocks_measured, before.blocks_measured + 10);
    }

    #[test]
    fn multibyte_text_is_split_on_character_boundaries() {
        let mut document = BlockDocument::new(Paragraphs);
        let text = "héllo wörld\n\nnaïve café ☕\n\nlast";
        document.update(text);
        let changed = "héllo wörld\n\nnaïve cafe ☕\n\nlast";
        document.update(changed);
        assert_eq!(snapshot(&document), full_parse(changed));
    }
}
