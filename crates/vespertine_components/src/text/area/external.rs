//! The external progressive text producer transaction (CMP-008 R18, R19).
//!
//! A producer such as speech recognition replaces a selected range with provisional texts and a
//! final text. [`ExternalReplacement`] is the state machine of that transaction over an
//! [`EditCore`]: the first non-empty text records one undo snapshot, later texts replace the
//! previous one unrecorded, and any change the transaction did not make cancels it. The caller
//! passes the core to every method; the transaction owns no editor state.

use crate::text::core::EditCore;
use std::ops::Range;

/// The phase an owner can show for the transaction (R18 g).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalPhase {
    /// No transaction.
    Idle,
    /// Begun, nothing applied yet.
    Starting,
    /// At least one text has been applied.
    Listening,
    /// The final text is being applied.
    Finalizing,
}

/// What an update did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalOutcome {
    /// The text replaced the target.
    Applied,
    /// The text was empty or whitespace-only and changed nothing; a final text still ends the
    /// transaction.
    Ignored,
    /// The guard failed (the document or selection was changed by someone else, or the core is
    /// read-only or composing): the transaction was cancelled and nothing was overwritten.
    Cancelled,
    /// The update carries a generation that is no longer current; it was dropped.
    Stale,
    /// No transaction is active.
    Inactive,
}

/// The state of an active transaction.
#[derive(Clone, Debug)]
struct Active {
    /// The source text captured by `begin`.
    captured: String,
    /// The range the next text replaces: the original selection, then the last inserted text.
    target: Range<usize>,
    /// Whether a text has been applied (the undo snapshot has been recorded).
    applied: bool,
    /// The document text the transaction last produced (or captured).
    produced: String,
    edit_epoch: u64,
    movement_epoch: u64,
}

/// The external replacement transaction.
#[derive(Clone, Debug)]
pub struct ExternalReplacement {
    phase: ExternalPhase,
    generation: u64,
    active: Option<Active>,
    changes: Vec<ExternalPhase>,
}

impl Default for ExternalReplacement {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalReplacement {
    /// An idle transaction at generation 0.
    pub fn new() -> Self {
        Self {
            phase: ExternalPhase::Idle,
            generation: 0,
            active: None,
            changes: Vec::new(),
        }
    }

    /// The current phase.
    pub fn phase(&self) -> ExternalPhase {
        self.phase
    }

    /// The generation producers must echo back. It is incremented by every cancellation (also one
    /// of an idle transaction, such as a document swap) and by every `begin`, so results of an
    /// earlier transaction are always stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a transaction is active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The range the next text will replace, while active.
    pub fn target(&self) -> Option<Range<usize>> {
        self.active.as_ref().map(|active| active.target.clone())
    }

    /// The source text captured when the transaction began, while active.
    pub fn captured_text(&self) -> Option<&str> {
        self.active.as_ref().map(|active| active.captured.as_str())
    }

    /// Takes the phase transitions since the last call, oldest first, so the owner can notify its
    /// state display (R18 g).
    pub fn drain_phase_changes(&mut self) -> Vec<ExternalPhase> {
        std::mem::take(&mut self.changes)
    }

    fn set_phase(&mut self, phase: ExternalPhase) {
        if self.phase != phase {
            self.phase = phase;
            self.changes.push(phase);
        }
    }

    /// Begins a transaction replacing `selection` (already projection-normalized). A previous
    /// transaction is cancelled first. Captures the source text and the target range; the phase
    /// becomes [`ExternalPhase::Starting`]. The core is not touched.
    pub fn begin(&mut self, core: &mut EditCore, selection: Range<usize>) {
        self.cancel();
        let text = core.text();
        let mut start = selection.start.min(selection.end).min(text.len());
        let mut end = selection.start.max(selection.end).min(text.len());
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.generation += 1;
        self.active = Some(Active {
            captured: text.to_owned(),
            target: start..end,
            applied: false,
            produced: text.to_owned(),
            edit_epoch: core.edit_epoch(),
            movement_epoch: core.movement_epoch(),
        });
        self.set_phase(ExternalPhase::Starting);
    }

