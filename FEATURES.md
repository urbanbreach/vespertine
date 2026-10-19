# Features

Every implemented specification is listed here, under its area, with the acceptance items it
covers and what verifies them. `./scripts/check-features` parses these tables, checks that every
verifier exists, runs them, and fails if any row fails; it is part of the landing gates.

Row syntax (one spec per row, rows in ascending spec order):

```
| <SPEC-ID> | A1, A2, ... | test:<crate>:<test name> ; example:<crate>:<example name> |
```

- `test:<crate>:<name>` is a test exactly as printed by `cargo test -p <crate> -- --list`
  (without the `: test` suffix).
- `example:<crate>:<name>` is a cargo example of the crate. It is built in release mode and run
  for 10 seconds in the current graphical session; it passes if it exits 0 or is still running
  at the timeout.
- Verifiers are separated by ` ; `. A row lists every acceptance item (`A1`, `A2`, ...) of its spec.

Visual and performance scenes (`FID-*`, `PERF-*`) live in `crates/vespertine_scenes`; see its README.

## RND

GPU renderer, compositing, shaders, visual effects, color handling.

| Spec | Acceptance | Verified by |
|---|---|---|

## TXT

Fonts, shaping, rasterization, text layout and text rendering.

| Spec | Acceptance | Verified by |
|---|---|---|

## LAY

Layout engine and layout styles.

| Spec | Acceptance | Verified by |
|---|---|---|

## ELM

Elements and the styling API.

| Spec | Acceptance | Verified by |
|---|---|---|

## CMP

Reusable components and widgets built from elements.

| Spec | Acceptance | Verified by |
|---|---|---|

## INP

Keyboard, mouse, touchpad, IME, focus, actions, drag and drop.

| Spec | Acceptance | Verified by |
|---|---|---|

## PLT

Linux platform integration.

| Spec | Acceptance | Verified by |
|---|---|---|

## ANM

Animation, timing, frame scheduling.

| Spec | Acceptance | Verified by |
|---|---|---|

## THM

Theming, design tokens, color systems.

| Spec | Acceptance | Verified by |
|---|---|---|

## PRF

Performance: caching, batching, allocation, scheduling, threading.

| Spec | Acceptance | Verified by |
|---|---|---|

## APP

Application runtime: entities, contexts, executors, actions, menus.

| Spec | Acceptance | Verified by |
|---|---|---|

## A11Y

Accessibility.

| Spec | Acceptance | Verified by |
|---|---|---|

## DEV

Developer tooling: inspector, profiling, debugging, hot reload.

| Spec | Acceptance | Verified by |
|---|---|---|
