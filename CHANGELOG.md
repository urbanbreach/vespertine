# Changelog

Entries are grouped by release, newest first, and summarize what changed by area. Specification
identifiers (for example `RND-003`) are the rows of [FEATURES.md](FEATURES.md), which lists the tests
and examples that verify each one.

## Unreleased

### Changed

- `backdrop_blur(sigma)` is a style (RND-010): an element blurs what was painted behind it,
  inside its bounds and corner radii, before its own shadows, background, children and border, so
  a frosted panel no longer needs a hand-placed `canvas`. The `backdrop_blur` example uses it.
- Backdrop blur follows opacity (RND-003, RND-010): a blur painted under an element opacity blends
  linearly between the sharp and the blurred backdrop, with no blur at opacity 0 and unchanged
  output at opacity 1. `BackdropBlur` scene records carry the opacity.
- Letter spacing (TXT-010): the inherited text style property `letter_spacing`
  (`Styled::letter_spacing`) takes px, rems or a fraction of the font size; negative values
  tighten. It is added after every grapheme cluster except the last one of a line, and widths,
  wrapping (`LineWrapper::wrap_line_with_runs`), truncation, hit testing, carets, selection and
  painted glyphs follow it, in plain text and in `vespertine_richtext` (so in Markdown documents
  too). `TextStyle::to_run` takes the rem size, `TextRun` has a `letter_spacing` field and
  `LineWrapper::should_truncate_line` takes the runs.
- Variable fonts render at the requested weight on Linux (TXT-011): a weight selects that point of
  a variable face's `wght` axis, clamped to its range, for shaping, metrics and rasterization;
  before, every weight rendered the default instance. Static faces are unchanged; `ital` and
  `slnt` axes are not applied. The test face `assets/fonts/test-sans-var/` is an ASCII subset of
  IBM Plex Sans Var, renamed "Vespertine Test Sans Var" as its license requires of modified
  versions.
- The appended-text and numeric shaping shortcuts are exact by construction (PRF-013, PRF-017):
  they run only where the backend guarantees the result equals full shaping, and their sampled
  validation, tolerances and disabled-size bookkeeping are gone. `PlatformTextSystem` gains
  `shapes_ascii_words_independently` and `shapes_pairwise`, both `false` by default; the Linux
  backend answers them from cosmic-text's per-segment shaping and from each font's own tables
  (numbers in fonts with code ligatures or a legacy `kern` table keep full shaping). Streaming
  keeps its speed. Median UI-thread CPU per frame against 0.2.0-alpha.1, interleaved runs on the
  same machine in its power-saver profile: PERF-STREAM 3220 → 2977 µs and PERF-QUADS 6598 → 6667
  µs (15 runs each); PERF-TEXT 3473 → 3464, PERF-LIST 1991 → 1996, PERF-FLEX 12208 → 12198 and
  PERF-IDLE 2122 → 2151 µs (7 runs each), all within the run-to-run spread.
- `MarkdownOptions::row_key` is an `Option`; without one, every document gets a key of its own
  (CMP-011). `LinkOptions::navigation` replaces `navigation_allowed`, and `LinkActivation` carries
  the resolved `navigation` address (TXT-008).
- Dismissal layers belong to their owner (CMP-002): `LayerHandle` unregisters its layer when its
  last clone drops, and `LayerRegistration`, `WeakLayerHandle`, `ESCAPE_OWNER_CONTEXT` and
  `TabChip::track_close_focus` are new. `Window::focused_input_is_composing` reports marked text in
  the focused input.
- The picker's Retry is a focusable button, and the rebindable `Retry` action (Ctrl-R) presses the
  visible one (CMP-006).
- `HostElement` is generic over the element it wraps, and `HostNode::wrap` attaches a host node's
  subscriptions and geometry reports to any leaf element (ELM-004). The test-support
  `HeadlessAppContext` exposes `opened_urls()`.
- The `svg()` docs say an SVG renders as a mask tinted by the text color and point to `img()` for
  SVGs that keep their colors.

### Fixed

- Retained frames (PRF-001, PLT-002): a global change rebuilds the views that read it in every
  window, not only in the first one drawn. Views nested in a group keep group hover and active
  styling, and controls in a disabled group stay disabled, when their own view is rebuilt alone;
  views nested in an unchanged disabled group are replayed instead of rebuilt each time the group
  is built. Present callbacks of later views are no longer lost or duplicated when a spliced view
  changes how many it registers. Changing the text rendering mode redraws every window.
