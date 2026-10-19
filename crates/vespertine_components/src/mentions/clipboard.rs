//! Scoped clipboard payload and the paste decision (CMP-010 R15, R16).
//!
//! Copy writes the expanded plain text as the clipboard text and a versioned JSON payload as the
//! structured metadata (PLT-009). Paste restores the chips only when the payload still matches the
//! clipboard text and the attachment numbers it references belong to the pasting editor's scope.

use super::reference::GrammarSet;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use vespertine::{ClipboardEntry, ClipboardItem};

/// The payload format version written into the metadata.
pub const PAYLOAD_VERSION: u32 = 1;

/// The structured metadata of an editor copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedClipboardPayload {
    /// The exact selected source string.
    pub source: String,
    /// The expanded plain text; equal to the clipboard text at copy time.
    pub text: String,
    /// The copying editor's attachment scope identifier.
    pub scope: String,
}

#[derive(Serialize)]
struct WireOut<'a> {
    version: u32,
    source: &'a str,
    text: &'a str,
    scope: &'a str,
}

#[derive(Deserialize)]
struct WireIn {
    version: u32,
    source: String,
    text: String,
    scope: String,
}

impl ScopedClipboardPayload {
    /// A payload from its parts.
    pub fn new(
        source: impl Into<String>,
        text: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            text: text.into(),
            scope: scope.into(),
        }
    }

    /// The payload of copying `selection` of `source` (R15): the exact selected source string, its
    /// expanded plain text and the editor's `scope`. Returns `None` for an empty or invalid
    /// selection.
    pub fn for_selection(
        source: &str,
        selection: Range<usize>,
        grammars: &GrammarSet,
        scope: &str,
    ) -> Option<Self> {
        let selected = source
            .get(selection.clone())
            .filter(|text| !text.is_empty())?;
        Some(Self::new(
            selected,
            expanded_plain_text(source, selection, grammars),
            scope,
        ))
    }

    /// The versioned JSON metadata string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.wire()).expect("a struct of strings serializes")
    }

    /// Parses metadata written by [`to_json`](Self::to_json); `None` for any other JSON, malformed
    /// JSON or an unknown version.
    pub fn from_json(json: &str) -> Option<Self> {
        let wire: WireIn = serde_json::from_str(json).ok()?;
        (wire.version == PAYLOAD_VERSION).then(|| Self::new(wire.source, wire.text, wire.scope))
    }

    fn wire(&self) -> WireOut<'_> {
        WireOut {
            version: PAYLOAD_VERSION,
            source: &self.source,
            text: &self.text,
            scope: &self.scope,
        }
    }

    /// The clipboard item to write: the expanded text with the payload as JSON metadata.
    pub fn to_clipboard_item(&self) -> ClipboardItem {
        ClipboardItem::new_string_with_json_metadata(self.text.clone(), self.wire())
    }

    /// The payload carried by a clipboard item, read from the metadata of its first string entry.
    pub fn from_clipboard_item(item: &ClipboardItem) -> Option<Self> {
        item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::String(string) => string.metadata.as_deref().and_then(Self::from_json),
            _ => None,
        })
    }
}

/// The plain text of `range` of `source` (R15): every chip inside the range is replaced by its
/// grammar's expanded text, text between chips stays verbatim. Chips only partly inside the range
/// stay verbatim (the editor always selects whole chips). An invalid range gives an empty string.
pub fn expanded_plain_text(source: &str, range: Range<usize>, grammars: &GrammarSet) -> String {
    let Some(selected) = source.get(range.clone()) else {
        return String::new();
    };
    let mut out = String::with_capacity(selected.len());
    let mut cursor = range.start;
    for merged in grammars.parse(source) {
        let chip = &merged.descriptor.range;
        if chip.start >= range.start && chip.end <= range.end {
            out.push_str(&source[cursor..chip.start]);
            out.push_str(&grammars.expanded_text(&merged));
            cursor = chip.end;
        }
    }
    out.push_str(&source[cursor..range.end]);
    out
}

