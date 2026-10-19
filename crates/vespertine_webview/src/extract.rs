//! Helper discovery and content-addressed extraction (PLT-014 R3).

use crate::profile::hex;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The helper executable compiled by `build.rs`, embedded in the application binary.
pub static EMBEDDED_HELPER: &[u8] = include_bytes!(env!("VSP_WEBKIT_HELPER"));

/// The cache directory that holds extracted helpers: the directory named by `XDG_CACHE_HOME`,
/// else `$HOME/.cache`, plus `<app_name>/browser`.
pub fn cache_directory(app_name: &str) -> Result<PathBuf, String> {
    cache_directory_from(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        app_name,
    )
}

fn cache_directory_from(
    xdg_cache_home: Option<&OsStr>,
    home: Option<&OsStr>,
    app_name: &str,
) -> Result<PathBuf, String> {
    let unresolved = || "Could not locate the browser cache directory".to_owned();
    let base = match xdg_cache_home.filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => match home.filter(|value| !value.is_empty()) {
            Some(home) => PathBuf::from(home).join(".cache"),
            None => return Err(unresolved()),
        },
    };
    if !base.is_absolute() {
        return Err(unresolved());
    }
    Ok(base.join(app_name).join("browser"))
}

/// The default per-application directory name: the file name of the running executable.
pub fn default_app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "vespertine".to_owned())
}

/// The content-addressed file name for helper `bytes`: `webkit-` plus the lowercase hexadecimal
/// SHA-256 of the bytes.
pub fn extracted_name(bytes: &[u8]) -> String {
    format!("webkit-{}", hex(&Sha256::digest(bytes)))
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Extracts `bytes` into `dir` under their content-addressed name and returns the path.
///
/// An existing file of that name with identical content is reused. Otherwise the bytes are
/// written to a unique staging file in the same directory (exclusive creation, mode 0700),
/// flushed to disk and atomically renamed over the final name; the staging file is removed on any
/// failure. The final mode is 0700. Concurrent callers obtain the same path and leave exactly one
/// file behind.
pub fn extract_into(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|error| {
        format!(
            "Could not create the browser cache directory {}: {error}",
            dir.display()
        )
    })?;
    let final_path = dir.join(extracted_name(bytes));
    if file_matches(&final_path, bytes) {
        set_executable_mode(&final_path)?;
        return Ok(final_path);
    }
    let staging = dir.join(format!(
        ".staging-{}-{}",
        std::process::id(),
        STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = write_staging(&staging, bytes).and_then(|()| {
        fs::rename(&staging, &final_path)
            .map_err(|error| format!("Could not install the browser helper: {error}"))
    });
    if let Err(error) = result {
        let _ = fs::remove_file(&staging);
        // A concurrent extraction may have installed the same content meanwhile.
        if file_matches(&final_path, bytes) {
            set_executable_mode(&final_path)?;
            return Ok(final_path);
        }
        return Err(error);
    }
    set_executable_mode(&final_path)?;
    Ok(final_path)
}

fn file_matches(path: &Path, bytes: &[u8]) -> bool {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() == bytes.len() as u64 => {
            fs::read(path).is_ok_and(|existing| existing == bytes)
        }
        _ => false,
    }
}

fn write_staging(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file: File = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)
        .map_err(|error| format!("Could not stage the browser helper: {error}"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not write the browser helper: {error}"))
}

fn set_executable_mode(path: &Path) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not set the browser helper permissions: {error}"))
}

/// Extracts the embedded helper into the user's cache directory.
pub fn extract_embedded_helper(app_name: &str) -> Result<PathBuf, String> {
    extract_into(&cache_directory(app_name)?, EMBEDDED_HELPER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Arc, Barrier};

    #[test]
    fn eight_threads_extract_one_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("cache");
        let bytes: Arc<Vec<u8>> = Arc::new((0..300_000u32).map(|n| (n % 251) as u8).collect());
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (dir, bytes, barrier) = (dir.clone(), bytes.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    extract_into(&dir, &bytes).unwrap()
                })
            })
            .collect();
        let paths: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(paths.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(fs::read(&paths[0]).unwrap(), *bytes);
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(fs::metadata(&paths[0]).unwrap().mode() & 0o777, 0o700);
        assert!(
            paths[0]
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("webkit-")
        );
    }

    #[test]
    fn identical_content_is_reused_and_wrong_content_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let path = extract_into(root.path(), b"helper bytes").unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let again = extract_into(root.path(), b"helper bytes").unwrap();
        assert_eq!(path, again);
        assert_eq!(fs::metadata(&again).unwrap().modified().unwrap(), modified);
        // A corrupted file of the same name is replaced by the correct bytes.
        fs::write(&path, b"corrupted!!!").unwrap();
        extract_into(root.path(), b"helper bytes").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"helper bytes");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn name_is_the_sha256_of_the_bytes() {
        // SHA-256 of the empty input.
        assert_eq!(
            extracted_name(b""),
            "webkit-e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn cache_directory_follows_xdg_then_home() {
        let os = OsStr::new;
        assert_eq!(
            cache_directory_from(Some(os("/tmp/xdg-cache")), Some(os("/home/x")), "demo").unwrap(),
            PathBuf::from("/tmp/xdg-cache/demo/browser")
        );
        assert_eq!(
            cache_directory_from(None, Some(os("/home/someone")), "demo").unwrap(),
            PathBuf::from("/home/someone/.cache/demo/browser")
        );
        assert_eq!(
            cache_directory_from(Some(os("")), Some(os("/home/someone")), "demo").unwrap(),
            PathBuf::from("/home/someone/.cache/demo/browser")
        );
        assert_eq!(
            cache_directory_from(None, None, "demo").unwrap_err(),
            "Could not locate the browser cache directory"
        );
        assert!(cache_directory_from(Some(os("relative")), None, "demo").is_err());
    }
}
