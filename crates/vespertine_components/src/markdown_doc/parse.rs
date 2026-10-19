//! The Markdown parser (CMP-011 R1-R3, R19): a block tree with byte ranges built on
//! `pulldown-cmark`, with formula delimiters decided by [`crate::markdown::scan`] and shielded from
//! emphasis, link destinations with spaces wrapped, bare URLs linked, display formulas split out
//! of paragraphs and a nesting cap of 32.

use super::scan::{FormulaSpan, Protected, find_urls, scan_formulas, wrap_destinations};
use super::tree::{Align, Block, BlockKind, Formula, ImageInfo, Item, Run, Style, TaskMarker};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// The nesting cap for blocks and inlines (R1).
pub const MAX_DEPTH: usize = 32;

/// Result of parsing a slice of the source.
pub struct Parsed {
    /// The blocks of the slice, with ranges in whole-source coordinates.
    pub blocks: Vec<Block>,
    /// Whether the slice contains a link reference definition (which acts at a distance).
    pub has_reference_definitions: bool,
}

/// Parses a whole source.
pub fn parse(source: &str) -> Vec<Block> {
    parse_slice(source, 0).blocks
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// Parses `source[base..]`; every range in the result is shifted by `base`.
pub fn parse_slice(source: &str, base: usize) -> Parsed {
    let text = &source[base..];
    let reference_parser = Parser::new_ext(text, options());
    let has_reference_definitions = reference_parser
        .reference_definitions()
        .iter()
        .next()
        .is_some();
    drop(reference_parser);
    let blocks = build(text, base);
    Parsed {
        blocks,
        has_reference_definitions,
    }
}

/// A rewrite of the text before the final parse: formula shielding and link destination wraps.
struct Rewrite {
    text: String,
    /// Positions in `text` of inserted bytes (each one byte long), ascending.
    inserted: Vec<usize>,
    formulas: Vec<FormulaSpan>,
}

impl Rewrite {
    fn identity(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            inserted: Vec::new(),
            formulas: Vec::new(),
        }
    }

    /// Maps an offset of the rewritten text to the original text.
    fn map(&self, offset: usize) -> usize {
        offset - self.inserted.partition_point(|at| *at < offset)
    }

    fn map_range(&self, range: Range<usize>) -> Range<usize> {
        self.map(range.start)..self.map(range.end)
    }
}

fn needs_scan(text: &str) -> bool {
    text.contains('$') || text.contains("\\(") || text.contains("\\[") || text.contains("](/")
}

/// What the first parse tells about the text: inline containers and protected ranges.
struct Survey {
    containers: Vec<Range<usize>>,
    protected: Protected,
}

fn survey(text: &str) -> Survey {
    let mut containers: Vec<Range<usize>> = Vec::new();
    let mut protected: Vec<Range<usize>> = Vec::new();
    // Stack of open block-ish tags: true when the context can hold bare inline events (items).
    let mut stack: Vec<Tag> = Vec::new();
    let mut loose: Option<Range<usize>> = None;
    let flush = |loose: &mut Option<Range<usize>>, containers: &mut Vec<Range<usize>>| {
        if let Some(range) = loose.take() {
            containers.push(range);
        }
    };
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                match &tag {
                    Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell => {
                        flush(&mut loose, &mut containers);
                        containers.push(range.clone());
                    }
                    Tag::CodeBlock(_) | Tag::HtmlBlock => {
                        flush(&mut loose, &mut containers);
                        protected.push(range.clone());
                    }
                    Tag::List(_) | Tag::Item | Tag::BlockQuote(_) | Tag::Table(_) => {
                        flush(&mut loose, &mut containers);
                    }
                    _ => {}
                }
                stack.push(tag);
            }
            Event::End(end) => {
                if matches!(end, TagEnd::Link | TagEnd::Image) {
                    // The part of the link after its last text child is the destination.
                    protected.push(link_tail(text, &range));
                }
                stack.pop();
                if !matches!(
                    end,
                    TagEnd::Emphasis
                        | TagEnd::Strong
                        | TagEnd::Strikethrough
                        | TagEnd::Link
                        | TagEnd::Image
                        | TagEnd::Superscript
                        | TagEnd::Subscript
                ) {
                    flush(&mut loose, &mut containers);
                }
            }
            Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) => {
                protected.push(range.clone());
                extend_loose(&stack, &mut loose, &range);
            }
            Event::Text(_) | Event::SoftBreak | Event::HardBreak => {
                extend_loose(&stack, &mut loose, &range);
            }
            _ => {}
        }
    }
    flush(&mut loose, &mut containers);
    // Bare URLs are links and stay untouched by the formula scan.
    for container in &containers {
        for url in find_urls(&text[container.clone()]) {
            protected.push(container.start + url.start..container.start + url.end);
        }
    }
    containers.sort_by_key(|r| r.start);
    containers.dedup();
    Survey {
        containers,
        protected: Protected::new(protected),
    }
}

