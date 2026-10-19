//! Cross-element document selection (TXT-008 R5-R14): the press/promote/drag state machine, span
//! resolution, group-aware joining and the selection identity.

use crate::display_map::OriginalText;
use crate::registry::{Registry, RunKey, SurfaceId};
use crate::runs::floor_char_boundary;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;
use vespertine::{Pixels, Point};

/// The selected part of one run. It snapshots the run's text at drag time so copy still works
/// after the run leaves the registry (virtualized rows).
#[derive(Clone, Debug)]
pub struct Span {
    /// Surface that owns the run.
    pub surface: SurfaceId,
    /// Run key.
    pub key: RunKey,
    /// Selected byte range of the run's displayed text.
    pub range: Range<usize>,
    /// The run's full text at drag time, with its source mapping.
    pub text: Arc<OriginalText>,
    /// Group identity (first run of a sequence of adjacent text children), if any.
    pub group: Option<RunKey>,
}

impl Span {
    /// Whether the span selects nothing.
    pub fn is_empty(&self) -> bool {
        self.range.start >= self.range.end
    }

    /// The source text the span copies: never the ellipsis, slot placeholders or display-only
    /// break opportunities.
    pub fn copied_text(&self) -> &str {
        self.text.copy(self.range.clone())
    }
}

/// Effects a press or move asks its caller to perform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Blur the focused control.
    pub blur: bool,
    /// Refresh the window (the spans changed).
    pub refresh: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    surface: SurfaceId,
    key: RunKey,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Pending,
    Dragging,
}

/// The scoped selection state.
#[derive(Clone, Debug)]
pub struct Selection {
    phase: Phase,
    anchor: Option<Anchor>,
    spans: Vec<Span>,
    forward: bool,
    last_reported: u64,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            anchor: None,
            spans: Vec::new(),
            forward: true,
            last_reported: 0,
        }
    }
}

/// The word at `index` (R6): the maximal run of alphanumeric characters and underscores that
/// touches `index`; otherwise the single character at `index` if it is not whitespace; otherwise
/// empty. `index` is clamped down to a character boundary.
pub fn word_range(text: &str, index: usize) -> Range<usize> {
    let index = floor_char_boundary(text, index);
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let at = text[index..].chars().next();
    let before = text[..index].chars().next_back();
    if at.is_some_and(is_word) || before.is_some_and(is_word) {
        let start = text[..index]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(index, |(ix, _)| ix);
        let end = text[index..]
            .char_indices()
            .find(|(_, c)| !is_word(*c))
            .map_or(text.len(), |(ix, _)| index + ix);
        return start..end;
    }
    match at {
        Some(c) if !c.is_whitespace() => index..index + c.len_utf8(),
        _ => index..index,
    }
}

