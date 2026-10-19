//! Runs the test suites of the development tooling packages (DEV-009) under Node and Bun:
//! `create-vespertine` (project creation) and `vespertine-dev` (rebuild watcher, screenshot
//! collector). They are pure JavaScript and need no native addon.

#[path = "../harness/mod.rs"]
mod harness;

use std::path::Path;

fn suite(package: &str, program: &str, args: &[&str]) {
    // The harness always selects an addon through the environment; these packages never load one.
    let status = harness::run_package(
        package,
        program,
        args,
        Path::new("/nonexistent/no-addon.node"),
    );
    assert!(
        status.success(),
        "{program} {args:?} failed in {package}: {status}"
    );
}

#[test]
fn create_vespertine_tests_pass_under_node() {
    suite("create-vespertine", "node", &["--test", "test/*.test.js"]);
}

#[test]
fn create_vespertine_tests_pass_under_bun() {
    suite("create-vespertine", "bun", &["test"]);
}

#[test]
fn vespertine_dev_tests_pass_under_node() {
    suite("vespertine-dev", "node", &["--test", "test/*.test.js"]);
}

#[test]
fn vespertine_dev_tests_pass_under_bun() {
    suite("vespertine-dev", "bun", &["test"]);
}
