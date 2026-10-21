//! The helper process: spawn, startup handshake, reader/writer/monitor threads, hang detection
//! and shutdown (PLT-014 R5, R6, R11 to R14).

use crate::extract;
use crate::page::{NativePage, PageShared};
use crate::protocol::{
    self, Command, Direction, MAX_STARTUP_PAYLOAD, Message, ProtocolError, Ready, Transfer, kind,
};
use crate::state::{ERROR_HELPER_HUNG, ERROR_HELPER_STOPPED};
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Outgoing queue bound: messages.
pub const OUTGOING_MAX_MESSAGES: usize = 1024;
/// Outgoing queue bound: bytes.
pub const OUTGOING_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Timing parameters (PLT-014 Parameters). Tests shorten them; the defaults are normative.
#[derive(Clone, Copy, Debug)]
pub struct Timings {
    /// Total handshake deadline measured from process spawn.
    pub handshake: Duration,
    /// Interval between ping commands while a helper has pages.
    pub ping_interval: Duration,
    /// Time after the first unanswered ping at which the helper is considered hung.
    pub hang_deadline: Duration,
    /// How long a closed helper may take to exit before it is killed.
    pub shutdown_grace: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            handshake: Duration::from_secs(10),
            ping_interval: Duration::from_secs(2),
            hang_deadline: Duration::from_secs(8),
            shutdown_grace: Duration::from_secs(3),
        }
    }
}

/// A function that provides the helper's end of the channel instead of spawning an executable:
/// for embedders that supervise the helper themselves and for in-process scripted helpers in
/// tests. It runs on the startup thread and may block (for example to withhold the ready
/// message); the optional child is killed and reaped on failure like a spawned helper.
#[derive(Clone)]
pub struct Connector(
    pub Arc<dyn Fn(&HelperConfig) -> Result<(UnixStream, Option<Child>), String> + Send + Sync>,
);

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Connector")
    }
}

/// How to start a helper.
#[derive(Clone, Debug)]
pub struct HelperConfig {
    /// Explicit helper executable; otherwise the embedded helper is extracted to the cache.
    pub helper_path: Option<PathBuf>,
    /// Absolute profile root (`--profile`); `None` starts an ephemeral engine context.
    pub profile_root: Option<PathBuf>,
    /// Transfers the application accepts, most preferred first among the helper's advertised ones.
    pub transfers: Vec<Transfer>,
    /// Per-application cache subdirectory name used for extraction; defaults to the executable
    /// name.
    pub app_name: Option<String>,
    /// Extra environment variables for the helper process (used by tests to script the
    /// synthetic helper).
    pub env: Vec<(String, String)>,
    /// Replaces spawning an executable (see [`Connector`]); `helper_path` is then unused.
    pub connector: Option<Connector>,
    pub timings: Timings,
}

impl Default for HelperConfig {
    fn default() -> Self {
        Self {
            helper_path: None,
            profile_root: None,
            // shm when available, else inline.
            transfers: vec![Transfer::SharedMemory, Transfer::Inline],
            app_name: None,
            env: Vec::new(),
            connector: None,
            timings: Timings::default(),
        }
    }
}

impl HelperConfig {
    /// The mode to configure: shared memory when the helper advertises it and the application
    /// allows it, else inline, the mandatory baseline.
    pub fn select_transfer(&self, ready: &Ready) -> Transfer {
        if self.transfers.contains(&Transfer::SharedMemory)
            && ready.transfers.contains(&Transfer::SharedMemory)
        {
            Transfer::SharedMemory
        } else {
            Transfer::Inline
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Outgoing queue
// ---------------------------------------------------------------------------------------------

#[derive(Copy, Clone, Eq, PartialEq)]
enum Coalesce {
    None,
    Move,
    Resize,
}

struct Entry {
    bytes: Vec<u8>,
    page: u32,
    coalesce: Coalesce,
}

#[derive(Default)]
struct QueueState {
    entries: VecDeque<Entry>,
    bytes: usize,
    closed: bool,
}

/// The bounded queue drained by the writer thread.
struct Outgoing {
    state: Mutex<QueueState>,
    ready: Condvar,
}

enum PushError {
    Closed,
    Overflow,
}

impl Outgoing {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState::default()),
            ready: Condvar::new(),
        }
    }