impl Selection {
    /// Whether a press is pending or a drag is live.
    pub fn is_pressed(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// Whether a drag is live.
    pub fn is_dragging(&self) -> bool {
        self.phase == Phase::Dragging
    }

    /// The spans, in document order.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Surface that owns the selection.
    pub fn surface(&self) -> Option<SurfaceId> {
        self.spans.first().map(|span| span.surface)
    }

    /// Whether any span selects text.
    pub fn has_text(&self) -> bool {
        self.spans.iter().any(|span| !span.is_empty())
    }

    /// Identity: 0 when inactive or every span is empty, otherwise a hash over each non-empty
    /// span's (run key, range, group, text snapshot) (R13).
    pub fn identity(&self) -> u64 {
        if !self.has_text() {
            return 0;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for span in self.spans.iter().filter(|span| !span.is_empty()) {
            span.surface.hash(&mut hasher);
            span.key.hash(&mut hasher);
            span.range.start.hash(&mut hasher);
            span.range.end.hash(&mut hasher);
            span.group.hash(&mut hasher);
            span.text.displayed().hash(&mut hasher);
            span.text.source().hash(&mut hasher);
        }
        hasher.finish().max(1)
    }

    /// The joined selected text (R10), or `None` when the selection is empty.
    ///
    /// Spans are walked in document order; between two spans a newline is inserted unless both
    /// have the same non-null group. Empty kept spans (blank lines between the endpoints) take
    /// part in the walk and so produce the blank line between their neighbors.
    pub fn selected_text(&self) -> Option<String> {
        if !self.has_text() {
            return None;
        }
        let mut out = String::new();
        let mut previous: Option<&Span> = None;
        for span in &self.spans {
            if let Some(previous) = previous {
                let same_group = previous.group.is_some() && previous.group == span.group;
                if !same_group {
                    out.push('\n');
                }
            }
            out.push_str(span.copied_text());
            previous = Some(span);
        }
        Some(out)
    }

    /// Reports a change of the selection identity: `Some(text)` (or `Some(None)` when the
    /// selection became empty) once per distinct identity, never for the initial empty state.
    pub fn take_change(&mut self) -> Option<Option<String>> {
        let identity = self.identity();
        if identity == self.last_reported {
            return None;
        }
        self.last_reported = identity;
        Some(self.selected_text())
    }

    /// The selected range to wash in the run `key` of `surface`: only on the surface that owns the
    /// selection (R9, R14).
    pub fn wash_range(&self, surface: SurfaceId, key: RunKey) -> Option<Range<usize>> {
        self.spans
            .iter()
            .find(|span| span.surface == surface && span.key == key && !span.is_empty())
            .map(|span| span.range.clone())
    }

    /// Replaces the spans programmatically (select-all, find navigation); no press is pending.
    pub fn set_spans(&mut self, spans: Vec<Span>) {
        self.spans = spans;
        self.anchor = None;
        self.phase = Phase::Idle;
    }

    /// Removes the selection and any pending press. Returns whether spans were removed.
    pub fn clear(&mut self) -> bool {
        let had = !self.spans.is_empty();
        self.spans.clear();
        self.anchor = None;
        self.phase = Phase::Idle;
        had
    }

    /// Handles a left-button press (R5): click 1 arms a pending press and selects nothing; click 2
    /// selects the word, click 3 or more the whole run text.
    pub fn press(
        &mut self,
        registry: &Registry,
        position: Point<Pixels>,
        click_count: usize,
    ) -> Outcome {
        use crate::registry::PressTarget;
        match registry.press_target(position) {
            PressTarget::Blocked | PressTarget::NoRun => {
                let cleared = self.clear();
                Outcome {
                    blur: false,
                    refresh: cleared,
                }
            }
            PressTarget::Run { run, index } => {
                let registered = &registry.runs()[run];
                let had = !self.spans.is_empty();
                self.spans.clear();
                self.anchor = Some(Anchor {
                    surface: registered.surface,
                    key: registered.key,
                    index,
                });
                self.forward = true;
                self.phase = Phase::Pending;
                match click_count {
                    0 | 1 => Outcome {
                        blur: false,
                        refresh: had,
                    },
                    2 => {
                        let range = word_range(registered.text.displayed(), index);
                        self.begin_with_span(registry, run, range);
                        Outcome {
                            blur: true,
                            refresh: true,
                        }
                    }
                    _ => {
                        let range = 0..registered.text.displayed().len();
                        self.begin_with_span(registry, run, range);
                        Outcome {
                            blur: true,
                            refresh: true,
                        }
                    }
                }
            }
        }
    }

    /// Selects `range` of the registered run at `run` (double and triple click). The anchor moves
    /// to the start of the range so a following drag extends from it.
    pub fn begin_with_span(&mut self, registry: &Registry, run: usize, range: Range<usize>) {
        let registered = &registry.runs()[run];
        self.spans = vec![Span {
            surface: registered.surface,
            key: registered.key,
            range: range.clone(),
            text: registered.text.clone(),
            group: registered.group,
        }];
        self.anchor = Some(Anchor {
            surface: registered.surface,
            key: registered.key,
            index: range.start,
        });
        self.forward = true;
        self.phase = Phase::Pending;
    }

    /// Handles a move. The first move reported as dragging promotes a pending press to a live drag
    /// (the focused control is blurred); moves of a live drag update the head.
    pub fn drag_move(
        &mut self,
        registry: &Registry,
        position: Point<Pixels>,
        dragging: bool,
    ) -> Outcome {
        if !dragging {
            return Outcome::default();
        }
        let mut outcome = Outcome::default();
        match self.phase {
            Phase::Idle => return outcome,
            Phase::Pending => {
                self.phase = Phase::Dragging;
                outcome.blur = true;
            }
            Phase::Dragging => {}
        }
        outcome.refresh = self.update_head(registry, position);
        outcome
    }

    /// Re-resolves the spans for a head at `position` (R4, R8). Returns whether the spans changed.
    pub fn update_head(&mut self, registry: &Registry, position: Point<Pixels>) -> bool {
        let Some(anchor) = self.anchor else {
            return false;
        };
        let Some((head_run, head_index)) = registry.drag_head(position, Some(anchor.surface))
        else {
            return false;
        };
        let new_spans = match registry.run_index(anchor.surface, anchor.key) {
            Some(anchor_run) => {
                let anchor_index = clamp_to_run(registry, anchor_run, anchor.index);
                let head_index = clamp_to_run(registry, head_run, head_index);
                self.forward = (anchor_run, anchor_index) <= (head_run, head_index);
                resolve_spans(registry, (anchor_run, anchor_index), (head_run, head_index))
            }
            None => match self.resolve_unmounted(registry, (head_run, head_index)) {
                Some(spans) => spans,
                None => return false,
            },
        };
        let changed = !same_spans(&self.spans, &new_spans);
        self.spans = new_spans;
        changed
    }

    /// R8: the anchor run is no longer painted. The recorded direction preserves the side of the
    /// accumulated spans.
    fn resolve_unmounted(&self, registry: &Registry, head: (usize, usize)) -> Option<Vec<Span>> {
        let surface = self.anchor?.surface;
        // Painted runs that overlap the old selection, in span order.
        let painted: Vec<(usize, usize)> = self
            .spans
            .iter()
            .enumerate()
            .filter_map(|(span_ix, span)| {
                registry
                    .run_index(surface, span.key)
                    .map(|run_ix| (span_ix, run_ix))
            })
            .collect();
        if self.forward {
            let &(span_ix, run_ix) = painted.first()?;
            let start = clamp_to_run(registry, run_ix, self.spans[span_ix].range.start);
            let mut spans = self.spans[..span_ix].to_vec();
            spans.extend(resolve_spans(registry, (run_ix, start), head));
            Some(spans)
        } else {
            let &(span_ix, run_ix) = painted.last()?;
            let end = clamp_to_run(registry, run_ix, self.spans[span_ix].range.end);
            let mut spans = resolve_spans(registry, head, (run_ix, end));
            spans.extend(self.spans[span_ix + 1..].iter().cloned());
            Some(spans)
        }
    }

    /// Handles the release of the button. A pending press that never became a drag is cancelled;
    /// returns `true` only when a live drag ended (the drag-end callback must be invoked only
    /// then: a tap must not trigger it).
    pub fn release(&mut self) -> bool {
        let was_dragging = self.phase == Phase::Dragging;
        self.phase = Phase::Idle;
        self.anchor = None;
        was_dragging
    }
}

fn clamp_to_run(registry: &Registry, run: usize, index: usize) -> usize {
    floor_char_boundary(registry.runs()[run].text.displayed(), index)
}

fn same_spans(a: &[Span], b: &[Span]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.surface == b.surface
                && a.key == b.key
                && a.range == b.range
                && a.group == b.group
                && Arc::ptr_eq(&a.text, &b.text)
        })
}

