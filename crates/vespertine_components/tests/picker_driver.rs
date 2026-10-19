//! CMP-006 A7, A8, A13: asynchronous sources protected by request generations, driven by the
//! executor with a fake clock.

use futures::channel::oneshot;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vespertine::{AppContext, Entity, TestAppContext};
use vespertine_components::picker::driver::{Fetcher, PickerSources, SourceQuery};
use vespertine_components::picker::sources::{Answer, SourcePolicy};

type Reply = oneshot::Sender<Result<Answer<String>, String>>;

#[derive(Default)]
struct Log {
    requests: Vec<SourceQuery>,
    replies: Vec<Option<Reply>>,
}

fn fetcher(log: &Rc<RefCell<Log>>) -> Fetcher<String> {
    let log = log.clone();
    Rc::new(move |request, cx| {
        let (tx, rx) = oneshot::channel();
        let mut log = log.borrow_mut();
        log.requests.push(request);
        log.replies.push(Some(tx));
        cx.background_spawn(async move { rx.await.unwrap_or_else(|_| Err("cancelled".into())) })
    })
}

fn answer(rows: &[&str]) -> Result<Answer<String>, String> {
    Ok(Answer::complete(rows.iter().map(|row| row.to_string()).collect()))
}

fn rows(sources: &Entity<PickerSources<String>>, cx: &mut TestAppContext) -> Vec<String> {
    sources.read_with(cx, |sources, _| sources.set().rows(0).to_vec())
}

fn setup(
    cx: &mut TestAppContext,
    policy: SourcePolicy,
) -> (Entity<PickerSources<String>>, Rc<RefCell<Log>>) {
    let log = Rc::new(RefCell::new(Log::default()));
    let f = fetcher(&log);
    let sources = cx.new(|cx| PickerSources::new(vec![(policy, f)], cx));
    (sources, log)
}

fn reply(log: &Rc<RefCell<Log>>, index: usize, result: Result<Answer<String>, String>) {
    let tx = log.borrow_mut().replies[index].take().expect("reply once");
    tx.send(result).ok();
}

#[vespertine::test]
fn answers_arriving_in_reverse_order_keep_the_newer_results_and_a_failure_retries(
    cx: &mut TestAppContext,
) {
    let (sources, log) = setup(cx, SourcePolicy::name_like());
    sources.update(cx, |s, cx| s.set_query("ab", cx));
    sources.update(cx, |s, cx| s.set_query("abc", cx));
    cx.run_until_parked();
    assert_eq!(log.borrow().requests.len(), 2);
    // The newer answer arrives first, then the stale one.
    reply(&log, 1, answer(&["abc-result"]));
    cx.run_until_parked();
    assert_eq!(rows(&sources, cx), ["abc-result"]);
    // The first request was cancelled when it was superseded (its task dropped); even if it had
    // answered, the generation check would drop it.
    let _ = log.borrow_mut().replies[0].take().map(|tx| tx.send(answer(&["stale"])));
    cx.run_until_parked();
    assert_eq!(rows(&sources, cx), ["abc-result"]);

    // A third query fails: the rows are replaced by the error and retry succeeds.
    sources.update(cx, |s, cx| s.set_query("abcd", cx));
    cx.run_until_parked();
    reply(&log, 2, Err("index offline".into()));
    cx.run_until_parked();
    sources.read_with(cx, |s, _| {
        assert_eq!(s.set().error(0), Some("index offline"));
        assert!(s.set().rows(0).is_empty());
    });
    let generation_before = sources.read_with(cx, |s, _| s.set().generation(0));
    sources.update(cx, |s, cx| s.retry(0, cx));
    cx.run_until_parked();
    assert_eq!(log.borrow().requests.len(), 4);
    assert!(sources.read_with(cx, |s, _| s.set().generation(0)) > generation_before);
    reply(&log, 3, answer(&["recovered"]));
    cx.run_until_parked();
    assert_eq!(rows(&sources, cx), ["recovered"]);
    sources.read_with(cx, |s, _| assert!(s.set().error(0).is_none()));
}

#[vespertine::test]
fn a_content_like_source_sends_one_request_for_edits_closer_than_the_debounce(
    cx: &mut TestAppContext,
) {
    let (sources, log) = setup(cx, SourcePolicy::content_like());
    sources.update(cx, |s, cx| s.set_query("ab", cx));
    cx.executor().advance_clock(Duration::from_millis(50));
    sources.update(cx, |s, cx| s.set_query("abc", cx));
    cx.executor().advance_clock(Duration::from_millis(79));
    cx.run_until_parked();
    assert!(log.borrow().requests.is_empty(), "still inside the pause");
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    let requests = log.borrow().requests.clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].query, "abc");
}

