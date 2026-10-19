//! Asynchronous result sources protected by request generations (CMP-006 R10, R11, R15).
//!
//! A [`SourceSet`] is a pure scheduler. The owner feeds it the query and answers together with
//! a monotonic `now` and executes the returned [`Effect`]s (send a request, cancel one); the set
//! owns the generation counters, the typing debounce, the minimum query length, the indexing
//! refresh backoff and the paging bookkeeping, so none of those rules depends on a clock or an
//! executor and every one is testable with a fake clock.

use std::time::Duration;

/// A request generation. Every answer carries the generation of the request it settles.
pub type Generation = u64;

/// Debounce of content-like sources (R11).
pub const CONTENT_DEBOUNCE: Duration = Duration::from_millis(80);
/// Minimum non-empty query length of a source (R11).
pub const MIN_QUERY_CHARS: usize = 2;
/// First indexing refresh wait (R11).
pub const INDEXING_REFRESH_INITIAL: Duration = Duration::from_millis(500);
/// Largest indexing refresh wait (R11).
pub const INDEXING_REFRESH_MAX: Duration = Duration::from_secs(2);
/// Status text of a source with a request in flight (R11).
pub const SEARCHING_STATUS: &str = "Searching\u{2026}";
/// Status text of a source that is still indexing (R11).
pub const INDEXING_STATUS: &str = "Indexing\u{2026}";

/// How a source reacts to typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// Searches at once on every query change.
    NameLike,
    /// Waits for typing to pause (the policy's debounce).
    ContentLike,
}

/// Owner-configurable policy of one source; the constructors carry the defaults of R11 and R12.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourcePolicy {
    /// Name-like or content-like.
    pub kind: SourceKind,
    /// Quiet time after the last edit before a request is sent.
    pub debounce: Duration,
    /// Shortest non-empty query that is sent; shorter queries clear the source.
    pub min_query_chars: usize,
    /// Whether the empty query is sent (the source's default "recent" list).
    pub allow_empty_query: bool,
    /// Rows shown per section in the summary view (default 5).
    pub summary_limit: usize,
    /// Rows shown per section in a dedicated scope (default 50, 100 for content-like sources).
    pub scope_limit: usize,
    /// Matches shown per file for content hits (default 3); applied by the owner's row mapping.
    pub matches_per_file: usize,
}

impl SourcePolicy {
    /// A name-like source: immediate, 50 rows in a dedicated scope.
    pub fn name_like() -> Self {
        Self {
            kind: SourceKind::NameLike,
            debounce: Duration::ZERO,
            min_query_chars: MIN_QUERY_CHARS,
            allow_empty_query: true,
            summary_limit: 5,
            scope_limit: 50,
            matches_per_file: 3,
        }
    }

    /// A content-like source: 80 ms debounce, 100 rows in a dedicated scope, no empty query.
    pub fn content_like() -> Self {
        Self {
            kind: SourceKind::ContentLike,
            debounce: CONTENT_DEBOUNCE,
            allow_empty_query: false,
            scope_limit: 100,
            ..Self::name_like()
        }
    }

    /// Whether `query` is sent to the source (R11): the empty query only when allowed, any other
    /// query once it has at least `min_query_chars` characters.
    pub fn accepts(&self, query: &str) -> bool {
        let chars = query.chars().count();
        if chars == 0 {
            self.allow_empty_query
        } else {
            chars >= self.min_query_chars
        }
    }
}

/// A successful answer (or page) of a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer<R> {
    /// The rows of this answer (or page).
    pub rows: Vec<R>,
    /// The total number of matches when the source knows it (`Showing X of Y`).
    pub total: Option<usize>,
    /// The source is still indexing: the rows are partial and are refreshed (R11).
    pub indexing: bool,
    /// More pages can be requested (R15).
    pub has_more: bool,
}

impl<R> Answer<R> {
    /// A complete answer: no indexing, no further pages.
    pub fn complete(rows: Vec<R>) -> Self {
        Self {
            rows,
            total: None,
            indexing: false,
            has_more: false,
        }
    }
}

