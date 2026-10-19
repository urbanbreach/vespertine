//! The embedded browser surface (CMP-022).
//!
//! [`BrowserSurface`] is ordinary framework chrome (back, forward and reload buttons, an address
//! input, an open-in-default-browser button, validation and placeholder states) around a page
//! area that shows an embedded web page rendered by the helper process of `vespertine_webview`.
//! It owns address normalization and the scheme policy ([`model`]), loading and error
//! presentation, page-provided menus, presentation states, per-profile website-data selection
//! (through the [`BrowserContext`] it is created with) and the lifecycle rules that make
//! asynchronous engine startup safe. Colours come from a [`BrowserTheme`]; behaviour, states and
//! interaction are specified, not an application look.
//!
//! Register the key bindings once with [`bind_keys`] (or [`bind_shortcuts`] for embedder chords),
//! create a surface with [`BrowserSurface::new`], set its presentation (a surface is hidden until
//! the embedder says otherwise; see [`Presentation::for_host`]) and navigate it.

pub mod address_input;
pub mod menu;
pub mod model;
pub mod shortcuts;
pub mod surface;
pub mod theme;
mod widgets;

pub use address_input::{AddressEvent, AddressInput};
pub use menu::{MenuCommand, MenuState, command_for_action};
pub use model::{
    allowed_frame_navigation, allowed_navigation, loopback, normalize_address,
    normalize_explicit_address,
};
pub use shortcuts::{
    Back, BrowserShortcuts, CloseTab, FocusAddress, Forward, KEY_CONTEXT, NewTab, Reload,
    bind_keys, bind_shortcuts,
};
pub use surface::{
    ADDRESS_PLACEHOLDER, BodyKind, BrowserEvent, BrowserSurface, ExternalOpener, Placeholder,
    PlaceholderAction,
};
pub use theme::{BrowserMetrics, BrowserTheme};
pub use vespertine_webview::{BrowserContext, BrowserProfile, PageState, Presentation};
