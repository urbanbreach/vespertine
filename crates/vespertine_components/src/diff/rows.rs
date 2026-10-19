//! The row model: flattening a parsed patch into virtual rows (CMP-015 R6-R8, R20, R24, R25,
//! R27, R29).
//!
//! Everything here is pure data. The row list depends on the parsed patch, the collapsed files,
//! the line cap, the presentation mode, the annotations and the fold stand-ins; it never depends
//! on theme colors. Heights are analytic: a color-only theme change keeps every row and every
//! height, while a metrics change rebuilds the list sizing.

use super::model::{DiffFile, DiffLine, LineKind, file_notices};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use vespertine_theme::document::DocumentMetrics;

/// Width of one digit in the number gutters (R11).
pub const GUTTER_DIGIT_WIDTH: f32 = 6.6;
/// Constant part of the gutter width: 8 px right padding plus a 6 px left gap (R11).
pub const GUTTER_PADDING: f32 = 14.;
/// Narrowest number gutter (R11).
pub const GUTTER_MIN_WIDTH: f32 = 36.;
/// Height of a draft (editing) row (R27).
pub const DRAFT_HEIGHT: f32 = 116.;
/// Most content a fold stand-in builds, in px (R26).
pub const STAND_IN_MAX_PX: f32 = 2400.;

/// Gutter width `G = max(digits(max(max_line, 1)) x 6.6 + 14, 36)` (R11).
pub fn gutter_width(max_line: u32) -> f32 {
    let digits = max_line.max(1).ilog10() + 1;
    (digits as f32 * GUTTER_DIGIT_WIDTH + GUTTER_PADDING).max(GUTTER_MIN_WIDTH)
}

/// Unified or side-by-side presentation (R20).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DiffMode {
    /// One column of lines with old and new gutters.
    #[default]
    Unified,
    /// Two halves: old on the left, new on the right.
    Split,
}

/// Which version of a file a line number refers to (R16, R27).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// The old (pre-change) file.
    Old,
    /// The new (post-change) file.
    New,
}

/// A side-aware source location (R16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceRef {
    /// The side the number belongs to.
    pub side: Side,
    /// The line number on that side.
    pub line: u32,
}

/// The source reference of a line for highlighting and navigation (R16): deleted = (old, old
/// number); added = (new, new number); context = (new, new number) when a new document exists,
/// else (old, old number); meta = none.
pub fn line_source_ref(line: &DiffLine, has_new_document: bool) -> Option<SourceRef> {
    match line.kind {
        LineKind::Deleted => Some(SourceRef {
            side: Side::Old,
            line: line.old?,
        }),
        LineKind::Added => Some(SourceRef {
            side: Side::New,
            line: line.new?,
        }),
        LineKind::Context => match (line.new, line.old) {
            (Some(new), _) if has_new_document => Some(SourceRef {
                side: Side::New,
                line: new,
            }),
            (_, Some(old)) if !has_new_document => Some(SourceRef {
                side: Side::Old,
                line: old,
            }),
            (Some(new), None) => Some(SourceRef {
                side: Side::New,
                line: new,
            }),
            _ => None,
        },
        LineKind::Meta => None,
    }
}

/// The annotation anchor of a line (R27): a deleted line anchors to its old number, every other
/// non-meta line to its new number (context lines annotate the new side); meta lines are not
/// annotatable.
pub fn annotation_anchor(line: &DiffLine) -> Option<SourceRef> {
    match line.kind {
        LineKind::Deleted => line.old.map(|old| SourceRef {
            side: Side::Old,
            line: old,
        }),
        LineKind::Added | LineKind::Context => line.new.map(|new| SourceRef {
            side: Side::New,
            line: new,
        }),
        LineKind::Meta => None,
    }
}

/// A review annotation anchored to `(path, side, line)` (R27).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Annotation {
    /// Identity used by the edit and remove callbacks.
    pub id: String,
    /// Display path of the file (the post-rename path).
    pub path: String,
    /// The side of the anchor.
    pub side: Side,
    /// The line number on that side.
    pub line: u32,
    /// The note text.
    pub body: String,
}

/// An annotation being edited (R27): at its anchor a draft row of height 116 is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// Display path of the file.
    pub path: String,
    /// The side of the anchor.
    pub side: Side,
    /// The line number on that side.
    pub line: u32,
    /// The annotation being edited (replaced by the draft row), `None` for a new note.
    pub editing: Option<String>,
}

/// Analytic height of an annotation card (R29): `20 + 22 + 18 x body lines + 6`, where body
/// lines are the sum over the body's logical lines of `max(1, ceil(characters / 64))`, clamped
/// to 1..=8.
pub fn card_height(body: &str) -> f32 {
    let lines: usize = body
        .split('\n')
        .map(|line| line.chars().count().div_ceil(64).max(1))
        .sum();
    20. + 22. + 18. * lines.clamp(1, 8) as f32 + 6.
}

/// Fixed row heights (R8). The row model is independent of colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowMetrics {
    /// File header row height, default 36.
    pub file_header: f32,
    /// Notice row height, default 24.
    pub notice: f32,
    /// Hunk header row height, default 28.
    pub hunk_header: f32,
    /// Line row height, default 21.
    pub line: f32,
    /// Bottom padding row height, default 8.
    pub bottom_pad: f32,
    /// Show-more row height (the file header height), default 36.
    pub show_more: f32,
}

impl Default for RowMetrics {
    fn default() -> Self {
        Self::from_theme(&DocumentMetrics::default())
    }
}

impl RowMetrics {
    /// The row heights of a theme's metrics; the show-more row is as high as a file header.
    pub fn from_theme(metrics: &DocumentMetrics) -> Self {
        Self {
            file_header: metrics.diff_file_header_height,
            notice: metrics.diff_notice_height,
            hunk_header: metrics.diff_hunk_header_height,
            line: metrics.diff_line_height,
            bottom_pad: metrics.diff_body_bottom_pad,
            show_more: metrics.diff_file_header_height,
        }
    }

    /// The same metrics with another line height (R23).
    pub fn with_line_height(mut self, line: f32) -> Self {
        self.line = line;
        self
    }
}

/// The diff text size and line height for an owner-scaled code font size (R23): the text size is
/// `clamp(code x 12 / 12.5)` (the text-size clamp of 8..=32) and the line height is
/// `text size x 21 / 12`, so the default size reproduces 12 and 21.
pub fn scaled_text_metrics(code_size: f32) -> (f32, f32) {
    let size = (code_size * 12. / 12.5).clamp(8., 32.);
    (size, size * 21. / 12.)
}

