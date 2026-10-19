//! Searchable, paged pickers and command palettes (CMP-006): the native half.
//!
//! The picker is not coupled to a data source. [`sources`] protects asynchronous sources with
//! request generations, debounce, minimum query length, indexing backoff and paging; [`results`]
//! turns the sources' rows into sections with limits, caps and footer states; [`highlight`] keeps
//! the highlighted row by identity and computes the reveal offset; [`model`] holds the query,
//! scope tabs, depth, favorites, keyboard mode and the Enter latch; [`actions`] holds the
//! rebindable key actions and [`view`] the picker surface itself (query field, tabs, list, footer
//! and secondary panel) on the dismissal-layer stack.

pub mod actions;
pub mod driver;
pub mod highlight;
pub mod model;
pub mod results;
pub mod sources;
pub mod view;

pub use view::{MENU_PRIORITY, PRIORITY, Picker, PickerObserver, PickerPart, SectionProvider};