- Fonts registered after text is on screen are used by that text without its string changing:
  carried text measurements and retained replay follow the font generation (TXT-003).
  `TestHost::register_font` registers with the text system its window renders with (DEV-006), and
  so do fonts registered through `TestAppContext`, `VisualTestContext`, `TestApp` and
  `VisualTestAppContext`.
- Scroll layers (PRF-004): cached tiles show images updated in place and a changed scroller
  background, and a layer whose owner is nested in a replayed view shows the current offset when
  it is wheeled past its recorded region, instead of the previous frame.
- Retained list rows (PRF-005) rebuild when their owner is notified after a programmatic scroll
  (`set_offset`, `scroll_by`, `scroll_to`, ...), so data changed in the same frame shows instead
  of stale rows; only offsets set through a scrollbar setter count as scrolling (new
  `UniformListScrollHandle::set_offset_from_scrollbar`), and a wheel step is no longer counted
  twice. A programmatic scroll with a notification now rebuilds the visible rows: in `vsp-bench`
  (release, mean UI-thread CPU per frame, retention on) `list-uniform-scroll` went from 0.31 to
  0.48 ms and `list-uniform-live` from 0.31 to 0.62 ms, the latter having shown stale rows before.
- Two layout styles that differ only in their last enum fields no longer share a fingerprint, so
  a changed style is always applied in release builds (LAY-001). Clearing a trailing viewport
  reservation after walking up a row-retained list leaves no extra scroll extent, and a list whose
  viewport is no taller than its padding keeps its last row's reservation (LAY-002). A padded
  scroll container no longer scrolls by its padding when it holds a full-size absolutely
  positioned child or no children: absolutely positioned children count only as far as they reach
  past the padding box.
- Panes (LAY-004): closing panes take no keyboard input, focus or accessibility actions, and a
  closing pane that holds keyboard focus hands it to the enclosing focusable element (or clears
  it); enabling reduced motion settles running slot transitions at once; a route glide that starts
  after an idle period starts from rest instead of jumping. A pane focus restore no longer takes
  focus from a dialog focused before the next frame (INP-007).
- Floating positions, gaps, offsets and snap margins too large for `f32` pixels fall back to the
  defaults instead of producing NaN placement (LAY-005). Changing the motion preference while the
  system asks for reduced motion switches activity indicators at once (ANM-004). Restoring a window
  whose minimum size exceeds the work area keeps its titlebar on screen (PLT-008).
- Dark file-icon palette remapping no longer panics on SVG text with non-ASCII characters.
- Accessibility and input: retained host nodes with a click listener expose an accessible Click
  action (INP-006); a queued screen-reader focus request no longer focuses an element that became
  inert; a keystroke consumed by an interceptor, such as the Escape that dismisses a floating layer
  or a key captured by the shortcut recorder, no longer reaches a key listener (INP-007);
  action-availability queries leave out handlers above a modal scope, so menus and command
  palettes no longer offer blocked actions (CMP-003); host `img` and `svg` nodes keep their role,
  `aria-*` properties, hover and active styles and listeners, and apply the `motion` property like
  any host element (ELM-004, ANM-002).
- Linux: pending timers share one deadline queue instead of holding a file descriptor each, so
  many long timers can no longer exhaust descriptors. On X11, text a legacy owner answers as
  Latin-1 `STRING` pastes again (PLT-009), and windows announce XDND drop support with the
  predefined `ATOM` property type, so drag sources that check it see them as drop targets. Text
  draws each glyph at its shaped offset, so combining marks sit on their base letter, and the
  up/down caret lookup splits a marked cluster at its advance midpoint. `vespertine_linux`, and so
  `cargo test -p vespertine_test_host`, builds without the `wayland` and `x11` features.
- Composition surfaces (PLT-001): dropping or removing a surface also removes surfaces only its
  handlers owned; surfaces with only a wheel handler receive scroll events; a second button
  pressed during a press goes to the surface that owns the press; files dropped on an overlay
  surface reach the owning window (in window coordinates on Wayland, through the overlay window on
  X11).
- Markdown (CMP-011, CMP-013): messages under one text root keep their own selectable text, so
  selection and copy work across a transcript, also when two views show the same document; a
  document that stops streaming shows its canonical text instead of the auto-closed repair; task
  markers of loose list items render; while streaming, an unfinished code fence inside a quote or
  list gains no repair characters and reference links render resolved; nested Mermaid fences get
  distinct frame ids.
