//! Acceptance tests of DEV-003 for the headless runner, the workload catalogue and lockstep
//! verification. Windows are real windows on the deterministic test platform with real text
//! shaping; steps are scripted, so every number asserted here is deterministic.

use anyhow::Result;
use std::{cell::RefCell, collections::BTreeSet, rc::Rc, sync::Arc};
use vespertine::{
    AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PlatformTextSystem,
    Render, Styled as _, Window, div,
};
use vespertine_bench::{
    Cost, Driver, Family, FaultKind, Harness, Instructions, Meter, Metric, RetentionMode,
    RunOptions, Stage, Workload, catalogue,
    harness::{Host, real_text_system},
    metrics::InstructionSource,
    run_scenario, select,
    verify::{FaultPlan, Lockstep, LockstepOptions},
    workloads::{QuoteBook, Schedule, SortColumn, TranscriptPattern, format_volume, quote_driver},
};

/// Allocation counts are process-wide, so tests that measure them run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn text() -> Arc<dyn PlatformTextSystem> {
    real_text_system()
}

fn meter() -> Rc<RefCell<Meter>> {
    Rc::new(RefCell::new(Meter::new()))
}

fn options(warmup: u64, frames: u64) -> RunOptions {
    RunOptions {
        warmup: Some(warmup),
        frames: Some(frames),
        phase_times: false,
    }
}

fn workload(name: &str) -> Workload {
    let found = select(&[name.to_owned()]);
    *found
        .iter()
        .find(|workload| workload.name == name)
        .expect("the workload exists")
}

// ---------------------------------------------------------------------------------------------
// A1: one draw per step; forced draws counted separately; the step's own cost is not the frame's
// ---------------------------------------------------------------------------------------------

struct Counter {
    value: u64,
    padding: Vec<Vec<u8>>,
}

impl Render for Counter {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(format!("count {}", self.value))
    }
}

/// Notifies the view on even steps and leaves the window clean on odd ones. Optionally the
/// mutation burns CPU and allocates a known amount.
struct Alternating {
    model: Entity<Counter>,
    burn_milliseconds: u64,
    allocations: usize,
}

impl Driver for Alternating {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let model = &self.model;
        let (burn, allocations) = (self.burn_milliseconds, self.allocations);
        stage.mutate(|app| {
            let spin_until = std::time::Instant::now() + std::time::Duration::from_millis(burn);
            while std::time::Instant::now() < spin_until {
                std::hint::black_box(0u64);
            }
            model.update(app, |counter, cx| {
                counter
                    .padding
                    .extend((0..allocations).map(|_| vec![7u8; 64]));
                if frame.is_multiple_of(2) {
                    counter.value += 1;
                    cx.notify();
                }
            });
        });
    }
}

fn counter_setup(
    burn_milliseconds: u64,
    allocations: usize,
) -> impl Fn(&mut dyn Host) -> Result<Box<dyn Driver>> {
    move |host| {
        let mut model = None;
        host.open_window(|_, cx| {
            let entity = cx.new(|_| Counter {
                value: 0,
                padding: Vec::new(),
            });
            model = Some(entity.clone());
            AnyView::from(entity)
        })?;
        Ok(Box::new(Alternating {
            model: model.expect("the window built the counter"),
            burn_milliseconds,
            allocations,
        }))
    }
}

fn alternating() -> Workload {
    Workload {
        name: "test-alternating",
        family: Family::Baseline,
        description: "notifies on even steps, leaves the window clean on odd ones",
        warmup: 2,
        frames: 10,
        setup: |host| counter_setup(0, 0)(host),
    }
}

