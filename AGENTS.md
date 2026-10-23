# Repository Guidelines

## Project Overview

Vespertine is a retained-mode GPU UI framework for native desktop apps. It is derived from Zed's GPUI
(pinned upstream commit in `UPSTREAM.md`) and adds components, theming, a retained host tree, and
React/Solid bindings through a Node-API addon. The Linux backends (Wayland and X11, rendered with wgpu
on Vulkan) are verified. The macOS and Windows backends are preserved from upstream and only
type-checked. iOS and WebAssembly are not supported.

- The project is an early alpha, so APIs may break without deprecation.
- Releases are git tags only (e.g. `v0.2.0-alpha.3`), with nothing published to crates.io or npm. Downstream apps (e.g. `../agent-harness`) depend on every crate at the **same tag**.
- Read `README.md` before structural work, and `crates/vespertine/docs/contexts.md` before touching entity or context code.

## Architecture & Data Flow

```
Application ─▶ App (Rc state, owns all entities) ─▶ Window (root view, frame state)
  Render::render ─▶ element tree ─▶ request_layout (Taffy) ─▶ prepaint (bounds, hitboxes) ─▶ paint ─▶ Scene
  Scene ─▶ PlatformWindow ─▶ vespertine_wgpu renderer ─▶ GPU
JS (React/Solid) ─▶ mutation batch ─▶ vespertine_js_node (N-API) ─▶ vespertine_js ─▶ HostTree ─▶ HostView ─▶ ordinary elements
```

- **State:** `App` owns all state, and `Entity<T>` is a handle to it. Read and write go through `entity.read(cx)` / `entity.update(cx, …)`, and `cx.notify()` invalidates observers and views. Never hold an `Rc<T>` to entity state. See `crates/vespertine/src/_ownership_and_data_flow.rs` and `docs/contexts.md`.
- **Element lifecycle:** the phases run in a fixed order, `request_layout → prepaint → paint`, and `Drawable` panics when they run out of order (`element.rs`). The Taffy adapter is in `taffy.rs`, and backend-neutral scene records are in `scene.rs`. Element IDs must be unique among the children of the nearest identified ancestor.
- **Async:** futures from `cx.spawn(async move |this, cx| …)` run on the main thread. A `Context<T>` spawn receives a `WeakEntity<T>`, so it does not keep the entity alive. Send CPU-heavy work to `cx.background_executor()`. `LocalExecutor` (`vespertine_scheduler`) is `!Send`. `ApplicationHandle::update` is non-reentrant because it borrows a `RefCell`.
- **Platform:** the `vespertine::Platform` trait is in `platform.rs`. `vespertine_platform::current_platform(headless)` selects the backend at compile time. Backend methods return `anyhow::Result`, and the defaults may return unsupported-operation errors. When you extend a cross-platform trait, also edit `vespertine_macos` and `vespertine_windows` and type-check them (see Development Commands).
- **JS host:** `HostTree::apply_batch` decodes and resolves the whole batch before it commits. A batch that fails leaves the tree untouched, and resending an identical batch must not bump the revision or trigger a redraw. The tree is shared as `Arc<Mutex<HostTree>>`, and code recovers from a poisoned lock. In automation mode stdout is reserved for the protocol, so never print there.

## Key Directories

| Path | Purpose |
| --- | --- |
| `crates/vespertine` | Core: app, window, elements, layout, scene, input, a11y, executors, test support |
| `crates/vespertine_platform`, `vespertine_linux` | Backend selection; Linux Wayland/X11/headless backend |
| `crates/vespertine_macos`, `vespertine_apple`, `vespertine_windows` | Upstream backends; unverified, type-check only |
| `crates/vespertine_wgpu` | Renderer, GPU context, atlas, headless renderer |
| `crates/vespertine_components` | Editors, markdown, diagrams (`diagram`), math (`math`), browser (`browser`, Linux only) |
| `crates/vespertine_theme`, `vespertine_richtext` | Theme models and import; rich text |
| `crates/vespertine_host_tree`, `vespertine_js`, `vespertine_js_node` | Retained JS tree, host renderer, N-API addon |
| `crates/vespertine_webview` | Linux WebKitGTK helper process, compositing |
| `crates/vespertine_test_host` | Offscreen test host and automation fixtures |
| `crates/vespertine_scenes`, `vespertine_bench`, `vespertine_perf` | `FID-*`/`PERF-*` scenes, `vsp-bench`, perf tooling |
| `crates/vespertine_desktop`, `vespertine_updater` | Portals/D-Bus/X11 helpers; signed AppImage updates |
| `packages/` | npm workspace: `vespertine-host`, `vespertine-react`, `vespertine-solid`, `vespertine-dev`, `vespertine-examples`, `starter`, `create-vespertine` |
| `third_party/taffy` | Vendored taffy (MIT); never reformat it (`disable_all_formatting`) |
| `scripts/third-party` | License audit; generates `THIRD_PARTY.md` |

