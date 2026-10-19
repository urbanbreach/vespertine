//! Embedded web view for Linux (PLT-014, PLT-015, PLT-016).
//!
//! The web engine runs in a separate helper process (WebKitGTK 4.1 on GTK 3, offscreen); this
//! crate never links GTK or WebKit. It provides the framed protocol ([`protocol`]), helper
//! discovery and extraction ([`extract`]), the helper process with its handshake, hang detection
//! and shutdown ([`helper`]), profiles ([`profile`]), contexts with the shared-helper registry and
//! asynchronous startup ([`context`]), pages with their state and latest frame ([`page`],
//! [`frames`], [`state`]).
//!
//! # Runtime requirements
//!
//! A prebuilt application needs the WebKitGTK 4.1 and json-glib runtime libraries: on
//! Debian/Ubuntu `libwebkit2gtk-4.1-0` and `libjson-glib-1.0-0` (development packages
//! `libwebkit2gtk-4.1-dev`, `libjson-glib-dev`); on Fedora `webkit2gtk4.1` and `json-glib`
//! (development: `webkit2gtk4.1-devel`, `json-glib-devel`). WebKitGTK stays system-managed and
//! receives security updates from the distribution. WebKitGTK is licensed under LGPL-2.1 (with
//! BSD-licensed parts), GTK 3 under LGPL-2.1 and json-glib under LGPL-2.1; they are linked only
//! into the separate helper executable and obtained from the system.

pub mod context;
pub mod extract;
pub mod frames;
pub mod gpu;
pub mod helper;
pub mod input;
pub mod page;
pub mod profile;
pub mod protocol;
pub mod state;
pub mod testing;
pub mod web_page;

pub use context::BrowserContext;
pub use helper::{Helper, HelperConfig, Timings};
pub use page::{NativePage, PageShared};
pub use profile::BrowserProfile;
pub use state::{InputState, MenuItem, MenuRequest, PageEvent, PageState, Presentation};
pub use web_page::{
    PaintPath, WebPage, WebPageEvent, frame_placement, page_element, select_paint_path,
};
