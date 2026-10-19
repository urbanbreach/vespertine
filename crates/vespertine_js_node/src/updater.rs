//! Signed application updates (APP-004): `checkUpdate` and the available-update object.
//!
//! Both operations run on the addon's async runtime (never on the JavaScript thread, and the
//! installation on its own worker thread) and report every failure as a rejected promise.

use std::sync::{Arc, OnceLock};

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;
use vespertine_http_client::{HttpClient, read_proxy_from_env};
use vespertine_reqwest_client::ReqwestClient;
use vespertine_updater::{
    CheckOptions, SystemEnvironment, USER_AGENT_VALUE, Update, Updater, apply_certificate_defaults,
};

fn to_napi(error: impl std::fmt::Display) -> Error {
    Error::from_reason(error.to_string())
}

/// The process-wide updater over the application's HTTP client, created on first use after the
/// certificate environment defaults are in place.
fn updater() -> Result<Updater> {
    static UPDATER: OnceLock<std::result::Result<Updater, String>> = OnceLock::new();
    UPDATER
        .get_or_init(|| {
            apply_certificate_defaults(&SystemEnvironment);
            let client =
                ReqwestClient::proxy_and_user_agent(read_proxy_from_env(), USER_AGENT_VALUE)
                    .map_err(|error| format!("could not create the HTTP client: {error:#}"))?;
            let client: Arc<dyn HttpClient> = Arc::new(client);
            Updater::new(client).map_err(|error| error.to_string())
        })
        .clone()
        .map_err(Error::from_reason)
}

/// A newer signed release found by `checkUpdate`.
#[napi]
pub struct AvailableUpdate {
    inner: Arc<Update>,
}

#[napi]
impl AvailableUpdate {
    #[napi(getter)]
    pub fn current_version(&self) -> String {
        self.inner.current_version()
    }

    #[napi(getter)]
    pub fn version(&self) -> String {
        self.inner.version()
    }

    #[napi(getter)]
    pub fn notes(&self) -> Option<String> {
        self.inner.notes().map(str::to_string)
    }

    #[napi(getter)]
    pub fn date(&self) -> Option<String> {
        self.inner.date().map(str::to_string)
    }

    #[napi(getter)]
    pub fn download_url(&self) -> String {
        self.inner.download_url().to_string()
    }

    #[napi(getter)]
    pub fn format(&self) -> String {
        self.inner.format().name().to_string()
    }

    /// Downloads, verifies and installs the package; resolves with nothing. The process is
    /// neither relaunched nor quit.
    #[napi]
    pub fn download_and_install<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let update = self.inner.clone();
        env.spawn_future(async move { update.download_and_install().await.map_err(to_napi) })
    }
}

/// Checks the release feeds for a version newer than `currentVersion`; a promise of an
/// available update or null.
#[napi]
pub fn check_update<'env>(
    env: &'env Env,
    current_version: String,
    options: Option<Value>,
) -> Result<PromiseRaw<'env, Option<AvailableUpdate>>> {
    env.spawn_future(async move {
        let options: CheckOptions = match options {
            None | Some(Value::Null) => CheckOptions::default(),
            Some(value) => serde_json::from_value(value)
                .map_err(|error| Error::from_reason(format!("invalid update options: {error}")))?,
        };
        let update = updater()?
            .check(&current_version, &options)
            .await
            .map_err(to_napi)?;
        Ok(update.map(|update| AvailableUpdate {
            inner: Arc::new(update),
        }))
    })
}
