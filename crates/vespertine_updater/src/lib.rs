//! Signed packaged-application updates (APP-004).
//!
//! An [`Updater`] checks an ordered list of release feeds for a newer version of the
//! application ([`Updater::check`]) and returns an [`Update`] that downloads the package,
//! verifies its Minisign signature against a public key and installs it
//! ([`Update::download_and_install`]). On Linux the package is a self-contained AppImage
//! that replaces the running one in place; the application keeps running from its open
//! image and the caller decides when to quit or relaunch. Prompts, progress and relaunching
//! are the caller's.
//!
//! Requests go through an injected [`HttpClient`] (the application's own client, in
//! production `vespertine_reqwest_client::ReqwestClient`), and the installation reads its
//! environment through an injectable [`Environment`]. Both operations are asynchronous
//! futures that never block the thread polling them: the network waits on the client, the
//! verification and file replacement run on a dedicated worker thread.

mod error;
mod feed;
mod http;
mod install;
mod options;
mod platform;
mod verify;
mod version;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use semver::Version;
use vespertine_http_client::{HttpClient, Url};

pub use error::{Error, Result};
pub use feed::UpdateFormat;
pub use http::USER_AGENT_VALUE;
pub use install::{
    DEFAULT_CERT_DIR, DEFAULT_CERT_FILE, Environment, SystemEnvironment, apply_certificate_defaults,
};
pub use options::{CheckOptions, Header, InstallMode};
pub use platform::{Os, Platform};
pub use verify::verify_package;
pub use version::{is_newer, parse_version, precedence};

use feed::{Feed, Package, Packages};
use http::RequestKind;
use options::RequestSettings;

/// Checks release feeds and creates [`Update`]s.
#[derive(Clone)]
pub struct Updater {
    client: Arc<dyn HttpClient>,
    environment: Arc<dyn Environment>,
    platform: Platform,
}

impl Updater {
    /// An updater for the running platform using the real process environment.
    pub fn new(client: Arc<dyn HttpClient>) -> Result<Self> {
        Ok(Self {
            client,
            environment: Arc::new(SystemEnvironment),
            platform: Platform::detect()?,
        })
    }

    /// Replaces the environment the installation and certificate defaults use.
    pub fn with_environment(mut self, environment: Arc<dyn Environment>) -> Self {
        self.environment = environment;
        self
    }

    /// Replaces the platform the feeds are resolved for.
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// Checks the endpoints of `options` in order for a version newer than `current_version`.
    ///
    /// Invalid input fails before any request is made. A 204 response from any endpoint ends
    /// the check with `Ok(None)`; otherwise endpoints are tried until one yields a parsed
    /// release, remembering failures. A release that is not strictly newer gives `Ok(None)`.
    pub async fn check(
        &self,
        current_version: &str,
        options: &CheckOptions,
    ) -> Result<Option<Update>> {
        let current = parse_version(current_version).map_err(|source| Error::InvalidVersion {
            version: current_version.to_string(),
            source,
        })?;
        let validated = options.validate()?;
        let endpoints = validated
            .endpoints
            .iter()
            .map(|endpoint| self.platform.expand_endpoint(endpoint, &current))
            .collect::<Result<Vec<_>>>()?;
        apply_certificate_defaults(&*self.environment);

        let mut last_error = None;
        let mut release = None;
        for endpoint in &endpoints {
            match self
                .fetch_release(endpoint, &current, &validated.settings)
                .await
            {
                Ok(Fetched::NoContent) => return Ok(None),
                Ok(Fetched::Release(found)) => {
                    release = Some(found);
                    break;
                }
                Ok(Fetched::Unavailable) => {}
                Err(error) => last_error = Some(error),
            }
        }
        let Some(release) = release else {
            return Err(last_error.unwrap_or(Error::ReleaseNotFound));
        };
        if !is_newer(&release.version, &current) {
            return Ok(None);
        }
        let package = match release.packages {
            Packages::Single(package) => package,
            Packages::Platforms(mut platforms) => {
                let key = self.platform.key();
                platforms.remove(&key).ok_or(Error::TargetNotFound(key))?
            }
        };
        Ok(Some(Update {
            current_version: current,
            version: release.version,
            notes: release.notes,
            date: release.date,
            package,
            public_key: options.pubkey.clone(),
            settings: validated.settings,
            installer_args: validated.installer_args,
            install_mode: validated.install_mode,
            client: self.client.clone(),
            environment: self.environment.clone(),
        }))
    }

