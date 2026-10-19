//! Reference descriptors and owner-supplied grammars (CMP-010 R1).
//!
//! The editable source always contains the canonical reference text. A [`ReferenceGrammar`] parses
//! that text into [`ReferenceDescriptor`]s; several grammars are combined in a [`GrammarSet`],
//! which merges, sorts and de-overlaps their descriptors.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;
use vespertine::SharedString;

/// What a reference points at. The kind selects the icon, the label rules and the hover detail.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ReferenceKind {
    /// A file; its identity is a path.
    File,
    /// A directory; its identity is a path (the hover detail adds a trailing `/`).
    Directory,
    /// An image attachment; chips of this kind can be activated (CMP-010 R9).
    Image,
    /// Any other file attachment.
    Attachment,
    /// A command; its label shows the name as a prefix of the disambiguating suffix.
    Command,
    /// A skill; labelled like a command.
    Skill,
    /// A conversation; its identity is an opaque id, so the label is always the title.
    Conversation,
    /// An owner-defined kind, identified by a short tag.
    Other(String),
}

impl ReferenceKind {
    /// Whether disambiguated labels read `name · suffix` (commands and skills).
    pub fn shows_prefix(&self) -> bool {
        matches!(self, Self::Command | Self::Skill)
    }

    /// Whether the identity is an opaque id that must never appear in the label.
    pub fn has_opaque_identity(&self) -> bool {
        matches!(self, Self::Conversation)
    }

    /// Whether a plain press and release on a chip of this kind activates it (CMP-010 R9).
    pub fn is_activatable(&self) -> bool {
        matches!(self, Self::Image)
    }

    /// Whether chips of this kind never show a hover detail popup (CMP-010 R8).
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image)
    }

    /// The name of the glyph the default icon provider uses for this kind.
    pub fn default_glyph(&self) -> &str {
        match self {
            Self::File => "file",
            Self::Directory => "folder",
            Self::Image => "image",
            Self::Attachment => "paperclip",
            Self::Command => "command",
            Self::Skill => "sparkles",
            Self::Conversation => "message",
            Self::Other(tag) => tag,
        }
    }
}

/// One reference found in the source text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceDescriptor {
    /// The source byte range of the canonical reference text.
    pub range: Range<usize>,
    /// What the reference points at.
    pub kind: ReferenceKind,
    /// The short name shown in the chip (for a path, its base name).
    pub name: String,
    /// The identity: a path, a detail string or an opaque id. Shown in the hover detail.
    pub identity: String,
    /// The attachment number the reference names, if it references an attachment.
    pub attachment: Option<u32>,
}

/// The icon a chip shows in its icon slot (CMP-010 R6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChipIcon {
    /// A monochrome glyph by name; tinted with the muted text role.
    Glyph(SharedString),
    /// A themed (polychrome) icon by name; keeps its own colors.
    Themed(SharedString),
}

/// The owner-supplied reference grammar.
pub trait ReferenceGrammar: Send + Sync {
    /// Parses `source` into reference descriptors. Ranges must be non-empty, inside `source` and on
    /// character boundaries; other descriptors are ignored by [`GrammarSet`].
    fn parse(&self, source: &str) -> Vec<ReferenceDescriptor>;

    /// The plain text a copy of the chip produces (CMP-010 R15): for an attachment its label, for
    /// a file reference its path text.
    fn expanded_text(&self, descriptor: &ReferenceDescriptor) -> String {
        default_expanded_text(descriptor)
    }

    /// The icon chosen from the descriptor's kind and identity.
    fn icon(&self, descriptor: &ReferenceDescriptor) -> ChipIcon {
        default_icon(descriptor)
    }
}

/// The expanded text used when a grammar does not override it: the identity of a file or
/// directory, the name of everything else (and of every attachment).
pub fn default_expanded_text(descriptor: &ReferenceDescriptor) -> String {
    match (&descriptor.kind, descriptor.attachment) {
        (_, Some(_)) => descriptor.name.clone(),
        (ReferenceKind::File | ReferenceKind::Directory, None) => descriptor.identity.clone(),
        _ => descriptor.name.clone(),
    }
}

