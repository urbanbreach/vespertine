//! Bounded bidirectional history (CMP-029 R22-R28): at most five pages mounted, real empty edge
//! rows that trigger loads, two-commit insertion and eviction, and pixel-exact anchor restoration
//! when pages land while the reader waits in an edge row.
//!
//! This is a consumer pattern, not a network client: [`History`] tracks identities, pages,
//! cursors and the request in flight, and turns a landing page into list splices
//! ([`HistoryCommit`]) plus a scroll restoration ([`Restore`]). The consumer owns the item
//! content (looked up by identity) and the asynchronous fetch.

use std::{collections::HashSet, hash::Hash, ops::Range};

use vespertine::{ListOffset, ListState, px};

/// Messages per page (R22).
pub const DEFAULT_PAGE_SIZE: usize = 12;
/// Pages that remain mounted after settlement (R23).
pub const MAX_PAGES: usize = 5;
/// Height of an empty edge row in px (R23).
pub const EDGE_ROW_HEIGHT: f32 = 160.0;

/// The direction of a page request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Items before the cursor.
    Previous,
    /// Items after the cursor.
    Next,
    /// A page centered around a target item.
    Around,
}

/// A page request. Cursors are item identities and are exclusive: the returned page never
/// repeats the named item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request<I> {
    /// The direction.
    pub direction: Direction,
    /// The cursor (the target item for [`Direction::Around`]).
    pub cursor: Option<I>,
}

/// A returned page: items plus the identity of its first item as the previous cursor (absent when
/// it starts at the first item) and of its last item as the next cursor (absent at the last item).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<I> {
    /// The items, oldest first.
    pub items: Vec<I>,
    /// The cursor for older history.
    pub previous_cursor: Option<I>,
    /// The cursor for newer history.
    pub next_cursor: Option<I>,
}

/// One row of the mounted list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryRow<I> {
    /// The real empty leading edge row (older history exists).
    LeadingEdge,
    /// A loaded item.
    Item(I),
    /// The real empty trailing edge row (newer history exists).
    TrailingEdge,
}

/// A splice of the mounted list: rows `old_range` were replaced by `new_count` rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryCommit {
    /// The replaced rows, in the list's rows before this commit.
    pub old_range: Range<usize>,
    /// The number of rows that replace them.
    pub new_count: usize,
}

/// The reader's position, sampled at response arrival (R25).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReaderSample {
    /// The top anchor row of the list (the item index of the logical scroll top).
    pub index: usize,
    /// The pixel offset within that row.
    pub offset: f32,
    /// The viewport height.
    pub viewport: f32,
}

impl ReaderSample {
    /// Samples the list's current top anchor and viewport height.
    pub fn from_list(list: &ListState) -> Self {
        let top = list.logical_scroll_top();
        let viewport = f32::from(list.viewport_bounds().size.height);
        let (max, position) = list.scrollbar_extents();
        if max > px(0.0) && position >= max - px(0.5) {
            // The viewport bottom sits on the bottom of the last row: the at-end sentinel, an
            // index beyond the last row (R27).
            return Self {
                index: list.item_count(),
                offset: 0.0,
                viewport,
            };
        }
        Self {
            index: top.item_ix,
            offset: f32::from(top.offset_in_item),
            viewport,
        }
    }
}

/// Where to scroll after the commits of a landing page: row `index` at signed offset `offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Restore {
    /// The row to anchor.
    pub index: usize,
    /// The signed pixel offset (negative places the row below the viewport top).
    pub offset: f32,
}

/// Everything a landing page did: the commits in order (insertion first, then eviction as its own
/// commit), and an explicit restoration when the reader waited in an edge row.
#[derive(Clone, Debug, PartialEq)]
pub struct Landing {
    /// Synchronous commits, in the order they must be applied.
    pub commits: Vec<HistoryCommit>,
    /// The scroll restoration, when one must be forced.
    pub restore: Option<Restore>,
}

/// The bounded paging state of one conversation or log.
#[derive(Clone, Debug)]
pub struct History<I: Clone + Eq + Hash> {
    page_size: usize,
    pages: Vec<Vec<I>>,
    previous_cursor: Option<I>,
    next_cursor: Option<I>,
    pending: Option<Request<I>>,
}