/// One virtual row of the flattened viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A file header.
    FileHeader {
        /// File index.
        file: usize,
    },
    /// A notice of the file (index into [`file_notices`]).
    Notice {
        /// File index.
        file: usize,
        /// Notice index.
        index: usize,
    },
    /// A hunk header.
    HunkHeader {
        /// File index.
        file: usize,
        /// Hunk index.
        hunk: usize,
    },
    /// A line row of the unified layout (code and meta lines).
    Line {
        /// File index.
        file: usize,
        /// Hunk index.
        hunk: usize,
        /// Line index in the hunk.
        line: usize,
    },
    /// A row of the split layout: indices of the left (old) and right (new) lines.
    Split {
        /// File index.
        file: usize,
        /// Hunk index.
        hunk: usize,
        /// Left line, or an empty filler.
        left: Option<usize>,
        /// Right line, or an empty filler.
        right: Option<usize>,
    },
    /// An annotation card (index into the annotation list).
    Card {
        /// File index.
        file: usize,
        /// Annotation index.
        annotation: usize,
    },
    /// The draft editing row.
    Draft {
        /// File index.
        file: usize,
    },
    /// The bottom padding after a file's body.
    BottomPad {
        /// File index.
        file: usize,
    },
    /// The show-more row with the remaining line count.
    ShowMore {
        /// Number of remaining line rows.
        remaining: usize,
    },
    /// The clipped stand-in row of a folding file (index into [`Flat::stand_ins`]).
    StandIn {
        /// File index.
        file: usize,
        /// Index into [`Flat::stand_ins`].
        index: usize,
    },
}

impl Row {
    /// The file a row belongs to (`None` for the show-more row).
    pub fn file(&self) -> Option<usize> {
        match *self {
            Row::FileHeader { file }
            | Row::Notice { file, .. }
            | Row::HunkHeader { file, .. }
            | Row::Line { file, .. }
            | Row::Split { file, .. }
            | Row::Card { file, .. }
            | Row::Draft { file }
            | Row::BottomPad { file }
            | Row::StandIn { file, .. } => Some(file),
            Row::ShowMore { .. } => None,
        }
    }
}

/// The clipped stand-in of a folding file (R26): the first rows a clip can reveal and the
/// analytic height of the whole body.
#[derive(Clone, Debug, PartialEq)]
pub struct StandIn {
    /// File index.
    pub file: usize,
    /// The body rows that fit in [`STAND_IN_MAX_PX`] (never the whole body of a huge file).
    pub rows: Vec<Row>,
    /// Analytic height of the file's entire expanded body.
    pub body_height: f32,
}

/// The row range of one file in [`Flat::rows`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRows {
    /// File index.
    pub file: usize,
    /// Rows from the header (inclusive) to the end of the file's rows (exclusive).
    pub range: Range<usize>,
}

/// The flattened rows, the per-file row ranges and the fold stand-ins.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flat {
    /// Rows in order.
    pub rows: Vec<Row>,
    /// Per-file row ranges in order (files after a cap are absent).
    pub files: Vec<FileRows>,
    /// Stand-ins referenced by [`Row::StandIn`].
    pub stand_ins: Vec<StandIn>,
}

/// Everything a flatten depends on.
pub struct FlattenOptions<'a> {
    /// Row heights.
    pub metrics: RowMetrics,
    /// Global cap on line rows (R7).
    pub max_lines: Option<usize>,
    /// Collapsed display paths (R6).
    pub collapsed: &'a HashSet<String>,
    /// Unified or split.
    pub mode: DiffMode,
    /// Review annotations (R27).
    pub annotations: &'a [Annotation],
    /// The draft being edited.
    pub draft: Option<&'a Draft>,
    /// Files presented as stand-ins instead of steady rows (R26), by file index.
    pub folding: &'a HashSet<usize>,
}

/// One pair of a split row: the left (old) and right (new) line indices (R20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitPair {
    /// Line shown in the left column.
    pub left: Option<usize>,
    /// Line shown in the right column.
    pub right: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LastCode {
    Deleted,
    Added,
}

/// The pending block of the pairing machine of R20.
#[derive(Default)]
struct Block {
    deleted: Vec<usize>,
    added: Vec<usize>,
    deleted_meta: Vec<usize>,
    added_meta: Vec<usize>,
    deleted_count: usize,
    added_count: usize,
    deleted_meta_count: usize,
    added_meta_count: usize,
    last: Option<LastCode>,
}

impl Block {
    fn rows(&self) -> usize {
        self.deleted_count.max(self.added_count)
            + self.deleted_meta_count.max(self.added_meta_count)
    }

    fn has_meta(&self) -> bool {
        self.deleted_meta_count + self.added_meta_count > 0
    }
}

/// The pairing machine of R20. With `record` the rows are produced (at most `limit`); without it
/// only the number of rows is counted (no allocation proportional to a run).
fn pair_machine(lines: &[DiffLine], limit: Option<usize>, record: bool) -> (Vec<SplitPair>, usize) {
    let mut out: Vec<SplitPair> = Vec::new();
    let mut total = 0usize;
    let mut block = Block::default();
    let limit = limit.unwrap_or(usize::MAX);

    fn flush(
        block: &mut Block,
        out: &mut Vec<SplitPair>,
        total: &mut usize,
        limit: usize,
        record: bool,
    ) {
        if record {
            for index in 0..block.deleted_count.max(block.added_count) {
                if out.len() >= limit {
                    break;
                }
                out.push(SplitPair {
                    left: block.deleted.get(index).copied(),
                    right: block.added.get(index).copied(),
                });
            }
            for index in 0..block.deleted_meta_count.max(block.added_meta_count) {
                if out.len() >= limit {
                    break;
                }
                out.push(SplitPair {
                    left: block.deleted_meta.get(index).copied(),
                    right: block.added_meta.get(index).copied(),
                });
            }
        }
        *total += block.rows();
        *block = Block::default();
    }

    for (index, line) in lines.iter().enumerate() {
        if record && out.len() >= limit {
            break;
        }
        match line.kind {
            LineKind::Deleted => {
                if block.added_count > 0 || block.has_meta() {
                    flush(&mut block, &mut out, &mut total, limit, record);
                }
                if record && block.deleted.len() < limit {
                    block.deleted.push(index);
                }
                block.deleted_count += 1;
                block.last = Some(LastCode::Deleted);
            }
            LineKind::Added => {
                if block.added_meta_count > 0 {
                    flush(&mut block, &mut out, &mut total, limit, record);
                }
                if record && block.added.len() < limit {
                    block.added.push(index);
                }
                block.added_count += 1;
                block.last = Some(LastCode::Added);
            }
            LineKind::Meta => {
                let to_deleted = block.last != Some(LastCode::Added);
                let to_added = block.last != Some(LastCode::Deleted);
                if to_deleted {
                    if record && block.deleted_meta.len() < limit {
                        block.deleted_meta.push(index);
                    }
                    block.deleted_meta_count += 1;
                }
                if to_added {
                    if record && block.added_meta.len() < limit {
                        block.added_meta.push(index);
                    }
                    block.added_meta_count += 1;
                }
            }
            LineKind::Context => {
                flush(&mut block, &mut out, &mut total, limit, record);
                if record && out.len() < limit {
                    out.push(SplitPair {
                        left: Some(index),
                        right: Some(index),
                    });
                }
                total += 1;
            }
        }
    }
    flush(&mut block, &mut out, &mut total, limit, record);
    (out, total)
}

