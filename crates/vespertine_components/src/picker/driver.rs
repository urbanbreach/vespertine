//! The entity that runs a [`SourceSet`] on the executor (CMP-006 R10, R11, R15).
//!
//! [`PickerSources`] owns the in-flight request tasks and the one timer that serves the
//! debounce and the indexing refresh deadlines of the set. Every answer is settled through the
//! set's generation check, so an older answer can never overwrite newer results whatever order
//! the answers arrive in; clearing or superseding a source drops (cancels) its task.

use super::sources::{Answer, Effect, Generation, SourcePolicy, SourceSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use vespertine::{App, Context, EventEmitter, Task};

/// A request handed to a source's fetcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceQuery {
    /// The query to search.
    pub query: String,
    /// The page: 0 for the first answer, then 1, 2, ...
    pub page: usize,
    /// The generation the answer will be settled against.
    pub generation: Generation,
}

/// Starts a source's request. The returned task is dropped (cancelled) when the source is
/// cleared or superseded.
pub type Fetcher<R> = Rc<dyn Fn(SourceQuery, &mut App) -> Task<Result<Answer<R>, String>>>;

/// Emitted whenever rows, statuses or flags changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourcesChanged;

/// The sources of one picker bound to the executor.
pub struct PickerSources<R: 'static> {
    set: SourceSet<R>,
    fetchers: Vec<Fetcher<R>>,
    in_flight: Vec<Option<Task<()>>>,
    deadline: Option<Task<()>>,
    origin: Instant,
}

impl<R: 'static> EventEmitter<SourcesChanged> for PickerSources<R> {}

impl<R: 'static> PickerSources<R> {
    /// One source per `(policy, fetcher)` pair, in declared order.
    pub fn new(sources: Vec<(SourcePolicy, Fetcher<R>)>, cx: &mut Context<Self>) -> Self {
        let (policies, fetchers): (Vec<_>, Vec<_>) = sources.into_iter().unzip();
        Self {
            in_flight: (0..fetchers.len()).map(|_| None).collect(),
            set: SourceSet::new(policies),
            fetchers,
            deadline: None,
            origin: cx.background_executor().now(),
        }
    }

    /// The set: rows, statuses, errors, paging flags.
    pub fn set(&self) -> &SourceSet<R> {
        &self.set
    }

    fn now(&self, cx: &App) -> Duration {
        cx.background_executor().now().saturating_duration_since(self.origin)
    }

    /// Opening: the default (empty-query) requests start immediately.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        let now = self.now(cx);
        let effects = self.set.open(now);
        self.run(effects, cx);
    }

    /// A query (or scope) change: every source begins a new generation.
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        let now = self.now(cx);
        let effects = self.set.set_query(now, query);
        self.run(effects, cx);
    }

    /// Closing: clears every source, cancels in-flight requests and empties the rows.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let effects = self.set.clear_all();
        self.run(effects, cx);
    }

    /// Retry after an error.
    pub fn retry(&mut self, source: usize, cx: &mut Context<Self>) {
        let effects = self.set.retry(source);
        self.run(effects, cx);
    }

    /// The list reports it is scrolled to its end.
    pub fn scrolled_to_end(&mut self, cx: &mut Context<Self>) {
        let effects = self.set.scrolled_to_end();
        self.run(effects, cx);
    }

    /// Retry after a failed page request.
    pub fn retry_page(&mut self, source: usize, cx: &mut Context<Self>) {
        let effects = self.set.retry_page(source);
        self.run(effects, cx);
    }

    fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Cancel { source } => self.in_flight[source] = None,
                Effect::Request {
                    source,
                    generation,
                    query,
                    page,
                } => {
                    let request = SourceQuery {
                        query,
                        page,
                        generation,
                    };
                    let fetch = self.fetchers[source].clone();
                    let answer = fetch(request, cx);
                    self.in_flight[source] = Some(cx.spawn(async move |this, cx| {
                        let result = answer.await;
                        this.update(cx, |this, cx| {
                            this.settle(source, generation, page, result, cx)
                        })
                        .ok();
                    }));
                }
            }
        }
        self.arm_deadline(cx);
        cx.emit(SourcesChanged);
        cx.notify();
    }

    fn settle(
        &mut self,
        source: usize,
        generation: Generation,
        page: usize,
        result: Result<Answer<R>, String>,
        cx: &mut Context<Self>,
    ) {
        let now = self.now(cx);
        let settled = if page == 0 {
            self.set.answer(now, source, generation, result)
        } else {
            self.set.page_answer(source, generation, result)
        };
        if settled {
            self.in_flight[source] = None;
            self.arm_deadline(cx);
            cx.emit(SourcesChanged);
            cx.notify();
        }
    }

    /// One timer serves the earliest debounce or indexing-refresh deadline.
    fn arm_deadline(&mut self, cx: &mut Context<Self>) {
        self.deadline = None;
        let Some(due) = self.set.next_deadline() else {
            return;
        };
        let wait = due.saturating_sub(self.now(cx));
        self.deadline = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                let now = this.now(cx);
                let effects = this.set.tick(now);
                this.run(effects, cx);
            })
            .ok();
        }));
    }
}