impl<I: Clone + Eq + Hash> History<I> {
    /// Mounts exactly one page (R22): the newest page by default, or the page the consumer's
    /// `initialPage` returned for a requested item.
    pub fn mount(page_size: usize, page: Page<I>) -> Self {
        Self {
            page_size: page_size.max(1),
            pages: if page.items.is_empty() {
                Vec::new()
            } else {
                vec![page.items]
            },
            previous_cursor: page.previous_cursor,
            next_cursor: page.next_cursor,
            pending: None,
        }
    }

    /// The page size.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// The number of mounted pages (at most [`MAX_PAGES`] once settled).
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The loaded items in order.
    pub fn items(&self) -> impl Iterator<Item = &I> {
        self.pages.iter().flatten()
    }

    /// The number of loaded items.
    pub fn item_count(&self) -> usize {
        self.pages.iter().map(Vec::len).sum()
    }

    /// Whether older history exists (a real leading edge row is mounted).
    pub fn has_leading_edge(&self) -> bool {
        self.previous_cursor.is_some()
    }

    /// Whether newer history exists (a real trailing edge row is mounted).
    pub fn has_trailing_edge(&self) -> bool {
        self.next_cursor.is_some()
    }

    /// The number of mounted list rows: items plus the edge rows.
    pub fn row_count(&self) -> usize {
        self.item_count()
            + usize::from(self.has_leading_edge())
            + usize::from(self.has_trailing_edge())
    }

    /// The mounted rows in order.
    pub fn rows(&self) -> Vec<HistoryRow<I>> {
        let mut rows = Vec::with_capacity(self.row_count());
        if self.has_leading_edge() {
            rows.push(HistoryRow::LeadingEdge);
        }
        rows.extend(self.items().cloned().map(HistoryRow::Item));
        if self.has_trailing_edge() {
            rows.push(HistoryRow::TrailingEdge);
        }
        rows
    }

    /// The row index of the first loaded item.
    pub fn first_item_row(&self) -> usize {
        usize::from(self.has_leading_edge())
    }

    /// The row index of loaded item `id`.
    pub fn row_of(&self, id: &I) -> Option<usize> {
        self.items()
            .position(|item| item == id)
            .map(|position| position + self.first_item_row())
    }

    /// The request in flight.
    pub fn pending(&self) -> Option<&Request<I>> {
        self.pending.as_ref()
    }

