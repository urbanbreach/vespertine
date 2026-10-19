//! Match resolution over logical groups (TXT-009 R6, R7, R9, R10): query matches per group,
//! explicit UTF-16 ranges over the joined document, per-run segments, a content-addressed result
//! cache, and the identity the count callback compares.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use super::matcher::{GroupMatcher, Query, utf16_len, utf16_to_byte};
use super::spec::HighlightDeclaration;

/// One logical group (R6): the adjacent primitive text pieces of one host element. Empty pieces
/// are transparent. A query match may span the pieces of a group but never crosses groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupInput {
    /// Stable identity of the group (the caller's element identity).
    pub id: u64,
    pub pieces: Vec<Arc<str>>,
}

impl GroupInput {
    pub fn new<S: Into<Arc<str>>>(id: u64, pieces: impl IntoIterator<Item = S>) -> Self {
        Self {
            id,
            pieces: pieces.into_iter().map(Into::into).collect(),
        }
    }
}

/// The part of a match that lies inside one piece (one painted run).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    pub group_id: u64,
    /// Index of the piece in [`GroupInput::pieces`].
    pub piece_index: usize,
    /// Byte range inside that piece. Never empty.
    pub range: Range<usize>,
}

/// Where a match came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchKind {
    Query,
    Range,
}

/// One match: counted once however many pieces or rows it crosses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMatch {
    /// Index into the declaration's specifications.
    pub spec_index: usize,
    /// Static per-specification ordinal: query matches in document order first, then range
    /// matches in the order the ranges were supplied. The range matches' ordinals are final;
    /// query matches are renumbered in paint order (see `Numbering`).
    pub ordinal: usize,
    pub kind: MatchKind,
    /// Document order; at least one segment.
    pub segments: Vec<Segment>,
}

/// The cached, color-free and number-free result of matching a declaration against groups.
#[derive(Clone, Debug)]
pub struct Resolution {
    /// All matches of all specifications across all groups (the `matchCount`, R9).
    pub total: usize,
    /// Ordered by specification, then ordinal.
    pub matches: Vec<ResolvedMatch>,
    /// Hash of the total and of every match's specification, ordinal and segments; it is
    /// independent of colors, radius and `activeIndex`.
    pub identity: u64,
    by_run: HashMap<(u64, usize), Vec<(usize, usize)>>,
}

impl Resolution {
    /// The resolution of nothing: no matches.
    pub fn empty() -> Self {
        Self::from_matches(Vec::new())
    }

    fn from_matches(matches: Vec<ResolvedMatch>) -> Self {
        let mut by_run: HashMap<(u64, usize), Vec<(usize, usize)>> = HashMap::new();
        let mut hasher = DefaultHasher::new();
        matches.len().hash(&mut hasher);
        for (match_index, resolved) in matches.iter().enumerate() {
            resolved.spec_index.hash(&mut hasher);
            resolved.ordinal.hash(&mut hasher);
            resolved.segments.hash(&mut hasher);
            for (segment_index, segment) in resolved.segments.iter().enumerate() {
                by_run
                    .entry((segment.group_id, segment.piece_index))
                    .or_default()
                    .push((match_index, segment_index));
            }
        }
        Self {
            total: matches.len(),
            identity: hasher.finish(),
            matches,
            by_run,
        }
    }

    /// The segments that fall inside piece `piece_index` of group `group_id`, each with the index
    /// of its match in [`Resolution::matches`] (the key under which painted numbers are kept).
    /// Segments come in match order.
    pub fn segments_for(
        &self,
        group_id: u64,
        piece_index: usize,
    ) -> impl Iterator<Item = (usize, &Segment)> {
        self.by_run
            .get(&(group_id, piece_index))
            .into_iter()
            .flatten()
            .map(|&(match_index, segment_index)| {
                (
                    match_index,
                    &self.matches[match_index].segments[segment_index],
                )
            })
    }
}