fn extend_loose(stack: &[Tag], loose: &mut Option<Range<usize>>, range: &Range<usize>) {
    // Tight list items hold their inline events directly.
    if matches!(stack.last(), Some(Tag::Item)) {
        match loose {
            Some(open) => open.end = range.end,
            None => *loose = Some(range.clone()),
        }
    }
}

/// The destination part of a link or image event range: from the closing bracket to the end.
fn link_tail(text: &str, range: &Range<usize>) -> Range<usize> {
    let slice = &text[range.clone()];
    // Autolinks and bare forms are protected whole; inline links from their `](`.
    match slice.rfind("](") {
        Some(at) => range.start + at..range.end,
        None => range.clone(),
    }
}

fn rewrite(text: &str, formulas_on: bool) -> Rewrite {
    if !needs_scan(text) {
        return Rewrite::identity(text);
    }
    let control = text.bytes().any(|b| b == 1 || b == 2);
    let survey = survey(text);
    let mut formulas: Vec<FormulaSpan> = Vec::new();
    if formulas_on && !control {
        for container in &survey.containers {
            formulas.extend(scan_formulas(text, container.clone(), &survey.protected));
        }
        formulas.sort_by_key(|f| f.range.start);
    }
    // Wrap destinations (outside formulas).
    let mut shielded: Vec<Range<usize>> = survey.protected.ranges().to_vec();
    shielded.extend(formulas.iter().map(|f| f.range.clone()));
    let shield = Protected::new(shielded);
    let mut wraps: Vec<(usize, usize)> = Vec::new();
    for container in &survey.containers {
        wraps.extend(wrap_destinations(text, container.clone(), &shield));
    }
    wraps.sort();
    if formulas.is_empty() && wraps.is_empty() {
        return Rewrite::identity(text);
    }
    // Build the rewritten text: formula bytes become 1 (first) / 2 (continuation), newlines and
    // block prefixes of continuation lines stay.
    let bytes = text.as_bytes();
    let mut shielded_text: Vec<u8> = bytes.to_vec();
    for formula in &formulas {
        let quote_prefix = quote_context(text, formula.range.start);
        let mut at_line_start = false;
        let mut first = true;
        let mut prefix_done = true;
        for i in formula.range.clone() {
            let b = bytes[i];
            if b == b'\n' {
                at_line_start = true;
                prefix_done = false;
                continue;
            }
            if at_line_start && !prefix_done {
                if b == b' ' || b == b'\t' || (quote_prefix && b == b'>') {
                    continue;
                }
                prefix_done = true;
                at_line_start = false;
                shielded_text[i] = 2;
                continue;
            }
            shielded_text[i] = if first { 1 } else { 2 };
            first = false;
        }
    }
    let mut out = Vec::with_capacity(shielded_text.len() + wraps.len() * 2);
    let mut inserted = Vec::new();
    let mut cursor = 0;
    for (start, close) in &wraps {
        out.extend_from_slice(&shielded_text[cursor..*start]);
        inserted.push(out.len());
        out.push(b'<');
        out.extend_from_slice(&shielded_text[*start..*close]);
        inserted.push(out.len());
        out.push(b'>');
        cursor = *close;
    }
    out.extend_from_slice(&shielded_text[cursor..]);
    Rewrite {
        text: String::from_utf8(out).unwrap_or_else(|_| text.to_owned()),
        inserted,
        formulas,
    }
}

