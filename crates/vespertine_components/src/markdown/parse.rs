//! One CommonMark pass (strikethrough and task lists only) that keeps exact source ranges.

use super::Face;
use super::text::{is_blank, line_start, trim_terminator};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// Inline elements whose delimiters the layer can hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InlineKind {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
}

/// An inline element with its full parser range, delimiters included.
#[derive(Clone, Debug)]
pub(crate) struct InlineElement {
    pub range: Range<usize>,
    pub kind: InlineKind,
}

/// An ATX or setext heading.
#[derive(Clone, Debug)]
pub(crate) struct Heading {
    pub range: Range<usize>,
    /// Number of enclosing quote and list-item containers.
    pub container_depth: usize,
}

/// A fenced or indented code block.
#[derive(Clone, Debug)]
pub(crate) struct CodeBlock {
    pub range: Range<usize>,
    pub fenced: bool,
    /// Info string of a fence (empty for indented blocks).
    pub info: String,
    /// Source ranges of the body text, container prefixes excluded.
    pub segments: Vec<Range<usize>>,
    /// End of the block for [`CodeBlock::contains`].
    code_end: usize,
    /// Whether an offset equal to `code_end` still counts as inside the block.
    end_inclusive: bool,
}

impl CodeBlock {
    /// R9: strictly inside the block, or at its end for indented and unclosed fenced blocks.
    pub(crate) fn contains(&self, offset: usize) -> bool {
        self.range.start < offset
            && (offset < self.code_end || (self.end_inclusive && offset == self.code_end))
    }
}

/// Everything the layer needs from one parse.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    /// Face ranges in document order.
    pub faces: Vec<(Range<usize>, Face)>,
    pub inline: Vec<InlineElement>,
    /// Inline code spans, sorted and non-overlapping.
    pub code_spans: Vec<Range<usize>>,
    pub headings: Vec<Heading>,
    /// Ranges of list items; each starts at the item's marker.
    pub items: Vec<Range<usize>>,
    pub blocks: Vec<CodeBlock>,
}

impl Parsed {
    pub(crate) fn parse(source: &str) -> Parsed {
        let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let mut parsed = Parsed::default();
        let mut containers = 0usize;
        let mut open_block = false;
        for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Strong => parsed.push_inline(range, InlineKind::Strong, Face::Bold),
                    Tag::Emphasis => parsed.push_inline(range, InlineKind::Emphasis, Face::Italic),
                    Tag::Strikethrough => {
                        parsed.push_inline(range, InlineKind::Strikethrough, Face::Strikethrough)
                    }
                    Tag::Heading { .. } => {
                        parsed.faces.push((range.clone(), Face::Bold));
                        parsed.headings.push(Heading {
                            range,
                            container_depth: containers,
                        });
                    }
                    Tag::BlockQuote(_) => containers += 1,
                    Tag::Item => {
                        containers += 1;
                        parsed.items.push(range);
                    }
                    Tag::CodeBlock(kind) => {
                        let (fenced, info) = match kind {
                            CodeBlockKind::Fenced(info) => (true, info.to_string()),
                            CodeBlockKind::Indented => (false, String::new()),
                        };
                        parsed.faces.push((range.clone(), Face::Code));
                        parsed.blocks.push(CodeBlock {
                            code_end: range.end,
                            range,
                            fenced,
                            info,
                            segments: Vec::new(),
                            end_inclusive: !fenced,
                        });
                        open_block = true;
                    }
                    _ => {}
                },
                Event::End(end) => match end {
                    TagEnd::BlockQuote(_) | TagEnd::Item => {
                        containers = containers.saturating_sub(1)
                    }
                    TagEnd::CodeBlock => {
                        open_block = false;
                        if let Some(block) = parsed.blocks.last_mut() {
                            block.finish(source);
                        }
                    }
                    _ => {}
                },
                Event::Code(_) => parsed.push_inline(range, InlineKind::Code, Face::Code),
                Event::Text(_) if open_block => {
                    if let Some(block) = parsed.blocks.last_mut() {
                        block.segments.push(range);
                    }
                }
                _ => {}
            }
        }
        parsed
    }

    fn push_inline(&mut self, range: Range<usize>, kind: InlineKind, face: Face) {
        self.faces.push((range.clone(), face));
        if kind == InlineKind::Code {
            self.code_spans.push(range.clone());
        }
        self.inline.push(InlineElement { range, kind });
    }

    /// The code block whose R9 range contains `offset`, if any (blocks never overlap).
    pub(crate) fn block_containing(&self, offset: usize) -> Option<&CodeBlock> {
        let index = self
            .blocks
            .partition_point(|block| block.range.start < offset);
        index
            .checked_sub(1)
            .map(|index| &self.blocks[index])
            .filter(|block| block.contains(offset))
    }

    /// The code block whose whole range covers `index`.
    pub(crate) fn block_covering(&self, index: usize) -> Option<&CodeBlock> {
        let position = self
            .blocks
            .partition_point(|block| block.range.start <= index);
        position
            .checked_sub(1)
            .map(|position| &self.blocks[position])
            .filter(|block| block.range.end > index)
    }

    /// A heading whose range overlaps `range` (headings never overlap one another).
    pub(crate) fn heading_overlapping(&self, range: &Range<usize>) -> Option<&Heading> {
        let index = self
            .headings
            .partition_point(|heading| heading.range.start < range.end);
        index
            .checked_sub(1)
            .map(|index| &self.headings[index])
            .filter(|heading| heading.range.end > range.start)
    }

    /// Whether a parsed inline code span contains `offset` strictly inside.
    pub(crate) fn inside_inline_code(&self, offset: usize) -> bool {
        self.code_span_ending_after(offset)
            .is_some_and(|range| range.start < offset)
    }

    /// The first inline code span that ends after `offset`.
    pub(crate) fn code_span_ending_after(&self, offset: usize) -> Option<&Range<usize>> {
        let index = self.code_spans.partition_point(|range| range.end <= offset);
        self.code_spans.get(index)
    }
}

