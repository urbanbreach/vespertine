//! Sampling CPU profiler and hardware counters for the runner (`--features profile`).
//!
//! `VSP_COUNT=<file>` writes the user-space instruction and cycle counters of all threads, polled
//! every 20 ms, when the runner exits, headed by the time the result file was written (the end of
//! the measured window). The window (its length is in the driver's results) can be cut out of the
//! series, without startup or teardown.
//!
//! Sampling is enabled by `VSP_PROFILE=<report file>`. Sampling (process CPU time, all threads) starts
//! `VSP_PROFILE_DELAY_MS` (default 3500) after process start so that startup and warmup are not
//! charged, at `VSP_PROFILE_HZ` (default 500) samples per CPU second, and the report (inclusive and
//! self cost by function, self cost by source line) is written when the runner exits.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct Profiler {
    sampler: Option<(Sender<()>, JoinHandle<()>)>,
    counters: Option<Counters>,
    instructions: Option<InstructionProfile>,
}

/// User-space instruction and cycle counters of this process and its threads, polled in the
/// background so that a window of the run can be cut out afterwards.
struct Counters {
    path: String,
    stop: Sender<()>,
    poller: JoinHandle<Vec<[u64; 3]>>,
}

/// The performance monitoring units that execute user code. A hybrid CPU has one per core type
/// and a hardware event only counts while the thread runs on cores of the unit it was opened
/// for, so every event is opened once per unit. Zero stands for the single legacy unit.
fn pmu_types() -> Vec<u64> {
    let types: Vec<u64> = ["cpu_core", "cpu_atom"]
        .iter()
        .filter_map(|name| std::fs::read_to_string(format!("/sys/devices/{name}/type")).ok())
        .filter_map(|text| text.trim().parse().ok())
        .collect();
    if types.is_empty() { vec![0] } else { types }
}

/// A hardware event for one unit: the unit's type travels in the upper half of `config`.
fn hardware_config(event: u64, pmu: u64) -> u64 {
    event | (pmu << 32)
}

fn open_counters(event: u64) -> Vec<libc::c_int> {
    pmu_types()
        .into_iter()
        .map(|pmu| {
            // `perf_event_attr`: type, size, config, sample_period, sample_type, read_format, flags.
            let mut attr = [0u64; 16];
            attr[0] = 128 << 32; // type PERF_TYPE_HARDWARE (0), size
            attr[1] = hardware_config(event, pmu);
            // inherit (bit 1), pinned (bit 2: never multiplexed away), exclude_kernel (bit 5),
            // exclude_hv (bit 6)
            attr[5] = (1 << 1) | (1 << 2) | (1 << 5) | (1 << 6);
            // SAFETY: `attr` is a valid, fully initialized perf_event_attr of the declared size.
            let fd =
                unsafe { libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), 0, -1, -1, 0) };
            assert!(fd >= 0, "opening a performance counter");
            fd as libc::c_int
        })
        .collect()
}

fn read_counters(fds: &[libc::c_int]) -> u64 {
    fds.iter()
        .map(|&fd| {
            let mut value = 0u64;
            // SAFETY: reads at most 8 bytes into `value`.
            let read = unsafe { libc::read(fd, (&mut value as *mut u64).cast(), 8) };
            assert_eq!(read, 8, "reading performance counter");
            value
        })
        .sum()
}

/// `[wall-clock ns, instructions, cycles]`.
fn sample(instructions: &[libc::c_int], cycles: &[libc::c_int]) -> [u64; 3] {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
    [
        now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64,
        read_counters(instructions),
        read_counters(cycles),
    ]
}

impl Counters {
    fn start() -> Option<Self> {
        let path = std::env::var("VSP_COUNT").ok()?;
        let instructions = open_counters(1);
        let cycles = open_counters(0);
        let (stop, stopped) = mpsc::channel::<()>();
        let poller = std::thread::spawn(move || {
            let mut samples = vec![sample(&instructions, &cycles)];
            while stopped.recv_timeout(Duration::from_millis(20)).is_err() {
                samples.push(sample(&instructions, &cycles));
            }
            samples.push(sample(&instructions, &cycles));
            samples
        });
        Some(Self { path, stop, poller })
    }