    /// Replaces the target with a provisional `text`.
    pub fn apply_provisional(&mut self, core: &mut EditCore, text: &str) -> ExternalOutcome {
        self.apply(core, text, false)
    }

    /// Applies the final `text` and ends the transaction. An empty or whitespace-only final text
    /// changes nothing (the latest provisional text stays) but still ends the transaction.
    pub fn finalize(&mut self, core: &mut EditCore, text: &str) -> ExternalOutcome {
        self.apply(core, text, true)
    }

    /// [`apply_provisional`](Self::apply_provisional) for a producer that holds `generation`;
    /// returns [`ExternalOutcome::Stale`] when it is not the current one.
    pub fn apply_provisional_for_generation(
        &mut self,
        generation: u64,
        core: &mut EditCore,
        text: &str,
    ) -> ExternalOutcome {
        if generation != self.generation {
            return ExternalOutcome::Stale;
        }
        self.apply_provisional(core, text)
    }

    /// [`finalize`](Self::finalize) for a producer that holds `generation`; returns
    /// [`ExternalOutcome::Stale`] when it is not the current one.
    pub fn finalize_for_generation(
        &mut self,
        generation: u64,
        core: &mut EditCore,
        text: &str,
    ) -> ExternalOutcome {
        if generation != self.generation {
            return ExternalOutcome::Stale;
        }
        self.finalize(core, text)
    }

    /// Cancels the transaction. The text already inserted stays committed as its own undo step.
    /// Increments the generation, so later results of the cancelled transaction are rejected.
    pub fn cancel(&mut self) {
        self.generation += 1;
        self.active = None;
        self.set_phase(ExternalPhase::Idle);
    }

    /// Cancels the transaction when the core changed in a way the transaction did not make: a
    /// different text, edit epoch or movement epoch (manual edit, input-method edit, paste,
    /// caret movement, undo/redo, `set_value`, document swap), or a composition in progress.
    /// Returns whether it cancelled.
    pub fn sync(&mut self, core: &EditCore) -> bool {
        if self.active.is_some() && !self.guard_holds(core) {
            self.cancel();
            return true;
        }
        false
    }

    fn guard_holds(&self, core: &EditCore) -> bool {
        self.active.as_ref().is_some_and(|active| {
            core.text() == active.produced
                && core.edit_epoch() == active.edit_epoch
                && core.movement_epoch() == active.movement_epoch
                && core.marked().is_none()
        })
    }

    fn apply(&mut self, core: &mut EditCore, text: &str, is_final: bool) -> ExternalOutcome {
        if self.active.is_none() {
            return ExternalOutcome::Inactive;
        }
        if !self.guard_holds(core) || core.is_read_only() {
            self.cancel();
            return ExternalOutcome::Cancelled;
        }
        if text.trim().is_empty() {
            if is_final {
                self.set_phase(ExternalPhase::Finalizing);
                self.end();
            }
            return ExternalOutcome::Ignored;
        }
        if is_final {
            self.set_phase(ExternalPhase::Finalizing);
        }
        let Some(active) = self.active.as_mut() else {
            return ExternalOutcome::Inactive;
        };
        if !active.applied {
            core.set_selection(active.target.clone(), false);
            active.target = core.selection();
            core.record_snapshot();
        }
        let start = active.target.start;
        let inserted_len = core.normalize(text).len();
        core.replace_unrecorded(active.target.clone(), text);
        active.target = start..start + inserted_len;
        active.applied = true;
        active.produced = core.text().to_owned();
        active.edit_epoch = core.edit_epoch();
        active.movement_epoch = core.movement_epoch();
        if is_final {
            self.end();
        } else {
            self.set_phase(ExternalPhase::Listening);
        }
        ExternalOutcome::Applied
    }

    /// Ends a finished transaction without cancelling it (no generation bump).
    fn end(&mut self) {
        self.active = None;
        self.set_phase(ExternalPhase::Idle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "👩🏽‍💻 replace café";

    fn setup() -> (EditCore, Range<usize>) {
        let core = EditCore::new(TEXT, true);
        let start = TEXT.find("replace").unwrap();
        (core, start..start + "replace".len())
    }

    #[test]
    fn provisional_then_final_is_one_undo_step() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection.clone());
        assert_eq!(external.phase(), ExternalPhase::Starting);
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.undo_len(), 0);

