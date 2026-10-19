//! DEV-003 A2: lockstep verification of retained output, with fault injection.
//!
//! Each workload is drawn twice from fresh state, once with retained frames and once with
//! retention disabled, through identical steps; every frame the expanded scene primitives
//! (order, clipping, geometry, color, text layout and what each sprite shows), the hit-test
//! results and the positions seen by listeners are compared. Different atlas packing is
//! tolerated; a single one-device-pixel difference in geometry, color or paint order is not.
//!
//! The example checks both directions and exits non-zero when either is wrong:
//! the unmodified runs must PASS (also with a deliberately different atlas packing), and a
//! seeded fault of each kind must FAIL, on exactly the frame that shows it.
//!
//! ```sh
//! cargo run -p vespertine_bench --example lockstep_verification
//! ```

use std::process::ExitCode;
use vespertine_bench::{
    FaultKind, RunOptions,
    harness::real_text_system,
    select,
    verify::{FaultPlan, Lockstep, LockstepOptions},
};

fn main() -> ExitCode {
    let text_system = real_text_system();
    let run = RunOptions {
        warmup: Some(3),
        frames: Some(21),
        phase_times: false,
    };
    let mut ok = true;
    let mut expect = |condition: bool, what: String| {
        println!("{} {what}", if condition { "ok  " } else { "FAIL" });
        ok &= condition;
    };
    for workload in select(&[
        "quotes-sparse".into(),
        "typing-validation".into(),
        "wheel-scroll".into(),
        "transcript-160".into(),
    ]) {
        for perturb_atlas in [false, true] {
            let report = Lockstep::new()
                .run(
                    &text_system,
                    &workload,
                    &LockstepOptions {
                        perturb_atlas,
                        ..LockstepOptions::default()
                    },
                    &run,
                )
                .expect("lockstep run");
            expect(
                report.passed(),
                format!(
                    "{} agrees on {} frames{} (digest {:016x})",
                    workload.name,
                    report.frames_compared,
                    if perturb_atlas {
                        " with different atlas packing"
                    } else {
                        ""
                    },
                    report.digest
                ),
            );
        }
    }
    let quotes = select(&["quotes-sparse".into()])[0];
    for (index, kind) in FaultKind::ALL.into_iter().enumerate() {
        // A seeded frame for the first kind, fixed frames for the others, so that all three
        // differences are shown.
        let plan = if index == 0 {
            FaultPlan::seeded(7, 24)
        } else {
            FaultPlan {
                frame: 10 + index as u64 * 5,
                kind,
            }
        };
        let plan = FaultPlan { kind, ..plan };
        let report = Lockstep::new()
            .run(
                &text_system,
                &quotes,
                &LockstepOptions {
                    fault: Some(plan),
                    ..LockstepOptions::default()
                },
                &run,
            )
            .expect("lockstep run");
        let caught = report
            .mismatch
            .as_ref()
            .is_some_and(|mismatch| mismatch.frame == plan.frame);
        expect(
            caught,
            format!(
                "a one-pixel {kind:?} fault at frame {} is caught on that frame ({})",
                plan.frame,
                report
                    .mismatch
                    .as_ref()
                    .map_or("not detected".to_owned(), |m| m.check.to_owned())
            ),
        );
    }
    if ok {
        println!("lockstep verification behaves as specified");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
