//! Feedback sounds (PLT-011).
//!
//! [`Sounds`] plays short, caller-categorised sounds through the desktop's
//! command-line audio players. Playback never runs on the calling thread,
//! work is bounded (one playback in flight per category, four overall; excess
//! requests are dropped, not queued) and every outcome is reported through
//! [`SoundOutcome`], so a missing audio system is distinguishable from
//! success without ever panicking or blocking the UI.
//!
//! The module only handles generic values: category keys, bytes and paths.
//! Player execution and time are behind [`PlayerBackend`] and [`Clock`] so
//! behavior can be tested with fake players and an injected clock.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::channel::oneshot;

/// How often a running player is polled.
pub const PLAYER_POLL_INTERVAL: Duration = Duration::from_millis(25);
/// How long a player may run before it is killed and reaped.
pub const PLAYER_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum playbacks in flight for one category.
pub const MAX_IN_FLIGHT_PER_CATEGORY: usize = 1;
/// Maximum playbacks in flight across all categories.
pub const MAX_IN_FLIGHT_TOTAL: usize = 4;

/// A command-line audio player: the program and the arguments that precede
/// the sound file, which is always the final argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerCommand {
    /// Executable name, resolved through `PATH`.
    pub program: &'static str,
    /// Arguments placed before the file path.
    pub args: &'static [&'static str],
}

impl fmt::Display for PlayerCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.program)?;
        for arg in self.args {
            write!(f, " {arg}")?;
        }
        Ok(())
    }
}

/// The players tried by [`Sounds`], in order.
pub const PLAYERS: [PlayerCommand; 5] = [
    PlayerCommand {
        program: "paplay",
        args: &[],
    },
    PlayerCommand {
        program: "pw-play",
        args: &[],
    },
    PlayerCommand {
        program: "aplay",
        args: &["-q"],
    },
    PlayerCommand {
        program: "ffplay",
        args: &["-nodisp", "-autoexit", "-loglevel", "quiet"],
    },
    PlayerCommand {
        program: "mpv",
        args: &["--no-video", "--really-quiet"],
    },
];

/// Where the audio of a category comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundSource {
    /// Encoded audio (WAV is the mandatory format), written to a private
    /// temporary file for each playback.
    Bytes(Arc<[u8]>),
    /// An existing audio file, played in place and never modified or removed.
    Path(PathBuf),
}

impl SoundSource {
    /// A source backed by encoded audio bytes.
    pub fn bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self::Bytes(bytes.into())
    }

    /// A source backed by an audio file.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }
}

/// Why one attempted player did not play the sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerFailure {
    /// The player command line, for example `aplay -q`.
    pub player: String,
    /// A readable reason: not found, spawn error, exit status.
    pub reason: String,
}

impl fmt::Display for PlayerFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.player, self.reason)
    }
}

/// The result of [`Sounds::play`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundOutcome {
    /// A player exited successfully.
    Played,
    /// The master switch or the category is disabled; no process was started
    /// and no file was created.
    Disabled,
    /// The category already had a playback in flight, or four playbacks were
    /// in flight overall. The request was not queued.
    Dropped,
    /// No player could be started or every started player exited
    /// unsuccessfully (no audio output); the reason from each attempted
    /// player is retained.
    Unavailable(Vec<PlayerFailure>),
    /// The temporary file, the process handling, or a player that had to be
    /// killed for exceeding [`PLAYER_TIMEOUT`] failed; carries a readable
    /// reason. Also reported for a category that was never registered.
    Failed(String),
}

/// The exit of a finished player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerExit {
    /// Whether the player exited successfully.
    pub success: bool,
    /// Human-readable status such as `exit status: 1`.
    pub description: String,
}

/// A running player process.
pub trait PlayerChild: Send {
    /// Non-blocking check for exit.
    fn try_wait(&mut self) -> io::Result<Option<PlayerExit>>;
    /// Kills the process.
    fn kill(&mut self) -> io::Result<()>;
    /// Blocks until the (killed) process is reaped.
    fn wait(&mut self) -> io::Result<PlayerExit>;
}