    async fn fetch_release(
        &self,
        endpoint: &Url,
        current: &Version,
        settings: &RequestSettings,
    ) -> Result<Fetched> {
        let reply = http::get(&self.client, endpoint, settings, RequestKind::Feed).await?;
        if reply.status == vespertine_http_client::StatusCode::NO_CONTENT {
            return Ok(Fetched::NoContent);
        }
        if !reply.status.is_success() {
            return Ok(Fetched::Unavailable);
        }
        let body = reply.bytes().await?;
        let release = match feed::parse_feed(&body, &self.platform)? {
            Feed::Manifest(manifest) => Release {
                version: manifest.version,
                notes: manifest.notes,
                date: manifest.date,
                packages: manifest.packages,
            },
            Feed::GitHub(github) => {
                // The signature is only needed, and only fetched, for a newer release.
                let signature = if is_newer(&github.version, current) {
                    self.fetch_signature(&github.signature_url, settings)
                        .await?
                } else {
                    String::new()
                };
                Release {
                    version: github.version,
                    notes: github.notes,
                    date: github.date,
                    packages: Packages::Single(Package {
                        url: github.package_url,
                        signature,
                        format: UpdateFormat::AppImage,
                    }),
                }
            }
        };
        Ok(Fetched::Release(release))
    }

    async fn fetch_signature(&self, url: &Url, settings: &RequestSettings) -> Result<String> {
        let reply = http::get(&self.client, url, settings, RequestKind::Download).await?;
        if !reply.status.is_success() {
            return Err(Error::SignatureStatus(reply.status));
        }
        String::from_utf8(reply.bytes().await?).map_err(|_| Error::SignatureNotUtf8)
    }
}

struct Release {
    version: Version,
    notes: Option<String>,
    date: Option<String>,
    packages: Packages,
}

enum Fetched {
    /// HTTP 204: no update is available.
    NoContent,
    /// A non-success status: try the next endpoint, nothing to remember.
    Unavailable,
    Release(Release),
}

/// A newer release the running application can download and install.
pub struct Update {
    current_version: Version,
    version: Version,
    notes: Option<String>,
    date: Option<String>,
    package: Package,
    public_key: String,
    settings: RequestSettings,
    installer_args: Vec<String>,
    install_mode: InstallMode,
    client: Arc<dyn HttpClient>,
    environment: Arc<dyn Environment>,
}

impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Update")
            .field("current_version", &self.current_version.to_string())
            .field("version", &self.version.to_string())
            .field("notes", &self.notes)
            .field("date", &self.date)
            .field("download_url", &self.package.url.as_str())
            .field("format", &self.package.format)
            .finish_non_exhaustive()
    }
}

impl Update {
    /// The normalized version of the running application.
    pub fn current_version(&self) -> String {
        self.current_version.to_string()
    }

    /// The normalized version of the release.
    pub fn version(&self) -> String {
        self.version.to_string()
    }

    pub fn notes(&self) -> Option<&str> {
        self.notes.as_deref()
    }

    /// The publication date as an RFC 3339 timestamp.
    pub fn date(&self) -> Option<&str> {
        self.date.as_deref()
    }

    pub fn download_url(&self) -> &str {
        self.package.url.as_str()
    }

    pub fn format(&self) -> UpdateFormat {
        self.package.format
    }

    /// The installer arguments of the check; they have no effect on Linux.
    pub fn installer_args(&self) -> &[String] {
        &self.installer_args
    }

    /// The install mode of the check; it has no effect on Linux.
    pub fn install_mode(&self) -> InstallMode {
        self.install_mode
    }

    /// The timeout applied to every request of this update.
    pub fn timeout(&self) -> Option<Duration> {
        self.settings.timeout
    }

    /// Downloads the package, verifies it against the signature and public key and installs
    /// it. Nothing on disk changes unless verification succeeds; the application is neither
    /// relaunched nor quit.
    pub async fn download_and_install(&self) -> Result<()> {
        install::ensure_installable(self.package.format)?;
        apply_certificate_defaults(&*self.environment);
        let reply = http::get(
            &self.client,
            &self.package.url,
            &self.settings,
            RequestKind::Download,
        )
        .await?;
        if !reply.status.is_success() {
            return Err(Error::DownloadStatus(reply.status));
        }
        let bytes = reply.bytes().await?;
        let public_key = self.public_key.clone();
        let signature = self.package.signature.clone();
        let format = self.package.format;
        let environment = self.environment.clone();
        run_on_worker_thread(move || {
            verify_package(&bytes, &public_key, &signature)?;
            install::install(&*environment, format, &bytes)
        })
        .await?
    }
}

/// Runs `work` on its own thread so that neither the verification nor the file replacement
/// blocks the thread polling the returned future.
fn run_on_worker_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = Result<T>> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("vespertine-updater".to_string())
        .spawn(move || {
            sender.send(work()).ok();
        });
    async move {
        spawned?;
        receiver.await.map_err(|_| Error::WorkerStopped)
    }
}
