# vespertine_bench

Reproducible frame-cost benchmarks and retained-output verification (DEV-003). Nothing here is
required by production rendering: it uses the `test-support` surfaces of `vespertine`
(`Window::frame_stats`, `set_retention_enabled`, `painted_primitives`, ...).

## `vsp-bench`

```sh
cargo run --release -p vespertine_bench --bin vsp-bench -- --list
cargo run --release -p vespertine_bench --bin vsp-bench -- quotes --frames 200 --warmup 30
cargo run --release -p vespertine_bench --bin vsp-bench -- transcript --retention on --verify
cargo run --release -p vespertine_bench --bin vsp-bench -- quotes-sparse --verify-only --fault 7   # must FAIL
cargo run --release -p vespertine_bench --bin vsp-bench -- wheel-scroll --window                  # real window
```

Each workload is created fresh for each retention mode in a real window on the deterministic test
platform (880 x 480 logical px, scale factor 2, 60 Hz frame interval) with real text shaping
(Noto Sans, Liberation Mono). One scripted step (a model or input change) is applied per frame; the
runner drains application tasks and effects until the window is parked and times the complete
resulting frame, subtracting the step's own CPU time, wall time and allocations (what the step
passes to `Stage::mutate`; observers and subscriptions it triggers are the frame's). If a step
leaves the window clean the runner forces exactly one draw and counts it as a *forced* draw,
separately from natural draws.

Per metric the report gives mean, median, 95th percentile (nearest rank) and maximum per frame:
thread CPU time, wall time, process-wide allocation calls and bytes, and retired user-space
instructions of the UI thread. Instructions are `unavailable (reason)` when `perf_event_open`
cannot be opened or the counter does not advance; never zero. The window's `FrameStats` (views
built/reused/spliced, layout nodes, writes, measurements, rebinds, lines shaped, layer and list
counters, and with `--phase-times` the phase timers) are reported alongside. A platform header
records compositor and backend, scale factor, refresh rate, CPU/GPU, build profile and exactly which
clocks were measured. Debug builds are not comparable with release builds.

`--verify` / `--verify-only` run lockstep verification: a retained and a retention-disabled window
are drawn through identical steps from fresh state and every frame the expanded scene primitives
(paint order, clipping, geometry, color, text layout, and what each sprite shows, with atlas
packing ignored), hit-test results and listener-observed positions are compared. `--a11y` adds
the accessibility tree, `--pixels` the GPU-rendered pixels. `--fault SEED` places a seeded
one-pixel fault in the retained window; the run must fail. New checks implement
`verify::FrameCheck` and are added with `Lockstep::with_check`.

### Workloads

| family | workloads |
| --- | --- |
| baseline | `idle-unchanged` (every frame a forced draw) |
| fields | `typing-validation` |
| selection | `selection-toggle` |
| lists | `list-uniform-scroll`, `list-uniform-live`, `list-variable-scroll`, `list-variable-live` |
| quotes (200 rows) | `quotes-sparse` (4 of 200 rows per frame), `quotes-dense` (100 of 200), `quotes-sorted` (sort every 15 frames, exactly one price update between sorts) |
| virtual table | `quotes-virtual` (10,000 rows of 26 px, scrolled 37 px per frame) |
| settings | `settings-navigation`, `settings-theme` |
| nested layout | `nested-layout` |
| wheel | `wheel-scroll` |
| scrollbar | `scrollbar-drag` |
| transcripts | `transcript-160`, `transcript-200` (four trackpad events totaling 40 px per frame, 120-frame sweeps, 30-frame dwells, a jump every 240 frames, a 108-frame highlight) |
| workspace | `workspace-docked` |
| PERF-DEV-003 | `perf-dev-003-sparse` (warmup 30), `-dense` (warmup 430), `-sorted` (warmup 830); the scene of the same name is in `vespertine_scenes` |

A workload's step is a pure function of the absolute frame index (warm-up frames included), so the
same workload run twice gives identical draws and counters.

## Examples

- `retention_showcase`: real-window showcase with independent retention and data-refresh controls,
  automatic idle/sidebar/page/table/refresh runs, status metrics every 500 ms, and a controlled
  retention off/on comparison in one process.
- `lockstep_verification`: lockstep verification passes on identical windows (also with different
  atlas packing) and fails on a seeded one-pixel fault of every kind.
- `panel_updates`: updating one panel of a dashboard rebuilds that panel only.
