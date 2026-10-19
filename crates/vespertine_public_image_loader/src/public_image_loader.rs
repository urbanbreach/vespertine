//! A restricted HTTP client for images that come from untrusted content (ELM-004 R23-R29).
//!
//! It is an opt-in transport installed through the application's HTTP client setter in place
//! of the general client. Every failure ends in a load-failed result (an error from `send` or
//! from reading the body); dropping the request future or the body cancels the transfer.
//! The client only sends GET, never forwards credentials or a referer, never uses a proxy,
//! accepts only HTTPS URLs on public hosts, follows a bounded number of redirects (checking
//! every hop), bounds the response (declared and streamed), and resolves names to public
//! addresses only.

use futures::{AsyncRead, future::BoxFuture};
use http::{HeaderValue, Method, StatusCode, header};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use thiserror::Error;
use vespertine_http_client::{
    AsyncBody, Host, HttpClient, RedirectPolicy, RequestTimeout, Response, Url,
};
use vespertine_reqwest_client::ReqwestClient;

/// The maximum response size: 16 MiB.
pub const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
/// The maximum number of redirects followed.
pub const MAX_REDIRECTS: u32 = 5;
/// The connect timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The total timeout for a request, including redirects and the body.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// The limits and host requirements of a loader. Hosts choose the policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicImagePolicy {
    /// Accept only `https` URLs.
    pub require_https: bool,
    /// Accept only public hosts (public domain names, public IP literals) and resolve names
    /// to public addresses only.
    pub require_public_host: bool,
    /// The redirect limit; the redirect after this many has failed.
    pub max_redirects: u32,
    /// The response size limit in bytes.
    pub max_response_bytes: u64,
    /// The connect timeout.
    pub connect_timeout: Duration,
    /// The total timeout.
    pub total_timeout: Duration,
}

impl PublicImagePolicy {
    /// The production policy: HTTPS only, public hosts only, at most 5 redirects, 16 MiB,
    /// 10 s connect and 30 s total timeouts.
    pub const fn production() -> Self {
        Self {
            require_https: true,
            require_public_host: true,
            max_redirects: MAX_REDIRECTS,
            max_response_bytes: MAX_RESPONSE_BYTES,
            connect_timeout: CONNECT_TIMEOUT,
            total_timeout: TOTAL_TIMEOUT,
        }
    }

    /// A policy for tests that relaxes the scheme and public-host requirements so a loopback
    /// test server (which speaks plain HTTP) can be reached. It keeps the redirect limit, the
    /// size limit, the timeouts and the no-credentials rule. It is never the default.
    pub const fn relaxed_for_tests() -> Self {
        Self {
            require_https: false,
            require_public_host: false,
            ..Self::production()
        }
    }
}

impl Default for PublicImagePolicy {
    fn default() -> Self {
        Self::production()
    }
}

/// Why a request was refused or failed.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PublicImageError {
    /// The URL (initial or redirect target) is not acceptable under the policy.
    #[error("URL refused: {0}")]
    UrlRefused(String),
    /// A method other than GET was requested.
    #[error("only GET requests are allowed, got {0}")]
    MethodRefused(String),
    /// The redirect limit was reached.
    #[error("too many redirects (limit {0})")]
    TooManyRedirects(u32),
    /// A redirect had no usable `Location`.
    #[error("invalid redirect: {0}")]
    InvalidRedirect(String),
    /// The declared content length exceeds the limit.
    #[error("response of {declared} bytes exceeds the {limit} byte limit")]
    DeclaredTooLarge {
        /// The declared length.
        declared: u64,
        /// The limit.
        limit: u64,
    },
    /// More bytes were streamed than the limit allows.
    #[error("response exceeds the {limit} byte limit")]
    StreamTooLarge {
        /// The limit.
        limit: u64,
    },
    /// A name resolved to no public address.
    #[error("no public address for {0}")]
    NoPublicAddress(String),
}