/// Pairs a hunk's lines into split rows (R20).
pub fn split_pairs(lines: &[DiffLine]) -> Vec<SplitPair> {
    pair_machine(lines, None, true).0
}

/// The first `max_rows` rows of [`split_pairs`], computed without pairing more than can be
/// revealed: the output and the pending runs are capped (R20).
pub fn split_pairs_upto(lines: &[DiffLine], max_rows: usize) -> Vec<SplitPair> {
    pair_machine(lines, Some(max_rows), true).0
}

/// The number of rows [`split_pairs`] would produce, counted without building them.
pub fn split_row_count(lines: &[DiffLine]) -> usize {
    pair_machine(lines, None, false).1
}

/// A row anchor (R24): the file and, for a line row, its side-aware source reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowAnchor {
    /// File index.
    pub file: usize,
    /// The source reference of the first line of a line row.
    pub source: Option<SourceRef>,
}

/// Anchor of row `index`: its file and, for a line row, the source reference of its line (the
/// left line wins in a split row).
pub fn row_anchor(files: &[DiffFile], flat: &Flat, index: usize) -> Option<RowAnchor> {
    let row = *flat.rows.get(index)?;
    let file = row.file()?;
    let source = match row {
        Row::Line {
            file, hunk, line, ..
        } => line_source_ref(&files[file].hunks[hunk].lines[line], true),
        Row::Split {
            file,
            hunk,
            left,
            right,
        } => {
            let lines = &files[file].hunks[hunk].lines;
            left.and_then(|i| line_source_ref(&lines[i], true))
                .or_else(|| right.and_then(|i| line_source_ref(&lines[i], true)))
        }
        _ => None,
    };
    Some(RowAnchor { file, source })
}

/// The row to reveal for an anchor after a re-flatten (R24): the row holding the same source
/// reference, else the first row of the same file.
pub fn find_anchor_row(files: &[DiffFile], flat: &Flat, anchor: RowAnchor) -> Option<usize> {
    if let Some(wanted) = anchor.source {
        for (index, row) in flat.rows.iter().enumerate() {
            let hit = match *row {
                Row::Line {
                    file, hunk, line, ..
                } if file == anchor.file => {
                    line_source_ref(&files[file].hunks[hunk].lines[line], true) == Some(wanted)
                }
                Row::Split {
                    file,
                    hunk,
                    left,
                    right,
                } if file == anchor.file => {
                    let lines = &files[file].hunks[hunk].lines;
                    [left, right]
                        .into_iter()
                        .flatten()
                        .any(|i| line_source_ref(&lines[i], true) == Some(wanted))
                }
                _ => false,
            };
            if hit {
                return Some(index);
            }
        }
    }
    flat.files
        .iter()
        .find(|rows| rows.file == anchor.file)
        .map(|rows| rows.range.start)
}

/// Resolves the sticky file header (R25) from the per-file row ranges: the file containing the
/// first visible row, except when that row is the file's own header and it has not been
/// scrolled past (offset within the row at most 0). Found by binary search; a list position
/// beyond the rows (a briefly stale list) yields none.
pub fn sticky_file_header(files: &[FileRows], first_visible: usize, offset: f32) -> Option<usize> {
    let position = files.partition_point(|rows| rows.range.end <= first_visible);
    let rows = files.get(position)?;
    if !rows.range.contains(&first_visible) {
        return None;
    }
    if first_visible == rows.range.start && offset <= 0. {
        return None;
    }
    Some(rows.file)
}

/// How far a sticky header is pushed up by the next file's header (R25):
/// `min(next_header_top - header height, 0)` with the top measured from the viewport top; no
/// visible next header means no push.
pub fn sticky_push(next_header_top: Option<f32>, header_height: f32) -> f32 {
    next_header_top.map_or(0., |top| (top - header_height).min(0.))
}

/// Maps annotation anchors to annotation indices of one file path.
struct AnnotationIndex<'a> {
    by_anchor: HashMap<(&'a str, Side, u32), Vec<usize>>,
}

impl<'a> AnnotationIndex<'a> {
    fn new(annotations: &'a [Annotation]) -> Self {
        let mut by_anchor: HashMap<(&str, Side, u32), Vec<usize>> = HashMap::new();
        for (index, annotation) in annotations.iter().enumerate() {
            by_anchor
                .entry((annotation.path.as_str(), annotation.side, annotation.line))
                .or_default()
                .push(index);
        }
        Self { by_anchor }
    }

    fn at<'s>(&'s self, path: &'s str, anchor: SourceRef) -> &'s [usize] {
        self.by_anchor
            .get(&(path, anchor.side, anchor.line))
            .map_or(&[], Vec::as_slice)
    }
}

/// Where rows go: straight into the list, or into a fold stand-in accumulator.
struct Out {
    rows: Vec<Row>,
    stand_in: Option<StandInAcc>,
}

struct StandInAcc {
    prefix: Vec<Row>,
    px: f32,
    height: f32,
}

impl Out {
    fn push(&mut self, row: Row, height: f32) {
        match &mut self.stand_in {
            None => self.rows.push(row),
            Some(acc) => {
                acc.height += height;
                if acc.px < STAND_IN_MAX_PX {
                    acc.prefix.push(row);
                    acc.px += height;
                }
            }
        }
    }

    /// Whether rows are still being materialized (always, except past a stand-in's clip).
    fn materializing(&self) -> bool {
        self.stand_in
            .as_ref()
            .is_none_or(|acc| acc.px < STAND_IN_MAX_PX)
    }

    /// Accounts for rows that are never materialized (analytic height only).
    fn skip(&mut self, count: usize, height_each: f32) {
        if let Some(acc) = &mut self.stand_in {
            acc.height += count as f32 * height_each;
        }
    }
}

struct Flattener<'a> {
    files: &'a [DiffFile],
    options: &'a FlattenOptions<'a>,
    index: AnnotationIndex<'a>,
    count: usize,
    out: Out,
}

