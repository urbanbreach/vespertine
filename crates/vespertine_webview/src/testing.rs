//! In-process scripted helpers for tests of embedders (CMP-022 and others).
//!
//! A [`Peer`] sits on one end of a socket pair inside the test process and speaks exactly the
//! helper protocol of PLT-015: it sends the ready message, records every command it receives and
//! sends whatever messages the test injects. [`ScriptedStartup`] provides a [`BrowserContext`]
//! whose helpers are such peers, with a gate that withholds the ready message and a switchable
//! startup failure, so startup rules can be tested without an engine or a child process.

use crate::helper::Connector;
use crate::protocol::{
    Direction, encode_inline_frame, encode_message, kind, read_message, send_with_fds, write_all,
};
use crate::{BrowserContext, Helper, HelperConfig};
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Polls `condition` every few milliseconds until it holds or `timeout` passed.
pub fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Behaviours of a scripted peer beyond recording commands.
#[derive(Clone, Debug, Default)]
pub struct PeerScript {
    /// Answer `load` with the state messages a real helper sends (loading, then done with the
    /// title "Scripted <url>").
    pub echo_load: bool,
    /// Answer `resize` with an inline frame filled with this 0xRRGGBB colour.
    pub frame_color: Option<u32>,
    /// Answer `ping` with a pong.
    pub answer_pings: bool,
}

/// The helper side of a socket pair, run by the test.
pub struct Peer {
    writer: UnixStream,
    commands: Arc<Mutex<Vec<(u32, Value)>>>,
}

impl Peer {
    /// Writes the ready message and runs the reader thread; returns the application's end.
    fn run(ready: Option<Value>, error: Option<String>, script: PeerScript) -> (UnixStream, Peer) {
        let (ours, theirs) = UnixStream::pair().expect("socket pair");
        let writer = theirs.try_clone().expect("clone socket");
        let commands: Arc<Mutex<Vec<(u32, Value)>>> = Arc::default();
        let log = commands.clone();
        let first = match (&error, &ready) {
            (Some(message), _) => {
                encode_message(Direction::ToApplication, kind::ERROR, 0, message.as_bytes())
            }
            (None, Some(ready)) => encode_message(
                Direction::ToApplication,
                kind::READY,
                0,
                ready.to_string().as_bytes(),
            ),
            (None, None) => encode_message(Direction::ToApplication, kind::READY, 0, b""),
        }
        .expect("first message fits");
        write_all(theirs.as_raw_fd(), &first).expect("write the first message");
        let reply = writer.try_clone().expect("clone socket");
        std::thread::Builder::new()
            .name("scripted-peer".into())
            .spawn(move || {
                let mut source = &theirs;
                while let Ok(message) = read_message(&mut source, Direction::ToHelper) {
                    let Some(object) = message.json_object() else {
                        continue;
                    };
                    let command = Value::Object(object);
                    let page = message.page;
                    let send = |kind: u8, page: u32, payload: &[u8]| {
                        if let Ok(bytes) =
                            encode_message(Direction::ToApplication, kind, page, payload)
                        {
                            let _ = write_all(reply.as_raw_fd(), &bytes);
                        }
                    };
                    match command["cmd"].as_str() {
                        Some("load") if script.echo_load => {
                            let url = command["url"].as_str().unwrap_or_default();
                            send(
                                kind::STATE,
                                page,
                                json!({"url": url, "title": "", "error": null, "loading": true,
                                       "can_back": false, "can_forward": false})
                                .to_string()
                                .as_bytes(),
                            );
                            send(
                                kind::STATE,
                                page,
                                json!({"url": url, "title": format!("Scripted {url}"),
                                       "error": null, "loading": false,
                                       "can_back": true, "can_forward": false})
                                .to_string()
                                .as_bytes(),
                            );
                        }
                        Some("resize") => {
                            if let Some(color) = script.frame_color {
                                let width = command["width"].as_u64().unwrap_or(1) as u32;
                                let height = command["height"].as_u64().unwrap_or(1) as u32;
                                let scale = command["scale"].as_f64().unwrap_or(1.0) as f32;
                                let pixel =
                                    [color as u8, (color >> 8) as u8, (color >> 16) as u8, 255];
                                let pixels: Vec<u8> = pixel
                                    .iter()
                                    .copied()
                                    .cycle()
                                    .take(width as usize * height as usize * 4)
                                    .collect();
                                send(
                                    kind::FRAME,
                                    page,
                                    &encode_inline_frame(width, height, scale, &pixels),
                                );
                            }
                        }
                        Some("ping") if script.answer_pings => {
                            send(
                                kind::PONG,
                                0,
                                json!({"nonce": command["nonce"]}).to_string().as_bytes(),
                            );
                        }
                        _ => {}
                    }
                    log.lock().push((page, command));
                }
            })
            .expect("spawn the scripted peer");
        (ours, Peer { writer, commands })
    }

    /// Starts a peer advertising inline and shared-memory transfer and the "eval" capability
    /// and attaches a helper to it.
    pub fn start() -> (Arc<Helper>, Peer) {
        Self::start_with(json!({
            "protocol": 1, "engine": "scripted",
            "transfers": ["inline", "shm"], "capabilities": ["eval"],
        }))
    }

    /// Starts a peer with the given ready payload and attaches a helper to it.
    pub fn start_with(ready: Value) -> (Arc<Helper>, Peer) {
        let (ours, peer) = Self::run(Some(ready), None, PeerScript::default());
        let helper = Helper::attach(ours, None, Instant::now(), HelperConfig::default())
            .expect("scripted handshake");
        (Arc::new(helper), peer)
    }

