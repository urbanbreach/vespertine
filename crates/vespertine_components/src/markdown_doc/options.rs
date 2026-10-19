//! Owner hooks of the Markdown view (CMP-011 R22): all optional, all absent by default.

use super::cache::FlattenCache;
use super::flatten::StreamingFade;
use super::tree::{ImageInfo, TaskMarker};
use crate::code::{CardAction, CodeScrollState};
use crate::math::MathStyle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{AnyElement, App, SharedString, Window};
use vespertine_richtext::LinkOptions;
use vespertine_richtext::syntax::HighlightedDocument;
use vespertine_theme::document::DocumentTheme;

/// Task item options: with them a task item shows a checkbox instead of its literal marker.
#[derive(Clone, Default)]
pub struct TaskOptions {
    /// Called with the task's marker (byte range, checked state) when a checkbox is clicked. With
    /// no callback the checkboxes are disabled. The owner applies the toggle to the source
    /// (CMP-014 R12) and re-feeds the document.
    pub on_toggle: Option<Rc<dyn Fn(&TaskMarker, &mut Window, &mut App)>>,
}

pub use crate::diagram::{DiagramHandler, DiagramView};

/// Image resolver: the image, a stable element key and the theme; `None` declines (the label
/// link is shown instead).
pub type ImageResolver =
    Rc<dyn Fn(&ImageInfo, &str, &DocumentTheme, &mut Window, &mut App) -> Option<AnyElement>>;

/// Media resolution.
#[derive(Clone, Default)]
pub struct MediaOptions {
    /// Diagram handler for fences whose info string is `mermaid`.
    pub diagram: Option<DiagramHandler>,
    /// Image resolver.
    pub image: Option<ImageResolver>,
}

/// Per-block code UI state (keyed by the top-level block index).
#[derive(Clone, Default)]
pub struct CodeUi {
    /// Wrapped fit-to-width rows instead of horizontal scrolling.
    pub wrapped: bool,
    /// Owner-controlled scroll state of the block.
    pub scroll: Option<CodeScrollState>,
}

/// Code block controls.
#[derive(Clone, Default)]
pub struct CodeOptions {
    /// Per-block UI state keyed by block index.
    pub ui: HashMap<usize, CodeUi>,
    /// Shows a wrap toggle in every code header; called with the block index.
    pub on_toggle_wrap: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
    /// The index of the block the owner marked as just copied.
    pub copied_index: Option<usize>,
    /// Shows a copy action in every code header; called with the block index and the exact code.
    pub on_copy: Option<Rc<dyn Fn(usize, &str, &mut Window, &mut App)>>,
    /// Extra header actions for a block: `(block index, language)`.
    pub actions: Option<Rc<dyn Fn(usize, &str) -> Vec<CardAction>>>,
    /// A syntax highlight source per code block: `(block index, language, code)`; `None` uses
    /// the shared cache.
    pub highlight: Option<Rc<dyn Fn(usize, &str, &str) -> Option<Arc<HighlightedDocument>>>>,
}

/// Everything an owner may supply.
#[derive(Clone)]
pub struct MarkdownOptions {
    /// The theme.
    pub theme: DocumentTheme,
    /// A stable prefix for element identifiers.
    pub row_key: SharedString,
    /// Link routing and policy.
    pub link_options: LinkOptions,
    /// Task options.
    pub task: Option<TaskOptions>,
    /// Media options.
    pub media: MediaOptions,
    /// Code block controls.
    pub code: CodeOptions,
    /// Formula settings (CMP-012 style): enabled, face and size percentage.
    pub math: MathStyle,
    /// Whether the display tree (hanging syntax repaired) is shown instead of the canonical one.
    pub streaming: bool,
    /// Whether the document takes part in selection (links and search keep working when off).
    pub selectable: bool,
    /// The paint-only fade of the streaming tail (applied to the last block).
    pub fade: Option<StreamingFade>,
    /// The flatten cache and block generations of the document being rendered (set by the view).
    pub cache: Option<Rc<RefCell<FlattenCache>>>,
    /// Generation of each top-level block (parallel to the rendered blocks).
    pub generations: Rc<Vec<u64>>,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            theme: DocumentTheme::dark(),
            row_key: "md".into(),
            link_options: LinkOptions::default(),
            task: None,
            media: MediaOptions::default(),
            code: CodeOptions::default(),
            math: MathStyle::new(),
            streaming: false,
            selectable: true,
            fade: None,
            cache: None,
            generations: Rc::new(Vec::new()),
        }
    }
}
