//! APP-005 A5 on a layer-shell-capable compositor (Hyprland): opens a layer-shell surface per
//! case (each layer, anchor combinations including opposite-edge stretch, margins, exclusive
//! zone with and without an exclusive edge, keyboard interactivity) and checks the
//! compositor's placement through `hyprctl`.
//!
//! ```text
//! cargo run --release -p vespertine_js_node --example layer_shell
//! ```

#![allow(
    clippy::disallowed_methods,
    reason = "the example drives Node child processes"
)]

#[path = "../harness/mod.rs"]
mod harness;

use std::process::{Child, Command, Stdio};

fn spawn(index: usize, addon: &std::path::Path) -> Child {
    Command::new("node")
        .current_dir(harness::package_dir())
        .args(["examples/layer_shell.mjs", &index.to_string()])
        .env("VESPERTINE_NATIVE_ADDON", addon)
        .stdin(Stdio::null())
        .spawn()
        .expect("failed to run node")
}

fn main() {
    let addon = harness::build_addon(!cfg!(debug_assertions));
    let listing = Command::new("node")
        .current_dir(harness::package_dir())
        .args(["examples/layer_shell.mjs", "--list"])
        .stderr(Stdio::inherit())
        .output()
        .expect("failed to run node");
    let changes_reserved: Vec<bool> =
        serde_json::from_slice(&listing.stdout).expect("case listing");
    let count = changes_reserved.len();

    // Cases that leave the reserved area alone do not disturb each other, so they run
    // together; the cases that reserve space run one after another afterwards.
    let mut failures = 0;
    let mut wait = |index: usize, mut child: Child| {
        let status = child.wait().expect("wait for node");
        if !status.success() {
            eprintln!("layer-shell case {index} failed: {status}");
            failures += 1;
        }
    };
    let parallel: Vec<(usize, Child)> = (0..count)
        .filter(|index| !changes_reserved[*index])
        .map(|index| (index, spawn(index, &addon)))
        .collect();
    for (index, child) in parallel {
        wait(index, child);
    }
    for index in (0..count).filter(|index| changes_reserved[*index]) {
        wait(index, spawn(index, &addon));
    }
    if failures > 0 {
        eprintln!("{failures} of {count} layer-shell cases failed");
        std::process::exit(1);
    }
    println!("all {count} layer-shell cases passed");
}
