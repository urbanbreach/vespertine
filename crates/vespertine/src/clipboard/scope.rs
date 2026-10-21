//! Scoped structured copy and paste.
//!
//! Editors that show *projections* (inline chips standing for files, mentions or
//! attachments) copy two things at once: plain text in which every projection is replaced by
//! its expanded plain-text form, and metadata describing the projections. The metadata is an
//! opaque, versioned JSON payload (it travels in the string entry's metadata, see
//! [`ClipboardString::metadata`](crate::ClipboardString)) that records
//!
//! * the payload version ([`SCOPED_METADATA_VERSION`]),
//! * a *scope identity*, an application-chosen string such as an editor family name,
//! * the exact copied text, and
//! * every projection with its application-defined reference and its byte range in that text.
//!
//! [`resolve_paste`] restores the structure only when all of these hold: the item is a single
//! string entry carrying metadata, the metadata parses, its version is understood, the scope is
//! accepted by the pasting editor, the recorded text equals the clipboard text and the ranges
//! are well formed. In every other case the plain expanded text is used and no error is
//! raised, so metadata is never required and stale metadata is never applied.

use std::ops::Range;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::ClipboardItem;

/// The only metadata payload version this implementation writes and understands.
pub const SCOPED_METADATA_VERSION: u32 = 1;

/// One projection of copied content: an application-defined reference and the byte range in
/// the expanded text that the projection's expanded form occupies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection<R> {
    /// The source-aware reference from which the projection can be recreated.
    pub reference: R,
    /// The byte range of the expanded form in the copied text.
    pub range: Range<usize>,
}

/// Copied content: expanded plain text plus the projections it contains.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedContent<R> {
    text: String,
    projections: Vec<Projection<R>>,
}

/// The ranges of a [`ScopedContent`] are not usable: out of bounds, not on character
/// boundaries, empty-reversed, unordered or overlapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidProjectionRanges;

impl std::fmt::Display for InvalidProjectionRanges {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "projection ranges must lie on character boundaries of the text, in ascending order and without overlap",
        )
    }
}

impl std::error::Error for InvalidProjectionRanges {}

impl<R> ScopedContent<R> {
    /// Creates content from expanded text and its projections.
    ///
    /// Ranges are byte ranges into `text`, must lie on character boundaries, ascend and not
    /// overlap.
    pub fn new(
        text: String,
        projections: Vec<Projection<R>>,
    ) -> Result<Self, InvalidProjectionRanges> {
        let mut end_of_previous = 0;
        for projection in &projections {
            let range = &projection.range;
            if range.start < end_of_previous
                || range.start > range.end
                || range.end > text.len()
                || !text.is_char_boundary(range.start)
                || !text.is_char_boundary(range.end)
            {
                return Err(InvalidProjectionRanges);
            }
            end_of_previous = range.end;
        }
        Ok(Self { text, projections })
    }

    /// Content without projections: just text.
    pub fn plain(text: String) -> Self {
        Self {
            text,
            projections: Vec::new(),
        }
    }

    /// The expanded plain text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The projections, in text order.
    pub fn projections(&self) -> &[Projection<R>] {
        &self.projections
    }

    /// Builds the clipboard item for copying or cutting this content from an editor of the
    /// given scope.
    ///
    /// Content without projections yields plain text without metadata. Otherwise the item is
    /// a single string entry whose text is the expanded text and whose metadata is the
    /// scope-tagged payload.
    pub fn into_clipboard_item(self, scope: &str) -> ClipboardItem
    where
        R: Serialize,
    {
        if self.projections.is_empty() {
            return ClipboardItem::new_string(self.text);
        }
        let payload = Payload {
            version: SCOPED_METADATA_VERSION,
            scope: scope.to_owned(),
            text: self.text.clone(),
            projections: self.projections,
        };
        ClipboardItem::new_string_with_json_metadata(self.text, payload)
    }
}

/// The outcome of pasting the text representation of a clipboard item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopedPaste<R> {
    /// The structure may be restored: recreate the projections from their references.
    Structured(ScopedContent<R>),
    /// Insert this plain expanded text; no structure applies.
    Plain(String),
}

#[derive(Serialize, Deserialize)]
struct Payload<R> {
    version: u32,
    scope: String,
    text: String,
    projections: Vec<Projection<R>>,
}

#[derive(Deserialize)]
struct Envelope {
    version: u32,
}

/// Decides how the text of `item` is pasted into an editor whose scope compatibility
/// predicate is `accepts_scope`.
///
/// Returns `None` when the item has no text. Otherwise returns [`ScopedPaste::Structured`]
/// when every condition for restoring structure holds (see the module documentation) and
/// [`ScopedPaste::Plain`] with the clipboard text in all other cases. Never fails and never
/// applies partial structure.
pub fn resolve_paste<R: DeserializeOwned>(
    item: &ClipboardItem,
    accepts_scope: impl Fn(&str) -> bool,
) -> Option<ScopedPaste<R>> {
    let text = item.text().filter(|text| !text.is_empty())?;
    match restore_structure(item, &text, accepts_scope) {
        Some(content) => Some(ScopedPaste::Structured(content)),
        None => Some(ScopedPaste::Plain(text)),
    }
}