#[test]
fn dev_003_a1_exactly_one_draw_per_step_and_forced_draws_are_separate() {
    let _serial = serial();
    let (text, meter) = (text(), meter());
    let report = run_scenario(
        &text,
        &meter,
        &alternating(),
        RetentionMode::On,
        &options(2, 10),
    )
    .unwrap();
    for sample in &report.samples {
        let outcome = sample.outcome;
        let draws = outcome.natural_draws + outcome.forced_draws;
        assert_eq!(draws, 1, "frame {}: {outcome:?}", sample.frame);
        // Even frames notify the view (a natural draw); odd frames leave the window clean, so
        // the runner forces the one draw.
        assert_eq!(
            u64::from(outcome.forced_draws),
            sample.frame % 2,
            "frame {}",
            sample.frame
        );
    }
    assert_eq!(report.natural_draws, 5);
    assert_eq!(report.forced_draws, 5);
    assert_eq!(
        report.stats.draws, 10,
        "the window's own draw count equals the accounted draws"
    );

    let idle = run_scenario(
        &text,
        &meter,
        &workload("idle-unchanged"),
        RetentionMode::On,
        &options(1, 6),
    )
    .unwrap();
    assert_eq!((idle.natural_draws, idle.forced_draws), (0, 6));
    let busy = run_scenario(
        &text,
        &meter,
        &workload("quotes-sparse"),
        RetentionMode::On,
        &options(1, 6),
    )
    .unwrap();
    assert_eq!((busy.natural_draws, busy.forced_draws), (6, 0));
}

#[test]
fn dev_003_a1_the_known_step_cost_is_not_in_the_frame_figures() {
    let _serial = serial();
    const ALLOCATIONS: usize = 20_000;
    const BURN_MS: u64 = 30;
    let (text, meter) = (text(), meter());
    let costly = Workload {
        setup: |host| counter_setup(BURN_MS, ALLOCATIONS)(host),
        ..alternating()
    };
    let costly_report =
        run_scenario(&text, &meter, &costly, RetentionMode::On, &options(1, 6)).unwrap();
    let cheap_report = run_scenario(
        &text,
        &meter,
        &alternating(),
        RetentionMode::On,
        &options(1, 6),
    )
    .unwrap();
    for (costly, cheap) in costly_report.samples.iter().zip(&cheap_report.samples) {
        let (step, frame) = (costly.outcome.step, costly.outcome.frame);
        // The step really cost what it was made to cost...
        assert!(step.allocations >= ALLOCATIONS as u64, "{step:?}");
        assert!(step.cpu_ns >= BURN_MS * 1_000_000, "{step:?}");
        assert!(step.wall_ns >= BURN_MS * 1_000_000, "{step:?}");
        // ...and none of it is in the frame: the frame's allocations are far below the step's
        // and within a small constant of the same frame without the cost, and its CPU time is
        // nowhere near the burn.
        assert!(
            frame.allocations < ALLOCATIONS as u64 / 4,
            "frame {} allocations {} leaked step cost {}",
            costly.frame,
            frame.allocations,
            step.allocations
        );
        assert!(
            frame.allocations.abs_diff(cheap.outcome.frame.allocations) <= 64,
            "{frame:?} against {:?}",
            cheap.outcome.frame
        );
        assert!(frame.cpu_ns < BURN_MS * 1_000_000 / 2, "{frame:?}");
        assert!(
            cheap.outcome.step.cpu_ns < BURN_MS * 1_000_000 / 10,
            "{:?}",
            cheap.outcome.step
        );
    }
}

// ---------------------------------------------------------------------------------------------
// A2: lockstep verification
// ---------------------------------------------------------------------------------------------

fn lockstep(
    workload: &Workload,
    options: &LockstepOptions,
    run: &RunOptions,
) -> vespertine_bench::verify::VerifyReport {
    Lockstep::new()
        .run(&text(), workload, options, run)
        .unwrap()
}

#[test]
fn dev_003_a2_retained_and_full_windows_agree_on_every_frame() {
    let _serial = serial();
    for name in ["quotes-sparse", "typing-validation", "wheel-scroll"] {
        let report = lockstep(
            &workload(name),
            &LockstepOptions::default(),
            &options(2, 12),
        );
        assert!(report.passed(), "{name}: {:?}", report.mismatch);
        assert_eq!(report.frames_compared, 14);
        assert_eq!(
            report.checks,
            ["scene primitives", "hit tests", "listener positions"]
        );
    }
}

