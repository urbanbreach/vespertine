# vespertine_scenes

Scenes and the scene runner for fidelity (`FID-<ID>-<n>`) and performance (`PERF-<ID>`) measurement.
The runner binary `vsp-scene-runner` implements the runner protocol (`specs/scenes/PROTOCOL.md`); the
driver (`vsp-driver`, in the harness under `~/Projects/vespertine-cleanroom/clean/harness/`) builds it,
captures scenes and measures CPU time per frame.

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

`scene.rs`, `runner.rs`, `common.rs`, `corpus.rs` and the six baseline `perf/*.rs` files are the
**single source of truth** shared with the upstream baseline runner: the driver copies them into
the upstream workspace and compiles them unchanged against upstream GPUI (where the `vespertine`
dependency name is bound to upstream). Keep them using only `vespertine::...`, `inventory` (through
`register_scene!`) and `libc`, with no proc/derive macros. Do not add Vespertine-only API to them.
Vespertine-only scenes go in new files.

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
`capture_frame()` (default 0). Scenes that need more than the base window setup should say so in the
spec and the runner core, not in the scene. A scene the build cannot render is simply not registered;
asking for an unregistered id exits with code 3 (the driver records N/A).

## Building the runner

Use the driver's builder, which applies the shared release settings (opt-level 3, thin LTO,
codegen-units 1, no debug assertions, `-C symbol-mangling-version=v0`, toolchain 1.98.1) identically
for the upstream and the Vespertine runner:

```sh
export RUSTC_WRAPPER=$HOME/Projects/vespertine-cleanroom/tools/bin/sccache SCCACHE_DIR=$HOME/Projects/vespertine-cleanroom/tools/sccache-cache SCCACHE_CACHE_SIZE=80G
H=~/Projects/vespertine-cleanroom/clean/harness
D=$H/driver/vsp-driver/target/release/vsp-driver     # (cd $H/driver/vsp-driver && cargo build --release)
R=$($D build-vespertine --repo ~/Projects/vespertine/.worktrees/<owner>)   # prints <worktree>/target/release/vsp-scene-runner
U=$($D build-upstream --repo ~/Projects/vespertine)                         # baseline (shared sources from the repo's main checkout)
```

(`build-upstream --repo <worktree>` builds the baseline from your worktree's copy of the shared files.)
Plain `cargo build -p vespertine_scenes` works for quick checks but is not comparable for timing.

## Capturing a scene and comparing with the reference

The driver needs the Hyprland session (it creates a temporary headless output; see the harness README).
Fidelity scene sizes come from `reports/fidelity/SIZES.tsv`.

```sh
REP=~/Projects/vespertine-cleanroom/reports/fidelity
$D capture --runner vsp=$R --scene FID-RND-001-1 --scene-sizes $REP/SIZES.tsv --out-dir target/scene-out/cap
$D compare target/scene-out/cap/vsp-FID-RND-001-1.png $REP/ref/FID-RND-001-1.png --diff target/scene-out/cap/diff.png
```

A scene passes when at most 0.5 % of pixels differ by more than 8/255 in any channel (`compare` exits
0); the diff image shows differing pixels in red.

## Measuring against the upstream runner

```sh
$D perf --runner upstream=$U --runner vsp=$R --scene PERF-LIST --runs 10 --out-dir target/scene-out/perf
cat target/scene-out/perf/results.md          # median/min/max CPU µs per frame and the ratio to the first runner
```

Omit `--scene` for all six baseline scenes (`PERF-IDLE` runs in idle mode). Other ids need
`--scene-size ID=WxH` (or `--scene-sizes FILE`) and run in perf mode. Runners are interleaved with
rotated order within one invocation; measurements are only comparable within one invocation, and
the machine is shared, so note its load.

## Profiling

`--features profile` builds the Vespertine runner with measurement aids that are inert unless their
environment variable is set (pass them through the driver with `--runner-env KEY=VALUE`):

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

Build with the driver's release settings (plus line tables for the profilers) in a separate target
directory, and never compare timings of this binary with the driver's builds:

```sh
CARGO_TARGET_DIR=target/prof RUSTFLAGS="-C symbol-mangling-version=v0" RUSTUP_TOOLCHAIN=1.98.1 \
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release -p vespertine_scenes --features profile
$D perf --runner vsp=target/prof/release/vsp-scene-runner --scene PERF-LIST --runs 1 --frames 1200 \
    --runner-env 'VK_LOADER_DRIVERS_SELECT=*intel_icd*' --runner-env VSP_IPROF=$PWD/list.prof
```
