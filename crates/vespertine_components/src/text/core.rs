//! The editing core shared by the single-line field (CMP-007) and the multiline editor
//! (CMP-008): value, selection model, marked range, undo history with typing coalescing,
//! controlled-value synchronization and the native text-input replacement semantics.
//!
//! The core is a plain data model: it owns no layout, focus or clock. Callers pass the current
//! time in milliseconds to every edit so coalescing is deterministic.

use super::boundaries::{
    ceil_char_boundary, ceil_grapheme_boundary, floor_char_boundary, floor_grapheme_boundary,
    is_single_grapheme, line_end, line_range_with_feed, line_start, next_grapheme, prev_grapheme,
    snap_grapheme_boundary, word_at, word_left, word_right,
};
use super::utf16;
use std::{borrow::Cow, collections::VecDeque, ops::Range};

/// Maximum number of undo snapshots (CMP-007 R19).
pub const UNDO_CAPACITY: usize = 200;
/// Idle window of typing-run coalescing in milliseconds (CMP-007 R21).
pub const COALESCE_WINDOW_MS: f64 = 700.0;
/// Capacity of the controlled-value pending queue (CMP-007 R5).
pub const PENDING_CAPACITY: usize = 32;

/// Which visual row a caret at a soft-wrap boundary belongs to (CMP-008 R4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Affinity {
    /// The end of the upper row.
    Upstream,
    /// The start of the lower row.
    #[default]
    Downstream,
}

/// A restorable state taken before an edit (CMP-007 R19).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The text.
    pub text: String,
    /// The selection (UTF-8 bytes).
    pub selection: Range<usize>,
    /// Whether the caret is at the selection start.
    pub reversed: bool,
    /// The caret affinity.
    pub affinity: Affinity,
}

/// The kind of an edit, used for history coalescing (CMP-007 R21).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Typed or input-method text.
    Insert,
    /// Backspace-style deletion.
    DeleteBackward,
    /// Delete-style deletion.
    DeleteForward,
    /// Paste: always its own step.
    Paste,
    /// Cut: always its own step.
    Cut,
    /// Any other edit: always its own step.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunKind {
    Insert,
    DeleteBackward,
    DeleteForward,
}

#[derive(Clone, Copy, Debug)]
struct Run {
    kind: RunKind,
    /// Insertion tail, or the start of the last deletion.
    position: usize,
    last_ms: f64,
}

/// The outcome of [`EditCore::acknowledge_or_apply`] (CMP-007 R5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acknowledgement {
    /// The pushed value matched an emitted pending value; nothing changed.
    Acknowledged,
    /// An external value identical to the current text; nothing changed.
    Unchanged,
    /// An external replacement: text replaced, caret at the end, history cleared.
    Replaced,
}

/// The granularity of a pointer selection gesture (CMP-007 R9, CMP-008 R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Characters: the plain caret and shift extension.
    Char,
    /// Whole UAX #29 word segments.
    Word,
    /// Whole logical lines (the entire value in a single-line field).
    Line,
}

/// The unit and anchor range of a pointer drag that started with [`EditCore::press`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragAnchor {
    /// The unit the drag keeps.
    pub unit: Unit,
    /// The selection the press produced.
    pub range: Range<usize>,
}

/// The editing core.
#[derive(Clone, Debug)]
pub struct EditCore {
    text: String,
    selection: Range<usize>,
    reversed: bool,
    affinity: Affinity,
    marked: Option<Range<usize>>,
    multiline: bool,
    read_only: bool,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    run: Option<Run>,
    pending: VecDeque<String>,
    composition_base: Option<Snapshot>,
    allow_tab_text: bool,
    atomics: Vec<Range<usize>>,
    last_edit: Option<(Range<usize>, usize)>,
    movement_epoch: u64,
    edit_epoch: u64,
}

impl EditCore {
    /// A core holding `value` with the caret collapsed at its end (CMP-007 R1). A single-line
    /// core normalizes line breaks (R3).
    pub fn new(value: &str, multiline: bool) -> Self {
        let mut core = Self {
            text: String::new(),
            selection: 0..0,
            reversed: false,
            affinity: Affinity::Downstream,
            marked: None,
            multiline,
            read_only: false,
            undo: VecDeque::new(),
            redo: Vec::new(),
            run: None,
            pending: VecDeque::new(),
            composition_base: None,
            allow_tab_text: false,
            atomics: Vec::new(),
            last_edit: None,
            movement_epoch: 0,
            edit_epoch: 0,
        };
        core.text = core.normalize(value).into_owned();
        let end = core.text.len();
        core.selection = end..end;
        core
    }

    // ---- state ------------------------------------------------------------------------------

    /// The value.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The selection as a byte range.
    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    /// Whether the caret sits at the selection start.
    pub fn is_reversed(&self) -> bool {
        self.reversed
    }

    /// The caret affinity.
    pub fn affinity(&self) -> Affinity {
        self.affinity
    }

    /// Sets the caret affinity.
    pub fn set_affinity(&mut self, affinity: Affinity) {
        self.affinity = affinity;
    }

    /// The active end of the selection.
    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    /// The fixed end of the selection.
    pub fn anchor(&self) -> usize {
        if self.reversed {
            self.selection.end
        } else {
            self.selection.start
        }
    }

