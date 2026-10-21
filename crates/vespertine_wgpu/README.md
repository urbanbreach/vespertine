# vespertine_wgpu

The Linux (Wayland and X11) GPU renderer, built on wgpu.

## Recording path (PRF-007)

A frame packs every instance array (quads, shadows, underlines, the three sprite kinds, path
vertices and path sprites) into one reusable instance buffer with one staging write. Sections start
at multiples of the device's storage-buffer offset alignment and are at least 16 bytes. Bind groups
are cached by the identity of the texture view or the `(buffer, offset, size)` range and released
after more than two unused frames. Pipeline and bind-group commands are suppressed when the tracked
state of the current render pass is unchanged; every pass restart resets the tracked state. The
three global uniform values are compared with the bytes last uploaded and written only on change.
Consecutive path batches whose bounds (expanded by 2 device pixels) do not overlap are rasterized
together, up to 64 batches per pass; the first group is rasterized before the main pass and later
groups in the middle of the frame.

## Backdrop blur (RND-003, PRF-009)

`Window::paint_backdrop_blur` inserts an invisible ordering primitive and a `BackdropBlur` scene
record. The renderer processes an effect immediately before the first batch whose first order is at
or above the effect's order: it copies the framebuffer region (the envelope: visible region plus
`ceil(3 sigma) + 2` device pixels, clamped to the drawable) into a retained snapshot, runs a
horizontal and a vertical Gaussian pass over scissored regions of two downsampled targets, and
replaces the pixels inside the rounded bounds with the result; below an effect opacity of 1 the
composite blends the result over the unblurred pixels by that opacity (a blend constant). Surfaces
that cannot be copied from (restricted OpenGL-backed surfaces) render effect frames through a
full-frame intermediate that is then presented with a non-blending copy; frames without effects
always present directly.

Blur resources: one retained set (snapshot plus two filter targets, quantized to 256 device pixels
and clamped to the drawable), at most 32 effects per frame, Gaussian weights computed once per
effect (table for radius up to 128, full-support fallback beyond).

## Bounded resources (PRF-010) on Linux

Implemented on Linux:

- R1 two frames of maximum presentation latency (unchanged from the base);
- R2 path resolve/multisample textures created only when a frame contains paths, never eagerly on
  resize or for surfaceless rendering;
- R3 path intermediates released after 30 consecutive path-free frames; the blur set and
  presentation intermediate released after 30 consecutive effect-free frames (separate counter);
- R4 instance buffer starts at 2 MiB and is halved after 120 consecutive frames using at most
  half of its capacity, never below 2 MiB;
- R5 idle trim: `PlatformWindow::trim_renderer_resources` is called when a window parks and
  releases path intermediates after a path-free last frame and blur storage after an effect-free
  last frame; it never touches the atlas or the instance buffer;
- R6 identical behavior for surfaceless rendering.

Not implemented on Linux (native macOS/Windows-only policies N1-N5): the drawable pool of two, the
multi-size 64-pixel-quantized blur-storage cache with byte budgets, the four-entry Gaussian-kernel
LRU cache keyed by sigma tolerance, and per-frame autorelease bounds.

## Memory accounting (DEV-002)

Set `VESPERTINE_RENDERER_MEMORY` to any value other than empty or `0` to print a one-line renderer
memory report to standard error after the first completed frame and then at most once per ten
seconds of rendering. No timers are scheduled.