/// Whether an IP address is public (R26): none of the refused ranges.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let refused = a == 0 // "this network", includes 0.0.0.0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b)) // shared address space
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // documentation
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 224; // multicast, reserved and limited broadcast
    !refused
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // IPv4-mapped (::ffff:a.b.c.d).
    if let Some(embedded) = ip.to_ipv4_mapped() {
        return is_public_ipv4(embedded);
    }
    // IPv4-compatible (::a.b.c.d); also covers the unspecified and loopback addresses.
    if segments[..6].iter().all(|segment| *segment == 0) {
        let [high, low] = [segments[6], segments[7]];
        return is_public_ipv4(Ipv4Addr::new(
            (high >> 8) as u8,
            high as u8,
            (low >> 8) as u8,
            low as u8,
        ));
    }
    // NAT64 (64:ff9b::/96): judged by the embedded IPv4 address.
    if segments[0] == 0x64 && segments[1] == 0xff9b && segments[2..6].iter().all(|s| *s == 0) {
        let [high, low] = [segments[6], segments[7]];
        return is_public_ipv4(Ipv4Addr::new(
            (high >> 8) as u8,
            high as u8,
            (low >> 8) as u8,
            low as u8,
        ));
    }
    // 6to4 (2002::/16): the embedded IPv4 address follows the prefix.
    if segments[0] == 0x2002 {
        let [high, low] = [segments[1], segments[2]];
        return is_public_ipv4(Ipv4Addr::new(
            (high >> 8) as u8,
            high as u8,
            (low >> 8) as u8,
            low as u8,
        ));
    }
    let refused = segments[0] & 0xff00 == 0xff00 // multicast
        || segments[0] & 0xfe00 == 0xfc00 // unique local
        || segments[0] & 0xffc0 == 0xfe80 // link-local
        || segments[0] & 0xffc0 == 0xfec0 // site-local
        || (segments[0] == 0x2001 && segments[1] == 0); // Teredo
    !refused
}

/// Checks a URL against the policy (R25). Applies to the initial URL and every redirect hop.
pub fn check_url(url: &Url, policy: &PublicImagePolicy) -> Result<(), PublicImageError> {
    let refuse = |reason: &str| Err(PublicImageError::UrlRefused(format!("{reason}: {url}")));
    let scheme = url.scheme();
    let scheme_ok = scheme == "https" || (!policy.require_https && scheme == "http");
    if !scheme_ok {
        return refuse("scheme is not https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return refuse("URL carries credentials");
    }
    let Some(host) = url.host() else {
        return refuse("URL has no host");
    };
    if !policy.require_public_host {
        return Ok(());
    }
    match host {
        Host::Domain(domain) => {
            let domain = domain.to_ascii_lowercase();
            let domain = domain.strip_suffix('.').unwrap_or(&domain);
            let local_suffix = [".localhost", ".local", ".internal"]
                .iter()
                .any(|suffix| domain.ends_with(suffix));
            if !domain.contains('.') || local_suffix {
                return refuse("host is not a public domain name");
            }
        }
        Host::Ipv4(ip) => {
            if !is_public_ip(IpAddr::V4(ip)) {
                return refuse("IP address is not public");
            }
        }
        Host::Ipv6(ip) => {
            if !is_public_ip(IpAddr::V6(ip)) {
                return refuse("IP address is not public");
            }
        }
    }
    Ok(())
}

/// Resolves `host` and keeps only public addresses (R27); fails with "no public address" when
/// none remains.
pub fn lookup_public_addresses(host: &str, port: u16) -> Result<Vec<SocketAddr>, PublicImageError> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|_| PublicImageError::NoPublicAddress(host.to_string()))?;
    filter_public_addresses(host, addresses)
}

/// Drops every non-public address; fails when none remains.
pub fn filter_public_addresses(
    host: &str,
    addresses: impl IntoIterator<Item = SocketAddr>,
) -> Result<Vec<SocketAddr>, PublicImageError> {
    let public: Vec<_> = addresses
        .into_iter()
        .filter(|address| is_public_ip(address.ip()))
        .collect();
    if public.is_empty() {
        Err(PublicImageError::NoPublicAddress(host.to_string()))
    } else {
        Ok(public)
    }
}