/// Cached state of one group: text, offset-mapped lowercase copy and per-query results.
struct GroupEntry {
    pieces: Vec<Arc<str>>,
    piece_starts: Vec<usize>,
    matcher: GroupMatcher,
    results: Vec<(Query, Vec<Range<usize>>)>,
    utf16: OnceCell<usize>,
}

impl GroupEntry {
    fn new(pieces: &[Arc<str>]) -> Self {
        let mut piece_starts = Vec::with_capacity(pieces.len());
        let mut text = String::new();
        for piece in pieces {
            piece_starts.push(text.len());
            text.push_str(piece);
        }
        Self {
            pieces: pieces.to_vec(),
            piece_starts,
            matcher: GroupMatcher::new(text),
            results: Vec::new(),
            utf16: OnceCell::new(),
        }
    }

    fn ensure_results(&mut self, query: &Query) {
        if !self.results.iter().any(|(cached, _)| cached == query) {
            let found = self.matcher.find(query);
            self.results.push((query.clone(), found));
        }
    }

    fn results(&self, query: &Query) -> &[Range<usize>] {
        self.results
            .iter()
            .find(|(cached, _)| cached == query)
            .map_or(&[], |(_, found)| found)
    }

    fn utf16_len(&self) -> usize {
        *self.utf16.get_or_init(|| utf16_len(self.matcher.text()))
    }

    /// Splits a byte range of the group's concatenated text into per-piece segments.
    fn segments(&self, group_id: u64, range: Range<usize>, out: &mut Vec<Segment>) {
        for (piece_index, (start, piece)) in self.piece_starts.iter().zip(&self.pieces).enumerate()
        {
            let low = range.start.max(*start);
            let high = range.end.min(start + piece.len());
            if low < high {
                out.push(Segment {
                    group_id,
                    piece_index,
                    range: low - start..high - start,
                });
            }
        }
    }
}

/// Start (UTF-16 offset in the joined document) and length of every group; groups are joined by a
/// single newline, so group `i + 1` starts one unit after group `i` ends.
struct DocumentLayout {
    starts: Vec<usize>,
    lens: Vec<usize>,
    total: usize,
}

impl DocumentLayout {
    fn new(entries: &[GroupEntry]) -> Self {
        let mut starts = Vec::with_capacity(entries.len());
        let mut lens = Vec::with_capacity(entries.len());
        let mut next = 0;
        for entry in entries {
            starts.push(next);
            let len = entry.utf16_len();
            lens.push(len);
            next += len + 1;
        }
        Self {
            starts,
            lens,
            total: next.saturating_sub(1),
        }
    }

    /// The group containing document offset `offset` (a group's end offset belongs to that
    /// group; the separator newline lies between its end and the next group's start) and the
    /// UTF-16 offset inside it.
    fn locate(&self, offset: usize) -> Option<(usize, usize)> {
        let after = self.starts.partition_point(|&start| start <= offset);
        let group = after.checked_sub(1)?;
        let inner = offset - self.starts[group];
        (inner <= self.lens[group]).then_some((group, inner))
    }
}

/// Reasons an explicit range is rejected (R6).
fn range_segments(
    range: &Range<usize>,
    layout: &DocumentLayout,
    entries: &[GroupEntry],
    ids: &[u64],
) -> Result<Vec<Segment>, &'static str> {
    if range.start >= range.end {
        return Err("reversed or empty range");
    }
    if range.end > layout.total {
        return Err("range extends past the end of the document");
    }
    let byte_of = |offset: usize| -> Result<(usize, usize), &'static str> {
        let (group, inner) = layout.locate(offset).ok_or("offset outside the document")?;
        let byte = utf16_to_byte(entries[group].matcher.text(), inner)
            .ok_or("endpoint falls inside a surrogate pair")?;
        Ok((group, byte))
    };
    let (first, first_byte) = byte_of(range.start)?;
    let (last, last_byte) = byte_of(range.end)?;
    let mut segments = Vec::new();
    for group in first..=last {
        let text_len = entries[group].matcher.text().len();
        let start = if group == first { first_byte } else { 0 };
        let end = if group == last { last_byte } else { text_len };
        if start < end {
            entries[group].segments(ids[group], start..end, &mut segments);
        }
    }
    Ok(segments)
}

