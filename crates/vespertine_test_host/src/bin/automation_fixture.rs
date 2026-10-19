//! A launchable application that serves a [`TestHost`] showing the fixture tree over standard
//! input and output with the automation protocol. Used by the DEV-007 tests and as the reference
//! for how a host application exposes the contract: build the renderer, then serve it.
//!
//! Environment: `FIXTURE_WIDTH` / `FIXTURE_HEIGHT` (logical pixels; default 400 x 300) and
//! `FIXTURE_ECHO_ENV` (the name of an environment variable whose value is shown as text, so a
//! launcher can prove its environment was merged) and `FIXTURE_ECHO_CWD` (show the working
//! directory instead).

use serde_json::json;
use vespertine_test_host::{
    HostOptions, RetainedHost, TestHost,
    automation::{AutomationHost, serve_stdio},
    fixture::FixtureTree,
};

fn main() -> std::process::ExitCode {
    let size = |name: &str, default: f64| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let options =
        HostOptions::with_size(size("FIXTURE_WIDTH", 400.0), size("FIXTURE_HEIGHT", 300.0));
    let mut tree = FixtureTree::new();
    let echo = if std::env::var_os("FIXTURE_ECHO_CWD").is_some() {
        std::env::current_dir()
            .ok()
            .map(|dir| dir.display().to_string())
    } else {
        std::env::var("FIXTURE_ECHO_ENV")
            .ok()
            .and_then(|name| std::env::var(name).ok())
    };
    let batch = vespertine_test_host::fixture::scenario_batch(echo);
    if let Err(error) = tree.apply_batch(&batch.to_string()) {
        eprintln!("fixture batch rejected: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let mut host = match TestHost::with_tree(options, tree) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("cannot create the test host: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    host.flush();
    // Plain log lines may share the stream with frames; they are never mistaken for messages.
    println!("id: 7");
    println!("event: log");
    println!("fixture ready: {}", json!({ "pid": std::process::id() }));
    let host: &mut dyn AutomationHost = &mut host;
    match serve_stdio(host) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("serve failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
