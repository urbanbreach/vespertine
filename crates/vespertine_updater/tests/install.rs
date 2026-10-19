//! Download, verification and installation (APP-004 A3, A4): the real Linux replacement path
//! over temporary directories, with the process environment injected.

mod support;

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use support::*;
use vespertine_updater::{CheckOptions, Error, UpdateFormat};

const OLD: &[u8] = b"OLD-EXECUTABLE";

/// Temporary directories laid out like a user's machine.
struct Harness {
    _directory: tempfile::TempDir,
    bin: PathBuf,
    tmp: PathBuf,
    cache: PathBuf,
    home: PathBuf,
    target: PathBuf,
}

impl Harness {
    fn new(mode: u32) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let [bin, tmp, cache, home] = ["bin", "tmp", "cache", "home"].map(|name| root.join(name));
        for path in [&bin, &tmp, &cache, &home] {
            std::fs::create_dir(path).unwrap();
        }
        let target = write_executable(&bin, "app.AppImage", OLD, mode);
        Self {
            _directory: directory,
            bin,
            tmp,
            cache,
            home,
            target,
        }
    }

    /// APPIMAGE, the system temporary directory and XDG_CACHE_HOME are all set.
    fn environment(&self) -> TestEnvironment {
        TestEnvironment::new("/nonexistent/running-executable")
            .var("APPIMAGE", &self.target)
            .var("XDG_CACHE_HOME", &self.cache)
            .temp_dir(&self.tmp)
    }

    fn snapshot(&self) -> (Vec<u8>, u32, u64) {
        (
            std::fs::read(&self.target).unwrap(),
            mode_of(&self.target),
            std::fs::metadata(&self.target).unwrap().ino(),
        )
    }

    /// Every directory a staging directory could have been created in.
    fn roots(&self) -> Vec<PathBuf> {
        vec![
            self.tmp.clone(),
            self.cache.clone(),
            self.home.join(".cache"),
            self.bin.clone(),
        ]
    }

    fn assert_no_staging_data(&self) {
        for root in self.roots() {
            assert_eq!(
                staging_leftovers(&root),
                Vec::<String>::new(),
                "{}",
                root.display()
            );
        }
        let entries = |dir: &Path| std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0);
        assert_eq!(entries(&self.tmp), 0, "the temporary directory is empty");
        assert_eq!(entries(&self.cache), 0, "the cache directory is empty");
        assert_eq!(
            entries(&self.bin),
            1,
            "only the executable is in its directory"
        );
    }
}