    /// Whether the leading (`Previous`) or trailing (`Next`) edge row shows its loading state.
    /// The loading state lives inside the existing edge row: list and row identities are kept.
    pub fn is_loading(&self, direction: Direction) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|request| request.direction == direction)
    }

    /// A report of the list's visible range (mounted row indices, end exclusive) (R23).
    ///
    /// A request starts ONLY when its edge row is visible: no prefetch threshold, the reader
    /// reaches the empty space and waits. When both edges qualify the leading edge wins. A
    /// pending request blocks every other request, including repeated visibility reports.
    pub fn on_visible_range(&mut self, visible: Range<usize>) -> Option<Request<I>> {
        if self.pending.is_some() || visible.is_empty() {
            return None;
        }
        let rows = self.row_count();
        let request = if self.has_leading_edge() && visible.start == 0 {
            Request {
                direction: Direction::Previous,
                cursor: self.previous_cursor.clone(),
            }
        } else if self.has_trailing_edge() && visible.end >= rows {
            Request {
                direction: Direction::Next,
                cursor: self.next_cursor.clone(),
            }
        } else {
            return None;
        };
        self.pending = Some(request.clone());
        Some(request)
    }

    /// Starts an around request for a jump (R28). Ignored (returns `None`) while another request
    /// is pending.
    pub fn begin_jump(&mut self, target: I) -> Option<Request<I>> {
        if self.pending.is_some() {
            return None;
        }
        let request = Request {
            direction: Direction::Around,
            cursor: Some(target),
        };
        self.pending = Some(request.clone());
        Some(request)
    }

    /// Clears the request in flight (a failed fetch: the edge row can trigger again).
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    /// Applies a landing `Previous` or `Next` page (R24-R27).
    ///
    /// `sample` is the reader's position at response arrival (not at request start), so the
    /// reader can keep scrolling during the request. Incoming items already known are dropped; a
    /// page that becomes empty changes nothing. Insertion is one commit; eviction of the far end
    /// is a second, separate commit, so that each stays a pure insert or a pure removal and keeps
    /// measured heights and logical anchors.
    pub fn apply_page(
        &mut self,
        direction: Direction,
        page: Page<I>,
        sample: ReaderSample,
    ) -> Landing {
        self.pending = None;
        debug_assert!(direction != Direction::Around, "use apply_around");
        let known: HashSet<&I> = self.items().collect();
        let fresh: Vec<I> = page
            .items
            .iter()
            .filter(|item| !known.contains(item))
            .cloned()
            .collect();
        if fresh.is_empty() {
            return Landing {
                commits: Vec::new(),
                restore: None,
            };
        }
        let inserted = fresh.len();
        let old_rows = self.row_count();
        let old_lead = self.first_item_row();
        let old_trailing_row = old_rows.saturating_sub(1);
        let mut commits = Vec::new();
        let mut restore = None;

        match direction {
            Direction::Previous => {
                self.pages.insert(0, fresh);
                self.previous_cursor = page.previous_cursor;
                // The leading edge row is replaced by the new items when no older history
                // remains; otherwise the items are inserted after it.
                let removed_edge = old_lead == 1 && !self.has_leading_edge();
                let start = if removed_edge { 0 } else { old_lead };
                commits.push(HistoryCommit {
                    old_range: start..old_lead,
                    new_count: inserted,
                });
                // Eviction: keep the first MAX_PAGES pages.
                if self.pages.len() > MAX_PAGES {
                    let evicted: usize = self.pages[MAX_PAGES..].iter().map(Vec::len).sum();
                    self.pages.truncate(MAX_PAGES);
                    let tail_row = self.first_item_row() + self.item_count();
                    let had_trailing = self.has_trailing_edge();
                    let last = self.items().last().cloned();
                    self.next_cursor = last;
                    // The evicted rows (and the old trailing edge row, if any) become one
                    // trailing edge row: a removal at the far end.
                    let old_end = tail_row + evicted + usize::from(had_trailing);
                    commits.push(HistoryCommit {
                        old_range: tail_row..old_end,
                        new_count: 1,
                    });
                }
                if sample.index == 0 && old_lead == 1 {
                    restore = Some(Restore {
                        index: self.first_item_row() + inserted,
                        offset: sample.offset.max(0.0) - EDGE_ROW_HEIGHT,
                    });
                }
            }
            Direction::Next => {
                let old_trailing = self.has_trailing_edge();
                self.pages.push(fresh);
                self.next_cursor = page.next_cursor;
                let removed_edge = old_trailing && !self.has_trailing_edge();
                let insert_at = old_rows - usize::from(old_trailing);
                commits.push(HistoryCommit {
                    old_range: insert_at..insert_at + usize::from(removed_edge),
                    new_count: inserted,
                });
                // The first appended item's index counts rows before eviction.
                let mut first_appended_item = self.item_count() - inserted;
                if self.pages.len() > MAX_PAGES {
                    let evicted: usize = self.pages[..self.pages.len() - MAX_PAGES]
                        .iter()
                        .map(Vec::len)
                        .sum();
                    let drop_pages = self.pages.len() - MAX_PAGES;
                    self.pages.drain(..drop_pages);
                    first_appended_item -= evicted;
                    let had_leading = self.has_leading_edge();
                    let first = self.items().next().cloned();
                    self.previous_cursor = first;
                    // The evicted rows (and the old leading edge row, if any) become one
                    // leading edge row: a removal at the far end.
                    commits.push(HistoryCommit {
                        old_range: 0..evicted + usize::from(had_leading),
                        new_count: 1,
                    });
                }
                if old_trailing && sample.index >= old_trailing_row {
                    let offset = if sample.index > old_trailing_row {
                        EDGE_ROW_HEIGHT - sample.viewport
                    } else {
                        sample.offset
                    };
                    restore = Some(Restore {
                        index: first_appended_item + self.first_item_row(),
                        offset,
                    });
                }
            }
            Direction::Around => unreachable!("handled by apply_around"),
        }
        Landing { commits, restore }
    }

    /// Applies the page of a jump to an unloaded item (R28): the cache is replaced with that
    /// single page in one synchronous commit. Returns the commit and the row to scroll to: the
    /// target's index plus one when a leading edge row exists.
    pub fn apply_around(&mut self, target: &I, page: Page<I>) -> (HistoryCommit, Option<usize>) {
        self.pending = None;
        let old_rows = self.row_count();
        self.pages = if page.items.is_empty() {
            Vec::new()
        } else {
            vec![page.items]
        };
        self.previous_cursor = page.previous_cursor;
        self.next_cursor = page.next_cursor;
        let commit = HistoryCommit {
            old_range: 0..old_rows,
            new_count: self.row_count(),
        };
        (commit, self.row_of(target))
    }

    /// Applies commits to the list synchronously, in order, then performs the restoration. The
    /// list's rows must be rendered from [`History::rows`] after this returns.
    pub fn apply_to_list(&self, list: &ListState, landing: &Landing) {
        for commit in &landing.commits {
            list.splice(commit.old_range.clone(), commit.new_count);
        }
        if let Some(restore) = landing.restore {
            list.scroll_to(ListOffset {
                item_ix: restore.index,
                offset_in_item: px(restore.offset),
            });
        }
    }
}

