//! Reusable components built from elements.
//!
//! [`browser`] is the embedded browser surface of CMP-022: ordinary framework chrome (history and
//! reload buttons, an address input, an open-in-default-browser button, validation and
//! placeholder states) around the embedded web page of `vespertine_webview`.
//!
//! [`media`] holds the bounded image layer and the zoomable viewer of CMP-020.
//!
//! [`terminal`] is the interactive terminal grid surface of CMP-021: an `alacritty_terminal`
//! emulator, column-pinned painting, key and paste encoding, selection, tabs and a bottom dock.
//! [`floating`] is the foundation under every popup control (CMP-002): the popup lifecycle,
//! anchored and contained menu surfaces, the window-scoped dismissal-layer stack, trigger press
//! tracking, and the shared stepping, ranking and hover-intent rules.
//!
//! [`controls`] holds the activation controls of CMP-001: buttons, switches and sliders.
//!
//! [`tree`] is the interactive virtual file tree of CMP-016: a pure [`tree::TreeModel`] (lazy
//! loading, selection, cursor, anchor), path-attached scroll anchors, drag-and-drop destination
//! logic, inline rename, the hierarchical search-result tree and the [`tree::Tree`] view that
//! renders them with the virtual list.

pub mod audio;
pub mod browser;
pub mod chart;
pub mod controls;
pub mod dialog;
pub mod diff;
pub mod elastic;
pub mod floating;
pub mod math;
pub mod media;
pub mod picker;
pub mod scene3d;
pub mod scroll;
pub mod select;
pub mod shader;
pub mod status;
pub mod streaming;
pub mod terminal;
pub mod tokens;
pub mod tooltip;
pub mod tree;
