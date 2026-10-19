#![allow(clippy::disallowed_methods)]
//! Compiles the WebKitGTK helper executable (`helper/*.c`) and exposes its path to the crate,
//! which embeds the bytes (PLT-014 R3/R4).

use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

const MODULES: [&str; 2] = ["webkit2gtk-4.1", "json-glib-1.0"];

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1);
}

fn pkg_config(argument: &str) -> Vec<String> {
    let output = Command::new("pkg-config")
        .arg(argument)
        .args(MODULES)
        .output()
        .unwrap_or_else(|error| {
            fail(&format!(
                "could not run pkg-config ({error}); the embedded web view helper needs the \
                 pkg-config modules {} and {}. Install the development packages of your \
                 distribution: libwebkit2gtk-4.1-dev and libjson-glib-dev (Debian/Ubuntu) or \
                 webkit2gtk4.1-devel and json-glib-devel (Fedora).",
                MODULES[0], MODULES[1]
            ))
        });
    if !output.status.success() {
        fail(&format!(
            "the pkg-config modules {} and {} were not found ({}); the embedded web view helper \
             needs them to build. Install the development packages of your distribution: \
             libwebkit2gtk-4.1-dev and libjson-glib-dev (Debian/Ubuntu) or webkit2gtk4.1-devel \
             and json-glib-devel (Fedora).",
            MODULES[0],
            MODULES[1],
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn main() {
    println!("cargo:rerun-if-env-changed=VSP_WEBKIT_HELPER_BINARY");
    if let Some(prebuilt) = env::var_os("VSP_WEBKIT_HELPER_BINARY") {
        // Packagers and developers may embed an already built helper instead of compiling one.
        println!(
            "cargo:rerun-if-changed={}",
            PathBuf::from(&prebuilt).display()
        );
        println!(
            "cargo:rustc-env=VSP_WEBKIT_HELPER={}",
            PathBuf::from(prebuilt).display()
        );
        return;
    }
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let helper_dir = manifest_dir.join("helper");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", helper_dir.display());

    let mut sources: Vec<PathBuf> = fs::read_dir(&helper_dir)
        .unwrap_or_else(|error| fail(&format!("{}: {error}", helper_dir.display())))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "c"))
        .collect();
    sources.sort();
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    for header in fs::read_dir(&helper_dir).into_iter().flatten().flatten() {
        println!("cargo:rerun-if-changed={}", header.path().display());
    }

    let cflags = pkg_config("--cflags");
    let libs = pkg_config("--libs");
    let helper = out_dir.join("vsp-webkit-helper");
    let compiler = env::var("CC").unwrap_or_else(|_| "cc".to_owned());
    let status = Command::new(&compiler)
        .args([
            "-std=c11",
            "-D_GNU_SOURCE",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Wno-unused-parameter",
        ])
        .arg("-I")
        .arg(&helper_dir)
        .args(&cflags)
        .args(&sources)
        .arg("-o")
        .arg(&helper)
        .args(&libs)
        .status()
        .unwrap_or_else(|error| fail(&format!("could not run the C compiler {compiler}: {error}")));
    if !status.success() {
        fail("compiling the web view helper failed");
    }
    println!("cargo:rustc-env=VSP_WEBKIT_HELPER={}", helper.display());
}
