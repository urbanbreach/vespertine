//! Single-value select (CMP-005): the native half.
//!
//! [`model`] holds the option descriptors, the id-tracked highlight and the keyboard policy;
//! [`view`] is the [`Select`] entity: a button-role trigger and a contained, scrollable menu on
//! the floating surfaces of CMP-002.

pub mod model;
pub mod view;

pub use model::{Leading, OptionId, SelectOption, compact};
pub use view::Select;