fn new_package() -> Vec<u8> {
    // Large enough to need several reads; deterministic.
    (0..2_000_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect()
}

/// Serves a dynamic manifest and `package`, checks and runs `download_and_install`. Returns
/// the result and the server (to inspect requests).
fn run_update(
    environment: TestEnvironment,
    package: Vec<u8>,
    signature: String,
    public_key: String,
    format: &str,
) -> (Result<(), Error>, Server) {
    let format = format.to_string();
    let server = Server::start(move |base, request| match request.target.as_str() {
        "/feed" => Reply::ok(dynamic_manifest(
            "2.0.0",
            &format!("{base}/package"),
            &signature,
            &format,
        )),
        "/package" => Reply::ok(package.clone()),
        _ => Reply::status(404),
    });
    let options = CheckOptions {
        endpoints: vec![server.url("/feed")],
        pubkey: public_key,
        ..CheckOptions::default()
    };
    let updater = updater(Arc::new(environment));
    let update = block_on(updater.check("1.0.0", &options))
        .unwrap()
        .expect("an update is available");
    let result = block_on(update.download_and_install());
    (result, server)
}

// ---------------------------------------------------------------------------------------
// A3: verification gates the installation

#[test]
fn a3_a_valid_signature_replaces_the_content_and_keeps_the_permission_bits() {
    let package = new_package();
    for prehashed in [false, true] {
        for mode in [0o755, 0o750, 0o700] {
            let harness = Harness::new(mode);
            let (result, server) = run_update(
                harness.environment(),
                package.clone(),
                key().signature(&package, prehashed),
                key().public_key(),
                "appimage",
            );
            result.unwrap_or_else(|error| panic!("prehashed={prehashed} mode={mode:o}: {error}"));
            assert_eq!(std::fs::read(&harness.target).unwrap(), package);
            assert_eq!(mode_of(&harness.target), mode, "prehashed={prehashed}");
            harness.assert_no_staging_data();
            assert_eq!(server.targets(), ["/feed", "/package"]);
        }
    }
}

#[test]
fn a3_an_invalid_signature_leaves_the_installed_bytes_untouched() {
    let package = new_package();
    let valid = key().signature(&package, true);
    let valid_legacy = key().signature(&package, false);
    let encode = |text: &str| STANDARD.encode(text);

    // Same key id as the real key, different secret.
    let impostor = TestKey::with_key_id_of(11, &key());
    let tampered_comment = {
        let file = key().signature_file(&package, false);
        encode(&file.replace("timestamp:1700000000", "timestamp:1700000001"))
    };
    let truncated = {
        let file = key().signature_file(&package, false);
        encode(&file[..file.find("trusted comment").unwrap()])
    };
    let mut tampered_package = package.clone();
    tampered_package[1_000_000] ^= 1;

    struct Case {
        name: &'static str,
        package: Vec<u8>,
        signature: String,
        public_key: String,
        expect: &'static str,
    }
    let cases = vec![
        Case {
            name: "another key",
            package: package.clone(),
            signature: other_key().signature(&package, true),
            public_key: key().public_key(),
            expect: "key id mismatch",
        },
        Case {
            name: "another key (legacy)",
            package: package.clone(),
            signature: other_key().signature(&package, false),
            public_key: key().public_key(),
            expect: "key id mismatch",
        },
        Case {
            name: "same key id, different key",
            package: package.clone(),
            signature: impostor.signature(&package, true),
            public_key: key().public_key(),
            expect: "does not match",
        },
        Case {
            name: "tampered package",
            package: tampered_package.clone(),
            signature: valid.clone(),
            public_key: key().public_key(),
            expect: "does not match",
        },
        Case {
            name: "tampered package (legacy)",
            package: tampered_package,
            signature: valid_legacy,
            public_key: key().public_key(),
            expect: "does not match",
        },
        Case {
            name: "tampered trusted comment",
            package: package.clone(),
            signature: tampered_comment,
            public_key: key().public_key(),
            expect: "does not match",
        },
        Case {
            name: "signature of other data",
            package: package.clone(),
            signature: key().signature(b"something else", true),
            public_key: key().public_key(),
            expect: "does not match",
        },
        Case {
            name: "malformed base64 signature",
            package: package.clone(),
            signature: "!!! not base64 !!!".into(),
            public_key: key().public_key(),
            expect: "signature is not valid base64",
        },
        Case {
            name: "malformed base64 public key",
            package: package.clone(),
            signature: valid.clone(),
            public_key: "%%%".into(),
            expect: "public key is not valid base64",
        },
        Case {
            name: "non-UTF-8 signature",
            package: package.clone(),
            signature: STANDARD.encode([0xff, 0xfe, 0xfd]),
            public_key: key().public_key(),
            expect: "signature is not valid UTF-8",
        },
        Case {
            name: "non-UTF-8 public key",
            package: package.clone(),
            signature: valid.clone(),
            public_key: STANDARD.encode([0xff, 0xfe, 0xfd]),
            expect: "public key is not valid UTF-8",
        },
        Case {
            name: "malformed signature",
            package: package.clone(),
            signature: encode("just some text"),
            public_key: key().public_key(),
            expect: "malformed signature",
        },
        Case {
            name: "truncated signature",
            package: package.clone(),
            signature: truncated,
            public_key: key().public_key(),
            expect: "malformed signature",
        },
        Case {
            name: "empty signature",
            package: package.clone(),
            signature: String::new(),
            public_key: key().public_key(),
            expect: "malformed signature",
        },
        Case {
            name: "malformed public key",
            package: package.clone(),
            signature: valid,
            public_key: encode("untrusted comment: x\nnot-a-key"),
            expect: "malformed public key",
        },
    ];
    for case in cases {
        let harness = Harness::new(0o755);
        let before = harness.snapshot();
        let (result, _) = run_update(
            harness.environment(),
            case.package,
            case.signature,
            case.public_key,
            "appimage",
        );
        let error = result.expect_err(case.name);
        assert!(
            matches!(&error, Error::Verification(message) if message.contains(case.expect)),
            "{}: {error}",
            case.name
        );
        assert_eq!(
            harness.snapshot(),
            before,
            "{}: the executable changed",
            case.name
        );
        harness.assert_no_staging_data();
    }
}

#[test]
fn a3_a_non_success_download_status_is_an_error_naming_the_status() {
    let harness = Harness::new(0o755);
    let before = harness.snapshot();
    let server = Server::start(|base, request| match request.target.as_str() {
        "/feed" => Reply::ok(dynamic_manifest(
            "2.0.0",
            &format!("{base}/package"),
            "c2ln",
            "appimage",
        )),
        _ => Reply::status(404),
    });
    let options = CheckOptions {
        endpoints: vec![server.url("/feed")],
        pubkey: key().public_key(),
        ..CheckOptions::default()
    };
    let update = block_on(updater(Arc::new(harness.environment())).check("1.0.0", &options))
        .unwrap()
        .unwrap();
    let error = block_on(update.download_and_install()).unwrap_err();
    assert!(
        matches!(&error, Error::DownloadStatus(status) if status.as_u16() == 404),
        "{error}"
    );
    assert!(error.to_string().contains("404"));
    assert_eq!(harness.snapshot(), before);
    harness.assert_no_staging_data();
}

#[test]
fn a3_installation_runs_off_the_thread_that_polls_the_future() {
    let harness = Harness::new(0o755);
    let package = new_package();
    let installer_thread = Arc::new(Mutex::new(None));
    let environment = {
        let installer_thread = installer_thread.clone();
        harness.environment().on_write(move |path, bytes| {
            *installer_thread.lock().unwrap() = Some(std::thread::current().id());
            std::fs::write(path, bytes)
        })
    };
    let (result, _) = run_update(
        environment,
        package.clone(),
        key().signature(&package, true),
        key().public_key(),
        "appimage",
    );
    result.unwrap();
    let installer_thread = installer_thread
        .lock()
        .unwrap()
        .expect("the file was written");
    assert_ne!(installer_thread, std::thread::current().id());
}

// ---------------------------------------------------------------------------------------
// A4: rollback, unsupported formats, staging candidates

fn signed(package: &[u8]) -> (String, String) {
    (key().signature(package, true), key().public_key())
}

#[test]
fn a4_a_failed_write_restores_the_original_executable() {
    let harness = Harness::new(0o751);
    let before = harness.snapshot();
    let package = new_package();
    let staged = Arc::new(Mutex::new(None));
    let environment = {
        let (tmp, staged) = (harness.tmp.clone(), staged.clone());
        harness.environment().on_write(move |path, _| {
            // Mid-installation: the old executable is staged, the new one is half written.
            let directory = std::fs::read_dir(&tmp)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            *staged.lock().unwrap() = Some((
                directory
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                mode_of(&directory),
                std::fs::read(directory.join("current_app.AppImage")).unwrap(),
                path.exists(),
            ));
            std::fs::write(path, b"PARTIAL")?;
            Err(std::io::Error::other("disk full"))
        })
    };
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(environment, package, signature, public_key, "appimage");
    let error = result.unwrap_err();
    assert!(
        matches!(&error, Error::Io(io) if io.to_string() == "disk full"),
        "{error}"
    );
    assert_eq!(
        harness.snapshot(),
        before,
        "the original is back at its path, same file"
    );
    harness.assert_no_staging_data();
    let (name, directory_mode, staged_content, target_existed) =
        staged.lock().unwrap().clone().unwrap();
    assert!(name.starts_with("current_app"));
    assert_eq!(directory_mode, 0o700);
    assert_eq!(staged_content, OLD);
    assert!(
        !target_existed,
        "the target path is free while the new bytes are written"
    );
}

#[test]
fn a4_a_failed_permission_change_restores_the_original_executable() {
    let harness = Harness::new(0o755);
    let before = harness.snapshot();
    let package = new_package();
    let environment = harness
        .environment()
        .on_set_permissions(|_, _| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(environment, package, signature, public_key, "appimage");
    let error = result.unwrap_err();
    assert!(
        matches!(&error, Error::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied),
        "{error}"
    );
    assert_eq!(harness.snapshot(), before);
    harness.assert_no_staging_data();
}

#[test]
fn a4_a_failed_restore_returns_the_restore_error_and_keeps_the_previous_executable() {
    let harness = Harness::new(0o755);
    let package = new_package();
    // The write fails and leaves a non-empty directory where the executable belongs, so
    // renaming the staged executable back fails too.
    let environment = harness.environment().on_write(|path, _| {
        std::fs::create_dir(path)?;
        std::fs::write(path.join("blocker"), b"x")?;
        Err(std::io::Error::other("disk full"))
    });
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(environment, package, signature, public_key, "appimage");
    let error = result.unwrap_err();
    let Error::Io(io) = &error else {
        panic!("expected an I/O error, got {error}")
    };
    assert!(!io.to_string().starts_with("disk full"), "{error}");
    assert!(error.to_string().contains("staging directory"), "{error}");
    // The previous executable survives in the kept staging directory.
    let kept = staging_leftovers(&harness.tmp);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(
        std::fs::read(harness.tmp.join(&kept[0]).join("current_app.AppImage")).unwrap(),
        OLD
    );
}

#[test]
fn a4_an_unsupported_package_format_fails_before_touching_the_filesystem() {
    for format in ["nsis", "wix", "app"] {
        let harness = Harness::new(0o755);
        let before = harness.snapshot();
        let package = new_package();
        let (signature, public_key) = signed(&package);
        let (result, server) = run_update(
            harness.environment(),
            package,
            signature,
            public_key,
            format,
        );
        let error = result.unwrap_err();
        assert!(
            matches!(error, Error::UnsupportedFormat(found) if found.name() == format),
            "{format}: {error}"
        );
        assert_eq!(
            server.targets(),
            ["/feed"],
            "nothing is downloaded for {format}"
        );
        assert_eq!(harness.snapshot(), before);
        harness.assert_no_staging_data();
    }
    assert_eq!(UpdateFormat::AppImage.name(), "appimage");
}

#[test]
fn a4_without_a_candidate_on_the_executables_filesystem_installation_fails() {
    let harness = Harness::new(0o755);
    let before = harness.snapshot();
    let package = new_package();
    let target = harness.target.clone();
    // Every staging directory reports a different device than the executable.
    let environment = harness
        .environment()
        .devices(move |path| Some(if path == target { 1 } else { 2 }));
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(environment, package, signature, public_key, "appimage");
    let error = result.unwrap_err();
    assert!(
        matches!(error, Error::TempDirNotOnSameMountPoint),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "the temporary directory is not on the same mount point as the AppImage being replaced"
    );
    assert_eq!(harness.snapshot(), before);
    harness.assert_no_staging_data();
}

/// Installs through `environment` and reports the directory the old executable was staged in
/// (observed mid-installation), checking that the installation succeeded and left nothing.
fn staging_root(harness: &Harness, environment: TestEnvironment) -> PathBuf {
    let package = new_package();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let environment = {
        let (roots, seen) = (harness.roots(), seen.clone());
        environment.on_write(move |path, bytes| {
            for root in &roots {
                for name in staging_leftovers(root) {
                    if root.join(&name).join("current_app.AppImage").exists() {
                        seen.lock().unwrap().push(root.clone());
                    }
                }
            }
            std::fs::write(path, bytes)
        })
    };
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(
        environment,
        package.clone(),
        signature,
        public_key,
        "appimage",
    );
    result.unwrap();
    assert_eq!(std::fs::read(&harness.target).unwrap(), package);
    for root in harness.roots() {
        assert_eq!(
            staging_leftovers(&root),
            Vec::<String>::new(),
            "{}",
            root.display()
        );
    }
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.len(),
        1,
        "exactly one staging directory held the old executable: {seen:?}"
    );
    seen[0].clone()
}

#[test]
fn a4_staging_candidates_are_tried_in_order() {
    // 1. The system temporary directory wins when it is on the executable's filesystem.
    let harness = Harness::new(0o755);
    assert_eq!(staging_root(&harness, harness.environment()), harness.tmp);

    // 2. A temporary directory on another device is skipped (its empty staging directory
    //    removed) in favor of XDG_CACHE_HOME.
    let harness = Harness::new(0o755);
    let tmp = harness.tmp.clone();
    let environment = harness
        .environment()
        .devices(move |path| path.starts_with(&tmp).then_some(2));
    assert_eq!(staging_root(&harness, environment), harness.cache);

    // 3. Without XDG_CACHE_HOME the cache is `.cache` under HOME.
    let harness = Harness::new(0o755);
    std::fs::create_dir(harness.home.join(".cache")).unwrap();
    let tmp = harness.tmp.clone();
    let environment = TestEnvironment::new("/nonexistent/running-executable")
        .var("APPIMAGE", &harness.target)
        .var("HOME", &harness.home)
        .temp_dir(&harness.tmp)
        .devices(move |path| path.starts_with(&tmp).then_some(2));
    assert_eq!(
        staging_root(&harness, environment),
        harness.home.join(".cache")
    );

    // 4. XDG_CACHE_HOME takes precedence over HOME.
    let harness = Harness::new(0o755);
    std::fs::create_dir(harness.home.join(".cache")).unwrap();
    let tmp = harness.tmp.clone();
    let environment = harness
        .environment()
        .var("HOME", &harness.home)
        .devices(move |path| path.starts_with(&tmp).then_some(2));
    assert_eq!(staging_root(&harness, environment), harness.cache);

    // 5. An unavailable temporary directory is skipped.
    let harness = Harness::new(0o755);
    let environment = TestEnvironment::new("/nonexistent/running-executable")
        .var("APPIMAGE", &harness.target)
        .var("XDG_CACHE_HOME", &harness.cache);
    assert_eq!(staging_root(&harness, environment), harness.cache);

    // 6. With the temporary directory and the cache on other devices, the directory of the
    //    executable is used.
    let harness = Harness::new(0o755);
    let (tmp, cache) = (harness.tmp.clone(), harness.cache.clone());
    let environment = harness
        .environment()
        .devices(move |path| (path.starts_with(&tmp) || path.starts_with(&cache)).then_some(2));
    assert_eq!(staging_root(&harness, environment), harness.bin);

    // 7. Neither XDG_CACHE_HOME nor HOME: skipped as well.
    let harness = Harness::new(0o755);
    let tmp = harness.tmp.clone();
    let environment = TestEnvironment::new("/nonexistent/running-executable")
        .var("APPIMAGE", &harness.target)
        .temp_dir(&harness.tmp)
        .devices(move |path| path.starts_with(&tmp).then_some(2));
    assert_eq!(staging_root(&harness, environment), harness.bin);
}

#[test]
fn a4_a_staging_directory_that_cannot_be_created_is_an_error_not_a_skipped_candidate() {
    let harness = Harness::new(0o755);
    let before = harness.snapshot();
    let package = new_package();
    let environment = TestEnvironment::new("/nonexistent/running-executable")
        .var("APPIMAGE", &harness.target)
        .temp_dir(harness.tmp.join("missing"));
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(environment, package, signature, public_key, "appimage");
    let error = result.unwrap_err();
    assert!(
        matches!(&error, Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "{error}"
    );
    assert_eq!(harness.snapshot(), before);
}

#[test]
fn a4_the_running_executable_is_replaced_when_appimage_is_unset_and_must_exist() {
    let harness = Harness::new(0o755);
    let link = harness.home.join("link-to-app");
    std::os::unix::fs::symlink(&harness.target, &link).unwrap();
    let package = new_package();
    // No APPIMAGE: the canonical path of the running executable (a symlink here) is used.
    let environment = TestEnvironment::new(&link).temp_dir(&harness.tmp);
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(
        environment,
        package.clone(),
        signature.clone(),
        public_key.clone(),
        "appimage",
    );
    result.unwrap();
    assert_eq!(std::fs::read(&harness.target).unwrap(), package);
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let environment = TestEnvironment::new("/nonexistent/running-executable")
        .var("APPIMAGE", harness.bin.join("missing.AppImage"))
        .temp_dir(&harness.tmp);
    let (result, _) = run_update(environment, package, signature, public_key, "appimage");
    let error = result.unwrap_err();
    assert!(
        matches!(&error, Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "{error}"
    );
}

#[allow(
    clippy::disallowed_methods,
    reason = "the test runs a real child process from the replaced executable"
)]
#[test]
fn a4_installation_does_not_quit_or_relaunch_the_running_process() {
    let sleep = ["/usr/bin/sleep", "/bin/sleep"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .expect("a sleep executable");
    let harness = Harness::new(0o755);
    std::fs::copy(&sleep, &harness.target).unwrap();
    std::fs::set_permissions(
        &harness.target,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let mut running = std::process::Command::new(&harness.target)
        .arg("60")
        .spawn()
        .unwrap();
    let before = harness.snapshot();

    let package = new_package();
    let (signature, public_key) = signed(&package);
    let (result, _) = run_update(
        harness.environment(),
        package.clone(),
        signature,
        public_key,
        "appimage",
    );
    let still_running = running.try_wait().unwrap();
    running.kill().ok();
    running.wait().ok();
    result.unwrap();

    assert!(
        still_running.is_none(),
        "the running executable was not stopped"
    );
    assert_eq!(std::fs::read(&harness.target).unwrap(), package);
    assert_ne!(
        harness.snapshot().2,
        before.2,
        "the path now holds a new file"
    );
    // This test process is evidence in itself: it is still here, and no second process was
    // started by the installation.
    harness.assert_no_staging_data();
}
