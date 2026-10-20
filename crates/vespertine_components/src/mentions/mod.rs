//! Pure logic of inline mention and attachment projections (CMP-010).
//!
//! Editable source ranges that follow an owner-defined reference grammar are displayed as atomic
//! chips while the source keeps the canonical reference text. Everything here is independent of
//! the editor entity and the window: it computes projections, labels, wrap boundaries, pill
//! geometry, hover and activation state, insertion and removal edits, attachment ownership and the
//! scoped clipboard decision from plain inputs, and the editor of CMP-008 applies the results.
//!
//! - [`reference`]: descriptors, [`ReferenceGrammar`], [`GrammarSet`] (R1)
//! - [`label`]: label disambiguation and compaction (R3)
//! - [`chip`]: chip display strings, projection, dangling references, read-only rows (R2, R4, R17)
//! - [`wrap`]: wrap boundaries of lines with chips (R5)
//! - [`pill`]: pill geometry and vertical centering (R6, R7)
//! - [`hover`]: the hover-detail state machine (R8)
//! - [`activation`]: image-chip click activation (R9)
//! - [`insertion`]: `insert_reference`, `replace_reference`, `remove_attachment_references` (R10, R11)
//! - [`attachments`]: per-scope attachments, stash and numbering (R13, R14)
//! - [`clipboard`]: the scoped clipboard payload and the paste decision (R15, R16)
//! - [`layer`]: the editor layer that projects references as chips and reconciles attachments (R1-R4, R13)

pub mod activation;
pub mod attachments;
pub mod chip;
pub mod clipboard;
pub mod hover;
pub mod insertion;
pub mod label;
pub mod layer;
pub mod pill;
pub mod reference;
pub mod wrap;

pub use activation::{
    ActivationTarget, CLICK_DRAG_THRESHOLD, ImageChipActivation, shows_pointing_hand,
};
pub use attachments::{
    Attachment, AttachmentId, AttachmentKind, AttachmentScope, Reconciliation, Removal,
    StagedAttachment,
};
pub use chip::{
    ChipDisplay, ChipSpan, ICON_SLOT_CHARS, NBSP, ProjectedReference, ProjectionReplacement,
    SIDE_BEARING_CHARS, TRAILING_PAD_CHARS, chip_display, project_reference_text,
    project_reference_text_with_live, project_references,
};
pub use clipboard::{PAYLOAD_VERSION, ScopedClipboardPayload, expanded_plain_text, paste_text};
pub use hover::{
    HOVER_DETAIL_DELAY_MS, HoverChip, HoverDetail, HoverState, HoverTarget, POPUP_GAP,
    POPUP_HEIGHT, PopupSide, TimerRequest, detail_text, popup_top,
};
pub use insertion::{
    EditGate, ReferenceInsertion, ReferenceRemoval, insert_reference, join_links,
    remove_attachment_references, replace_reference,
};
pub use label::{
    COMPACT_LIMIT, PREFIX_SEPARATOR, compact_label, compact_labels, disambiguated_labels,
    reference_labels,
};
pub use layer::{ChipInfo, MentionsLayer};
pub use pill::{
    CENTERING_CLAMP, FaceMetrics, IconWell, PillGeometry, PillRect, centering_offset,
    pill_geometry, snap_to_device,
};
pub use reference::{
    ChipIcon, ClosureGrammar, GrammarSet, MergedReference, ReferenceDescriptor, ReferenceGrammar,
    ReferenceKind, default_expanded_text, default_icon,
};
pub use wrap::{CharWrapLine, ChipSegment, WrapGlyphs, WrapRow, chip_segments, wrap_rows};