/// Something the owner has to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Send a request. `page` is 0 for a fresh request and the page number for a page request.
    Request {
        /// Index of the source.
        source: usize,
        /// The generation the answer must carry.
        generation: Generation,
        /// The query to search.
        query: String,
        /// Page number, 0 for the first.
        page: usize,
    },
    /// Cancel the in-flight request of the source.
    Cancel {
        /// Index of the source.
        source: usize,
    },
}

struct Slot<R> {
    policy: SourcePolicy,
    generation: Generation,
    rows: Vec<R>,
    total: Option<usize>,
    loading: bool,
    indexing: bool,
    error: Option<String>,
    has_more: bool,
    next_page: usize,
    page_requested: bool,
    loading_more: bool,
    page_error: Option<String>,
    debounce_due: Option<Duration>,
    refresh_due: Option<Duration>,
    refreshes: u32,
    /// The query the rows and the in-flight request belong to.
    query: String,
    /// A request that was cut short by a newer generation or a clear and may need a cancel.
    in_flight: bool,
}

impl<R> Slot<R> {
    fn new(policy: SourcePolicy) -> Self {
        Self {
            policy,
            generation: 0,
            rows: Vec::new(),
            total: None,
            loading: false,
            indexing: false,
            error: None,
            has_more: false,
            next_page: 1,
            page_requested: false,
            loading_more: false,
            page_error: None,
            debounce_due: None,
            refresh_due: None,
            refreshes: 0,
            query: String::new(),
            in_flight: false,
        }
    }

    /// Drops everything that belongs to the previous request without touching the rows.
    fn supersede(&mut self, source: usize, effects: &mut Vec<Effect>) {
        self.generation += 1;
        self.debounce_due = None;
        self.refresh_due = None;
        self.refreshes = 0;
        self.page_requested = false;
        self.loading_more = false;
        self.page_error = None;
        self.has_more = false;
        if self.in_flight {
            self.in_flight = false;
            effects.push(Effect::Cancel { source });
        }
    }

    fn send(&mut self, source: usize, effects: &mut Vec<Effect>) {
        self.loading = true;
        self.in_flight = true;
        self.next_page = 1;
        effects.push(Effect::Request {
            source,
            generation: self.generation,
            query: self.query.clone(),
            page: 0,
        });
    }

    fn clear(&mut self, source: usize, effects: &mut Vec<Effect>) {
        self.supersede(source, effects);
        self.rows.clear();
        self.total = None;
        self.loading = false;
        self.indexing = false;
        self.error = None;
        self.has_more = false;
    }
}

/// The asynchronous sources of one picker and their generation counters.
pub struct SourceSet<R> {
    slots: Vec<Slot<R>>,
    query: String,
}

impl<R> SourceSet<R> {
    /// A set with one source per policy, in declared order.
    pub fn new(policies: impl IntoIterator<Item = SourcePolicy>) -> Self {
        Self {
            slots: policies.into_iter().map(Slot::new).collect(),
            query: String::new(),
        }
    }

    /// Number of sources.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether there are no sources.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The policy of a source.
    pub fn policy(&self, source: usize) -> &SourcePolicy {
        &self.slots[source].policy
    }

    /// The current query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The current generation of a source.
    pub fn generation(&self, source: usize) -> Generation {
        self.slots[source].generation
    }

    /// The rows of the last settled answer; they stay while a newer request is in flight.
    pub fn rows(&self, source: usize) -> &[R] {
        &self.slots[source].rows
    }

    /// The total reported by the last answer, when known.
    pub fn total(&self, source: usize) -> Option<usize> {
        self.slots[source].total
    }

    /// A request (or a debounce wait) is pending: the loading status shows (R11).
    pub fn is_loading(&self, source: usize) -> bool {
        let slot = &self.slots[source];
        slot.loading || slot.debounce_due.is_some()
    }

    /// The source reported it is still indexing.
    pub fn is_indexing(&self, source: usize) -> bool {
        self.slots[source].indexing
    }

    /// The error message of the current generation, if its answer failed.
    pub fn error(&self, source: usize) -> Option<&str> {
        self.slots[source].error.as_deref()
    }

