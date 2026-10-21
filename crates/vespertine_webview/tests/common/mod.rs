//! Shared fixtures: scripted synthetic helper configuration and polling utilities.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use vespertine_webview::{BrowserContext, HelperConfig};

pub const SYNTHETIC_HELPER: &str = env!("CARGO_BIN_EXE_vsp-synthetic-helper");

/// A temporary directory with the control files of one scripted helper.
pub struct Synth {
    pub dir: tempfile::TempDir,
    pub env: Vec<(String, String)>,
}

impl Synth {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let env = vec![("VSP_SYNTH_LOG".to_owned(), log.display().to_string())];
        Self { dir, env }
    }

    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.env.push((key.to_owned(), value.into()));
        self
    }

    /// A gate file: the helper withholds its ready message until [`Synth::release`] creates it.
    pub fn gated(self) -> Self {
        let gate = self.gate_path();
        self.with("VSP_SYNTH_GATE", gate.display().to_string())
    }

    pub fn gate_path(&self) -> PathBuf {
        self.dir.path().join("gate")
    }

    pub fn release(&self) {
        std::fs::write(self.gate_path(), b"go").unwrap();
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.path().join("log.jsonl")
    }

    pub fn config(&self) -> HelperConfig {
        HelperConfig {
            helper_path: Some(PathBuf::from(SYNTHETIC_HELPER)),
            env: self.env.clone(),
            ..HelperConfig::default()
        }
    }

    pub fn context(&self) -> BrowserContext {
        BrowserContext::with_config(self.config())
    }

    /// Every logged line as JSON.
    pub fn log(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.log_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// The commands the helper received, as `(page, cmd json)`.
    pub fn commands(&self) -> Vec<(u32, serde_json::Value)> {
        self.log()
            .into_iter()
            .filter_map(|entry| {
                Some((
                    entry.get("page")?.as_u64()? as u32,
                    entry.get("msg")?.clone(),
                ))
            })
            .collect()
    }

    pub fn started_pids(&self) -> Vec<u32> {
        self.log()
            .into_iter()
            .filter_map(|entry| entry.get("started")?.as_u64().map(|pid| pid as u32))
            .collect()
    }
}

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

/// Whether `pid` still has a process table entry. A zombie counts as existing: it is gone only
/// once its parent has reaped it.
pub fn process_exists(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}