    fn push(&self, entry: Entry) -> Result<(), PushError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(PushError::Closed);
        }
        if entry.coalesce != Coalesce::None
            && let Some(last) = state.entries.back_mut()
            && last.page == entry.page
            && last.coalesce == entry.coalesce
        {
            let old = last.bytes.len();
            let new = entry.bytes.len();
            *last = entry;
            state.bytes = state.bytes - old + new;
            self.ready.notify_one();
            return Ok(());
        }
        if state.entries.len() >= OUTGOING_MAX_MESSAGES
            || state.bytes + entry.bytes.len() > OUTGOING_MAX_BYTES
        {
            return Err(PushError::Overflow);
        }
        state.bytes += entry.bytes.len();
        state.entries.push_back(entry);
        self.ready.notify_one();
        Ok(())
    }

    /// Blocks until a message is available; `None` once the queue is closed.
    fn pop(&self) -> Option<Entry> {
        let mut state = self.state.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(entry) = state.entries.pop_front() {
                state.bytes -= entry.bytes.len();
                return Some(entry);
            }
            self.ready.wait(&mut state);
        }
    }

    fn close(&self) {
        let mut state = self.state.lock();
        state.closed = true;
        state.entries.clear();
        state.bytes = 0;
        self.ready.notify_all();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.state.lock().entries.len()
    }
}

// ---------------------------------------------------------------------------------------------
// Helper state
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Lifecycle {
    failed: Option<String>,
    closing: bool,
}

#[derive(Default)]
struct PingState {
    next_nonce: u64,
    newest: u64,
    last_sent: Option<Instant>,
    unanswered_since: Option<Instant>,
}

pub(crate) struct HelperShared {
    pid: u32,
    socket: UnixStream,
    child: Mutex<Option<Child>>,
    ready: Ready,
    config: HelperConfig,
    transfer: Mutex<Transfer>,
    lifecycle: Mutex<Lifecycle>,
    pages: Mutex<HashMap<u32, Weak<PageShared>>>,
    next_page: AtomicU32,
    outgoing: Outgoing,
    ping: Mutex<PingState>,
}

impl HelperShared {
    fn failure(&self) -> Option<String> {
        self.lifecycle.lock().failed.clone()
    }

    fn is_closing(&self) -> bool {
        self.lifecycle.lock().closing
    }

    /// Sends a command; never blocks on the helper.
    pub(crate) fn send(&self, page: u32, command: &Command) -> Result<(), String> {
        if let Some(failure) = self.failure() {
            return Err(failure);
        }
        let bytes = command.encode(page).map_err(|error| error.to_string())?;
        let coalesce = match command {
            Command::Move { .. } => Coalesce::Move,
            Command::Resize { .. } => Coalesce::Resize,
            _ => Coalesce::None,
        };
        match self.outgoing.push(Entry {
            bytes,
            page,
            coalesce,
        }) {
            Ok(()) => Ok(()),
            Err(PushError::Closed) => Err("The browser helper is closed".to_owned()),
            Err(PushError::Overflow) => {
                self.fail(ERROR_HELPER_HUNG);
                Err(ERROR_HELPER_HUNG.to_owned())
            }
        }
    }

    pub(crate) fn current_transfer(&self) -> Transfer {
        *self.transfer.lock()
    }

    /// Falls back to the next transfer mode down after a rejected pool (PLT-015 R9, R14).
    pub(crate) fn fall_back_to_inline(&self) {
        let mut transfer = self.transfer.lock();
        if *transfer != Transfer::Inline {
            *transfer = Transfer::Inline;
            drop(transfer);
            let _ = self.send(
                0,
                &Command::Configure {
                    transfer: Transfer::Inline,
                },
            );
        }
    }

    /// Marks the helper failed: every live page shows `message`, pending output is discarded, the
    /// channel is closed and the child is killed and reaped.
    pub(crate) fn fail(&self, message: &str) {
        {
            let mut lifecycle = self.lifecycle.lock();
            if lifecycle.failed.is_some() || lifecycle.closing {
                return;
            }
            lifecycle.failed = Some(message.to_owned());
        }
        self.outgoing.close();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        for page in self.live_pages() {
            page.helper_failed(message);
        }
        self.kill_and_reap();
    }