/// A resolver that only ever returns public addresses.
#[derive(Clone, Copy, Debug, Default)]
pub struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addresses = tokio::task::spawn_blocking(move || lookup_public_addresses(&host, 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

/// Wraps a body and fails once more than `limit` bytes have been read.
struct LimitedReader<R> {
    inner: R,
    remaining: u64,
    limit: u64,
}

impl<R: AsyncRead + Unpin> AsyncRead for LimitedReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        // Read one byte past the remaining budget so exceeding the limit is detected.
        let allowed = (this.remaining.saturating_add(1)).min(buf.len() as u64) as usize;
        match Pin::new(&mut this.inner).poll_read(cx, &mut buf[..allowed]) {
            Poll::Ready(Ok(read)) => {
                if read as u64 > this.remaining {
                    this.remaining = 0;
                    Poll::Ready(Err(io::Error::other(PublicImageError::StreamTooLarge {
                        limit: this.limit,
                    })))
                } else {
                    this.remaining -= read as u64;
                    Poll::Ready(Ok(read))
                }
            }
            other => other,
        }
    }
}

/// The restricted HTTP client.
pub struct PublicImageHttpClient {
    client: Arc<ReqwestClient>,
    policy: PublicImagePolicy,
    user_agent: Option<HeaderValue>,
}

/// Credentials and ambient context that are never forwarded.
const STRIPPED_HEADERS: [header::HeaderName; 4] = [
    header::AUTHORIZATION,
    header::PROXY_AUTHORIZATION,
    header::COOKIE,
    header::REFERER,
];

impl PublicImageHttpClient {
    /// Builds a client with the given policy and user agent.
    pub fn new(policy: PublicImagePolicy, user_agent: &str) -> anyhow::Result<Self> {
        let user_agent = HeaderValue::from_str(user_agent)?;
        let mut headers = header::HeaderMap::new();
        headers.insert(header::USER_AGENT, user_agent.clone());
        let mut builder = reqwest::Client::builder()
            .connect_timeout(policy.connect_timeout)
            .default_headers(headers)
            // The proxy would resolve names itself and bypass the address checks.
            .no_proxy()
            // Redirects are followed by `send`, which checks every hop.
            .redirect_policy(reqwest::redirect::Policy::none())
            .referer(false);
        if policy.require_public_host {
            builder = builder.dns_resolver(Arc::new(PublicOnlyResolver));
        }
        let client = builder
            .use_rustls_tls()
            .use_preconfigured_tls(vespertine_http_client_tls::tls_config())
            .build()?;
        Ok(Self {
            client: Arc::new(ReqwestClient::from(client)),
            policy,
            user_agent: Some(user_agent),
        })
    }

    /// Builds a client with the production policy.
    pub fn production(user_agent: &str) -> anyhow::Result<Self> {
        Self::new(PublicImagePolicy::production(), user_agent)
    }

    /// The policy in force.
    pub fn policy(&self) -> &PublicImagePolicy {
        &self.policy
    }
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

impl HttpClient for PublicImageHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        self.user_agent.as_ref()
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: http::Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let policy = self.policy.clone();
        let (parts, _body) = req.into_parts();
        let mut headers = parts.headers;
        for name in &STRIPPED_HEADERS {
            headers.remove(name);
        }
        let method = parts.method;
        let uri = parts.uri.to_string();
        // Each hop goes through the inner client (redirects off, one body deadline).
        let inner = self.client.clone();
        Box::pin(async move {
            if method != Method::GET {
                return Err(PublicImageError::MethodRefused(method.to_string()).into());
            }
            let mut url = Url::parse(&uri)
                .map_err(|error| PublicImageError::UrlRefused(format!("{uri}: {error}")))?;
            let started = Instant::now();
            let mut redirects = 0u32;
            loop {
                check_url(&url, &policy)?;
                let remaining = policy
                    .total_timeout
                    .checked_sub(started.elapsed())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| anyhow::anyhow!("request timed out"))?;
                let mut builder = http::Request::builder()
                    .method(Method::GET)
                    .uri(url.as_str())
                    .extension(RedirectPolicy::NoFollow)
                    .extension(RequestTimeout(remaining));
                *builder.headers_mut().expect("builder is valid") = headers.clone();
                let request = builder.body(AsyncBody::empty())?;
                let response = inner.send(request).await?;
                let status = response.status();
                if is_redirect(status) {
                    if redirects >= policy.max_redirects {
                        return Err(PublicImageError::TooManyRedirects(policy.max_redirects).into());
                    }
                    let location = response
                        .headers()
                        .get(header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            PublicImageError::InvalidRedirect("missing Location header".into())
                        })?;
                    url = url.join(location).map_err(|error| {
                        PublicImageError::InvalidRedirect(format!("{location}: {error}"))
                    })?;
                    redirects += 1;
                    continue;
                }
                return limit_response(response, policy.max_response_bytes);
            }
        })
    }
}

