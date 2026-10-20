//! Regression tests for the repository's gate scripts (`scripts/third-party`,
//! `scripts/check-features`): each loads the script as a module and drives one function with a
//! controlled environment.

use std::path::Path;

/// Loads the script named by `sys.argv[1]` (no `.py` extension) as the module `module`.
const LOAD: &str = r#"
import importlib.machinery, importlib.util, os, sys, tempfile
loader = importlib.machinery.SourceFileLoader("gate_script", sys.argv[1])
spec = importlib.util.spec_from_loader("gate_script", loader)
module = importlib.util.module_from_spec(spec)
loader.exec_module(module)
"#;

fn run_python(script: &str, body: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    #[allow(
        clippy::disallowed_methods,
        reason = "a test that runs a repository script's functions and waits for them"
    )]
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!("{LOAD}{body}"))
        .arg(root.join(script))
        .current_dir(&root)
        .output()
        .expect("python3 runs");
    assert!(
        output.status.success(),
        "{script} check failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_license_audit_falls_back_to_the_lockfile_when_npm_cannot_be_launched() {
    run_python(
        "scripts/third-party",
        r#"
import shutil
with tempfile.TemporaryDirectory() as empty:
    os.environ["PATH"] = empty
    assert shutil.which("npm") is None
    rows, first_party = module.npm_packages()
assert rows, "the lockfile lists third-party packages"
assert first_party, "the lockfile lists first-party packages"
"#,
    );
}

#[test]
fn a_test_executable_that_exits_unsuccessfully_fails_its_selected_rows() {
    run_python(
        "scripts/check-features",
        r##"
def run_rows(exit_code):
    with tempfile.TemporaryDirectory() as tmp:
        executable = os.path.join(tmp, "tests")
        with open(executable, "w") as handle:
            handle.write(f"#!/bin/sh\necho 'test one ... ok'\necho 'test two ... ok'\nexit {exit_code}\n")
        os.chmod(executable, 0o755)
        module.build_test_executables = lambda *args: {"krate": [executable]}
        module.list_tests = lambda exe, manifest_dir, *extra: set() if extra else {"one", "two"}
        rows = [
            module.Row(1, "RND-001", "A1", [("test", "krate", "one")]),
            module.Row(2, "RND-002", "A1", [("test", "krate", "one"), ("test", "krate", "two")]),
        ]
        packages = {"krate": {"id": "krate 0.1.0", "manifest_path": os.path.join(tmp, "Cargo.toml")}}
        module.check_tests(rows, packages, True)
        return rows

for row in run_rows(0):
    assert not row.errors, row.errors
for row in run_rows(101):
    assert row.errors, "result lines say ok but the executable failed"
    assert all("status 101" in error for error in row.errors), row.errors
"##,
    );
}
