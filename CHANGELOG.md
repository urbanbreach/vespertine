# Changelog

Entries are grouped by release, newest first, and summarize what changed by area. Specification
identifiers (for example `RND-003`) are the rows of [FEATURES.md](FEATURES.md), which lists the tests
and examples that verify each one.

## Unreleased

A cleanup release: duplicate implementations, unused API, tests that could not fail and the tooling
of the autonomous build are removed, and the crates build in more places.

### Fixed

- The macOS and Windows backends compile again (a Linux-only `bail` import in `vespertine`, a renamed
  `util` import in `vespertine_windows`), and both type-check from Linux: the `vespertine_apple` build
  script asks Cargo for the target OS. `vespertine_desktop` and `vespertine_wgpu` build for Windows and
  macOS, and so does `vespertine_components`.
- Projects that depend on Vespertine by path or git build: taffy is a path dependency instead of a
  `[patch.crates-io]` entry (which Cargo applies only in the top-level workspace), and the vendored
  crate is its own workspace root, so checkouts nested in another workspace can run `cargo fmt --all`.
- Text layout no longer assembles lines from cached digit pairs or a cached prefix (glyphs could land
  up to 0.5 px from the platform's positions, and per-size tables were never freed); every cache miss
  is shaped by the platform. PRF-013 is withdrawn. Measured cost: the PERF-STREAM scene (a streaming
  transcript) takes about 16% more CPU per frame; PERF-TEXT is unchanged.

### Changed

- `vespertine_components` has default-on features `browser` (Linux only; needs `vespertine_webview`),
  `diagram` (Mermaid through `merman`) and `math` (4.2 MB of math faces). Without them, Mermaid fences
  and formulas render as source.
- The browser address bar is a chromeless `TextField`: `BrowserSurface::address()` returns
  `Entity<TextField>`, and pasted line breaks become spaces.
- The Node addon contains the offscreen test renderer (`TestRenderer`, `testHostAvailable`) and the
  framework's test support only when built with `scripts/build-native.mjs --test-host` (cargo feature
  `offscreen` of `vespertine_js_node`); live automation needs only the new `paint-observer` feature of
  `vespertine`. `vespertine_test_host` gains an `offscreen` feature (default on).
- `scripts/third-party --check` is an explicit gate instead of part of `cargo test`.
- `vespertine_theme` has one `Appearance` enum (`theme::Appearance`) and one variant re-key helper
  (`library::rekeyed_variant_id`), and decodes hex colors with the core CSS parser.
- Test fixtures left the public API: `vespertine_desktop::context::fake`, the glyph-blur reference
  helpers of `vespertine` (now `test-support` only), `streaming::SampleService` of
  `vespertine_components`, and the `instrument`, `reference` and `workload` modules of
  `vespertine_host_tree`.
- The `data_table` example of `vespertine_components` is now `commit_table`.

### Removed

- The vendored `polling` crate; the workspace uses the published 3.11.0.
- `vespertine_residency` and its PERF-PRF-017 scene; PRF-017 keeps A3 (fade spans never reshape).
- Second implementations: the core link element (`vespertine_richtext` keeps the link layer; target
  classification stays in `vespertine::link`), the core tree drag-and-drop policies and `DragSession`
  (the components tree has its own), `KeyboardActivation` (buttons use `ActivationLatch`), the
  comparison diff viewer (`diff::compare`, `diff::jsdiff`, FID-CMP-015-2; `DiffView` remains),
  `vespertine_richtext::CodeView` (use `CodeBlock`), the badges checklist panel (use `StatusSection`),
  `CountReporter` and `OwnershipStack`, `code::ThumbMetrics` and `floating::Side` (use
  `vespertine::FloatingSide`).
- Unused API: the vertical tab-group engine, `ControlledState`, streaming viewport memory,
  `CodeBlock::row_after` and other uncalled builders and helpers in `vespertine`,
  `vespertine_components`, `vespertine_theme`, `vespertine_webview`, `vespertine_desktop` and
  `vespertine_test_host`.
- Tooling: `scripts/land`, `scripts/test-land` and `scripts/check-features` (FEATURES.md remains as
  documentation), the real-window mode and `retention_showcase` example of `vsp-bench`, and the scene
  runner's `profile` feature.
- Tests that could not fail or repeated others, and the fidelity tests that read reference images from
  outside the repository.

## 0.1.0 - 2026-10-20

Initial release: a fork of upstream GPUI (pinned in [UPSTREAM.md](UPSTREAM.md)) with 136 implemented
specifications in 13 areas. Linux (Wayland and X11) is verified; the macOS and Windows backends are
preserved from upstream and not built or verified here.

### Rendering (RND)

- Scoped edge fade and antialiased rounded content clipping applied per fragment.
- Ordered Gaussian backdrop blur compositing.
- Feathered inverse rounded-rectangle image masks, fitted image painting, composite-time
  transformed and blurred glyph and icon painting.
- Shared-device GPU texture surfaces on Linux; raw BGRA images with explicit nearest-neighbor
  painting; layered surface painting; custom vector and text canvas painting.

### Text (TXT)

- Punctuation-aware line wrapping, fallback fonts that keep the requested style, runtime font
  registration with resolution retry, font catalogs and independent typography roles, run-level glyph
  painting with translation-stable origins.
- `vespertine_richtext`: shaped rich text with source-aware geometry, bounded cached syntax coloration,
  document-wide selectable text with safe link activation, inherited search washes and explicit ranges.

### Layout and elements (LAY, ELM)

- Persistent incremental layout nodes (vendored taffy with a multi-entry measurement cache),
  layout-time trailing viewport reservation, composable responsive layout, collapsible resizable pane
  composition, trigger-relative floating layer positioning.
