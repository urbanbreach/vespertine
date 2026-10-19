//! Test doubles shared by the updater's integration tests: a scripted local HTTP server, a
//! Minisign signer, an injectable environment and package/executable helpers.

#![allow(
    dead_code,
    reason = "each test file uses a subset of the shared support"
)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::Permissions;
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use blake2::{Blake2b512, Digest as _};
use ed25519_dalek::{Signer as _, SigningKey};
use vespertine_http_client::HttpClient;
use vespertine_reqwest_client::ReqwestClient;
use vespertine_updater::{Environment, Platform, Updater};

// ---------------------------------------------------------------------------------------
// HTTP server

/// A request as received by the server.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub target: String,
    /// Header names are lowercase.
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

/// What the server answers.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    /// Waits this long before answering (to provoke client timeouts).
    pub stall: Option<Duration>,
}

impl Reply {
    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
            stall: None,
        }
    }

    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            stall: None,
        }
    }

    pub fn stalled(duration: Duration) -> Self {
        Self {
            status: 200,
            body: b"{}".to_vec(),
            stall: Some(duration),
        }
    }
}

type Handler = dyn Fn(&str, &Recorded) -> Reply + Send + Sync;

/// A local HTTP server on an ephemeral port. The handler receives the server's base URL
/// (`http://127.0.0.1:<port>`) and the request.
pub struct Server {
    base: String,
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub fn start(handler: impl Fn(&str, &Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        {
            let (base, requests, stop) = (base.clone(), requests.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let (base, requests, handler) =
                        (base.clone(), requests.clone(), handler.clone());
                    std::thread::spawn(move || serve(stream, &base, &requests, &*handler));
                }
            });
        }
        Self {
            base,
            address,
            requests,
            stop,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    pub fn targets(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|request| request.target)
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so its thread ends.
        TcpStream::connect_timeout(&self.address, Duration::from_millis(200)).ok();
    }
}

fn serve(stream: TcpStream, base: &str, requests: &Mutex<Vec<Recorded>>, handler: &Handler) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let request = Recorded {
        method,
        target,
        headers,
    };
    requests.lock().unwrap().push(request.clone());
    let reply = handler(base, &request);
    if let Some(stall) = reply.stall {
        std::thread::sleep(stall);
    }
    let reason = match reply.status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    };
    let mut stream = stream;
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.body.len()
    );
    stream.write_all(head.as_bytes()).ok();
    stream.write_all(&reply.body).ok();
    stream.flush().ok();
}

/// A URL on a port nothing listens on (connection refused).
pub fn closed_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{address}/feed")
}

// ---------------------------------------------------------------------------------------
// Minisign

/// A Minisign key pair with a fixed seed and key id, producing the base64 texts the updater
/// takes. Signatures follow the Minisign file format (legacy `Ed` and prehashed `ED`).
pub struct TestKey {
    signing: SigningKey,
    key_id: [u8; 8],
}

impl TestKey {
    pub fn new(seed: u8, key_id: [u8; 8]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&[seed; 32]),
            key_id,
        }
    }

    pub fn with_key_id_of(seed: u8, other: &TestKey) -> Self {
        Self::new(seed, other.key_id)
    }

    /// The text of a `minisign.pub` file.
    pub fn public_key_file(&self) -> String {
        let mut raw = b"Ed".to_vec();
        raw.extend_from_slice(&self.key_id);
        raw.extend_from_slice(self.signing.verifying_key().as_bytes());
        format!(
            "untrusted comment: minisign public key\n{}\n",
            STANDARD.encode(raw)
        )
    }

    /// The public key as the updater takes it: the base64 of the `minisign.pub` contents.
    pub fn public_key(&self) -> String {
        STANDARD.encode(self.public_key_file())
    }

    /// The text of a `.sig` file for `data`.
    pub fn signature_file(&self, data: &[u8], prehashed: bool) -> String {
        let message = if prehashed {
            Blake2b512::digest(data).to_vec()
        } else {
            data.to_vec()
        };
        let signature = self.signing.sign(&message).to_bytes();
        let mut raw = if prehashed {
            b"ED".to_vec()
        } else {
            b"Ed".to_vec()
        };
        raw.extend_from_slice(&self.key_id);
        raw.extend_from_slice(&signature);
        let trusted_comment = "timestamp:1700000000\tfile:package";
        let mut global = signature.to_vec();
        global.extend_from_slice(trusted_comment.as_bytes());
        let global_signature = self.signing.sign(&global).to_bytes();
        format!(
            "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {trusted_comment}\n{}\n",
            STANDARD.encode(raw),
            STANDARD.encode(global_signature)
        )
    }

    /// The signature as the updater takes it: the base64 of the `.sig` contents.
    pub fn signature(&self, data: &[u8], prehashed: bool) -> String {
        STANDARD.encode(self.signature_file(data, prehashed))
    }
}

pub fn key() -> TestKey {
    TestKey::new(7, *b"VESPERT1")
}

pub fn other_key() -> TestKey {
    TestKey::new(9, *b"OTHERKEY")
}

// ---------------------------------------------------------------------------------------
// Environment

type DeviceFn = dyn Fn(&Path) -> Option<u64> + Send + Sync;
type WriteFn = dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync;
type PermissionsFn = dyn Fn(&Path, &Permissions) -> io::Result<()> + Send + Sync;

/// An [`Environment`] over explicit variables, with hooks to fake devices and to fail file
/// operations. Nothing here touches the real process environment.
pub struct TestEnvironment {
    vars: Mutex<BTreeMap<String, OsString>>,
    temp_dir: Option<PathBuf>,
    exe: PathBuf,
    devices: Option<Box<DeviceFn>>,
    write: Option<Box<WriteFn>>,
    permissions: Option<Box<PermissionsFn>>,
    set_calls: Mutex<Vec<String>>,
}