/// The icon used when a grammar does not override it: a glyph named after the kind.
pub fn default_icon(descriptor: &ReferenceDescriptor) -> ChipIcon {
    ChipIcon::Glyph(SharedString::from(
        descriptor.kind.default_glyph().to_owned(),
    ))
}

type ParseFn = dyn Fn(&str) -> Vec<ReferenceDescriptor> + Send + Sync;
type ExpandFn = dyn Fn(&ReferenceDescriptor) -> String + Send + Sync;
type IconFn = dyn Fn(&ReferenceDescriptor) -> ChipIcon + Send + Sync;

/// A [`ReferenceGrammar`] assembled from closures. Without overrides it uses
/// [`default_expanded_text`] and [`default_icon`].
pub struct ClosureGrammar {
    parse: Box<ParseFn>,
    expanded: Option<Box<ExpandFn>>,
    icon: Option<Box<IconFn>>,
}

impl ClosureGrammar {
    /// A grammar that parses with `parse`.
    pub fn new(parse: impl Fn(&str) -> Vec<ReferenceDescriptor> + Send + Sync + 'static) -> Self {
        Self {
            parse: Box::new(parse),
            expanded: None,
            icon: None,
        }
    }

    /// Overrides the expanded plain text of a chip.
    pub fn with_expanded_text(
        mut self,
        expanded: impl Fn(&ReferenceDescriptor) -> String + Send + Sync + 'static,
    ) -> Self {
        self.expanded = Some(Box::new(expanded));
        self
    }

    /// Overrides the icon provider.
    pub fn with_icon(
        mut self,
        icon: impl Fn(&ReferenceDescriptor) -> ChipIcon + Send + Sync + 'static,
    ) -> Self {
        self.icon = Some(Box::new(icon));
        self
    }
}

impl ReferenceGrammar for ClosureGrammar {
    fn parse(&self, source: &str) -> Vec<ReferenceDescriptor> {
        (self.parse)(source)
    }

    fn expanded_text(&self, descriptor: &ReferenceDescriptor) -> String {
        match &self.expanded {
            Some(expanded) => expanded(descriptor),
            None => default_expanded_text(descriptor),
        }
    }

    fn icon(&self, descriptor: &ReferenceDescriptor) -> ChipIcon {
        match &self.icon {
            Some(icon) => icon(descriptor),
            None => default_icon(descriptor),
        }
    }
}

/// A descriptor together with the grammar that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergedReference {
    /// The descriptor.
    pub descriptor: ReferenceDescriptor,
    /// Index of the producing grammar in the [`GrammarSet`].
    pub grammar: usize,
}

/// The grammars of an editor.
#[derive(Clone, Default)]
pub struct GrammarSet {
    grammars: Vec<Arc<dyn ReferenceGrammar>>,
}

impl GrammarSet {
    /// An empty set: no references, the source is shown verbatim.
    pub fn new() -> Self {
        Self::default()
    }

    /// A set of the given grammars.
    pub fn from_grammars(grammars: Vec<Arc<dyn ReferenceGrammar>>) -> Self {
        Self { grammars }
    }

    /// Adds a grammar after the existing ones; earlier grammars win ties between descriptors that
    /// start at the same offset.
    pub fn push(&mut self, grammar: Arc<dyn ReferenceGrammar>) {
        self.grammars.push(grammar);
    }

    /// Whether the set holds no grammar.
    pub fn is_empty(&self) -> bool {
        self.grammars.is_empty()
    }

