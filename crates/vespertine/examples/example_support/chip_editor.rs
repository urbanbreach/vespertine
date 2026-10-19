//! A tiny reference editor model for the rich clipboard example and tests.
//!
//! Content is a sequence of units: plain characters and *chips*, inline projections standing
//! for files or mentions. A chip's expanded plain-text form is `@name`. The model implements
//! [`EditTarget`], so the standard cut, copy, paste and select-all actions work on it exactly
//! as they would on a real editor. It is not a text editor: there is no layout, no caret
//! movement and no rendering.
#![allow(dead_code)]

use std::ops::Range;

use serde::{Deserialize, Serialize};
use vespertine::{
    ClipboardItem, EditState, EditTarget, Image, PasteContent, PastePolicy, Projection,
    ScopedContent,
};

/// The source-aware reference of a chip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChipRef {
    File { path: String },
    Mention { name: String },
}

impl ChipRef {
    /// What the user sees when the chip is expanded.
    pub fn expanded(&self) -> String {
        match self {
            ChipRef::File { path } => format!("@{path}"),
            ChipRef::Mention { name } => format!("@{name}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    Char(char),
    Chip(ChipRef),
}

pub struct ChipEditor {
    /// Scope identity written with structured copies.
    pub scope: &'static str,
    /// Scopes whose structured clipboard content this editor restores.
    pub accepted_scopes: &'static [&'static str],
    pub editable: bool,
    pub policy: PastePolicy,
    units: Vec<Unit>,
    /// Selection as a range of unit indices.
    selection: Range<usize>,
    /// Images and paths pasted by a media-first owner.
    pub attachments: Vec<String>,
}

impl ChipEditor {
    /// An editor of the given scope that restores chips from the listed scopes.
    pub fn new(scope: &'static str, accepted_scopes: &'static [&'static str]) -> Self {
        Self {
            scope,
            accepted_scopes,
            editable: true,
            policy: PastePolicy::text_first(),
            units: Vec::new(),
            selection: 0..0,
            attachments: Vec::new(),
        }
    }

    /// A plain text field: it knows no chips, so structured content never applies.
    pub fn plain_field() -> Self {
        Self::new("plain", &[])
    }

    pub fn with_policy(mut self, policy: PastePolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.editable = false;
        self
    }

    /// Replaces the selection with `text`.
    pub fn type_text(&mut self, text: &str) {
        self.replace_selection(text.chars().map(Unit::Char).collect());
    }

    /// Replaces the selection with a chip.
    pub fn insert_chip(&mut self, reference: ChipRef) {
        self.replace_selection(vec![Unit::Chip(reference)]);
    }

    /// Selects the units in `range`.
    pub fn select(&mut self, range: Range<usize>) {
        self.selection = range;
    }

    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    /// The content with every chip expanded to its plain-text form.
    pub fn expanded_text(&self) -> String {
        self.units.iter().map(unit_text).collect()
    }

    pub fn chips(&self) -> Vec<&ChipRef> {
        self.units
            .iter()
            .filter_map(|unit| match unit {
                Unit::Chip(reference) => Some(reference),
                Unit::Char(_) => None,
            })
            .collect()
    }

    /// Content with chips shown as `[expanded]`, for display.
    pub fn display(&self) -> String {
        self.units
            .iter()
            .map(|unit| match unit {
                Unit::Char(character) => character.to_string(),
                Unit::Chip(reference) => format!("[{}]", reference.expanded()),
            })
            .collect()
    }

    fn replace_selection(&mut self, replacement: Vec<Unit>) {
        let start = self.selection.start;
        let inserted = replacement.len();
        self.units.splice(self.selection.clone(), replacement);
        self.selection = start + inserted..start + inserted;
    }
}

fn unit_text(unit: &Unit) -> String {
    match unit {
        Unit::Char(character) => character.to_string(),
        Unit::Chip(reference) => reference.expanded(),
    }
}

impl EditTarget for ChipEditor {
    type Reference = ChipRef;

    fn edit_state(&self) -> EditState {
        EditState {
            selection_empty: self.selection.is_empty(),
            editable: self.editable,
            content_empty: self.units.is_empty(),
        }
    }

    fn paste_policy(&self) -> &PastePolicy {
        &self.policy
    }

    fn accepts_scope(&self, scope: &str) -> bool {
        self.accepted_scopes.contains(&scope)
    }

    fn selected_item(&self) -> Option<ClipboardItem> {
        if self.selection.is_empty() {
            return None;
        }
        let mut text = String::new();
        let mut projections = Vec::new();
        for unit in &self.units[self.selection.clone()] {
            let start = text.len();
            text.push_str(&unit_text(unit));
            if let Unit::Chip(reference) = unit {
                projections.push(Projection {
                    reference: reference.clone(),
                    range: start..text.len(),
                });
            }
        }
        let content = ScopedContent::new(text, projections).ok()?;
        Some(content.into_clipboard_item(self.scope))
    }

    fn delete_selection(&mut self) {
        self.replace_selection(Vec::new());
    }

    fn insert(&mut self, paste: PasteContent<ChipRef>) {
        match paste {
            PasteContent::Structured(content) => {
                let mut units = Vec::new();
                let mut next = 0;
                for projection in content.projections() {
                    units.extend(
                        content.text()[next..projection.range.start]
                            .chars()
                            .map(Unit::Char),
                    );
                    units.push(Unit::Chip(projection.reference.clone()));
                    next = projection.range.end;
                }
                units.extend(content.text()[next..].chars().map(Unit::Char));
                self.replace_selection(units);
            }
            PasteContent::Text(text) => self.type_text(&text),
            PasteContent::Image(image) => {
                self.attachments.push(describe_image(&image));
            }
            PasteContent::Paths(paths) => {
                self.attachments
                    .extend(paths.iter().map(|path| format!("file {}", path.display())));
            }
        }
    }

    fn select_all(&mut self) {
        self.selection = 0..self.units.len();
    }
}

fn describe_image(image: &Image) -> String {
    format!(
        "image {} ({} bytes)",
        image.format.extension(),
        image.bytes.len()
    )
}

/// Everything the rich clipboard scenario produces.
pub struct Scenario {
    /// The item the chips editor copied.
    pub copied: ClipboardItem,
    /// Editor of a compatible scope: chips are restored.
    pub compatible: ChipEditor,
    /// Editor of an incompatible scope: plain expanded text.
    pub incompatible: ChipEditor,
    /// A plain field: plain expanded text.
    pub plain: ChipEditor,
    /// The item holding an image plus text.
    pub image_with_text: ClipboardItem,
    /// Owner preferring media: the image is used.
    pub media_first: ChipEditor,
    /// Owner preferring text: the text is used.
    pub text_first: ChipEditor,
}

/// Copies from an editor with chips, pastes into editors of different scopes, then pastes an
/// image-plus-text item into owners with different paste policies. The clipboard is the item
/// handed from the copy to each paste, exactly what a platform clipboard returns.
pub fn run_scenario() -> Scenario {
    use vespertine::{
        ClipboardEntry, ClipboardString, ImageFormat,
        clipboard::edit::{EditAction, EditOutcome, perform},
    };

    let mut source = ChipEditor::new("docs", &["docs"]);
    source.type_text("see ");
    source.insert_chip(ChipRef::File {
        path: "main.rs".into(),
    });
    source.type_text(" and ");
    source.insert_chip(ChipRef::Mention {
        name: "alice".into(),
    });
    perform(&mut source, EditAction::SelectAll, None);
    let EditOutcome::WriteToClipboard(copied) = perform(&mut source, EditAction::Copy, None) else {
        panic!("copy of a non-empty selection writes to the clipboard");
    };

    let paste_into = |mut editor: ChipEditor, item: &ClipboardItem| {
        perform(&mut editor, EditAction::Paste, Some(item));
        editor
    };
    let compatible = paste_into(ChipEditor::new("docs", &["docs"]), &copied);
    let incompatible = paste_into(ChipEditor::new("chat", &["chat"]), &copied);
    let plain = paste_into(ChipEditor::plain_field(), &copied);

    // A browser offers an image plus text.
    let image = Image::from_bytes(ImageFormat::Png, vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
    let image_with_text = ClipboardItem::from_entries([
        ClipboardEntry::String(ClipboardString::new("a photo".into())),
        ClipboardEntry::Image(image),
    ]);
    let media_first = paste_into(
        ChipEditor::plain_field().with_policy(PastePolicy::media_first()),
        &image_with_text,
    );
    let text_first = paste_into(ChipEditor::plain_field(), &image_with_text);

    Scenario {
        copied,
        compatible,
        incompatible,
        plain,
        image_with_text,
        media_first,
        text_first,
    }
}
