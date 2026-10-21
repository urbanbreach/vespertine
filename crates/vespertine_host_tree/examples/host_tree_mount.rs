//! `PERF-PRF-015`: the large-mount workload of PRF-015 (A7).
//!
//! 100,000 hosts, 8 style payloads of about 1.4 KiB (two key orders each), a mount batch
//! of 220,000 logical operations, 100 resend frames and 10 destroy frames. It records the
//! allocation count/bytes, peak resident memory (VmHWM) and batch-apply latency of the
//! compact host tree and of a straightforward reference build (whole-batch generic value
//! tree plus a parsed style per node), then verifies that both build the identical tree.
//!
//! Run in release mode, each build in its own process so resident-memory peaks are
//! independent:
//!
//! ```text
//! cargo run --release -p vespertine_host_tree --example host_tree_mount
//! ```
//!
//! `HOST_TREE_MOUNT_HOSTS` scales the host count down for smoke runs. The first argument
//! may be `tree`, `reference` or `verify` to run a single part.

use std::time::Instant;

#[path = "../tests/support/mod.rs"]
mod support;

use support::instrument::{self, AllocStats, CountingAllocator};
use support::reference::ReferenceTree;
use support::workload::{
    FULL_HOSTS, LAST_DESTROY_FRAME, LAST_RESEND_FRAME, Workload, verify_against_reference,
};
use vespertine_host_tree::HostTree;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn proc_status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix(field)?
                    .trim_start_matches(':')
                    .trim()
                    .strip_suffix("kB")?
                    .trim()
                    .parse()
                    .ok()
            })
        })
        .unwrap_or(0)
}

/// Resets the process peak resident size (VmHWM) to the current resident size.
fn reset_peak_rss() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn mib_kb(kb: u64) -> f64 {
    kb as f64 / 1024.0
}

struct Totals {
    count: u64,
    bytes: u64,
    peak_bytes: i64,
}

impl Totals {
    fn new() -> Self {
        Self {
            count: 0,
            bytes: 0,
            peak_bytes: 0,
        }
    }

    fn add(&mut self, stats: AllocStats) {
        self.count += stats.count;
        self.bytes += stats.bytes;
        self.peak_bytes = self.peak_bytes.max(stats.peak_bytes);
    }
}

/// Runs frames 0..=110 with `apply` (returns destroyed count and whether the commit
/// invalidated the window) and prints the measurements.
fn run(
    label: &str,
    workload: &Workload,
    mut apply: impl FnMut(&str) -> (usize, bool),
    report: impl Fn() -> String,
) {
    let mount = workload.mount_batch();
    println!(
        "[{label}] transport: mount batch {:.1} MiB, {} logical ops, {} wire tuples",
        mib(mount.len() as u64),
        workload.mount_logical_ops(),
        workload.mount_wire_ops()
    );
    reset_peak_rss();
    let rss_before = proc_status_kb("VmRSS");

    instrument::reset();
    let started = Instant::now();
    let (_, redraw) = apply(&mount);
    let mount_latency = started.elapsed();
    let mount_alloc = instrument::snapshot();
    let mount_hwm = proc_status_kb("VmHWM");
    println!(
        "[{label}] mount: {:.1} ms, allocations {} ({:.1} MiB total), peak transient {:.1} MiB, \
         peak resident +{:.1} MiB over the transport baseline, redraw={redraw}",
        started_ms(mount_latency),
        mount_alloc.count,
        mib(mount_alloc.bytes),
        mib(mount_alloc.peak_bytes.max(0) as u64),
        mib_kb(mount_hwm.saturating_sub(rss_before)),
    );
    drop(mount);

    let mut resend = Totals::new();
    let mut resend_latencies = Vec::new();
    let mut redraw_commits = 0;
    for frame in 1..=LAST_RESEND_FRAME {
        let batch = workload.resend_batch(frame);
        instrument::reset();
        let started = Instant::now();
        let (_, redraw) = apply(&batch);
        resend_latencies.push(started_ms(started.elapsed()));
        resend.add(instrument::snapshot());
        redraw_commits += redraw as u64;
    }
    println!(
        "[{label}] resend frames 1..=100: mean {:.2} ms, max {:.2} ms, allocations {} ({:.1} MiB total, \
         peak transient {:.2} MiB), window-invalidating commits {redraw_commits}",
        resend_latencies.iter().sum::<f64>() / resend_latencies.len() as f64,
        resend_latencies.iter().cloned().fold(0.0, f64::max),
        resend.count,
        mib(resend.bytes),
        mib(resend.peak_bytes.max(0) as u64),
    );

    let mut destroy = Totals::new();
    let mut destroy_latencies = Vec::new();
    let mut destroyed = 0;
    for frame in 101..=LAST_DESTROY_FRAME {
        let batch = workload.destroy_batch(frame);
        instrument::reset();
        let started = Instant::now();
        let (count, _) = apply(&batch);
        destroy_latencies.push(started_ms(started.elapsed()));
        destroy.add(instrument::snapshot());
        destroyed += count;
    }
    println!(
        "[{label}] destroy frames 101..=110: mean {:.1} ms, max {:.1} ms, destroyed {destroyed}, \
         allocations {} ({:.1} MiB total)",
        destroy_latencies.iter().sum::<f64>() / destroy_latencies.len() as f64,
        destroy_latencies.iter().cloned().fold(0.0, f64::max),
        destroy.count,
        mib(destroy.bytes),
    );
    println!(
        "[{label}] peak resident (VmHWM) over the whole run: {:.1} MiB (+{:.1} MiB over the transport baseline)",
        mib_kb(proc_status_kb("VmHWM")),
        mib_kb(proc_status_kb("VmHWM").saturating_sub(rss_before)),
    );
    println!("[{label}] {}", report());
}

