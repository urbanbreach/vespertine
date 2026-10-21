//! `vsp-bench`: the headless frame-cost benchmark and lockstep verifier (DEV-003).

use anyhow::{Result, anyhow, bail};
use std::{cell::RefCell, process::ExitCode, rc::Rc};
use vespertine_bench::{
    Meter, RetentionMode, RunOptions, catalogue,
    harness::real_text_system,
    platform_report::PlatformReport,
    report, run_scenario, select,
    verify::{FaultPlan, Lockstep, LockstepOptions},
};

const USAGE: &str = "\
usage: vsp-bench [FILTER ...] [options]

Runs the deterministic workloads whose name equals or contains a FILTER (all when none).

options:
  --list                 list the workloads and exit
  --frames N             measured frames (default: the workload's, normally 200)
  --warmup N             warm-up frames (default: the workload's, normally 30)
  --retention on|off|both  retained frames on, off, or both (default both)
  --verify               also run lockstep verification (retained against full render)
  --verify-only          run lockstep verification instead of measuring
  --fault SEED           with --verify: place a seeded one-pixel fault (the run must FAIL)
  --a11y                 with --verify: also compare accessibility trees
  --pixels               with --verify: also compare GPU-rendered pixels
  --perturb-atlas        with --verify: give the retained window a different atlas packing history
  --phase-times          enable the phase and shaping timers (counters always run)
  --json                 machine-readable output
  -h, --help             this text
";

#[derive(Default)]
struct Cli {
    filters: Vec<String>,
    list: bool,
    frames: Option<u64>,
    warmup: Option<u64>,
    retention: Vec<RetentionMode>,
    verify: bool,
    verify_only: bool,
    fault: Option<u64>,
    a11y: bool,
    pixels: bool,
    perturb_atlas: bool,
    phase_times: bool,
    json: bool,
}

fn number(args: &mut impl Iterator<Item = String>, name: &str) -> Result<u64> {
    args.next()
        .ok_or_else(|| anyhow!("{name} needs a value"))?
        .parse()
        .map_err(|_| anyhow!("{name} needs a whole number"))
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Cli>> {
    let mut cli = Cli::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--list" => cli.list = true,
            "--frames" => cli.frames = Some(number(&mut args, "--frames")?),
            "--warmup" => cli.warmup = Some(number(&mut args, "--warmup")?),
            "--retention" => {
                cli.retention = match args.next().as_deref() {
                    Some("on") => vec![RetentionMode::On],
                    Some("off") => vec![RetentionMode::Off],
                    Some("both") => vec![RetentionMode::On, RetentionMode::Off],
                    _ => bail!("--retention takes on, off or both"),
                }
            }
            "--verify" => cli.verify = true,
            "--verify-only" => cli.verify_only = true,
            "--fault" => cli.fault = Some(number(&mut args, "--fault")?),
            "--a11y" => cli.a11y = true,
            "--pixels" => cli.pixels = true,
            "--perturb-atlas" => cli.perturb_atlas = true,
            "--phase-times" => cli.phase_times = true,
            "--json" => cli.json = true,
            other if other.starts_with('-') => bail!("unknown option {other}\n\n{USAGE}"),
            filter => cli.filters.push(filter.to_owned()),
        }
    }
    if cli.retention.is_empty() {
        cli.retention = vec![RetentionMode::On, RetentionMode::Off];
    }
    if cli.fault.is_some() && !(cli.verify || cli.verify_only) {
        bail!("--fault needs --verify or --verify-only");
    }
    Ok(Some(cli))
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("vsp-bench: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let Some(cli) = parse(std::env::args().skip(1))? else {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    };
    if cli.list {
        for workload in catalogue() {
            println!(
                "{:24} {:?}: {}",
                workload.name, workload.family, workload.description
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    let workloads = select(&cli.filters);
    if workloads.is_empty() {
        bail!("no workload matches {:?}; --list shows them", cli.filters);
    }
    let options = RunOptions {
        warmup: cli.warmup,
        frames: cli.frames,
        phase_times: cli.phase_times,
    };
    let text_system = real_text_system();
    let meter = Rc::new(RefCell::new(Meter::new()));
    let instructions = match meter.borrow().instructions_unavailable() {
        None => Ok(()),
        Some(reason) => Err(reason.to_owned()),
    };
    let platform =
        PlatformReport::collect(instructions.as_ref().map(|_| ()).map_err(String::as_str));
    let mut json = Vec::new();
    if !cli.json {
        print!("{}", platform.text());
    }
    let mut failed = false;
    for workload in &workloads {
        if !cli.verify_only {
            for mode in &cli.retention {
                let report = run_scenario(&text_system, &meter, workload, *mode, &options)?;
                if cli.json {
                    json.push(report::scenario_json(&report));
                } else {
                    println!("{}", report::scenario_text(&report));
                }
            }
        }
        if cli.verify || cli.verify_only {
            let frames = options.frames.unwrap_or(workload.frames)
                + options.warmup.unwrap_or(workload.warmup);
            let report = Lockstep::new().run(
                &text_system,
                workload,
                &LockstepOptions {
                    frames: None,
                    accessibility: cli.a11y,
                    pixels: cli.pixels,
                    fault: cli.fault.map(|seed| FaultPlan::seeded(seed, frames)),
                    perturb_atlas: cli.perturb_atlas,
                },
                &options,
            )?;
            // With a seeded fault the run is expected to fail; a pass would mean the
            // verification missed a one-pixel difference.
            failed |= report.passed() == cli.fault.is_some();
            if cli.json {
                json.push(report::verify_json(&report));
            } else {
                print!("{}", report::verify_text(&report));
            }
        }
    }
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "platform": platform.text(),
                "results": json,
            }))?
        );
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
