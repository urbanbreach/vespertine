//! Modal and nonmodal dialogs and confirmation forms (CMP-003): the native half.
//!
//! [`model`] holds the action descriptors, the dismissal policy and the tab-stop wrap rule; [`view`]
//! is the dialog surface built on the dismissal-layer stack and contained menus of CMP-002.

pub mod model;
pub mod view;

pub use model::{ActionKind, Activation, DialogAction, DialogModel, Dismissal, DismissalPolicy};
pub use view::{Dialog, DialogObserver, DialogPart, LimitsProvider, limits_following};
