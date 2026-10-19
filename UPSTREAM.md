# Upstream provenance

Vespertine is derived from https://github.com/zed-industries/zed at commit
`56934ec0560bce8929025497411de7f5322fb6ca` (2026-10-17).
The first commit imports the tracked files of the Linux GPUI dependency closure
verbatim; a second verbatim import adds the Apple, macOS, and Windows crates after
the requested desktop scope expansion. Development, benchmark, and example
dependencies are included. Every imported workspace crate declares
`license = "Apache-2.0"`.

## Crate mapping

| Upstream path | Vespertine crate and directory under `crates/` |
| --- | --- |
| `crates/bench_metrics` | `vespertine_bench_metrics` |
| `crates/collections` | `vespertine_collections` |
| `crates/refineable/derive_refineable` | `vespertine_derive_refineable` |
| `crates/gpui` | `vespertine` |
| `crates/gpui_apple` | `vespertine_apple` |
| `crates/gpui_linux` | `vespertine_linux` |
| `crates/gpui_macos` | `vespertine_macos` |
| `crates/gpui_macros` | `vespertine_macros` |
| `crates/gpui_platform` | `vespertine_platform` |
| `crates/gpui_shared_string` | `vespertine_shared_string` |
| `crates/gpui_tokio` | `vespertine_tokio` |
| `crates/gpui_util` | `vespertine_util` |
| `crates/gpui_wgpu` | `vespertine_wgpu` |
| `crates/gpui_windows` | `vespertine_windows` |
| `crates/http_client` | `vespertine_http_client` |
| `crates/http_client_tls` | `vespertine_http_client_tls` |
| `crates/path` | `vespertine_path` |
| `tooling/perf` | `vespertine_perf` |
| `crates/refineable` | `vespertine_refineable` |
| `crates/reqwest_client` | `vespertine_reqwest_client` |
| `crates/scheduler` | `vespertine_scheduler` |
| `crates/sum_tree` | `vespertine_sum_tree` |
| `crates/util` | `vespertine_util_core` |
| `crates/util_macros` | `vespertine_util_macros` |
| `crates/zlog` | `vespertine_zlog` |
| `crates/ztracing` | `vespertine_ztracing` |
| `crates/ztracing_macro` | `vespertine_ztracing_macro` |

`gpui_util` and `util` would both map to `vespertine_util` under literal prefix
replacement. To avoid the collision, `gpui_util` retains that name and `util`
becomes `vespertine_util_core`. The nested derive crate and `tooling/perf` are
flattened into the standalone `crates/` layout.

Tests, benchmarks, and the text example require upstream's Lilex Regular/Bold
and IBM Plex Sans Regular font fixtures. These are copied under `assets/fonts/`
with their original SIL Open Font License 1.1 notices. They are assets, not
workspace crates; the crate code remains Apache-2.0.

## Platform scope

Linux (Wayland and X11) is the verified platform. macOS and Windows backends,
platform APIs, target dependencies, features, and build-script branches are
preserved from upstream and are checked through cross-target `cargo check`
where the Linux host's SDK/C compiler support permits. The macOS check enables
`vespertine_platform/runtime_shaders` to avoid build-time Metal compilation.
Native macOS and Windows runtime behavior has not been exercised here.

The `gpui_ios` and `gpui_web` crates are excluded, along with iOS and browser/wasm
code paths, dependencies, features, and platform-specific tests. The shared
`gpui_apple` crate is retained because the macOS backend requires it.

## Synchronizing with upstream

Keep an independent checkout of the pinned upstream commit; do not merge Zed's
entire workspace into this standalone repository. The two import commits provide
unmodified baselines (the derive crate and perf paths are flattened).
For each mapping above, compare current source with its upstream directory and
review native desktop behavior and namespace changes:

```sh
git log --oneline
# Example, with an independent upstream checkout at /path/to/zed:
git diff --no-index /path/to/zed/crates/gpui crates/vespertine
# Compare upstream changes between sync points before porting:
git -C /path/to/zed diff 56934ec0560bce8929025497411de7f5322fb6ca NEW_COMMIT -- crates/gpui
```

Repeat for each table row, accounting for the crate namespaces and excluded
iOS/browser branches. Check licenses before expanding the dependency closure.
Preserve upstream dependency versions and necessary source patches; start with
upstream's lockfile and let Cargo prune it instead of broadly updating.
The `windows-capture` patch preserves `scap`'s upstream target-only transitive
source pin even when resolving the Linux build.

## Intentional deviations

