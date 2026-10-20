# vespertine_scenes

Scenes and a scene runner for fidelity (`FID-<ID>-<n>`) and performance (`PERF-<ID>`) measurement.
A scene is a fixed-size window whose content is a pure function of a frame index. The runner binary
`vsp-scene-runner` opens one scene in a real window and either measures CPU time per frame or holds
a settled frame on screen for an external screenshot tool.

```
src/
  lib.rs            module wiring (generated lists for perf/ and fid/)
  scene.rs          `Scene` trait, `register_scene!`, `create_scene`
  runner.rs         runner core: CLI, window, perf/idle/capture modes, result file
  common.rs corpus.rs   helpers shared by scenes
  perf/<id>.rs      PERF-* scenes   (list flex text quads stream idle = the six baseline scenes)
  fid/<id>.rs       FID-* scenes
  bin/vsp-scene-runner.rs   Vespertine's `main`
build.rs            declares every src/perf/*.rs and src/fid/*.rs as a module
```

`scene.rs`, `runner.rs`, `common.rs`, `corpus.rs` and the six baseline `perf/*.rs` files (`list`, `flex`,
`text`, `quads`, `stream`, `idle`) are written so that they also compile unchanged against upstream
GPUI, with the `vespertine` dependency name bound to upstream, which keeps the baselines comparable
with it. Keep them using only `vespertine::...`, `inventory` (through `register_scene!`) and `libc`,
with no proc/derive macros, and do not add Vespertine-only API to them. Vespertine-only scenes go in
new files.

## Adding a scene

Create `src/fid/<id>.rs` (e.g. `src/fid/rnd_001.rs` holding `FID-RND-001-1` and `-2`) or
`src/perf/<id>.rs` (e.g. `src/perf/prf_004.rs` for `PERF-PRF-004`). No other file is edited: `build.rs`
makes the file a module and the file registers its scenes.

```rust
use crate::common::{hex, INK};
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, IntoElement, Styled, Window, div};

struct Rnd001;

impl Scene for Rnd001 {
    fn size(&self) -> (f32, f32) { (400.0, 300.0) }          // logical px, as in the spec
    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div().size_full().bg(hex(INK)).into_any_element()
    }
}

register_scene!("FID-RND-001-1", || Rnd001);
```

A scene's per-frame update must be a pure function of the frame index `f`. Capture mode renders
`capture_frame()` (default 0). A scene the build cannot render is simply not registered; asking
for an unregistered id exits with code 3.

Scene ids are the strings passed to `register_scene!`; `grep -rho 'register_scene!(\s*"[A-Z0-9-]*"' src`
lists most of them (a few registrations span lines). The runner has no listing option.

## Running the runner

```sh
cargo run --release -p vespertine_scenes --bin vsp-scene-runner -- \
    --scene PERF-LIST --mode perf --out target/perf-list.json
cat target/perf-list.json
```

Options: `--scene <ID>` and `--mode <perf|idle|capture>` and `--out <result.json>` are required;
`--warmup <N>` (default 120), `--frames <N>` (default 600), `--ready <file>` (required in capture
mode) and `--label <text>` (copied into the result). The window is `vsp-scene:<ID>` with app id
`vsp-scene`, unfocused, not resizable, with server-side decorations and the scene's logical size, and it requests
animation frames at the display rate. Exit codes: 0 success, 1 runtime failure, 2 bad command line,
3 unregistered scene, 4 capture lifetime expired.

- **perf**: renders `--warmup` frames, then times `--frames` frames and writes
  `{"scene", "mode", "label", "frames", "cpu_ns", "wall_ns"}` (process CPU time of all threads and
  wall time) to `--out`.
- **idle**: renders the warmup frames, then measures the CPU time of an idle interval of `--frames`
  display slots (60 Hz) during which nothing is requested; used by `PERF-IDLE`.
- **capture**: renders 60 frames of `capture_frame()`, waits 500 ms, writes `--out`, creates the
  `--ready` file and stays open for 30 seconds so that a screenshot tool can capture the window; the
  runner then exits with code 4. A capturing tool should wait for the ready file, take the
  screenshot and terminate the runner.

```sh
cargo run --release -p vespertine_scenes --bin vsp-scene-runner -- \
    --scene FID-RND-001-1 --mode capture --out target/capture.json --ready target/capture.ready
```

Timings are only comparable within one machine state: measure several runs in alternation, note the
machine load, and compare release builds only.

## Profiling

`--features profile` builds the runner with measurement aids that are inert unless their environment
variable is set:

- `VSP_IPROF=<file>`: samples the instruction pointer of the main thread every `VSP_IPROF_PERIOD`
  (default 100 000) user instructions through the kernel's performance counters and, when the runner
  exits, writes the samples per module and the cost per function and source line (inlined frames
  resolved). The "stable samples" line counts the samples in the runner binary, libc and libm: the
  instructions of the framework itself, without the spinning of GPU drivers and the compositor
  connection, which make plain instruction and CPU-time counts vary by tens of percent between runs
  on a loaded machine. Two runs with different `--frames` give the stable instructions of one frame
  without startup.
- `VSP_COUNT=<file>`: the whole process's user-space instruction and cycle counters (all threads),
  polled every 20 ms, headed by the time the result file was written.
- `VSP_PROFILE=<file>`: a timer-driven sampling CPU profiler (`pprof`, process CPU time of all
  threads) starting at `VSP_PROFILE_DELAY_MS` (default 3500, past startup and warmup) at
  `VSP_PROFILE_HZ` (default 500), reporting inclusive and self cost per function, self cost per
  source line and per caller chain.

The release profile already carries line tables. Build with the feature, run, and rebuild without it
before taking timings: a binary with the profilers must never be compared with one without.

```sh
cargo build --release -p vespertine_scenes --features profile
VSP_PROFILE=$PWD/target/list.prof target/release/vsp-scene-runner \
    --scene PERF-LIST --mode perf --frames 1200 --out target/prof-list.json
```
