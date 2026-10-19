//! Chip display strings and the source-to-display projection of references (CMP-010 R2, R4, R17).

use super::label::reference_labels;
use super::reference::{ChipIcon, GrammarSet, MergedReference, ReferenceDescriptor};
use std::collections::BTreeSet;
use std::ops::Range;

/// The non-breaking space the chip padding and the label spaces are made of.
pub const NBSP: char = '\u{00A0}';
/// Padding characters before the icon slot (the side bearing).
pub const SIDE_BEARING_CHARS: usize = 1;
/// Padding characters reserved for the icon, painted over by it.
pub const ICON_SLOT_CHARS: usize = 5;
/// Padding characters after the label.
pub const TRAILING_PAD_CHARS: usize = 2;

/// The display string of one chip and where its parts sit (byte ranges inside `text`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChipDisplay {
    /// Side bearing, icon slot, label with non-breaking spaces, trailing pad.
    pub text: String,
    /// The icon slot: the five characters the icon is painted over.
    pub icon_slot: Range<usize>,
    /// The label characters.
    pub label: Range<usize>,
}

/// Builds the display string of a chip (R2): one non-breaking space, five non-breaking spaces for
/// the icon slot, `label` with every space replaced by a non-breaking space, and two non-breaking
/// spaces. The padding characters are shaped in the owner's fixed padding face; the label in the
/// surrounding font, which is why the ranges are exposed.
pub fn chip_display(label: &str) -> ChipDisplay {
    let pad = NBSP.len_utf8();
    let mut text = String::with_capacity(
        label.len() + (SIDE_BEARING_CHARS + ICON_SLOT_CHARS + TRAILING_PAD_CHARS) * pad,
    );
    for _ in 0..SIDE_BEARING_CHARS {
        text.push(NBSP);
    }
    let slot_start = text.len();
    for _ in 0..ICON_SLOT_CHARS {
        text.push(NBSP);
    }
    let label_start = text.len();
    text.extend(label.chars().map(|c| if c == ' ' { NBSP } else { c }));
    let label_end = text.len();
    for _ in 0..TRAILING_PAD_CHARS {
        text.push(NBSP);
    }
    ChipDisplay {
        text,
        icon_slot: slot_start..label_start,
        label: label_start..label_end,
    }
}

/// One replacement of the editor's projection (CMP-008 R16): `range` is a source byte range,
/// `display` its display string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionReplacement {
    /// The replaced source byte range.
    pub range: Range<usize>,
    /// The display string.
    pub display: String,
    /// Whether the replaced range behaves as one unit for caret, selection and deletion.
    pub atomic: bool,
}

/// A reference after projection: either a chip or, when dangling, its plain label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedReference {
    /// The descriptor and its grammar.
    pub reference: MergedReference,
    /// The label text (disambiguated and compacted).
    pub label: String,
    /// The chip icon; `None` for a dangling reference, which has no chip.
    pub icon: Option<ChipIcon>,
    /// The projection replacement for the reference's source range.
    pub replacement: ProjectionReplacement,
    /// Part ranges of `replacement.display` when the reference is a chip.
    pub layout: Option<ChipDisplay>,
}

impl ProjectedReference {
    /// Whether the reference is displayed as a chip (not dangling).
    pub fn is_chip(&self) -> bool {
        self.layout.is_some()
    }
}

/// Projects every reference of `source` (R1 to R4).
///
/// `live` is the set of attachment numbers the owner has registered; a reference carrying a
/// number outside it is dangling and shows its plain label text without a chip (R4). `None`
/// treats every number as live.
pub fn project_references(
    source: &str,
    grammars: &GrammarSet,
    live: Option<&BTreeSet<u32>>,
) -> Vec<ProjectedReference> {
    let merged = grammars.parse(source);
    let labels = reference_labels(merged.iter().map(|m| &m.descriptor));
    merged
        .into_iter()
        .zip(labels)
        .map(|(reference, label)| {
            let dangling = match (reference.descriptor.attachment, live) {
                (Some(number), Some(live)) => !live.contains(&number),
                _ => false,
            };
            let range = reference.descriptor.range.clone();
            if dangling {
                ProjectedReference {
                    replacement: ProjectionReplacement {
                        range,
                        display: label.clone(),
                        atomic: false,
                    },
                    reference,
                    label,
                    icon: None,
                    layout: None,
                }
            } else {
                let display = chip_display(&label);
                let icon = grammars.icon(&reference);
                ProjectedReference {
                    replacement: ProjectionReplacement {
                        range,
                        display: display.text.clone(),
                        atomic: true,
                    },
                    reference,
                    label,
                    icon: Some(icon),
                    layout: Some(display),
                }
            }
        })
        .collect()
}

/// A chip inside a projected display string (R17).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChipSpan {
    /// The chip's byte range in the display string.
    pub display: Range<usize>,
    /// The icon slot's byte range in the display string.
    pub icon_slot: Range<usize>,
    /// The label's byte range in the display string.
    pub label_range: Range<usize>,
    /// The chip's source byte range.
    pub source: Range<usize>,
    /// The label text with ordinary spaces.
    pub label: String,
    /// The icon to paint in the icon slot.
    pub icon: ChipIcon,
    /// The descriptor the chip was built from.
    pub descriptor: ReferenceDescriptor,
}

/// Projects `source` for a read-only row (R17): the display string and the ordered chip spans over
/// it. Every attachment number counts as live; see [`project_reference_text_with_live`].
pub fn project_reference_text(source: &str, grammars: &GrammarSet) -> (String, Vec<ChipSpan>) {
    project_reference_text_with_live(source, grammars, None)
}