    /// The selected text.
    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.clone()]
    }

    /// The marked (composing) range.
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Whether every mutation is rejected (CMP-007 R2).
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether a typed Tab character is inserted. By default it is not: Tab and Shift+Tab are
    /// never consumed by an editor, so they traverse focus (CMP-007 R8).
    pub fn set_allow_tab_text(&mut self, allow: bool) {
        self.allow_tab_text = allow;
    }

    /// The last text change as (replaced byte range of the old text, inserted byte length),
    /// cleared by this call. Undo, redo and whole-value replacement report the whole text.
    pub fn take_edit(&mut self) -> Option<(Range<usize>, usize)> {
        self.last_edit.take()
    }

    /// Sets the atomic ranges: sorted, non-overlapping source ranges (projected chips) that
    /// carets never sit inside. Carets snap to the nearer end, selections widen, movement and
    /// deletion cross a range in one step (CMP-008 R16).
    pub fn set_atomic_ranges(&mut self, ranges: Vec<Range<usize>>) {
        self.atomics = ranges;
    }

    /// The atomic ranges.
    pub fn atomic_ranges(&self) -> &[Range<usize>] {
        &self.atomics
    }

    /// The atomic range containing `offset` (its start included, its end excluded).
    pub fn atomic_at(&self, offset: usize) -> Option<Range<usize>> {
        self.atomics
            .iter()
            .find(|range| range.start <= offset && offset < range.end)
            .cloned()
    }

    /// A caret offset: snapped to a grapheme boundary and, inside an atomic range, to the nearer
    /// end by midpoint (below 50% to the start).
    pub fn snap_caret(&self, offset: usize) -> usize {
        let offset = snap_grapheme_boundary(&self.text, offset);
        match self
            .atomics
            .iter()
            .find(|range| range.start < offset && offset < range.end)
        {
            Some(range) if (offset - range.start) * 2 < range.end - range.start => range.start,
            Some(range) => range.end,
            None => offset,
        }
    }

    /// An offset moved in a direction: inside an atomic range it goes to the end it moves toward.
    fn snap_directed(&self, offset: usize, toward_end: bool) -> usize {
        match self
            .atomics
            .iter()
            .find(|range| range.start < offset && offset < range.end)
        {
            Some(range) if toward_end => range.end,
            Some(range) => range.start,
            None => offset,
        }
    }

    /// Widens a range so it never cuts an atomic range.
    pub fn expand_atomic(&self, range: Range<usize>) -> Range<usize> {
        self.snap_directed(range.start, false)..self.snap_directed(range.end, true)
    }

    /// Sets the read-only flag.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Counter incremented by every selection or caret change that is not part of an edit.
    pub fn movement_epoch(&self) -> u64 {
        self.movement_epoch
    }

    /// Counter incremented by every change of the text.
    pub fn edit_epoch(&self) -> u64 {
        self.edit_epoch
    }

    /// Number of available undo steps.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Number of available redo steps.
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// The emitted values not yet seen echoed, oldest first.
    pub fn pending(&self) -> impl Iterator<Item = &str> {
        self.pending.iter().map(String::as_str)
    }

    /// The pre-composition snapshot while an input-method composition is in progress.
    pub fn composition_base(&self) -> Option<&Snapshot> {
        self.composition_base.as_ref()
    }

    /// Normalizes `text` for this core: in a single-line core every line break (CRLF, CR or LF;
    /// CRLF counts once) becomes one space (CMP-007 R3); a multiline core keeps it verbatim.
    pub fn normalize<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.multiline || !text.contains(['\r', '\n']) {
            return Cow::Borrowed(text);
        }
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    out.push(' ');
                }
                '\n' => out.push(' '),
                other => out.push(other),
            }
        }
        Cow::Owned(out)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            reversed: self.reversed,
            affinity: self.affinity,
        }
    }

    fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo.len() == UNDO_CAPACITY {
            self.undo.pop_front();
        }
        self.undo.push_back(snapshot);
    }

    fn emit(&mut self) {
        if self.pending.len() == PENDING_CAPACITY {
            self.pending.pop_front();
        }
        self.pending.push_back(self.text.clone());
        self.edit_epoch += 1;
    }

    fn clamp(&self, range: Range<usize>) -> Range<usize> {
        let a = floor_char_boundary(&self.text, range.start);
        let b = floor_char_boundary(&self.text, range.end);
        a.min(b)..a.max(b)
    }

    // ---- selection ------------------------------------------------------------------------------

    /// Sets the selection; both ends are snapped to grapheme boundaries (R6). Breaks the typing
    /// run (R7).
    pub fn set_selection(&mut self, range: Range<usize>, reversed: bool) {
        let start = snap_grapheme_boundary(&self.text, range.start);
        let end = snap_grapheme_boundary(&self.text, range.end);
        let (start, end) = (start.min(end), start.max(end));
        let (start, end) = if start == end {
            let caret = self.snap_caret(start);
            (caret, caret)
        } else {
            (
                self.snap_directed(start, false),
                self.snap_directed(end, true),
            )
        };
        self.selection = start..end;
        self.reversed = reversed && self.selection.start != self.selection.end;
        self.affinity = Affinity::Downstream;
        self.touch_movement();
    }

    fn touch_movement(&mut self) {
        self.run = None;
        self.movement_epoch += 1;
    }

    /// Moves the caret to `offset`, collapsing or (with `extend`) extending the selection from
    /// its anchor; extension past the anchor flips the reversed flag (R7).
    pub fn move_to(&mut self, offset: usize, extend: bool) {
        if extend {
            let anchor = self.anchor();
            let offset = snap_grapheme_boundary(&self.text, offset);
            let offset = self.snap_directed(offset, offset >= anchor);
            self.selection = anchor.min(offset)..anchor.max(offset);
            self.reversed = offset < anchor;
        } else {
            let offset = self.snap_caret(offset);
            self.selection = offset..offset;
            self.reversed = false;
        }
        self.affinity = Affinity::Downstream;
        self.touch_movement();
    }

    /// Left: with a non-empty selection collapses to its start, else moves one grapheme; with
    /// `extend` moves the active end one grapheme (R7).
    pub fn move_left(&mut self, extend: bool) {
        if extend {
            let target = prev_grapheme(&self.text, self.cursor());
            let target = self.snap_directed(target, false);
            self.move_to(target, true);
        } else if !self.selection.is_empty() {
            self.move_to(self.selection.start, false);
        } else {
            let target = prev_grapheme(&self.text, self.cursor());
            let target = self.snap_directed(target, false);
            self.move_to(target, false);
        }
    }

    /// Right: with a non-empty selection collapses to its end, else moves one grapheme.
    pub fn move_right(&mut self, extend: bool) {
        if extend {
            let target = next_grapheme(&self.text, self.cursor());
            let target = self.snap_directed(target, true);
            self.move_to(target, true);
        } else if !self.selection.is_empty() {
            self.move_to(self.selection.end, false);
        } else {
            let target = next_grapheme(&self.text, self.cursor());
            let target = self.snap_directed(target, true);
            self.move_to(target, false);
        }
    }

    /// Word left.
    pub fn move_word_left(&mut self, extend: bool) {
        let target = word_left(&self.text, self.cursor());
        let target = self.snap_directed(target, false);
        self.move_to(target, extend);
    }

    /// Word right.
    pub fn move_word_right(&mut self, extend: bool) {
        let target = word_right(&self.text, self.cursor());
        let target = self.snap_directed(target, true);
        self.move_to(target, extend);
    }

    /// Home: the start of the logical line.
    pub fn move_line_start(&mut self, extend: bool) {
        let target = line_start(&self.text, self.cursor());
        self.move_to(target, extend);
    }

    /// End: the content end of the logical line, before a trailing CR LF pair.
    pub fn move_line_end(&mut self, extend: bool) {
        let target = line_end(&self.text, self.cursor());
        self.move_to(target, extend);
    }

    /// Document start.
    pub fn move_document_start(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    /// Document end.
    pub fn move_document_end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    /// Select all: anchor 0, caret at the end.
    pub fn select_all(&mut self) {
        self.selection = 0..self.text.len();
        self.reversed = false;
        self.affinity = Affinity::Downstream;
        self.touch_movement();
    }

    // ---- pointer gestures -------------------------------------------------------------------------------

    /// A primary press at the hit index with a click count (CMP-007 R9, CMP-008 R8). `triple_all`
    /// makes a third click select the entire value instead of the logical line.
    pub fn press(
        &mut self,
        hit: usize,
        clicks: usize,
        shift: bool,
        triple_all: bool,
    ) -> DragAnchor {
        let raw = hit;
        let hit = snap_grapheme_boundary(&self.text, hit);
        match clicks {
            0 | 1 => {
                if shift {
                    self.move_to(hit, true);
                    DragAnchor {
                        unit: Unit::Char,
                        range: self.anchor()..self.anchor(),
                    }
                } else {
                    self.move_to(hit, false);
                    let caret = self.selection.start;
                    DragAnchor {
                        unit: Unit::Char,
                        range: caret..caret,
                    }
                }
            }
            2 => {
                let range = self
                    .atomic_at(raw)
                    .unwrap_or_else(|| word_at(&self.text, hit));
                self.set_selection(range.clone(), false);
                DragAnchor {
                    unit: Unit::Word,
                    range,
                }
            }
            _ => {
                let range = self.line_unit(hit, triple_all);
                self.set_selection(range.clone(), false);
                DragAnchor {
                    unit: Unit::Line,
                    range,
                }
            }
        }
    }

    fn line_unit(&self, hit: usize, all: bool) -> Range<usize> {
        if all {
            0..self.text.len()
        } else {
            line_range_with_feed(&self.text, hit)
        }
    }

    /// Extends a drag that started with [`EditCore::press`] to the hit index, keeping its unit
    /// (the union of the anchor unit and the unit under the pointer).
    pub fn drag_to(&mut self, anchor: &DragAnchor, hit: usize, triple_all: bool) {
        let raw = hit;
        let hit = snap_grapheme_boundary(&self.text, hit);
        let target = match anchor.unit {
            Unit::Char => {
                let caret = self.snap_directed(hit, hit >= anchor.range.start);
                caret..caret
            }
            Unit::Word => self
                .atomic_at(raw)
                .unwrap_or_else(|| word_at(&self.text, hit)),
            Unit::Line => self.line_unit(hit, triple_all),
        };
        let (selection, reversed) = if target.start < anchor.range.start {
            (target.start..anchor.range.end, true)
        } else {
            (anchor.range.start..target.end.max(anchor.range.end), false)
        };
        let (selection, reversed) = if anchor.unit == Unit::Char {
            if target.start < anchor.range.start {
                (target.start..anchor.range.end, true)
            } else {
                (anchor.range.start..target.start, false)
            }
        } else {
            (selection, reversed)
        };
        self.selection = selection.clone();
        self.reversed = reversed && !selection.is_empty();
        self.affinity = Affinity::Downstream;
        self.touch_movement();
    }

    // ---- editing ------------------------------------------------------------------------------------

    fn classify(&self, range: &Range<usize>, text: &str, kind: EditKind) -> Option<RunKind> {
        match kind {
            EditKind::Insert => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) if range.is_empty() && !ch.is_whitespace() => {
                        Some(RunKind::Insert)
                    }
                    _ => None,
                }
            }
            EditKind::DeleteBackward
                if text.is_empty() && is_single_grapheme(&self.text, range.clone()) =>
            {
                Some(RunKind::DeleteBackward)
            }
            EditKind::DeleteForward
                if text.is_empty() && is_single_grapheme(&self.text, range.clone()) =>
            {
                Some(RunKind::DeleteForward)
            }
            _ => None,
        }
    }

    fn can_merge(&self, run_kind: RunKind, range: &Range<usize>, now_ms: f64) -> bool {
        let Some(run) = self.run else {
            return false;
        };
        run.kind == run_kind
            && now_ms - run.last_ms < COALESCE_WINDOW_MS
            && now_ms >= run.last_ms
            && match run_kind {
                RunKind::Insert => range.start == run.position,
                RunKind::DeleteBackward => range.end == run.position,
                RunKind::DeleteForward => range.start == run.position,
            }
    }

    /// Replaces `range` with `text` as one recorded edit (CMP-007 R19, R21). The caret ends
    /// collapsed after the inserted text. Returns whether the text changed; rejected when
    /// read-only.
    pub fn apply(&mut self, range: Range<usize>, text: &str, kind: EditKind, now_ms: f64) -> bool {
        if self.read_only {
            return false;
        }
        let range = self.expand_atomic(self.clamp(range));
        let text = self.normalize(text).into_owned();
        if range.is_empty() && text.is_empty() {
            return false;
        }
        let run_kind = self.classify(&range, &text, kind);
        let merged = run_kind.is_some_and(|run_kind| self.can_merge(run_kind, &range, now_ms));
        if !merged {
            let snapshot = self.snapshot();
            self.push_undo(snapshot);
        }
        self.redo.clear();
        self.run = run_kind.map(|kind| Run {
            kind,
            position: match kind {
                RunKind::Insert => range.start + text.len(),
                RunKind::DeleteBackward | RunKind::DeleteForward => range.start,
            },
            last_ms: now_ms,
        });
        self.splice(range, &text);
        true
    }

    /// Replaces `range` with `text` without recording history (an in-progress composition or an
    /// external replacement transaction, CMP-008 R18).
    pub fn replace_unrecorded(&mut self, range: Range<usize>, text: &str) -> bool {
        if self.read_only {
            return false;
        }
        let range = self.expand_atomic(self.clamp(range));
        let text = self.normalize(text).into_owned();
        if range.is_empty() && text.is_empty() {
            return false;
        }
        self.run = None;
        self.redo.clear();
        self.splice(range, &text);
        true
    }

    /// Records the current state as an undo step without changing it (the single snapshot of
    /// an edit sequence that is applied unrecorded afterwards). Breaks the typing run.
    pub fn record_snapshot(&mut self) {
        let snapshot = self.snapshot();
        self.push_undo(snapshot);
        self.redo.clear();
        self.run = None;
    }

    fn splice(&mut self, range: Range<usize>, text: &str) {
        self.last_edit = Some((range.clone(), text.len()));
        self.text.replace_range(range.clone(), text);
        let end = ceil_char_boundary(&self.text, range.start + text.len());
        let end = ceil_grapheme_boundary(&self.text, end);
        self.selection = end..end;
        self.reversed = false;
        self.affinity = Affinity::Downstream;
        self.marked = None;
        self.emit();
    }

    /// Backspace: removes the selection, or one grapheme before the caret.
    pub fn delete_backward(&mut self, now_ms: f64) -> bool {
        if !self.selection.is_empty() {
            return self.apply(self.selection.clone(), "", EditKind::Other, now_ms);
        }
        let caret = self.cursor();
        let start = self.snap_directed(prev_grapheme(&self.text, caret), false);
        self.apply(start..caret, "", EditKind::DeleteBackward, now_ms)
    }

    /// Delete: removes the selection, or one grapheme after the caret.
    pub fn delete_forward(&mut self, now_ms: f64) -> bool {
        if !self.selection.is_empty() {
            return self.apply(self.selection.clone(), "", EditKind::Other, now_ms);
        }
        let caret = self.cursor();
        let end = self.snap_directed(next_grapheme(&self.text, caret), true);
        self.apply(caret..end, "", EditKind::DeleteForward, now_ms)
    }

    /// Word-delete backward; with a non-empty selection removes only the selection.
    pub fn delete_word_backward(&mut self, now_ms: f64) -> bool {
        if !self.selection.is_empty() {
            return self.apply(self.selection.clone(), "", EditKind::Other, now_ms);
        }
        let caret = self.cursor();
        self.apply(
            self.snap_directed(word_left(&self.text, caret), false)..caret,
            "",
            EditKind::Other,
            now_ms,
        )
    }

    /// Word-delete forward.
    pub fn delete_word_forward(&mut self, now_ms: f64) -> bool {
        if !self.selection.is_empty() {
            return self.apply(self.selection.clone(), "", EditKind::Other, now_ms);
        }
        let caret = self.cursor();
        self.apply(
            caret..self.snap_directed(word_right(&self.text, caret), true),
            "",
            EditKind::Other,
            now_ms,
        )
    }

    /// Deletes from the caret to the start of the logical line.
    pub fn delete_to_line_start(&mut self, now_ms: f64) -> bool {
        if !self.selection.is_empty() {
            return self.apply(self.selection.clone(), "", EditKind::Other, now_ms);
        }
        let caret = self.cursor();
        self.apply(
            line_start(&self.text, caret)..caret,
            "",
            EditKind::Other,
            now_ms,
        )
    }

    /// Deletes from the caret to the content end of the logical line.
    pub fn delete_to_line_end(&mut self, now_ms: f64) -> bool {
        if !self.selection.is_empty() {
            return self.apply(self.selection.clone(), "", EditKind::Other, now_ms);
        }
        let caret = self.cursor();
        self.apply(
            caret..line_end(&self.text, caret),
            "",
            EditKind::Other,
            now_ms,
        )
    }

    /// Inserts typed text at the selection (the replacement semantics of R18 without a platform
    /// range).
    pub fn insert(&mut self, text: &str, now_ms: f64) -> bool {
        self.replace_text(None, text, now_ms)
    }

    /// Pastes plain text over the selection as its own undo step (R16).
    pub fn paste(&mut self, text: &str, now_ms: f64) -> bool {
        self.apply(self.selection.clone(), text, EditKind::Paste, now_ms)
    }

    /// Removes the selection as its own undo step (the deletion half of a cut, R16).
    pub fn cut_selection(&mut self, now_ms: f64) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.apply(self.selection.clone(), "", EditKind::Cut, now_ms)
    }

    // ---- history ---------------------------------------------------------------------------------------

    fn restore(&mut self, snapshot: Snapshot) {
        self.last_edit = Some((0..self.text.len(), snapshot.text.len()));
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.reversed = snapshot.reversed;
        self.affinity = snapshot.affinity;
        self.marked = None;
        self.composition_base = None;
        self.run = None;
        self.emit();
    }

    /// Undo (R19). A no-op when read-only or when the stack is empty.
    pub fn undo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        let Some(snapshot) = self.undo.pop_back() else {
            return false;
        };
        let current = self.snapshot();
        self.redo.push(current);
        self.restore(snapshot);
        true
    }

    /// Redo (R19).
    pub fn redo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.push_undo(current);
        self.restore(snapshot);
        true
    }

    // ---- whole-value replacement and controlled sync ------------------------------------------------------

    fn reset_document(&mut self, text: String) {
        self.last_edit = Some((0..self.text.len(), text.len()));
        self.text = text;
        let end = self.text.len();
        self.selection = end..end;
        self.reversed = false;
        self.affinity = Affinity::Downstream;
        self.marked = None;
        self.composition_base = None;
        self.undo.clear();
        self.redo.clear();
        self.run = None;
        self.movement_epoch += 1;
        self.edit_epoch += 1;
    }

    /// Replaces the whole value: a new document, not an edit (CMP-007 R4). Clears history and
    /// the marked range, collapses the selection at the end and returns whether the value
    /// differs from the previous one.
    pub fn set_value(&mut self, value: &str) -> bool {
        let value = self.normalize(value).into_owned();
        let changed = value != self.text;
        self.reset_document(value);
        self.pending.clear();
        changed
    }

    /// The controlled-value push of the owner (R5).
    pub fn acknowledge_or_apply(&mut self, value: &str) -> Acknowledgement {
        let value = self.normalize(value).into_owned();
        if let Some(index) = self.pending.iter().rposition(|pending| *pending == value) {
            self.pending.drain(..=index);
            return Acknowledgement::Acknowledged;
        }
        self.pending.clear();
        if value == self.text {
            return Acknowledgement::Unchanged;
        }
        self.reset_document(value);
        Acknowledgement::Replaced
    }

    // ---- native text input (CMP-007 R17, R18, R20) ------------------------------------------------------------------

    /// `text_for_range`: the text of a UTF-16 range and the adjusted range.
    pub fn text_for_range(&self, range16: Range<usize>) -> (String, Range<usize>) {
        let range = utf16::range_from_utf16(&self.text, &range16);
        (
            self.text[range.clone()].to_owned(),
            utf16::range_to_utf16(&self.text, &range),
        )
    }

    /// `selected_text_range`: the UTF-16 selection and the reversed flag.
    pub fn selected_range_utf16(&self) -> (Range<usize>, bool) {
        (
            utf16::range_to_utf16(&self.text, &self.selection),
            self.reversed,
        )
    }

    /// `marked_text_range` in UTF-16.
    pub fn marked_range_utf16(&self) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| utf16::range_to_utf16(&self.text, range))
    }

    /// Converts a UTF-16 range to a UTF-8 range of the value.
    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        utf16::range_from_utf16(&self.text, range)
    }

    /// Converts a UTF-8 range to UTF-16.
    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        utf16::range_to_utf16(&self.text, range)
    }

    /// The range an input-method replacement targets: the explicit platform range, else the
    /// marked range, else the selection (R18).
    fn target_range(&self, explicit16: Option<Range<usize>>) -> Range<usize> {
        let range = match explicit16 {
            Some(range16) => utf16::range_from_utf16(&self.text, &range16),
            None => self
                .marked
                .clone()
                .unwrap_or_else(|| self.selection.clone()),
        };
        let range = if self.composition_base.is_some() {
            range
        } else {
            let start = floor_grapheme_boundary(&self.text, range.start);
            let end = ceil_grapheme_boundary(&self.text, range.end);
            start..end
        };
        self.expand_atomic(range)
    }

    /// `replace_text_in_range`: inserts text, commits any composition (R18, R20).
    pub fn replace_text(&mut self, range16: Option<Range<usize>>, text: &str, now_ms: f64) -> bool {
        if self.read_only {
            return false;
        }
        let range16_is_none = range16.is_none();
        let range = self.target_range(range16);
        if let Some(base) = self.composition_base.take() {
            // The commit: the pre-composition snapshot already covers it (R20).
            let text = self.normalize(text).into_owned();
            self.run = None;
            self.redo.clear();
            let changed = !(range.is_empty() && text.is_empty());
            if changed {
                self.splice(range, &text);
            } else {
                self.marked = None;
            }
            self.drop_noop_composition(Some(base));
            return changed;
        }
        if text == "\t" && range16_is_none && !self.allow_tab_text {
            return false;
        }
        self.marked = None;
        self.apply(range, text, EditKind::Insert, now_ms)
    }

    /// `replace_and_mark_text_in_range`: replaces the target and marks the inserted text; the
    /// first call of a composition records the pre-composition snapshot (R18, R20).
    pub fn replace_and_mark(
        &mut self,
        range16: Option<Range<usize>>,
        text: &str,
        selected16: Option<Range<usize>>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let range = self.target_range(range16);
        let text = self.normalize(text).into_owned();
        if self.composition_base.is_none() {
            if range.is_empty() && text.is_empty() {
                return false;
            }
            let base = self.snapshot();
            self.push_undo(base.clone());
            self.composition_base = Some(base);
        }
        self.run = None;
        self.redo.clear();
        let start = range.start;
        self.last_edit = Some((range.clone(), text.len()));
        self.text.replace_range(range, &text);
        self.marked = (!text.is_empty()).then(|| start..start + text.len());
        let inserted_len16 = utf16::len_utf16(&text);
        let insertion16 = utf16::to_utf16(&self.text, start);
        let selection = match selected16 {
            Some(selected) => {
                let from = insertion16 + selected.start.min(inserted_len16);
                let to = insertion16 + selected.end.min(inserted_len16);
                let a = utf16::from_utf16(&self.text, from);
                let b = utf16::from_utf16(&self.text, to);
                a.min(b)..a.max(b)
            }
            None => {
                let end = start + text.len();
                end..end
            }
        };
        self.reversed = false;
        self.selection = selection;
        self.affinity = Affinity::Downstream;
        self.emit();
        if self.marked.is_none() {
            let base = self.composition_base.take();
            self.drop_noop_composition(base);
        }
        true
    }

    /// `unmark_text`: ends the composition keeping the marked text as typed.
    pub fn unmark(&mut self) {
        self.marked = None;
        self.composition_base = None;
    }

    /// Drops the pre-composition snapshot when the composition ended without changing anything
    /// (a cancelled composition is not an undo step).
    fn drop_noop_composition(&mut self, base: Option<Snapshot>) {
        if let Some(base) = base
            && base.text == self.text
            && self.undo.back() == Some(&base)
        {
            self.undo.pop_back();
        }
    }

    /// Cancels the composition's marked state when the field loses its input (focus loss).
    pub fn cancel_marking(&mut self) {
        self.unmark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(core: &mut EditCore, text: &str, start_ms: f64, step: f64) -> f64 {
        let mut now = start_ms;
        for ch in text.chars() {
            core.insert(&ch.to_string(), now);
            now += step;
        }
        now
    }

    #[test]
    fn initial_caret_is_collapsed_at_the_end() {
        let core = EditCore::new("héllo", false);
        assert_eq!(core.selection(), 6..6);
        assert!(!core.is_reversed());
    }

    #[test]
    fn single_line_normalizes_every_line_break_to_one_space() {
        let mut core = EditCore::new("", false);
        core.insert("a\r\nb\nc\rd", 0.0);
        assert_eq!(core.text(), "a b c d");
        let mut core = EditCore::new("", false);
        core.paste("a\r\nb\nc\rd", 0.0);
        assert_eq!(core.text(), "a b c d");
        core.set_value("a\r\nb\nc\rd");
        assert_eq!(core.text(), "a b c d");
        core.replace_and_mark(None, "x\r\ny", None);
        assert_eq!(core.text(), "a b c dx y");
        let mut multi = EditCore::new("a\r\nb", true);
        assert_eq!(multi.text(), "a\r\nb");
        multi.paste("\r\n", 0.0);
        assert_eq!(multi.text(), "a\r\nb\r\n");
    }

    #[test]
    fn controlled_value_acknowledgement_and_replacement() {
        let mut core = EditCore::new("", false);
        core.insert("a", 0.0);
        core.insert("b", 1.0);
        core.insert("c", 2.0);
        assert_eq!(core.pending().collect::<Vec<_>>(), ["a", "ab", "abc"]);
        assert_eq!(
            core.acknowledge_or_apply("ab"),
            Acknowledgement::Acknowledged
        );
        assert_eq!(core.text(), "abc");
        assert_eq!(core.pending().collect::<Vec<_>>(), ["abc"]);
        assert_eq!(core.undo_len(), 1);
        assert_eq!(core.acknowledge_or_apply("zzz"), Acknowledgement::Replaced);
        assert_eq!(core.text(), "zzz");
        assert_eq!(core.selection(), 3..3);
        assert_eq!(core.undo_len(), 0);
        assert_eq!(core.redo_len(), 0);
        assert_eq!(core.pending().count(), 0);
        assert_eq!(core.acknowledge_or_apply("zzz"), Acknowledgement::Unchanged);
        assert_eq!(core.text(), "zzz");
    }

    #[test]
    fn pending_queue_is_bounded_and_drops_the_oldest() {
        let mut core = EditCore::new("", false);
        for i in 0..40 {
            core.insert(&char::from(b'a' + (i % 26) as u8).to_string(), i as f64);
        }
        assert_eq!(core.pending().count(), PENDING_CAPACITY);
        assert_eq!(core.pending().last().unwrap(), core.text());
    }

    #[test]
    fn set_value_is_a_new_document() {
        let mut core = EditCore::new("one", false);
        core.insert("x", 0.0);
        core.replace_and_mark(None, "ni", None);
        assert!(core.set_value("fresh"));
        assert_eq!(core.selection(), 5..5);
        assert_eq!(core.undo_len(), 0);
        assert!(core.marked().is_none());
        assert!(!core.set_value("fresh"));
        assert_eq!(core.undo_len(), 0);
    }

    #[test]
    fn caret_movement_and_deletion_follow_grapheme_rules() {
        let mut core = EditCore::new("e\u{301}👩\u{200d}💻a", false);
        core.move_to(0, false);
        core.move_right(false);
        assert_eq!(core.cursor(), 3);
        core.move_right(false);
        assert_eq!(core.cursor(), 3 + "👩\u{200d}💻".len());
        core.move_left(false);
        core.move_left(false);
        assert_eq!(core.cursor(), 0);
        core.move_to(3 + "👩\u{200d}💻".len(), false);
        core.delete_backward(0.0);
        assert_eq!(core.text(), "e\u{301}a");
        core.move_to(0, false);
        core.delete_forward(1.0);
        assert_eq!(core.text(), "a");
    }

    #[test]
    fn crlf_in_multiline_text_is_never_split() {
        let mut core = EditCore::new("a\r\nb", true);
        core.move_to(1, false);
        core.move_right(false);
        assert_eq!(core.cursor(), 3);
        core.move_left(false);
        assert_eq!(core.cursor(), 1);
        core.move_to(2, false);
        assert_eq!(core.cursor(), 1);
        core.move_to(1, false);
        core.move_line_end(false);
        assert_eq!(core.cursor(), 1);
        core.move_to(4, false);
        core.delete_backward(0.0);
        assert_eq!(core.text(), "a\r\n");
        core.delete_backward(10_000.0);
        assert_eq!(core.text(), "a");
    }

    #[test]
    fn selection_extension_flips_the_reversed_flag() {
        let mut core = EditCore::new("hello", false);
        core.move_to(2, false);
        core.move_right(true);
        assert_eq!(core.selection(), 2..3);
        assert!(!core.is_reversed());
        core.move_left(true);
        core.move_left(true);
        assert_eq!(core.selection(), 1..2);
        assert!(core.is_reversed());
        core.move_left(false);
        assert_eq!(core.selection(), 1..1);
    }

    #[test]
    fn word_and_line_edge_deletes_with_selection_delete_only_the_selection() {
        let mut core = EditCore::new("foo bar baz", false);
        core.delete_word_backward(0.0);
        assert_eq!(core.text(), "foo bar ");
        core.move_to(0, false);
        core.delete_word_forward(10_000.0);
        assert_eq!(core.text(), " bar ");
        core.move_to(3, false);
        core.delete_to_line_start(20_000.0);
        assert_eq!(core.text(), "r ");
        core.move_to(0, false);
        core.delete_to_line_end(30_000.0);
        assert_eq!(core.text(), "");
        let mut core = EditCore::new("foo bar", false);
        core.set_selection(0..3, false);
        core.delete_word_backward(0.0);
        assert_eq!(core.text(), " bar");
    }

    #[test]
    fn click_counts_select_caret_extension_word_and_line() {
        let mut core = EditCore::new("foo bar", false);
        core.press(1, 1, false, false);
        assert_eq!(core.selection(), 1..1);
        core.press(5, 1, true, false);
        assert_eq!(core.selection(), 1..5);
        core.press(5, 2, false, false);
        assert_eq!(core.selection(), 4..7);
        core.press(5, 3, false, false);
        assert_eq!(core.selection(), 0..7);
        core.press(2, 4, false, false);
        assert_eq!(core.selection(), 0..7);
    }

    #[test]
    fn word_drag_extends_by_whole_words_keeping_the_anchor_word() {
        let mut core = EditCore::new("foo bar baz", false);
        let anchor = core.press(5, 2, false, false);
        assert_eq!(core.selection(), 4..7);
        core.drag_to(&anchor, 9, false);
        assert_eq!(core.selection(), 4..11);
        assert!(!core.is_reversed());
        core.drag_to(&anchor, 1, false);
        assert_eq!(core.selection(), 0..7);
        assert!(core.is_reversed());
    }

    #[test]
    fn multiline_triple_click_selects_the_line_with_its_feed() {
        let mut core = EditCore::new("ab\ncd\nef", true);
        core.press(4, 3, false, false);
        assert_eq!(core.selection(), 3..6);
        core.press(7, 3, false, false);
        assert_eq!(core.selection(), 6..8);
        core.press(4, 3, false, true);
        assert_eq!(core.selection(), 0..8);
    }

    #[test]
    fn typing_within_the_window_is_one_step_and_pauses_or_spaces_split() {
        let mut core = EditCore::new("", false);
        typed(&mut core, "hello", 0.0, 100.0);
        assert_eq!(core.undo_len(), 1);
        let mut core = EditCore::new("", false);
        core.insert("a", 0.0);
        core.insert("b", 699.0);
        core.insert("c", 1398.0);
        assert_eq!(
            core.undo_len(),
            1,
            "the idle window refreshes on every merge"
        );
        core.insert("d", 1398.0 + 700.0);
        assert_eq!(core.undo_len(), 2);
        let mut core = EditCore::new("", false);
        typed(&mut core, "ab cd", 0.0, 10.0);
        assert_eq!(core.undo_len(), 3, "ab | space | cd");
        core.undo();
        assert_eq!(core.text(), "ab ");
        core.undo();
        assert_eq!(core.text(), "ab");
        core.undo();
        assert_eq!(core.text(), "");
        core.redo();
        assert_eq!(core.text(), "ab");
    }

    #[test]
    fn backward_and_forward_delete_runs_coalesce_separately() {
        let mut core = EditCore::new("abcdef", false);
        core.move_to(3, false);
        core.delete_backward(0.0);
        core.delete_backward(10.0);
        assert_eq!(core.text(), "adef");
        assert_eq!(core.undo_len(), 1);
        core.delete_forward(20.0);
        core.delete_forward(30.0);
        assert_eq!(core.text(), "af");
        assert_eq!(core.undo_len(), 2);
        core.undo();
        assert_eq!(core.text(), "adef");
        core.undo();
        assert_eq!(core.text(), "abcdef");
    }

    #[test]
    fn non_contiguous_or_moved_edits_break_the_run() {
        let mut core = EditCore::new("", false);
        core.insert("a", 0.0);
        core.move_to(0, false);
        core.insert("b", 10.0);
        assert_eq!(core.undo_len(), 2);
        core.insert("c", 20.0);
        assert_eq!(core.undo_len(), 2);
        core.move_to(3, false);
        core.insert("d", 30.0);
        assert_eq!(core.undo_len(), 3);
    }

    #[test]
    fn paste_and_cut_are_always_separate_steps() {
        let mut core = EditCore::new("", false);
        core.paste("x", 0.0);
        core.paste("y", 1.0);
        assert_eq!(core.undo_len(), 2);
        core.set_selection(0..1, false);
        core.cut_selection(2.0);
        core.set_selection(0..1, false);
        core.cut_selection(3.0);
        assert_eq!(core.undo_len(), 4);
        assert_eq!(core.text(), "");
    }

    #[test]
    fn history_is_capped_and_redo_is_invalidated_by_a_new_edit() {
        let mut core = EditCore::new("", false);
        for i in 0..210 {
            core.paste("x", i as f64);
        }
        assert_eq!(core.undo_len(), UNDO_CAPACITY);
        let mut undone = 0;
        while core.undo() {
            undone += 1;
        }
        assert_eq!(undone, 200);
        assert_eq!(core.text().len(), 10);
        assert_eq!(core.redo_len(), 200);
        core.paste("z", 1.0);
        assert_eq!(core.redo_len(), 0);
        assert!(!core.redo());
    }

    #[test]
    fn undo_restores_the_reversed_flag() {
        let mut core = EditCore::new("hello", false);
        core.set_selection(1..4, true);
        assert!(core.is_reversed());
        core.paste("X", 0.0);
        assert_eq!(core.text(), "hXo");
        core.undo();
        assert_eq!(core.selection(), 1..4);
        assert!(core.is_reversed());
        core.redo();
        assert_eq!(core.text(), "hXo");
        assert_eq!(core.selection(), 2..2);
    }

    #[test]
    fn read_only_rejects_every_mutation_but_allows_movement() {
        let mut core = EditCore::new("hello", false);
        core.set_read_only(true);
        assert!(!core.insert("x", 0.0));
        assert!(!core.paste("x", 0.0));
        assert!(!core.delete_backward(0.0));
        assert!(!core.replace_and_mark(None, "x", None));
        core.set_selection(0..5, false);
        assert!(!core.cut_selection(0.0));
        assert!(!core.undo());
        assert_eq!(core.text(), "hello");
        assert_eq!(core.undo_len(), 0);
        core.move_to(2, false);
        assert_eq!(core.selection(), 2..2);
        assert_eq!(core.pending().count(), 0);
    }

    #[test]
    fn native_input_uses_utf16_ranges_and_rounds_surrogate_pairs() {
        let mut core = EditCore::new("a🌍b", false);
        assert_eq!(core.selected_range_utf16(), (4..4, false));
        let (text, adjusted) = core.text_for_range(2..3);
        assert_eq!(text, "🌍");
        assert_eq!(adjusted, 1..3);
        core.replace_text(Some(2..4), "X", 0.0);
        assert_eq!(core.text(), "aX");
    }

    #[test]
    fn marking_replaces_the_target_and_selects_the_relative_range() {
        let mut core = EditCore::new("ab", false);
        core.set_selection(1..1, false);
        core.replace_and_mark(None, "にほ", Some(1..2));
        assert_eq!(core.text(), "aにほb");
        assert_eq!(core.marked(), Some(1..7));
        assert_eq!(core.marked_range_utf16(), Some(1..3));
        assert_eq!(core.selection(), 4..7);
        core.replace_and_mark(None, "にほん", None);
        assert_eq!(core.text(), "aにほんb");
        assert_eq!(core.selection(), 10..10);
        core.replace_and_mark(None, "", None);
        assert_eq!(core.marked(), None);
        assert_eq!(core.text(), "ab");
    }

    #[test]
    fn a_composition_is_one_undo_unit_restoring_the_pre_composition_state() {
        let mut core = EditCore::new("xyz", false);
        core.set_selection(1..2, false);
        core.replace_and_mark(None, "に", None);
        core.replace_and_mark(None, "にほ", None);
        core.replace_and_mark(None, "にほん", None);
        core.replace_and_mark(None, "にほんご", None);
        assert_eq!(core.undo_len(), 1);
        core.replace_text(None, "日本語", 0.0);
        assert_eq!(core.text(), "x日本語z");
        assert_eq!(core.marked(), None);
        assert_eq!(core.selection(), 10..10);
        assert_eq!(core.undo_len(), 1);
        core.undo();
        assert_eq!(core.text(), "xyz");
        assert_eq!(core.selection(), 1..2);
        core.redo();
        assert_eq!(core.text(), "x日本語z");
    }

    #[test]
    fn a_cancelled_composition_leaves_no_undo_step() {
        let mut core = EditCore::new("abc", false);
        core.replace_and_mark(None, "に", None);
        core.replace_and_mark(None, "", None);
        assert_eq!(core.text(), "abc");
        assert_eq!(core.undo_len(), 0);
    }

    #[test]
    fn explicit_platform_range_wins_over_marked_and_selection() {
        let mut core = EditCore::new("hello world", false);
        core.set_selection(0..5, false);
        core.replace_text(Some(6..11), "there", 0.0);
        assert_eq!(core.text(), "hello there");
        assert_eq!(core.selection(), 11..11);
    }

    fn chip_core() -> EditCore {
        // "ab" + a 10-byte atomic range + "cd".
        let mut core = EditCore::new("ab0123456789cd", true);
        core.set_atomic_ranges(vec![2..12]);
        core
    }

    #[test]
    fn atomic_carets_snap_to_the_nearer_end_by_midpoint() {
        let mut core = chip_core();
        core.move_to(6, false);
        assert_eq!(core.selection(), 2..2, "40% snaps to the start");
        core.move_to(8, false);
        assert_eq!(core.selection(), 12..12, "60% snaps to the end");
        core.move_to(7, false);
        assert_eq!(core.selection(), 12..12, "the midpoint snaps to the end");
    }

    #[test]
    fn atomic_selections_widen_and_movement_crosses_in_one_step() {
        let mut core = chip_core();
        core.set_selection(0..5, false);
        assert_eq!(core.selection(), 0..12);
        core.set_selection(5..14, false);
        assert_eq!(core.selection(), 2..14);
        core.set_selection(0..2, false);
        assert_eq!(core.selection(), 0..2, "touching an end does not widen");
        core.move_to(2, false);
        core.move_right(false);
        assert_eq!(core.selection(), 12..12);
        core.move_left(false);
        assert_eq!(core.selection(), 2..2);
        core.move_to(12, false);
        core.delete_backward(0.0);
        assert_eq!(
            core.text(),
            "abcd",
            "Backspace after a chip deletes it whole"
        );
        core.undo();
        assert_eq!(core.text(), "ab0123456789cd");
        core.move_to(2, false);
        core.delete_forward(1.0);
        assert_eq!(core.text(), "abcd");
    }

    #[test]
    fn double_click_selects_the_chip_and_ime_ranges_are_normalized() {
        let mut core = chip_core();
        core.press(5, 2, false, false);
        assert_eq!(core.selection(), 2..12);
        core.set_selection(0..0, false);
        core.replace_text(Some(4..6), "X", 0.0);
        assert_eq!(core.text(), "abXcd");
    }
}