impl CodeBlock {
    /// Decides the R9 end semantics once the block's events are complete.
    fn finish(&mut self, source: &str) {
        let trimmed = trim_terminator(source, self.range.end);
        if !self.fenced {
            self.code_end = trimmed;
            self.end_inclusive = true;
            return;
        }
        if self.fence_closed(source, trimmed) {
            self.code_end = trimmed;
            self.end_inclusive = false;
        } else {
            // An unclosed fence runs to the end of its container; at the very end of the
            // document the caret may sit on a fresh line that is still inside the fence.
            self.code_end = if self.range.end < source.len() {
                trimmed
            } else {
                self.range.end
            };
            self.end_inclusive = true;
        }
    }

    /// A fenced block is closed when it has a closing line: more than the opening line, a last
    /// line that is not body text per the parser, and that is only the fence character repeated
    /// at least as many times as the opening, once quote markers are stripped.
    fn fence_closed(&self, source: &str, trimmed_end: usize) -> bool {
        let bytes = source.as_bytes();
        let start = self.range.start;
        let Some(&fence) = bytes.get(start) else {
            return false;
        };
        if fence != b'`' && fence != b'~' {
            return false;
        }
        let opening = bytes[start..]
            .iter()
            .take_while(|&&byte| byte == fence)
            .count();
        let Some(newline) = bytes[start..trimmed_end]
            .iter()
            .position(|&byte| byte == b'\n')
        else {
            return false;
        };
        if start + newline >= trimmed_end {
            return false;
        }
        let last_start = line_start(source, trimmed_end);
        if self
            .segments
            .last()
            .is_some_and(|segment| segment.end > last_start)
        {
            return false;
        }
        let mut line = &source[last_start..trimmed_end];
        loop {
            line = line.trim_start_matches([' ', '\t']);
            match line.strip_prefix('>') {
                Some(rest) => line = rest,
                None => break,
            }
        }
        let line = line.trim_end_matches([' ', '\t', '\r']);
        !is_blank(line) && line.bytes().all(|byte| byte == fence) && line.len() >= opening
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(source: &str) -> CodeBlock {
        Parsed::parse(source).blocks.remove(0)
    }

    #[test]
    fn fenced_blocks_know_whether_they_are_closed() {
        assert!(block("```\n- item").end_inclusive);
        assert!(block("```\ncode\n```").code_end == 12 && !block("```\ncode\n```").end_inclusive);
        // Four backticks closed only by three: the last line is body text.
        assert!(block("````\ncode\n```").end_inclusive);
        assert!(block("```").end_inclusive);
        assert!(block("```\n").end_inclusive);
        assert!(!block("~~~\ncode\n~~~~\n").end_inclusive);
    }

    #[test]
    fn indented_blocks_end_inclusive_without_the_terminator() {
        let source = "    code\nplain";
        let block = block(source);
        assert!(block.end_inclusive);
        assert_eq!(block.code_end, 8);
    }
}