/// Resolves declarations against groups and caches the outcome (R10).
///
/// Results are cached by the declaration's matcher hash AND the content of the groups: calling
/// again with groups and matcher fields unchanged returns the same `Arc` without scanning; a
/// changed query rescans each group once, reusing each unchanged group's lowercased copy and
/// offset map; a changed group is rebuilt and rescanned alone.
///
/// Nested declarations (R3) are the caller's business: pass only the groups the declaration owns,
/// and let [`OwnershipStack`] pick the owner while walking the tree.
#[derive(Default)]
pub struct Resolver {
    entries: HashMap<u64, GroupEntry>,
    last: Option<Cached>,
}

struct Cached {
    hash: u64,
    declaration: HighlightDeclaration,
    groups: Vec<GroupInput>,
    resolution: Arc<Resolution>,
}

impl Resolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches `declaration` against `groups` (document order). Explicit ranges index the joined
    /// document of exactly these groups.
    pub fn resolve(
        &mut self,
        declaration: &HighlightDeclaration,
        groups: &[GroupInput],
    ) -> Arc<Resolution> {
        let hash = declaration.matcher_hash();
        if let Some(cached) = &self.last
            && cached.hash == hash
            && cached.declaration.same_matcher(declaration)
            && cached.groups == groups
        {
            return cached.resolution.clone();
        }

        // Reuse the entry of every group whose pieces did not change.
        let mut old = std::mem::take(&mut self.entries);
        let mut entries: Vec<GroupEntry> = groups
            .iter()
            .map(|group| match old.remove(&group.id) {
                Some(entry) if entry.pieces == group.pieces => entry,
                _ => GroupEntry::new(&group.pieces),
            })
            .collect();
        let ids: Vec<u64> = groups.iter().map(|group| group.id).collect();

        let layout = declaration
            .has_ranges()
            .then(|| DocumentLayout::new(&entries));
        let mut matches = Vec::new();
        let mut queries = Vec::new();
        for (spec_index, spec) in declaration.specs().iter().enumerate() {
            let first = matches.len();
            if !spec.query.is_empty() {
                let query = Query::new(&spec.query, spec.case_sensitive, spec.whole_word);
                for (entry, &group_id) in entries.iter_mut().zip(&ids) {
                    entry.ensure_results(&query);
                    for range in entry.results(&query) {
                        let mut segments = Vec::new();
                        entry.segments(group_id, range.clone(), &mut segments);
                        matches.push(ResolvedMatch {
                            spec_index,
                            ordinal: matches.len() - first,
                            kind: MatchKind::Query,
                            segments,
                        });
                    }
                }
                queries.push(query);
            }
            if let Some(layout) = &layout {
                for range in &spec.ranges {
                    match range_segments(range, layout, &entries, &ids) {
                        Ok(segments) if segments.is_empty() => {}
                        Ok(segments) => matches.push(ResolvedMatch {
                            spec_index,
                            ordinal: matches.len() - first,
                            kind: MatchKind::Range,
                            segments,
                        }),
                        Err(reason) => {
                            log::warn!("highlight range {range:?} rejected: {reason}");
                        }
                    }
                }
            }
        }

        // Keep only results a later keystroke could still reuse.
        for entry in &mut entries {
            entry.results.retain(|(cached, _)| queries.contains(cached));
        }
        self.entries = groups.iter().map(|group| group.id).zip(entries).collect();

        let resolution = Arc::new(Resolution::from_matches(matches));
        self.last = Some(Cached {
            hash,
            declaration: declaration.clone(),
            groups: groups.to_vec(),
            resolution: resolution.clone(),
        });
        resolution
    }
}