/// Starts player processes. [`SystemPlayers`] runs real commands; tests
/// supply fakes.
pub trait PlayerBackend: Send + Sync {
    /// Starts `command` with `file` as its final argument and all standard
    /// streams closed. A missing program is `io::ErrorKind::NotFound`.
    fn spawn(&self, command: &PlayerCommand, file: &Path) -> io::Result<Box<dyn PlayerChild>>;
}

/// Time source for player polling. [`SystemClock`] uses real time; tests
/// inject a clock whose `sleep` advances virtual time.
pub trait Clock: Send + Sync {
    /// Time elapsed since an arbitrary fixed origin; monotonic.
    fn now(&self) -> Duration;
    /// Waits for `duration`.
    fn sleep(&self, duration: Duration);
}

/// Real monotonic time.
pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Runs the real command-line players.
pub struct SystemPlayers;

struct SystemChild(Child);

impl PlayerChild for SystemChild {
    fn try_wait(&mut self) -> io::Result<Option<PlayerExit>> {
        Ok(self.0.try_wait()?.map(|status| PlayerExit {
            success: status.success(),
            description: status.to_string(),
        }))
    }

    fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }

    fn wait(&mut self) -> io::Result<PlayerExit> {
        let status = self.0.wait()?;
        Ok(PlayerExit {
            success: status.success(),
            description: status.to_string(),
        })
    }
}

impl PlayerBackend for SystemPlayers {
    // Runs on the dedicated playback thread (never the UI thread), where a
    // blocking spawn is the specified behavior.
    #[allow(
        clippy::disallowed_methods,
        reason = "runs on the dedicated playback thread, never the UI thread"
    )]
    fn spawn(&self, command: &PlayerCommand, file: &Path) -> io::Result<Box<dyn PlayerChild>> {
        let child = Command::new(command.program)
            .args(command.args)
            .arg(file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Box::new(SystemChild(child)))
    }
}

/// A future resolving to the [`SoundOutcome`] of one [`Sounds::play`] call.
///
/// Dropping it does not cancel the playback.
pub struct SoundPlayback {
    state: PlaybackState,
}

enum PlaybackState {
    Ready(Option<SoundOutcome>),
    Pending(oneshot::Receiver<SoundOutcome>),
}

impl SoundPlayback {
    fn ready(outcome: SoundOutcome) -> Self {
        Self {
            state: PlaybackState::Ready(Some(outcome)),
        }
    }

    /// Blocks the calling thread until the outcome is known. Never call this
    /// from the UI thread; it exists for tests and worker threads.
    pub fn wait(self) -> SoundOutcome {
        futures::executor::block_on(self)
    }
}

impl Future for SoundPlayback {
    type Output = SoundOutcome;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<SoundOutcome> {
        match &mut self.state {
            PlaybackState::Ready(outcome) => Poll::Ready(
                outcome
                    .take()
                    .expect("SoundPlayback polled after completion"),
            ),
            PlaybackState::Pending(receiver) => match Pin::new(receiver).poll(cx) {
                Poll::Ready(Ok(outcome)) => Poll::Ready(outcome),
                Poll::Ready(Err(_)) => Poll::Ready(SoundOutcome::Failed(
                    "the playback thread ended without reporting a result".to_string(),
                )),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

struct Category {
    source: SoundSource,
    enabled: bool,
}

#[derive(Default)]
struct State {
    master_enabled: bool,
    categories: HashMap<Arc<str>, Category>,
    in_flight: HashSet<Arc<str>>,
}

struct Shared {
    state: Mutex<State>,
    backend: Arc<dyn PlayerBackend>,
    clock: Arc<dyn Clock>,
    temp_dir: PathBuf,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves plain data behind; keep going.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Categorised feedback sounds with per-category and master switches.
///
/// Cloning shares the same categories and in-flight accounting.
#[derive(Clone)]
pub struct Sounds {
    shared: Arc<Shared>,
}

impl Default for Sounds {
    fn default() -> Self {
        Self::new()
    }
}

impl Sounds {
    /// Sounds played through the real command-line players, with temporary
    /// files in the user's runtime directory (or the system temporary
    /// directory when none is set). The master switch starts enabled.
    pub fn new() -> Self {
        Self::with_runtime(
            Arc::new(SystemPlayers),
            Arc::new(SystemClock::new()),
            default_temp_dir(),
        )
    }

    /// Sounds with injected player execution, clock and temporary directory.
    pub fn with_runtime(
        backend: Arc<dyn PlayerBackend>,
        clock: Arc<dyn Clock>,
        temp_dir: PathBuf,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    master_enabled: true,
                    ..State::default()
                }),
                backend,
                clock,
                temp_dir,
            }),
        }
    }

