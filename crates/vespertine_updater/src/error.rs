use std::io;

use vespertine_http_client::StatusCode;

use crate::feed::UpdateFormat;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can make a check or an installation fail. Every variant has a
/// human-readable message; callers surface it unchanged.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid current version `{version}`: {source}")]
    InvalidVersion {
        version: String,
        source: semver::Error,
    },
    #[error("at least one update endpoint is required")]
    NoEndpoints,
    #[error("the Minisign public key is required")]
    MissingPublicKey,
    #[error("invalid update endpoint `{url}`: {source}")]
    InvalidUrl {
        url: String,
        source: url::ParseError,
    },
    #[error("invalid header `{name}`: {reason}")]
    InvalidHeader { name: String, reason: String },
    #[error("invalid install mode `{0}`; expected passive, quiet or basicUi")]
    InvalidInstallMode(String),
    #[error("the request timeout must be a positive number of milliseconds")]
    InvalidTimeout,
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
    #[error("request to {url} failed: {message}")]
    Request { url: String, message: String },
    #[error("could not fetch a valid release")]
    ReleaseNotFound,
    #[error("malformed release feed: {0}")]
    MalformedFeed(String),
    #[error("the release feed has no `{0}` field")]
    MissingField(&'static str),
    #[error("invalid publication date `{0}`: expected an RFC 3339 timestamp")]
    InvalidDate(String),
    #[error("unknown package format `{0}`; expected nsis, wix, app or appimage")]
    UnknownFormat(String),
    #[error("invalid release version `{version}`: {source}")]
    InvalidReleaseVersion {
        version: String,
        source: semver::Error,
    },
    #[error("no release asset named `*_{arch}.AppImage` for target `{target}` and arch `{arch}`")]
    AssetNotFound { target: String, arch: String },
    #[error("the release has no signature asset `{0}`")]
    SignatureAssetMissing(String),
    #[error("downloading the signature failed with HTTP status {0}")]
    SignatureStatus(StatusCode),
    #[error("the signature file is not valid UTF-8")]
    SignatureNotUtf8,
    #[error("the release has no package for the platform `{0}`")]
    TargetNotFound(String),
    #[error("downloading the update failed with HTTP status {0}")]
    DownloadStatus(StatusCode),
    #[error("installing a `{0}` package is not supported on this platform")]
    UnsupportedFormat(UpdateFormat),
    #[error("signature verification failed: {0}")]
    Verification(String),
    #[error(
        "the temporary directory is not on the same mount point as the AppImage being replaced"
    )]
    TempDirNotOnSameMountPoint,
    #[error("the installation worker stopped unexpectedly")]
    WorkerStopped,
    #[error(transparent)]
    Io(#[from] io::Error),
}