/// Parses an item link of the forms `item:<id>` (the only accepted form); anything else is
/// invalid and ignored by the jump (R28).
pub fn parse_item_link(link: &str) -> Option<usize> {
    let id = link.strip_prefix("item:")?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse().ok()
}

/// The page start for an around request (R22): `clamp(target - floor(page_size / 2), 0, total -
/// page_size)`.
pub fn around_start(target: usize, page_size: usize, total: usize) -> usize {
    let max_start = total.saturating_sub(page_size);
    target.saturating_sub(page_size / 2).min(max_start)
}

#[cfg(test)]
#[path = "../../tests/support/sample_service.rs"]
mod sample_service;

#[cfg(test)]
mod tests {
    use super::sample_service::SampleService;
    use super::*;

    fn service() -> SampleService {
        SampleService::new(7)
    }

    fn mounted() -> History<usize> {
        let page = service().initial_page(DEFAULT_PAGE_SIZE, None);
        History::mount(DEFAULT_PAGE_SIZE, page)
    }

    #[test]
    fn a8_one_page_mount_with_the_default_fixture() {
        let history = mounted();
        assert_eq!(history.page_count(), 1);
        assert_eq!(history.item_count(), 12);
        assert_eq!(
            history.items().copied().collect::<Vec<_>>(),
            (388..400).collect::<Vec<_>>()
        );
        // Older history exists, newer does not.
        assert!(history.has_leading_edge() && !history.has_trailing_edge());
        assert_eq!(history.rows().first(), Some(&HistoryRow::LeadingEdge));
        assert_eq!(history.row_count(), 13);
        // Deterministic variable heights.
        assert_eq!(service().height(5), service().height(5));
        let heights: HashSet<u32> = (0..50).map(|i| service().height(i) as u32).collect();
        assert!(heights.len() > 5);
        assert_ne!(
            SampleService::new(1).height(3),
            SampleService::new(2).height(3)
        );
    }

    #[test]
    fn around_requests_center_the_target() {
        assert_eq!(around_start(200, 12, 400), 194);
        assert_eq!(around_start(2, 12, 400), 0);
        assert_eq!(around_start(399, 12, 400), 388);
        let page = service().initial_page(12, Some(200));
        assert_eq!(page.items.first(), Some(&194));
        assert_eq!(page.previous_cursor, Some(194));
        assert_eq!(page.next_cursor, Some(205));
        let first = service().initial_page(12, Some(0));
        assert_eq!(first.previous_cursor, None);
    }

    #[test]
    fn cursors_are_exclusive() {
        let s = service();
        let prev = s.fetch_page(
            12,
            &Request {
                direction: Direction::Previous,
                cursor: Some(388),
            },
        );
        assert_eq!(prev.items, (376..388).collect::<Vec<_>>());
        let next = s.fetch_page(
            12,
            &Request {
                direction: Direction::Next,
                cursor: Some(205),
            },
        );
        assert_eq!(next.items, (206..218).collect::<Vec<_>>());
        assert_eq!(next.previous_cursor, Some(206));
    }