    /// Registers (or replaces) a category with its source and enabled flag.
    pub fn register_category(
        &self,
        category: impl Into<Arc<str>>,
        source: SoundSource,
        enabled: bool,
    ) {
        self.shared
            .state()
            .categories
            .insert(category.into(), Category { source, enabled });
    }

    /// Enables or disables one category. Returns `false` when the category is
    /// not registered.
    pub fn set_category_enabled(&self, category: &str, enabled: bool) -> bool {
        match self.shared.state().categories.get_mut(category) {
            Some(entry) => {
                entry.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// Whether a registered category is enabled.
    pub fn category_enabled(&self, category: &str) -> Option<bool> {
        self.shared
            .state()
            .categories
            .get(category)
            .map(|entry| entry.enabled)
    }

    /// Sets the master switch; while off every play resolves to
    /// [`SoundOutcome::Disabled`].
    pub fn set_master_enabled(&self, enabled: bool) {
        self.shared.state().master_enabled = enabled;
    }

    /// Whether the master switch is on.
    pub fn master_enabled(&self) -> bool {
        self.shared.state().master_enabled
    }

    /// Plays the sound of `category` on a background thread.
    ///
    /// Returns immediately. A disabled category or master switch resolves to
    /// [`SoundOutcome::Disabled`] without touching the audio system; a request
    /// beyond the in-flight limits resolves to [`SoundOutcome::Dropped`].
    pub fn play(&self, category: &str) -> SoundPlayback {
        let (key, source) = {
            let mut state = self.shared.state();
            if !state.master_enabled {
                return SoundPlayback::ready(SoundOutcome::Disabled);
            }
            let Some((key, entry)) = state.categories.get_key_value(category) else {
                return SoundPlayback::ready(SoundOutcome::Failed(format!(
                    "sound category {category:?} is not registered"
                )));
            };
            if !entry.enabled {
                return SoundPlayback::ready(SoundOutcome::Disabled);
            }
            let (key, source) = (key.clone(), entry.source.clone());
            // `in_flight` holds at most one entry per category
            // ([`MAX_IN_FLIGHT_PER_CATEGORY`]).
            if state.in_flight.len() >= MAX_IN_FLIGHT_TOTAL || state.in_flight.contains(&key) {
                return SoundPlayback::ready(SoundOutcome::Dropped);
            }
            state.in_flight.insert(key.clone());
            (key, source)
        };

        let (sender, receiver) = oneshot::channel();
        let permit = Permit {
            shared: self.shared.clone(),
            category: key,
        };
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("feedback-sound".to_string())
            .spawn(move || {
                let outcome = run_playback(&shared, &source);
                // Free the slot before reporting, so a caller that reacts to
                // the outcome can immediately play again.
                drop(permit);
                sender.send(outcome).ok();
            });
        match spawned {
            Ok(_) => SoundPlayback {
                state: PlaybackState::Pending(receiver),
            },
            // The closure, and with it the permit, was dropped on failure.
            Err(error) => SoundPlayback::ready(SoundOutcome::Failed(format!(
                "could not start the playback thread: {error}"
            ))),
        }
    }
}

/// Releases a category's in-flight slot when the playback ends.
struct Permit {
    shared: Arc<Shared>,
    category: Arc<str>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.shared.state().in_flight.remove(&self.category);
    }
}

fn run_playback(shared: &Shared, source: &SoundSource) -> SoundOutcome {
    // Declared first so the temporary file outlives the players and is
    // removed on every exit path.
    let temp_file;
    let file: &Path = match source {
        SoundSource::Bytes(bytes) => match TempSoundFile::create(&shared.temp_dir, bytes) {
            Ok(created) => {
                temp_file = created;
                temp_file.path()
            }
            Err(error) => {
                return SoundOutcome::Failed(format!(
                    "could not create the temporary sound file: {error}"
                ));
            }
        },
        SoundSource::Path(path) => {
            if let Err(error) = std::fs::metadata(path) {
                return SoundOutcome::Failed(format!(
                    "sound file {} is not readable: {error}",
                    path.display()
                ));
            }
            path
        }
    };

    let mut failures = Vec::new();
    for command in &PLAYERS {
        match shared.backend.spawn(command, file) {
            Err(error) => failures.push(PlayerFailure {
                player: command.to_string(),
                reason: if error.kind() == io::ErrorKind::NotFound {
                    "not found".to_string()
                } else {
                    format!("could not be started: {error}")
                },
            }),
            Ok(mut child) => match wait_for_player(shared.clock.as_ref(), child.as_mut()) {
                WaitResult::Exited(exit) if exit.success => return SoundOutcome::Played,
                WaitResult::Exited(exit) => failures.push(PlayerFailure {
                    player: command.to_string(),
                    reason: format!("exited unsuccessfully ({})", exit.description),
                }),
                WaitResult::TimedOut => {
                    return SoundOutcome::Failed(format!(
                        "{command} did not finish within {} s and was killed",
                        PLAYER_TIMEOUT.as_secs()
                    ));
                }
                WaitResult::Error(error) => {
                    return SoundOutcome::Failed(format!("{command}: {error}"));
                }
            },
        }
    }
    SoundOutcome::Unavailable(failures)
}

enum WaitResult {
    Exited(PlayerExit),
    TimedOut,
    Error(String),
}

/// Polls `child` every [`PLAYER_POLL_INTERVAL`]; kills and reaps it when it
/// has not finished within [`PLAYER_TIMEOUT`].
fn wait_for_player(clock: &dyn Clock, child: &mut dyn PlayerChild) -> WaitResult {
    let start = clock.now();
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => return WaitResult::Exited(exit),
            Ok(None) => {}
            Err(error) => {
                kill_and_reap(child);
                return WaitResult::Error(format!("could not poll the player: {error}"));
            }
        }
        if clock.now().saturating_sub(start) >= PLAYER_TIMEOUT {
            kill_and_reap(child);
            return WaitResult::TimedOut;
        }
        clock.sleep(PLAYER_POLL_INTERVAL);
    }
}

