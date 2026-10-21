# Vespertine

Vespertine is a fork of [GPUI](https://github.com/zed-industries/zed), Zed's GPU-accelerated Rust UI
framework. The framework API follows upstream GPUI under the `vespertine` crate namespace: the same
hybrid immediate and retained model (entities, views, elements, a Tailwind-style styling API,
actions, an integrated async executor), with the additions listed below. Nothing is published to
crates.io or npm (`publish = false`); depend on the crates by path or git from a checkout.

Linux, on both Wayland and X11, is the verified platform. The macOS and Windows backends are
preserved from upstream but have not been built or verified in this repository (see
[Platform status](#platform-status)). iOS and browser/WebAssembly support are excluded.

Vespertine is derived from [zed-industries/zed](https://github.com/zed-industries/zed), copyright
Zed Industries, Inc., and distributed under Apache-2.0. See [Licensing](#licensing).

## What Vespertine adds to GPUI

[FEATURES.md](FEATURES.md) lists every implemented capability, 136 specifications in 13 areas,
with the tests and examples that verify each one. In summary:

### Rendering effects (RND)

- Scoped edge fade (`Window::with_edge_fade`) and antialiased rounded content clipping
  (`Window::with_rounded_clip`, `rounded_clip`), applied per fragment to everything painted inside.
- Ordered Gaussian backdrop blur (`Window::paint_backdrop_blur`); feathered inverse
  rounded-rectangle image masks and fitted image painting; transformed and blurred glyph and icon
  painting.
- Shared-device GPU texture surfaces on Linux (`surface`), raw BGRA images with explicit
  nearest-neighbor painting, layered surface plates, and canvases that paint vector shapes and text.

### Text and rich text (TXT)

- Punctuation-aware line wrapping (closing punctuation, CJK), fallback fonts that keep the requested
  weight and style, runtime font registration, font catalogs, and independent interface, code and
  terminal typography roles.
- `vespertine_richtext`: shaped rich text with source-aware geometry (carets, link hit boxes, one
  rectangle per wrapped row), bounded cached syntax coloration, document-wide selection and copy,
  safe link activation, and search washes.

### Retained frames and performance (PRF, LAY-001, ANM-001)

- Dependency-driven retained frames: a view whose tracked inputs did not change replays its previous
  output without running `render`, layout, prepaint or paint (see the `_retained_frames` module
  documentation of `vespertine`).
- Persistent incremental layout nodes (with a vendored taffy), incremental scene ordering,
  compact interaction storage, adaptive tiled scroll-content reuse, incremental virtual-list row
  replay, a translation-invariant tessellation cache, batched GPU uploads and render-pass reuse,
  bounded renderer resources, carried text measurements, bounded shaping caches, and in-place
  image atlas updates.
- Demand-gated display frame delivery, shared parked animation clocks, and retained state that
  suppresses no-op commits.

### Layout and elements (LAY, ELM, ANM)

- Variable-height virtual lists with windowing, anchors, padding and estimates, trailing viewport
  reservation, incremental scrolling from negative offsets, composable responsive layout,
  collapsible resizable panes, and trigger-relative floating layers.
- Image fitting with element-space rounded corners, asynchronous images and media hosts,
  color-preserving vector icons, cached source-space image treatments, serializable layout and
  interactive styling, persistent custom-element adapters, and layout-neutral outlines.
- Native declarative motion with retargeting and interruption, and presence management with
  exit-animation completion.

### Components (CMP)

`vespertine_components` provides components built from elements, among them: buttons,
switches and sliders; floating menus, popovers, dialogs, tooltips and hover cards; select, pickers
and command palettes; single-line and multiline text editing with native composition; Markdown
editing and rich Markdown documents with inline and display math, themed diagrams, code blocks and
code editors; unified and split diff views; virtual file trees; tabs and segmented navigation;
toasts; data tables and charts; image preview and viewer; a terminal grid surface; an embedded
browser surface; loading placeholders and progress; retained three-dimensional and shader image
surfaces; audio activity visualizations; badges and review cards; streaming bottom-follow lists;
elastic overscroll; and scrollbars. The `workbench` example composes many of them.

### Embedded web view (PLT-014 to PLT-016)

`vespertine_webview` runs WebKitGTK 4.1 in a separate helper process (never linked into the
application) and composites its frames into the window with the scene layered around the native
content. It provides the helper protocol, lifecycle and failure handling, messaging, pixel transfer,
input routing, focus and text composition. `vespertine_components::browser` adds the surrounding
browser chrome. Linux only.

### Linux platform integration (PLT, INP)

- Wayland and X11 backends (`vespertine_linux`): presentation-synchronized native updates, explicit
  toolkit scale on X11, the X11 input-method handshake, client-side window chrome, window geometry
  lifecycle and close interception, layer-shell windows, and reentrant appearance updates.
- Text, image, path and scoped rich clipboard; native file pickers and external destination opening
  through the desktop portals; desktop notifications and optional feedback sounds;
  permission-aware capture and global shortcuts (`vespertine_desktop`); a bounded accessibility
  context and device permission status.
- Input: stable pointer capture, hover suppression during wheel scroll, robust touch-fling release
  velocity, subset-constrained tab traversal, focus-scoped actions and shortcuts, typed drag and
  drop with autoscroll.

### JavaScript host with React and Solid adapters (APP, DEV-009, DEV-010)

A JavaScript process (Bun or Node) owns a declarative UI while rendering, input and windows run on
a native UI thread. `vespertine_host_tree` holds the retained tree and applies atomic, serialized
mutation batches; `vespertine_js` is the Node-API-independent host core and `vespertine_js_node`
the Node-API addon. The `@vespertine/host` package wraps the addon, and `@vespertine/react` and
`@vespertine/solid` are renderer adapters with headless components, reactive window, selection and
search helpers, and (React) a runtime error display with remount recovery. `vespertine_updater`
provides signed packaged-application updates (Minisign-verified AppImage replacement).
`create-vespertine` and `@vespertine/dev` create and develop projects.

### Offscreen test host and automation (DEV)

`vespertine_test_host` drives the production retained tree, layout, hit testing, event dispatch and
GPU renderer through an offscreen window that never reaches the compositor, and exposes the painted
pixels and geometry for inspection, plus application automation with strict locators.
`vespertine_bench` and the `vespertine` diagnostics add reproducible frame-cost benchmarks,
retained-output lockstep verification, frame-overlay statistics, committed paint geometry
observation, and opt-in renderer memory accounting.

### Theming (THM)

`vespertine_theme`: shared CSS color parsing into bounded sRGB, runtime-overridable component
palettes and metrics, semantic themes with live appearance resolution, and theme import with
validation and image-derived palette hints.

### Accessibility (A11Y)

Declarative accessibility properties and leaf metadata on elements (AccessKit), and accessibility
tree replay that cooperates with retained frames. See the `_accessibility` module documentation of
`vespertine`.

## Crates

All crates live under `crates/` and are workspace members. Crates marked *upstream* derive from the
corresponding GPUI/Zed crate ([UPSTREAM.md](UPSTREAM.md) has the mapping); the others exist only in
Vespertine.

| Crate | Purpose |
| --- | --- |
| `vespertine` | The framework (upstream `gpui`): application, entities, views, elements, layout, text, windows, input, animation, accessibility and the Vespertine additions above. |
| `vespertine_apple` | Shared Apple-platform rendering, dispatch and system services (*upstream*; unverified here). |
| `vespertine_bench` | Frame-cost benchmarks (`vsp-bench`) and retained-output lockstep verification. |
| `vespertine_bench_metrics` | Criterion measurements beyond wall time: hardware counters, `getrusage`, allocations (*upstream*). |
| `vespertine_collections` | Collection types used across the workspace (*upstream*). |
| `vespertine_components` | Reusable components built from elements, and the `workbench` example. |
| `vespertine_derive_refineable` | Derive macro for refinement types (*upstream*). |
| `vespertine_desktop` | Desktop services: feedback sounds, capture, global shortcuts, accessibility context and permission status. |
| `vespertine_host_tree` | Retained host tree with atomic serialized mutation batches and interned shared styles. |
| `vespertine_http_client` | HTTP client abstraction (*upstream*). |
| `vespertine_http_client_tls` | TLS configuration for the HTTP client (*upstream*). |
| `vespertine_js` | Native JavaScript host core: renderer object, UI thread, command channel, window options, path prompts. |
| `vespertine_js_node` | Node-API addon exposing the JavaScript host to Bun and Node. |
| `vespertine_linux` | Linux backends: Wayland and X11 windowing, input, clipboard, portals, notifications (*upstream*). |
| `vespertine_macos` | macOS backend (*upstream*; unverified here). |
| `vespertine_macros` | Procedural macros of the framework (*upstream*). |
| `vespertine_path` | Absolute and relative path types (*upstream*). |
| `vespertine_perf` | Test performance profiler tool (*upstream*). |
| `vespertine_platform` | Re-exports the platform traits and `application()`, which selects the backend for the host OS (*upstream*). |
| `vespertine_public_image_loader` | Restricted HTTPS client for images from untrusted content. |
| `vespertine_refineable` | The `Refineable` trait (*upstream*). |
| `vespertine_reqwest_client` | `reqwest`-based HTTP client (*upstream*). |
| `vespertine_richtext` | Shaped rich text, syntax coloration, selection, links, search washes. |
| `vespertine_scenes` | Fidelity and performance scenes and the `vsp-scene-runner` binary. |
| `vespertine_scheduler` | Task scheduler and executors (*upstream*). |
| `vespertine_shared_string` | The `SharedString` type (*upstream*). |
| `vespertine_sum_tree` | Concurrency-friendly B-tree with summaries (*upstream*). |
| `vespertine_test_host` | Offscreen production-path visual test host and application automation. |
| `vespertine_theme` | Component palettes, semantic themes, theme import. |
| `vespertine_tokio` | Tokio integration with the framework executor (*upstream*). |
| `vespertine_updater` | Signed packaged-application updates. |
| `vespertine_util` | Small utilities of the framework (*upstream* `gpui_util`). |
| `vespertine_util_core` | General utilities (*upstream* `util`). |
| `vespertine_util_macros` | Utility macros (*upstream*). |
| `vespertine_webview` | Embedded WebKitGTK web view: helper protocol, lifecycle, pixel transfer, compositing. Linux only. |
| `vespertine_wgpu` | GPU renderer on wgpu for Linux (*upstream*); see its README. |
| `vespertine_windows` | Windows backend (*upstream*; unverified here). |
| `vespertine_zlog` | Logging (*upstream*). |
| `vespertine_ztracing` | Tracing and profiling spans (*upstream*). |
| `vespertine_ztracing_macro` | Macros for `vespertine_ztracing` (*upstream*). |

JavaScript packages (an npm workspace under `packages/`):

| Package | Purpose |
| --- | --- |
| `@vespertine/host` | Runtime for the native addon: mutation queue, event routing, observers, frame loop, idle garbage collection, update check. |
| `@vespertine/react` | React renderer adapter, headless components and the offscreen testing helper. |
| `@vespertine/solid` | Solid renderer adapter and headless components; includes the Bun/Babel integration. |
| `@vespertine/examples` | Reference example surfaces for the React adapter. |
| `@vespertine/dev` | Rebuild-on-change workflow (`vespertine-dev`) and curated screenshots. |
| `create-vespertine` | Creates a project from the `packages/starter` template. |

## Requirements

- The Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by `rustup`), with
  `rustfmt`, `clippy` and `rust-src`.
- Linux with a Wayland or X11 session and a working Vulkan driver. The framework links the
  development packages of `libxcb`, `libxkbcommon` and `libxkbcommon-x11`.
- For `vespertine_webview` (a workspace member, so part of every workspace build): `pkg-config` and
  the `webkit2gtk-4.1` and `json-glib-1.0` development files (Debian/Ubuntu `libwebkit2gtk-4.1-dev`,
  `libjson-glib-dev`; Fedora `webkit2gtk4.1-devel`, `json-glib-devel`). Set
  `VSP_WEBKIT_HELPER_BINARY` to embed a prebuilt helper instead of compiling `helper/*.c`.
- For the JavaScript packages: Node 20 or later, or Bun.

## Build and test

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --all-targets --release
# The macOS and Windows backends type-check from Linux
# (rustup target add aarch64-apple-darwin x86_64-pc-windows-msvc):
cargo check -p vespertine_windows --lib --no-default-features --target x86_64-pc-windows-msvc
cargo check -p vespertine_macos --lib --features font-kit,runtime_shaders --target aarch64-apple-darwin
./scripts/check-features --no-run-examples
./scripts/check-features --spec RND-006        # one specification, including running its example
```

`cargo test --workspace` also runs `scripts/third-party --check` (the dependency license policy).
`./scripts/check-features` verifies the rows of FEATURES.md: it builds every referenced test and
example, runs the tests, and launches each example for ten seconds in the current graphical session
(a window opens per example; `--no-run-examples` only builds them, `--spec ID` restricts it to one
specification).

The JavaScript packages are an npm workspace (one install, one lockfile; run `npm install` in `packages/`
once for their third-party dependencies):

```sh
cd packages/vespertine-host
node scripts/build-native.mjs --release   # builds the Node-API addon, installs native/vespertine_js.node
npm test
cd ../vespertine-react && npm test        # likewise for the other packages
cargo test -p vespertine_js_node          # builds the addon and runs the package tests under Node and Bun
```

## Running examples

Examples are Cargo examples of their crates; `cargo run --example <name>` finds an example of any
workspace member when its name is unique.

```sh
cargo run --release --example hello_world
# Force X11 rather than Wayland:
env -u WAYLAND_DISPLAY cargo run --release --example hello_world
# The agent workbench, built only from Vespertine components. The scripted run drives the window for
# 60 seconds, asserts its invariants (bottom-follow while streaming, input cleared on submit, active
# tab) and exits 0 on success:
cargo run --release --example workbench
cargo run --release --example workbench -- --scripted 60
```

More examples, by crate (`cargo run --release -p <crate> --example <name>`):

| Crate | Examples |
| --- | --- |
| `vespertine` | `backdrop_blur`, `rounded_clip`, `gpu_surface`, `surface_plate`, `canvas_text`, `image_fit`, `virtual_list_windowed`, `pane_group`, `motion_retarget`, `custom_chrome`, `a11y`, `input`, ... (see `crates/vespertine/examples/README.md`) |
| `vespertine_components` | `workbench`, `markdown_editor`, `code_editor`, `diff_viewer`, `math_formulas`, `chart_gallery`, `file_tree`, `commit_table`, `terminal_surface`, `browser_surface`, `toasts`, `scrollbars`, ... (35) |
| `vespertine_webview` | `web_page`, `web_page_layers` |
| `vespertine_richtext` | `rich_geometry` |
| `vespertine_theme` | `theme_appearance`, `theme_import`, `color_swatches`, `document_theme_live` |
| `vespertine_bench` | `retention_showcase`, `lockstep_verification`, `panel_updates` |
| `vespertine_desktop` | `a11y_context`, `capture_service`, `permission_status` |
| `vespertine_linux` | `native_input`, `native_layering`, `native_popover`, `native_resize`, `wayland_protocol` |
| `vespertine_wgpu` | `edge_fade`, `glyph_transform`, `registered_font`, `typography` |
| `vespertine_host_tree` | `host_tree_mount`, `mutation_import_bench` |

Frame-cost benchmarks: `cargo run --release -p vespertine_bench --bin vsp-bench -- --list` (see
`crates/vespertine_bench/README.md`).

## Scenes

`vespertine_scenes` holds fixed-size scenes, each a pure function of a frame index: `FID-<ID>-<n>`
scenes for visual fidelity and `PERF-<ID>` scenes for per-frame CPU cost. The runner opens one scene
in a window. Perf mode renders warmup and measured frames and writes a JSON result; capture mode holds
a settled frame on screen for an external screenshot tool:

```sh
cargo run --release -p vespertine_scenes --bin vsp-scene-runner -- \
    --scene PERF-LIST --mode perf --out target/perf-list.json
cargo run --release -p vespertine_scenes --bin vsp-scene-runner -- \
    --scene FID-RND-001-1 --mode capture --out target/capture.json --ready target/capture.ready
```

The crate README describes the modes, how to add a scene and how to profile.

## Platform status

| Platform | Status |
| --- | --- |
| Linux, Wayland | Verified: test suite, examples, scenes. |
| Linux, X11 | Verified: test suite, examples, scenes. Explicit toolkit scale is supported. |
| macOS | Upstream backend (`vespertine_macos`, `vespertine_apple`) preserved. Type-checks from Linux (see [Build and test](#build-and-test)); not run or verified. |
| Windows | Upstream backend (`vespertine_windows`) preserved. Type-checks from Linux (see [Build and test](#build-and-test)); not run or verified. |
| iOS, browser/WebAssembly | Excluded. |

The embedded web view exists only on Linux. When a cross-platform trait was extended, the macOS and
Windows code was edited to follow it. Both backends type-check on a Linux host, but crates that
compile C code for the target need that target's toolchain, so full builds and runs need the target
operating system; [UPSTREAM.md](UPSTREAM.md) has the details.

## Licensing

Vespertine is licensed under the Apache License 2.0 ([LICENSE-APACHE](LICENSE-APACHE)); every
workspace crate declares `license = "Apache-2.0"`. [NOTICE](NOTICE) records the origin (Zed Industries,
Inc.), the bundled fonts with their own licenses (Lilex and IBM Plex Sans under SIL OFL 1.1; the math
faces of `vespertine_components` under SIL OFL 1.1 or the GUST Font License) and the vendored taffy
(MIT). [THIRD_PARTY.md](THIRD_PARTY.md) lists every third-party Rust crate, npm package and bundled
data set with its license; it is generated by `scripts/third-party` (run it after changing
`Cargo.lock` or `packages/package-lock.json`), and `cargo test` fails when a dependency violates the
license policy stated at its top.

## Upstream sync

[UPSTREAM.md](UPSTREAM.md) records the pinned upstream commit, the mapping from upstream crates to
Vespertine crates, the crates that have no upstream counterpart, the intentional deviations, and the
procedure for comparing and porting upstream changes. Do not merge Zed's workspace wholesale; compare
each mapped directory against an independent checkout of upstream.

## Contributing

- Work on a branch in a git worktree, e.g. `git worktree add .worktrees/<name> -b work/<name> main`.
- Every implemented capability has a row in [FEATURES.md](FEATURES.md) under its area:
  `| <SPEC-ID> | A1, A2 | test:<crate>:<test name> ; example:<crate>:<example name> |`. Test names are
  exactly as printed by `cargo test -p <crate> -- --list` (without the `: test` suffix). Verifiers are
  tests or examples, and `./scripts/check-features` must pass.
- `./scripts/land`, run in the worktree, rebases the branch onto `main`, queues it in a merge train,
  runs the gates once on the composed candidate (`cargo fmt --all --check`, clippy with
  `-D warnings`, `cargo test --workspace`, `./scripts/check-features --no-run-examples`) and
  fast-forwards `main`; a failing branch is isolated by bisection and the rest land. It expects the
  main checkout at `~/Projects/vespertine` unless `VSP_LAND_MAIN` says otherwise. `./scripts/land
  --dry-run` does everything except moving `main`, and `./scripts/test-land` tests the script.
- Optional: set `RUSTC_WRAPPER` to `sccache` to share a build cache between worktrees.
- Changes that alter a documented behavior update this README, the crate READMEs, UPSTREAM.md,
  NOTICE and CHANGELOG.md as applicable. See [CHANGELOG.md](CHANGELOG.md).