    /// Parses `source` with every grammar and merges the results (CMP-010 R1): descriptors are
    /// sorted by source start (ties keep grammar order, then parse order), a descriptor that
    /// overlaps an earlier accepted one is ignored, and a descriptor with an empty, out-of-bounds
    /// or non-boundary range is dropped.
    pub fn parse(&self, source: &str) -> Vec<MergedReference> {
        let mut all: Vec<MergedReference> = Vec::new();
        for (grammar, parser) in self.grammars.iter().enumerate() {
            for descriptor in parser.parse(source) {
                let range = &descriptor.range;
                let valid = range.start < range.end
                    && range.end <= source.len()
                    && source.is_char_boundary(range.start)
                    && source.is_char_boundary(range.end);
                if valid {
                    all.push(MergedReference {
                        descriptor,
                        grammar,
                    });
                }
            }
        }
        all.sort_by_key(|merged| merged.descriptor.range.start);
        let mut accepted: Vec<MergedReference> = Vec::with_capacity(all.len());
        for merged in all {
            let overlaps = accepted
                .last()
                .is_some_and(|last| merged.descriptor.range.start < last.descriptor.range.end);
            if !overlaps {
                accepted.push(merged);
            }
        }
        accepted
    }

    /// The merged descriptors without their grammar indices (`references()` of the editor).
    pub fn references(&self, source: &str) -> Vec<ReferenceDescriptor> {
        self.parse(source)
            .into_iter()
            .map(|merged| merged.descriptor)
            .collect()
    }

    /// The expanded plain text of a merged reference, from its own grammar.
    pub fn expanded_text(&self, merged: &MergedReference) -> String {
        self.grammars[merged.grammar].expanded_text(&merged.descriptor)
    }

    /// The icon of a merged reference, from its own grammar.
    pub fn icon(&self, merged: &MergedReference) -> ChipIcon {
        self.grammars[merged.grammar].icon(&merged.descriptor)
    }

