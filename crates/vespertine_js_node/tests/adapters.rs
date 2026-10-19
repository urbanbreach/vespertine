//! Runs the React and Solid adapter test suites (APP-009, APP-010, DEV-010) under both Node and
//! Bun. The adapters are exercised against the JavaScript reference tree; the native host tests
//! live in `native_adapters.rs`.

#[path = "../harness/mod.rs"]
mod harness;

fn suite(package: &str, program: &str, args: &[&str]) {
    static ADDON: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let addon = ADDON.get_or_init(|| harness::build_addon(false));
    let status = harness::run_package(package, program, args, addon);
    assert!(status.success(), "{package}: {program} {args:?} failed: {status}");
}

#[test]
fn react_adapter_tests_pass_under_node() {
    suite("vespertine-react", "node", &["--test", "test/*.test.js"]);
}

#[test]
fn react_adapter_tests_pass_under_bun() {
    suite("vespertine-react", "bun", &["test"]);
}

#[test]
fn solid_adapter_tests_pass_under_node() {
    suite("vespertine-solid", "node", &["--conditions=browser", "--test", "test/*.test.js"]);
}

#[test]
fn solid_adapter_tests_pass_under_bun() {
    suite("vespertine-solid", "bun", &["test"]);
}
