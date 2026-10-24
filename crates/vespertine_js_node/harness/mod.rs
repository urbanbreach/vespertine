//! Shared by the tests and examples of this crate: builds the addon and runs JavaScript
//! with Node and Bun against it.

#![allow(
    dead_code,
    reason = "each test and example uses a subset of this shared harness"
)]
#![allow(
    clippy::disallowed_methods,
    reason = "the harness runs cargo, Node and Bun as child processes"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

pub fn package_dir() -> PathBuf {
    packages_dir().join("vespertine-host")
}

/// The npm workspace holding every JavaScript package.
pub fn packages_dir() -> PathBuf {
    workspace_root().join("packages")
}

/// Holds an exclusive lock, shared by every test process of this workspace, while the harness
/// installs dependencies or builds and installs the addon. Nextest runs each test in its own
/// process, so the in-process `OnceLock`s alone would let two `npm install`s run in one
/// workspace at once.
fn setup_lock() -> std::fs::File {
    let workspace: String = workspace_root()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let path = std::env::temp_dir().join(format!("vespertine-js-harness{workspace}.lock"));
    let file = std::fs::File::create(&path).expect("create the harness lock file");
    file.lock().expect("lock the harness lock file");
    file
}

/// Installs the npm workspace dependencies (once per process; npm keeps its own cache, so a
/// repeated install is quick and works offline once the packages were fetched).
pub fn install_dependencies() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        let _lock = setup_lock();
        let status = Command::new("npm")
            .current_dir(packages_dir())
            .args(["install", "--no-audit", "--no-fund", "--prefer-offline"])
            .stdin(Stdio::null())
            .status()
            .expect("failed to run npm");
        assert!(status.success(), "npm install failed: {status}");
    });
}

/// Runs `program args...` inside `packages/<package>` with the addon selected through the
/// environment (dependencies are installed first).
pub fn run_package(
    package: &str,
    program: &str,
    args: &[&str],
    addon: &Path,
) -> std::process::ExitStatus {
    install_dependencies();
    Command::new(program)
        .current_dir(packages_dir().join(package))
        .args(args)
        .env("VESPERTINE_NATIVE_ADDON", addon)
        .stdin(Stdio::null())
        .status()
        .unwrap_or_else(|error| panic!("failed to run {program}: {error}"))
}

/// Builds the addon, with the offscreen test renderer the JavaScript tests drive, with cargo and
/// installs it as `<target>/vespertine-js/vespertine_js.node` (the extension Node requires to
/// load a native module). Returns its path. The install renames a fresh copy over the old file,
/// so a runtime of a concurrent test process that has the old addon loaded keeps its mapping.
pub fn build_addon(release: bool) -> PathBuf {
    let _lock = setup_lock();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace_root())
        .args([
            "build",
            "-p",
            "vespertine_js_node",
            "--features",
            "offscreen",
            "--message-format=json",
        ])
        .stderr(Stdio::inherit());
    if release {
        command.arg("--release");
    }
    let output = command.output().expect("failed to run cargo");
    assert!(
        output.status.success(),
        "building vespertine_js_node failed"
    );
    let stdout = String::from_utf8(output.stdout).expect("cargo output is UTF-8");
    let library = stdout
        .lines()
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| {
            message["reason"] == "compiler-artifact"
                && message["target"]["name"] == "vespertine_js_node"
        })
        .flat_map(|message| message["filenames"].as_array().cloned().unwrap_or_default())
        .filter_map(|name| name.as_str().map(PathBuf::from))
        .find(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("so" | "dylib" | "dll")
            )
        })
        .expect("cargo reported no vespertine_js_node library");
    let directory = library
        .parent()
        .expect("artifact directory")
        .join("vespertine-js");
    std::fs::create_dir_all(&directory).expect("create addon directory");
    let addon = directory.join("vespertine_js.node");
    let staged = directory.join(format!("vespertine_js.node.{}", std::process::id()));
    std::fs::copy(&library, &staged).expect("stage the addon");
    std::fs::rename(&staged, &addon).expect("install the addon");
    addon
}

/// Runs `program args...` in the package directory with the addon selected through the
/// environment. `unset` removes variables (used to force X11 by dropping `WAYLAND_DISPLAY`).
pub fn run(program: &str, args: &[&str], addon: &Path, unset: &[&str]) -> std::process::ExitStatus {
    install_dependencies();
    let mut command = Command::new(program);
    command
        .current_dir(package_dir())
        .args(args)
        .env("VESPERTINE_NATIVE_ADDON", addon)
        .stdin(Stdio::null());
    for name in unset {
        command.env_remove(name);
    }
    command
        .status()
        .unwrap_or_else(|error| panic!("failed to run {program}: {error}"))
}
