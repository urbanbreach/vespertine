//! Requests through the application's HTTP client.

use std::sync::Arc;

use futures::AsyncReadExt as _;
use vespertine_http_client::http::header::{ACCEPT, USER_AGENT};
use vespertine_http_client::http::{HeaderValue, Method};
use vespertine_http_client::{
    AsyncBody, HttpClient, HttpRequestExt as _, RedirectPolicy, Request, StatusCode, Url,
};

use crate::error::{Error, Result};
use crate::options::RequestSettings;

/// Sent on package and signature downloads unless the caller supplies a `User-Agent`.
pub const USER_AGENT_VALUE: &str = concat!("vespertine-updater/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestKind {
    /// A release feed: `Accept: application/json` by default.
    Feed,
    /// A signature or package download: `Accept: application/octet-stream` and a
    /// `User-Agent` by default.
    Download,
}

/// A response whose body is not read yet.
pub(crate) struct Reply {
    pub status: StatusCode,
    url: String,
    body: AsyncBody,
}

impl Reply {
    /// Reads the complete body.
    pub async fn bytes(mut self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.body
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| Error::Request {
                url: self.url,
                message: error.to_string(),
            })?;
        Ok(bytes)
    }
}

/// Sends a GET request with the caller's headers and timeout. A transport failure is an
/// [`Error::Request`]; HTTP statuses are for the caller to interpret.
pub(crate) async fn get(
    client: &Arc<dyn HttpClient>,
    url: &Url,
    settings: &RequestSettings,
    kind: RequestKind,
) -> Result<Reply> {
    let failed = |error: &dyn std::fmt::Display| Error::Request {
        url: url.to_string(),
        message: error.to_string(),
    };
    let supplied = |name| settings.headers.iter().any(|(header, _)| header == name);
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(url.as_str())
        .follow_redirects(RedirectPolicy::FollowLimit(10));
    if !supplied(&ACCEPT) {
        builder = builder.header(
            ACCEPT,
            match kind {
                RequestKind::Feed => "application/json",
                RequestKind::Download => "application/octet-stream",
            },
        );
    }
    if kind == RequestKind::Download && !supplied(&USER_AGENT) {
        builder = builder.header(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    }
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in &settings.headers {
            headers.append(name.clone(), value.clone());
        }
    }
    if let Some(timeout) = settings.timeout {
        builder = builder.timeout(timeout);
    }
    let request = builder
        .body(AsyncBody::empty())
        .map_err(|error| failed(&error))?;
    let response = client
        .send(request)
        .await
        .map_err(|error| failed(&format!("{error:#}")))?;
    Ok(Reply {
        status: response.status(),
        url: url.to_string(),
        body: response.into_body(),
    })
}