/// Whether the line containing `index` starts (after whitespace) with a block quote marker.
fn quote_context(text: &str, index: usize) -> bool {
    let line_start = text[..index].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..].trim_start().starts_with('>')
}

fn build(text: &str, base: usize) -> Vec<Block> {
    // Every formula pair must yield exactly one formula in the parse (R19); a parse that fails
    // is redone with all dollars literal, then with no rewrites at all.
    let attempts = [(true, true), (false, true), (false, false)];
    let mut result = Vec::new();
    for (attempt, (formulas_on, rewrite_on)) in attempts.into_iter().enumerate() {
        let rewrite = if rewrite_on {
            rewrite(text, formulas_on)
        } else {
            Rewrite::identity(text)
        };
        let expected = rewrite.formulas.len();
        let mut builder = Builder::new(text, &rewrite, base);
        builder.run();
        let verified = builder.formulas_emitted == expected;
        result = builder.finish();
        if verified || attempt == 2 {
            break;
        }
    }
    result
}

enum Frame {
    Root(Vec<Block>),
    Quote {
        range: Range<usize>,
        blocks: Vec<Block>,
    },
    List {
        range: Range<usize>,
        start: Option<u64>,
        items: Vec<Item>,
    },
    Item {
        range: Range<usize>,
        blocks: Vec<Block>,
        task: Option<TaskMarker>,
    },
    Para {
        range: Range<usize>,
        runs: Vec<Run>,
        implicit: bool,
    },
    Heading {
        range: Range<usize>,
        level: u8,
        runs: Vec<Run>,
    },
    Code {
        range: Range<usize>,
        info: String,
        fenced: bool,
        code: String,
    },
    Html {
        range: Range<usize>,
        text: String,
    },
    Table {
        range: Range<usize>,
        aligns: Vec<Align>,
        header: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
        in_head: bool,
    },
    Cell(Vec<Run>),
    /// A collapsed subtree (depth cap): text only.
    Collapsed {
        text: String,
    },
}

struct Builder<'a> {
    source: &'a str,
    rewrite: &'a Rewrite,
    base: usize,
    frames: Vec<Frame>,
    style_depth: [usize; 3],
    links: Vec<String>,
    image: Option<(String, String, String, usize)>,
    collapsed_depth: usize,
    inline_depth: usize,
    formula_cursor: usize,
    formulas_emitted: usize,
    formula_newlines_left: usize,
    in_formula: bool,
    kept_depth: usize,
}

impl<'a> Builder<'a> {
    fn new(source: &'a str, rewrite: &'a Rewrite, base: usize) -> Self {
        Self {
            source,
            rewrite,
            base,
            frames: vec![Frame::Root(Vec::new())],
            style_depth: [0; 3],
            links: Vec::new(),
            image: None,
            collapsed_depth: 0,
            inline_depth: 0,
            formula_cursor: 0,
            formulas_emitted: 0,
            formula_newlines_left: 0,
            in_formula: false,
            kept_depth: 0,
        }
    }

    fn range(&self, range: Range<usize>) -> Range<usize> {
        let mapped = self.rewrite.map_range(range);
        mapped.start + self.base..mapped.end + self.base
    }

    fn style(&self) -> Style {
        Style {
            bold: self.style_depth[0] > 0,
            strike: self.style_depth[1] > 0,
            italic: self.style_depth[2] > 0,
            code: false,
        }
    }

    fn run(&mut self) {
        let parser = Parser::new_ext(&self.rewrite.text, options()).into_offset_iter();
        for (event, range) in parser {
            self.event(event, range);
        }
    }

    fn finish(mut self) -> Vec<Block> {
        while self.frames.len() > 1 {
            self.close_frame();
        }
        match self.frames.pop() {
            Some(Frame::Root(blocks)) => split_display(blocks),
            _ => Vec::new(),
        }
    }

    // ----- frames

