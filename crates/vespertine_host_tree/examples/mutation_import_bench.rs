//! DEV-008: native cost of importing a recorded real mutation queue.
//!
//! Applies the batches recorded from a React reconciler run (the fixture is written by
//! `packages/vespertine-host/scripts/record-fixture.mjs`) through the production batch parser
//! and tree, and reports for each measured phase the elapsed time, the allocation churn (every
//! allocation and the growth of every reallocation), the bytes still live at the end of the
//! phase and the number of allocations, measured by a wrapping global allocator. Experimental
//! alternatives (generic JSON, raw tuples, compact styles, MessagePack) are measured on the same
//! bytes and labelled as alternatives; they are not shipped behavior.
//!
//! Needs no display server and no GPU:
//!
//! ```text
//! cargo run --release -p vespertine_host_tree --example mutation_import_bench -- [fixture.ndjson] [iterations]
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use serde_json::value::RawValue;
use vespertine_host_tree::HostTree;

// ---------------------------------------------------------------------------------- allocator

static CHURN: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicI64 = AtomicI64::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);

/// Global allocator wrapper. `churn` counts the bytes of every allocation and the growth of
/// every reallocation; `live` follows allocations, frees and reallocation growth and shrinkage;
/// `allocs` counts allocation, zeroed allocation and reallocation calls.
struct MeterAlloc;

unsafe impl GlobalAlloc for MeterAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            if new_size > layout.size() {
                CHURN.fetch_add((new_size - layout.size()) as u64, Ordering::Relaxed);
            }
            LIVE.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
        }
        moved
    }
}

fn record_alloc(size: usize) {
    CHURN.fetch_add(size as u64, Ordering::Relaxed);
    LIVE.fetch_add(size as i64, Ordering::Relaxed);
    ALLOCS.fetch_add(1, Ordering::Relaxed);
}

#[global_allocator]
static ALLOCATOR: MeterAlloc = MeterAlloc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counters {
    churn: u64,
    live: i64,
    allocs: u64,
}

fn counters() -> Counters {
    Counters {
        churn: CHURN.load(Ordering::Relaxed),
        live: LIVE.load(Ordering::Relaxed),
        allocs: ALLOCS.load(Ordering::Relaxed),
    }
}

