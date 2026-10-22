# Upstream provenance

Vespertine is derived from https://github.com/zed-industries/zed at commit
`56934ec0560bce8929025497411de7f5322fb6ca` (2026-10-17).
The first commit imports the tracked files of the Linux GPUI dependency closure
verbatim; a second verbatim import adds the Apple, macOS, and Windows crates after
the requested desktop scope expansion. Development, benchmark, and example
dependencies are included. Every imported workspace crate declares
`license = "Apache-2.0"`. Everything added since those two commits (the crates
listed under "Crates without an upstream counterpart", the `packages/` npm
workspace, the scripts, and changes inside the imported crates) is Vespertine's
own work, also under Apache-2.0.

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

## Crates without an upstream counterpart

These workspace crates exist only in Vespertine; there is nothing to compare them with
when synchronizing:

`vespertine_bench`, `vespertine_components`, `vespertine_desktop`, `vespertine_host_tree`,
`vespertine_js`, `vespertine_js_node`, `vespertine_public_image_loader`,
`vespertine_richtext`, `vespertine_scenes`, `vespertine_test_host`,
`vespertine_theme`, `vespertine_updater` and `vespertine_webview`.

The npm packages under `packages/` and the scripts under `scripts/` are likewise
Vespertine's own. The README lists what each crate does.

Beyond these crates, the imported crates carry Vespertine changes (most of them in
`vespertine`, `vespertine_wgpu`, `vespertine_linux` and `vespertine_platform`); expect every
mapped directory to differ from its upstream counterpart by more than renames.

## Platform scope

Linux (Wayland and X11) is the verified platform: its backends are exercised by the test
suite, the examples and the scenes. The macOS and Windows backends, platform APIs, target
dependencies, features and build-script branches are preserved from upstream and have never
been built or run in this repository. Where Vespertine extended a cross-platform trait or
shared API, the macOS and Windows code was edited to match, without being compiled. Treat
those backends as unverified.

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
- Included only required font fixtures (Lilex and IBM Plex Sans under `assets/fonts/`),
  preserving their original licenses. Vespertine adds the six math faces bundled by
  `vespertine_components` (`crates/vespertine_components/src/math/fonts/`) and the variable test
  face `assets/fonts/test-sans-var/` (a renamed ASCII subset of IBM Plex Sans Var, as its Reserved
  Font Name requires); THIRD_PARTY.md lists them with their licenses.
- Preserved macOS/Windows tests, `windows-manifest`, and runtime Metal shaders.
- Vendored taffy 0.13.0 under `third_party/taffy` (MIT; a path dependency rather than a
  `[patch.crates-io]` entry, so projects that depend on Vespertine build it too), changed in
  two files. `src/tree/cache.rs`: each node retains sixteen measurement results (at least
  nine, LAY-001 R4) found by key and replaced with a second-chance policy, instead of one
  result per constraint kind; layout results are unchanged. `src/tree/taffy_tree.rs`: adds
  `TaffyTree::compute_subtree_layout_with_measure`, which recomputes one subtree with its own
  size fixed. Its `Cargo.toml` also declares an empty `[workspace]`, so the crate is its own
  workspace root even when this checkout sits inside another workspace. Taffy 0.14 was not
  adopted because it changes flex/grid/block results.
- Linux dispatcher timers (`vespertine_linux/src/linux/timers.rs`) are kept in one deadline queue
  behind a single kernel timer (`timerfd`) registered with the timer thread's epoll set. The calling
  thread arms that kernel timer itself whenever its deadline comes first, instead of sending a
  message to a timer thread that arms it; `BackgroundExecutor::timer` completes on the thread that
  fires the timer (`Task::from_timer`) instead of on a background worker, except under the test
  scheduler. A timer therefore wakes one thread, the timer thread when it fires, instead of the
  timer thread twice and a worker twice, and pending timers hold no file descriptor each.
  The desktop HTTP client's tokio runtime (`vespertine_reqwest_client`) is created by its first
  request instead of when the client is built.
- The Linux text backend (`vespertine_wgpu/src/cosmic_text_system.rs`) gives a variable face one
  font id per `wght` instance and passes that instance's variation coordinates to metrics, glyph
  metrics and the scaler; upstream loads every face at `Weight::NORMAL` without coordinates.
  `PlatformTextSystem` gains `shapes_ascii_words_independently` and `shapes_pairwise` (both `false`
  by default; the Linux backend answers them from cosmic-text's per-segment shaping and the font's
  own tables) so the shaping shortcuts run only where they are exact.
- Text style and layout types carry letter spacing: `TextStyle::letter_spacing`,
  `TextRun::letter_spacing`, `TextStyle::to_run(len, rem_size)`,
  `LineWrapper::wrap_line_with_runs` and a `runs` parameter of `LineWrapper::should_truncate_line`;
  `StyledText::with_default_highlights` builds its runs at layout time, when the rem size is known.
  `Style` gains `backdrop_blur`, painted by `Style::paint` before shadows and background.
- `App::set_text_rendering_mode` refreshes every window when the mode changes, and
  `DispatchTree::available_actions` / `is_action_available` leave out handlers above a modal
  scope, as dispatch does. `Window::focused_input_is_composing` reports whether the focused input
  holds marked text.
- The headless test context (`HeadlessAppContext`) keeps its test platform (`opened_urls()`) and
  returns the application's own text system from `text_system()`, so fonts registered through it
  reach its windows.
- `UniformListScrollHandle::set_offset_from_scrollbar` sets an offset that retained list rows count
  as scrolling; other programmatic offset changes leave an owner's notification to rebuild the
  visible rows.
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
  and macOS-main-thread-only tests retain their original annotations. Vespertine adds
  two ignored benchmark-style tests that print timings or counters (in
  `crates/vespertine/src/scene_order/tests.rs` and
  `crates/vespertine/src/taffy/persistence_tests.rs`); no other tests are ignored.

## Cross-target verification limitations

The macOS (`aarch64-apple-darwin`) and Windows (`x86_64-pc-windows-msvc`) backend libraries
type-check on the Linux host once `rustup` has installed both standard-library targets:

```sh
cargo check -p vespertine_windows --lib --no-default-features --target x86_64-pc-windows-msvc
cargo check -p vespertine_macos --lib --features font-kit,runtime_shaders --target aarch64-apple-darwin
```

With `runtime_shaders` the `vespertine_apple` build script stitches the Metal source for compilation
at run time; precompiled Metal shaders need Xcode on macOS. The Windows build script compiles HLSL
only on Windows (release builds; debug builds compile it at run time). Crates whose dependencies
compile C code for the target (`ring` and `aws-lc-sys` through the HTTP client, `zstd-sys`, and
`onig_sys` through syntax coloring) need that target's C toolchain (an Apple SDK, or MSVC), so
`cargo check --workspace --target <target>` does not complete on Linux. Neither backend has been
run.

## Linux verification

The workspace gates (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo test --workspace`, `./scripts/third-party --check`) run on Linux against the
Wayland and X11 backends; see the README for commands.
