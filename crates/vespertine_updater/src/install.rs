//! Installing a verified package: the process environment the installer depends on and the
//! Linux AppImage replacement.

use std::ffi::OsString;
use std::fs::Permissions;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::feed::UpdateFormat;

/// Default value of `SSL_CERT_FILE` when it is unset.
pub const DEFAULT_CERT_FILE: &str = "/etc/ssl/certs/ca-certificates.crt";
/// Default value of `SSL_CERT_DIR` when it is unset.
pub const DEFAULT_CERT_DIR: &str = "/etc/ssl/certs";

/// The process environment and file operations the updater depends on, injectable so the
/// installation can be exercised without touching the real process environment.
pub trait Environment: Send + Sync + 'static {
    /// The value of an environment variable, `None` when unset.
    fn var(&self, name: &str) -> Option<OsString>;

    /// Sets an environment variable of the process (used only for the certificate defaults).
    fn set_var(&self, name: &str, value: &str);

    /// The system temporary directory, `None` when the system has none.
    fn temp_dir(&self) -> Option<PathBuf>;

    /// The path of the running executable.
    fn current_exe(&self) -> io::Result<PathBuf>;

    /// The id of the device (filesystem) holding `path`.
    fn device_id(&self, path: &Path) -> io::Result<u64> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Ok(std::fs::metadata(path)?.dev())
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }

    /// Creates or replaces the file at `path` with `bytes`.
    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    /// Applies `permissions` to the file at `path`.
    fn set_permissions(&self, path: &Path, permissions: &Permissions) -> io::Result<()> {
        std::fs::set_permissions(path, permissions.clone())
    }
}

/// The real process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemEnvironment;

impl Environment for SystemEnvironment {
    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn set_var(&self, name: &str, value: &str) {
        // SAFETY: the updater sets variables only here, only when they are unset, and only
        // before its first request, which is the documented contract of the certificate
        // defaults; callers must not read the environment concurrently from foreign threads
        // at that moment, exactly as with any other `set_var`.
        unsafe { std::env::set_var(name, value) }
    }

    fn temp_dir(&self) -> Option<PathBuf> {
        Some(std::env::temp_dir())
    }

    fn current_exe(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }
}

fn non_empty_var(environment: &dyn Environment, name: &str) -> Option<OsString> {
    environment.var(name).filter(|value| !value.is_empty())
}

/// On Linux, points `SSL_CERT_FILE` and `SSL_CERT_DIR` at the system certificate store when
/// they are unset. Called before the first request of a check or a download.
pub fn apply_certificate_defaults(environment: &dyn Environment) {
    if !cfg!(target_os = "linux") {
        return;
    }
    for (name, default) in [
        ("SSL_CERT_FILE", DEFAULT_CERT_FILE),
        ("SSL_CERT_DIR", DEFAULT_CERT_DIR),
    ] {
        if environment.var(name).is_none() {
            environment.set_var(name, default);
        }
    }
}

/// Installs `package` for `format` on this platform. Linux supports only AppImage packages.
pub(crate) fn install(
    environment: &dyn Environment,
    format: UpdateFormat,
    package: &[u8],
) -> Result<()> {
    ensure_installable(format)?;
    #[cfg(target_os = "linux")]
    {
        linux::replace_appimage(environment, package)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (environment, package);
        Err(Error::UnsupportedPlatform(
            "installing packages is only supported on linux".to_string(),
        ))
    }
}

/// Fails before anything is touched when `format` cannot be installed here.
pub(crate) fn ensure_installable(format: UpdateFormat) -> Result<()> {
    if cfg!(target_os = "linux") && format != UpdateFormat::AppImage {
        return Err(Error::UnsupportedFormat(format));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    const STAGING_PREFIX: &str = "current_app";
    const STAGED_FILE_NAME: &str = "current_app.AppImage";

    /// Where staging directories are tried, in order.
    enum Root {
        SystemTemp,
        Cache,
        TargetDirectory,
    }

    impl Root {
        const ORDER: [Root; 3] = [Root::SystemTemp, Root::Cache, Root::TargetDirectory];

        fn resolve(&self, environment: &dyn Environment, target: &Path) -> Option<PathBuf> {
            match self {
                Root::SystemTemp => environment.temp_dir(),
                Root::Cache => non_empty_var(environment, "XDG_CACHE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        non_empty_var(environment, "HOME")
                            .map(|home| PathBuf::from(home).join(".cache"))
                    }),
                Root::TargetDirectory => target.parent().map(Path::to_path_buf),
            }
        }
    }

    /// Replaces the running AppImage (the path in `APPIMAGE`, else the canonical path of the
    /// running executable) with `package`. The old executable is first renamed into a private
    /// staging directory on the same filesystem, so the swap needs no copy and a failure can
    /// put it back.
    pub(super) fn replace_appimage(environment: &dyn Environment, package: &[u8]) -> Result<()> {
        let target = match non_empty_var(environment, "APPIMAGE") {
            Some(path) => PathBuf::from(path),
            None => environment.current_exe()?.canonicalize()?,
        };
        let permissions = std::fs::metadata(&target)?.permissions();
        let target_device = environment.device_id(&target)?;

        for root in Root::ORDER {
            let Some(root) = root.resolve(environment, &target) else {
                continue;
            };
            let staging = tempfile::Builder::new()
                .prefix(STAGING_PREFIX)
                .tempdir_in(&root)?;
            if environment.device_id(staging.path())? != target_device {
                // Dropping the empty staging directory removes it.
                continue;
            }
            std::fs::set_permissions(staging.path(), Permissions::from_mode(0o700))?;
            let staged = staging.path().join(STAGED_FILE_NAME);
            std::fs::rename(&target, &staged)?;
            let installed = environment
                .write_file(&target, package)
                .and_then(|()| environment.set_permissions(&target, &permissions));
            return match installed {
                Ok(()) => {
                    if let Err(error) = staging.close() {
                        log::warn!("could not remove the update staging directory: {error}");
                    }
                    Ok(())
                }
                Err(error) => match std::fs::rename(&staged, &target) {
                    Ok(()) => Err(error.into()),
                    Err(restore_error) => {
                        // Never delete the only copy of the previous executable.
                        let kept = staging.keep();
                        log::error!(
                            "could not restore {} after a failed update ({error}); the staging \
                             directory {} was kept",
                            target.display(),
                            kept.display()
                        );
                        Err(io::Error::new(
                            restore_error.kind(),
                            format!(
                                "{restore_error}; the staging directory {} was kept",
                                kept.display()
                            ),
                        )
                        .into())
                    }
                },
            };
        }
        Err(Error::TempDirNotOnSameMountPoint)
    }
}
