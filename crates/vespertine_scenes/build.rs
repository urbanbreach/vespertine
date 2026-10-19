//! Declares every `src/perf/*.rs` and `src/fid/*.rs` file as a module of `perf` / `fid`, so that
//! scene files can be added without editing a shared module list.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-changed=build.rs");
    for group in ["perf", "fid"] {
        let dir = manifest_dir.join("src").join(group);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
            .map(|entry| entry.expect("directory entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .filter(|path| path.file_stem().is_some_and(|stem| stem != "mod"))
            .collect();
        files.sort();
        let mut modules = String::new();
        for path in files {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("UTF-8 file name");
            let name: String = stem
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            let name = if name.starts_with(|c: char| c.is_ascii_digit()) {
                format!("s{name}")
            } else {
                name
            };
            println!("cargo:rerun-if-changed={}", path.display());
            writeln!(
                modules,
                "#[path = {:?}]\npub mod {name};",
                path.display().to_string()
            )
            .unwrap();
        }
        fs::write(out_dir.join(format!("{group}_modules.rs")), modules).expect("write module list");
    }
}