impl TestEnvironment {
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self {
            vars: Mutex::new(BTreeMap::new()),
            temp_dir: None,
            exe: exe.into(),
            devices: None,
            write: None,
            permissions: None,
            set_calls: Mutex::new(Vec::new()),
        }
    }

    pub fn var(self, name: &str, value: impl Into<OsString>) -> Self {
        self.vars
            .lock()
            .unwrap()
            .insert(name.to_string(), value.into());
        self
    }

    pub fn temp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = Some(dir.into());
        self
    }

    /// Overrides the device id of a path; `None` falls back to the real device.
    pub fn devices(
        mut self,
        devices: impl Fn(&Path) -> Option<u64> + Send + Sync + 'static,
    ) -> Self {
        self.devices = Some(Box::new(devices));
        self
    }

    pub fn on_write(
        mut self,
        write: impl Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.write = Some(Box::new(write));
        self
    }

    pub fn on_set_permissions(
        mut self,
        permissions: impl Fn(&Path, &Permissions) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.permissions = Some(Box::new(permissions));
        self
    }

    /// Variables the updater set (`NAME=value`), in order.
    pub fn set_calls(&self) -> Vec<String> {
        self.set_calls.lock().unwrap().clone()
    }

    pub fn current_var(&self, name: &str) -> Option<OsString> {
        self.vars.lock().unwrap().get(name).cloned()
    }
}

impl Environment for TestEnvironment {
    fn var(&self, name: &str) -> Option<OsString> {
        self.vars.lock().unwrap().get(name).cloned()
    }

    fn set_var(&self, name: &str, value: &str) {
        self.set_calls
            .lock()
            .unwrap()
            .push(format!("{name}={value}"));
        self.vars
            .lock()
            .unwrap()
            .insert(name.to_string(), value.into());
    }

    fn temp_dir(&self) -> Option<PathBuf> {
        self.temp_dir.clone()
    }

    fn current_exe(&self) -> io::Result<PathBuf> {
        Ok(self.exe.clone())
    }

    fn device_id(&self, path: &Path) -> io::Result<u64> {
        use std::os::unix::fs::MetadataExt as _;
        if let Some(device) = self.devices.as_ref().and_then(|devices| devices(path)) {
            return Ok(device);
        }
        Ok(std::fs::metadata(path)?.dev())
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match &self.write {
            Some(write) => write(path, bytes),
            None => std::fs::write(path, bytes),
        }
    }

    fn set_permissions(&self, path: &Path, permissions: &Permissions) -> io::Result<()> {
        match &self.permissions {
            Some(hook) => hook(path, permissions),
            None => std::fs::set_permissions(path, permissions.clone()),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Helpers

pub fn linux_x86_64() -> Platform {
    Platform::from_parts("linux", "x86_64").unwrap()
}

/// An updater over the production HTTP client (so timeouts, redirects and TLS plumbing are
/// the real ones) for `linux-x86_64`.
pub fn updater(environment: Arc<dyn Environment>) -> Updater {
    let client: Arc<dyn HttpClient> = Arc::new(ReqwestClient::new());
    Updater::new(client)
        .unwrap()
        .with_platform(linux_x86_64())
        .with_environment(environment)
}

/// An updater whose environment never touches the machine (no APPIMAGE, no files).
pub fn check_only_updater() -> Updater {
    updater(Arc::new(TestEnvironment::new("/nonexistent/app")))
}

pub fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    futures::executor::block_on(future)
}

pub fn dynamic_manifest(version: &str, url: &str, signature: &str, format: &str) -> String {
    serde_json::json!({
        "version": version,
        "url": url,
        "signature": signature,
        "format": format,
        "notes": "Release notes",
        "pub_date": "2025-01-02T03:04:05Z",
    })
    .to_string()
}

/// Writes an executable file with `contents` and `mode` into `dir`.
pub fn write_executable(dir: &Path, name: &str, contents: &[u8], mode: u32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    std::fs::set_permissions(&path, Permissions::from_mode(mode)).unwrap();
    path
}

pub fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Names of the staging directories (`current_app*`) left in `dir`.
pub fn staging_leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("current_app"))
                .collect()
        })
        .unwrap_or_default()
}

/// An HTTP client that records the URIs it is asked for and answers every request with a
/// fixed status, never touching the network.
pub struct RecordingClient {
    status: u16,
    uris: Mutex<Vec<String>>,
}

impl RecordingClient {
    pub fn new(status: u16) -> Arc<Self> {
        Arc::new(Self {
            status,
            uris: Mutex::new(Vec::new()),
        })
    }

    pub fn uris(&self) -> Vec<String> {
        self.uris.lock().unwrap().clone()
    }
}

impl HttpClient for RecordingClient {
    fn user_agent(&self) -> Option<&vespertine_http_client::http::HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&vespertine_http_client::Url> {
        None
    }

    fn send(
        &self,
        request: vespertine_http_client::Request<vespertine_http_client::AsyncBody>,
    ) -> futures::future::BoxFuture<
        'static,
        vespertine_http_client::Result<
            vespertine_http_client::Response<vespertine_http_client::AsyncBody>,
        >,
    > {
        self.uris.lock().unwrap().push(request.uri().to_string());
        let response = vespertine_http_client::Response::builder()
            .status(self.status)
            .body(vespertine_http_client::AsyncBody::empty())
            .map_err(Into::into);
        Box::pin(async move { response })
    }
}