fn kill_and_reap(child: &mut dyn PlayerChild) {
    if let Err(error) = child.kill() {
        log::warn!("failed to kill sound player: {error}");
    }
    if let Err(error) = child.wait() {
        log::warn!("failed to reap sound player: {error}");
    }
}

fn default_temp_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if Path::new(&dir).is_dir() => PathBuf::from(dir),
        _ => std::env::temp_dir(),
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A private temporary sound file, removed when dropped.
struct TempSoundFile {
    path: PathBuf,
}

impl TempSoundFile {
    /// Creates `<dir>/vespertine-sound-<pid>-<counter>.wav` exclusively with
    /// mode 0600 and writes `bytes`. A name that already exists is skipped,
    /// never reused or followed.
    fn create(dir: &Path, bytes: &[u8]) -> io::Result<Self> {
        Self::create_with_counter(dir, bytes, || TEMP_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    fn create_with_counter(
        dir: &Path,
        bytes: &[u8],
        mut next_counter: impl FnMut() -> u64,
    ) -> io::Result<Self> {
        let pid = std::process::id();
        let mut last_error = None;
        for _ in 0..16 {
            let counter = next_counter();
            let path = dir.join(format!("vespertine-sound-{pid}-{counter}.wav"));
            match create_exclusive(&path) {
                Ok(mut file) => {
                    let temp = Self { path };
                    // On a write error `temp` is dropped and removes the file.
                    file.write_all(bytes)?;
                    file.flush()?;
                    return Ok(temp);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| io::Error::other("no unused temporary file name was available")))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempSoundFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            log::warn!(
                "failed to remove temporary sound file {}: {error}",
                self.path.display()
            );
        }
    }
}

/// Creates `path` with `O_CREAT | O_EXCL | O_NOFOLLOW` and mode 0600. Fails
/// with `AlreadyExists` when anything, including a symlink, is at `path`.
fn create_exclusive(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Virtual time that `sleep` advances instantly.
    #[derive(Default)]
    struct VirtualClock {
        now: Mutex<Duration>,
        sleeps: AtomicU64,
    }

    impl Clock for VirtualClock {
        fn now(&self) -> Duration {
            *self.now.lock().unwrap()
        }

        fn sleep(&self, duration: Duration) {
            *self.now.lock().unwrap() += duration;
            self.sleeps.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    enum Behavior {
        Missing,
        SpawnError,
        Exit {
            success: bool,
        },
        /// Runs until `release` is set, then exits successfully.
        UntilReleased(Arc<AtomicBool>),
        Never,
    }

    #[derive(Default)]
    struct Observed {
        spawns: Vec<(String, PathBuf)>,
        /// (mode, contents) of the file as seen when a player started.
        files: Vec<(u32, Vec<u8>)>,
        killed: u32,
        reaped: u32,
    }

    struct FakePlayers {
        behaviors: Mutex<HashMap<&'static str, Behavior>>,
        observed: Arc<Mutex<Observed>>,
    }

    impl FakePlayers {
        fn new(behaviors: &[(&'static str, Behavior)]) -> Arc<Self> {
            Arc::new(Self {
                behaviors: Mutex::new(behaviors.iter().cloned().collect()),
                observed: Arc::default(),
            })
        }
    }

    struct FakeChild {
        behavior: Behavior,
        observed: Arc<Mutex<Observed>>,
    }

    impl PlayerChild for FakeChild {
        fn try_wait(&mut self) -> io::Result<Option<PlayerExit>> {
            let success = match &self.behavior {
                Behavior::Exit { success } => *success,
                Behavior::UntilReleased(release) if release.load(Ordering::SeqCst) => true,
                _ => return Ok(None),
            };
            Ok(Some(PlayerExit {
                success,
                description: if success {
                    "exit status: 0".to_string()
                } else {
                    "exit status: 1".to_string()
                },
            }))
        }

        fn kill(&mut self) -> io::Result<()> {
            self.observed.lock().unwrap().killed += 1;
            Ok(())
        }

        fn wait(&mut self) -> io::Result<PlayerExit> {
            self.observed.lock().unwrap().reaped += 1;
            Ok(PlayerExit {
                success: false,
                description: "signal: 9 (SIGKILL)".to_string(),
            })
        }
    }

    impl PlayerBackend for FakePlayers {
        fn spawn(&self, command: &PlayerCommand, file: &Path) -> io::Result<Box<dyn PlayerChild>> {
            let behavior = self
                .behaviors
                .lock()
                .unwrap()
                .get(command.program)
                .cloned()
                .unwrap_or(Behavior::Missing);
            let mut observed = self.observed.lock().unwrap();
            observed
                .spawns
                .push((command.to_string(), file.to_path_buf()));
            match behavior {
                Behavior::Missing => Err(io::ErrorKind::NotFound.into()),
                Behavior::SpawnError => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "permission denied",
                )),
                behavior => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        if let Ok(metadata) = std::fs::metadata(file) {
                            let contents = std::fs::read(file).unwrap();
                            observed
                                .files
                                .push((metadata.permissions().mode() & 0o777, contents));
                        }
                    }
                    Ok(Box::new(FakeChild {
                        behavior,
                        observed: self.observed.clone(),
                    }))
                }
            }
        }
    }

    fn sounds_with(players: &Arc<FakePlayers>, clock: Arc<dyn Clock>, dir: &Path) -> Sounds {
        let sounds = Sounds::with_runtime(players.clone(), clock, dir.to_path_buf());
        for category in ["a", "b", "c", "d", "e"] {
            sounds.register_category(category, SoundSource::bytes(b"RIFFfake".to_vec()), true);
        }
        sounds
    }

    fn dir_is_empty(dir: &Path) -> bool {
        std::fs::read_dir(dir).unwrap().next().is_none()
    }

    #[test]
    fn player_order_and_arguments_match_the_specification() {
        let rendered: Vec<String> = PLAYERS.iter().map(ToString::to_string).collect();
        assert_eq!(
            rendered,
            [
                "paplay",
                "pw-play",
                "aplay -q",
                "ffplay -nodisp -autoexit -loglevel quiet",
                "mpv --no-video --really-quiet",
            ]
        );
    }

    #[test]
    fn missing_first_player_falls_through_to_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[
            ("paplay", Behavior::Missing),
            ("pw-play", Behavior::Exit { success: false }),
            ("aplay", Behavior::Exit { success: true }),
        ]);
        let sounds = sounds_with(&players, Arc::new(VirtualClock::default()), dir.path());

        assert_eq!(sounds.play("a").wait(), SoundOutcome::Played);

        let observed = players.observed.lock().unwrap();
        let tried: Vec<&str> = observed.spawns.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(tried, ["paplay", "pw-play", "aplay -q"]);
        // The sound file is the same, private, owner-only file for each try.
        assert!(
            observed
                .spawns
                .iter()
                .all(|(_, file)| file == &observed.spawns[0].1)
        );
        assert!(dir_is_empty(dir.path()));
    }