    /// Writes the wall-clock time (ns) at which the runner wrote its result file (the end of the
    /// measured window; 0 when there is none), then one `<wall-clock ns> <instructions> <cycles>`
    /// line per poll.
    fn finish(self) {
        let _ = self.stop.send(());
        let samples = self.poller.join().expect("counter poller");
        let result_time = std::env::args()
            .skip_while(|arg| arg != "--out")
            .nth(1)
            .and_then(|out| std::fs::metadata(out).ok())
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos() as u64);
        let mut text = format!("{result_time}\n");
        for [time, instructions, cycles] in samples {
            let _ = writeln!(text, "{time} {instructions} {cycles}");
        }
        std::fs::write(&self.path, text).expect("writing counter file");
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Sampling of the instruction counter of the main thread (`VSP_IPROF=<report file>`): the
/// instruction pointer every `VSP_IPROF_PERIOD` (default 100 000) user instructions, read from the
/// kernel's ring buffer, resolved to inlined functions and lines when the runner exits. Unlike
/// timer sampling the distribution does not depend on machine load.
struct InstructionProfile {
    path: String,
    stop: Sender<()>,
    thread: JoinHandle<Vec<(u64, u64)>>,
}

const RING_PAGES: usize = 128;

impl InstructionProfile {
    fn start() -> Option<Self> {
        let path = std::env::var("VSP_IPROF").ok()?;
        let period = env_u64("VSP_IPROF_PERIOD", 100_000);
        // SAFETY: querying the page size has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let bases: Vec<usize> = pmu_types()
            .into_iter()
            .map(|pmu| {
                // `perf_event_attr`: type, size, config, sample_period, sample_type, read_format,
                // flags.
                let mut attr = [0u64; 16];
                attr[0] = 128 << 32; // PERF_TYPE_HARDWARE, size
                attr[1] = hardware_config(1, pmu); // PERF_COUNT_HW_INSTRUCTIONS
                attr[2] = period;
                attr[3] = 1 | (1 << 2); // PERF_SAMPLE_IP | PERF_SAMPLE_TIME
                // pinned (bit 2: never multiplexed away), exclude_kernel (bit 5), exclude_hv
                // (bit 6), precise_ip = 2 (bits 15-16), use_clockid (bit 25); `clockid` is the
                // upper half of word 11.
                attr[5] = (1 << 2) | (1 << 5) | (1 << 6) | (2 << 15) | (1 << 25);
                attr[11] = (libc::CLOCK_MONOTONIC as u64) << 32;
                // SAFETY: `attr` is a valid perf_event_attr of the declared size.
                let mut fd = unsafe {
                    libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), 0, -1, -1, 0)
                };
                if fd < 0 {
                    attr[5] = (1 << 2) | (1 << 5) | (1 << 6) | (1 << 25);
                    // SAFETY: as above.
                    fd = unsafe {
                        libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), 0, -1, -1, 0)
                    };
                }
                assert!(fd >= 0, "opening the instruction sampling counter");
                // SAFETY: maps the counter's ring buffer (one control page and `RING_PAGES` data
                // pages).
                let map = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        (1 + RING_PAGES) * page,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        fd as libc::c_int,
                        0,
                    )
                };
                assert!(map != libc::MAP_FAILED, "mapping the sampling ring buffer");
                map as usize
            })
            .collect();
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let mut ips = Vec::new();
            let data_size = RING_PAGES * page;
            let drain = |ips: &mut Vec<(u64, u64)>| {
                for &base in &bases {
                    let head_ptr = (base + 1024) as *const std::sync::atomic::AtomicU64;
                    let tail_ptr = (base + 1032) as *const std::sync::atomic::AtomicU64;
                    // SAFETY: both addresses lie inside the control page of the mapping.
                    let (head, mut tail) = unsafe {
                        (
                            (*head_ptr).load(std::sync::atomic::Ordering::Acquire),
                            (*tail_ptr).load(std::sync::atomic::Ordering::Relaxed),
                        )
                    };
                    let read = |offset: u64, out: &mut [u8]| {
                        for (ix, byte) in out.iter_mut().enumerate() {
                            let at = (offset as usize + ix) % data_size;
                            // SAFETY: `at` is inside the data area of the mapping.
                            *byte = unsafe { *((base + page + at) as *const u8) };
                        }
                    };
                    while tail < head {
                        let mut header = [0u8; 8];
                        read(tail, &mut header);
                        let kind = u32::from_le_bytes(header[0..4].try_into().unwrap());
                        let size = u16::from_le_bytes(header[6..8].try_into().unwrap()) as u64;
                        if kind == 9 {
                            let mut body = [0u8; 16];
                            read(tail + 8, &mut body);
                            ips.push((
                                u64::from_le_bytes(body[8..16].try_into().unwrap()),
                                u64::from_le_bytes(body[0..8].try_into().unwrap()),
                            ));
                        }
                        tail += size.max(8);
                    }
                    // SAFETY: as above; publishes the consumed position to the kernel.
                    unsafe { (*tail_ptr).store(tail, std::sync::atomic::Ordering::Release) };
                }
            };
            while stopped.recv_timeout(Duration::from_millis(25)).is_err() {
                drain(&mut ips);
            }
            drain(&mut ips);
            ips
        });
        Some(Self { path, stop, thread })
    }

    fn finish(self) {
        let _ = self.stop.send(());
        let samples = self.thread.join().expect("instruction sampler");
        // Keep the samples of the measured window of a perf run: it ends when the result file was
        // written and lasts as long as the result says.
        let clock = |id| {
            let mut now = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: `now` is a valid out-pointer.
            unsafe { libc::clock_gettime(id, &mut now) };
            now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64
        };
        let (real_now, mono_now) = (clock(libc::CLOCK_REALTIME), clock(libc::CLOCK_MONOTONIC));
        let window = std::env::args()
            .skip_while(|arg| arg != "--out")
            .nth(1)
            .and_then(|out| {
                let text = std::fs::read_to_string(&out).ok()?;
                let wall = text.split("\"wall_ns\":").nth(1)?;
                let wall: u64 = wall.trim().trim_end_matches('}').trim().parse().ok()?;
                let written = std::fs::metadata(&out).ok()?.modified().ok()?;
                let written = written
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_nanos() as u64;
                let end = mono_now.checked_sub(real_now.checked_sub(written)?)?;
                Some((end.checked_sub(wall)?, end))
            });
        let ips: Vec<u64> = samples
            .iter()
            .filter(|(time, _)| window.is_none_or(|(start, end)| (start..=end).contains(time)))
            .map(|(_, ip)| *ip)
            .collect();
        let by_ip = by_ip_counts(&ips);
        let total = ips.len() as isize;
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
        let ranges: Vec<(u64, u64, String)> = maps
            .lines()
            .filter(|line| {
                line.split_whitespace()
                    .nth(1)
                    .is_some_and(|perms| perms.contains('x'))
            })
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let (low, high) = parts.next()?.split_once('-')?;
                let path = parts.nth(4).unwrap_or("[anon]").to_string();
                Some((
                    u64::from_str_radix(low, 16).ok()?,
                    u64::from_str_radix(high, 16).ok()?,
                    path,
                ))
            })
            .collect();
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let module_of = |ip: u64| -> String {
            ranges
                .iter()
                .find(|(low, high, _)| (*low..*high).contains(&ip))
                .map_or_else(|| "[unmapped]".to_string(), |(_, _, path)| path.clone())
        };
        let mut by_module: HashMap<String, isize> = HashMap::new();
        for (ip, count) in &by_ip_counts(&ips) {
            *by_module.entry(module_of(*ip)).or_default() += count;
        }
        let stable: isize = by_module
            .iter()
            .filter(|(module, _)| {
                **module == exe || module.contains("libc.so") || module.contains("libm.so")
            })
            .map(|(_, count)| count)
            .sum();
        let mut by_line: HashMap<String, isize> = HashMap::new();
        let mut by_inline: HashMap<String, isize> = HashMap::new();
        let mut by_function: HashMap<String, isize> = HashMap::new();
        for (ip, count) in by_ip {
            if module_of(ip) != exe {
                continue;
            }
            let mut frames: Vec<(String, String)> = Vec::new();
            backtrace::resolve(ip as *mut std::ffi::c_void, |symbol| {
                let name = symbol
                    .name()
                    .map_or_else(|| format!("{ip:#x}"), |name| short(&name.to_string()));
                let file = symbol
                    .filename()
                    .and_then(|path| path.file_name())
                    .map_or_else(
                        || "?".to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                frames.push((name, format!("{file}:{}", symbol.lineno().unwrap_or(0))));
            });
            if frames.is_empty() {
                frames.push((format!("{ip:#x}"), "?".to_string()));
            }
            let (name, line) = &frames[0];
            *by_line.entry(format!("{line} {name}")).or_default() += count;
            *by_inline.entry(name.clone()).or_default() += count;
            let outer = &frames[frames.len() - 1].0;
            *by_function.entry(outer.clone()).or_default() += count;
        }
        let mut out = String::new();
        let _ = writeln!(
            out,
            "window: {}",
            if window.is_some() {
                "measured window of the run"
            } else {
                "whole run"
            }
        );
        let relative = |time: u64| time as i64 - mono_now as i64;
        let _ = writeln!(
            out,
            "debug (ns relative to exit): samples {:?}..{:?} window {:?}",
            samples.first().map(|s| relative(s.0)),
            samples.last().map(|s| relative(s.0)),
            window.map(|(start, end)| (relative(start), relative(end)))
        );
        let _ = writeln!(out, "instruction samples: {total}");
        let _ = writeln!(out, "stable samples (runner, libc, libm): {stable}");
        section(&mut out, "modules", by_module, total, 20);
        section(
            &mut out,
            "self by physical function (inlined callees included)",
            by_function,
            total,
            70,
        );
        section(&mut out, "self by innermost function", by_inline, total, 70);
        section(&mut out, "self by line", by_line, total, 90);
        std::fs::write(&self.path, out).expect("writing instruction profile");
    }
}

