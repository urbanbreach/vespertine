//! Runs the JavaScript `checkUpdate` tests (APP-004) under Node and Bun against the built
//! addon and a local HTTP server.

#[path = "../harness/mod.rs"]
mod harness;

fn suite(program: &str, args: &[&str]) {
    static ADDON: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let addon = ADDON.get_or_init(|| harness::build_addon(false));
    let status = harness::run(program, args, addon, &[]);
    assert!(status.success(), "{program} {args:?} failed: {status}");
}

#[test]
fn updater_javascript_tests_pass_under_node() {
    suite("node", &["--test", "test/updater.test.js"]);
}

#[test]
fn updater_javascript_tests_pass_under_bun() {
    suite("bun", &["test", "test/updater.test.js"]);
}