/// Like [`project_reference_text`] with the owner's live attachment numbers (R4): dangling
/// references become plain label text and produce no span.
pub fn project_reference_text_with_live(
    source: &str,
    grammars: &GrammarSet,
    live: Option<&BTreeSet<u32>>,
) -> (String, Vec<ChipSpan>) {
    let projected = project_references(source, grammars, live);
    let mut display = String::with_capacity(source.len());
    let mut spans = Vec::new();
    let mut cursor = 0;
    for item in projected {
        let range = item.replacement.range.clone();
        display.push_str(&source[cursor..range.start]);
        let start = display.len();
        display.push_str(&item.replacement.display);
        if let (Some(layout), Some(icon)) = (item.layout, item.icon) {
            spans.push(ChipSpan {
                display: start..display.len(),
                icon_slot: start + layout.icon_slot.start..start + layout.icon_slot.end,
                label_range: start + layout.label.start..start + layout.label.end,
                source: range.clone(),
                label: item.label,
                icon,
                descriptor: item.reference.descriptor,
            });
        }
        cursor = range.end;
    }
    display.push_str(&source[cursor..]);
    (display, spans)
}

#[cfg(test)]
mod tests {
    use super::super::reference::ReferenceGrammar;
    use super::super::reference::test_support::bracket_grammar;
    use super::*;
    use std::sync::Arc;

    fn grammars() -> GrammarSet {
        GrammarSet::from_grammars(vec![
            Arc::new(bracket_grammar()) as Arc<dyn ReferenceGrammar>
        ])
    }

    #[test]
    fn chip_display_pads_with_nbsp_and_replaces_label_spaces() {
        let chip = chip_display("Image 1.png");
        let expected = format!("{n}{n}{n}{n}{n}{n}Image{n}1.png{n}{n}", n = NBSP);
        assert_eq!(chip.text, expected);
        assert_eq!(
            &chip.text[chip.icon_slot.clone()],
            NBSP.to_string().repeat(5)
        );
        assert_eq!(&chip.text[chip.label.clone()], format!("Image{NBSP}1.png"));
        assert_eq!(chip.text.chars().filter(|c| *c == NBSP).count(), 8 + 1);
        assert!(!chip.text.contains(' '));
    }

    #[test]
    fn projection_of_source_with_chips() {
        let source = "compare ⟦img:1:Image 1.png⟧ ⟦img:2:Image 2.png⟧ ";
        let (display, spans) = project_reference_text(source, &grammars());
        let first = chip_display("Image 1.png").text;
        let second = chip_display("Image 2.png").text;
        assert_eq!(display, format!("compare {first} {second} "));
        assert_eq!(spans.len(), 2);
        assert_eq!(&display[spans[0].display.clone()], first);
        assert_eq!(&display[spans[1].display.clone()], second);
        assert_eq!(&source[spans[0].source.clone()], "⟦img:1:Image 1.png⟧");
        assert_eq!(
            &display[spans[0].label_range.clone()],
            format!("Image{NBSP}1.png")
        );
        assert_eq!(spans[0].label, "Image 1.png");
        assert_eq!(spans[0].descriptor.attachment, Some(1));
        assert!(spans[0].display.end <= spans[1].display.start);
    }

    #[test]
    fn source_without_references_is_unchanged() {
        let (display, spans) = project_reference_text("plain text", &grammars());
        assert_eq!(display, "plain text");
        assert!(spans.is_empty());
        let (display, _) = project_reference_text("anything", &GrammarSet::new());
        assert_eq!(display, "anything");
    }

    #[test]
    fn replacements_are_atomic_chips_with_source_ranges() {
        let source = "a ⟦file:src/x.rs⟧ b";
        let projected = project_references(source, &grammars(), None);
        assert_eq!(projected.len(), 1);
        assert!(projected[0].is_chip());
        assert!(projected[0].replacement.atomic);
        assert_eq!(
            &source[projected[0].replacement.range.clone()],
            "⟦file:src/x.rs⟧"
        );
        assert_eq!(projected[0].label, "x.rs");
    }

    #[test]
    fn unregistered_attachment_number_displays_as_plain_label() {
        let source = "see ⟦img:7:Image 7.png⟧ and ⟦img:1:Image 1.png⟧";
        let live = BTreeSet::from([1u32]);
        let projected = project_references(source, &grammars(), Some(&live));
        assert!(!projected[0].is_chip());
        assert!(!projected[0].replacement.atomic);
        assert_eq!(projected[0].replacement.display, "Image 7.png");
        assert!(projected[0].icon.is_none());
        assert!(projected[1].is_chip());

        let (display, spans) = project_reference_text_with_live(source, &grammars(), Some(&live));
        let chip = chip_display("Image 1.png").text;
        assert_eq!(display, format!("see Image 7.png and {chip}"));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].descriptor.attachment, Some(1));

        // Registering the number rebuilds the projection with a chip.
        let live = BTreeSet::from([1u32, 7]);
        let projected = project_references(source, &grammars(), Some(&live));
        assert!(projected.iter().all(ProjectedReference::is_chip));
    }

    #[test]
    fn references_without_numbers_are_never_dangling() {
        let live = BTreeSet::new();
        let projected = project_references("⟦file:a/b⟧", &grammars(), Some(&live));
        assert!(projected[0].is_chip());
    }

    #[test]
    fn labels_are_disambiguated_across_the_whole_row() {
        let source = "⟦file:a/x.rs⟧ ⟦file:b/x.rs⟧ ⟦file:y.rs⟧";
        let (_, spans) = project_reference_text(source, &grammars());
        let labels: Vec<&str> = spans.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["a/x.rs", "b/x.rs", "y.rs"]);
    }
}
