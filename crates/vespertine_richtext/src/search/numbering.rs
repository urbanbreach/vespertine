//! Paint-time match numbering (TXT-009 R7, R8): one sequence per declaration shared by retained
//! text and by strings that native widgets generate while painting.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use super::matcher::{GroupMatcher, Query};
use super::resolve::{MatchKind, ResolvedMatch};
use super::spec::HighlightSpec;

/// Cached search results for native-generated text kept this many queries per string.
const NATIVE_QUERIES_PER_TEXT: usize = 4;

#[derive(Default)]
struct Sequence {
    /// The frame id given to [`Numbering::begin_frame_if_new`], if that entry point was used.
    frame_id: Option<u64>,
    /// Next paint-order ordinal per specification index.
    counters: Vec<usize>,
    /// Ordinal assigned in this frame to `(spec_index, run_key, match_index_in_run)`.
    assigned: HashMap<(usize, u64, usize), usize>,
}

struct NativeEntry {
    matcher: GroupMatcher,
    results: Vec<(Query, Vec<Range<usize>>)>,
    /// Frame of the last use; entries unused for a whole frame are dropped.
    used: u64,
}

#[derive(Default)]
struct NativeCache {
    frame: u64,
    entries: HashMap<String, NativeEntry>,
}

/// The numbering of one declaration. Share it (`Rc<Numbering>`) between every view or context
/// created for the declaration so that they draw from one sequence (R8). All methods take
/// `&self`.
#[derive(Default)]
pub struct Numbering {
    sequence: RefCell<Sequence>,
    native: RefCell<NativeCache>,
}

impl Numbering {
    pub fn new() -> Self {
        Self::default()
    }

    /// A shared handle, ready to be given to several contexts.
    pub fn shared() -> Rc<Self> {
        Rc::new(Self::new())
    }

    /// Restarts the sequence at the beginning of a frame. Native-text results unused during the
    /// previous frame are released.
    pub fn begin_frame(&self) {
        {
            let mut sequence = self.sequence.borrow_mut();
            sequence.counters.clear();
            sequence.assigned.clear();
        }
        let mut native = self.native.borrow_mut();
        native.frame += 1;
        let frame = native.frame;
        native.entries.retain(|_, entry| entry.used + 1 >= frame);
    }

    /// [`begin_frame`](Self::begin_frame) for contexts that share this numbering: only the first
    /// caller with a given `frame_id` restarts the sequence. Returns whether it restarted.
    pub fn begin_frame_if_new(&self, frame_id: u64) -> bool {
        if self.sequence.borrow().frame_id == Some(frame_id) {
            return false;
        }
        self.begin_frame();
        self.sequence.borrow_mut().frame_id = Some(frame_id);
        true
    }

    /// The paint-order ordinal of a query match. The first time `(spec_index, run_key,
    /// match_index_in_run)` is seen in a frame it receives the next number of the
    /// specification's sequence; later calls (the same run painting again) return that number.
    ///
    /// `run_key` identifies the painted unit and `match_index_in_run` the match inside it; every
    /// segment of one match must be asked with the same pair so that a match crossing pieces
    /// counts once: use any key shared by the whole [`Resolution`](super::Resolution) with the
    /// match's index in `Resolution::matches` for retained text, and a key of the native run
    /// with the index into [`match_native_text`](Self::match_native_text)'s result for generated
    /// text.
    pub fn ordinal_for(&self, spec_index: usize, run_key: u64, match_index_in_run: usize) -> usize {
        let mut sequence = self.sequence.borrow_mut();
        let sequence = &mut *sequence;
        if let Some(&ordinal) = sequence
            .assigned
            .get(&(spec_index, run_key, match_index_in_run))
        {
            return ordinal;
        }
        if sequence.counters.len() <= spec_index {
            sequence.counters.resize(spec_index + 1, 0);
        }
        let ordinal = sequence.counters[spec_index];
        sequence.counters[spec_index] += 1;
        sequence
            .assigned
            .insert((spec_index, run_key, match_index_in_run), ordinal);
        ordinal
    }

    /// The ordinal of a retained match (index `match_index` of its resolution, identified by
    /// `resolution_key`): paint order for query matches, the resolution's static ordinal (query
    /// total plus supplied order) for explicit-range matches.
    pub fn retained_ordinal(
        &self,
        resolution_key: u64,
        match_index: usize,
        resolved: &ResolvedMatch,
    ) -> usize {
        match resolved.kind {
            MatchKind::Query => self.ordinal_for(resolved.spec_index, resolution_key, match_index),
            MatchKind::Range => resolved.ordinal,
        }
    }

    /// The displayed number of ordinal `ordinal`: `matchIndexOffset + ordinal`.
    pub fn display_number(spec: &HighlightSpec, ordinal: usize) -> usize {
        spec.display_number(ordinal)
    }