    #[test]
    fn a8_edge_rows_trigger_requests_only_when_visible_leading_wins_and_pending_blocks() {
        let mut history = mounted();
        // Off screen: no prefetch.
        assert_eq!(history.on_visible_range(3..13), None);
        // The leading edge row is visible: request, cursor-exclusive.
        let request = history.on_visible_range(0..6).expect("request");
        assert_eq!(request.direction, Direction::Previous);
        assert_eq!(request.cursor, Some(388));
        assert!(history.is_loading(Direction::Previous));
        // No second request while one is pending (repeated reports included).
        assert_eq!(history.on_visible_range(0..6), None);
        assert_eq!(history.begin_jump(7), None);
        // Rows and identities are retained while loading.
        assert_eq!(history.row_count(), 13);

        // With both edges visible, the leading one wins.
        let mut both = History::mount(
            12,
            Page {
                items: (100..112).collect(),
                previous_cursor: Some(100),
                next_cursor: Some(111),
            },
        );
        let request = both.on_visible_range(0..14).expect("request");
        assert_eq!(request.direction, Direction::Previous);
        both.cancel_pending();
        let request = both.on_visible_range(8..14).expect("request");
        assert_eq!(request.direction, Direction::Next);
    }

    #[test]
    fn a9_older_page_lands_with_the_leading_row_anchor_restored_exactly() {
        let mut history = mounted();
        let request = history.on_visible_range(0..4).unwrap();
        let page = service().fetch_page(12, &request);
        let sample = ReaderSample {
            index: 0,
            offset: 40.0,
            viewport: 500.0,
        };
        let landing = history.apply_page(Direction::Previous, page, sample);
        // One pure insertion after the (retained) leading edge row.
        assert_eq!(
            landing.commits,
            vec![HistoryCommit {
                old_range: 1..1,
                new_count: 12
            }]
        );
        assert_eq!(history.page_count(), 2);
        // Old first item: new index 1 + 12, offset 40 - 160 = -120.
        assert_eq!(
            landing.restore,
            Some(Restore {
                index: 13,
                offset: -120.0
            })
        );
        assert_eq!(history.row_of(&388), Some(13));
    }

    #[test]
    fn a9_a_retained_item_anchor_needs_no_forced_scroll() {
        let mut history = mounted();
        let page = service().fetch_page(
            12,
            &Request {
                direction: Direction::Previous,
                cursor: Some(388),
            },
        );
        let sample = ReaderSample {
            index: 5,
            offset: 12.0,
            viewport: 500.0,
        };
        let landing = history.apply_page(Direction::Previous, page, sample);
        assert_eq!(landing.restore, None);
    }

    fn mid_history() -> History<usize> {
        History::mount(12, service().initial_page(12, Some(200)))
    }

    #[test]
    fn a9_newer_page_lands_at_the_trailing_row_sentinel_and_inside_it() {
        // At-end sentinel: anchor index beyond the trailing row; viewport 500.
        let mut history = mid_history();
        let trailing = history.row_count() - 1;
        let page = service().fetch_page(
            12,
            &Request {
                direction: Direction::Next,
                cursor: Some(205),
            },
        );
        let landing = history.apply_page(
            Direction::Next,
            page.clone(),
            ReaderSample {
                index: trailing + 1,
                offset: 0.0,
                viewport: 500.0,
            },
        );
        // First appended item takes the empty row's place: leading edge + 12 old items.
        let expected_index = 1 + 12;
        assert_eq!(
            landing.restore,
            Some(Restore {
                index: expected_index,
                offset: 160.0 - 500.0
            })
        );
        assert_eq!(history.row_of(&206), Some(expected_index));
        // The new items go before the (retained) trailing edge row: a pure insertion.
        assert_eq!(
            landing.commits,
            vec![HistoryCommit {
                old_range: 13..13,
                new_count: 12
            }]
        );

        // A viewport shorter than the row: a positive offset.
        let mut history = mid_history();
        let trailing = history.row_count() - 1;
        let landing = history.apply_page(
            Direction::Next,
            page.clone(),
            ReaderSample {
                index: trailing + 1,
                offset: 0.0,
                viewport: 100.0,
            },
        );
        assert_eq!(landing.restore.unwrap().offset, 60.0);

        // Inside the trailing row at offset 30: the within-row offset is kept.
        let mut history = mid_history();
        let trailing = history.row_count() - 1;
        let landing = history.apply_page(
            Direction::Next,
            page,
            ReaderSample {
                index: trailing,
                offset: 30.0,
                viewport: 500.0,
            },
        );
        assert_eq!(landing.restore.unwrap().offset, 30.0);
    }

