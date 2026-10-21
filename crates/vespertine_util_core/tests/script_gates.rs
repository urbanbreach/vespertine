//! Regression test for the repository's license audit script (`scripts/third-party`): it loads the
//! script as a module and drives one function with a controlled environment.

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
