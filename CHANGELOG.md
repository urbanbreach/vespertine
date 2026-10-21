# Changelog

Entries are grouped by release, newest first, and summarize what changed by area. Specification
identifiers (for example `RND-003`) are the rows of [FEATURES.md](FEATURES.md), which lists the tests
and examples that verify each one.

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