#[test]
fn dev_003_a2_a_seeded_one_pixel_fault_fails_every_kind_of_difference() {
    let _serial = serial();
    let quotes = workload("quotes-sparse");
    for kind in FaultKind::ALL {
        let report = lockstep(
            &quotes,
            &LockstepOptions {
                fault: Some(FaultPlan { frame: 6, kind }),
                ..LockstepOptions::default()
            },
            &options(2, 12),
        );
        let mismatch = report
            .mismatch
            .as_ref()
            .unwrap_or_else(|| panic!("{kind:?} was not detected"));
        assert_eq!(
            mismatch.frame, 6,
            "{kind:?}: the run fails on the frame that shows the fault"
        );
        assert_eq!(mismatch.check, "scene primitives");
        assert_eq!(
            report.frames_compared, 7,
            "the run stops at the first mismatch"
        );
    }
    // Seeded plans are reproducible and land inside the run.
    for seed in 0..8 {
        let plan = FaultPlan::seeded(seed, 40);
        assert_eq!(plan, FaultPlan::seeded(seed, 40));
        assert!((20..40).contains(&plan.frame), "{plan:?}");
    }
}

#[test]
fn dev_003_a2_different_atlas_placement_is_tolerated() {
    let _serial = serial();
    let quotes = workload("quotes-sparse");
    let (text, meter) = (text(), meter());
    // The retained window gets a different packing history: its tile identities differ.
    let tile_ids = |perturb: bool| {
        let mut harness = Harness::new(text.clone(), meter.clone(), true);
        if perturb {
            harness.perturb_atlas_on_open();
        }
        let _driver = (quotes.setup)(&mut harness).unwrap();
        harness
            .read_window(|window, _| window.painted_tile_ids())
            .unwrap()
    };
    let (plain, perturbed) = (tile_ids(false), tile_ids(true));
    assert_eq!(plain.len(), perturbed.len());
    assert_ne!(
        plain, perturbed,
        "the perturbation must change the atlas identities"
    );

    let report = lockstep(
        &quotes,
        &LockstepOptions {
            perturb_atlas: true,
            ..LockstepOptions::default()
        },
        &options(2, 10),
    );
    assert!(report.passed(), "{:?}", report.mismatch);
}

// ---------------------------------------------------------------------------------------------
// A3: workloads for every family of R4; the R5 transcript patterns
// ---------------------------------------------------------------------------------------------

#[test]
fn dev_003_a3_every_workload_family_has_a_workload_that_runs_in_both_modes() {
    let _serial = serial();
    let all = catalogue();
    let names: BTreeSet<_> = all.iter().map(|workload| workload.name).collect();
    assert_eq!(names.len(), all.len(), "workload names are unique");
    for family in Family::R4 {
        assert!(
            all.iter().any(|workload| workload.family == family),
            "{family:?} has no workload"
        );
    }
    let (text, meter) = (text(), meter());
    for workload in &all {
        for mode in [RetentionMode::On, RetentionMode::Off] {
            let report = run_scenario(&text, &meter, workload, mode, &options(1, 2))
                .unwrap_or_else(|error| panic!("{} {mode:?}: {error:#}", workload.name));
            assert_eq!(report.samples.len(), 2, "{}", workload.name);
            assert!(
                report.natural_draws + report.forced_draws >= 2,
                "{} {mode:?}: {} natural {} forced",
                workload.name,
                report.natural_draws,
                report.forced_draws
            );
        }
    }
}

#[test]
fn dev_003_a3_workload_filters_select_by_name_or_substring() {
    let _serial = serial();
    let exact = select(&["quotes-dense".to_owned()]);
    assert_eq!(exact.len(), 1);
    let quotes = select(&["quotes".to_owned()]);
    assert!(
        quotes
            .iter()
            .all(|workload| workload.name.contains("quotes"))
    );
    assert!(quotes.len() >= 4);
    assert_eq!(select(&[]).len(), catalogue().len());
    assert!(select(&["no-such-workload".to_owned()]).is_empty());
}