fn restore_structure<R: DeserializeOwned>(
    item: &ClipboardItem,
    text: &str,
    accepts_scope: impl Fn(&str) -> bool,
) -> Option<ScopedContent<R>> {
    let metadata = item.metadata()?;
    let envelope: Envelope = serde_json::from_str(metadata).ok()?;
    if envelope.version != SCOPED_METADATA_VERSION {
        return None;
    }
    let payload: Payload<R> = serde_json::from_str(metadata).ok()?;
    if !accepts_scope(&payload.scope) || payload.text != text {
        return None;
    }
    ScopedContent::new(payload.text, payload.projections).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    type Content = ScopedContent<String>;

    fn chip_content() -> Content {
        // "see @main.rs and @lib.rs": two projections in the expanded text.
        let text = "see @main.rs and @lib.rs".to_owned();
        ScopedContent::new(
            text,
            vec![
                Projection {
                    reference: "file:main.rs".into(),
                    range: 4..12,
                },
                Projection {
                    reference: "file:lib.rs".into(),
                    range: 17..24,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn copy_without_projections_is_plain_text() {
        let item = Content::plain("abc".into()).into_clipboard_item("docs");
        assert_eq!(item, ClipboardItem::new_string("abc".into()));
        assert_eq!(item.metadata(), None);
    }

    #[test]
    fn compatible_scope_restores_structure_in_order() {
        let item = chip_content().into_clipboard_item("docs");
        assert_eq!(item.text().as_deref(), Some("see @main.rs and @lib.rs"));
        let pasted = resolve_paste::<String>(&item, |scope| scope == "docs").unwrap();
        assert_eq!(pasted, ScopedPaste::Structured(chip_content()));
    }

    #[test]
    fn incompatible_scope_pastes_plain_expanded_text() {
        let item = chip_content().into_clipboard_item("docs");
        let pasted = resolve_paste::<String>(&item, |scope| scope == "chat").unwrap();
        assert_eq!(
            pasted,
            ScopedPaste::Plain("see @main.rs and @lib.rs".into())
        );
    }

    #[test]
    fn text_mismatch_pastes_plain_text() {
        let mut item = chip_content().into_clipboard_item("docs");
        let metadata = item.metadata().cloned();
        // Another program replaced the clipboard text; the old metadata lingers.
        item = ClipboardItem::new_string_with_metadata("replaced".into(), metadata.unwrap());
        let pasted = resolve_paste::<String>(&item, |_| true).unwrap();
        assert_eq!(pasted, ScopedPaste::Plain("replaced".into()));
    }

    #[test]
    fn unknown_version_pastes_plain_text() {
        let metadata = serde_json::json!({
            "version": 99,
            "scope": "docs",
            "text": "x",
            "projections": [{"reference": "r", "range": {"start": 0, "end": 1}}],
        })
        .to_string();
        let item = ClipboardItem::new_string_with_metadata("x".into(), metadata);
        let pasted = resolve_paste::<String>(&item, |_| true).unwrap();
        assert_eq!(pasted, ScopedPaste::Plain("x".into()));
    }

    #[test]
    fn malformed_metadata_pastes_plain_text() {
        for metadata in [
            "not json",
            "{}",
            "[]",
            r#"{"version":1,"scope":"docs"}"#,
            // Reference of the wrong shape for the pasting editor.
            r#"{"version":1,"scope":"docs","text":"x","projections":[{"reference":7,"range":{"start":0,"end":1}}]}"#,
            // Range outside the text.
            r#"{"version":1,"scope":"docs","text":"x","projections":[{"reference":"r","range":{"start":0,"end":9}}]}"#,
        ] {
            let item = ClipboardItem::new_string_with_metadata("x".into(), metadata.into());
            let pasted = resolve_paste::<String>(&item, |_| true).unwrap();
            assert_eq!(pasted, ScopedPaste::Plain("x".into()), "{metadata}");
        }
    }

    #[test]
    fn metadata_on_multi_entry_items_is_not_applied() {
        let mut item = chip_content().into_clipboard_item("docs");
        item.entries
            .push(crate::ClipboardEntry::Image(crate::Image::empty()));
        let pasted = resolve_paste::<String>(&item, |_| true).unwrap();
        assert_eq!(
            pasted,
            ScopedPaste::Plain("see @main.rs and @lib.rs".into())
        );
    }

    #[test]
    fn items_without_text_have_nothing_to_paste() {
        let item = ClipboardItem::new_image(&crate::Image::empty());
        assert_eq!(resolve_paste::<String>(&item, |_| true), None);
        assert_eq!(
            resolve_paste::<String>(&ClipboardItem::new_string(String::new()), |_| true),
            None
        );
    }

    #[allow(clippy::reversed_empty_ranges)]
    #[test]
    fn invalid_ranges_are_rejected_when_building_content() {
        let projection = |range| Projection {
            reference: (),
            range,
        };
        assert!(ScopedContent::new("héllo".into(), vec![projection(0..2)]).is_err());
        assert!(ScopedContent::new("abc".into(), vec![projection(0..4)]).is_err());
        assert!(ScopedContent::new("abc".into(), vec![projection(2..1)]).is_err());
        assert!(
            ScopedContent::new("abcd".into(), vec![projection(0..3), projection(2..4)]).is_err()
        );
        assert!(
            ScopedContent::new("abcd".into(), vec![projection(0..2), projection(2..4)]).is_ok()
        );
    }
}
