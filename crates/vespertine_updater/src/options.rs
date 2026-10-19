use std::time::Duration;

use serde::Deserialize;
use vespertine_http_client::Url;
use vespertine_http_client::http::{HeaderName, HeaderValue};

use crate::error::{Error, Result};

/// How a Windows installer presents itself; carried for every platform but only
/// consumed by the Windows package handlers (it has no effect on Linux).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstallMode {
    #[default]
    Passive,
    Quiet,
    BasicUi,
}

impl InstallMode {
    /// Accepts `passive`, `quiet`, `basicUi` and their capitalized forms.
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "passive" | "Passive" => Ok(Self::Passive),
            "quiet" | "Quiet" => Ok(Self::Quiet),
            "basicUi" | "BasicUi" => Ok(Self::BasicUi),
            other => Err(Error::InvalidInstallMode(other.to_string())),
        }
    }
}

/// One caller-supplied HTTP header.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// The options of a check (the second argument of the JavaScript `checkUpdate`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckOptions {
    /// Feed URLs, tried in order.
    pub endpoints: Vec<String>,
    /// The base64 text of the Minisign public-key file contents.
    pub pubkey: String,
    pub headers: Option<Vec<Header>>,
    /// Per-request timeout in milliseconds; no timeout when absent.
    pub timeout_ms: Option<u64>,
    pub installer_args: Option<Vec<String>>,
    pub install_mode: Option<String>,
}

/// The request settings retained by an available update for its later download.
#[derive(Debug, Clone, Default)]
pub(crate) struct RequestSettings {
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub timeout: Option<Duration>,
}

/// [`CheckOptions`] after validation.
pub(crate) struct ValidatedOptions {
    pub endpoints: Vec<Url>,
    pub settings: RequestSettings,
    pub installer_args: Vec<String>,
    pub install_mode: InstallMode,
}

impl CheckOptions {
    /// Validates everything that can be validated without network traffic.
    pub(crate) fn validate(&self) -> Result<ValidatedOptions> {
        if self.endpoints.is_empty() {
            return Err(Error::NoEndpoints);
        }
        if self.pubkey.trim().is_empty() {
            return Err(Error::MissingPublicKey);
        }
        let endpoints = self
            .endpoints
            .iter()
            .map(|endpoint| {
                Url::parse(endpoint).map_err(|source| Error::InvalidUrl {
                    url: endpoint.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let headers = self
            .headers
            .iter()
            .flatten()
            .map(|header| {
                let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|error| {
                    Error::InvalidHeader {
                        name: header.name.clone(),
                        reason: error.to_string(),
                    }
                })?;
                let value =
                    HeaderValue::from_str(&header.value).map_err(|error| Error::InvalidHeader {
                        name: header.name.clone(),
                        reason: error.to_string(),
                    })?;
                Ok((name, value))
            })
            .collect::<Result<Vec<_>>>()?;
        let timeout = match self.timeout_ms {
            None => None,
            Some(0) => return Err(Error::InvalidTimeout),
            Some(milliseconds) => Some(Duration::from_millis(milliseconds)),
        };
        let install_mode = match &self.install_mode {
            None => InstallMode::default(),
            Some(mode) => InstallMode::parse(mode)?,
        };
        Ok(ValidatedOptions {
            endpoints,
            settings: RequestSettings { headers, timeout },
            installer_args: self.installer_args.clone().unwrap_or_default(),
            install_mode,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_mode_accepts_both_spellings_and_nothing_else() {
        for (text, mode) in [
            ("passive", InstallMode::Passive),
            ("Passive", InstallMode::Passive),
            ("quiet", InstallMode::Quiet),
            ("Quiet", InstallMode::Quiet),
            ("basicUi", InstallMode::BasicUi),
            ("BasicUi", InstallMode::BasicUi),
        ] {
            assert_eq!(InstallMode::parse(text).unwrap(), mode);
        }
        for text in ["", "silent", "QUIET", "basicui", " quiet"] {
            assert!(
                matches!(InstallMode::parse(text), Err(Error::InvalidInstallMode(_))),
                "{text:?}"
            );
        }
    }
}
