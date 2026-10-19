//! PRF-017 A1: streams text into a long block document and counts parse and layout calls per
//! block. Settled blocks are re-parsed and re-measured zero times per update after warm-up and
//! the final document equals a full re-parse; the naive full re-parse of every update is
//! measured for comparison. Exits 0 when the counts and the equality hold.

use std::{
    cell::RefCell,
    collections::HashMap,
    time::{Duration, Instant},
};
use vespertine_residency::{BlockDocument, BlockModel, ParsedBlock};

const SETTLED_BLOCKS: usize = 10_000;
const UPDATES: usize = 600;
const WARM_UP: usize = 120;
const BASELINE_UPDATES: usize = 20;

/// Paragraphs separated by a blank line; a paragraph is open until its blank line arrives.
/// A block is its start offset; parse and measure calls are counted per block.
#[derive(Default)]
struct Paragraphs {
    parses: RefCell<HashMap<usize, u32>>,
    measures: RefCell<HashMap<usize, u32>>,
}

impl BlockModel for Paragraphs {
    type Block = usize;
    type Layout = usize;

    fn parse(&self, source: &str, start: usize) -> ParsedBlock<usize> {
        *self.parses.borrow_mut().entry(start).or_default() += 1;
        match source[start..].find("\n\n") {
            Some(at) => ParsedBlock {
                block: start,
                end: start + at + 2,
                open: false,
            },
            None => ParsedBlock {
                block: start,
                end: source.len(),
                open: true,
            },
        }
    }

    fn measure(&self, block: &usize, text: &str) -> usize {
        *self.measures.borrow_mut().entry(*block).or_default() += 1;
        // Stand-in for shaping: one line per 110 characters.
        text.trim_end().chars().count().div_ceil(110).max(1)
    }
}

/// 40 characters of text that depend on `index`.
fn line(index: usize) -> String {
    const WORDS: [&str; 8] = [
        "amber", "birch", "cedar", "dusk", "ember", "fjord", "grove", "heron",
    ];
    let mut line = String::new();
    let mut word = index;
    while line.len() < 40 {
        line.push_str(WORDS[word % WORDS.len()]);
        line.push('_');
        word = word / 3 + 5 * index + line.len();
    }
    line.truncate(40);
    line
}

fn chunk(f: usize) -> String {
    line(f % 1000)[20..40].to_string()
}

fn main() {
    let settled: String = (0..SETTLED_BLOCKS)
        .map(|j| format!("{}\n\n", &line(j % 1000)[..20]))
        .collect();

    // Incremental: one document, one appended chunk per frame.
    let mut document = BlockDocument::new(Paragraphs::default());
    document.update(&settled);
    let mut text = settled.clone();
    let mut parsed_after_warm_up = 0u64;
    let mut incremental_time = Duration::ZERO;
    for f in 0..UPDATES {
        let piece = chunk(f);
        text.push_str(&piece);
        let started = Instant::now();
        let stats = document.append(&piece);
        if f >= WARM_UP {
            incremental_time += started.elapsed();
            parsed_after_warm_up += stats.parsed as u64;
            assert_eq!(
                stats.reused, SETTLED_BLOCKS,
                "update {f} reused every settled block"
            );
            assert!(
                stats.parsed <= 1,
                "update {f} parsed {} blocks",
                stats.parsed
            );
            assert_eq!(stats.measured, stats.parsed);
        }
    }

    // Every settled block was parsed and measured once (the initial load) and never again.
    let parses = document.model().parses.borrow();
    let measures = document.model().measures.borrow();
    let settled = &document.blocks()[..SETTLED_BLOCKS];
    let reparsed = settled
        .iter()
        .filter(|b| parses[&b.range.start] != 1)
        .count();
    let remeasured = settled
        .iter()
        .filter(|b| measures[&b.range.start] != 1)
        .count();
    assert_eq!((reparsed, remeasured), (0, 0), "settled blocks were redone");
    drop((parses, measures));

    // Naive baseline: a full re-parse of the whole text on every update.
    let mut baseline_text = text.clone();
    let mut baseline_time = Duration::ZERO;
    let mut baseline_parsed = 0u64;
    for f in 0..BASELINE_UPDATES {
        baseline_text.push_str(&chunk(f));
        let started = Instant::now();
        let mut fresh = BlockDocument::new(Paragraphs::default());
        let stats = fresh.update(&baseline_text);
        baseline_time += started.elapsed();
        baseline_parsed += stats.parsed as u64;
    }

    // The incremental result equals a full parse of the final text.
    let mut reference = BlockDocument::new(Paragraphs::default());
    reference.update(&text);
    assert_eq!(document.source(), reference.source());
    assert_eq!(document.blocks().len(), reference.blocks().len());
    for (a, b) in document.blocks().iter().zip(reference.blocks()) {
        assert_eq!(
            (a.range.clone(), a.open, a.layout),
            (b.range.clone(), b.open, b.layout)
        );
    }

    let measured = (UPDATES - WARM_UP) as f64;
    println!("stream_document: {SETTLED_BLOCKS} settled blocks, {UPDATES} appended chunks");
    println!(
        "incremental: {:.2} blocks parsed per update after warm-up, {:.1} us per update",
        parsed_after_warm_up as f64 / measured,
        incremental_time.as_secs_f64() * 1e6 / measured,
    );
    println!(
        "full re-parse: {:.0} blocks parsed per update, {:.1} us per update",
        baseline_parsed as f64 / BASELINE_UPDATES as f64,
        baseline_time.as_secs_f64() * 1e6 / BASELINE_UPDATES as f64,
    );
}
