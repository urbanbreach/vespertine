//! What a measurement ran on (DEV-003 R8): compositor and backend, scale factor, refresh rate,
//! hardware, build profile and exactly which clocks were measured. Reports from differing
//! baselines are never combined: every report names its baseline.

use crate::harness::{REFRESH_HZ, SCALE_FACTOR};
use std::{env, fs};

/// The environment of a measurement.
#[derive(Clone, Debug)]
pub struct PlatformReport {
    /// The display backend and compositor the process ran under.
    pub backend: String,
    /// Scale factor of the measured window.
    pub scale_factor: f32,
    /// Refresh rate, Hz, and where it comes from.
    pub refresh: String,
    /// CPU model and logical core count.
    pub cpu: String,
    /// GPUs found.
    pub gpu: String,
    /// Build profile.
    pub profile: &'static str,
    /// The quantities measured.
    pub measured: Vec<String>,
    /// What the compared configurations differ in: only the optimization under evaluation.
    pub baseline: String,
}

impl PlatformReport {
    /// Collects the environment of a headless run. `instructions` says whether instruction
    /// counts are measured (with the reason when not).
    pub fn collect(instructions: Result<(), &str>) -> Self {
        let mut measured = vec![
            "UI-thread CPU time (per-thread CPU clock)".to_owned(),
            "wall time".to_owned(),
            "process-wide allocation calls and bytes".to_owned(),
        ];
        measured.push(match instructions {
            Ok(()) => "retired user-space instructions of the UI thread".to_owned(),
            Err(reason) => format!("instruction count: unavailable ({reason})"),
        });
        measured.push("not measured: process CPU, GPU time".to_owned());
        Self {
            backend: backend(),
            scale_factor: SCALE_FACTOR,
            refresh: format!("{REFRESH_HZ} Hz (simulated frame interval of the test clock)"),
            cpu: cpu(),
            gpu: gpu(),
            profile: if cfg!(debug_assertions) {
                "dev (unoptimized, debug assertions on): not comparable with release numbers"
            } else {
                "release (optimized, debug assertions off)"
            },
            measured,
            baseline:
                "same build; only the optimization under evaluation (retained frames) is switched"
                    .to_owned(),
        }
    }

    /// Plain text, one fact per line.
    pub fn text(&self) -> String {
        let mut out = String::new();
        out.push_str("platform: headless runner\n");
        out.push_str(&format!("  backend/compositor: {}\n", self.backend));
        out.push_str(&format!("  scale factor: {}\n", self.scale_factor));
        out.push_str(&format!("  refresh rate: {}\n", self.refresh));
        out.push_str(&format!("  cpu: {}\n", self.cpu));
        out.push_str(&format!("  gpu: {}\n", self.gpu));
        out.push_str(&format!("  build profile: {}\n", self.profile));
        out.push_str("  measured:\n");
        for item in &self.measured {
            out.push_str(&format!("    {item}\n"));
        }
        out.push_str(&format!("  baseline: {}\n", self.baseline));
        out
    }
}

fn backend() -> String {
    let session = env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_owned());
    let wayland = env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|value| !value.is_empty());
    let compositor = if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        "Hyprland".to_owned()
    } else {
        env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown compositor".to_owned())
    };
    let display = match (
        &wayland,
        env::var("DISPLAY").ok().filter(|value| !value.is_empty()),
    ) {
        (Some(socket), _) => format!("Wayland ({socket}), {compositor}"),
        (None, Some(display)) => format!("X11 ({display}), {compositor}"),
        (None, None) => "no display server".to_owned(),
    };
    format!(
        "deterministic test platform (no display server involved); session: {display}, \
         XDG_SESSION_TYPE={session}"
    )
}

fn cpu() -> String {
    let info = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = info
        .lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split(':').nth(1))
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|| "unknown CPU".to_owned());
    let cores = std::thread::available_parallelism().map_or(0, usize::from);
    format!("{model}, {cores} logical cores")
}

fn gpu() -> String {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        let mut names: Vec<_> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("card") && !name.contains('-'))
            .collect();
        names.sort();
        for name in names {
            let device = format!("/sys/class/drm/{name}/device");
            let read = |file: &str| {
                fs::read_to_string(format!("{device}/{file}"))
                    .ok()
                    .map(|text| text.trim().to_owned())
            };
            let vendor = read("vendor").unwrap_or_default();
            let vendor = match vendor.as_str() {
                "0x10de" => "NVIDIA",
                "0x1002" => "AMD",
                "0x8086" => "Intel",
                "0x1af4" | "0x1234" => "virtual",
                "" => "unknown vendor",
                _ => "other vendor",
            };
            let driver = fs::read_link(format!("{device}/driver"))
                .ok()
                .and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "no driver".to_owned());
            found.push(format!(
                "{name}: {vendor} device {} (driver {driver})",
                read("device").unwrap_or_else(|| "?".to_owned())
            ));
        }
    }
    let gpus = if found.is_empty() {
        "no DRM devices found".to_owned()
    } else {
        found.join("; ")
    };
    format!("not used by the headless runner (present: {gpus})")
}
