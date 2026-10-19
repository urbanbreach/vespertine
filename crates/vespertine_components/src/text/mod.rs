//! Text editing components: the single-line field (CMP-007), the multiline editor (CMP-008) and
//! the editing core they share.

pub mod actions;
pub mod area;
pub mod boundaries;
pub mod common;
pub mod core;
pub mod editor;
pub mod field;
pub mod utf16;

pub use area::{SubmitMode, TextArea, TextAreaEvent};
pub use common::{EditMenuState, FieldTheme, PastePreference, TextMetrics};
pub use core::{Acknowledgement, Affinity, DragAnchor, EditCore, EditKind, Snapshot, Unit};
pub use field::{TextField, TextFieldEvent};
