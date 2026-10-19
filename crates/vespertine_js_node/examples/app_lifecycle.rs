//! APP-005 A1/A2 on a real display server: drives the JavaScript examples against the native
//! addon, once on Wayland (when `WAYLAND_DISPLAY` is set) and once on X11 (when `DISPLAY` is
//! set, with Wayland hidden from the process), then runs the busy-JavaScript example (Bun).
//!
//! ```text
//! cargo run --release -p vespertine_js_node --example app_lifecycle
//! ```

#[path = "../harness/mod.rs"]
mod harness;

fn main() {
    let addon = harness::build_addon(!cfg!(debug_assertions));
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    assert!(
        wayland || x11,
        "no display server: set WAYLAND_DISPLAY or DISPLAY"
    );

    let mut failed = false;
    let mut check = |label: &str, program: &str, args: &[&str], unset: &[&str]| {
        println!("== {label}");
        let status = harness::run(program, args, &addon, unset);
        if !status.success() {
            eprintln!("{label} failed: {status}");
            failed = true;
        }
    };

    if wayland {
        check(
            "lifecycle (Wayland, node)",
            "node",
            &["examples/lifecycle.mjs"],
            &[],
        );
    }
    if x11 {
        // Dropping WAYLAND_DISPLAY makes the platform connect through X11.
        check(
            "lifecycle (X11, node)",
            "node",
            &["examples/lifecycle.mjs"],
            &["WAYLAND_DISPLAY"],
        );
    } else {
        println!("== X11 pass skipped: DISPLAY is not set");
    }
    // A1: the window paints while JavaScript is blocked, and tick reports false on close.
    check(
        "busy JavaScript (bun)",
        "bun",
        &["examples/busy_window.mjs", "1"],
        &[],
    );
    if failed {
        std::process::exit(1);
    }
}