    /// More pages are available and none is outstanding or failed.
    pub fn has_more(&self, source: usize) -> bool {
        self.slots[source].has_more
    }

    /// A page request is in flight.
    pub fn is_loading_more(&self, source: usize) -> bool {
        self.slots[source].loading_more
    }

    /// The error of the last page request.
    pub fn page_error(&self, source: usize) -> Option<&str> {
        self.slots[source].page_error.as_deref()
    }

    /// The status text of a source: searching, indexing or none.
    pub fn status(&self, source: usize) -> Option<&'static str> {
        if self.is_loading(source) {
            Some(SEARCHING_STATUS)
        } else if self.is_indexing(source) {
            Some(INDEXING_STATUS)
        } else {
            None
        }
    }

    /// A query change (or scope change): every source begins a new generation; sources that
    /// accept the query send a request (after their debounce), the others are cleared (R10, R11).
    pub fn set_query(&mut self, now: Duration, query: &str) -> Vec<Effect> {
        self.begin(now, query, false)
    }

    /// Opening the palette: the default (empty-query) requests start immediately (R10).
    pub fn open(&mut self, now: Duration) -> Vec<Effect> {
        self.begin(now, "", true)
    }

    fn begin(&mut self, now: Duration, query: &str, immediate: bool) -> Vec<Effect> {
        self.query = query.to_string();
        let mut effects = Vec::new();
        for (source, slot) in self.slots.iter_mut().enumerate() {
            if !slot.policy.accepts(query) {
                slot.clear(source, &mut effects);
                continue;
            }
            slot.supersede(source, &mut effects);
            slot.error = None;
            slot.query = query.to_string();
            if immediate || slot.policy.debounce.is_zero() {
                slot.send(source, &mut effects);
            } else {
                slot.debounce_due = Some(now + slot.policy.debounce);
            }
        }
        effects
    }

    /// Clears every source: the palette closed (R10). Generations advance, in-flight requests are
    /// cancelled and the rows are emptied.
    pub fn clear_all(&mut self) -> Vec<Effect> {
        self.query.clear();
        let mut effects = Vec::new();
        for (source, slot) in self.slots.iter_mut().enumerate() {
            slot.clear(source, &mut effects);
        }
        effects
    }

    /// The earliest pending deadline (a debounce or an indexing refresh), if any.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.slots
            .iter()
            .flat_map(|slot| [slot.debounce_due, slot.refresh_due])
            .flatten()
            .min()
    }

    /// Fires every deadline that is due at `now`: debounced requests and indexing refreshes.
    pub fn tick(&mut self, now: Duration) -> Vec<Effect> {
        let mut effects = Vec::new();
        for (source, slot) in self.slots.iter_mut().enumerate() {
            if slot.debounce_due.is_some_and(|due| due <= now) {
                slot.debounce_due = None;
                slot.send(source, &mut effects);
            }
            if slot.refresh_due.is_some_and(|due| due <= now) {
                // A refresh is a new request of the same query: a new generation, the partial
                // rows stay displayed until the answer arrives (R10, R11).
                let refreshes = slot.refreshes;
                slot.refresh_due = None;
                slot.generation += 1;
                slot.send(source, &mut effects);
                slot.refreshes = refreshes;
            }
        }
        effects
    }

    /// An answer arrived. It settles only when `generation` equals the source's current one and
    /// returns whether it did (R10). Rows are replaced; an error empties them.
    pub fn answer(
        &mut self,
        now: Duration,
        source: usize,
        generation: Generation,
        result: Result<Answer<R>, String>,
    ) -> bool {
        let slot = &mut self.slots[source];
        if slot.generation != generation || !slot.in_flight {
            return false;
        }
        slot.in_flight = false;
        slot.loading = false;
        match result {
            Ok(answer) => {
                slot.rows = answer.rows;
                slot.total = answer.total;
                slot.has_more = answer.has_more;
                slot.error = None;
                slot.indexing = answer.indexing;
                if answer.indexing {
                    let wait = (INDEXING_REFRESH_INITIAL * 2u32.saturating_pow(slot.refreshes))
                        .min(INDEXING_REFRESH_MAX);
                    slot.refreshes = slot.refreshes.saturating_add(1);
                    slot.refresh_due = Some(now + wait);
                } else {
                    slot.refreshes = 0;
                    slot.refresh_due = None;
                }
            }
            Err(message) => {
                slot.rows.clear();
                slot.total = None;
                slot.has_more = false;
                slot.indexing = false;
                slot.refresh_due = None;
                slot.error = Some(message);
            }
        }
        true
    }

    /// Retry after an error: begins a new request, a new generation (R10).
    pub fn retry(&mut self, source: usize) -> Vec<Effect> {
        let mut effects = Vec::new();
        let slot = &mut self.slots[source];
        slot.supersede(source, &mut effects);
        slot.error = None;
        slot.send(source, &mut effects);
        effects
    }

    /// The list reports it is scrolled to its end: requests the next page of the first source
    /// that has one, at most one outstanding page request per generation (R15).
    pub fn scrolled_to_end(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        for (source, slot) in self.slots.iter_mut().enumerate() {
            if slot.has_more && !slot.page_requested && slot.error.is_none() && !slot.loading {
                slot.page_requested = true;
                slot.loading_more = true;
                slot.page_error = None;
                effects.push(Effect::Request {
                    source,
                    generation: slot.generation,
                    query: slot.query.clone(),
                    page: slot.next_page,
                });
                break;
            }
        }
        effects
    }

    /// A page answer arrived; dropped when a newer generation began. Appends the rows.
    pub fn page_answer(
        &mut self,
        source: usize,
        generation: Generation,
        result: Result<Answer<R>, String>,
    ) -> bool {
        let slot = &mut self.slots[source];
        if slot.generation != generation || !slot.loading_more {
            return false;
        }
        slot.loading_more = false;
        match result {
            Ok(answer) => {
                slot.rows.extend(answer.rows);
                if answer.total.is_some() {
                    slot.total = answer.total;
                }
                slot.has_more = answer.has_more;
                slot.next_page += 1;
                slot.page_requested = false;
            }
            Err(message) => slot.page_error = Some(message),
        }
        true
    }

    /// Retry after a page error: the page is requested again (still one outstanding).
    pub fn retry_page(&mut self, source: usize) -> Vec<Effect> {
        let slot = &mut self.slots[source];
        if slot.page_error.is_none() || slot.loading_more {
            return Vec::new();
        }
        slot.page_error = None;
        slot.loading_more = true;
        vec![Effect::Request {
            source,
            generation: slot.generation,
            query: slot.query.clone(),
            page: slot.next_page,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn requests(effects: &[Effect]) -> Vec<(usize, Generation, String, usize)> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Request {
                    source,
                    generation,
                    query,
                    page,
                } => Some((*source, *generation, query.clone(), *page)),
                Effect::Cancel { .. } => None,
            })
            .collect()
    }

    fn rows(values: &[&str]) -> Answer<String> {
        Answer::complete(values.iter().map(|value| value.to_string()).collect())
    }

    #[test]
    fn older_answers_never_overwrite_newer_results_in_either_order() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let first = requests(&set.set_query(ms(0), "ab"));
        let second = requests(&set.set_query(ms(10), "abc"));
        assert_eq!(first[0].1 + 1, second[0].1);
        // The newer answer first, then the stale one.
        assert!(set.answer(ms(20), 0, second[0].1, Ok(rows(&["new"]))));
        assert!(!set.answer(ms(30), 0, first[0].1, Ok(rows(&["old"]))));
        assert_eq!(set.rows(0), ["new".to_string()]);

        // And the other arrival order: the stale answer comes first and is dropped.
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let first = requests(&set.set_query(ms(0), "ab"));
        let second = requests(&set.set_query(ms(10), "abc"));
        assert!(!set.answer(ms(20), 0, first[0].1, Ok(rows(&["old"]))));
        assert!(set.answer(ms(30), 0, second[0].1, Ok(rows(&["new"]))));
        assert_eq!(set.rows(0), ["new".to_string()]);
    }

    #[test]
    fn rows_of_the_last_answer_stay_while_a_newer_request_is_in_flight() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let first = requests(&set.set_query(ms(0), "ab"));
        set.answer(ms(5), 0, first[0].1, Ok(rows(&["one"])));
        let effects = set.set_query(ms(10), "abc");
        assert_eq!(set.rows(0), ["one".to_string()]);
        assert!(set.is_loading(0));
        assert_eq!(set.status(0), Some(SEARCHING_STATUS));
        assert_eq!(requests(&effects).len(), 1);
    }

    #[test]
    fn a_failed_answer_empties_the_rows_and_retry_begins_a_new_generation() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let first = requests(&set.set_query(ms(0), "ab"));
        set.answer(ms(5), 0, first[0].1, Ok(rows(&["one"])));
        let second = requests(&set.set_query(ms(10), "abc"));
        assert!(set.answer(ms(20), 0, second[0].1, Err("boom".into())));
        assert!(set.rows(0).is_empty());
        assert_eq!(set.error(0), Some("boom"));
        let retry = requests(&set.retry(0));
        assert_eq!(retry[0].1, second[0].1 + 1);
        assert_eq!(retry[0].2, "abc");
        assert!(set.error(0).is_none());
        assert!(set.answer(ms(30), 0, retry[0].1, Ok(rows(&["fixed"]))));
        assert_eq!(set.rows(0), ["fixed".to_string()]);
    }

    #[test]
    fn a_stale_failure_is_dropped() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let first = requests(&set.set_query(ms(0), "ab"));
        set.set_query(ms(1), "abc");
        assert!(!set.answer(ms(2), 0, first[0].1, Err("late".into())));
        assert!(set.error(0).is_none());
    }

    #[test]
    fn content_like_sources_debounce_and_send_one_request_for_close_edits() {
        let mut set = SourceSet::<String>::new([SourcePolicy::content_like()]);
        assert!(set.set_query(ms(0), "ab").is_empty());
        assert!(set.is_loading(0), "waiting for the pause shows the status");
        assert!(set.set_query(ms(50), "abc").is_empty());
        assert!(set.set_query(ms(100), "abcd").is_empty());
        assert_eq!(set.next_deadline(), Some(ms(180)));
        assert!(set.tick(ms(179)).is_empty());
        let sent = requests(&set.tick(ms(180)));
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].2, "abcd");
        assert_eq!(sent[0].1, set.generation(0));
        assert!(set.tick(ms(1000)).is_empty());
    }

    #[test]
    fn short_queries_clear_and_send_nothing() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let first = requests(&set.set_query(ms(0), "ab"));
        set.answer(ms(1), 0, first[0].1, Ok(rows(&["one"])));
        let effects = set.set_query(ms(2), "a");
        assert!(requests(&effects).is_empty());
        assert!(set.rows(0).is_empty());
        assert!(!set.is_loading(0));
        // The empty query is sent only by sources that keep a default list.
        let mut content = SourceSet::<String>::new([SourcePolicy::content_like()]);
        assert!(content.set_query(ms(0), "").is_empty());
        assert!(!content.is_loading(0));
        let mut recent = SourceSet::<String>::new([SourcePolicy::name_like()]);
        assert_eq!(requests(&recent.set_query(ms(0), "")).len(), 1);
    }

    #[test]
    fn clearing_cancels_the_in_flight_request_and_advances_the_generation() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let first = requests(&set.set_query(ms(0), "ab"));
        let effects = set.clear_all();
        assert_eq!(effects, vec![Effect::Cancel { source: 0 }]);
        assert!(set.generation(0) > first[0].1);
        assert!(!set.answer(ms(1), 0, first[0].1, Ok(rows(&["late"]))));
        assert!(set.rows(0).is_empty());
    }

    #[test]
    fn opening_starts_the_default_requests_immediately() {
        let mut set = SourceSet::<String>::new([
            SourcePolicy::name_like(),
            SourcePolicy {
                allow_empty_query: true,
                ..SourcePolicy::content_like()
            },
        ]);
        let sent = requests(&set.open(ms(0)));
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|request| request.2.is_empty()));
    }

    #[test]
    fn indexing_sources_refresh_after_500_1000_2000_and_2000_ms() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let mut now = ms(0);
        let mut sent = requests(&set.set_query(now, "ab"));
        let mut waits = Vec::new();
        for _ in 0..4 {
            let partial = Answer {
                indexing: true,
                ..rows(&["partial"])
            };
            assert!(set.answer(now, 0, sent[0].1, Ok(partial)));
            assert_eq!(set.status(0), Some(INDEXING_STATUS));
            assert_eq!(set.rows(0), ["partial".to_string()]);
            let due = set.next_deadline().expect("a refresh is scheduled");
            waits.push(due - now);
            assert!(set.tick(due - ms(1)).is_empty());
            now = due;
            sent = requests(&set.tick(now));
            assert_eq!(sent.len(), 1, "the refresh is a new request");
        }
        assert_eq!(waits, vec![ms(500), ms(1000), ms(2000), ms(2000)]);
        // When the source finishes the refreshes stop and the status clears.
        assert!(set.answer(now, 0, sent[0].1, Ok(rows(&["done"]))));
        assert_eq!(set.status(0), None);
        assert_eq!(set.next_deadline(), None);
    }

    #[test]
    fn a_new_query_restarts_the_indexing_backoff() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let sent = requests(&set.set_query(ms(0), "ab"));
        let partial = Answer {
            indexing: true,
            ..rows(&["p"])
        };
        set.answer(ms(0), 0, sent[0].1, Ok(partial.clone()));
        let sent = requests(&set.tick(ms(500)));
        set.answer(ms(500), 0, sent[0].1, Ok(partial.clone()));
        assert_eq!(set.next_deadline(), Some(ms(1500)));
        let sent = requests(&set.set_query(ms(600), "abc"));
        set.answer(ms(600), 0, sent[0].1, Ok(partial));
        assert_eq!(set.next_deadline(), Some(ms(1100)));
    }

    #[test]
    fn exactly_one_page_request_per_generation_and_none_after_a_newer_one() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let sent = requests(&set.set_query(ms(0), "ab"));
        let first_page = Answer {
            has_more: true,
            total: Some(5),
            ..rows(&["a", "b"])
        };
        set.answer(ms(1), 0, sent[0].1, Ok(first_page));
        let page = requests(&set.scrolled_to_end());
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].3, 1);
        assert!(set.scrolled_to_end().is_empty(), "one outstanding request");

        // A newer generation begins: the late page answer is dropped and asks for nothing.
        let newer = requests(&set.set_query(ms(2), "abc"));
        assert!(!set.page_answer(0, page[0].1, Ok(rows(&["c"]))));
        assert!(
            set.scrolled_to_end().is_empty(),
            "nothing to page before an answer"
        );
        let answer = Answer {
            has_more: true,
            ..rows(&["x"])
        };
        set.answer(ms(3), 0, newer[0].1, Ok(answer));
        let page = requests(&set.scrolled_to_end());
        assert_eq!(page.len(), 1);
        assert!(set.page_answer(
            0,
            page[0].1,
            Ok(Answer {
                has_more: false,
                ..rows(&["y"])
            })
        ));
        assert_eq!(set.rows(0), ["x".to_string(), "y".to_string()]);
        assert!(set.scrolled_to_end().is_empty());
    }

    #[test]
    fn a_failed_page_shows_an_error_and_retries_once() {
        let mut set = SourceSet::<String>::new([SourcePolicy::name_like()]);
        let sent = requests(&set.set_query(ms(0), "ab"));
        set.answer(
            ms(1),
            0,
            sent[0].1,
            Ok(Answer {
                has_more: true,
                ..rows(&["a"])
            }),
        );
        let page = requests(&set.scrolled_to_end());
        assert!(set.is_loading_more(0));
        set.page_answer(0, page[0].1, Err("net".into()));
        assert_eq!(set.page_error(0), Some("net"));
        assert!(
            set.scrolled_to_end().is_empty(),
            "the error blocks silent retries"
        );
        let retry = requests(&set.retry_page(0));
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].3, 1);
    }
}
