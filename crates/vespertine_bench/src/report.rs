//! Text and JSON rendering of runner results.

use crate::metrics::Metric;
use crate::runner::ScenarioReport;
use crate::verify::VerifyReport;
use serde_json::{Value, json};

fn metric_json(metric: &Metric, scale: f64) -> Value {
    match metric {
        Metric::Measured(d) => json!({
            "mean": d.mean / scale,
            "median": d.median / scale,
            "p95": d.p95 / scale,
            "max": d.max / scale,
        }),
        Metric::Unavailable(reason) => json!({ "unavailable": reason }),
    }
}

/// The report of one run as text.
pub fn scenario_text(report: &ScenarioReport) -> String {
    let stats = &report.stats;
    let mut out = format!(
        "{} [{}] warmup {} frames {}\n",
        report.name,
        report.mode.label(),
        report.warmup,
        report.frames
    );
    out.push_str(&format!(
        "  cpu (thread):  {}\n",
        report.cpu().describe(1e6, "ms")
    ));
    out.push_str(&format!(
        "  wall:          {}\n",
        report.wall().describe(1e6, "ms")
    ));
    out.push_str(&format!(
        "  allocations:   {}\n",
        report.allocations().describe(1.0, "calls")
    ));
    out.push_str(&format!(
        "  allocated:     {}\n",
        report.allocated_bytes().describe(1024.0, "KiB")
    ));
    out.push_str(&format!(
        "  instructions:  {}\n",
        report.instructions().describe(1e3, "k")
    ));
    out.push_str(&format!(
        "  draws:         {} natural, {} forced (counted separately)\n",
        report.natural_draws, report.forced_draws
    ));
    out.push_str(&format!(
        "  views:         built {} reused {} spliced {} (splice failures {})\n",
        stats.views_built, stats.views_reused, stats.views_spliced, stats.splice_failures
    ));
    out.push_str(&format!(
        "  layout:        nodes created {} reused {}, style writes {}, context writes {}, measurements {}, rebinds {}\n",
        stats.nodes_created,
        stats.nodes_reused,
        stats.style_writes,
        stats.context_writes,
        stats.measurements,
        stats.rebinds
    ));
    out.push_str(&format!(
        "  text:          lines shaped {}\n",
        stats.lines_shaped
    ));
    out.push_str(&format!(
        "  layers:        composites {} repaints {} dirty tiles {} rasterized tiles {} input rebuilds {}\n",
        stats.layer_composites,
        stats.layer_repaints,
        stats.tiles_dirty,
        stats.tiles_rasterized,
        stats.input_layer_rebuilds
    ));
    out.push_str(&format!(
        "  list rows:     built {} replayed {} measured {}\n",
        stats.rows_built, stats.rows_replayed, stats.rows_measured
    ));
    if !stats.frame_time.is_zero() {
        out.push_str(&format!(
            "  phase times:   frame {:.3} build {:.3} prepaint {:.3} paint {:.3} layout {:.3} shaping {:.3} ms (totals)\n",
            stats.frame_time.as_secs_f64() * 1e3,
            stats.build_time.as_secs_f64() * 1e3,
            stats.prepaint_time.as_secs_f64() * 1e3,
            stats.paint_time.as_secs_f64() * 1e3,
            stats.layout_time.as_secs_f64() * 1e3,
            stats.shaping_time.as_secs_f64() * 1e3
        ));
    }
    out
}

/// The report of one run as JSON.
pub fn scenario_json(report: &ScenarioReport) -> Value {
    let stats = &report.stats;
    json!({
        "workload": report.name,
        "retention": report.mode.label(),
        "warmup": report.warmup,
        "frames": report.frames,
        "cpu_ms": metric_json(&report.cpu(), 1e6),
        "wall_ms": metric_json(&report.wall(), 1e6),
        "allocations": metric_json(&report.allocations(), 1.0),
        "allocated_bytes": metric_json(&report.allocated_bytes(), 1.0),
        "instructions": metric_json(&report.instructions(), 1.0),
        "natural_draws": report.natural_draws,
        "forced_draws": report.forced_draws,
        "stats": {
            "views_built": stats.views_built,
            "views_reused": stats.views_reused,
            "views_spliced": stats.views_spliced,
            "splice_failures": stats.splice_failures,
            "nodes_created": stats.nodes_created,
            "nodes_reused": stats.nodes_reused,
            "style_writes": stats.style_writes,
            "context_writes": stats.context_writes,
            "measurements": stats.measurements,
            "rebinds": stats.rebinds,
            "lines_shaped": stats.lines_shaped,
            "layer_composites": stats.layer_composites,
            "layer_repaints": stats.layer_repaints,
            "tiles_dirty": stats.tiles_dirty,
            "tiles_rasterized": stats.tiles_rasterized,
            "input_layer_rebuilds": stats.input_layer_rebuilds,
            "rows_built": stats.rows_built,
            "rows_replayed": stats.rows_replayed,
            "rows_measured": stats.rows_measured,
        },
    })
}

/// A verification report as text.
pub fn verify_text(report: &VerifyReport) -> String {
    let mut out = format!(
        "{} lockstep verification: {} frames compared ({}), digest {:016x}, {} natural / {} forced draws: ",
        report.workload,
        report.frames_compared,
        report.checks.join(", "),
        report.digest,
        report.natural_draws,
        report.forced_draws
    );
    match &report.mismatch {
        None => out.push_str("PASS\n"),
        Some(mismatch) => {
            // Primitive descriptions are long: show the start of each line, JSON has it all.
            let detail: Vec<String> = mismatch
                .detail
                .lines()
                .map(|line| match line.char_indices().nth(240) {
                    Some((end, _)) => format!("{}...", &line[..end]),
                    None => line.to_owned(),
                })
                .collect();
            out.push_str(&format!(
                "FAIL at frame {} ({}):\n  {}\n",
                mismatch.frame,
                mismatch.check,
                detail.join("\n  ")
            ));
        }
    }
    out
}

/// A verification report as JSON.
pub fn verify_json(report: &VerifyReport) -> Value {
    json!({
        "workload": report.workload,
        "frames_compared": report.frames_compared,
        "checks": report.checks,
        "passed": report.passed(),
        "digest": format!("{:016x}", report.digest),
        "natural_draws": report.natural_draws,
        "forced_draws": report.forced_draws,
        "mismatch": report.mismatch.as_ref().map(|mismatch| json!({
            "frame": mismatch.frame,
            "check": mismatch.check,
            "detail": mismatch.detail,
        })),
    })
}