    /// Sends a message of `kind` for `page`.
    pub fn send(&self, kind: u8, page: u32, payload: &[u8]) {
        let bytes = encode_message(Direction::ToApplication, kind, page, payload)
            .expect("injected message fits");
        write_all(self.writer.as_raw_fd(), &bytes).expect("peer write");
    }

    /// Sends a JSON message.
    pub fn send_json(&self, kind: u8, page: u32, value: Value) {
        self.send(kind, page, value.to_string().as_bytes());
    }

    /// Sends a message with file descriptors attached (pool announcements).
    pub fn send_with_fds(&self, kind: u8, page: u32, payload: &[u8], fds: &[RawFd]) {
        let bytes = encode_message(Direction::ToApplication, kind, page, payload)
            .expect("injected message fits");
        send_with_fds(self.writer.as_raw_fd(), &bytes, fds).expect("peer write");
    }

    /// Simulates the helper dying: closes the helper side of the channel.
    pub fn kill_channel(&self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }

    /// Every command received so far as `(page, command json)`.
    pub fn commands(&self) -> Vec<(u32, Value)> {
        self.commands.lock().clone()
    }

    /// Commands with the given "cmd" name received so far.
    pub fn named(&self, name: &str) -> Vec<Value> {
        self.commands()
            .into_iter()
            .filter(|(_, command)| command["cmd"] == name)
            .map(|(_, command)| command)
            .collect()
    }

    /// Commands with the given name for one page.
    pub fn named_for(&self, page: u32, name: &str) -> Vec<Value> {
        self.commands()
            .into_iter()
            .filter(|(id, command)| *id == page && command["cmd"] == name)
            .map(|(_, command)| command)
            .collect()
    }

    /// Waits (up to five seconds) until at least `count` commands named `name` arrived and
    /// returns them; panics on timeout.
    pub fn wait_named(&self, name: &str, count: usize) -> Vec<Value> {
        assert!(
            wait_until(Duration::from_secs(5), || self.named(name).len() >= count),
            "timed out waiting for {count} {name} command(s); got {:?}",
            self.commands()
        );
        self.named(name)
    }

    /// Waits a moment and asserts that exactly `count` commands named `name` have arrived.
    pub fn assert_no_more(&self, name: &str, count: usize) {
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(self.named(name).len(), count, "unexpected {name} commands");
    }
}

struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

/// Scripted startup for contexts: every helper the context starts is an in-process [`Peer`].
pub struct ScriptedStartup {
    gate: Gate,
    gated: bool,
    failure: Mutex<Option<String>>,
    script: PeerScript,
    peers: Mutex<Vec<Arc<Peer>>>,
}

impl ScriptedStartup {
    /// Helpers become ready at once; loads are answered with state messages.
    pub fn new() -> Arc<Self> {
        Self::with(
            false,
            PeerScript {
                echo_load: true,
                ..PeerScript::default()
            },
        )
    }

    /// Helpers withhold their ready message until [`ScriptedStartup::release`].
    pub fn gated() -> Arc<Self> {
        Self::with(
            true,
            PeerScript {
                echo_load: true,
                ..PeerScript::default()
            },
        )
    }

    pub fn with(gated: bool, script: PeerScript) -> Arc<Self> {
        Arc::new(Self {
            gate: Gate {
                open: Mutex::new(!gated),
                changed: Condvar::new(),
            },
            gated,
            failure: Mutex::new(None),
            script,
            peers: Mutex::new(Vec::new()),
        })
    }

    /// Lets withheld ready messages through.
    pub fn release(&self) {
        *self.gate.open.lock() = true;
        self.gate.changed.notify_all();
    }

    /// While set, helpers answer startup with this error message instead of ready.
    pub fn set_failure(&self, message: Option<&str>) {
        *self.failure.lock() = message.map(str::to_owned);
    }

    /// A configuration whose helpers are scripted peers.
    pub fn config(self: &Arc<Self>) -> HelperConfig {
        let startup = self.clone();
        HelperConfig {
            connector: Some(Connector(Arc::new(move |_config| {
                if startup.gated {
                    let mut open = startup.gate.open.lock();
                    while !*open {
                        startup.gate.changed.wait(&mut open);
                    }
                }
                let error = startup.failure.lock().clone();
                let (ours, peer) = Peer::run(
                    Some(json!({
                        "protocol": 1, "engine": "scripted",
                        "transfers": ["inline", "shm"], "capabilities": ["eval"],
                    })),
                    error,
                    startup.script.clone(),
                );
                startup.peers.lock().push(Arc::new(peer));
                Ok((ours, None))
            }))),
            ..HelperConfig::default()
        }
    }

    /// An ephemeral context whose helpers are scripted peers.
    pub fn context(self: &Arc<Self>) -> BrowserContext {
        BrowserContext::with_config(self.config())
    }

    /// The peers started so far, oldest first (one per helper).
    pub fn peers(&self) -> Vec<Arc<Peer>> {
        self.peers.lock().clone()
    }

    /// Every command of every peer for `name`, as `(helper index, page, command)`.
    pub fn all_named(&self, name: &str) -> Vec<(usize, u32, Value)> {
        let mut found = Vec::new();
        for (index, peer) in self.peers().iter().enumerate() {
            for (page, command) in peer.commands() {
                if command["cmd"] == name {
                    found.push((index, page, command));
                }
            }
        }
        found
    }
}