    #[test]
    fn a9_settled_pages_never_exceed_five_and_eviction_is_a_separate_second_commit() {
        let mut history = mounted();
        let s = service();
        let mut cursor = 388;
        for round in 0..8 {
            history.on_visible_range(0..3).expect("request");
            let page = s.fetch_page(
                12,
                &Request {
                    direction: Direction::Previous,
                    cursor: Some(cursor),
                },
            );
            cursor = page.items[0];
            let landing = history.apply_page(
                Direction::Previous,
                page,
                ReaderSample {
                    index: 4,
                    offset: 0.0,
                    viewport: 500.0,
                },
            );
            assert!(history.page_count() <= MAX_PAGES, "round {round}");
            if round >= 4 {
                assert_eq!(landing.commits.len(), 2, "insert then evict, round {round}");
                // The eviction only removes rows at the far end: a pure suffix replacement.
                let evict = &landing.commits[1];
                assert_eq!(evict.new_count, 1);
                assert!(evict.old_range.start > landing.commits[0].old_range.start);
                // The newer edge row now exists because newer history was evicted.
                assert!(history.has_trailing_edge());
            } else {
                assert_eq!(landing.commits.len(), 1, "round {round}");
            }
        }
        assert_eq!(history.page_count(), MAX_PAGES);
    }

    #[test]
    fn next_pages_evict_the_head_and_restore_indices_after_both_commits() {
        let mut history = mid_history();
        let s = service();
        let mut cursor = 205;
        let mut last = None;
        for _ in 0..6 {
            history
                .on_visible_range(history.row_count() - 2..history.row_count())
                .expect("request");
            let page = s.fetch_page(
                12,
                &Request {
                    direction: Direction::Next,
                    cursor: Some(cursor),
                },
            );
            cursor = *page.items.last().unwrap();
            let trailing = history.row_count() - 1;
            let landing = history.apply_page(
                Direction::Next,
                page,
                ReaderSample {
                    index: trailing + 1,
                    offset: 0.0,
                    viewport: 300.0,
                },
            );
            assert!(history.page_count() <= MAX_PAGES);
            last = Some(landing);
        }
        let landing = last.unwrap();
        assert_eq!(landing.commits.len(), 2);
        let restore = landing.restore.unwrap();
        // The restore index names the first appended item in the settled list.
        let first_appended = cursor - 11;
        assert_eq!(history.row_of(&first_appended), Some(restore.index));
        assert!(history.has_leading_edge());
    }

    #[test]
    fn duplicates_are_dropped_and_an_empty_page_changes_nothing() {
        let mut history = mounted();
        history.on_visible_range(0..2).unwrap();
        let page = Page {
            items: vec![390, 391],
            previous_cursor: Some(388),
            next_cursor: None,
        };
        let landing = history.apply_page(
            Direction::Previous,
            page,
            ReaderSample {
                index: 0,
                offset: 0.0,
                viewport: 400.0,
            },
        );
        assert!(landing.commits.is_empty() && landing.restore.is_none());
        assert_eq!(history.item_count(), 12);
        assert!(history.pending().is_none(), "the request is settled");
    }

    #[test]
    fn a10_a_jump_replaces_the_cache_with_one_page_and_names_the_target_row() {
        let mut history = mounted();
        let request = history.begin_jump(200).expect("request");
        assert_eq!(request.direction, Direction::Around);
        // A request during a pending fetch is ignored.
        assert_eq!(history.begin_jump(10), None);
        assert_eq!(history.on_visible_range(0..4), None);
        let page = service().fetch_page(12, &request);
        let (commit, row) = history.apply_around(&200, page);
        assert_eq!(commit.old_range, 0..13);
        assert_eq!(history.page_count(), 1);
        assert_eq!(commit.new_count, history.row_count());
        // Page 194..206 below a leading edge row: row = 1 + (200 - 194).
        assert_eq!(row, Some(1 + 6));
        assert!(history.has_leading_edge() && history.has_trailing_edge());
    }

    #[test]
    fn a10_only_item_links_of_the_accepted_form_are_valid() {
        assert_eq!(parse_item_link("item:200"), Some(200));
        for bad in [
            "", "item:", "item:-3", "item:abc", "msg:4", "item:4:5", "item: 4", "ITEM:4",
        ] {
            assert_eq!(parse_item_link(bad), None, "{bad}");
        }
    }
}