    fn kill_and_reap(&self) {
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub(crate) fn live_pages(&self) -> Vec<Arc<PageShared>> {
        let mut pages = self.pages.lock();
        pages.retain(|_, page| page.strong_count() > 0);
        pages.values().filter_map(Weak::upgrade).collect()
    }

    pub(crate) fn register_page(&self, page: &Arc<PageShared>) {
        self.pages.lock().insert(page.id(), Arc::downgrade(page));
    }

    pub(crate) fn unregister_page(&self, id: u32) {
        self.pages.lock().remove(&id);
    }

    fn page(&self, id: u32) -> Option<Arc<PageShared>> {
        self.pages.lock().get(&id).and_then(Weak::upgrade)
    }

    fn has_exited(&self) -> bool {
        match self.child.lock().as_mut() {
            Some(child) => !matches!(child.try_wait(), Ok(None)),
            None => self.pid != 0,
        }
    }

    fn dispatch(self: &Arc<Self>, message: Message) {
        match message.kind {
            kind::PONG => {
                if let Some(object) = message.json_object()
                    && let Some(nonce) = object.get("nonce").and_then(|v| v.as_u64())
                {
                    let mut ping = self.ping.lock();
                    if nonce == ping.newest {
                        ping.unanswered_since = None;
                    }
                }
            }
            kind::READY | kind::ERROR => {}
            _ => {
                let Some(page) = self.page(message.page) else {
                    return;
                };
                page.receive(self, message);
            }
        }
    }

    fn reader_loop(self: Arc<Self>) {
        let mut source = &self.socket;
        loop {
            match protocol::read_message(&mut source, Direction::ToApplication) {
                Ok(message) => self.dispatch(message),
                Err(error) => {
                    if !self.is_closing() {
                        log::warn!("browser helper {} channel failed: {error}", self.pid);
                        self.fail(ERROR_HELPER_STOPPED);
                    }
                    return;
                }
            }
        }
    }

    fn writer_loop(self: Arc<Self>) {
        let fd = self.socket.as_raw_fd();
        while let Some(entry) = self.outgoing.pop() {
            if let Err(error) = protocol::write_all(fd, &entry.bytes) {
                if !self.is_closing() {
                    log::warn!("browser helper {} write failed: {error}", self.pid);
                    self.fail(ERROR_HELPER_STOPPED);
                }
                return;
            }
        }
    }

    fn page_count(&self) -> usize {
        self.live_pages().len()
    }

    /// One monitor iteration: sends pings and detects a hung helper. Returns whether the monitor
    /// must keep running.
    fn monitor_tick(&self, now: Instant) -> bool {
        {
            let lifecycle = self.lifecycle.lock();
            if lifecycle.failed.is_some() || lifecycle.closing {
                return false;
            }
        }
        if !self.ready.has_capability("ping") || self.page_count() == 0 {
            return true;
        }
        let timings = self.config.timings;
        let nonce = {
            let mut ping = self.ping.lock();
            if let Some(since) = ping.unanswered_since
                && now.duration_since(since) >= timings.hang_deadline
            {
                drop(ping);
                log::warn!("browser helper {} stopped answering pings", self.pid);
                self.fail(ERROR_HELPER_HUNG);
                return false;
            }
            let due = ping
                .last_sent
                .is_none_or(|sent| now.duration_since(sent) >= timings.ping_interval);
            if !due {
                return true;
            }
            ping.next_nonce += 1;
            let nonce = ping.next_nonce;
            ping.newest = nonce;
            ping.last_sent = Some(now);
            if ping.unanswered_since.is_none() {
                ping.unanswered_since = Some(now);
            }
            nonce
        };
        let _ = self.send(0, &Command::Ping { nonce });
        true
    }
}

/// Process-wide record of helpers shutting down, so that a restart of the same profile waits
/// for the previous helper to release the profile lock (PLT-014 R12).
pub(crate) static SHUTTING_DOWN: Mutex<Vec<(PathBuf, Arc<(Mutex<bool>, Condvar)>)>> =
    Mutex::new(Vec::new());

/// Blocks until every helper of `root` that is shutting down has exited.
pub(crate) fn wait_for_shutdowns(root: &std::path::Path) {
    let latches: Vec<_> = SHUTTING_DOWN
        .lock()
        .iter()
        .filter(|(path, _)| path == root)
        .map(|(_, latch)| latch.clone())
        .collect();
    for latch in latches {
        let (done, condvar) = &*latch;
        let mut done = done.lock();
        while !*done {
            condvar.wait(&mut done);
        }
    }
}

/// A running helper process serving any number of pages.
pub struct Helper {
    shared: Arc<HelperShared>,
}

impl Helper {
    /// Spawns the helper described by `config` and performs the startup handshake. Blocking:
    /// call it from a startup thread, never from the UI thread.
    // Spawning blocks only the dedicated startup thread this runs on, never the UI thread.
    #[allow(clippy::disallowed_methods)]
    pub fn spawn(config: &HelperConfig) -> Result<Helper, String> {
        if let Some(connector) = &config.connector {
            let started = Instant::now();
            let (socket, child) = (connector.0)(config)?;
            return Self::handshake(socket, child, started, config.clone());
        }
        let helper_path = match &config.helper_path {
            Some(path) => path.clone(),
            None => {
                let app_name = config
                    .app_name
                    .clone()
                    .unwrap_or_else(extract::default_app_name);
                extract::extract_embedded_helper(&app_name)?
            }
        };
        let (ours, theirs) = UnixStream::pair().map_err(|error| {
            format!("Could not start WebKitGTK: {error}. Install the WebKitGTK 4.1 runtime for your distribution.")
        })?;
        let their_fd: RawFd = theirs.as_raw_fd();
        let mut process = Process::new(&helper_path);
        process
            .arg("--channel")
            .arg(their_fd.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(root) = &config.profile_root {
            process.arg("--profile").arg(root);
        }
        process.envs(config.env.iter().map(|(key, value)| (key, value)));
        // SAFETY: only async-signal-safe fcntl runs between fork and exec; it clears
        // close-on-exec on the helper's end of the channel so the child inherits it.
        unsafe {
            process.pre_exec(move || {
                if libc::fcntl(their_fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let started = Instant::now();
        let child = process.spawn().map_err(|error| {
            format!("Could not start WebKitGTK: {error}. Install the WebKitGTK 4.1 runtime for your distribution.")
        })?;
        drop(theirs);
        Self::handshake(ours, Some(child), started, config.clone())
    }

    /// Performs the startup handshake on an already connected channel (a helper supervised by
    /// the caller, or an in-process scripted peer in tests). `started` is the instant the
    /// 10 second deadline is measured from.
    pub fn attach(
        socket: UnixStream,
        child: Option<Child>,
        started: Instant,
        config: HelperConfig,
    ) -> Result<Helper, String> {
        Self::handshake(socket, child, started, config)
    }

    fn handshake(
        socket: UnixStream,
        mut child: Option<Child>,
        started: Instant,
        config: HelperConfig,
    ) -> Result<Helper, String> {
        let fail = |child: &mut Option<Child>, message: String| -> Result<Helper, String> {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(message)
        };
        let first = match read_first_message(&socket, started, config.timings.handshake) {
            Ok(message) => message,
            Err(message) => return fail(&mut child, message),
        };
        let ready = match first.kind {
            kind::ERROR => {
                if first.page != 0 {
                    return fail(&mut child, "Invalid browser startup response".to_owned());
                }
                return fail(
                    &mut child,
                    String::from_utf8_lossy(&first.payload).into_owned(),
                );
            }
            kind::READY if first.page == 0 => match Ready::parse(&first.payload) {
                Ok(ready) => ready,
                Err(message) => return fail(&mut child, message),
            },
            _ => return fail(&mut child, "Invalid browser startup response".to_owned()),
        };
        if socket.set_read_timeout(None).is_err() {
            return fail(&mut child, "Invalid browser startup response".to_owned());
        }
        let transfer = config.select_transfer(&ready);
        let pid = child.as_ref().map_or(0, Child::id);
        let shared = Arc::new(HelperShared {
            pid,
            socket,
            child: Mutex::new(child),
            ready,
            config,
            transfer: Mutex::new(transfer),
            lifecycle: Mutex::new(Lifecycle::default()),
            pages: Mutex::new(HashMap::new()),
            next_page: AtomicU32::new(1),
            outgoing: Outgoing::new(),
            ping: Mutex::new(PingState::default()),
        });
        // The writer thread must be running before the first command is queued; the queue
        // itself is unbounded in time, so order does not matter, but start all threads now.
        let reader = shared.clone();
        std::thread::Builder::new()
            .name("webview-reader".into())
            .spawn(move || reader.reader_loop())
            .map_err(|error| format!("Could not start the browser reader: {error}"))?;
        let writer = shared.clone();
        std::thread::Builder::new()
            .name("webview-writer".into())
            .spawn(move || writer.writer_loop())
            .map_err(|error| format!("Could not start the browser writer: {error}"))?;
        let monitor = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("webview-monitor".into())
            .spawn(move || {
                loop {
                    let Some(shared) = monitor.upgrade() else {
                        return;
                    };
                    if !shared.monitor_tick(Instant::now()) {
                        return;
                    }
                    drop(shared);
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
            .map_err(|error| format!("Could not start the browser monitor: {error}"))?;
        let _ = shared.send(0, &Command::Configure { transfer });
        Ok(Helper { shared })
    }

    /// The helper's process id (0 for a helper without a child process).
    pub fn pid(&self) -> u32 {
        self.shared.pid
    }

    /// The helper's ready message.
    pub fn ready(&self) -> &Ready {
        &self.shared.ready
    }

    /// The transfer mode currently configured.
    pub fn transfer(&self) -> Transfer {
        self.shared.current_transfer()
    }

    /// The failure message when the helper failed or hung.
    pub fn failure(&self) -> Option<String> {
        self.shared.failure()
    }

    /// Whether the helper can no longer be used: failed, hung, or its child exited.
    pub fn is_unusable(&self) -> bool {
        self.shared.failure().is_some() || self.shared.has_exited()
    }

    /// Number of pages that are still alive on this helper.
    pub fn page_count(&self) -> usize {
        self.shared.page_count()
    }

    /// Kills the child process. Used by tests that simulate a crash.
    #[doc(hidden)]
    pub fn kill_for_tests(&self) {
        if let Some(child) = self.shared.child.lock().as_mut() {
            let _ = child.kill();
        }
    }

    /// Creates a page on this helper: allocates the next identifier, registers its route and
    /// sends the `create` command.
    pub fn new_page(self: &Arc<Self>) -> Result<NativePage, String> {
        if let Some(failure) = self.shared.failure() {
            return Err(failure);
        }
        let id = self.shared.next_page.fetch_add(1, Ordering::Relaxed);
        NativePage::create(self.clone(), self.shared.clone(), id)
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let shared = &self.shared;
        {
            let mut lifecycle = shared.lifecycle.lock();
            if lifecycle.closing {
                return;
            }
            lifecycle.closing = true;
        }
        let failed = shared.failure().is_some();
        // Closing the channel (end-of-file) asks the helper to drain; unlike writing a final
        // command into a possibly full channel it cannot block on a hung helper.
        shared.outgoing.close();
        let _ = shared.socket.shutdown(std::net::Shutdown::Both);
        if failed {
            return;
        }
        let shared = shared.clone();
        let latch = Arc::new((Mutex::new(false), Condvar::new()));
        if let Some(root) = &shared.config.profile_root {
            SHUTTING_DOWN.lock().push((root.clone(), latch.clone()));
        }
        let root = shared.config.profile_root.clone();
        let spawned = std::thread::Builder::new()
            .name("webview-reaper".into())
            .spawn({
                move || {
                    reap(&shared);
                    if let Some(root) = &root {
                        SHUTTING_DOWN
                            .lock()
                            .retain(|(path, entry)| !(path == root && Arc::ptr_eq(entry, &latch)));
                    }
                    let (done, condvar) = &*latch;
                    *done.lock() = true;
                    condvar.notify_all();
                }
            });
        if spawned.is_err() {
            log::warn!("could not start the browser helper reaper thread");
        }
    }
}

/// Waits for the closed helper to exit, polling every 10 ms for at most the grace period; then
/// kills and reaps it with a logged warning.
fn reap(shared: &HelperShared) {
    let Some(mut child) = shared.child.lock().take() else {
        return;
    };
    let deadline = Instant::now() + shared.config.timings.shutdown_grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(_) => return,
        }
        if Instant::now() >= deadline {
            log::warn!(
                "browser helper {} did not exit within {:?}; killing it",
                shared.pid,
                shared.config.timings.shutdown_grace
            );
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A read source for the handshake that enforces the total deadline on every read.
struct DeadlineSource<'a> {
    socket: &'a UnixStream,
    deadline: Instant,
    timed_out: bool,
}

impl protocol::RecvBytes for DeadlineSource<'_> {
    fn recv(&mut self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.timed_out = true;
            return Err(io::Error::new(io::ErrorKind::TimedOut, "deadline passed"));
        }
        self.socket.set_read_timeout(Some(remaining))?;
        match protocol::recv_with_fds(self.socket.as_raw_fd(), buf, fds) {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                self.timed_out = true;
                Err(io::Error::new(io::ErrorKind::TimedOut, "deadline passed"))
            }
            other => other,
        }
    }
}

/// Reads the helper's first message with the bounded header and the total deadline.
fn read_first_message(
    socket: &UnixStream,
    started: Instant,
    deadline: Duration,
) -> Result<Message, String> {
    let mut source = DeadlineSource {
        socket,
        deadline: started + deadline,
        timed_out: false,
    };
    match protocol::read_message_limited(
        &mut source,
        Direction::ToApplication,
        Some(MAX_STARTUP_PAYLOAD),
    ) {
        Ok(message) => Ok(message),
        Err(ProtocolError::Io(error)) if source.timed_out => {
            let _ = error;
            Err(format!(
                "Browser startup did not complete: timed out after {} seconds",
                deadline.as_secs_f64().round() as u64
            ))
        }
        Err(ProtocolError::Io(error)) => Err(format!(
            "Browser helper could not start: {error}. Check the WebKitGTK 4.1 runtime and display."
        )),
        Err(ProtocolError::Eof) => Err(format!(
            "Browser helper could not start: {}. Check the WebKitGTK 4.1 runtime and display.",
            io::Error::from(io::ErrorKind::UnexpectedEof)
        )),
        // A response shorter than the header, or one announcing too much: invalid.
        Err(ProtocolError::Truncated | ProtocolError::Oversized { .. }) => {
            Err("Invalid browser startup response".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(page: u32, coalesce: Coalesce, size: usize) -> Entry {
        Entry {
            bytes: vec![0; size],
            page,
            coalesce,
        }
    }

    #[test]
    fn consecutive_moves_and_resizes_of_one_page_collapse() {
        let queue = Outgoing::new();
        for _ in 0..10 {
            assert!(queue.push(entry(1, Coalesce::Move, 10)).is_ok());
        }
        assert_eq!(queue.len(), 1);
        assert!(queue.push(entry(1, Coalesce::Resize, 10)).is_ok());
        assert!(queue.push(entry(1, Coalesce::Resize, 12)).is_ok());
        assert_eq!(queue.len(), 2);
        // A different page, or an intervening command, prevents merging.
        assert!(queue.push(entry(2, Coalesce::Resize, 10)).is_ok());
        assert!(queue.push(entry(2, Coalesce::None, 10)).is_ok());
        assert!(queue.push(entry(2, Coalesce::Resize, 10)).is_ok());
        assert_eq!(queue.len(), 5);
    }

    #[test]
    fn the_queue_overflows_at_1024_messages_or_4_mib() {
        let queue = Outgoing::new();
        for _ in 0..OUTGOING_MAX_MESSAGES {
            assert!(queue.push(entry(1, Coalesce::None, 1)).is_ok());
        }
        assert!(matches!(
            queue.push(entry(1, Coalesce::None, 1)),
            Err(PushError::Overflow)
        ));
        let queue = Outgoing::new();
        assert!(
            queue
                .push(entry(1, Coalesce::None, OUTGOING_MAX_BYTES))
                .is_ok()
        );
        assert!(matches!(
            queue.push(entry(1, Coalesce::None, 1)),
            Err(PushError::Overflow)
        ));
        queue.close();
        assert!(matches!(
            queue.push(entry(1, Coalesce::None, 1)),
            Err(PushError::Closed)
        ));
    }

    #[test]
    fn transfer_selection_follows_the_fallback_chain() {
        let config = HelperConfig::default();
        let ready = |transfers: Vec<Transfer>| Ready {
            protocol: 1,
            engine: String::new(),
            transfers,
            capabilities: vec![],
        };
        assert_eq!(
            config.select_transfer(&ready(vec![Transfer::Inline, Transfer::SharedMemory])),
            Transfer::SharedMemory
        );
        assert_eq!(
            config.select_transfer(&ready(vec![Transfer::Inline])),
            Transfer::Inline
        );
        let config = HelperConfig {
            transfers: vec![Transfer::Inline],
            ..HelperConfig::default()
        };
        assert_eq!(
            config.select_transfer(&ready(vec![Transfer::Inline, Transfer::SharedMemory])),
            Transfer::Inline
        );
    }
}
