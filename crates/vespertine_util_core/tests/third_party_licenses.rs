//! The dependency license policy (THIRD_PARTY.md): every crate and npm package of the workspace
//! must be available under Apache-2.0, MIT, BSD, ISC, Zlib, Unicode or OFL (fonts); nothing
//! copyleft. `scripts/third-party` applies the policy to `cargo tree`/`cargo metadata` and the npm
//! lockfile; this test runs it so that a dependency that breaks the policy fails the landing gates.

use std::path::Path;

#[test]
fn every_dependency_is_available_under_a_permitted_license() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    #[allow(
        clippy::disallowed_methods,
        reason = "a test that runs the repository's audit script and waits for it"
    )]
    let output = std::process::Command::new("python3")
        .arg(root.join("scripts/third-party"))
        .arg("--check")
        .current_dir(&root)
        .output()
        .expect("python3 runs scripts/third-party");
    assert!(
        output.status.success(),
        "license policy violated (see THIRD_PARTY.md for the policy):\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn third_party_md_lists_what_the_audit_finds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join("THIRD_PARTY.md")).expect("THIRD_PARTY.md exists");
    for section in [
        "## Policy",
        "## Vendored crates",
        "## Bundled fonts",
        "## Data bundled by dependencies",
        "## npm packages",
        "## Rust crates",
    ] {
        assert!(text.contains(section), "missing section {section}");
    }
    // Every vendored crate has a directory with its own license text.
    for entry in std::fs::read_dir(root.join("third_party")).expect("third_party exists") {
        let entry = entry.expect("directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            text.contains(&format!("| {name} |")),
            "third_party/{name} is not listed in THIRD_PARTY.md"
        );
        let has_license = std::fs::read_dir(entry.path())
            .expect("vendored crate directory")
            .filter_map(Result::ok)
            .any(|file| {
                let file = file.file_name().to_string_lossy().to_uppercase();
                file.starts_with("LICENSE") || file.starts_with("COPYING")
            });
        assert!(has_license, "third_party/{name} has no license file");
    }
}