impl Flattener<'_> {
    fn collapsed(&self, file: usize) -> bool {
        self.options.collapsed.contains(&self.files[file].path)
    }

    fn capped(&self) -> bool {
        self.options.max_lines.is_some_and(|max| self.count >= max)
    }

    fn card_row_height(&self, annotation: usize) -> f32 {
        card_height(&self.options.annotations[annotation].body)
    }

    fn push_line_row(&mut self, row: Row) {
        let height = self.options.metrics.line;
        self.out.push(row, height);
    }

    /// Cards (and the draft) that follow a row whose lines have these anchors.
    fn push_attachments(&mut self, file: usize, anchors: [Option<SourceRef>; 2]) {
        let files = self.files;
        let path = files[file].path.as_str();
        let mut seen: Option<SourceRef> = None;
        for anchor in anchors.into_iter().flatten() {
            if seen == Some(anchor) {
                continue;
            }
            seen = Some(anchor);
            let editing = self
                .options
                .draft
                .filter(|draft| {
                    draft.path == path && draft.side == anchor.side && draft.line == anchor.line
                })
                .map(|draft| draft.editing.as_deref());
            let cards = self.index.at(path, anchor).to_vec();
            for card in cards {
                if editing.flatten() == Some(self.options.annotations[card].id.as_str()) {
                    self.out.push(Row::Draft { file }, DRAFT_HEIGHT);
                    // The draft replaces the card it edits.
                    continue;
                }
                let height = self.card_row_height(card);
                self.out.push(
                    Row::Card {
                        file,
                        annotation: card,
                    },
                    height,
                );
            }
            if let Some(None) = editing {
                self.out.push(Row::Draft { file }, DRAFT_HEIGHT);
            }
        }
    }

    fn has_attachments(&self) -> bool {
        !self.options.annotations.is_empty() || self.options.draft.is_some()
    }

    /// Emits one hunk's code rows for `lines[..take]`. Returns whether the cap cut the hunk.
    fn emit_hunk(&mut self, file: usize, hunk: usize, take: usize) {
        let lines = &self.files[file].hunks[hunk].lines[..take];
        let line_height = self.options.metrics.line;
        match self.options.mode {
            DiffMode::Unified => {
                for (line, entry) in lines.iter().enumerate() {
                    if !self.out.materializing() && !self.has_attachments() {
                        self.out.skip(take - line, line_height);
                        return;
                    }
                    self.push_line_row(Row::Line { file, hunk, line });
                    if self.has_attachments() {
                        let anchor = annotation_anchor(entry);
                        self.push_attachments(file, [anchor, None]);
                    }
                }
            }
            DiffMode::Split => {
                let attachments = self.has_attachments();
                let total = split_row_count(lines);
                let wanted = if self.out.stand_in.is_some() && !attachments {
                    // Pair only what a clip can reveal (R20).
                    if self.out.materializing() {
                        ((STAND_IN_MAX_PX / line_height).ceil() as usize + 1).min(total)
                    } else {
                        0
                    }
                } else {
                    total
                };
                let pairs = if wanted < total {
                    split_pairs_upto(lines, wanted)
                } else {
                    split_pairs(lines)
                };
                for pair in &pairs {
                    self.push_line_row(Row::Split {
                        file,
                        hunk,
                        left: pair.left,
                        right: pair.right,
                    });
                    if attachments {
                        let anchor =
                            |index: Option<usize>| index.and_then(|i| annotation_anchor(&lines[i]));
                        self.push_attachments(file, [anchor(pair.left), anchor(pair.right)]);
                    }
                }
                self.out.skip(total - pairs.len(), line_height);
            }
        }
    }

    /// Emits a file's body. Returns `true` when a show-more row ended the whole list.
    fn emit_body(&mut self, file: usize, remaining_after: &[usize]) -> bool {
        let metrics = self.options.metrics;
        let notices = file_notices(&self.files[file]).len();
        for index in 0..notices {
            self.out.push(Row::Notice { file, index }, metrics.notice);
        }
        let hunks = self.files[file].hunks.len();
        for hunk in 0..hunks {
            let lines = self.files[file].hunks[hunk].lines.len();
            if self.capped() {
                let remaining = self.remaining_from(file, hunk, 0, remaining_after);
                self.out
                    .push(Row::ShowMore { remaining }, metrics.show_more);
                return true;
            }
            self.out
                .push(Row::HunkHeader { file, hunk }, metrics.hunk_header);
            let allowed = self
                .options
                .max_lines
                .map_or(usize::MAX, |max| max - self.count);
            let take = lines.min(allowed);
            self.emit_hunk(file, hunk, take);
            self.count += take;
            if take < lines {
                let remaining = self.remaining_from(file, hunk, take, remaining_after);
                self.out
                    .push(Row::ShowMore { remaining }, metrics.show_more);
                return true;
            }
        }
        self.out.push(Row::BottomPad { file }, metrics.bottom_pad);
        false
    }

    /// Remaining line rows from `(hunk, line)` of `file` to the end, skipping collapsed files
    /// and counting meta lines (R7). `after[f]` is the uncollapsed line total of files `f..`.
    fn remaining_from(&self, file: usize, hunk: usize, line: usize, after: &[usize]) -> usize {
        let hunks = &self.files[file].hunks;
        let own: usize = hunks[hunk].lines.len() - line
            + hunks[hunk + 1..]
                .iter()
                .map(|h| h.lines.len())
                .sum::<usize>();
        own + after[file + 1]
    }
}

/// Flattens `files` into rows (R6, R7, R20, R27).
///
/// For each file in order: a header row; unless the file is collapsed its notice rows, then per
/// hunk a hunk header followed by its line rows, then one bottom-padding row. `max_lines` caps
/// line rows globally (R7). Files in `options.folding` are presented as one stand-in row
/// carrying the first rows a clip can reveal and the analytic body height (R26).
pub fn flatten_rows(files: &[DiffFile], options: &FlattenOptions<'_>) -> Flat {
    let mut after = vec![0usize; files.len() + 1];
    for file in (0..files.len()).rev() {
        let lines = if options.collapsed.contains(&files[file].path) {
            0
        } else {
            files[file].line_count()
        };
        after[file] = after[file + 1] + lines;
    }
    let mut flattener = Flattener {
        files,
        options,
        index: AnnotationIndex::new(options.annotations),
        count: 0,
        out: Out {
            rows: Vec::new(),
            stand_in: None,
        },
    };
    let mut flat = Flat::default();
    for file in 0..files.len() {
        if file > 0 && flattener.capped() {
            if after[file] > 0 {
                flattener.out.rows.push(Row::ShowMore {
                    remaining: after[file],
                });
            }
            break;
        }
        let start = flattener.out.rows.len();
        flattener.out.rows.push(Row::FileHeader { file });
        let folding = options.folding.contains(&file);
        if flattener.collapsed(file) && !folding {
            flat.files.push(FileRows {
                file,
                range: start..flattener.out.rows.len(),
            });
            continue;
        }
        if folding {
            flattener.out.stand_in = Some(StandInAcc {
                prefix: Vec::new(),
                px: 0.,
                height: 0.,
            });
            // A collapsing file counts as expanded for the cap while its stand-in is shown.
        }
        let count_before = flattener.count;
        let stopped = flattener.emit_body(file, &after);
        if folding && flattener.collapsed(file) {
            // A collapsing file's steady state has no line rows: later files keep their place.
            flattener.count = count_before;
        }
        if folding {
            let acc = flattener.out.stand_in.take().expect("stand-in accumulator");
            let index = flat.stand_ins.len();
            flat.stand_ins.push(StandIn {
                file,
                rows: acc.prefix,
                body_height: acc.height,
            });
            flattener.out.rows.push(Row::StandIn { file, index });
        }
        flat.files.push(FileRows {
            file,
            range: start..flattener.out.rows.len(),
        });
        if stopped {
            break;
        }
    }
    flat.rows = flattener.out.rows;
    flat
}

