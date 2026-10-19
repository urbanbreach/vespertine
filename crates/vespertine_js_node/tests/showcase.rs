//! The curated documentation screenshot (DEV-009 R5): `vespertine-screenshots` runs
//! `cargo test showcase` and expects `target/screenshots/showcase.png`. The test renders the
//! task-list reference surface (APP-011) on the offscreen test host through Node and checks that
//! the PNG exists and has the window's size.

#[path = "../harness/mod.rs"]
mod harness;

use std::process::{Command, Stdio};

#[test]
fn showcase() {
    let addon = harness::build_addon(false);
    harness::install_dependencies();
    let output = harness::workspace_root().join("target/screenshots/showcase.png");
    let _ = std::fs::remove_file(&output);
    let status = Command::new("node")
        .current_dir(harness::packages_dir().join("vespertine-examples"))
        .arg("scripts/showcase.mjs")
        .arg(&output)
        .env("VESPERTINE_NATIVE_ADDON", &addon)
        .stdin(Stdio::null())
        .status()
        .expect("failed to run node");
    match status.code() {
        Some(0) => {}
        Some(2) => {
            eprintln!("showcase: no rendering adapter on this machine; no screenshot written");
            return;
        }
        _ => panic!("showcase.mjs failed: {status}"),
    }
    let bytes = std::fs::read(&output).expect("the showcase screenshot was written");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    // IHDR: width and height are the big-endian integers at offsets 16 and 20.
    let dimension = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!((dimension(16), dimension(20)), (940, 660));
}