- Variable-height virtual lists (windowing, anchors, padding, estimates) and incremental scrolling
  from negative item offsets.
- Image fitting with element-space rounded corners, asynchronous images and media hosts,
  color-preserving vector icons, cached source-space image treatments, serializable layout and
  interactive styling, persistent custom-element adapters, layout-neutral outlines.

### Components (CMP)

`vespertine_components`: buttons, switches and sliders; floating surfaces, anchored menus, popovers
and dismissal layers; modal and nonmodal dialogs; tooltips and hover cards; single-value select;
editable selection, searchable paged pickers and command palettes; single-line text fields and
multiline text editing with native composition; source-preserving Markdown editing; inline mention
and attachment projections; rich Markdown documents; inline and display math; themed diagrams with
source fallback; selectable code blocks and code editors; unified and split diff views; virtual file
trees; closable reorderable tabs and segmented navigation; paused timed toast stacks; resizable
reorderable data tables and graphs; bounded image preview and zoomable viewer; terminal grid
surfaces; an embedded browser surface; loading placeholders, progress and collapsible status;
two-dimensional charts; retained three-dimensional image surfaces and asynchronous shader image
surfaces; audio activity visualizations; badges, notices and review cards; streaming bottom-follow
and bidirectional history; phase-aware elastic overscroll; classic and auto-hiding scrollbars. The
`workbench` example composes several of them into an agent workbench.

### Input (INP)

- Stable pointer capture and pointer modality, hover suppression during wheel scroll, robust
  touch-fling release velocity, subset-constrained tab traversal, paint-scoped window-root key
  listeners, declarative host events and focus routing, focus-scoped actions and shortcuts, typed
  drag and drop with autoscroll.

### Animation (ANM)

- Demand-gated display frame delivery, native declarative motion with retargeting and interruption,
  presence management with exit-animation completion, shared parked animation clocks and motion
  policy.

### Retained frames and performance (PRF)

- Dependency-driven retained frames, incremental scene ordering, compact interaction storage,
  adaptive tiled scroll-content reuse, incremental virtual-list row replay, translation-invariant
  tessellation cache, batched GPU uploads and render-pass reuse, adapter-aware solid-fill
  specialization, region-bounded Gaussian processing, bounded renderer resources with idle reclamation.
- Idle wakeups: a background timer costs no executor-thread wakeups (it is a kernel timer armed by
  its caller and completed on the timer thread, which wakes only when it fires), an event-loop
  wakeup for another source (a compositor ping on a parked window) skips the timerfd and notifier
  bookkeeping of the vendored `polling` crate, and the desktop HTTP client creates its runtime on
  its first request.
- Carried text measurements, bounded shaping and font lookup caches, validated numeric shaping,
  size-specific shaping resource retention, compact retained state with no-op commit suppression,
  in-place image atlas updates, incremental document and media residency budgets
  (`vespertine_residency`).

### Linux platform (PLT)

- Presentation-synchronized native updates, reentrant desktop appearance and titlebar updates,
  explicit toolkit scale on X11, X11 input-method handshake, portable text-input intent and
  system-occlusion access, client-side window chrome, window geometry lifecycle and close
  interception.
- Text, image, path and scoped rich clipboard; native file pickers and external destination opening;
  desktop notifications and optional feedback sounds; permission-aware capture and global shortcuts;
  bounded accessibility context and device permission status (`vespertine_desktop`).
- Embedded web view (`vespertine_webview`): WebKitGTK 4.1 in a separate helper process, with lifecycle
  and failure handling, messaging, pixel transfer, compositing with scene layering around native
  content, input routing, focus and text composition.

### Applications and JavaScript host (APP)

- Nonloading asset inspection and complete image eviction, reactive views and owned background work,
  action-backed application menus.
- Signed packaged-application updates (`vespertine_updater`).
- Native JavaScript application and window lifecycle (`vespertine_js`, `vespertine_js_node`), atomic
  retained host-tree mutations (`vespertine_host_tree`), shared declarative host lifecycle and
  runtime helpers, one-shot garbage collection after idle bursts (`@vespertine/host`).
- React and Solid retained-renderer adapters with reactive window, selection and search helpers
  (`@vespertine/react`, `@vespertine/solid`).
- Data-dense reference example surfaces and a synchronized multi-pane editing gesture example
  (`@vespertine/examples`).

### Theming (THM)

- Shared CSS color parsing into bounded sRGB, runtime-overridable component palettes and metrics,
  semantic themes with live appearance resolution, theme import validation and image-derived palette
  hints (`vespertine_theme`).

### Accessibility (A11Y)

- Declarative accessibility properties and leaf metadata; accessibility tree replay with retained
  frames.

### Development and testing (DEV)

- Independent tree data structures without mandatory tracing, opt-in renderer memory accounting,
  reproducible frame-cost and retained-output diagnostics (`vespertine_bench`), programmatic
  frame-overlay statistics, committed paint geometry observation.
- Production-path offscreen visual test host and unified application automation with strict locators
  (`vespertine_test_host`), mutation deserialization and retained-memory measurement, project
  creation and native development workflows (`create-vespertine`, `@vespertine/dev`), runtime error
  display with remount recovery, embedded launch on the test platform.
- Scenes and the scene runner for fidelity and performance measurement (`vespertine_scenes`).

### Tooling and licensing

- `scripts/check-features` verifies the FEATURES.md rows; `scripts/land` lands branches through a
  merge train with the fmt, clippy, test and feature gates; `scripts/third-party` audits dependency
  licenses and generates THIRD_PARTY.md.
- Apache-2.0 throughout; bundled fonts and the vendored taffy keep their own licenses (NOTICE).

### Not included

- iOS and browser/WebAssembly support.
- Verification of the macOS and Windows backends.
