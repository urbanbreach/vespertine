//! Reusable components built from elements.
//!
//! [`browser`] is the embedded browser surface of CMP-022: ordinary framework chrome (history and
//! reload buttons, an address input, an open-in-default-browser button, validation and
//! placeholder states) around the embedded web page of `vespertine_webview`.
//!
//! [`media`] holds the bounded image layer and the zoomable viewer of CMP-020.
//! [`terminal`] is the interactive terminal grid surface of CMP-021: an `alacritty_terminal`
//! emulator, column-pinned painting, key and paste encoding, selection, tabs and a bottom dock.
//! [`floating`] is the foundation under every popup control (CMP-002): the popup lifecycle,
//! anchored and contained menu surfaces, the window-scoped dismissal-layer stack, trigger press
//! tracking, and the shared stepping, ranking and hover-intent rules.

pub mod audio;
pub mod browser;
pub mod chart;
pub mod diff;
pub mod elastic;
pub mod floating;
pub mod math;
pub mod media;
pub mod scroll;
pub mod status;
pub mod streaming;
pub mod terminal;
pub mod picker;
pub mod select;
pub mod tokens;
pub mod tooltip;
