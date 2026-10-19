//! Modal and nonmodal dialogs and confirmation forms (CMP-003): the native half.
//!
//! [`model`] holds the action descriptors, the dismissal policy and the tab-stop wrap rule; the
//! dialog surface itself is built on the dismissal-layer stack and contained menus of CMP-002.

pub mod model;