#[test]
fn dev_003_r5_transcript_patterns_follow_the_specification() {
    let _serial = serial();
    for messages in [160, 200] {
        let patterns: Vec<_> = (0..720)
            .map(|frame| TranscriptPattern::at(frame, messages))
            .collect();
        // Four trackpad events totaling 40 logical px per sweep frame, reversing after 120.
        for frame in 0..120 {
            let events = &patterns[frame].events;
            assert_eq!(events.len(), 4);
            assert_eq!(events.iter().sum::<f32>(), -40.0, "frame {frame}");
        }
        // A 30-frame dwell at the end, then the reverse sweep, then a dwell before the cycle
        // starts again.
        assert!((120..150).all(|frame| patterns[frame].events.is_empty()));
        assert!((150..270).all(|frame| patterns[frame].events.iter().sum::<f32>() == 40.0));
        assert!((270..300).all(|frame| patterns[frame].events.is_empty()));
        assert_eq!(
            patterns[300].events.iter().sum::<f32>(),
            -40.0,
            "the cycle repeats"
        );
        // A jump every 240 frames (not at frame 0).
        let jumps: Vec<usize> = (0..720)
            .filter(|frame| patterns[*frame].jump.is_some())
            .collect();
        assert_eq!(jumps, [240, 480]);
        assert!(
            jumps
                .iter()
                .all(|frame| patterns[*frame].jump.unwrap() < messages)
        );
        // A 108-frame highlight from every jump period's start.
        for start in [0usize, 240, 480] {
            assert!(
                (start..start + 108).all(|frame| patterns[frame].highlight.is_some()),
                "{start}"
            );
            assert!((start + 108..start + 240).all(|frame| patterns[frame].highlight.is_none()));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A4: an unavailable instruction counter is reported as unavailable, never as zero
// ---------------------------------------------------------------------------------------------

struct Counting(u64);

impl InstructionSource for Counting {
    fn read(&mut self) -> u64 {
        self.0 += 1_000;
        self.0
    }
}

struct Stuck;

impl InstructionSource for Stuck {
    fn read(&mut self) -> u64 {
        0
    }
}

#[test]
fn dev_003_a4_an_unavailable_instruction_counter_is_reported_not_zero() {
    let _serial = serial();
    let text = text();
    let denied = Rc::new(RefCell::new(Meter::with_instructions(
        Instructions::Unavailable("perf_event_open: permission denied".to_owned()),
    )));
    let report = run_scenario(
        &text,
        &denied,
        &alternating(),
        RetentionMode::On,
        &options(1, 4),
    )
    .unwrap();
    let instructions = report.instructions();
    assert_eq!(
        instructions,
        Metric::Unavailable("perf_event_open: permission denied".to_owned()),
        "never a distribution of zeros"
    );
    assert!(instructions.measured().is_none());
    assert!(
        instructions
            .describe(1.0, "k")
            .starts_with("unavailable (perf_event_open")
    );
    assert!(
        report
            .samples
            .iter()
            .all(|sample| sample.outcome.frame.instructions.is_none())
    );
    // The other metrics are unaffected.
    assert!(report.wall().measured().is_some());

    // A counter that opens but never advances is unavailable too, with its own reason.
    let stuck = Instructions::from_source(Box::new(Stuck));
    assert!(
        stuck
            .unavailable_reason()
            .unwrap()
            .contains("does not advance")
    );

    // A working counter is measured and nonzero.
    let working = Rc::new(RefCell::new(Meter::with_instructions(
        Instructions::from_source(Box::new(Counting(0))),
    )));
    assert!(working.borrow().instructions_unavailable().is_none());
    let report = run_scenario(
        &text,
        &working,
        &alternating(),
        RetentionMode::On,
        &options(1, 4),
    )
    .unwrap();
    let measured = report.instructions();
    assert!(measured.measured().unwrap().mean > 0.0, "{measured:?}");
}

#[test]
fn dev_003_a4_costs_subtract_and_add_without_losing_availability() {
    let _serial = serial();
    let some = Cost {
        cpu_ns: 10,
        wall_ns: 20,
        allocations: 3,
        allocated_bytes: 40,
        instructions: Some(50),
    };
    let none = Cost {
        instructions: None,
        ..some
    };
    assert_eq!((some - some).instructions, Some(0));
    assert_eq!((some - none).instructions, None);
    assert_eq!((some + some).cpu_ns, 20);
    assert_eq!((Cost::default() - some).cpu_ns, 0, "differences saturate");
}

// ---------------------------------------------------------------------------------------------
// A5: the quote workloads
// ---------------------------------------------------------------------------------------------

/// Runs `schedule` for `frames` steps and returns, per step, the number of row views whose render
/// ran (the rows the window rebuilt), with the probe of what the driver did.
fn rows_rebuilt_per_step(
    schedule: Schedule,
    frames: u64,
    mode: RetentionMode,
) -> (Vec<u64>, Rc<vespertine_bench::workloads::QuoteProbe>) {
    let mut harness = Harness::new(text(), meter(), false);
    let (mut driver, probe) = quote_driver(&mut harness, schedule).unwrap();
    harness.set_retention(mode).unwrap();
    harness.settle().unwrap();
    let mut rebuilt = Vec::new();
    for frame in 0..frames {
        let before = probe.rows_rendered.get();
        harness.run_frame(frame, &mut *driver).unwrap();
        rebuilt.push(probe.rows_rendered.get() - before);
    }
    (rebuilt, probe)
}

#[test]
fn dev_003_a5_sparse_and_dense_variants_update_exactly_4_and_100_of_200_rows() {
    let _serial = serial();
    for frame in 0..60 {
        let sparse = Schedule::Sparse.plan(frame, 200);
        let rows: BTreeSet<_> = sparse.updates.iter().copied().collect();
        assert_eq!(
            (sparse.updates.len(), rows.len()),
            (4, 4),
            "frame {frame}: distinct rows"
        );
        assert!(sparse.sort.is_none());
        let dense = Schedule::Dense.plan(frame, 200);
        let rows: BTreeSet<_> = dense.updates.iter().copied().collect();
        assert_eq!(
            (dense.updates.len(), rows.len()),
            (100, 100),
            "frame {frame}"
        );
    }
    // The retained window rebuilds exactly the rows that updated; the full render rebuilds all 200.
    let (sparse, probe) = rows_rebuilt_per_step(Schedule::Sparse, 12, RetentionMode::On);
    assert!(sparse.iter().all(|rebuilt| *rebuilt == 4), "{sparse:?}");
    assert!(
        probe
            .updates
            .borrow()
            .iter()
            .all(|(_, updated)| *updated == 4)
    );
    let (dense, probe) = rows_rebuilt_per_step(Schedule::Dense, 6, RetentionMode::On);
    assert!(dense.iter().all(|rebuilt| *rebuilt == 100), "{dense:?}");
    assert!(
        probe
            .updates
            .borrow()
            .iter()
            .all(|(_, updated)| *updated == 100)
    );
    let (full, _) = rows_rebuilt_per_step(Schedule::Sparse, 3, RetentionMode::Off);
    assert!(full.iter().all(|rebuilt| *rebuilt == 200), "{full:?}");
}

#[test]
fn dev_003_a5_sorting_happens_on_multiples_of_15_with_one_price_update_between_sorts() {
    let _serial = serial();
    let mut sorts = Vec::new();
    let mut updates_since_sort = 0;
    let mut between = Vec::new();
    let mut columns = Vec::new();
    for frame in 0..240u64 {
        let plan = Schedule::Sorted.plan(frame, 200);
        if let Some(column) = plan.sort {
            sorts.push(frame);
            columns.push(column);
            if frame > 0 {
                between.push(updates_since_sort);
            }
            updates_since_sort = 0;
        }
        updates_since_sort += plan.updates.len();
        assert!(plan.updates.len() <= 1, "frame {frame}");
    }
    assert_eq!(sorts, (0..16).map(|k| k * 15).collect::<Vec<_>>());
    assert!(between.iter().all(|updates| *updates == 1), "{between:?}");
    assert_eq!(
        &columns[..4],
        [
            SortColumn::Symbol,
            SortColumn::Price,
            SortColumn::Volume,
            SortColumn::Symbol
        ]
    );

    // The model and the window agree: the sort reorders every row, the single update rebuilds
    // exactly one row view.
    let (rebuilt, probe) = rows_rebuilt_per_step(Schedule::Sorted, 31, RetentionMode::On);
    assert_eq!(*probe.sorts.borrow(), [0, 15, 30]);
    assert_eq!(rebuilt[7], 1, "{rebuilt:?}");
    assert_eq!(rebuilt[22], 1, "{rebuilt:?}");
    assert!(
        rebuilt[15] > 100,
        "a price sort moves most rows: {rebuilt:?}"
    );
    assert_eq!(
        rebuilt[3], 0,
        "frames between sorts and updates rebuild nothing: {rebuilt:?}"
    );

    let mut book = QuoteBook::new(200);
    book.update(5, 7);
    book.sort(SortColumn::Price);
    assert_eq!(
        book.order[0],
        (0..200)
            .max_by(|a, b| book
                .row(*a)
                .price
                .total_cmp(&book.row(*b).price)
                .then(b.cmp(a)))
            .unwrap()
    );
    assert_eq!(format_volume(1_234_567), "1,234,567");
    assert_eq!(format_volume(999), "999");
}

#[test]
fn dev_003_a5_phased_schedule_follows_the_perf_dev_003_definition() {
    let _serial = serial();
    assert_eq!(
        Schedule::Phased.plan(10, 200),
        Schedule::Sparse.plan(10, 200)
    );
    assert_eq!(
        Schedule::Phased.plan(399, 200),
        Schedule::Sparse.plan(399, 200)
    );
    assert_eq!(Schedule::Phased.plan(400, 200).updates.len(), 100);
    assert_eq!(Schedule::Phased.plan(799, 200).updates.len(), 100);
    assert_eq!(Schedule::Phased.plan(809, 200).updates.len(), 0);
    let sorted_row = Schedule::Phased.plan(822, 200); // 822 = 54 * 15 + 12: no update
    assert!(sorted_row.updates.is_empty() && sorted_row.sort.is_none());
    let update = Schedule::Phased.plan(817, 200); // 817 mod 15 = 7: one update, row (817 / 15) mod 200
    assert_eq!(update.updates, [(817 / 15)]);
    // Row r after the update of version v: 10 + ((37 r + 101 v) mod 9000) / 100.
    assert_eq!(
        vespertine_bench::workloads::QuoteBook::new(200)
            .row(3)
            .price,
        10.0 + 111.0 / 100.0
    );
}

#[test]
fn dev_003_a5_the_virtual_table_renders_only_the_visible_rows_at_37_px_increments() {
    let _serial = serial();
    let mut harness = Harness::new(text(), meter(), false);
    let (mut driver, probe) = vespertine_bench::workloads::virtual_driver(&mut harness).unwrap();
    for frame in 0..40u64 {
        harness.run_frame(frame, &mut *driver).unwrap();
        // The table has been scrolled `frame + 1` times by 37 logical px.
        let offset = 37.0 * (frame + 1) as f32;
        let first = (offset / 26.0).floor() as usize;
        let rendered = probe.rendered.borrow().clone();
        assert!(!rendered.is_empty(), "frame {frame}");
        assert_eq!(rendered[0], first, "frame {frame}: offset {offset}");
        // The 452 px viewport (480 minus the 28 px header) shows at most 19 rows of 26 px, and
        // the list renders no more than a row beyond them: nowhere near 10,000.
        assert!(
            rendered.len() <= 20,
            "frame {frame}: {} rows rendered",
            rendered.len()
        );
        assert!(
            rendered.len() >= 17,
            "frame {frame}: {} rows rendered",
            rendered.len()
        );
        assert!(rendered.windows(2).all(|pair| pair[1] == pair[0] + 1));
    }
}

// ---------------------------------------------------------------------------------------------
// A6: the same scenario twice in one process is reproducible
// ---------------------------------------------------------------------------------------------

#[test]
fn dev_003_a6_the_same_scenario_twice_gives_identical_verification_and_draw_counts() {
    let _serial = serial();
    for name in ["quotes-sorted", "transcript-160", "workspace-docked"] {
        let workload = workload(name);
        let run = options(2, 18);
        let first = lockstep(&workload, &LockstepOptions::default(), &run);
        let second = lockstep(&workload, &LockstepOptions::default(), &run);
        assert!(first.passed(), "{name}: {:?}", first.mismatch);
        assert_eq!(
            first.digest, second.digest,
            "{name}: identical scene-verification digests"
        );
        assert_eq!(first.frames_compared, second.frames_compared);
        assert_eq!(first.natural_draws, second.natural_draws, "{name}");
        assert_eq!(first.forced_draws, second.forced_draws, "{name}");

        let (text, meter) = (text(), meter());
        let a = run_scenario(&text, &meter, &workload, RetentionMode::On, &run).unwrap();
        let b = run_scenario(&text, &meter, &workload, RetentionMode::On, &run).unwrap();
        assert_eq!(a.natural_draws, b.natural_draws, "{name}");
        assert_eq!(a.forced_draws, b.forced_draws, "{name}");
        assert_eq!(a.stats.views_built, b.stats.views_built, "{name}");
        assert_eq!(a.stats.views_reused, b.stats.views_reused, "{name}");
        assert_eq!(a.stats.nodes_created, b.stats.nodes_created, "{name}");
    }
}
