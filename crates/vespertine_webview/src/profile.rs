//! Profile identity and storage location (PLT-014 R9).

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Fixed version tag hashed before the identity parts.
const PROFILE_VERSION_TAG: &[u8] = b"vespertine-browser-profile-v1";

/// A website-data profile: cookies, localStorage, IndexedDB and cache live under its root.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct BrowserProfile {
    root: PathBuf,
}

impl BrowserProfile {
    /// Derives the profile root from a data directory and stable identity strings.
    ///
    /// The data directory is canonicalized (symbolic links resolved so that all windows of one
    /// installation agree); when that fails its absolute form is used. The identity parts are
    /// hashed with SHA-256 over a version tag followed, for each part, by its length as an 8 byte
    /// little endian integer and its bytes. The root is `<data dir>/browser/<hex digest>`.
    ///
    /// Returns `None` when the identity is unresolved (no parts, or an empty part): such an
    /// embedder must use the ephemeral default context instead of a shared persistent profile.
    pub fn new<S: AsRef<str>>(data_dir: &Path, identity: &[S]) -> Option<Self> {
        if identity.is_empty() || identity.iter().any(|part| part.as_ref().is_empty()) {
            return None;
        }
        let data_dir = std::fs::canonicalize(data_dir)
            .or_else(|_| std::path::absolute(data_dir))
            .ok()?;
        let mut hasher = Sha256::new();
        hasher.update(PROFILE_VERSION_TAG);
        for part in identity {
            let bytes = part.as_ref().as_bytes();
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        Some(Self {
            root: data_dir.join("browser").join(hex(&hasher.finalize())),
        })
    }

    /// The root storage directory of the profile.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Lowercase hexadecimal rendering of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_inputs_give_identical_roots_and_differences_give_different_roots() {
        let dir = tempfile::tempdir().unwrap();
        let a = BrowserProfile::new(dir.path(), &["app", "user"]).unwrap();
        let b = BrowserProfile::new(dir.path(), &["app", "user"]).unwrap();
        assert_eq!(a, b);
        let c = BrowserProfile::new(dir.path(), &["app", "other"]).unwrap();
        assert_ne!(a.root(), c.root());
        let other_dir = tempfile::tempdir().unwrap();
        let d = BrowserProfile::new(other_dir.path(), &["app", "user"]).unwrap();
        assert_ne!(a.root(), d.root());
        // The length prefix separates ("ab", "c") from ("a", "bc").
        let e = BrowserProfile::new(dir.path(), &["ab", "c"]).unwrap();
        let f = BrowserProfile::new(dir.path(), &["a", "bc"]).unwrap();
        assert_ne!(e.root(), f.root());
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(a.root().parent().unwrap(), canonical.join("browser"));
        let digest = a.root().file_name().unwrap().to_str().unwrap();
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    #[test]
    fn symbolic_links_resolve_to_one_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let via_real = BrowserProfile::new(&real, &["id"]).unwrap();
        let via_link = BrowserProfile::new(&link, &["id"]).unwrap();
        assert_eq!(via_real, via_link);
    }

    #[test]
    fn unresolved_identity_has_no_profile() {
        let dir = tempfile::tempdir().unwrap();
        assert!(BrowserProfile::new::<&str>(dir.path(), &[]).is_none());
        assert!(BrowserProfile::new(dir.path(), &["app", ""]).is_none());
    }

    #[test]
    fn missing_data_directory_uses_its_absolute_form() {
        let profile = BrowserProfile::new(Path::new("relative/missing-dir"), &["id"]).unwrap();
        assert!(profile.root().is_absolute());
        assert!(
            profile
                .root()
                .to_string_lossy()
                .contains("relative/missing-dir/browser/")
        );
    }
}