/// Starts a phase at a quiescent point: the benchmark is single threaded, so no other thread
/// allocates between the reset and the read. `live` is not reset; it is reported relative to
/// the baseline of the whole path.
fn reset_phase() {
    CHURN.store(0, Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
}

/// Known allocations, reallocations (growing and shrinking) and frees against the counters.
fn self_check() {
    let layout = |size| Layout::from_size_align(size, 8).unwrap();
    reset_phase();
    let before = counters();
    unsafe {
        let block = ALLOCATOR.alloc(layout(1000));
        let after_alloc = counters();
        assert_eq!(after_alloc.churn, 1000);
        assert_eq!(after_alloc.live - before.live, 1000);
        assert_eq!(after_alloc.allocs, 1);

        let block = ALLOCATOR.realloc(block, layout(1000), 3000);
        let after_grow = counters();
        assert_eq!(
            after_grow.churn, 3000,
            "growth counts only the added 2000 bytes"
        );
        assert_eq!(after_grow.live - before.live, 3000);
        assert_eq!(after_grow.allocs, 2);

        let block = ALLOCATOR.realloc(block, layout(3000), 500);
        let after_shrink = counters();
        assert_eq!(after_shrink.churn, 3000, "shrinking adds no churn");
        assert_eq!(after_shrink.live - before.live, 500);
        assert_eq!(after_shrink.allocs, 3);

        ALLOCATOR.dealloc(block, layout(500));
        let after_free = counters();
        assert_eq!(after_free.live, before.live);
        assert_eq!(after_free.churn, 3000);
        assert_eq!(after_free.allocs, 3);

        let zeroed = ALLOCATOR.alloc_zeroed(layout(64));
        assert_eq!(counters().churn, 3064);
        assert_eq!(counters().allocs, 4);
        ALLOCATOR.dealloc(zeroed, layout(64));
        assert_eq!(counters().live, before.live);
    }
    println!("allocator self-check: ok (alloc 1000, grow to 3000, shrink to 500, free, zeroed 64)");
}

// ---------------------------------------------------------------------------------- measuring

#[derive(Clone, Copy, Debug)]
struct Phase {
    elapsed: Duration,
    churn: u64,
    live: i64,
    allocs: u64,
}

/// Runs `body` as one measured phase. `baseline_live` is the live byte count before the
/// measured path started.
fn measure<R>(baseline_live: i64, body: impl FnOnce() -> R) -> (R, Phase) {
    reset_phase();
    let started = Instant::now();
    let result = body();
    let elapsed = started.elapsed();
    let end = counters();
    (
        result,
        Phase {
            elapsed,
            churn: end.churn,
            live: end.live - baseline_live,
            allocs: end.allocs,
        },
    )
}

struct Row {
    path: String,
    label: &'static str,
    phase: &'static str,
    median: Duration,
    sample: Phase,
}

/// Runs `run` for each iteration, returning one row per phase it reports. The counters of the
/// first iteration are reported; every later iteration must produce the same counters.
fn bench(
    path: &str,
    label: &'static str,
    iterations: usize,
    run: &mut dyn FnMut() -> Vec<(&'static str, Phase)>,
) -> Vec<Row> {
    let first = run();
    let mut times: Vec<Vec<Duration>> = first.iter().map(|(_, p)| vec![p.elapsed]).collect();
    for _ in 1..iterations {
        let next = run();
        for (index, (name, phase)) in next.iter().enumerate() {
            assert_eq!(
                (phase.churn, phase.live, phase.allocs),
                (
                    first[index].1.churn,
                    first[index].1.live,
                    first[index].1.allocs
                ),
                "{path}: phase {name} is not deterministic across iterations"
            );
            times[index].push(phase.elapsed);
        }
    }
    first
        .into_iter()
        .zip(times)
        .map(|((phase, sample), mut samples)| {
            samples.sort();
            Row {
                path: path.to_string(),
                label,
                phase,
                median: samples[samples.len() / 2],
                sample,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------- fixture

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

struct Fixture {
    batches: Vec<String>,
    operations: usize,
}

fn load_fixture(path: &str) -> (Fixture, usize, u64) {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
    let text = String::from_utf8(bytes.clone()).expect("fixture is UTF-8");
    let batches: Vec<String> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let operations = batches
        .iter()
        .map(|batch| {
            serde_json::from_str::<Vec<Value>>(batch)
                .expect("batch")
                .len()
        })
        .sum();
    (
        Fixture {
            batches,
            operations,
        },
        bytes.len(),
        fnv1a(&bytes),
    )
}

// ---------------------------------------------------------------------------------- paths

/// The shipped path: the production parser and tree. Phases: deserialization (decode every
/// operation and resolve every style payload into a shared style: `prepare_batch`), tree
/// construction (`commit_batch`), and the combined `apply_batch` path on a fresh tree.
fn shipped(fixture: &Fixture, iterations: usize) -> Vec<Row> {
    let mut rows = bench(
        "production parser + HostTree (prepare, commit)",
        "SHIPPED",
        iterations,
        &mut || {
            let baseline = counters().live;
            let mut tree = HostTree::new();
            let (prepared, deserialize) = measure(baseline, || {
                fixture
                    .batches
                    .iter()
                    .map(|batch| tree.prepare_batch(batch).expect("fixture batch decodes"))
                    .collect::<Vec<_>>()
            });
            let ((), construct) = measure(baseline, || {
                for batch in prepared {
                    tree.commit_batch(batch);
                }
            });
            drop(tree);
            vec![
                ("deserialization", deserialize),
                ("tree construction", construct),
            ]
        },
    );
    rows.extend(bench(
        "production parser + HostTree (apply_batch)",
        "SHIPPED",
        iterations,
        &mut || {
            let baseline = counters().live;
            let mut tree = HostTree::new();
            let ((), combined) = measure(baseline, || {
                for batch in &fixture.batches {
                    tree.apply_batch(batch).expect("fixture batch applies");
                }
            });
            // `combined.live` is the heap the retained tree owns after the whole queue.
            drop(tree);
            vec![("combined (retained heap in live)", combined)]
        },
    ));
    rows
}

fn alternatives(fixture: &Fixture, iterations: usize) -> Vec<Row> {
    let mut rows = Vec::new();

    rows.push(
        bench(
            "generic dynamic JSON (serde_json::Value)",
            "ALTERNATIVE",
            iterations,
            &mut || {
                let baseline = counters().live;
                let (parsed, phase) = measure(baseline, || {
                    fixture
                        .batches
                        .iter()
                        .map(|batch| serde_json::from_str::<Value>(batch).expect("parses"))
                        .collect::<Vec<_>>()
                });
                drop(parsed);
                vec![("deserialization", phase)]
            },
        )
        .remove(0),
    );

    rows.push(
        bench(
            "typed tuples of raw values (Vec<Vec<&RawValue>>)",
            "ALTERNATIVE",
            iterations,
            &mut || {
                let baseline = counters().live;
                let (parsed, phase) = measure(baseline, || {
                    fixture
                        .batches
                        .iter()
                        .map(|batch| {
                            serde_json::from_str::<Vec<Vec<&RawValue>>>(batch).expect("parses")
                        })
                        .collect::<Vec<_>>()
                });
                drop(parsed);
                vec![("deserialization", phase)]
            },
        )
        .remove(0),
    );

    // Compact style encoding: every setStyle payload object becomes an array of
    // [key index, value] pairs over a shared key dictionary.
    let mut dictionary: Vec<String> = Vec::new();
    let compact: Vec<String> = fixture
        .batches
        .iter()
        .map(|batch| {
            let mut ops: Vec<Vec<Value>> = serde_json::from_str(batch).expect("batch");
            for op in &mut ops {
                if op[0] == "setStyle"
                    && let Value::Object(style) = &op[2]
                {
                    let pairs: Vec<Value> = style
                        .iter()
                        .map(|(key, value)| {
                            let index =
                                dictionary.iter().position(|k| k == key).unwrap_or_else(|| {
                                    dictionary.push(key.clone());
                                    dictionary.len() - 1
                                });
                            Value::Array(vec![Value::from(index), value.clone()])
                        })
                        .collect();
                    op[2] = Value::Array(pairs);
                }
            }
            serde_json::to_string(&ops).expect("serializes")
        })
        .collect();
    rows.push(
        bench(
            "compact style encoding + raw tuples",
            "ALTERNATIVE",
            iterations,
            &mut || {
                let baseline = counters().live;
                let (parsed, phase) = measure(baseline, || {
                    compact
                        .iter()
                        .map(|batch| {
                            serde_json::from_str::<Vec<Vec<&RawValue>>>(batch).expect("parses")
                        })
                        .collect::<Vec<_>>()
                });
                drop(parsed);
                vec![("deserialization", phase)]
            },
        )
        .remove(0),
    );

    // MessagePack: the same operations encoded as a binary array of arrays.
    let packed: Vec<Vec<u8>> = fixture
        .batches
        .iter()
        .map(|batch| {
            let ops: Vec<Vec<Value>> = serde_json::from_str(batch).expect("batch");
            rmp_serde::to_vec(&ops).expect("encodes")
        })
        .collect();
    rows.push(
        bench(
            "binary MessagePack into dynamic values",
            "ALTERNATIVE",
            iterations,
            &mut || {
                let baseline = counters().live;
                let (parsed, phase) = measure(baseline, || {
                    packed
                        .iter()
                        .map(|bytes| rmp_serde::from_slice::<Value>(bytes).expect("decodes"))
                        .collect::<Vec<_>>()
                });
                drop(parsed);
                vec![("deserialization", phase)]
            },
        )
        .remove(0),
    );
    rows
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        format!(
            "{}/fixtures/react_mutation_queue.ndjson",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let iterations: usize = args
        .next()
        .map(|text| text.parse().expect("iterations is a number"))
        .unwrap_or(5)
        .max(1);

    self_check();
    let (fixture, bytes, hash) = load_fixture(&path);
    println!(
        "fixture: {path}\n  {} batches, {} operations, {bytes} bytes, fnv1a {hash:016x}; every row below used these bytes ({iterations} iterations, median time)",
        fixture.batches.len(),
        fixture.operations,
    );
    println!("  elapsed times include the allocator instrumentation");

    let mut rows = shipped(&fixture, iterations);
    rows.extend(alternatives(&fixture, iterations));

    println!(
        "\n{:<11} {:<52} {:<34} {:>11} {:>14} {:>12} {:>11}",
        "label", "path", "phase", "elapsed ms", "churn bytes", "live bytes", "allocations"
    );
    for row in &rows {
        println!(
            "{:<11} {:<52} {:<34} {:>11.3} {:>14} {:>12} {:>11}",
            row.label,
            row.path,
            row.phase,
            row.median.as_secs_f64() * 1000.0,
            row.sample.churn,
            row.sample.live,
            row.sample.allocs,
        );
    }
    println!(
        "\nSHIPPED rows run the production parser and retained tree. ALTERNATIVE rows are\n\
         experimental codecs local to this benchmark (deserialization only, not shipped behavior);\n\
         no gain is claimed for any of them: compare the measured columns only."
    );
}