/// The text to paste (R16). Structure is restored only when `payload` is present, its plain text
/// equals `clipboard_text` (the clipboard was not replaced meanwhile) and it carries a source
/// string; then the source is pasted if the payload's scope equals `editor_scope` or the source
/// contains no attachment-number references, and the plain expanded text otherwise (a foreign
/// scope must never name a wrong attachment). Without a usable payload the clipboard text is
/// pasted as plain text.
pub fn paste_text(
    clipboard_text: &str,
    payload: Option<&ScopedClipboardPayload>,
    editor_scope: &str,
    grammars: &GrammarSet,
) -> String {
    let Some(payload) = payload else {
        return clipboard_text.to_owned();
    };
    if payload.text != clipboard_text || payload.source.is_empty() {
        return clipboard_text.to_owned();
    }
    let keeps_chips =
        payload.scope == editor_scope || grammars.attachment_numbers(&payload.source).is_empty();
    if keeps_chips {
        payload.source.clone()
    } else {
        payload.text.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::super::reference::{ReferenceGrammar, test_support::bracket_grammar};
    use super::*;
    use std::sync::Arc;

    fn grammars() -> GrammarSet {
        GrammarSet::from_grammars(vec![
            Arc::new(bracket_grammar()) as Arc<dyn ReferenceGrammar>
        ])
    }

    const IMAGES: &str = "compare ⟦img:1:Image 1.png⟧ ⟦img:2:Image 2.png⟧ ";
    const FILES: &str = "see ⟦file:src/a.rs⟧ and ⟦file:b/c.rs⟧";

    #[test]
    fn expanded_text_replaces_attachments_by_label_and_files_by_path() {
        assert_eq!(
            expanded_plain_text(IMAGES, 0..IMAGES.len(), &grammars()),
            "compare Image 1.png Image 2.png "
        );
        assert_eq!(
            expanded_plain_text(FILES, 0..FILES.len(), &grammars()),
            "see src/a.rs and b/c.rs"
        );
    }

    #[test]
    fn expanded_text_covers_only_the_selection() {
        let start = FILES.find('⟦').unwrap();
        let end = FILES.find(" and").unwrap();
        assert_eq!(
            expanded_plain_text(FILES, start..end, &grammars()),
            "src/a.rs"
        );
        assert_eq!(expanded_plain_text(FILES, 0..start, &grammars()), "see ");
        assert_eq!(expanded_plain_text(FILES, 0..4, &grammars()), "see ");
        // A chip cut in half stays verbatim.
        let half = start + '⟦'.len_utf8() + 2;
        assert_eq!(
            expanded_plain_text(FILES, start..half, &grammars()),
            &FILES[start..half]
        );
        assert_eq!(expanded_plain_text(FILES, 3..99, &grammars()), "");
    }

    #[test]
    fn payload_for_selection_records_source_text_and_scope() {
        let payload =
            ScopedClipboardPayload::for_selection(IMAGES, 8..IMAGES.len(), &grammars(), "draft-a")
                .unwrap();
        assert_eq!(payload.source, &IMAGES[8..]);
        assert_eq!(payload.text, "Image 1.png Image 2.png ");
        assert_eq!(payload.scope, "draft-a");
        assert!(ScopedClipboardPayload::for_selection(IMAGES, 3..3, &grammars(), "s").is_none());
    }

    #[test]
    fn payload_round_trips_through_json_and_the_clipboard_item() {
        let payload = ScopedClipboardPayload::new(IMAGES, "plain", "draft-a");
        let json = payload.to_json();
        assert!(json.contains("\"version\":1"));
        assert_eq!(
            ScopedClipboardPayload::from_json(&json),
            Some(payload.clone())
        );

        let item = payload.to_clipboard_item();
        assert_eq!(item.text().as_deref(), Some("plain"));
        assert_eq!(
            ScopedClipboardPayload::from_clipboard_item(&item),
            Some(payload)
        );
    }

    #[test]
    fn foreign_metadata_is_not_a_payload() {
        assert!(ScopedClipboardPayload::from_json("not json").is_none());
        assert!(ScopedClipboardPayload::from_json("{}").is_none());
        assert!(
            ScopedClipboardPayload::from_json(
                r#"{"version":2,"source":"a","text":"a","scope":"s"}"#
            )
            .is_none()
        );
        assert!(
            ScopedClipboardPayload::from_json(r#"{"version":1,"text":"a","scope":"s"}"#).is_none()
        );
        let plain = ClipboardItem::new_string("plain".to_owned());
        assert!(ScopedClipboardPayload::from_clipboard_item(&plain).is_none());
        let other = ClipboardItem::new_string_with_metadata("t".to_owned(), "{\"x\":1}".to_owned());
        assert!(ScopedClipboardPayload::from_clipboard_item(&other).is_none());
    }

    fn copy(source: &str, scope: &str) -> (String, ScopedClipboardPayload) {
        let payload =
            ScopedClipboardPayload::for_selection(source, 0..source.len(), &grammars(), scope)
                .unwrap();
        (payload.text.clone(), payload)
    }

    #[test]
    fn same_scope_paste_restores_the_source() {
        let (text, payload) = copy(IMAGES, "draft-a");
        assert_eq!(
            paste_text(&text, Some(&payload), "draft-a", &grammars()),
            IMAGES
        );
    }

    #[test]
    fn foreign_scope_with_attachment_references_pastes_plain_expanded_text() {
        let (text, payload) = copy(IMAGES, "draft-a");
        assert_eq!(
            paste_text(&text, Some(&payload), "draft-b", &grammars()),
            "compare Image 1.png Image 2.png "
        );
    }

    #[test]
    fn source_without_attachment_references_pastes_as_chips_in_either_scope() {
        let (text, payload) = copy(FILES, "draft-a");
        assert_eq!(
            paste_text(&text, Some(&payload), "draft-a", &grammars()),
            FILES
        );
        assert_eq!(
            paste_text(&text, Some(&payload), "draft-b", &grammars()),
            FILES
        );
    }

    #[test]
    fn replaced_clipboard_text_ignores_the_metadata() {
        let (_, payload) = copy(IMAGES, "draft-a");
        assert_eq!(
            paste_text("something else", Some(&payload), "draft-a", &grammars()),
            "something else"
        );
    }

    #[test]
    fn paste_without_metadata_or_source_is_plain() {
        assert_eq!(paste_text("hello", None, "s", &grammars()), "hello");
        let empty_source = ScopedClipboardPayload::new("", "", "s");
        assert_eq!(paste_text("", Some(&empty_source), "s", &grammars()), "");
    }

    #[test]
    fn copy_then_paste_through_a_clipboard_item() {
        let payload =
            ScopedClipboardPayload::for_selection(IMAGES, 0..IMAGES.len(), &grammars(), "a")
                .unwrap();
        let item = payload.to_clipboard_item();
        let text = item.text().unwrap();
        let read_back = ScopedClipboardPayload::from_clipboard_item(&item);
        assert_eq!(
            paste_text(&text, read_back.as_ref(), "a", &grammars()),
            IMAGES
        );
        assert_eq!(
            paste_text(&text, read_back.as_ref(), "b", &grammars()),
            "compare Image 1.png Image 2.png "
        );
    }
}