fn by_ip_counts(ips: &[u64]) -> HashMap<u64, isize> {
    let mut by_ip: HashMap<u64, isize> = HashMap::new();
    for ip in ips {
        *by_ip.entry(*ip).or_default() += 1;
    }
    by_ip
}

pub fn start() -> Profiler {
    Profiler {
        counters: Counters::start(),
        instructions: InstructionProfile::start(),
        sampler: start_sampler(),
    }
}

fn start_sampler() -> Option<(Sender<()>, JoinHandle<()>)> {
    let path = std::env::var("VSP_PROFILE").ok()?;
    let delay = Duration::from_millis(env_u64("VSP_PROFILE_DELAY_MS", 3500));
    let hz = env_u64("VSP_PROFILE_HZ", 500) as i32;
    let (stop, stopped) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        if stopped.recv_timeout(delay).is_ok() {
            return;
        }
        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(hz)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .expect("starting profiler");
        let _ = stopped.recv();
        let report = guard.report().build().expect("building profile report");
        let mut total = 0isize;
        let mut self_by_name: HashMap<String, isize> = HashMap::new();
        let mut incl_by_name: HashMap<String, isize> = HashMap::new();
        let mut self_by_line: HashMap<String, isize> = HashMap::new();
        // Self cost by source line, split by the chain of the next callers.
        let mut by_caller: HashMap<String, isize> = HashMap::new();
        let mut by_thread: HashMap<String, isize> = HashMap::new();
        for (frames, count) in &report.data {
            total += count;
            *by_thread.entry(frames.thread_name_or_id()).or_default() += count;
            let mut seen = HashSet::new();
            let mut leaf = None;
            let mut chain: Vec<String> = Vec::new();
            for physical in &frames.frames {
                for symbol in physical {
                    let file = symbol.filename();
                    let file = file.rsplit('/').next().unwrap_or("?");
                    let name = format!("{file} {}", short(&symbol.name()));
                    if leaf.is_none() {
                        *self_by_name.entry(name.clone()).or_default() += count;
                        let line = format!("{file}:{} {name}", symbol.lineno());
                        *self_by_line.entry(line.clone()).or_default() += count;
                        leaf = Some(line);
                    } else if chain.len() < 5 && chain.last() != Some(&name) {
                        chain.push(name.clone());
                    }
                    if seen.insert(name.clone()) {
                        *incl_by_name.entry(name).or_default() += count;
                    }
                }
            }
            if let Some(leaf) = leaf {
                *by_caller
                    .entry(format!("{leaf}  <-  {}", chain.join(" <- ")))
                    .or_default() += count;
            }
        }
        let mut out = String::new();
        let _ = writeln!(out, "samples: {total} at {hz} Hz");
        section(&mut out, "threads", by_thread, total, 20);
        section(&mut out, "inclusive", incl_by_name, total, 120);
        section(&mut out, "self", self_by_name, total, 80);
        section(&mut out, "self by line", self_by_line, total, 60);
        section(&mut out, "self with callers", by_caller, total, 60);
        std::fs::write(&path, out).expect("writing profile report");
    });
    Some((stop, thread))
}

