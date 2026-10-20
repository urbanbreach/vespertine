//! Runs the JavaScript package's test suites (APP-007, APP-008 and the headless addon
//! checks of APP-005/006) under both Node and Bun.

#[path = "../harness/mod.rs"]
mod harness;

fn suite(program: &str, args: &[&str]) {
    static ADDON: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let addon = ADDON.get_or_init(|| harness::build_addon(false));
    let status = harness::run(program, args, addon, &[]);
    assert!(status.success(), "{program} {args:?} failed: {status}");
}

#[test]
fn javascript_tests_pass_under_node() {
    suite("node", &["--test", "test/*.test.js"]);
}

#[test]
fn javascript_tests_pass_under_bun() {
    suite("bun", &["test", "--timeout", "30000"]);
}