        assert_eq!(
            external.apply_provisional(&mut core, "你好"),
            ExternalOutcome::Applied
        );
        assert_eq!(external.phase(), ExternalPhase::Listening);
        assert_eq!(core.text(), "👩🏽‍💻 你好 café");
        assert_eq!(core.undo_len(), 1);
        let end = selection.start + "你好".len();
        assert_eq!(core.selection(), end..end);

        assert_eq!(
            external.apply_provisional(&mut core, "bonjour 🌍"),
            ExternalOutcome::Applied
        );
        assert_eq!(core.text(), "👩🏽‍💻 bonjour 🌍 café");
        assert_eq!(core.undo_len(), 1);
        assert_eq!(
            external.target(),
            Some(selection.start..selection.start + "bonjour 🌍".len())
        );

        assert_eq!(
            external.finalize(&mut core, "salut 🌍"),
            ExternalOutcome::Applied
        );
        assert_eq!(core.text(), "👩🏽‍💻 salut 🌍 café");
        assert_eq!(core.undo_len(), 1);
        assert_eq!(external.phase(), ExternalPhase::Idle);
        assert!(!external.is_active());
        let end = selection.start + "salut 🌍".len();
        assert_eq!(core.selection(), end..end);

        // Undo restores the text and the original selection; redo restores the final text.
        assert!(core.undo());
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.selection(), selection);
        assert_eq!(core.undo_len(), 0);
        assert!(core.redo());
        assert_eq!(core.text(), "👩🏽‍💻 salut 🌍 café");
        assert_eq!(core.undo_len(), 1);
    }

    #[test]
    fn phase_changes_are_reported_in_order() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        external.apply_provisional(&mut core, "a");
        external.apply_provisional(&mut core, "ab");
        external.finalize(&mut core, "abc");
        assert_eq!(
            external.drain_phase_changes(),
            vec![
                ExternalPhase::Starting,
                ExternalPhase::Listening,
                ExternalPhase::Finalizing,
                ExternalPhase::Idle
            ]
        );
        assert!(external.drain_phase_changes().is_empty());
    }

    #[test]
    fn empty_final_keeps_the_latest_provisional_text() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection.clone());
        external.apply_provisional(&mut core, "bonjour");
        assert_eq!(external.finalize(&mut core, ""), ExternalOutcome::Ignored);
        assert_eq!(core.text(), "👩🏽‍💻 bonjour café");
        assert_eq!(core.undo_len(), 1);
        assert_eq!(external.phase(), ExternalPhase::Idle);
        assert!(!external.is_active());
        assert!(core.undo());
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.selection(), selection);
    }

    #[test]
    fn empty_final_without_provisional_changes_nothing() {
        let (mut core, selection) = setup();
        core.set_selection(selection.clone(), false);
        let (edit, movement) = (core.edit_epoch(), core.movement_epoch());
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection.clone());
        assert_eq!(
            external.finalize(&mut core, " \n\t"),
            ExternalOutcome::Ignored
        );
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.selection(), selection);
        assert_eq!(core.undo_len(), 0);
        assert_eq!((core.edit_epoch(), core.movement_epoch()), (edit, movement));
        assert_eq!(external.phase(), ExternalPhase::Idle);
    }

    #[test]
    fn blank_provisional_never_erases_the_selection_or_the_latest_text() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        assert_eq!(
            external.apply_provisional(&mut core, ""),
            ExternalOutcome::Ignored
        );
        assert_eq!(
            external.apply_provisional(&mut core, "  "),
            ExternalOutcome::Ignored
        );
        assert_eq!(core.text(), TEXT);
        assert_eq!(external.phase(), ExternalPhase::Starting);
        external.apply_provisional(&mut core, "hello");
        assert_eq!(
            external.apply_provisional(&mut core, "\n "),
            ExternalOutcome::Ignored
        );
        assert_eq!(core.text(), "👩🏽‍💻 hello café");
        assert_eq!(external.phase(), ExternalPhase::Listening);
        assert!(external.is_active());
        assert_eq!(core.undo_len(), 1);
    }

    #[test]
    fn manual_edit_after_a_partial_cancels_and_a_stale_final_is_ignored() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection.clone());
        let generation = external.generation();
        external.apply_provisional_for_generation(generation, &mut core, "bonjour");
        assert_eq!(core.text(), "👩🏽‍💻 bonjour café");

        // A manual edit: append "!" at the end.
        let end = core.text().len();
        assert!(core.apply(end..end, "!", crate::text::core::EditKind::Other, 0.0));
        assert!(external.sync(&core));
        assert!(!external.is_active());
        assert_eq!(external.phase(), ExternalPhase::Idle);
        assert!(external.generation() > generation);

        assert_eq!(
            external.finalize_for_generation(generation, &mut core, "salut"),
            ExternalOutcome::Stale
        );
        assert_eq!(
            external.apply_provisional_for_generation(generation, &mut core, "salut"),
            ExternalOutcome::Stale
        );
        assert_eq!(
            external.finalize(&mut core, "salut"),
            ExternalOutcome::Inactive
        );
        assert_eq!(core.text(), "👩🏽‍💻 bonjour café!");

        // Undo order: manual edit, provisional text, original.
        assert!(core.undo());
        assert_eq!(core.text(), "👩🏽‍💻 bonjour café");
        assert!(core.undo());
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.selection(), selection);
        assert!(!core.undo());
    }

    #[test]
    fn update_after_a_manual_edit_cancels_instead_of_overwriting() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        external.apply_provisional(&mut core, "bonjour");
        let generation = external.generation();
        core.apply(0..0, "x", crate::text::core::EditKind::Other, 0.0);
        let edited = core.text().to_owned();
        assert_eq!(
            external.apply_provisional(&mut core, "salut"),
            ExternalOutcome::Cancelled
        );
        assert_eq!(core.text(), edited);
        assert!(external.generation() > generation);
        assert_eq!(external.phase(), ExternalPhase::Idle);
    }

    #[test]
    fn set_value_and_draft_swap_reject_late_results() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        let generation = external.generation();
        external.apply_provisional_for_generation(generation, &mut core, "bonjour");
        core.set_value("another draft");
        assert!(external.sync(&core));
        assert_eq!(
            external.finalize_for_generation(generation, &mut core, "salut"),
            ExternalOutcome::Stale
        );
        assert_eq!(core.text(), "another draft");
        assert_eq!(core.undo_len(), 0);

        // A swap to an identical text is still a new document (the epochs moved).
        let mut core = EditCore::new("same", true);
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, 0..4);
        let generation = external.generation();
        core.set_value("same");
        assert!(external.sync(&core));
        assert_eq!(
            external.apply_provisional_for_generation(generation, &mut core, "x"),
            ExternalOutcome::Stale
        );
        assert_eq!(core.text(), "same");
    }

    #[test]
    fn composition_commits_the_provisional_text_with_its_own_undo_step() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection.clone());
        external.apply_provisional(&mut core, "bonjour");
        // The editor cancels the transaction before the composition's own snapshot.
        assert!(!external.sync(&core));
        external.cancel();
        let caret = core.selection().end;
        assert!(core.replace_and_mark(None, "に", None));
        assert_eq!(core.text(), "👩🏽‍💻 bonjourに café");
        assert_eq!(core.selection().end, caret + "に".len());
        assert_eq!(external.finalize(&mut core, "x"), ExternalOutcome::Inactive);

        // Undo order: composition, provisional text, original.
        assert!(core.undo());
        assert_eq!(core.text(), "👩🏽‍💻 bonjour café");
        assert!(core.undo());
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.selection(), selection);
    }

    #[test]
    fn composition_noticed_by_sync_cancels() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        external.apply_provisional(&mut core, "bonjour");
        core.replace_and_mark(None, "に", None);
        assert!(external.sync(&core));
        assert!(!external.is_active());
        assert!(core.undo());
        assert_eq!(core.text(), "👩🏽‍💻 bonjour café");
    }

    #[test]
    fn caret_movement_cancels() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        external.apply_provisional(&mut core, "bonjour");
        assert!(!external.sync(&core));
        core.move_left(false);
        assert!(external.sync(&core));
        assert_eq!(core.text(), "👩🏽‍💻 bonjour café");
    }

    #[test]
    fn movement_before_the_first_text_cancels() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        core.move_to(0, false);
        assert_eq!(
            external.apply_provisional(&mut core, "x"),
            ExternalOutcome::Cancelled
        );
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.undo_len(), 0);
    }

    #[test]
    fn undo_cancels_and_leaves_the_document_alone() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        external.apply_provisional(&mut core, "bonjour");
        assert!(core.undo());
        assert!(external.sync(&core));
        assert_eq!(external.finalize(&mut core, "x"), ExternalOutcome::Inactive);
        assert_eq!(core.text(), TEXT);
    }

    #[test]
    fn begin_cancels_a_previous_transaction() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        external.apply_provisional(&mut core, "bonjour");
        let old = external.generation();
        let caret = core.selection();
        external.begin(&mut core, caret);
        assert!(external.generation() > old);
        assert_eq!(
            external.apply_provisional_for_generation(old, &mut core, "stale"),
            ExternalOutcome::Stale
        );
        assert_eq!(external.phase(), ExternalPhase::Starting);
        external.apply_provisional(&mut core, "!");
        assert_eq!(core.text(), "👩🏽‍💻 bonjour! café");
        assert_eq!(core.undo_len(), 2);
    }

    #[test]
    fn every_cancellation_increments_the_generation() {
        let mut core = EditCore::new("abc", true);
        let mut external = ExternalReplacement::new();
        let start = external.generation();
        external.cancel();
        assert_eq!(external.generation(), start + 1);
        external.begin(&mut core, 0..1);
        external.cancel();
        assert!(external.generation() >= start + 3);
        assert_eq!(external.phase(), ExternalPhase::Idle);
    }

    #[test]
    fn finalize_without_a_provisional_applies_and_ends() {
        let (mut core, selection) = setup();
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection.clone());
        assert_eq!(
            external.finalize(&mut core, "salut 🌍"),
            ExternalOutcome::Applied
        );
        assert_eq!(core.text(), "👩🏽‍💻 salut 🌍 café");
        assert_eq!(core.undo_len(), 1);
        assert!(core.undo());
        assert_eq!(core.selection(), selection);
    }

    #[test]
    fn insertion_at_a_caret_and_unclamped_selection() {
        let mut core = EditCore::new("héllo", true);
        let mut external = ExternalReplacement::new();
        // Inside the two-byte "é" and beyond the end: floored to char boundaries and clamped.
        external.begin(&mut core, 2..99);
        assert_eq!(external.target(), Some(1..6));
        external.apply_provisional(&mut core, "i");
        assert_eq!(core.text(), "hi");
        assert_eq!(external.captured_text(), Some("héllo"));
    }

    #[test]
    fn read_only_core_cancels() {
        let (mut core, selection) = setup();
        core.set_read_only(true);
        let mut external = ExternalReplacement::new();
        external.begin(&mut core, selection);
        assert_eq!(
            external.apply_provisional(&mut core, "x"),
            ExternalOutcome::Cancelled
        );
        assert_eq!(core.text(), TEXT);
        assert_eq!(core.undo_len(), 0);
    }

    #[test]
    fn inactive_updates_report_inactive() {
        let (mut core, _) = setup();
        let mut external = ExternalReplacement::new();
        let generation = external.generation();
        assert_eq!(
            external.apply_provisional(&mut core, "x"),
            ExternalOutcome::Inactive
        );
        assert_eq!(external.finalize(&mut core, "x"), ExternalOutcome::Inactive);
        assert_eq!(
            external.apply_provisional_for_generation(generation, &mut core, "x"),
            ExternalOutcome::Inactive
        );
        assert!(!external.sync(&core));
        assert_eq!(core.text(), TEXT);
    }
}