    fn push_block(&mut self, block: Block) {
        match self.frames.last_mut() {
            Some(Frame::Root(blocks)) | Some(Frame::Quote { blocks, .. }) => blocks.push(block),
            Some(Frame::Item { blocks, .. }) => blocks.push(block),
            Some(Frame::Collapsed { .. }) | None => {}
            _ => {
                // A block inside an inline context (malformed nesting): attach to the nearest
                // block container.
                for frame in self.frames.iter_mut().rev() {
                    match frame {
                        Frame::Root(blocks)
                        | Frame::Quote { blocks, .. }
                        | Frame::Item { blocks, .. } => {
                            blocks.push(block);
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn close_implicit(&mut self) {
        if matches!(self.frames.last(), Some(Frame::Para { implicit: true, .. })) {
            self.close_frame();
        }
    }

    fn close_frame(&mut self) {
        let Some(frame) = self.frames.pop() else {
            return;
        };
        match frame {
            Frame::Root(blocks) => self.frames.push(Frame::Root(blocks)),
            Frame::Quote { range, blocks } => self.push_block(Block {
                range,
                kind: BlockKind::Quote(blocks),
            }),
            Frame::List {
                range,
                start,
                items,
            } => self.push_block(Block {
                range,
                kind: BlockKind::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items,
                },
            }),
            Frame::Item { range, blocks, .. } => {
                if let Some(Frame::List { items, .. }) = self.frames.last_mut() {
                    items.push(Item { range, blocks });
                }
            }
            Frame::Para { range, runs, .. } => {
                let runs = finish_runs(runs);
                self.push_block(Block {
                    range,
                    kind: BlockKind::Paragraph(runs),
                });
            }
            Frame::Heading { range, level, runs } => {
                let runs = finish_runs(runs);
                self.push_block(Block {
                    range,
                    kind: BlockKind::Heading { level, runs },
                })
            }
            Frame::Code {
                range,
                info,
                fenced,
                mut code,
            } => {
                if code.ends_with('\n') {
                    code.pop();
                }
                self.push_block(Block {
                    range,
                    kind: BlockKind::Code { info, fenced, code },
                })
            }
            Frame::Html { range, text } => {
                let text = text.trim_end_matches('\n').to_owned();
                self.push_block(Block {
                    range,
                    kind: BlockKind::Paragraph(vec![Run::text(text)]),
                });
            }
            Frame::Table {
                range,
                aligns,
                header,
                rows,
                ..
            } => self.push_block(Block {
                range,
                kind: BlockKind::Table {
                    aligns,
                    header,
                    rows,
                },
            }),
            Frame::Cell(runs) => {
                let runs = finish_runs(runs);
                if let Some(Frame::Table {
                    header,
                    rows,
                    in_head,
                    ..
                }) = self.frames.last_mut()
                {
                    if *in_head {
                        header.push(runs);
                    } else if let Some(row) = rows.last_mut() {
                        row.push(runs);
                    }
                }
            }
            Frame::Collapsed { .. } => {}
        }
    }

    // ----- inline plumbing

    fn runs_mut(&mut self) -> Option<&mut Vec<Run>> {
        match self.frames.last_mut() {
            Some(Frame::Para { runs, .. })
            | Some(Frame::Heading { runs, .. })
            | Some(Frame::Cell(runs)) => Some(runs),
            _ => None,
        }
    }

    /// Makes sure inline content has a frame to go to (tight list items hold text directly).
    fn ensure_inline(&mut self, range: &Range<usize>) {
        if matches!(
            self.frames.last(),
            Some(Frame::Item { .. }) | Some(Frame::Quote { .. }) | Some(Frame::Root(_))
        ) {
            let range = self.range(range.clone());
            let mut runs = Vec::new();
            if let Some(task) = self.take_item_task() {
                runs.push(self.task_run(&task));
            }
            self.frames.push(Frame::Para {
                range,
                runs,
                implicit: true,
            });
        }
    }

    fn take_item_task(&mut self) -> Option<TaskMarker> {
        match self.frames.last_mut() {
            Some(Frame::Item { task, .. }) => task.take(),
            _ => None,
        }
    }

    /// The first run of a task item: the literal marker and a space.
    fn task_run(&self, task: &TaskMarker) -> Run {
        let start = task.range.start.saturating_sub(self.base);
        let end = task.range.end.saturating_sub(self.base);
        let literal = self.source.get(start..end).unwrap_or("[ ]");
        Run {
            text: format!("{literal} "),
            style: Style::default(),
            link: None,
            formula: None,
            task: Some(task.clone()),
            image: None,
        }
    }

    fn push_run(&mut self, run: Run) {
        if let Some(runs) = self.runs_mut() {
            if run.is_mergeable()
                && let Some(last) = runs.last_mut()
                && last.is_mergeable()
                && last.style == run.style
                && last.link == run.link
            {
                last.text.push_str(&run.text);
                return;
            }
            runs.push(run);
        }
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let style = self.style();
        let link = self.links.last().cloned();
        self.push_run(Run::styled(text, style, link));
    }

    fn emit_formula(&mut self) {
        let Some(span) = self.rewrite.formulas.get(self.formula_cursor).cloned() else {
            return;
        };
        self.formula_cursor += 1;
        self.formulas_emitted += 1;
        let tex = self.source[span.tex.clone()].to_owned();
        self.formula_newlines_left = self.source[span.range.clone()]
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
        self.in_formula = true;
        let mut style = self.style();
        style.code = false;
        let link = self.links.last().cloned();
        let nested = !style.is_plain() || link.is_some();
        let display = span.display && !nested;
        let text = if display {
            format!("$${tex}$$")
        } else {
            format!("${tex}$")
        };
        let run = Run {
            text,
            style,
            link,
            formula: Some(Formula { tex, display }),
            task: None,
            image: None,
        };
        self.push_run(run);
    }

    fn text_event(&mut self, text: &str) {
        if self.rewrite.formulas.is_empty() && !text.bytes().any(|b| b == 1 || b == 2) {
            self.push_text(text);
            return;
        }
        let mut literal = String::new();
        for c in text.chars() {
            match c {
                '\u{1}' => {
                    self.push_text(&std::mem::take(&mut literal));
                    self.emit_formula();
                }
                '\u{2}' => {}
                c => {
                    if self.in_formula && self.formula_newlines_left == 0 {
                        self.in_formula = false;
                    }
                    literal.push(c);
                }
            }
        }
        self.push_text(&literal);
        if self.in_formula && !text.is_empty() && !text.bytes().all(|b| b == 1 || b == 2) {
            self.in_formula = false;
        }
    }

    fn break_event(&mut self, hard: bool) {
        if self.in_formula && self.formula_newlines_left > 0 {
            self.formula_newlines_left -= 1;
            return;
        }
        self.in_formula = false;
        self.push_text(if hard { "\n" } else { " " });
    }

    // ----- events

    fn event(&mut self, event: Event, range: Range<usize>) {
        if self.collapsed_depth > 0 {
            self.collapsed_event(event);
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(end) => self.end(end),
            Event::Text(text) => {
                if self.in_image() {
                    self.image_text(&text);
                    return;
                }
                match self.frames.last_mut() {
                    Some(Frame::Code { code, .. }) => code.push_str(&text),
                    Some(Frame::Html { text: html, .. }) => html.push_str(&text),
                    _ => {
                        self.ensure_inline(&range);
                        self.text_event(&text);
                    }
                }
            }
            Event::Code(code) => {
                if self.in_image() {
                    self.image_text(&code);
                    return;
                }
                self.ensure_inline(&range);
                let mut style = self.style();
                style.code = true;
                let link = self.links.last().cloned();
                self.push_run(Run::styled(code.to_string(), style, link));
            }
            Event::Html(html) | Event::InlineHtml(html) => match self.frames.last_mut() {
                Some(Frame::Html { text, .. }) => text.push_str(&html),
                _ => {
                    self.ensure_inline(&range);
                    self.text_event(&html);
                }
            },
            Event::SoftBreak => {
                if self.in_image() {
                    self.image_text(" ");
                } else {
                    self.ensure_inline(&range);
                    self.break_event(false);
                }
            }
            Event::HardBreak => {
                if self.in_image() {
                    self.image_text(" ");
                } else {
                    self.ensure_inline(&range);
                    self.break_event(true);
                }
            }
            Event::Rule => {
                self.close_implicit();
                let range = self.range(range);
                self.push_block(Block {
                    range,
                    kind: BlockKind::Rule,
                });
            }
            Event::TaskListMarker(checked) => {
                let range = self.range(range);
                let marker = TaskMarker { range, checked };
                if let Some(Frame::Item { task, .. }) = self.frames.last_mut() {
                    *task = Some(marker);
                }
            }
            Event::FootnoteReference(label) => {
                self.ensure_inline(&range);
                self.push_text(&format!("[^{label}]"));
            }
            Event::InlineMath(tex) => {
                self.ensure_inline(&range);
                self.push_text(&format!("${tex}$"));
            }
            Event::DisplayMath(tex) => {
                self.ensure_inline(&range);
                self.push_text(&format!("$${tex}$$"));
            }
        }
    }

    fn in_image(&self) -> bool {
        self.image.is_some()
    }

    fn image_text(&mut self, text: &str) {
        if let Some((_, alt, _, _)) = self.image.as_mut() {
            alt.push_str(text);
        }
    }

    fn collapsed_event(&mut self, event: Event) {
        let text = match event {
            Event::Start(_) => {
                self.collapsed_depth += 1;
                return;
            }
            Event::End(end) => {
                self.collapsed_depth -= 1;
                let block_end = !matches!(
                    end,
                    TagEnd::Emphasis
                        | TagEnd::Strong
                        | TagEnd::Strikethrough
                        | TagEnd::Link
                        | TagEnd::Image
                        | TagEnd::Superscript
                        | TagEnd::Subscript
                );
                if let Some(Frame::Collapsed { text }) = self.frames.last_mut()
                    && block_end
                    && !text.is_empty()
                    && !text.ends_with('\n')
                {
                    text.push('\n');
                }
                if self.collapsed_depth == 0 {
                    self.flush_collapsed();
                }
                return;
            }
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                t.to_string()
            }
            Event::SoftBreak => " ".to_owned(),
            Event::HardBreak => "\n".to_owned(),
            Event::InlineMath(t) => format!("${t}$"),
            Event::DisplayMath(t) => format!("$${t}$$"),
            Event::FootnoteReference(l) => format!("[^{l}]"),
            Event::Rule | Event::TaskListMarker(_) => return,
        };
        if let Some(Frame::Collapsed { text: buffer }) = self.frames.last_mut() {
            buffer.push_str(&text.replace(['\u{1}', '\u{2}'], ""));
        }
    }

    fn flush_collapsed(&mut self) {
        let Some(Frame::Collapsed { text }) = self.frames.pop() else {
            return;
        };
        let text = text.trim_end_matches('\n').to_owned();
        if text.is_empty() {
            return;
        }
        if self.runs_mut().is_some() {
            self.push_text(&text);
        } else {
            let range = 0..0;
            self.push_block(Block {
                range,
                kind: BlockKind::Paragraph(vec![Run::text(text)]),
            });
        }
    }

    fn start_collapse(&mut self) {
        self.collapsed_depth = 1;
        self.frames.push(Frame::Collapsed {
            text: String::new(),
        });
    }

    fn start(&mut self, tag: Tag, range: Range<usize>) {
        let block_start = !matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::Superscript
                | Tag::Subscript
                | Tag::TableHead
                | Tag::TableRow
                | Tag::TableCell
        );
        if block_start {
            self.close_implicit();
        }
        let is_container = matches!(tag, Tag::BlockQuote(_) | Tag::List(_) | Tag::Item);
        let inline = matches!(
            tag,
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
        );
        if is_container && self.kept_depth >= MAX_DEPTH {
            self.start_collapse();
            return;
        }
        if inline && self.inline_depth >= MAX_DEPTH {
            return self.start_inline_collapse();
        }
        match tag {
            Tag::Paragraph => {
                let range = self.range(range);
                let mut runs = Vec::new();
                if let Some(task) = self.take_item_task() {
                    runs.push(self.task_run(&task));
                }
                self.frames.push(Frame::Para {
                    range,
                    runs,
                    implicit: false,
                });
            }
            Tag::Heading { level, .. } => {
                let range = self.range(range);
                self.frames.push(Frame::Heading {
                    range,
                    level: heading_level(level),
                    runs: Vec::new(),
                });
            }
            Tag::BlockQuote(_) => {
                self.kept_depth += 1;
                let range = self.range(range);
                self.frames.push(Frame::Quote {
                    range,
                    blocks: Vec::new(),
                });
            }
            Tag::CodeBlock(kind) => {
                let (info, fenced) = match kind {
                    CodeBlockKind::Fenced(info) => (info.trim().to_owned(), true),
                    CodeBlockKind::Indented => (String::new(), false),
                };
                let range = self.range(range);
                self.frames.push(Frame::Code {
                    range,
                    info,
                    fenced,
                    code: String::new(),
                });
            }
            Tag::HtmlBlock => {
                let range = self.range(range);
                self.frames.push(Frame::Html {
                    range,
                    text: String::new(),
                });
            }
            Tag::List(start) => {
                self.kept_depth += 1;
                let range = self.range(range);
                self.frames.push(Frame::List {
                    range,
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.kept_depth += 1;
                let range = self.range(range);
                self.frames.push(Frame::Item {
                    range,
                    blocks: Vec::new(),
                    task: None,
                });
            }
            Tag::Table(aligns) => {
                let range = self.range(range);
                self.frames.push(Frame::Table {
                    range,
                    aligns: aligns
                        .into_iter()
                        .map(|a| match a {
                            Alignment::Center => Align::Center,
                            Alignment::Right => Align::Right,
                            _ => Align::Left,
                        })
                        .collect(),
                    header: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(Frame::Table { in_head, .. }) = self.frames.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(Frame::Table { in_head, rows, .. }) = self.frames.last_mut()
                    && !*in_head
                {
                    rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.frames.push(Frame::Cell(Vec::new())),
            Tag::Emphasis => {
                self.inline_depth += 1;
                self.ensure_inline(&range);
                self.style_depth[2] += 1;
            }
            Tag::Strong => {
                self.inline_depth += 1;
                self.ensure_inline(&range);
                self.style_depth[0] += 1;
            }
            Tag::Strikethrough => {
                self.inline_depth += 1;
                self.ensure_inline(&range);
                self.style_depth[1] += 1;
            }
            Tag::Link { dest_url, .. } => {
                self.inline_depth += 1;
                self.ensure_inline(&range);
                self.links.push(dest_url.to_string());
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                self.inline_depth += 1;
                self.ensure_inline(&range);
                self.image = Some((dest_url.to_string(), String::new(), title.to_string(), 0));
            }
            // Unknown containers (footnote definitions, definition lists, metadata): their
            // children are spliced into the parent.
            _ => {}
        }
    }

    fn start_inline_collapse(&mut self) {
        // Inline depth cap: the deeper subtree becomes text of the innermost kept inline.
        self.collapsed_depth = 1;
        self.frames.push(Frame::Collapsed {
            text: String::new(),
        });
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::Table
            | TagEnd::TableCell => {
                self.close_implicit();
                self.close_frame();
            }
            TagEnd::BlockQuote(_) | TagEnd::List(_) => {
                self.close_implicit();
                self.kept_depth = self.kept_depth.saturating_sub(1);
                self.close_frame();
            }
            TagEnd::Item => {
                self.close_implicit();
                self.kept_depth = self.kept_depth.saturating_sub(1);
                self.close_frame();
            }
            TagEnd::TableHead => {
                if let Some(Frame::Table { in_head, .. }) = self.frames.last_mut() {
                    *in_head = false;
                }
            }
            TagEnd::TableRow => {}
            TagEnd::Emphasis => {
                self.inline_depth = self.inline_depth.saturating_sub(1);
                self.style_depth[2] = self.style_depth[2].saturating_sub(1);
            }
            TagEnd::Strong => {
                self.inline_depth = self.inline_depth.saturating_sub(1);
                self.style_depth[0] = self.style_depth[0].saturating_sub(1);
            }
            TagEnd::Strikethrough => {
                self.inline_depth = self.inline_depth.saturating_sub(1);
                self.style_depth[1] = self.style_depth[1].saturating_sub(1);
            }
            TagEnd::Link => {
                self.inline_depth = self.inline_depth.saturating_sub(1);
                self.links.pop();
            }
            TagEnd::Image => {
                self.inline_depth = self.inline_depth.saturating_sub(1);
                if let Some((dest, alt, title, _)) = self.image.take() {
                    let label = if alt.is_empty() {
                        dest.clone()
                    } else {
                        alt.clone()
                    };
                    let mut style = self.style();
                    style.code = false;
                    let run = Run {
                        text: label,
                        style,
                        link: Some(dest.clone()),
                        formula: None,
                        task: None,
                        image: Some(ImageInfo { dest, alt, title }),
                    };
                    self.push_run(run);
                }
            }
            _ => {}
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    // The levels are numbered 1 to 6.
    level as u8
}

/// Final inline pass: bare URLs become links and adjacent runs of identical style merge (R2).
pub fn finish_runs(runs: Vec<Run>) -> Vec<Run> {
    let mut linked: Vec<Run> = Vec::with_capacity(runs.len());
    for run in runs {
        if run.is_mergeable() && run.link.is_none() && !run.style.code {
            let urls = find_urls(&run.text);
            if !urls.is_empty() {
                let mut cursor = 0;
                for url in urls {
                    if url.start > cursor {
                        linked.push(Run::styled(&run.text[cursor..url.start], run.style, None));
                    }
                    let text = &run.text[url.clone()];
                    linked.push(Run::styled(text, run.style, Some(text.to_owned())));
                    cursor = url.end;
                }
                if cursor < run.text.len() {
                    linked.push(Run::styled(&run.text[cursor..], run.style, None));
                }
                continue;
            }
        }
        linked.push(run);
    }
    let mut merged: Vec<Run> = Vec::with_capacity(linked.len());
    for run in linked {
        if run.text.is_empty() && run.is_mergeable() {
            continue;
        }
        match merged.last_mut() {
            Some(last)
                if last.is_mergeable()
                    && run.is_mergeable()
                    && last.style == run.style
                    && last.link == run.link =>
            {
                last.text.push_str(&run.text)
            }
            _ => merged.push(run),
        }
    }
    merged
}

/// Splits paragraphs at display formulas (R13): a display formula alone in its paragraph is a
/// block of its own; one mid-paragraph ends the text before it and starts a new paragraph after
/// it. All pieces share the paragraph's source range. Containers are processed recursively.
fn split_display(blocks: Vec<Block>) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        let Block { range, kind } = block;
        match kind {
            BlockKind::Paragraph(runs) => {
                if !runs
                    .iter()
                    .any(|run| run.formula.as_ref().is_some_and(|f| f.display))
                {
                    out.push(Block {
                        range,
                        kind: BlockKind::Paragraph(runs),
                    });
                    continue;
                }
                let mut pending: Vec<Run> = Vec::new();
                let flush = |pending: &mut Vec<Run>, out: &mut Vec<Block>, range: &Range<usize>| {
                    let runs = trim_break_runs(std::mem::take(pending));
                    if !runs.is_empty() {
                        out.push(Block {
                            range: range.clone(),
                            kind: BlockKind::Paragraph(runs),
                        });
                    }
                };
                for run in runs {
                    if let Some(formula) = run.formula.clone()
                        && formula.display
                    {
                        flush(&mut pending, &mut out, &range);
                        out.push(Block {
                            range: range.clone(),
                            kind: BlockKind::DisplayMath(formula),
                        });
                    } else {
                        pending.push(run);
                    }
                }
                flush(&mut pending, &mut out, &range);
            }
            BlockKind::Quote(children) => out.push(Block {
                range,
                kind: BlockKind::Quote(split_display(children)),
            }),
            BlockKind::List {
                ordered,
                start,
                items,
            } => out.push(Block {
                range,
                kind: BlockKind::List {
                    ordered,
                    start,
                    items: items
                        .into_iter()
                        .map(|item| Item {
                            range: item.range,
                            blocks: split_display(item.blocks),
                        })
                        .collect(),
                },
            }),
            other => out.push(Block { range, kind: other }),
        }
    }
    out
}

/// Drops whitespace-only text at the edges of a split paragraph piece.
fn trim_break_runs(mut runs: Vec<Run>) -> Vec<Run> {
    while runs
        .first()
        .is_some_and(|run| run.is_mergeable() && run.text.trim().is_empty() && run.link.is_none())
    {
        runs.remove(0);
    }
    while runs
        .last()
        .is_some_and(|run| run.is_mergeable() && run.text.trim().is_empty() && run.link.is_none())
    {
        runs.pop();
    }
    runs
}