fn started_ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn run_tree(workload: &Workload) {
    let tree = std::cell::RefCell::new(HostTree::new());
    run(
        "tree",
        workload,
        |batch| {
            let result = tree.borrow_mut().apply_batch(batch).expect("batch applies");
            (result.destroyed.len(), result.redraw)
        },
        || {
            let tree = tree.borrow();
            let stats = tree.stats();
            format!(
                "final: {} live elements, {} interned styles, {} sweeps, {} style parses, \
                 {} redraw commits of {} batches",
                stats.live_elements,
                stats.style.interned,
                stats.style.sweeps,
                stats.style.parses,
                stats.redraw_commits,
                stats.batches_applied,
            )
        },
    );
}

fn run_reference(workload: &Workload) {
    let tree = std::cell::RefCell::new(ReferenceTree::new());
    run(
        "reference",
        workload,
        |batch| {
            let destroyed = tree.borrow_mut().apply_batch(batch).expect("batch applies");
            (destroyed.len(), true)
        },
        || format!("final: {} live elements", tree.borrow().len()),
    );
}

fn run_verify(hosts: u64) {
    let started = Instant::now();
    match verify_against_reference(hosts) {
        Ok(report) => println!(
            "[verify] tree identical to the reference build after the mount, frame {LAST_RESEND_FRAME} \
             and frame {LAST_DESTROY_FRAME} ({:.1} s): resend redraw commits {}, destroyed {}, \
             {} live elements, {} interned styles",
            started.elapsed().as_secs_f64(),
            report.resend_redraw_commits,
            report.destroyed,
            report.final_live_elements,
            report.final_interned,
        ),
        Err(error) => {
            eprintln!("[verify] MISMATCH: {error}");
            std::process::exit(1);
        }
    }
}

#[allow(clippy::disallowed_methods)]
fn run_in_child(part: &str) {
    let status = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .arg(part)
        .status()
        .expect("child process");
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn main() {
    let hosts = std::env::var("HOST_TREE_MOUNT_HOSTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(FULL_HOSTS);
    match std::env::args().nth(1).as_deref() {
        Some("tree") => run_tree(&Workload::new(hosts)),
        Some("reference") => run_reference(&Workload::new(hosts)),
        Some("verify") => run_verify(hosts),
        Some(other) => {
            eprintln!("unknown part {other:?}; expected tree, reference or verify");
            std::process::exit(2);
        }
        None => {
            println!("PERF-PRF-015 workload: {hosts} hosts");
            for part in ["tree", "reference", "verify"] {
                run_in_child(part);
            }
        }
    }
}
