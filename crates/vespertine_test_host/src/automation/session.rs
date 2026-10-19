//! The client side of the stdio transport: request/response correlation over SSE frames,
//! cancellation, closing, and launching a subprocess.

use super::{
    protocol::{AutomationError, Capability, ErrorCode, Message, PROTOCOL_VERSION, method},
    sse::{SseDecoder, encode_frame},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
};

type Callback<T> = Arc<dyn Fn(T) + Send + Sync>;

/// Optional observers of a session.
#[derive(Clone, Default)]
pub struct SessionOptions {
    /// Called with the data of every frame that is not a valid message (otherwise ignored).
    pub on_invalid_frame: Option<Callback<String>>,
    /// Called with every server-originated event (`console`, `frame`, `closed`, ...) as
    /// `(name, params)`. Events a client does not consume are ignored.
    pub on_event: Option<Callback<(String, Value)>>,
    /// Called with every line a launched process writes to its standard error.
    pub on_stderr: Option<Callback<String>>,
}

/// What a command to launch looks like.
#[derive(Clone, Debug, Default)]
pub struct LaunchOptions {
    /// The program to run.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Its working directory; the current one when `None`.
    pub cwd: Option<PathBuf>,
    /// Environment variables merged over the current process environment.
    pub env: Vec<(String, String)>,
}

/// What `initialize` reported.
#[derive(Clone, Debug, PartialEq)]
pub struct InitInfo {
    /// The negotiated protocol version.
    pub protocol_version: i64,
    /// The host process identity.
    pub pid: u64,
    /// What the host can do.
    pub capabilities: Vec<Capability>,
    /// Viewport width in logical pixels.
    pub width: f64,
    /// Viewport height in logical pixels.
    pub height: f64,
}

impl InitInfo {
    /// Reads an `initialize` result.
    pub fn from_json(result: &Value) -> Result<InitInfo, AutomationError> {
        method("initialize")
            .expect("initialize is in the catalog")
            .validate_result(result)?;
        Ok(InitInfo {
            protocol_version: result["protocolVersion"].as_i64().unwrap_or_default(),
            pid: result["pid"].as_u64().unwrap_or_default(),
            capabilities: result["capabilities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|capability| capability.as_str().and_then(Capability::parse))
                .collect(),
            width: result["width"].as_f64().unwrap_or_default(),
            height: result["height"].as_f64().unwrap_or_default(),
        })
    }
}

enum Slot {
    Waiting(&'static str),
    Ready(Result<Value, AutomationError>),
}

struct State {
    next_id: i64,
    pending: HashMap<i64, Slot>,
    closed: bool,
    decoder: SseDecoder,
}

struct Inner {
    writer: Mutex<Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>>,
    state: Mutex<State>,
    changed: Condvar,
    options: SessionOptions,
    child: Mutex<Option<Child>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Inner {
    fn feed(&self, chunk: &[u8]) {
        let mut invalid = Vec::new();
        let mut events = Vec::new();
        {
            let mut state = lock(&self.state);
            let messages = state
                .decoder
                .feed(chunk, |text| invalid.push(text.to_owned()));
            for message in messages {
                match message {
                    Message::Response { id, outcome } => {
                        let Some(id) = id.as_i64() else { continue };
                        if let Some(slot) = state.pending.get_mut(&id)
                            && let Slot::Waiting(method_name) = slot
                        {
                            let outcome = outcome.and_then(|result| {
                                if let Some(method) = method(method_name) {
                                    method.validate_result(&result)?;
                                }
                                Ok(result)
                            });
                            *slot = Slot::Ready(outcome);
                        }
                    }
                    Message::Event { name, params } => events.push((name, params)),
                    // A client never serves requests.
                    Message::Request { .. } => {}
                }
            }
            self.changed.notify_all();
        }
        if let Some(callback) = &self.options.on_invalid_frame {
            for text in invalid {
                callback(text);
            }
        }
        if let Some(callback) = &self.options.on_event {
            for event in events {
                callback(event);
            }
        }
    }

    fn close(&self) {
        {
            let mut state = lock(&self.state);
            state.closed = true;
            for slot in state.pending.values_mut() {
                if matches!(slot, Slot::Waiting(_)) {
                    *slot = Slot::Ready(Err(AutomationError::new(
                        ErrorCode::Closed,
                        "the session was closed",
                    )));
                }
            }
            self.changed.notify_all();
        }
        let child = lock(&self.child).take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), AutomationError> {
        if (lock(&self.writer))(bytes).is_err() {
            self.close();
            return Err(closed());
        }
        Ok(())
    }
}

fn closed() -> AutomationError {
    AutomationError::new(ErrorCode::Closed, "the session is closed")
}

/// A request that was sent and has not completed.
pub struct PendingCall {
    inner: Arc<Inner>,
    id: i64,
}

impl PendingCall {
    /// The request id, usable with [`Session::cancel`].
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Blocks until the response arrives, the request is cancelled or the session closes.
    pub fn wait(self) -> Result<Value, AutomationError> {
        let mut state = lock(&self.inner.state);
        loop {
            match state.pending.get(&self.id) {
                Some(Slot::Ready(_)) => {
                    let Some(Slot::Ready(outcome)) = state.pending.remove(&self.id) else {
                        unreachable!("slot was just observed ready");
                    };
                    return outcome;
                }
                Some(Slot::Waiting(_)) => {
                    state = self
                        .inner
                        .changed
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                None => return Err(closed()),
            }
        }
    }
}

/// A connection to a host speaking the automation protocol over a byte channel.
///
/// Writes go out through the supplied write callback; incoming chunks are pushed with
/// [`Session::feed`] (a launched process feeds itself from its standard output). Chunks may split
/// frames at arbitrary byte positions.
pub struct Session {
    inner: Arc<Inner>,
}

impl Session {
    /// A session over an existing channel: `write` sends bytes to the host; the caller feeds
    /// the host's output to [`Session::feed`]. No `initialize` is performed; call
    /// [`Session::initialize`].
    pub fn over_channel(
        write: impl FnMut(&[u8]) -> io::Result<()> + Send + 'static,
        options: SessionOptions,
    ) -> Session {
        Session {
            inner: Arc::new(Inner {
                writer: Mutex::new(Box::new(write)),
                state: Mutex::new(State {
                    next_id: 1,
                    pending: HashMap::new(),
                    closed: false,
                    decoder: SseDecoder::new(),
                }),
                changed: Condvar::new(),
                options,
                child: Mutex::new(None),
            }),
        }
    }