/// The analytic height of a row (R8, R29); `None` for a stand-in row, whose height is the
/// current value of its tween. Wrapped line rows report the line height (their real height is
/// measured by the list, R22).
pub fn row_height(row: &Row, metrics: &RowMetrics, annotations: &[Annotation]) -> Option<f32> {
    Some(match *row {
        Row::FileHeader { .. } => metrics.file_header,
        Row::Notice { .. } => metrics.notice,
        Row::HunkHeader { .. } => metrics.hunk_header,
        Row::Line { .. } | Row::Split { .. } => metrics.line,
        Row::Card { annotation, .. } => card_height(&annotations[annotation].body),
        Row::Draft { .. } => DRAFT_HEIGHT,
        Row::BottomPad { .. } => metrics.bottom_pad,
        Row::ShowMore { .. } => metrics.show_more,
        Row::StandIn { .. } => return None,
    })
}

/// The weighted mean of the actual rows' category heights (R18): the sum of row heights over all
/// rows divided by the row count, the line height when there are no rows. Stand-ins count their
/// body height.
pub fn mean_row_height(flat: &Flat, metrics: &RowMetrics, annotations: &[Annotation]) -> f32 {
    if flat.rows.is_empty() {
        return metrics.line;
    }
    let sum: f32 = flat
        .rows
        .iter()
        .map(|row| match row {
            Row::StandIn { index, .. } => flat.stand_ins[*index].body_height,
            row => row_height(row, metrics, annotations).unwrap_or(0.),
        })
        .sum();
    sum / flat.rows.len() as f32
}