/// Resolves spans between two endpoints `(run position, byte offset)`, in either direction (R7):
/// partial from the start offset in the first run, partial up to the end offset in the last run,
/// whole text in runs strictly between. Zero-length spans are omitted except that an empty run
/// strictly between the endpoint runs is kept so that its newline survives in the copy. Only runs
/// of the first endpoint's surface take part.
pub fn resolve_spans(registry: &Registry, a: (usize, usize), b: (usize, usize)) -> Vec<Span> {
    let (first, last) = if a <= b { (a, b) } else { (b, a) };
    let surface = registry.runs()[first.0].surface;
    let mut spans = Vec::new();
    for run_ix in first.0..=last.0 {
        let run = &registry.runs()[run_ix];
        if run.surface != surface {
            continue;
        }
        let text = run.text.displayed();
        let start = if run_ix == first.0 {
            floor_char_boundary(text, first.1)
        } else {
            0
        };
        let end = if run_ix == last.0 {
            floor_char_boundary(text, last.1)
        } else {
            text.len()
        };
        let strictly_between = run_ix > first.0 && run_ix < last.0;
        if end <= start && !(strictly_between && text.is_empty()) {
            continue;
        }
        spans.push(Span {
            surface,
            key: run.key,
            range: start..end.max(start),
            text: run.text.clone(),
            group: run.group,
        });
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::testing::run;
    use vespertine::{point, px};

    fn registry(lines: &[&str]) -> Registry {
        let mut registry = Registry::default();
        for (ix, line) in lines.iter().enumerate() {
            registry.register(run(ix as u32, 0, line, ix as f32 * 24.0, None));
        }
        registry
    }

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    #[test]
    fn word_selection_follows_the_touching_rule() {
        assert_eq!(word_range("foo_bar baz", 4), 0..7);
        assert_eq!(
            word_range("foo_bar baz", 7),
            0..7,
            "the character before touches"
        );
        assert_eq!(word_range("foo_bar baz", 8), 8..11);
        assert_eq!(word_range("a . b", 2), 2..3, "a lone punctuation character");
        assert_eq!(word_range("a   b", 2), 2..2, "whitespace selects nothing");
        assert_eq!(word_range("héllo wörld", 2), 0..6);
        assert_eq!(word_range("", 0), 0..0);
        // An offset inside a character is clamped down.
        assert_eq!(word_range("héllo", 2), 0..6);
    }

    #[test]
    fn a_press_alone_selects_nothing_and_does_not_blur() {
        let registry = registry(&["hello world"]);
        let mut selection = Selection::default();
        let outcome = selection.press(&registry, at(15., 10.), 1);
        assert_eq!(outcome, Outcome::default());
        assert!(!selection.has_text());
        assert!(selection.is_pressed() && !selection.is_dragging());
        assert!(
            selection.take_change().is_none(),
            "initial empty state emits nothing"
        );
        // A release cancels the pending press and is not a drag end.
        assert!(!selection.release());
        assert!(!selection.is_pressed());
    }

    #[test]
    fn the_first_dragging_move_promotes_and_blurs() {
        let registry = registry(&["hello world"]);
        let mut selection = Selection::default();
        selection.press(&registry, at(15., 10.), 1);
        // A move without the button held changes nothing.
        assert_eq!(
            selection.drag_move(&registry, at(55., 10.), false),
            Outcome::default()
        );
        let outcome = selection.drag_move(&registry, at(55., 10.), true);
        assert!(outcome.blur && outcome.refresh);
        assert!(selection.is_dragging());
        assert_eq!(selection.selected_text().as_deref(), Some("ello"));
        // Later moves do not blur again.
        let outcome = selection.drag_move(&registry, at(75., 10.), true);
        assert!(!outcome.blur && outcome.refresh);
        assert_eq!(selection.selected_text().as_deref(), Some("ello w"));
        assert!(selection.release(), "a live drag reports its end");
    }

    #[test]
    fn double_and_triple_click_select_word_and_run() {
        let registry = registry(&["foo_bar  baz", "second line"]);
        let mut selection = Selection::default();
        let outcome = selection.press(&registry, at(25., 10.), 2);
        assert!(outcome.blur && outcome.refresh);
        assert_eq!(selection.selected_text().as_deref(), Some("foo_bar"));
        let outcome = selection.press(&registry, at(25., 10.), 3);
        assert!(outcome.blur);
        assert_eq!(selection.selected_text().as_deref(), Some("foo_bar  baz"));
        // Double click on whitespace between two spaces selects nothing.
        let outcome = selection.press(&registry, at(85., 10.), 2);
        assert!(outcome.blur);
        assert!(!selection.has_text());
        assert!(!selection.release(), "a tap must not trigger the drag end");
    }

    #[test]
    fn drags_resolve_partial_whole_partial_in_both_directions() {
        let registry = registry(&["first line", "second line", "third line"]);
        let mut forward = Selection::default();
        forward.press(&registry, at(35., 10.), 1);
        forward.drag_move(&registry, at(45., 58.), true);
        assert_eq!(
            forward.selected_text().as_deref(),
            Some("st line\nsecond line\nthir")
        );
        let mut reverse = Selection::default();
        reverse.press(&registry, at(45., 58.), 1);
        reverse.drag_move(&registry, at(35., 10.), true);
        assert_eq!(reverse.selected_text(), forward.selected_text());
        // Contracting back to the anchor run narrows the selection again.
        reverse.drag_move(&registry, at(15., 50.), true);
        assert_eq!(reverse.selected_text().as_deref(), Some("hir"));
    }

    #[test]
    fn blank_lines_between_endpoints_survive_the_copy() {
        let registry = registry(&["alpha", "", "gamma"]);
        let mut selection = Selection::default();
        selection.press(&registry, at(5., 10.), 1);
        selection.drag_move(&registry, at(25., 58.), true);
        assert_eq!(selection.selected_text().as_deref(), Some("alpha\n\nga"));
        assert_eq!(selection.spans().len(), 3);
        assert!(selection.spans()[1].is_empty());
    }

    #[test]
    fn adjacent_pieces_of_one_group_copy_as_one_line() {
        let group = Some(RunKey::new("host", 0));
        let mut registry = Registry::default();
        registry.register(run(0, 0, "Hello ", 0.0, group));
        let mut second = run(1, 0, "world", 0.0, group);
        second.geometry = crate::registry::testing::ModelRun::new(60.0, 0.0, 5);
        registry.register(second);
        registry.register(run(2, 0, "next paragraph", 24.0, None));
        let mut selection = Selection::default();
        selection.press(&registry, at(0., 10.), 1);
        selection.drag_move(&registry, at(80., 34.), true);
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("Hello world\nnext par"),
            "same group joins without newline, other groups with one"
        );
    }

    #[test]
    fn spans_and_surfaces_never_leak() {
        let mut registry = Registry::default();
        registry.register(run(0, 1, "main surface", 0.0, None));
        registry.register(run(0, 2, "modal surface", 24.0, None));
        let mut selection = Selection::default();
        selection.press(&registry, at(5., 10.), 1);
        selection.drag_move(&registry, at(55., 34.), true);
        // Only the surface of the press takes part, so the modal run is not selected.
        assert_eq!(selection.selected_text().as_deref(), Some("main "));
        let key = RunKey::new("test", 0);
        assert!(selection.wash_range(SurfaceId(1), key).is_some());
        assert!(
            selection.wash_range(SurfaceId(2), key).is_none(),
            "equal keys never share a wash"
        );
    }

    #[test]
    fn change_notifications_are_deduplicated() {
        let registry = registry(&["hello world"]);
        let mut selection = Selection::default();
        assert!(selection.take_change().is_none());
        selection.press(&registry, at(15., 10.), 2);
        assert_eq!(selection.take_change(), Some(Some("hello".to_string())));
        assert!(
            selection.take_change().is_none(),
            "repeated frames emit nothing"
        );
        assert!(selection.clear());
        assert_eq!(
            selection.take_change(),
            Some(None),
            "clearing reports absent"
        );
        assert!(selection.take_change().is_none());
    }

    #[test]
    fn a_press_without_a_target_clears_the_selection() {
        let registry = registry(&["hello world"]);
        let mut selection = Selection::default();
        selection.press(&registry, at(15., 10.), 2);
        assert!(selection.has_text());
        // Far below every run: outside the vertical band.
        let outcome = selection.press(&registry, at(15., 300.), 1);
        assert!(outcome.refresh && !selection.has_text() && !selection.is_pressed());
    }

    #[test]
    fn non_selectable_regions_block_a_press_but_text_islands_reenable() {
        use crate::registry::StartRegion;
        use vespertine::{Bounds, size};
        let mut registry = registry(&["hello world"]);
        let bounds = |x: f32, w: f32| Bounds {
            origin: point(px(x), px(0.)),
            size: size(px(w), px(24.)),
        };
        registry.register_region(StartRegion {
            bounds: bounds(0., 120.),
            selectable: false,
        });
        let mut selection = Selection::default();
        selection.press(&registry, at(15., 10.), 2);
        assert!(!selection.has_text(), "blocked by the region");
        // An explicit text island inside registers later and wins.
        registry.register_region(StartRegion {
            bounds: bounds(0., 50.),
            selectable: true,
        });
        selection.press(&registry, at(15., 10.), 2);
        assert!(selection.has_text());
        selection.press(&registry, at(85., 10.), 2);
        assert!(
            !selection.has_text(),
            "outside the island the region still blocks"
        );
    }

    #[test]
    fn press_target_prefers_the_last_containing_run_then_the_nearest() {
        let mut registry = Registry::default();
        registry.register(run(0, 0, "underlay text", 0.0, None));
        registry.register(run(1, 0, "overlay", 0.0, None));
        let mut selection = Selection::default();
        selection.press(&registry, at(5., 10.), 3);
        assert_eq!(selection.selected_text().as_deref(), Some("overlay"));

        // Nearest by vertical distance first, horizontal on ties; never a weighted sum.
        let mut registry = Registry::default();
        registry.register(run(0, 0, "aaaaaaaaaa", 0.0, None));
        registry.register(run(1, 0, "bbbbbbbbbb", 48.0, None));
        let p = at(300., 30.); // 6 below the first, 18 above the second
        let target = registry.press_target(p);
        assert!(matches!(target, crate::registry::PressTarget::NoRun));
        // Within the vertical band of the nearest run the press counts, at the end of the line.
        let target = registry.press_target(at(300., 20.));
        assert_eq!(
            target,
            crate::registry::PressTarget::Run { run: 0, index: 10 }
        );
    }

    #[test]
    fn unmounted_anchor_keeps_the_forward_side_of_the_selection() {
        // Drag from run 0, then runs 0 and 1 unmount: the head keeps extending.
        let full = registry(&["zero zero", "one one", "two two", "three three"]);
        let mut selection = Selection::default();
        selection.press(&full, at(5., 10.), 1);
        selection.drag_move(&full, at(25., 34.), true);
        assert_eq!(selection.selected_text().as_deref(), Some("zero zero\non"));
        // Virtualization unmounts runs 0 (the anchor); runs 1.. remain, shifted up.
        let mut scrolled = Registry::default();
        for (ix, line) in ["one one", "two two", "three three"].iter().enumerate() {
            scrolled.register(run(ix as u32 + 1, 0, line, ix as f32 * 24.0, None));
        }
        selection.update_head(&scrolled, at(35., 58.));
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("zero zero\none one\ntwo two\nthr"),
            "old spans before the first painted run are kept"
        );
        // The drag still ends cleanly with the anchor unpainted.
        assert!(selection.release());
    }

    #[test]
    fn unmounted_anchor_backward_and_ignored_cases() {
        let full = registry(&["zero", "one", "two", "three"]);
        let mut selection = Selection::default();
        selection.press(&full, at(25., 82.), 1);
        selection.drag_move(&full, at(5., 34.), true);
        assert_eq!(selection.selected_text().as_deref(), Some("one\ntwo\nth"));
        // The anchor run (3) unmounts; runs 0..=2 remain.
        let partial = registry(&["zero", "one", "two"]);
        selection.update_head(&partial, at(5., 10.));
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("zero\none\ntwo\nth")
        );
        // No painted run overlaps the old spans: the update is ignored.
        let mut unrelated = Registry::default();
        unrelated.register(run(40, 0, "unrelated", 0.0, None));
        let before = selection.selected_text();
        assert!(!selection.update_head(&unrelated, at(5., 10.)));
        assert_eq!(selection.selected_text(), before);
    }

    #[test]
    fn copy_works_after_the_run_leaves_the_registry() {
        let full = registry(&["alpha beta", "gamma delta"]);
        let mut selection = Selection::default();
        selection.press(&full, at(5., 10.), 1);
        selection.drag_move(&full, at(35., 34.), true);
        selection.release();
        drop(full);
        assert_eq!(
            selection.selected_text().as_deref(),
            Some("alpha beta\ngam")
        );
    }
}
