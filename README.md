# Vespertine

Vespertine is a standalone fork of GPUI, Zed's GPU-accelerated Rust UI framework. Linux, with both Wayland and X11 backends, is the verified platform; macOS and Windows backends are preserved from upstream and are cross-target checked where the host toolchain permits. iOS and browser/WebAssembly support are excluded. The public framework API follows upstream GPUI under the `vespertine` crate namespace. It is derived from [zed-industries/zed](https://github.com/zed-industries/zed), copyright Zed Industries, Inc., and distributed under Apache-2.0; see [NOTICE](NOTICE), [LICENSE-APACHE](LICENSE-APACHE), and [UPSTREAM.md](UPSTREAM.md) for provenance and synchronization details.

## Build and run

Use the pinned Rust toolchain and Linux development libraries required by upstream GPUI (including fontconfig, FreeType, XKB, Wayland, and X11), plus a working Vulkan graphics driver.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --all-targets --release
cargo run --release --example hello_world
# Force X11 rather than Wayland, using your desktop's DISPLAY value:
env -u WAYLAND_DISPLAY cargo run --release --example hello_world
# The agent workbench, built only from Vespertine components; the scripted run drives the
# window for 60 seconds, asserts its invariants and exits 0 on success:
cargo run --release --example workbench
cargo run --release --example workbench -- --scripted 60
```

## Cross-target checks

```sh
rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-msvc
# Runtime shaders avoid requiring build-time Metal compilation on the host:
cargo check --workspace --target aarch64-apple-darwin --features vespertine_platform/runtime_shaders
```

Cross-target checks do not replace native runtime tests. Building native platform
bindings can require Windows C tools or an Apple SDK that is not present on Linux;
such toolchain failures are reported rather than bypassed.