    /// Whether the match with displayed number `display_number` is active: it equals
    /// `activeIndex`. When no painted match has that number nothing is active.
    pub fn is_active(spec: &HighlightSpec, display_number: usize) -> bool {
        spec.active_index == Some(display_number)
    }

    /// Matches `spec.query` in a string generated while a native widget paints (explicit ranges
    /// do not apply: they index the retained document). Byte ranges of `text`, in order. Results
    /// are cached per string and query while the string keeps being painted.
    pub fn match_native_text(&self, spec: &HighlightSpec, text: &str) -> Vec<Range<usize>> {
        if spec.query.is_empty() || text.is_empty() {
            return Vec::new();
        }
        let query = Query::new(&spec.query, spec.case_sensitive, spec.whole_word);
        let mut native = self.native.borrow_mut();
        let frame = native.frame;
        let entry = native
            .entries
            .entry(text.to_owned())
            .or_insert_with_key(|key| NativeEntry {
                matcher: GroupMatcher::new(key.clone()),
                results: Vec::new(),
                used: frame,
            });
        entry.used = frame;
        if let Some((_, found)) = entry.results.iter().find(|(cached, _)| *cached == query) {
            return found.clone();
        }
        let found = entry.matcher.find(&query);
        if entry.results.len() >= NATIVE_QUERIES_PER_TEXT {
            entry.results.remove(0);
        }
        entry.results.push((query, found.clone()));
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::matcher::scan_count;
    use crate::search::resolve::{GroupInput, Resolution, Resolver};
    use crate::search::spec::HighlightDeclaration;

    fn group(id: u64, text: &str) -> GroupInput {
        GroupInput::new(id, [text])
    }

    #[test]
    fn numbers_follow_paint_order_and_repeat_for_repainted_runs() {
        let numbering = Numbering::new();
        assert_eq!(numbering.ordinal_for(0, 7, 0), 0);
        assert_eq!(numbering.ordinal_for(0, 8, 0), 1);
        assert_eq!(numbering.ordinal_for(0, 8, 1), 2);
        // The same run painting twice keeps its numbers.
        assert_eq!(numbering.ordinal_for(0, 7, 0), 0);
        assert_eq!(numbering.ordinal_for(0, 8, 1), 2);
        // Specifications have separate sequences.
        assert_eq!(numbering.ordinal_for(1, 7, 0), 0);
        // The next frame restarts.
        numbering.begin_frame();
        assert_eq!(numbering.ordinal_for(0, 8, 1), 0);
        assert_eq!(numbering.ordinal_for(0, 7, 0), 1);
    }

    #[test]
    fn shared_contexts_draw_from_one_sequence() {
        let first = Numbering::shared();
        let second = Rc::clone(&first);
        assert!(first.begin_frame_if_new(1));
        assert_eq!(first.ordinal_for(0, 1, 0), 0);
        assert!(
            !second.begin_frame_if_new(1),
            "second view does not restart"
        );
        assert_eq!(second.ordinal_for(0, 2, 0), 1);
        assert_eq!(first.ordinal_for(0, 3, 0), 2);
        assert!(second.begin_frame_if_new(2));
        assert_eq!(first.ordinal_for(0, 3, 0), 0);
    }

    #[test]
    fn native_and_retained_text_interleave_in_paint_order() {
        let spec = HighlightSpec::new("a")
            .with_match_index_offset(10)
            .with_active_index(Some(11));
        let declaration = HighlightDeclaration::from_spec(spec.clone());
        let groups = [group(1, "a"), group(3, "a")];
        let resolution = Resolver::new().resolve(&declaration, &groups);
        assert_eq!(resolution.total, 2);
        let numbering = Numbering::new();
        let key = 42;

        let mut painted = Vec::new();
        let paint_retained = |group_id: u64, painted: &mut Vec<(String, usize)>| {
            for (match_index, _) in resolution.segments_for(group_id, 0) {
                let ordinal =
                    numbering.retained_ordinal(key, match_index, &resolution.matches[match_index]);
                painted.push((format!("retained{group_id}"), ordinal));
            }
        };
        let paint_native = |run: u64, text: &str, painted: &mut Vec<(String, usize)>| {
            for (index, _) in numbering.match_native_text(&spec, text).iter().enumerate() {
                painted.push((
                    format!("native{run}"),
                    numbering.ordinal_for(0, 1000 + run, index),
                ));
            }
        };

        paint_retained(1, &mut painted);
        paint_native(1, "xAx", &mut painted);
        paint_retained(3, &mut painted);
        paint_native(2, "aa", &mut painted);
        // Rows painted twice keep their numbers.
        paint_retained(1, &mut painted);
        paint_native(1, "xAx", &mut painted);

        let numbers: Vec<_> = painted
            .iter()
            .map(|(name, n)| (name.as_str(), *n))
            .collect();
        assert_eq!(
            numbers,
            vec![
                ("retained1", 0),
                ("native1", 1),
                ("retained3", 2),
                ("native2", 3),
                ("native2", 4),
                ("retained1", 0),
                ("native1", 1),
            ]
        );
        // Displayed numbers are offset by 10; activeIndex 11 marks exactly the native "xAx" match.
        let active: Vec<_> = painted
            .iter()
            .filter(|(_, n)| Numbering::is_active(&spec, Numbering::display_number(&spec, *n)))
            .collect();
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|(name, n)| name == "native1" && *n == 1));
    }

    #[test]
    fn a_match_crossing_pieces_is_numbered_once() {
        let groups = [GroupInput::new(1, ["Hello ", "Tom", "!"])];
        let declaration = HighlightDeclaration::from_spec(HighlightSpec::new("lo Tom"));
        let resolution = Resolver::new().resolve(&declaration, &groups);
        let numbering = Numbering::new();
        let mut ordinals = Vec::new();
        for piece in 0..3 {
            for (match_index, _) in resolution.segments_for(1, piece) {
                ordinals.push(numbering.retained_ordinal(
                    9,
                    match_index,
                    &resolution.matches[match_index],
                ));
            }
        }
        assert_eq!(ordinals, vec![0, 0]);
    }

    #[test]
    fn range_matches_keep_their_static_ordinal_after_the_query_matches() {
        // Document "a b a\nzzz": query "a" has two matches, the range covers "zzz".
        let groups = [group(1, "a b a"), group(2, "zzz")];
        let spec = HighlightSpec::new("a").with_ranges([6..9]);
        let resolution = Resolver::new().resolve(&HighlightDeclaration::from_spec(spec), &groups);
        let numbering = Numbering::new();
        // The range group paints first, then the query matches in reverse of document order.
        let range_ordinal = numbering.retained_ordinal(5, 2, &resolution.matches[2]);
        let second = numbering.retained_ordinal(5, 1, &resolution.matches[1]);
        let first = numbering.retained_ordinal(5, 0, &resolution.matches[0]);
        assert_eq!((range_ordinal, second, first), (2, 0, 1));
    }

    #[test]
    fn a_cursor_outside_the_mounted_window_marks_nothing() {
        // Mounted window holds matches 10 and 11 (offset 10); the cursor is at 3 or 30.
        for active in [3, 30] {
            let spec = HighlightSpec::new("a")
                .with_match_index_offset(10)
                .with_active_index(Some(active));
            assert!(
                (0..2).all(|n| !Numbering::is_active(&spec, Numbering::display_number(&spec, n)))
            );
        }
        let spec = HighlightSpec::new("a").with_active_index(None);
        assert!(!Numbering::is_active(&spec, 0));
    }

    #[test]
    fn native_text_matching_ignores_ranges_and_reuses_its_cache() {
        let spec = HighlightSpec::new("red").with_ranges([0..2]);
        let numbering = Numbering::new();
        let before = scan_count();
        let found = numbering.match_native_text(&spec, "Red and red");
        assert_eq!(found, vec![0..3, 8..11]);
        assert_eq!(numbering.match_native_text(&spec, "Red and red"), found);
        assert_eq!(scan_count() - before, 1);

        // A different query on the same text is a new scan; the case flag matters.
        let sensitive = spec.clone().with_case_sensitive(true);
        assert_eq!(
            numbering.match_native_text(&sensitive, "Red and red"),
            vec![8..11]
        );
        assert_eq!(scan_count() - before, 2);

        // Still cached the frame after its last use, released after an idle frame.
        numbering.begin_frame();
        assert_eq!(numbering.match_native_text(&spec, "Red and red"), found);
        assert_eq!(scan_count() - before, 2);
        numbering.begin_frame();
        numbering.begin_frame();
        assert_eq!(numbering.match_native_text(&spec, "Red and red"), found);
        assert_eq!(scan_count() - before, 3);

        // Ranges alone never match generated text.
        let ranges_only = HighlightSpec::from_ranges([0..2]);
        assert!(
            numbering
                .match_native_text(&ranges_only, "Red and red")
                .is_empty()
        );
        assert!(numbering.match_native_text(&spec, "").is_empty());
    }

    #[test]
    fn resolution_identity_is_untouched_by_numbering() {
        let groups = [group(1, "a a")];
        let declaration = HighlightDeclaration::from_spec(HighlightSpec::new("a"));
        let resolution: std::sync::Arc<Resolution> = Resolver::new().resolve(&declaration, &groups);
        let before = resolution.identity;
        let numbering = Numbering::new();
        numbering.retained_ordinal(1, 1, &resolution.matches[1]);
        assert_eq!(resolution.identity, before);
    }
}