/// Walks a tree and tells which declaration owns a text run (R3): the nearest declaring ancestor
/// owns it, and a descendant that declares overrides the ancestor for its whole subtree even when
/// its own declaration is empty.
///
/// Usage while collecting groups: `push(Some(decl))` when entering a host that declares a
/// highlight (an empty declaration counts), `push(None)` otherwise, `pop()` when leaving; at each
/// group, `owner()` is the declaration that resolves it. Each declaration gets only the groups it
/// owns, so the ancestor neither resolves nor counts matches inside an overriding subtree.
#[derive(Debug)]
pub struct OwnershipStack<D> {
    frames: Vec<Option<D>>,
}

impl<D> Default for OwnershipStack<D> {
    fn default() -> Self {
        Self { frames: Vec::new() }
    }
}

impl<D> OwnershipStack<D> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, declaration: Option<D>) {
        self.frames.push(declaration);
    }

    pub fn pop(&mut self) -> Option<D> {
        self.frames.pop().flatten()
    }

    /// The nearest declaration on the stack, if any.
    pub fn owner(&self) -> Option<&D> {
        self.frames.iter().rev().find_map(Option::as_ref)
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::matcher::scan_count;
    use crate::search::spec::HighlightSpec;

    fn group(id: u64, pieces: &[&str]) -> GroupInput {
        GroupInput::new(id, pieces.iter().copied())
    }

    fn decl(spec: HighlightSpec) -> HighlightDeclaration {
        HighlightDeclaration::from_spec(spec)
    }

    fn resolve(spec: HighlightSpec, groups: &[GroupInput]) -> Arc<Resolution> {
        Resolver::new().resolve(&decl(spec), groups)
    }

    fn seg(group_id: u64, piece_index: usize, range: Range<usize>) -> Segment {
        Segment {
            group_id,
            piece_index,
            range,
        }
    }

    #[test]
    fn a_match_spans_inline_pieces_and_counts_once() {
        let groups = [group(1, &["Hello ", "Tom", "!"])];
        let resolution = resolve(HighlightSpec::new("lo Tom"), &groups);
        assert_eq!(resolution.total, 1);
        assert_eq!(
            resolution.matches[0].segments,
            vec![seg(1, 0, 3..6), seg(1, 1, 0..3)]
        );
        // Per-run lookup finds the same match in both pieces and nowhere else.
        let first: Vec<_> = resolution.segments_for(1, 0).collect();
        let second: Vec<_> = resolution.segments_for(1, 1).collect();
        assert_eq!(first, vec![(0, &seg(1, 0, 3..6))]);
        assert_eq!(second, vec![(0, &seg(1, 1, 0..3))]);
        assert_eq!(resolution.segments_for(1, 2).count(), 0);
        assert_eq!(resolution.segments_for(9, 0).count(), 0);
    }

    #[test]
    fn empty_pieces_are_transparent() {
        let groups = [group(1, &["a", "", "b"])];
        let resolution = resolve(HighlightSpec::new("ab"), &groups);
        assert_eq!(resolution.total, 1);
        assert_eq!(
            resolution.matches[0].segments,
            vec![seg(1, 0, 0..1), seg(1, 2, 0..1)]
        );
    }

    #[test]
    fn matches_never_cross_groups() {
        let groups = [group(1, &["ab"]), group(2, &["cd"])];
        assert_eq!(resolve(HighlightSpec::new("bc"), &groups).total, 0);
        assert_eq!(resolve(HighlightSpec::new("b"), &groups).total, 1);
        assert_eq!(resolve(HighlightSpec::new("b\ncd"), &groups).total, 0);
    }

    #[test]
    fn query_matches_are_numbered_in_document_order_across_groups() {
        let groups = [group(1, &["a b a"]), group(2, &["a"])];
        let resolution = resolve(HighlightSpec::new("a"), &groups);
        let found: Vec<_> = resolution
            .matches
            .iter()
            .map(|m| {
                (
                    m.ordinal,
                    m.segments[0].group_id,
                    m.segments[0].range.clone(),
                )
            })
            .collect();
        assert_eq!(found, vec![(0, 1, 0..1), (1, 1, 4..5), (2, 2, 0..1)]);
    }

    #[test]
    fn explicit_ranges_index_the_joined_document() {
        // Groups of lengths 5 and 3: the document is "hello\nabc".
        let groups = [group(1, &["hello"]), group(2, &["abc"])];
        let second = resolve(HighlightSpec::from_ranges([6..9]), &groups);
        assert_eq!(second.total, 1);
        assert_eq!(second.matches[0].kind, MatchKind::Range);
        assert_eq!(second.matches[0].segments, vec![seg(2, 0, 0..3)]);

        // [5, 6) covers only the separator: nothing painted, not a match.
        let separator = resolve(HighlightSpec::from_ranges([5..6]), &groups);
        assert_eq!(separator.total, 0);
        assert!(separator.matches.is_empty());

        // It does not shift later ordinals.
        let resolution = resolve(HighlightSpec::from_ranges([5..6, 6..9, 0..1]), &groups);
        assert_eq!(resolution.total, 2);
        assert_eq!(resolution.matches[0].ordinal, 0);
        assert_eq!(resolution.matches[0].segments, vec![seg(2, 0, 0..3)]);
        assert_eq!(resolution.matches[1].ordinal, 1);
        assert_eq!(resolution.matches[1].segments, vec![seg(1, 0, 0..1)]);
    }

    #[test]
    fn a_range_overlapping_two_groups_is_split_and_counts_once() {
        let groups = [group(1, &["hello"]), group(2, &["abc"])];
        let resolution = resolve(HighlightSpec::from_ranges([3..8]), &groups);
        assert_eq!(resolution.total, 1);
        assert_eq!(
            resolution.matches[0].segments,
            vec![seg(1, 0, 3..5), seg(2, 0, 0..2)]
        );
        // A range across the whole document, separator included.
        let all = resolve(HighlightSpec::from_ranges([0..9]), &groups);
        assert_eq!(all.total, 1);
        assert_eq!(
            all.matches[0].segments,
            vec![seg(1, 0, 0..5), seg(2, 0, 0..3)]
        );
    }

    #[test]
    fn a_range_across_pieces_yields_one_segment_per_piece() {
        let groups = [group(1, &["ab", "", "cd"])];
        let resolution = resolve(HighlightSpec::from_ranges([1..3]), &groups);
        assert_eq!(
            resolution.matches[0].segments,
            vec![seg(1, 0, 1..2), seg(1, 2, 0..1)]
        );
    }

    #[test]
    fn ranges_use_utf16_units_and_reject_surrogate_interiors() {
        // "a", U+1F600 (two UTF-16 units, four bytes), "b".
        let groups = [group(1, &["a\u{1F600}b"])];
        let valid = resolve(HighlightSpec::from_ranges([1..3]), &groups);
        assert_eq!(valid.total, 1);
        assert_eq!(valid.matches[0].segments, vec![seg(1, 0, 1..5)]);
        // Either endpoint inside the surrogate pair rejects the range.
        assert_eq!(
            resolve(HighlightSpec::from_ranges([1..2]), &groups).total,
            0
        );
        assert_eq!(
            resolve(HighlightSpec::from_ranges([2..3]), &groups).total,
            0
        );
        // Rejected ranges consume no ordinal.
        let mixed = resolve(HighlightSpec::from_ranges([1..2, 3..4]), &groups);
        assert_eq!(mixed.total, 1);
        assert_eq!(mixed.matches[0].ordinal, 0);
        assert_eq!(mixed.matches[0].segments, vec![seg(1, 0, 5..6)]);
        // The document end is a valid endpoint; past it is rejected.
        assert_eq!(
            resolve(HighlightSpec::from_ranges([0..4]), &groups).total,
            1
        );
        assert_eq!(
            resolve(HighlightSpec::from_ranges([0..5]), &groups).total,
            0
        );
        assert_eq!(
            resolve(HighlightSpec::from_ranges([9..12]), &groups).total,
            0
        );
    }

    #[test]
    fn ranges_without_groups_are_rejected() {
        assert_eq!(resolve(HighlightSpec::from_ranges([0..1]), &[]).total, 0);
    }

    #[test]
    fn query_matches_come_first_then_ranges_per_specification() {
        let groups = [group(1, &["hello"]), group(2, &["abc"])];
        let spec = HighlightSpec::new("l").with_ranges([6..9, 0..1]);
        let resolution = resolve(spec, &groups);
        let found: Vec<_> = resolution
            .matches
            .iter()
            .map(|m| (m.ordinal, m.kind))
            .collect();
        assert_eq!(
            found,
            vec![
                (0, MatchKind::Query),
                (1, MatchKind::Query),
                (2, MatchKind::Range),
                (3, MatchKind::Range)
            ]
        );
        assert_eq!(resolution.matches[2].segments, vec![seg(2, 0, 0..3)]);
    }

    #[test]
    fn specifications_are_numbered_independently() {
        let groups = [group(1, &["red blue red"])];
        let declaration = HighlightDeclaration::from_specs([
            HighlightSpec::new("red"),
            HighlightSpec::new("blue"),
        ]);
        let resolution = Resolver::new().resolve(&declaration, &groups);
        let found: Vec<_> = resolution
            .matches
            .iter()
            .map(|m| (m.spec_index, m.ordinal))
            .collect();
        assert_eq!(found, vec![(0, 0), (0, 1), (1, 0)]);
        assert_eq!(resolution.total, 3);
    }

    #[test]
    fn nested_declarations_are_resolved_by_owner() {
        // outer ("a") owns groups 1 and 3; an inner declaration with an empty result owns group 2.
        let mut stack: OwnershipStack<HighlightDeclaration> = OwnershipStack::new();
        stack.push(Some(decl(HighlightSpec::new("a"))));
        assert_eq!(stack.owner().map(|d| d.specs().len()), Some(1));
        stack.push(None);
        assert_eq!(stack.owner().map(|d| d.specs().len()), Some(1));
        stack.push(Some(HighlightDeclaration::new()));
        // The empty inner declaration overrides the ancestor in its subtree.
        let inner_owner = stack.owner().unwrap();
        assert!(inner_owner.is_empty());
        stack.pop();
        stack.pop();
        assert!(!stack.owner().unwrap().is_empty());
        assert_eq!(stack.depth(), 1);

        let mut resolver = Resolver::new();
        let outer_groups = [group(1, &["a"]), group(3, &["a a"])];
        let outer = resolver.resolve(&decl(HighlightSpec::new("a")), &outer_groups);
        // Group 2 ("a") belongs to the inner owner: not counted by the ancestor.
        assert_eq!(outer.total, 3);
        assert_eq!(outer.segments_for(2, 0).count(), 0);
        let mut inner_resolver = Resolver::new();
        let inner = inner_resolver.resolve(&HighlightDeclaration::new(), &[group(2, &["a"])]);
        assert_eq!(inner.total, 0);
        assert_eq!(inner.identity, Resolution::empty().identity);
    }

    #[test]
    fn unchanged_inputs_reuse_the_resolution_without_scanning() {
        let groups = [group(1, &["red green"]), group(2, &["red"])];
        let mut resolver = Resolver::new();
        let base = decl(HighlightSpec::new("red").with_active_index(Some(0)));
        let first = resolver.resolve(&base, &groups);
        let scans = scan_count();

        let color = default_color();
        let variants = [
            HighlightSpec::new("red").with_active_index(Some(1)),
            HighlightSpec::new("red").with_color(color),
            HighlightSpec::new("red").with_active_color(color),
            HighlightSpec::new("red").with_radius(6.0),
            HighlightSpec::new("red").with_match_index_offset(10),
        ];
        for variant in variants {
            let again = resolver.resolve(&decl(variant), &groups);
            assert!(Arc::ptr_eq(&first, &again));
            assert_eq!(again.identity, first.identity);
        }
        assert_eq!(scan_count(), scans);
    }

    fn default_color() -> vespertine::Hsla {
        crate::search::spec::default_hex_color("#F0C04066").unwrap()
    }

    #[test]
    fn a_new_query_rescans_each_group_once_and_a_changed_group_alone() {
        let mut resolver = Resolver::new();
        let groups = [
            group(1, &["red green"]),
            group(2, &["red"]),
            group(3, &["blue"]),
        ];
        resolver.resolve(&decl(HighlightSpec::new("r")), &groups);

        let before = scan_count();
        resolver.resolve(&decl(HighlightSpec::new("re")), &groups);
        assert_eq!(scan_count() - before, 3, "one scan per group per keystroke");

        // The very same inputs hit the resolution cache.
        let before = scan_count();
        resolver.resolve(&decl(HighlightSpec::new("re")), &groups);
        assert_eq!(scan_count() - before, 0);

        // Only group 2 changes: only it is rescanned; the others reuse their results.
        let changed = [
            group(1, &["red green"]),
            group(2, &["reddish red"]),
            group(3, &["blue"]),
        ];
        let before = scan_count();
        let resolution = resolver.resolve(&decl(HighlightSpec::new("re")), &changed);
        assert_eq!(scan_count() - before, 1);
        assert_eq!(resolution.total, 4);
    }

    #[test]
    fn range_only_changes_do_not_scan_text() {
        let mut resolver = Resolver::new();
        let groups = [group(1, &["hello"]), group(2, &["abc"])];
        let before = scan_count();
        resolver.resolve(&decl(HighlightSpec::from_ranges([0..2])), &groups);
        resolver.resolve(&decl(HighlightSpec::from_ranges([6..9])), &groups);
        assert_eq!(scan_count() - before, 0);
    }

    #[test]
    fn identity_changes_with_query_even_when_the_count_is_equal() {
        let groups = [group(1, &["red blue"])];
        let mut resolver = Resolver::new();
        let red = resolver.resolve(&decl(HighlightSpec::new("red")), &groups);
        let blue = resolver.resolve(&decl(HighlightSpec::new("blue")), &groups);
        assert_eq!(red.total, blue.total);
        assert_ne!(red.identity, blue.identity);
        // Same matches from a different specification index are a different identity.
        let shifted = resolver.resolve(
            &HighlightDeclaration::from_specs([
                HighlightSpec::new("nothing"),
                HighlightSpec::new("red"),
            ]),
            &groups,
        );
        assert_ne!(shifted.identity, red.identity);
        // Group text edits that move the match change it as well.
        let moved = resolver.resolve(
            &decl(HighlightSpec::new("red")),
            &[group(1, &["x red blue"])],
        );
        assert_ne!(moved.identity, red.identity);
    }

    #[test]
    fn empty_declaration_resolves_to_the_empty_resolution() {
        let resolution = Resolver::new().resolve(&HighlightDeclaration::new(), &[group(1, &["a"])]);
        assert_eq!(resolution.total, 0);
        assert_eq!(resolution.identity, Resolution::empty().identity);
    }

    #[test]
    fn whole_word_and_case_flags_flow_through_resolution() {
        let groups = [group(1, &["foo_bar foo"])];
        let word = resolve(HighlightSpec::new("foo").with_whole_word(true), &groups);
        assert_eq!(word.total, 1);
        assert_eq!(word.matches[0].segments, vec![seg(1, 0, 8..11)]);
        let any = resolve(HighlightSpec::new("FOO"), &groups);
        assert_eq!(any.total, 2);
        assert_eq!(
            resolve(HighlightSpec::new("FOO").with_case_sensitive(true), &groups).total,
            0
        );
    }
}