    /// The attachment numbers referenced in `source`.
    pub fn attachment_numbers(&self, source: &str) -> BTreeSet<u32> {
        self.parse(source)
            .into_iter()
            .filter_map(|merged| merged.descriptor.attachment)
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A descriptor from its parts.
    pub fn descriptor(
        range: Range<usize>,
        kind: ReferenceKind,
        name: &str,
        identity: &str,
        attachment: Option<u32>,
    ) -> ReferenceDescriptor {
        ReferenceDescriptor {
            range,
            kind,
            name: name.to_owned(),
            identity: identity.to_owned(),
            attachment,
        }
    }

    /// Parses `⟦file:path⟧` as files and `⟦img:N:label⟧` as image attachment `N` named `label`.
    pub fn bracket_grammar() -> ClosureGrammar {
        ClosureGrammar::new(|source| {
            let mut found = Vec::new();
            let mut search = 0;
            while let Some(open) = source[search..].find('⟦') {
                let start = search + open;
                let Some(close) = source[start..].find('⟧') else {
                    break;
                };
                let end = start + close + '⟧'.len_utf8();
                let body = &source[start + '⟦'.len_utf8()..end - '⟧'.len_utf8()];
                let parts: Vec<&str> = body.splitn(3, ':').collect();
                match parts.as_slice() {
                    ["file", path] => found.push(descriptor(
                        start..end,
                        ReferenceKind::File,
                        path.rsplit('/').next().unwrap_or(path),
                        path,
                        None,
                    )),
                    ["img", number, label] => found.push(descriptor(
                        start..end,
                        ReferenceKind::Image,
                        label,
                        label,
                        number.parse().ok(),
                    )),
                    _ => {}
                }
                search = end;
            }
            found
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn set(grammars: Vec<ClosureGrammar>) -> GrammarSet {
        GrammarSet::from_grammars(
            grammars
                .into_iter()
                .map(|g| Arc::new(g) as Arc<dyn ReferenceGrammar>)
                .collect(),
        )
    }

    #[test]
    fn descriptors_of_several_grammars_are_sorted_by_start() {
        let first =
            ClosureGrammar::new(|_| vec![descriptor(10..14, ReferenceKind::File, "b", "b", None)]);
        let second =
            ClosureGrammar::new(|_| vec![descriptor(2..6, ReferenceKind::Command, "a", "a", None)]);
        let merged = set(vec![first, second]).parse("0123456789abcdefgh");
        let starts: Vec<usize> = merged.iter().map(|m| m.descriptor.range.start).collect();
        assert_eq!(starts, vec![2, 10]);
        assert_eq!(merged[0].grammar, 1);
        assert_eq!(merged[1].grammar, 0);
    }

    #[test]
    fn overlapping_descriptor_is_ignored_by_earliest_start() {
        let first = ClosureGrammar::new(|_| {
            vec![descriptor(4..10, ReferenceKind::File, "late", "late", None)]
        });
        let second = ClosureGrammar::new(|_| {
            vec![
                descriptor(2..6, ReferenceKind::File, "early", "early", None),
                descriptor(6..8, ReferenceKind::File, "after", "after", None),
            ]
        });
        let merged = set(vec![first, second]).parse("0123456789abcdef");
        let names: Vec<&str> = merged.iter().map(|m| m.descriptor.name.as_str()).collect();
        assert_eq!(names, vec!["early", "after"]);
    }

    #[test]
    fn equal_starts_keep_grammar_order() {
        let first = ClosureGrammar::new(|_| {
            vec![descriptor(0..4, ReferenceKind::File, "one", "one", None)]
        });
        let second = ClosureGrammar::new(|_| {
            vec![descriptor(0..6, ReferenceKind::File, "two", "two", None)]
        });
        let merged = set(vec![first, second]).parse("0123456789");
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].descriptor.name, "one");
    }

    #[test]
    fn invalid_ranges_are_dropped() {
        let grammar = ClosureGrammar::new(|_| {
            vec![
                descriptor(3..3, ReferenceKind::File, "empty", "e", None),
                descriptor(2..99, ReferenceKind::File, "far", "f", None),
                descriptor(1..2, ReferenceKind::File, "inside-char", "c", None),
                descriptor(0..2, ReferenceKind::File, "ok", "ok", None),
            ]
        });
        let merged = set(vec![grammar]).parse("éa b");
        let names: Vec<&str> = merged.iter().map(|m| m.descriptor.name.as_str()).collect();
        assert_eq!(names, vec!["ok"]);
    }

    #[test]
    fn defaults_expand_files_to_paths_and_attachments_to_names() {
        let file = descriptor(0..1, ReferenceKind::File, "x.rs", "src/x.rs", None);
        let image = descriptor(0..1, ReferenceKind::Image, "Image 1.png", "p", Some(1));
        let command = descriptor(0..1, ReferenceKind::Command, "review", "cmd/review", None);
        assert_eq!(default_expanded_text(&file), "src/x.rs");
        assert_eq!(default_expanded_text(&image), "Image 1.png");
        assert_eq!(default_expanded_text(&command), "review");
    }

    #[test]
    fn closure_overrides_are_used_per_grammar() {
        let grammar = ClosureGrammar::new(|_| {
            vec![descriptor(0..2, ReferenceKind::File, "x", "dir/x", None)]
        })
        .with_expanded_text(|d| format!("<{}>", d.identity))
        .with_icon(|_| ChipIcon::Themed("rust".into()));
        let set = set(vec![grammar]);
        let merged = set.parse("ab");
        assert_eq!(set.expanded_text(&merged[0]), "<dir/x>");
        assert_eq!(set.icon(&merged[0]), ChipIcon::Themed("rust".into()));
    }

    #[test]
    fn attachment_numbers_are_collected() {
        let set = set(vec![bracket_grammar()]);
        let numbers = set.attachment_numbers("⟦img:2:a.png⟧ ⟦file:x⟧ ⟦img:2:a.png⟧ ⟦img:5:b.png⟧");
        assert_eq!(numbers.into_iter().collect::<Vec<_>>(), vec![2, 5]);
    }
}