    #[test]
    fn all_players_missing_is_unavailable_with_per_player_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[
            ("paplay", Behavior::Missing),
            ("pw-play", Behavior::SpawnError),
            ("aplay", Behavior::Exit { success: false }),
        ]);
        let sounds = sounds_with(&players, Arc::new(VirtualClock::default()), dir.path());

        let SoundOutcome::Unavailable(failures) = sounds.play("a").wait() else {
            panic!("expected unavailable");
        };
        let summary: Vec<(String, String)> =
            failures.into_iter().map(|f| (f.player, f.reason)).collect();
        assert_eq!(
            summary,
            [
                ("paplay".to_string(), "not found".to_string()),
                (
                    "pw-play".to_string(),
                    "could not be started: permission denied".to_string()
                ),
                (
                    "aplay -q".to_string(),
                    "exited unsuccessfully (exit status: 1)".to_string()
                ),
                (
                    "ffplay -nodisp -autoexit -loglevel quiet".to_string(),
                    "not found".to_string()
                ),
                (
                    "mpv --no-video --really-quiet".to_string(),
                    "not found".to_string()
                ),
            ]
        );
        assert!(dir_is_empty(dir.path()));
    }

    #[test]
    fn player_that_never_exits_is_killed_and_reaped_at_ten_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[("paplay", Behavior::Never)]);
        let clock = Arc::new(VirtualClock::default());
        let sounds = sounds_with(&players, clock.clone(), dir.path());

        let outcome = sounds.play("a").wait();

        let SoundOutcome::Failed(reason) = outcome else {
            panic!("expected failed, got {outcome:?}");
        };
        assert!(
            reason.contains("paplay") && reason.contains("10 s"),
            "{reason}"
        );
        let observed = players.observed.lock().unwrap();
        assert_eq!((observed.killed, observed.reaped), (1, 1));
        // Polled every 25 ms for exactly ten seconds, then killed.
        assert_eq!(clock.sleeps.load(Ordering::SeqCst), 400);
        assert_eq!(clock.now(), PLAYER_TIMEOUT);
        drop(observed);
        assert!(dir_is_empty(dir.path()));
    }

    #[test]
    fn player_finishing_before_the_timeout_is_not_killed() {
        let dir = tempfile::tempdir().unwrap();
        let release = Arc::new(AtomicBool::new(false));
        let players = FakePlayers::new(&[("paplay", Behavior::UntilReleased(release.clone()))]);
        let clock = Arc::new(VirtualClock::default());
        // Release the player after 399 polls, one poll before the deadline.
        struct ReleasingClock {
            inner: Arc<VirtualClock>,
            release: Arc<AtomicBool>,
        }
        impl Clock for ReleasingClock {
            fn now(&self) -> Duration {
                self.inner.now()
            }
            fn sleep(&self, duration: Duration) {
                self.inner.sleep(duration);
                if self.inner.now() >= PLAYER_TIMEOUT - PLAYER_POLL_INTERVAL {
                    self.release.store(true, Ordering::SeqCst);
                }
            }
        }
        let sounds = sounds_with(
            &players,
            Arc::new(ReleasingClock {
                inner: clock,
                release,
            }),
            dir.path(),
        );

        assert_eq!(sounds.play("a").wait(), SoundOutcome::Played);
        assert_eq!(players.observed.lock().unwrap().killed, 0);
    }

    #[test]
    fn second_request_in_the_same_category_is_dropped_while_one_is_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let release = Arc::new(AtomicBool::new(false));
        let players = FakePlayers::new(&[("paplay", Behavior::UntilReleased(release.clone()))]);
        let sounds = sounds_with(&players, Arc::new(SystemClock::new()), dir.path());

        let first = sounds.play("a");
        assert_eq!(sounds.play("a").wait(), SoundOutcome::Dropped);
        // Another category is not affected by the per-category limit.
        let other = sounds.play("b");

        release.store(true, Ordering::SeqCst);
        assert_eq!(first.wait(), SoundOutcome::Played);
        assert_eq!(other.wait(), SoundOutcome::Played);

        // The slot is free again once playback ended.
        assert_eq!(sounds.play("a").wait(), SoundOutcome::Played);
        assert!(dir_is_empty(dir.path()));
    }

    #[test]
    fn fifth_overall_request_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let release = Arc::new(AtomicBool::new(false));
        let players = FakePlayers::new(&[("paplay", Behavior::UntilReleased(release.clone()))]);
        let sounds = sounds_with(&players, Arc::new(SystemClock::new()), dir.path());

        let running: Vec<SoundPlayback> = ["a", "b", "c", "d"]
            .iter()
            .map(|c| sounds.play(c))
            .collect();
        assert_eq!(sounds.play("e").wait(), SoundOutcome::Dropped);

        release.store(true, Ordering::SeqCst);
        for playback in running {
            assert_eq!(playback.wait(), SoundOutcome::Played);
        }
        assert_eq!(sounds.play("e").wait(), SoundOutcome::Played);
    }

    #[test]
    fn disabled_category_and_master_switch_never_start_a_process() {
        let dir = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[("paplay", Behavior::Exit { success: true })]);
        let sounds = sounds_with(&players, Arc::new(VirtualClock::default()), dir.path());

        assert!(sounds.set_category_enabled("a", false));
        assert!(!sounds.set_category_enabled("missing", false));
        assert_eq!(sounds.category_enabled("a"), Some(false));
        assert_eq!(sounds.play("a").wait(), SoundOutcome::Disabled);

        sounds.set_master_enabled(false);
        assert!(!sounds.master_enabled());
        assert_eq!(sounds.play("b").wait(), SoundOutcome::Disabled);

        assert!(players.observed.lock().unwrap().spawns.is_empty());
        assert!(dir_is_empty(dir.path()));

        sounds.set_master_enabled(true);
        assert_eq!(sounds.play("b").wait(), SoundOutcome::Played);
    }

    #[test]
    fn unregistered_category_fails_without_touching_the_audio_system() {
        let dir = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[]);
        let sounds = sounds_with(&players, Arc::new(VirtualClock::default()), dir.path());

        assert!(matches!(
            sounds.play("nope").wait(),
            SoundOutcome::Failed(reason) if reason.contains("nope")
        ));
        assert!(players.observed.lock().unwrap().spawns.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn temp_file_is_private_exclusive_and_removed_after_success_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[("paplay", Behavior::Exit { success: true })]);
        let sounds = sounds_with(&players, Arc::new(VirtualClock::default()), dir.path());
        assert_eq!(sounds.play("a").wait(), SoundOutcome::Played);
        {
            let observed = players.observed.lock().unwrap();
            let (mode, contents) = &observed.files[0];
            assert_eq!(*mode, 0o600);
            assert_eq!(contents, b"RIFFfake");
            let name = observed.spawns[0].1.file_name().unwrap().to_str().unwrap();
            assert!(
                name.starts_with(&format!("vespertine-sound-{}-", std::process::id())),
                "{name}"
            );
        }
        assert!(dir_is_empty(dir.path()));

        // Removed after a failed playback as well (timeout and all players
        // failing).
        let failing = FakePlayers::new(&[("paplay", Behavior::Exit { success: false })]);
        let sounds = sounds_with(&failing, Arc::new(VirtualClock::default()), dir.path());
        assert!(matches!(
            sounds.play("a").wait(),
            SoundOutcome::Unavailable(_)
        ));
        assert!(dir_is_empty(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_creation_never_reuses_or_follows_existing_names() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"precious").unwrap();

        let existing = dir.path().join("existing");
        std::fs::write(&existing, b"keep").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("created-through-link"), &dangling).unwrap();

        for path in [&existing, &link, &dangling] {
            let error = create_exclusive(path).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{path:?}");
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"precious");
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep");
        assert!(!dir.path().join("created-through-link").exists());

        let fresh = dir.path().join("fresh");
        create_exclusive(&fresh).unwrap();
        assert!(fresh.exists());
    }

    #[cfg(unix)]
    #[test]
    fn temp_file_creation_skips_names_taken_by_files_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        // Occupy a window of upcoming names with a file and symlinks.
        let next = 1000;
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"precious").unwrap();
        let mut occupied = Vec::new();
        for offset in 0..3u64 {
            let path = dir
                .path()
                .join(format!("vespertine-sound-{pid}-{}.wav", next + offset));
            if offset == 0 {
                std::fs::write(&path, b"squatter").unwrap();
            } else {
                std::os::unix::fs::symlink(&victim, &path).unwrap();
            }
            occupied.push(path);
        }

        let mut counter = 1000..;
        let created =
            TempSoundFile::create_with_counter(dir.path(), b"audio", || counter.next().unwrap())
                .unwrap();
        assert_eq!(
            created.path(),
            dir.path().join(format!("vespertine-sound-{pid}-1003.wav"))
        );
        assert!(!occupied.contains(&created.path().to_path_buf()));
        assert_eq!(std::fs::read(created.path()).unwrap(), b"audio");
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        assert_eq!(std::fs::read(&occupied[0]).unwrap(), b"squatter");
        drop(created);
        assert_eq!(std::fs::read(&occupied[0]).unwrap(), b"squatter");
    }

    #[test]
    fn temp_file_creation_failure_is_reported_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[("paplay", Behavior::Exit { success: true })]);
        let sounds = Sounds::with_runtime(
            players.clone(),
            Arc::new(VirtualClock::default()),
            dir.path().join("does-not-exist"),
        );
        sounds.register_category("a", SoundSource::bytes(b"x".to_vec()), true);

        assert!(matches!(
            sounds.play("a").wait(),
            SoundOutcome::Failed(reason) if reason.contains("temporary sound file")
        ));
        assert!(players.observed.lock().unwrap().spawns.is_empty());
    }

    #[test]
    fn path_source_is_played_in_place_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let sound = dir.path().join("ding.wav");
        std::fs::write(&sound, b"RIFF").unwrap();
        let work = tempfile::tempdir().unwrap();
        let players = FakePlayers::new(&[("paplay", Behavior::Exit { success: true })]);
        let sounds = Sounds::with_runtime(
            players.clone(),
            Arc::new(VirtualClock::default()),
            work.path().to_path_buf(),
        );
        sounds.register_category("ding", SoundSource::path(&sound), true);
        sounds.register_category(
            "absent",
            SoundSource::path(dir.path().join("absent.wav")),
            true,
        );

        assert_eq!(sounds.play("ding").wait(), SoundOutcome::Played);
        assert_eq!(players.observed.lock().unwrap().spawns[0].1, sound);
        assert!(sound.exists());
        assert!(matches!(
            sounds.play("absent").wait(),
            SoundOutcome::Failed(_)
        ));
        assert!(dir_is_empty(work.path()));
    }

    #[test]
    fn play_does_not_block_while_the_player_runs() {
        let dir = tempfile::tempdir().unwrap();
        let release = Arc::new(AtomicBool::new(false));
        let players = FakePlayers::new(&[("paplay", Behavior::UntilReleased(release.clone()))]);
        let sounds = sounds_with(&players, Arc::new(SystemClock::new()), dir.path());

        let started = std::time::Instant::now();
        let playback = sounds.play("a");
        // The player has not been released, so only a non-blocking play()
        // returns here.
        assert!(started.elapsed() < Duration::from_secs(1));
        release.store(true, Ordering::SeqCst);
        assert_eq!(playback.wait(), SoundOutcome::Played);
    }
}