Local-only directories such as `.worktrees/` are excluded via `.git/info/exclude`, so don't commit
anything from them or depend on them.

## Development Commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p vespertine_test_host --test host <test_name>    # one integration test
cargo test -p <crate> -- --list                               # exact names for FEATURES.md rows
cargo build --workspace --all-targets --release
# After changing a cross-platform trait or shared API (needs `rustup target add` once):
cargo check -p vespertine_windows --lib --no-default-features --target x86_64-pc-windows-msvc
cargo check -p vespertine_macos --lib --features font-kit,runtime_shaders --target aarch64-apple-darwin
./scripts/third-party            # regenerate THIRD_PARTY.md after Cargo.lock / package-lock.json changes
./scripts/third-party --check    # license policy gate (not part of cargo test)

cargo run --release --example hello_world
env -u WAYLAND_DISPLAY cargo run --release --example hello_world   # force X11
cargo run --release --example workbench -- --scripted 60           # self-asserting run, exits 0
cargo run --release -p vespertine_bench --bin vsp-bench -- --list
cargo run --release -p vespertine_scenes --bin vsp-scene-runner -- --scene PERF-LIST --mode perf --out target/perf-list.json

# JS (from packages/; run npm install once)
cd packages/vespertine-host && node scripts/build-native.mjs --release --test-host && npm test
cd ../vespertine-react && npm test
cargo test -p vespertine_js_node   # builds the test addon and runs the package suites under Node and Bun
```

## Code Conventions & Common Patterns

- **Format:** use rustfmt with edition and style edition 2024. Never reformat vendored or upstream-mirrored code you aren't changing.
- **Clippy denies** `dbg!`, `todo!`, `redundant_clone`, `declare_interior_mutable_const` and the methods in `clippy.toml`:
  - `std::process::Command::{spawn,output,status,stdin,stdout,stderr}` → use `smol::process::Command`.
  - `smol::Timer::after` → use `BackgroundExecutor::timer`, which is deterministic in tests.
  - `serde_json::from_reader` → use `from_slice`.
- **Errors:** `vespertine` re-exports `anyhow::Result`, and platform and app code use it. Libraries with typed failures (host tree, updater, components) use `thiserror`. Keep `SAFETY:` comments when you touch `unsafe`. The `Scene` clear path relies on `set_len(0)` with non-dropping records, so preserve that invariant.
- **Docs:** the core crate has `#![warn(missing_docs)]`, so every new public item needs a doc comment. Long-form guides are `_`-prefixed doc modules (`_accessibility.rs`, `_retained_frames.rs`).
- **Naming:**
  - Crates are `vespertine_<area>`, and a crate's root file is usually `src/<crate_name>.rs`, not `lib.rs`.
  - Env vars and binaries use the `VSP_*` / `vsp-*` prefixes.
  - Upstream names such as `GpuiMode` and `GpuiBorrow` remain. Don't rename them wholesale.
  - Tests use descriptive snake_case.
- **Features change the code that exists:**
  - Core: `test-support`, `bench-support`, `wayland`, `x11`, `font-kit`.
  - Components: `browser`, `diagram`, `math`, all on by default. `browser` pulls in WebKitGTK.
  - `vespertine_js_node`: `offscreen`, which must never be in production addons.
  - Gate new code behind the narrowest relevant feature, and test with the feature both on and off when you touch a gate.
- **Upstream discipline:**
  - Before you change GPUI-derived code, read the original in an independent Zed checkout at the pinned commit (`UPSTREAM.md` shows the `git diff --no-index` recipe).
  - Prefer small, local deviations, and record intentional ones in `UPSTREAM.md` → *Intentional deviations*.
  - Never merge Zed's workspace wholesale.
- **No workarounds over core invariants:**
  - Fix the framework at the layer that owns the behavior.
  - Never re-apply state after the framework has computed it, and never patch values owned by layout, the scene or an entity.
  - If such a change is unavoidable, add a comment that says what the core does, why that doesn't fit, and which call makes the change safe.
- **Commits:** write one imperative, sentence-case subject that describes the behavior. Don't use a conventional-commit prefix or a trailing period (e.g. `Gate the browser, diagram and math modules behind default-on cargo features`). Each commit should cover one logical change.

## Important Files