- Renamed all imported workspace crates and GPUI environment/logging namespaces;
  `util` uses the approved collision-resolution name `vespertine_util_core`.
- Excluded iOS and browser/WebAssembly support, including browser entrypoints,
  browser threading/tracing features, and WebGL-specific instance transport.
- Included only required font fixtures, preserving their original licenses.
- Preserved macOS/Windows tests, `windows-manifest`, and runtime Metal shaders.
- Vendored taffy 0.13.0 under `third_party/taffy` (MIT; `[patch.crates-io]`), changed
  only in `src/tree/cache.rs`: each node retains sixteen measurement results (at least
  nine, LAY-001 R4) found by key and replaced with a second-chance policy, instead of
  one result per constraint kind. Layout results are unchanged; taffy 0.14 was not
  adopted because it changes flex/grid/block results.
- Removed features `vespertine_platform/multithreaded`,
  `vespertine_scheduler/wasm-threads`, and `vespertine_ztracing/web`.
- Removed only these browser-transport tests:
  `webgl_shader_is_valid_wgsl_without_storage_buffers`,
  `webgl_record_sizes_match_shader_word_strides`, and
  `webgl_quad_layout_matches_fixed_decoder`.
- Corrected `foreground_work_reports_long_task_without_window_draw` to inspect
  its thread-local foreground journal for draw/present events rather than the
  process-global frame collector, which includes unrelated parallel tests.
  Its no-window-work and long-task reporting assertions remain enforced.
- Corrected the resource-counter test to touch fresh anonymous mapped pages
  instead of allocator memory that can already be resident. Linux disables
  huge pages for that mapping, preserving the original page-fault assertion.
- Updated the embedded-asset fixture's repository-relative path and logger's
  expected crate name to match the renamed workspace crates.
- Re-encoded `examples/image/black-cat-typing.gif` with GIF interlacing to avoid
  a byte sequence rejected by the repository naming gate. Coalesced frame pixel
  hashes and frame delays are identical.
- Upstream's already-ignored timing-printing tests (`frame_cost`, `hash_cost`)
  and macOS-main-thread-only tests retain their original annotations; no new
  tests are ignored.

## Cross-target verification limitations

Both Rust standard-library targets installed successfully. The Linux host could
not complete either cross-target check, so neither backend is claimed fully
type-checked or runtime-verified:

- Windows (`x86_64-pc-windows-msvc`): Cargo exited 101. `ring 0.17.14` and
  `zstd-sys 2.0.16+zstd.1.5.7` fail in their C build scripts because `lib.exe`
  is unavailable (`failed to find tool "lib.exe": No such file or directory`).
  `aws-lc-sys 0.45.0` also encounters host-C-compiler target incompatibilities.
- macOS (`aarch64-apple-darwin`, with `runtime_shaders`): Cargo exited 101.
  `ring 0.17.14` and `aws-lc-sys 0.45.0` encounter a host C compiler that rejects
  `-arch arm64` and `-mmacosx-version-min=11.0` (and `-gfull` for ring).
  No Apple cross-C-toolchain/SDK is configured on this host.

These are toolchain failures, not Rust diagnostics in the preserved backends.
No dependency or build-script behavior was disabled to bypass them.

Library-only workspace checks were also attempted on both targets and exited
101 with the same toolchain limitations (Windows additionally reports missing
`lib.exe` in `alloca 0.4.0`). Dependency-tree inspection confirms these are not
merely example/test edges: workspace member `vespertine_http_client_tls` normally
depends on `rustls` with default `aws_lc_rs`, and `vespertine_reqwest_client`
normally depends on reqwest's `rustls-tls-native-roots`/`ring` transport.
`vespertine_util_core` normally depends on `rust-embed` with `compression`, which
pulls `include-flate`/`include-flate-compress`, `zstd`, `zstd-safe`, and `zstd-sys`
(also through the include-flate codegen proc macro). Consequently `--lib` does
not remove these C prerequisites from the workspace graph.

## Linux verification

The root formatting, Clippy (`--workspace --all-targets -- -D warnings`),
workspace test, and release all-target build gates completed successfully.
The test suite reports 901 passed and 16 upstream-ignored tests/doctests.
After prebuilding the executable, both Wayland and forced-X11
(`env -u WAYLAND_DISPLAY DISPLAY=:0`) `hello_world` smoke commands launched
`target/release/examples/hello_world` and remained alive until the 8-second
timeout (exit 124). The desktop observation API exposed no windows, so visual
appearance was not independently confirmed.