- Text editing: a composition that completes a file mention keeps the typed prefix (CMP-010); word
  movement keeps a full stop with its combining marks, ending a composition leaves the caret on a
  grapheme boundary, and an edit after an interrupted composition is its own undo step (CMP-007);
  Enter during a composition in the picker query no longer activates a row, and paging resumes
  after an indexing refresh, for lists shorter than the viewport and after an empty page that
  still reports more results (CMP-006); disabling an open select closes its menu and commits
  nothing (`Select::set_disabled` takes the window), and option tooltips show (CMP-005); assistive
  technology can increment, decrement and set a slider (CMP-001).
- Truncation measures the line it paints (TXT-010): cuts fall only between grapheme clusters, the
  ellipsis takes the letter spacing of the run where the trimmed text ends, and the last kept
  cluster is spaced before the ellipsis even at a line break.
- Floating and chrome: releasing an open dialog, menu or focus tooltip without closing it no
  longer leaves a layer that swallows every later Escape, and Escape stays with an input that is
  composing and with a review draft inside a dialog (CMP-002, CMP-003, CMP-028); a hover card
  hides on Escape before the layer below it (CMP-004); the popup menu trigger is a tab stop
  (CMP-002); keyboard users can close and add tabs, and closing one moves focus to a surviving tab
  (CMP-017); a toast swipe that leaves the card still dismisses it (CMP-018); Enter or Space
  activates a status action once, badge actions are keyboard-operable, and focus rings no longer
  change a button's size (CMP-023, CMP-028).
- Diffs, code and trees: git-quoted target paths show unquoted and match their documents; a new
  patch or a wider viewer no longer leaves code panned out of view; a click on the old side of a
  split row reports the old line; a reversed file fold turns around at its current height
  (CMP-015). Unchanged code blocks keep their highlight instead of looking it up every frame
  (CMP-014). Revealing a tree entry hidden by the ignored filter reports it missing (CMP-016).
- Rich text: links open the address their policy resolved, so relative file links open under
  their root; releasing on a neighboring link activates neither; an Escape-dismissed disclosure
  stays dismissed while other links are visited (TXT-008). Explicit ranges and query matches are
  never both active, also across views that share one `Numbering`, whose explicit ranges now
  number after all their query matches in document order (`Numbering::join` and `SpecCount` are
  new, and `retained_ordinal` loses its `native_matches` parameter) (TXT-009). A cached highlight
  is never served past a stricter span limit (TXT-007).
- Duplicating a theme as editable never overwrites the file of a removed copy (THM-004).

## 0.2.0-alpha.1 - 2026-10-22

The first public release, an early alpha: the API changes between releases without deprecation.
Releases are tagged on GitHub and used as git dependencies (see "Using Vespertine" in the README);
nothing is published to crates.io or npm. This release removes duplicate implementations, unused API,
tests that could not fail and the tooling of the autonomous build, and the crates build in more places.

### Fixed

- The macOS and Windows backends compile again (a Linux-only `bail` import in `vespertine`, a renamed
  `util` import in `vespertine_windows`), and both type-check from Linux: the `vespertine_apple` build
  script asks Cargo for the target OS. `vespertine_desktop` and `vespertine_wgpu` build for Windows and
  macOS, and so does `vespertine_components`.
- Projects that depend on Vespertine by path or git build: taffy is a path dependency instead of a
  `[patch.crates-io]` entry (which Cargo applies only in the top-level workspace), and the vendored
  crate is its own workspace root, so checkouts nested in another workspace can run `cargo fmt --all`.
  The `diagram` feature of `vespertine_components` pins `merman-core` and `merman-render` to the
  `0.8.0-alpha.5` that `merman` needs; without Vespertine's lockfile, Cargo chose their incompatible
  0.8.0 releases and `merman` failed to compile.
- Numeric text shaping (PRF-013) keeps the learned tables of at most 256 font sizes (about 1 MB) and
  drops the least recently used. They grew with every distinct font size until fonts changed, so a
  smooth zoom over numbers added one per frame. A size whose check against full shaping failed stays
  on platform shaping whichever tables are dropped.

### Changed

- Every crate and JavaScript package carries the release version, `0.2.0-alpha.1`. The crates
  inherit it from the workspace (`version.workspace = true`); `vespertine` was `0.2.2`, the version
  of upstream GPUI.
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