#[vespertine::test]
fn a_one_character_query_clears_and_sends_nothing(cx: &mut TestAppContext) {
    let (sources, log) = setup(cx, SourcePolicy::name_like());
    sources.update(cx, |s, cx| s.set_query("ab", cx));
    cx.run_until_parked();
    reply(&log, 0, answer(&["one"]));
    cx.run_until_parked();
    assert_eq!(rows(&sources, cx), ["one"]);
    sources.update(cx, |s, cx| s.set_query("a", cx));
    cx.run_until_parked();
    assert_eq!(log.borrow().requests.len(), 1, "nothing was sent");
    assert!(rows(&sources, cx).is_empty());
}

#[vespertine::test]
fn an_indexing_source_refreshes_after_500_1000_2000_and_2000_ms(cx: &mut TestAppContext) {
    let (sources, log) = setup(cx, SourcePolicy::name_like());
    sources.update(cx, |s, cx| s.set_query("ab", cx));
    cx.run_until_parked();
    let partial = || {
        Ok(Answer {
            indexing: true,
            ..Answer::complete(vec!["partial".to_string()])
        })
    };
    let mut expected_requests = 1;
    for wait in [500u64, 1000, 2000, 2000] {
        reply(&log, expected_requests - 1, partial());
        cx.run_until_parked();
        sources.read_with(cx, |s, _| assert!(s.set().is_indexing(0)));
        assert_eq!(rows(&sources, cx), ["partial"], "partial rows stay displayed");
        cx.executor().advance_clock(Duration::from_millis(wait - 1));
        cx.run_until_parked();
        assert_eq!(log.borrow().requests.len(), expected_requests, "not before {wait} ms");
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        expected_requests += 1;
        assert_eq!(log.borrow().requests.len(), expected_requests, "refresh after {wait} ms");
    }
    reply(&log, expected_requests - 1, answer(&["done"]));
    cx.run_until_parked();
    sources.read_with(cx, |s, _| assert!(!s.set().is_indexing(0)));
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert_eq!(log.borrow().requests.len(), expected_requests, "no more refreshes");
}

#[vespertine::test]
fn scrolling_to_the_end_requests_exactly_one_next_page_and_none_after_a_newer_generation(
    cx: &mut TestAppContext,
) {
    let (sources, log) = setup(cx, SourcePolicy::name_like());
    sources.update(cx, |s, cx| s.set_query("ab", cx));
    cx.run_until_parked();
    reply(
        &log,
        0,
        Ok(Answer {
            has_more: true,
            ..Answer::complete(vec!["a".into(), "b".into()])
        }),
    );
    cx.run_until_parked();
    sources.update(cx, |s, cx| s.scrolled_to_end(cx));
    sources.update(cx, |s, cx| s.scrolled_to_end(cx));
    cx.run_until_parked();
    let requests = log.borrow().requests.clone();
    assert_eq!(requests.len(), 2, "one page request");
    assert_eq!(requests[1].page, 1);
    // A newer generation begins: the page answer is dropped and nothing pages.
    sources.update(cx, |s, cx| s.set_query("abc", cx));
    cx.run_until_parked();
    let _ = log.borrow_mut().replies[1].take().map(|tx| tx.send(answer(&["late"])));
    cx.run_until_parked();
    sources.update(cx, |s, cx| s.scrolled_to_end(cx));
    cx.run_until_parked();
    assert_eq!(log.borrow().requests.len(), 3, "only the new first-page request");
    assert!(rows(&sources, cx).iter().all(|row| row != "late"));
}

#[vespertine::test]
fn clearing_cancels_the_in_flight_request_and_empties_the_rows(cx: &mut TestAppContext) {
    let (sources, log) = setup(cx, SourcePolicy::name_like());
    sources.update(cx, |s, cx| s.set_query("ab", cx));
    cx.run_until_parked();
    reply(&log, 0, answer(&["x"]));
    cx.run_until_parked();
    sources.update(cx, |s, cx| s.set_query("abc", cx));
    cx.run_until_parked();
    sources.update(cx, |s, cx| s.clear(cx));
    cx.run_until_parked();
    assert!(rows(&sources, cx).is_empty());
    let late = log.borrow_mut().replies[1].take().expect("still held");
    assert!(late.send(answer(&["late"])).is_err(), "the request task was dropped");
}