fn limit_response(
    response: Response<AsyncBody>,
    limit: u64,
) -> anyhow::Result<Response<AsyncBody>> {
    if let Some(declared) = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        && declared > limit
    {
        return Err(PublicImageError::DeclaredTooLarge { declared, limit }.into());
    }
    let (parts, body) = response.into_parts();
    let body = match body.0 {
        vespertine_http_client::Inner::AsyncReader(reader) => {
            AsyncBody::from_reader(LimitedReader {
                inner: reader,
                remaining: limit,
                limit,
            })
        }
        vespertine_http_client::Inner::Bytes(cursor) => {
            if cursor.get_ref().len() as u64 > limit {
                return Err(PublicImageError::StreamTooLarge { limit }.into());
            }
            AsyncBody(vespertine_http_client::Inner::Bytes(cursor))
        }
        vespertine_http_client::Inner::Empty => AsyncBody::empty(),
    };
    Ok(Response::from_parts(parts, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncReadExt as _;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::Mutex,
        thread,
    };

    struct TestServer {
        port: u16,
        heads: Arc<Mutex<Vec<String>>>,
    }

    impl TestServer {
        /// Serves each connection with `handler(head, stream)` on its own thread.
        fn start(handler: impl Fn(&str, &mut std::net::TcpStream) + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().unwrap().port();
            let heads = Arc::new(Mutex::new(Vec::new()));
            let handler = Arc::new(handler);
            let recorded = heads.clone();
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let handler = handler.clone();
                    let recorded = recorded.clone();
                    thread::spawn(move || {
                        let mut head = Vec::new();
                        let mut byte = [0u8; 1];
                        while !head.ends_with(b"\r\n\r\n") {
                            match stream.read(&mut byte) {
                                Ok(1) => head.push(byte[0]),
                                _ => return,
                            }
                        }
                        let head = String::from_utf8_lossy(&head).into_owned();
                        recorded.lock().unwrap().push(head.clone());
                        handler(&head, &mut stream);
                    });
                }
            });
            Self { port, heads }
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        fn heads(&self) -> Vec<String> {
            self.heads.lock().unwrap().clone()
        }
    }

    fn request_path(head: &str) -> &str {
        head.split_whitespace().nth(1).unwrap_or("/")
    }

    fn respond(stream: &mut std::net::TcpStream, status: &str, headers: &[String], body: &[u8]) {
        let mut response = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        for header in headers {
            response.push_str(header);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    }

    fn relaxed() -> PublicImageHttpClient {
        PublicImageHttpClient::new(PublicImagePolicy::relaxed_for_tests(), "test-agent")
            .expect("client")
    }

    fn get(client: &PublicImageHttpClient, url: &str) -> anyhow::Result<(StatusCode, Vec<u8>)> {
        futures::executor::block_on(async {
            let request = http::Request::builder()
                .method(Method::GET)
                .uri(url)
                .body(AsyncBody::empty())?;
            let mut response = client.send(request).await?;
            let status = response.status();
            let mut body = Vec::new();
            response.body_mut().read_to_end(&mut body).await?;
            Ok((status, body))
        })
    }

    fn error_text(error: &anyhow::Error) -> String {
        format!("{error:#}")
    }

    #[test]
    fn refused_ip_ranges() {
        let refused_v4 = [
            "127.0.0.1",
            "127.255.255.254",
            "10.0.0.1",
            "10.255.255.255",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "255.255.255.255",
            "192.0.2.7",
            "198.51.100.7",
            "203.0.113.7",
            "224.0.0.1",
            "239.255.255.255",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.255",
            "192.0.0.8",
            "192.88.99.1",
            "198.18.0.1",
            "198.19.255.255",
            "240.0.0.1",
            "250.1.1.1",
        ];
        for text in refused_v4 {
            assert!(!is_public_ip(text.parse().unwrap()), "{text}");
        }
        let refused_v6 = [
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::ffff:192.168.0.1",
            "::127.0.0.1",
            "::10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "2002:7f00:1::",
            "2002:c0a8:101::",
            "ff02::1",
            "ff00::",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "febf::1",
            "fec0::1",
            "feff::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001::1",
        ];
        for text in refused_v6 {
            assert!(!is_public_ip(text.parse().unwrap()), "{text}");
        }
        let allowed = [
            "140.82.112.3",
            "185.199.108.133",
            "::ffff:140.82.112.3",
            "64:ff9b::8c52:7003",
            "2606:50c0:8000::153",
            "100.63.255.255",
            "100.128.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.255",
            "2002:8c52:7003::1",
            "2a00:1450:4001::1",
        ];
        for text in allowed {
            assert!(is_public_ip(text.parse().unwrap()), "{text}");
        }
    }

    #[test]
    fn url_acceptance() {
        let policy = PublicImagePolicy::production();
        let accepted = [
            "https://example.com/a.png",
            "https://images.example.co.uk:8443/x?y=1",
            "https://EXAMPLE.com./a.png",
            "https://140.82.112.3/a.png",
            "https://[2606:50c0:8000::153]/a.png",
        ];
        for text in accepted {
            assert_eq!(
                check_url(&Url::parse(text).unwrap(), &policy),
                Ok(()),
                "{text}"
            );
        }
        let refused = [
            "http://example.com/a.png",
            "https://user:pw@example.com/a.png",
            "https://user@example.com/a.png",
            "https://:pw@example.com/a.png",
            "https://localhost/a.png",
            "https://localhost./a.png",
            "https://printer.local/a.png",
            "https://app.localhost/a.png",
            "https://db.internal/a.png",
            "https://intranet/a.png",
            "https://127.0.0.1/a.png",
            "https://10.0.0.1/a.png",
            "https://192.168.1.1/a.png",
            "https://169.254.169.254/latest/meta-data",
            "https://100.100.1.1/a.png",
            "https://[::1]/a.png",
            "https://[fd00::1]/a.png",
            "https://[::ffff:127.0.0.1]/a.png",
            "ftp://example.com/a.png",
            "file:///etc/passwd",
        ];
        for text in refused {
            assert!(
                matches!(
                    check_url(&Url::parse(text).unwrap(), &policy),
                    Err(PublicImageError::UrlRefused(_))
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn production_policy_values() {
        let policy = PublicImagePolicy::default();
        assert_eq!(policy, PublicImagePolicy::production());
        assert!(policy.require_https && policy.require_public_host);
        assert_eq!(policy.max_redirects, 5);
        assert_eq!(policy.max_response_bytes, 16_777_216);
        assert_eq!(policy.connect_timeout, Duration::from_secs(10));
        assert_eq!(policy.total_timeout, Duration::from_secs(30));
        // The relaxed test policy keeps the limits and is not the default.
        let relaxed = PublicImagePolicy::relaxed_for_tests();
        assert_ne!(relaxed, policy);
        assert_eq!(relaxed.max_redirects, 5);
        assert_eq!(relaxed.max_response_bytes, 16_777_216);
        assert!(PublicImageHttpClient::production("test-agent").is_ok());
        assert!(
            PublicImageHttpClient::production("test-agent")
                .unwrap()
                .proxy()
                .is_none()
        );
    }

    #[test]
    fn production_client_refuses_loopback_before_connecting() {
        let server = TestServer::start(|_, stream| respond(stream, "200 OK", &[], b"ok"));
        let client = PublicImageHttpClient::production("test-agent").unwrap();
        let error = get(&client, &server.url("/")).unwrap_err();
        assert!(error_text(&error).contains("URL refused"), "{error:#}");
        assert!(
            server.heads().is_empty(),
            "the server must not be contacted"
        );
    }

    #[test]
    fn only_get_is_allowed() {
        let server = TestServer::start(|_, stream| respond(stream, "200 OK", &[], b"ok"));
        let client = relaxed();
        let request = http::Request::builder()
            .method(Method::POST)
            .uri(server.url("/"))
            .body(AsyncBody::from(b"payload".to_vec()))
            .unwrap();
        let error = futures::executor::block_on(client.send(request))
            .err()
            .expect("refused");
        assert!(error_text(&error).contains("only GET"), "{error:#}");
        assert!(server.heads().is_empty());
    }

    #[test]
    fn no_credentials_or_referer_are_sent() {
        let server = TestServer::start(|_, stream| respond(stream, "200 OK", &[], b"ok"));
        let client = relaxed();
        let request = http::Request::builder()
            .method(Method::GET)
            .uri(server.url("/img.png"))
            .header("Authorization", "Bearer secret")
            .header("Cookie", "session=1")
            .header("Proxy-Authorization", "Basic abc")
            .header("Referer", "https://app.example/")
            .header("X-Custom", "kept")
            .body(AsyncBody::empty())
            .unwrap();
        let (status, body) = futures::executor::block_on(async {
            let mut response = client.send(request).await.unwrap();
            let mut body = Vec::new();
            response.body_mut().read_to_end(&mut body).await.unwrap();
            (response.status(), body)
        });
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"ok"[..]));
        let head = server.heads().pop().unwrap().to_ascii_lowercase();
        assert!(head.starts_with("get /img.png http/1.1"), "{head}");
        for forbidden in ["authorization", "cookie", "referer", "secret"] {
            assert!(!head.contains(forbidden), "{forbidden} was sent: {head}");
        }
        assert!(head.contains("user-agent: test-agent"));
        assert!(head.contains("x-custom: kept"));
    }

    #[test]
    fn relaxed_policy_follows_a_redirect_to_a_loopback_server() {
        let target = TestServer::start(|_, stream| {
            respond(stream, "200 OK", &["Content-Length: 5".into()], b"hello")
        });
        let target_url = target.url("/final.png");
        let origin = TestServer::start(move |_, stream| {
            respond(
                stream,
                "302 Found",
                &[
                    format!("Location: {target_url}"),
                    "Content-Length: 0".into(),
                ],
                b"",
            )
        });
        let (status, body) = get(&relaxed(), &origin.url("/start")).expect("loaded");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"hello");
        assert_eq!(origin.heads().len(), 1);
        assert_eq!(target.heads().len(), 1);
        assert!(!target.heads()[0].to_ascii_lowercase().contains("referer"));
    }

    fn chain_server(length: usize) -> TestServer {
        // /r0 -> /r1 -> ... -> /r{length}, which answers 200.
        TestServer::start(move |head, stream| {
            let hop: usize = request_path(head)
                .trim_start_matches("/r")
                .parse()
                .unwrap_or(0);
            if hop >= length {
                respond(stream, "200 OK", &["Content-Length: 4".into()], b"done");
            } else {
                respond(
                    stream,
                    "301 Moved Permanently",
                    &[
                        format!("Location: /r{}", hop + 1),
                        "Content-Length: 0".into(),
                    ],
                    b"",
                );
            }
        })
    }

    #[test]
    fn five_redirects_are_followed_and_the_sixth_fails() {
        let five = chain_server(5);
        let (status, body) = get(&relaxed(), &five.url("/r0")).expect("five redirects");
        assert_eq!((status, body.as_slice()), (StatusCode::OK, &b"done"[..]));
        assert_eq!(five.heads().len(), 6);

        let six = chain_server(6);
        let error = get(&relaxed(), &six.url("/r0")).unwrap_err();
        assert!(
            error_text(&error).contains("too many redirects"),
            "{error:#}"
        );
        // The sixth redirect response was received but not followed.
        assert_eq!(six.heads().len(), 6);
    }

    #[test]
    fn every_redirect_hop_is_checked() {
        let secret = TestServer::start(|_, stream| respond(stream, "200 OK", &[], b"secret"));
        let secret_with_credentials = format!("http://user:pw@127.0.0.1:{}/secret", secret.port);
        let origin = TestServer::start(move |head, stream| {
            let location = match request_path(head) {
                "/credentials" => secret_with_credentials.clone(),
                "/scheme" => "ftp://127.0.0.1/file".to_string(),
                _ => "/nowhere".to_string(),
            };
            respond(
                stream,
                "302 Found",
                &[format!("Location: {location}"), "Content-Length: 0".into()],
                b"",
            )
        });
        for path in ["/credentials", "/scheme"] {
            let error = get(&relaxed(), &origin.url(path)).unwrap_err();
            assert!(
                error_text(&error).contains("URL refused"),
                "{path}: {error:#}"
            );
        }
        assert!(
            secret.heads().is_empty(),
            "a refused hop must not be followed"
        );

        // A redirect without a usable Location fails.
        let broken = TestServer::start(|_, stream| {
            respond(stream, "302 Found", &["Content-Length: 0".into()], b"")
        });
        let error = get(&relaxed(), &broken.url("/")).unwrap_err();
        assert!(error_text(&error).contains("invalid redirect"), "{error:#}");
    }

    #[test]
    fn declared_length_over_the_limit_fails_before_reading() {
        let server = TestServer::start(|_, stream| {
            respond(
                stream,
                "200 OK",
                &[format!("Content-Length: {}", MAX_RESPONSE_BYTES + 1)],
                b"tiny",
            )
        });
        let error = get(&relaxed(), &server.url("/big")).unwrap_err();
        assert!(error_text(&error).contains("exceeds"), "{error:#}");
    }

    #[test]
    fn streamed_bytes_over_the_limit_fail_and_the_limit_itself_passes() {
        fn chunked(total: usize) -> TestServer {
            TestServer::start(move |_, stream| {
                let mut head = String::from(
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n",
                );
                let _ = stream.write_all(head.as_bytes());
                head.clear();
                let block = vec![7u8; 1 << 20];
                let mut sent = 0;
                while sent < total {
                    let size = block.len().min(total - sent);
                    if stream
                        .write_all(format!("{size:x}\r\n").as_bytes())
                        .is_err()
                        || stream.write_all(&block[..size]).is_err()
                        || stream.write_all(b"\r\n").is_err()
                    {
                        return;
                    }
                    sent += size;
                }
                let _ = stream.write_all(b"0\r\n\r\n");
            })
        }
        let exact = chunked(MAX_RESPONSE_BYTES as usize);
        let (status, body) = get(&relaxed(), &exact.url("/")).expect("exactly the limit");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len() as u64, MAX_RESPONSE_BYTES);

        let over = chunked(MAX_RESPONSE_BYTES as usize + 1);
        let error = get(&relaxed(), &over.url("/")).unwrap_err();
        assert!(error_text(&error).contains("exceeds"), "{error:#}");
    }

    #[test]
    fn names_resolving_to_loopback_have_no_public_address() {
        let error = lookup_public_addresses("localhost", 443).unwrap_err();
        assert_eq!(error, PublicImageError::NoPublicAddress("localhost".into()));
        assert_eq!(error.to_string(), "no public address for localhost");

        let mixed = filter_public_addresses(
            "mixed.example",
            [
                "127.0.0.1:0",
                "10.1.1.1:0",
                "140.82.112.3:0",
                "[::1]:0",
                "[2606:50c0:8000::153]:0",
            ]
            .map(|text| text.parse::<SocketAddr>().unwrap()),
        )
        .unwrap();
        assert_eq!(
            mixed,
            vec![
                "140.82.112.3:0".parse::<SocketAddr>().unwrap(),
                "[2606:50c0:8000::153]:0".parse::<SocketAddr>().unwrap()
            ]
        );
        assert!(filter_public_addresses("none.example", []).is_err());
    }

    #[test]
    fn the_transport_resolves_through_the_public_only_resolver() {
        // A name that resolves to the loopback address cannot reach the local machine.
        let server = TestServer::start(|_, stream| respond(stream, "200 OK", &[], b"local"));
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .dns_resolver(Arc::new(PublicOnlyResolver))
            .build()
            .unwrap();
        let client = ReqwestClient::from(client);
        let request = http::Request::builder()
            .uri(format!("http://localhost:{}/", server.port))
            .body(AsyncBody::empty())
            .unwrap();
        let error = futures::executor::block_on(client.send(request))
            .err()
            .expect("refused");
        assert!(
            error_text(&error).contains("no public address"),
            "{error:#}"
        );
        assert!(server.heads().is_empty());
    }

    #[test]
    fn dropping_the_request_cancels_it() {
        let accepted = Arc::new(Mutex::new(false));
        let flag = accepted.clone();
        let server = TestServer::start(move |_, _stream| {
            *flag.lock().unwrap() = true;
            // Hold the connection open without answering.
            thread::sleep(Duration::from_millis(500));
        });
        let client = relaxed();
        let request = http::Request::builder()
            .uri(server.url("/slow"))
            .body(AsyncBody::empty())
            .unwrap();
        let mut future = client.send(request);
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        for _ in 0..200 {
            assert!(std::future::Future::poll(future.as_mut(), &mut cx).is_pending());
            if *accepted.lock().unwrap() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(*accepted.lock().unwrap());
        drop(future);
    }
}