- `crates/vespertine/src/vespertine.rs`: core module root and public exports.
- `crates/vespertine/src/{app.rs,app/context.rs,window.rs,element.rs,taffy.rs,scene.rs,platform.rs}`: the core model and pipeline.
- `crates/vespertine_platform/src/vespertine_platform.rs`: backend selection.
- `crates/vespertine_host_tree/src/tree.rs`, `crates/vespertine_js/src/{runtime.rs,view.rs}`, `crates/vespertine_js_node/src/lib.rs`: the JS bridge.
- `packages/vespertine-host/scripts/build-native.mjs`: builds the addon and installs it to `native/vespertine_js.node`. That file is generated, so never edit it.
- `Cargo.toml` (workspace deps, lints, profiles `dev`/`dbg`/`release`/`release-fast`), `clippy.toml`, `rust-toolchain.toml`.
- `FEATURES.md` (spec catalog), `CHANGELOG.md`, `UPSTREAM.md`, `THIRD_PARTY.md` (generated), `NOTICE`.

## Runtime/Tooling Preferences

- **Rust:** use the toolchain pinned in `rust-toolchain.toml` (1.98.1), edition 2024, resolver 2. Consumers may build with a newer toolchain.
- **Versioning:** the version lives once in `[workspace.package]`, and crates inherit it. JS `package.json` files repeat it, so bump both together.
- **System dependencies:** `libxcb`, `libxkbcommon`, `libxkbcommon-x11`, a Vulkan driver, and, for `browser`, `pkg-config` with `webkit2gtk-4.1` and `json-glib-1.0`. `VSP_WEBKIT_HELPER_BINARY` skips compiling the helper.
- **JS:** one npm workspace in `packages/` with a single lockfile (`packages/package-lock.json`). Use npm for workspace installs and Node ≥ 20. Bun runs the starter (`bun src/main.tsx`) and the second test pass. Never add a second lockfile.
- **Process spawning and timers:** go through `smol` and the executor (see Clippy above), never through blocking std APIs.

## Testing & QA

- **Rust:**
  - Use standard `#[test]`, with unit tests in-source and integration tests in `crates/*/tests/*.rs`.
  - `TestAppContext` runs on a seedable `TestDispatcher` with a fake HTTP client, so tests stay deterministic. Never use wall-clock sleeps.
  - Visual tests use `VisualTestContext` / `TestHost` (`vespertine_test_host`, feature `offscreen`) and assert pixels directly. There are no golden-image files, so don't read reference images from outside the repo.
- **Tests must assert real behavior.** Don't write circular tests, such as comparing a parser with itself or keeping a Rust mirror of shader formulas just to test it. Recent history deleted tests like these.
- **JS:** packages use `node --test test/*.test.js` (Solid adds `--conditions=browser`). They need an addon built with `--test-host`.
- **Performance:** `vsp-bench --verify` checks that the retained output matches a full render. For perf-sensitive changes (shaping, layout, scene), measure with `vsp-bench` or a `PERF-*` scene before and after, and record the numbers in `CHANGELOG.md`. There's no automated perf gate.
- **FEATURES.md:**
  - Every implemented capability has a row: `| <SPEC-ID> | A1, A2 | test:<crate>:<exact test name> ; example:<crate>:<example> |`.
  - Rows are in ascending spec order, and acceptance items without a verifier are left out.
  - Nothing runs these rows, so run the verifiers for the area you changed yourself.
  - Repoint or remove a row whenever you rename or delete its test.
- **No CI or coverage target.** Before you hand off, run the gates under Development Commands: fmt, clippy `-D warnings`, the relevant `cargo test -p`, and the license check if lockfiles changed.

## Documentation Obligations

When a change alters documented behavior, update the affected docs in the same change:

- `CHANGELOG.md`:
  - Releases are listed newest first, with headings `## <version> - <date>`.
  - Put new entries under a `## Unreleased` heading at the top.
  - Group entries under `### Changed` / `### Fixed` / `### Removed`, and name the spec IDs.
- `FEATURES.md`: add or adjust rows (see Testing & QA).
- `README.md` and the crate READMEs (`vespertine`, `vespertine_bench`, `vespertine_scenes`, `vespertine_wgpu`, `vespertine_zlog`): update the commands, features and examples they document.
- `UPSTREAM.md`: update for mapping changes, new non-upstream crates, deviations and sync points.
- `NOTICE` and `THIRD_PARTY.md`: update for new vendored code, fonts or data, or new dependencies. Rerun `scripts/third-party`, which rejects GPL, AGPL and LGPL.

## Working With Downstream Consumers

Fix requests often come from apps built on Vespertine, such as `../agent-harness`:

- Work in a worktree: `git worktree add .worktrees/<name> -b work/<name> main`.
- Implement the capability **generically**. Never add consumer-specific APIs, names or workarounds.
- Each fix ships with a Vespertine test or example, a FEATURES.md row and a CHANGELOG entry, exactly like any other change.
- Don't push or tag without the maintainer's approval. Consumers pick the fix up through a local `[patch."https://github.com/urbanbreach/vespertine"]` override during development, then through a new release tag.