pub fn finish(profiler: Profiler) {
    if let Some((stop, thread)) = profiler.sampler {
        let _ = stop.send(());
        let _ = thread.join();
    }
    if let Some(counters) = profiler.counters {
        counters.finish();
    }
    if let Some(instructions) = profiler.instructions {
        instructions.finish();
    }
}

fn section(out: &mut String, title: &str, map: HashMap<String, isize>, total: isize, limit: usize) {
    let mut rows: Vec<_> = map.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let _ = writeln!(out, "\n== {title}");
    for (name, count) in rows.into_iter().take(limit) {
        let _ = writeln!(
            out,
            "{:6.2}% {count:6}  {name}",
            100.0 * count as f64 / total as f64
        );
    }
}

/// Drops generic arguments (`name<...>`, keeping qualified paths `<T as Trait>::f`) so that rows
/// stay readable.
fn short(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut stack: Vec<bool> = Vec::new();
    let mut skipping = 0usize;
    for ch in name.chars() {
        match ch {
            '<' => {
                let generic = skipping > 0
                    || out
                        .chars()
                        .last()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_');
                stack.push(generic);
                if generic {
                    skipping += 1;
                } else {
                    out.push(ch);
                }
            }
            '>' => match stack.pop() {
                Some(true) => skipping -= 1,
                _ => out.push(ch),
            },
            _ if skipping == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}