/// The analytic height of a file's expanded body: everything after the header row (R26).
pub fn analytic_body_height(files: &[DiffFile], file: usize, options: &FlattenOptions<'_>) -> f32 {
    let mut collapsed = options.collapsed.clone();
    collapsed.remove(&files[file].path);
    let folding: HashSet<usize> = HashSet::from([file]);
    let single = std::slice::from_ref(&files[file]);
    let local = FlattenOptions {
        metrics: options.metrics,
        max_lines: None,
        collapsed: &collapsed,
        mode: options.mode,
        annotations: options.annotations,
        draft: options.draft,
        folding: &folding,
    };
    // `single` is flattened as file 0, so its annotations are matched by path as usual.
    let flat = flatten_rows(single, &local);
    flat.stand_ins
        .first()
        .map_or(0., |stand_in| stand_in.body_height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::model::parse_patch;

    fn files(patch: &str) -> Vec<DiffFile> {
        parse_patch(patch)
    }

    fn opts<'a>(
        collapsed: &'a HashSet<String>,
        folding: &'a HashSet<usize>,
        max_lines: Option<usize>,
        mode: DiffMode,
    ) -> FlattenOptions<'a> {
        FlattenOptions {
            metrics: RowMetrics::default(),
            max_lines,
            collapsed,
            mode,
            annotations: &[],
            draft: None,
            folding,
        }
    }

    fn flat(files: &[DiffFile], max_lines: Option<usize>, collapsed: &[&str]) -> Flat {
        let collapsed: HashSet<String> = collapsed.iter().map(|p| (*p).to_owned()).collect();
        flatten_rows(
            files,
            &opts(&collapsed, &HashSet::new(), max_lines, DiffMode::Unified),
        )
    }

    const TWO: &str = "diff --git a/a.txt b/a.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+c\n\\ No newline\n@@ -9 +9 @@\n x\ndiff --git a/b.txt b/b.txt\n@@ -1,2 +1,2 @@\n-p\n+q\n";

    fn show_more(flat: &Flat) -> Vec<usize> {
        flat.rows
            .iter()
            .filter_map(|row| match row {
                Row::ShowMore { remaining } => Some(*remaining),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn gutter_width_for_one_to_seven_digits() {
        let expected = [36., 36., 36., 40.4, 47., 53.6, 60.2];
        for (digits, want) in (1..=7u32).zip(expected) {
            let max = 10u32.pow(digits) - 1;
            assert!(
                (gutter_width(max) - want).abs() < 1e-3,
                "{digits} digits: {} vs {want}",
                gutter_width(max)
            );
        }
        assert_eq!(gutter_width(0), 36., "digits of max(max_line, 1)");
        assert!((3. * 6.6 + 14. - 33.8f32).abs() < 1e-4);
        assert!((gutter_width(10_000) - 47.).abs() < 1e-3);
    }

    #[test]
    fn rows_of_a_two_file_patch() {
        let files = files(TWO);
        let flat = flat(&files, None, &[]);
        use Row::*;
        assert_eq!(
            flat.rows,
            vec![
                FileHeader { file: 0 },
                HunkHeader { file: 0, hunk: 0 },
                Line {
                    file: 0,
                    hunk: 0,
                    line: 0
                },
                Line {
                    file: 0,
                    hunk: 0,
                    line: 1
                },
                Line {
                    file: 0,
                    hunk: 0,
                    line: 2
                },
                Line {
                    file: 0,
                    hunk: 0,
                    line: 3
                },
                HunkHeader { file: 0, hunk: 1 },
                Line {
                    file: 0,
                    hunk: 1,
                    line: 0
                },
                BottomPad { file: 0 },
                FileHeader { file: 1 },
                HunkHeader { file: 1, hunk: 0 },
                Line {
                    file: 1,
                    hunk: 0,
                    line: 0
                },
                Line {
                    file: 1,
                    hunk: 0,
                    line: 1
                },
                BottomPad { file: 1 },
            ]
        );
        assert_eq!(flat.files[0].range, 0..9);
        assert_eq!(flat.files[1].range, 9..14);
    }

    #[test]
    fn collapsed_files_contribute_only_their_header() {
        let files = files(TWO);
        let flat = flat(&files, None, &["a.txt"]);
        assert_eq!(flat.rows.len(), 1 + 5);
        assert_eq!(flat.rows[0], Row::FileHeader { file: 0 });
        assert_eq!(flat.rows[1], Row::FileHeader { file: 1 });
        let both = self::flat(&files, None, &["a.txt", "b.txt"]);
        assert_eq!(both.rows.len(), 2);
    }

    #[test]
    fn notices_come_after_the_header() {
        let files = files("diff --git a/a b/b\nrename from a\nrename to b\nnew mode 100755\n");
        let flat = flat(&files, None, &[]);
        use Row::*;
        assert_eq!(
            flat.rows,
            vec![
                FileHeader { file: 0 },
                Notice { file: 0, index: 0 },
                Notice { file: 0, index: 1 },
                BottomPad { file: 0 },
            ]
        );
    }

    #[test]
    fn max_lines_caps_globally_and_counts_meta_lines() {
        let files = files(TWO);
        // File a has 5 line rows (4 + 1 across two hunks, meta included), file b has 2.
        let all = flat(&files, None, &[]);
        assert!(show_more(&all).is_empty());
        let capped = flat(&files, Some(3), &[]);
        // Cut before line 3 (the meta line): remaining = 1 + 1 (second hunk) + 2 (file b) = 4.
        assert_eq!(show_more(&capped), vec![4]);
        assert_eq!(capped.rows.last(), Some(&Row::ShowMore { remaining: 4 }));
        assert!(
            !capped
                .rows
                .iter()
                .any(|r| matches!(r, Row::FileHeader { file: 1 })),
            "no header after the cap"
        );
        // Cap at exactly the end of the first hunk: the next hunk header is replaced.
        let boundary = flat(&files, Some(4), &[]);
        assert_eq!(show_more(&boundary), vec![3]);
        assert_eq!(
            boundary
                .rows
                .iter()
                .filter(|r| matches!(r, Row::HunkHeader { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_file_ending_exactly_at_the_cap_emits_show_more_only_when_more_follows() {
        let files = files(TWO);
        // 5 lines in file a: cap 5 reaches the end of a; file b still has lines.
        let at_end = flat(&files, Some(5), &[]);
        assert_eq!(show_more(&at_end), vec![2]);
        assert_eq!(at_end.rows.last(), Some(&Row::ShowMore { remaining: 2 }));
        assert_eq!(
            at_end.rows[at_end.rows.len() - 2],
            Row::BottomPad { file: 0 },
            "the first file keeps its bottom padding"
        );
        // Nothing follows when the later file is collapsed: no show-more, no header.
        let none_left = flat(&files, Some(5), &["b.txt"]);
        assert!(show_more(&none_left).is_empty());
        assert_eq!(none_left.rows.last(), Some(&Row::BottomPad { file: 0 }));
        // Whole patch exactly at the cap emits nothing extra.
        let exact = flat(&files, Some(7), &[]);
        assert!(show_more(&exact).is_empty());
    }

    #[test]
    fn remaining_count_skips_collapsed_files_and_one_line_wording() {
        let files = files(TWO);
        let capped = flat(&files, Some(5), &["a.txt"]);
        // a is collapsed (header only): count 0 >= cap? no: 0 < 5, so b is emitted fully.
        assert!(show_more(&capped).is_empty());
        let one_left = flat(&files, Some(6), &[]);
        assert_eq!(show_more(&one_left), vec![1]);
        let collapsed_middle = {
            let three = parse_patch(&format!(
                "{TWO}diff --git a/c.txt b/c.txt\n@@ -1 +1 @@\n-z\n+y\n"
            ));
            flat(&three, Some(5), &["b.txt"])
        };
        // After a's 5 lines the cap is reached; b is collapsed so only c's 2 lines remain.
        assert_eq!(show_more(&collapsed_middle), vec![2]);
    }

    #[test]
    fn max_lines_zero_shows_first_header_notices_then_show_more() {
        let files = files(
            "diff --git a/n b/n\nnew file mode 100644\n@@ -0,0 +1 @@\n+x\ndiff --git a/m b/m\n@@ -1 +1 @@\n-y\n",
        );
        let flat = flat(&files, Some(0), &[]);
        use Row::*;
        assert_eq!(
            flat.rows,
            vec![
                FileHeader { file: 0 },
                Notice { file: 0, index: 0 },
                ShowMore { remaining: 2 },
            ]
        );
    }

    #[test]
    fn empty_trailing_hunk_emits_a_zero_count_show_more() {
        let files = files("diff --git a/a b/a\n@@ -1 +1 @@\n-x\n@@ -5 +5 @@\n");
        let flat = flat(&files, Some(1), &[]);
        assert_eq!(show_more(&flat), vec![0]);
    }

    #[test]
    fn row_heights_and_mean_seed() {
        let files = files(TWO);
        let flat = flat(&files, None, &[]);
        let metrics = RowMetrics::default();
        let total: f32 = flat
            .rows
            .iter()
            .map(|r| row_height(r, &metrics, &[]).unwrap())
            .sum();
        assert_eq!(total, 2. * 36. + 3. * 28. + 7. * 21. + 2. * 8.);
        let mean = mean_row_height(&flat, &metrics, &[]);
        assert!((mean - total / 14.).abs() < 1e-4);
        assert_eq!(mean_row_height(&Flat::default(), &metrics, &[]), 21.);
    }

    #[test]
    fn scaled_text_metrics_reproduce_defaults() {
        let (size, height) = scaled_text_metrics(12.5);
        assert!((size - 12.).abs() < 1e-5 && (height - 21.).abs() < 1e-4);
        let (size, height) = scaled_text_metrics(25.);
        assert!((size - 24.).abs() < 1e-5 && (height - 42.).abs() < 1e-4);
        assert_eq!(scaled_text_metrics(1.).0, 8.);
        assert_eq!(scaled_text_metrics(100.).0, 32.);
    }

    // --- split pairing ---

    fn lines(spec: &str) -> Vec<DiffLine> {
        spec.chars()
            .enumerate()
            .map(|(i, c)| DiffLine {
                kind: match c {
                    '-' => LineKind::Deleted,
                    '+' => LineKind::Added,
                    ' ' => LineKind::Context,
                    '\\' => LineKind::Meta,
                    other => panic!("bad spec {other}"),
                },
                text: i.to_string(),
                old: None,
                new: None,
            })
            .collect()
    }

    fn pairs(spec: &str) -> Vec<(Option<usize>, Option<usize>)> {
        split_pairs(&lines(spec))
            .into_iter()
            .map(|p| (p.left, p.right))
            .collect()
    }

    #[test]
    fn three_for_one_rewrite_is_one_paired_row_and_two_one_sided_rows() {
        assert_eq!(
            pairs("---+"),
            vec![(Some(0), Some(3)), (Some(1), None), (Some(2), None)]
        );
        assert_eq!(
            pairs("-+++"),
            vec![(Some(0), Some(1)), (None, Some(2)), (None, Some(3))]
        );
    }

    #[test]
    fn a_deletion_after_additions_starts_a_new_block() {
        // "-a +b -c +d" is two edits.
        assert_eq!(pairs("-+-+"), vec![(Some(0), Some(1)), (Some(2), Some(3))]);
        assert_eq!(pairs("+-"), vec![(None, Some(0)), (Some(1), None)]);
    }

    #[test]
    fn context_is_mirrored_and_flushes() {
        assert_eq!(
            pairs("- +"),
            vec![(Some(0), None), (Some(1), Some(1)), (None, Some(2))]
        );
    }

    #[test]
    fn meta_lines_stay_paired_with_their_edit() {
        // -a \m +b : the marker follows the deletion; it never shares a row with code.
        assert_eq!(pairs("-\\+"), vec![(Some(0), Some(2)), (Some(1), None)]);
        // -a +b \m : the marker follows the addition.
        assert_eq!(pairs("-+\\"), vec![(Some(0), Some(1)), (None, Some(2))]);
        // both markers: -a \m1 +b \m2 pairs the metas on one row.
        assert_eq!(
            pairs("-\\+\\"),
            vec![(Some(0), Some(2)), (Some(1), Some(3))]
        );
        // a marker after context belongs to both sides (the same line).
        assert_eq!(pairs(" \\"), vec![(Some(0), Some(0)), (Some(1), Some(1))]);
        // a deletion after a marker flushes the block first.
        assert_eq!(
            pairs("-\\-"),
            vec![(Some(0), None), (Some(1), None), (Some(2), None)]
        );
    }

    #[test]
    fn the_right_column_never_holds_a_deletion_and_the_left_never_an_addition() {
        let spec = "--+-++ -\\+ ++--\\ -";
        let lines = lines(spec);
        for pair in split_pairs(&lines) {
            if let Some(right) = pair.right {
                assert_ne!(lines[right].kind, LineKind::Deleted);
            }
            if let Some(left) = pair.left {
                assert_ne!(lines[left].kind, LineKind::Added);
            }
        }
    }

    #[test]
    fn bounded_pairing_is_the_prefix_of_the_full_pairing() {
        let specs = [
            "---+",
            "-+-+ -\\+ ++--",
            "- - - + + \\ -+",
            "----------+",
            "+++++++-----",
        ];
        for spec in specs {
            let lines = lines(spec);
            let full = split_pairs(&lines);
            assert_eq!(split_row_count(&lines), full.len(), "{spec}");
            for max in 0..=full.len() + 2 {
                let bounded = split_pairs_upto(&lines, max);
                let expect: Vec<_> = full.iter().take(max).copied().collect();
                assert_eq!(bounded, expect, "{spec} max {max}");
            }
        }
    }

    #[test]
    fn bounded_pairing_of_a_huge_run_does_not_collect_the_run() {
        let mut lines = lines(&"-".repeat(200_000));
        lines.extend(self::lines("+"));
        let bounded = split_pairs_upto(&lines, 10);
        assert_eq!(bounded.len(), 10);
        assert_eq!(
            bounded[0],
            SplitPair {
                left: Some(0),
                right: Some(200_000)
            }
        );
        assert_eq!(bounded[9].right, None);
        assert_eq!(split_row_count(&lines), 200_000);
    }

    #[test]
    fn split_flatten_pairs_lines_and_counts_the_cap_in_diff_lines() {
        let files = files("diff --git a/a b/a\n@@ -1,3 +1,3 @@\n a\n-b\n-c\n+d\n e\n");
        let collapsed = HashSet::new();
        let folding = HashSet::new();
        let flat = flatten_rows(&files, &opts(&collapsed, &folding, None, DiffMode::Split));
        let splits = flat
            .rows
            .iter()
            .filter(|r| matches!(r, Row::Split { .. }))
            .count();
        assert_eq!(splits, 4);
        let capped = flatten_rows(
            &files,
            &opts(&collapsed, &folding, Some(2), DiffMode::Split),
        );
        assert_eq!(capped.rows.last(), Some(&Row::ShowMore { remaining: 3 }));
        assert_eq!(
            capped
                .rows
                .iter()
                .filter(|r| matches!(r, Row::Split { .. }))
                .count(),
            2
        );
    }

    // --- anchors, cards, sticky ---

    #[test]
    fn annotation_anchors_and_card_heights() {
        let patch = files("diff --git a/a b/a\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n\\ x\n");
        let lines = &patch[0].hunks[0].lines;
        assert_eq!(
            annotation_anchor(&lines[1]),
            Some(SourceRef {
                side: Side::Old,
                line: 2
            })
        );
        assert_eq!(
            annotation_anchor(&lines[2]),
            Some(SourceRef {
                side: Side::New,
                line: 2
            })
        );
        assert_eq!(
            annotation_anchor(&lines[0]),
            Some(SourceRef {
                side: Side::New,
                line: 1
            }),
            "context annotates the new side"
        );
        assert_eq!(annotation_anchor(&lines[3]), None);
        assert_eq!(card_height("one line"), 20. + 22. + 18. + 6.);
        assert_eq!(card_height(""), 66.);
        assert_eq!(card_height(&"x".repeat(64)), 66.);
        assert_eq!(card_height(&"x".repeat(65)), 84.);
        assert_eq!(card_height("a\nb\nc"), 20. + 22. + 54. + 6.);
        assert_eq!(card_height(&"x".repeat(64 * 20)), 20. + 22. + 18. * 8. + 6.);
        assert_eq!(card_height(&"a\n".repeat(30)), 20. + 22. + 18. * 8. + 6.);
        assert_eq!(DRAFT_HEIGHT, 116.);
    }

    #[test]
    fn source_references_follow_r16() {
        let patch = files("diff --git a/a b/a\n@@ -3,2 +5,2 @@\n a\n-b\n+c\n\\ x\n");
        let lines = &patch[0].hunks[0].lines;
        assert_eq!(
            line_source_ref(&lines[0], true),
            Some(SourceRef {
                side: Side::New,
                line: 5
            })
        );
        assert_eq!(
            line_source_ref(&lines[0], false),
            Some(SourceRef {
                side: Side::Old,
                line: 3
            })
        );
        assert_eq!(
            line_source_ref(&lines[1], true),
            Some(SourceRef {
                side: Side::Old,
                line: 4
            })
        );
        assert_eq!(line_source_ref(&lines[3], true), None);
    }

    fn annotated(mode: DiffMode, extra: Option<Draft>) -> Flat {
        let files = files("diff --git a/a b/a\n@@ -1,3 +1,3 @@\n a\n-b\n+c\n d\n");
        let annotations = vec![
            Annotation {
                id: "n1".into(),
                path: "a".into(),
                side: Side::Old,
                line: 2,
                body: "old note".into(),
            },
            Annotation {
                id: "n2".into(),
                path: "a".into(),
                side: Side::New,
                line: 2,
                body: "new note".into(),
            },
            Annotation {
                id: "n3".into(),
                path: "a".into(),
                side: Side::New,
                line: 1,
                body: "context note".into(),
            },
        ];
        let collapsed = HashSet::new();
        let folding = HashSet::new();
        let options = FlattenOptions {
            metrics: RowMetrics::default(),
            max_lines: None,
            collapsed: &collapsed,
            mode,
            annotations: &annotations,
            draft: extra.as_ref(),
            folding: &folding,
        };
        flatten_rows(&files, &options)
    }

    fn cards(flat: &Flat) -> Vec<String> {
        flat.rows
            .iter()
            .map(|row| match row {
                Row::Card { annotation, .. } => format!("card{annotation}"),
                Row::Draft { .. } => "draft".into(),
                Row::Line { line, .. } => format!("line{line}"),
                Row::Split { left, right, .. } => format!("split{left:?}{right:?}"),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn cards_follow_the_row_they_anchor_to_in_both_modes() {
        let unified = cards(&annotated(DiffMode::Unified, None));
        assert_eq!(
            unified,
            [
                "line0", "card2", "line1", "card0", "line2", "card1", "line3"
            ]
        );
        // Split: the old-side card staged in unified mode still displays; the left anchor's
        // cards come before the right anchor's.
        let split = cards(&annotated(DiffMode::Split, None));
        assert_eq!(
            split,
            [
                "split Some(0)Some(0)".replace("split ", "split"),
                "card2".into(),
                "splitSome(1)Some(2)".into(),
                "card0".into(),
                "card1".into(),
                "splitSome(3)Some(3)".into(),
            ]
        );
    }

    #[test]
    fn identical_anchors_on_both_sides_appear_once() {
        let flat = annotated(DiffMode::Split, None);
        assert_eq!(
            flat.rows
                .iter()
                .filter(|r| matches!(r, Row::Card { annotation: 2, .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_draft_is_one_row_at_its_anchor_and_replaces_the_card_it_edits() {
        let new_note = Draft {
            path: "a".into(),
            side: Side::New,
            line: 2,
            editing: None,
        };
        let with_draft = cards(&annotated(DiffMode::Unified, Some(new_note)));
        assert_eq!(
            with_draft,
            [
                "line0", "card2", "line1", "card0", "line2", "card1", "draft", "line3"
            ]
        );
        let edit = Draft {
            path: "a".into(),
            side: Side::Old,
            line: 2,
            editing: Some("n1".into()),
        };
        let editing = cards(&annotated(DiffMode::Unified, Some(edit)));
        assert_eq!(
            editing,
            [
                "line0", "card2", "line1", "draft", "line2", "card1", "line3"
            ]
        );
    }

    #[test]
    fn row_anchors_survive_mode_switches() {
        let files = files("diff --git a/a b/a\n@@ -1,3 +1,3 @@\n a\n-b\n+c\n d\n");
        let collapsed = HashSet::new();
        let folding = HashSet::new();
        let unified = flatten_rows(&files, &opts(&collapsed, &folding, None, DiffMode::Unified));
        let split = flatten_rows(&files, &opts(&collapsed, &folding, None, DiffMode::Split));
        // The deleted line row of the unified mode is found in the split row that holds it.
        let deleted_row = 3;
        let anchor = row_anchor(&files, &unified, deleted_row).unwrap();
        assert_eq!(
            anchor.source,
            Some(SourceRef {
                side: Side::Old,
                line: 2
            })
        );
        let target = find_anchor_row(&files, &split, anchor).unwrap();
        assert_eq!(
            split.rows[target],
            Row::Split {
                file: 0,
                hunk: 0,
                left: Some(1),
                right: Some(2)
            }
        );
        // A file-level anchor falls back to the file's first row.
        let header = row_anchor(&files, &unified, 1).unwrap();
        assert_eq!(header.source, None);
        assert_eq!(find_anchor_row(&files, &split, header), Some(0));
        let missing = RowAnchor {
            file: 0,
            source: Some(SourceRef {
                side: Side::New,
                line: 99,
            }),
        };
        assert_eq!(find_anchor_row(&files, &split, missing), Some(0));
    }

    #[test]
    fn sticky_header_resolution() {
        let ranges = vec![
            FileRows {
                file: 0,
                range: 0..9,
            },
            FileRows {
                file: 1,
                range: 9..14,
            },
        ];
        assert_eq!(
            sticky_file_header(&ranges, 0, 0.),
            None,
            "own header, not scrolled"
        );
        assert_eq!(sticky_file_header(&ranges, 0, -3.), None);
        assert_eq!(
            sticky_file_header(&ranges, 0, 4.),
            Some(0),
            "header scrolled partly past"
        );
        assert_eq!(
            sticky_file_header(&ranges, 4, 0.),
            Some(0),
            "inside the file"
        );
        assert_eq!(sticky_file_header(&ranges, 8, 0.), Some(0));
        assert_eq!(sticky_file_header(&ranges, 9, 0.), None);
        assert_eq!(sticky_file_header(&ranges, 10, 2.), Some(1));
        assert_eq!(sticky_file_header(&ranges, 14, 0.), None, "stale position");
        assert_eq!(sticky_file_header(&ranges, 999, 0.), None);
        assert_eq!(sticky_file_header(&[], 0, 5.), None);
    }

    #[test]
    fn sticky_push_never_overlaps_the_next_header() {
        assert_eq!(sticky_push(None, 36.), 0.);
        assert_eq!(sticky_push(Some(100.), 36.), 0.);
        assert_eq!(sticky_push(Some(36.), 36.), 0.);
        assert_eq!(sticky_push(Some(20.), 36.), -16.);
        assert_eq!(sticky_push(Some(-5.), 36.), -41.);
        // The pushed header's bottom edge never passes the next header's top.
        for top in [-10., 0., 10., 35., 36., 80.] {
            let push = sticky_push(Some(top), 36.);
            assert!(36. + push <= top.max(36.).max(0.) + 0.001 || push == 0.);
            assert!(push + 36. <= top || push == 0.);
        }
    }

    #[test]
    fn analytic_body_height_matches_the_flattened_rows() {
        let files = files(TWO);
        let collapsed = HashSet::new();
        let folding = HashSet::new();
        let options = opts(&collapsed, &folding, None, DiffMode::Unified);
        // file a: 2 hunk headers, 5 lines, bottom pad.
        assert_eq!(
            analytic_body_height(&files, 0, &options),
            2. * 28. + 5. * 21. + 8.
        );
        let split = opts(&collapsed, &folding, None, DiffMode::Split);
        // split rows of a: ctx, pair(-b,+c), meta row, ctx x = 4 rows.
        assert_eq!(
            analytic_body_height(&files, 0, &split),
            2. * 28. + 4. * 21. + 8.
        );
    }

    #[test]
    fn stand_in_builds_only_the_revealable_slice() {
        let mut patch = String::from("diff --git a/big b/big\n@@ -1,5000 +1,5000 @@\n");
        for i in 0..5000 {
            patch.push_str(&format!(" line {i}\n"));
        }
        let files = parse_patch(&patch);
        let collapsed = HashSet::new();
        let folding = HashSet::from([0usize]);
        let flat = flatten_rows(&files, &opts(&collapsed, &folding, None, DiffMode::Unified));
        assert_eq!(flat.rows.len(), 2);
        let stand_in = &flat.stand_ins[0];
        assert_eq!(stand_in.body_height, 28. + 5000. * 21. + 8.);
        let px: f32 = stand_in
            .rows
            .iter()
            .map(|r| row_height(r, &RowMetrics::default(), &[]).unwrap())
            .sum();
        assert!(px >= STAND_IN_MAX_PX && px < STAND_IN_MAX_PX + 21.1, "{px}");
        assert!(stand_in.rows.len() < 200);
    }
}