    /// Launches a process with piped standard input, output and error, merging `env` over the
    /// current environment, running in `cwd` if given. Its output feeds the session; the process
    /// is terminated when the session is closed. No `initialize` is performed.
    #[allow(
        clippy::disallowed_methods,
        reason = "launching is a blocking client operation by design: it runs on the caller's \
                  thread and returns once the process is spawned"
    )]
    pub fn launch(launch: &LaunchOptions, options: SessionOptions) -> io::Result<Session> {
        let mut command = Command::new(&launch.command);
        command
            .args(&launch.args)
            .envs(launch.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &launch.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let mut stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let session = Session::over_channel(
            move |bytes| {
                stdin.write_all(bytes)?;
                stdin.flush()
            },
            options,
        );
        *lock(&session.inner.child) = Some(child);

        let inner = session.inner.clone();
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => inner.feed(&buffer[..read]),
                }
            }
            inner.close();
        });
        let inner = session.inner.clone();
        thread::spawn(move || {
            let mut text = String::new();
            let mut stderr = io::BufReader::new(stderr);
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                match io::BufRead::read_until(&mut stderr, b'\n', &mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        text.clear();
                        text.push_str(String::from_utf8_lossy(&bytes).trim_end());
                        if let Some(callback) = &inner.options.on_stderr {
                            callback(text.clone());
                        }
                    }
                }
            }
        });
        Ok(session)
    }

    /// Pushes a chunk of the host's output into the session.
    pub fn feed(&self, chunk: &[u8]) {
        self.inner.feed(chunk);
    }

    /// Whether the session is closed.
    pub fn is_closed(&self) -> bool {
        lock(&self.inner.state).closed
    }

    /// Validates and sends a request, returning the pending call without waiting.
    pub fn begin(&self, method_name: &str, params: Value) -> Result<PendingCall, AutomationError> {
        let method = super::protocol::method(method_name).ok_or_else(|| {
            AutomationError::protocol_issues(
                format!("unknown method {method_name:?}"),
                vec![format!("method: unknown method {method_name:?}")],
            )
        })?;
        method.validate_params(&params)?;
        let id = {
            let mut state = lock(&self.inner.state);
            if state.closed {
                return Err(closed());
            }
            let id = state.next_id;
            state.next_id += 1;
            state.pending.insert(id, Slot::Waiting(method.name));
            id
        };
        let frame = encode_frame(&Message::Request {
            id: json!(id),
            method: method.name.to_owned(),
            params,
        });
        if let Err(error) = self.inner.write(&frame) {
            lock(&self.inner.state).pending.remove(&id);
            return Err(error);
        }
        Ok(PendingCall {
            inner: self.inner.clone(),
            id,
        })
    }

    /// Sends a request and waits for its outcome.
    pub fn call(&self, method_name: &str, params: Value) -> Result<Value, AutomationError> {
        self.begin(method_name, params)?.wait()
    }

    /// Negotiates protocol version 1 and reads the process identity, capabilities and viewport.
    pub fn initialize(&self, client: &str) -> Result<InitInfo, AutomationError> {
        let result = self.call(
            "initialize",
            json!({ "protocolVersion": PROTOCOL_VERSION, "client": client }),
        )?;
        InitInfo::from_json(&result)
    }

    /// Completes the pending request `id` with a `Cancelled` error and tells the host to cancel
    /// it. Returns whether a pending request was cancelled.
    pub fn cancel(&self, id: i64) -> bool {
        let cancelled = {
            let mut state = lock(&self.inner.state);
            match state.pending.get_mut(&id) {
                Some(slot @ Slot::Waiting(_)) => {
                    *slot = Slot::Ready(Err(AutomationError::new(
                        ErrorCode::Cancelled,
                        format!("request {id} was cancelled"),
                    )));
                    self.inner.changed.notify_all();
                    true
                }
                _ => false,
            }
        };
        if cancelled {
            let cancel_id = {
                let mut state = lock(&self.inner.state);
                let cancel_id = state.next_id;
                state.next_id += 1;
                cancel_id
            };
            let frame = encode_frame(&Message::Request {
                id: json!(cancel_id),
                method: "cancel".to_owned(),
                params: json!({ "id": id }),
            });
            // Best effort: the caller is already completed with Cancelled.
            let _ = self.inner.write(&frame);
        }
        cancelled
    }

    /// Closes the session: every outstanding request fails with `Closed`, further calls fail with
    /// `Closed`, and an owned process is terminated.
    pub fn close(&self) {
        self.inner.close();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.inner.close();
    }
}
