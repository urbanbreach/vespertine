//! The Markdown document state and view element (CMP-011 API shape): a document with an
//! incremental parser and a flatten cache, and the element that renders it.

use super::cache::FlattenCache;
use super::incremental::IncrementalParser;
use super::options::MarkdownOptions;
use super::render::render_blocks;
use super::tree::{Block, source_line};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use vespertine::{
    App, Div, ElementId, InteractiveElement, Interactivity, IntoElement, ParentElement, RenderOnce,
    SharedString, Stateful, StatefulInteractiveElement, StyleRefinement, Styled, Window, div, px,
};
use vespertine_richtext::{UserSelect, user_select};

/// A Markdown document: the stored source, its incremental parse and the streaming flag. Clones
/// share the same state.
#[derive(Clone, Default)]
pub struct MarkdownDocument {
    inner: Rc<RefCell<Inner>>,
}

#[derive(Default)]
struct Inner {
    parser: IncrementalParser,
    cache: Rc<RefCell<FlattenCache>>,
    /// Generation of every canonical top-level block; blocks before the stable prefix of an
    /// append keep theirs.
    generations: Vec<u64>,
    next_generation: u64,
    /// Revision counter: bumped on every change, so views can tell a theme or paint change from
    /// a content change.
    revision: u64,
    key: DocumentKey,
}

/// A row key unique to one document: the default row key of its views.
struct DocumentKey(SharedString);

impl Default for DocumentKey {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(format!("md{}", NEXT.fetch_add(1, Ordering::Relaxed)).into())
    }
}

impl Inner {
    fn sync_generations(&mut self, stable_prefix: usize) {
        let len = self
            .parser
            .display_tree()
            .len()
            .max(self.parser.tree().len());
        self.generations.truncate(stable_prefix);
        while self.generations.len() < len {
            self.next_generation += 1;
            self.generations.push(self.next_generation);
        }
        // Blocks from the stable prefix on changed: new generations.
        for slot in self.generations.iter_mut().skip(stable_prefix) {
            self.next_generation += 1;
            *slot = self.next_generation;
        }
        self.cache.borrow_mut().truncate(stable_prefix);
    }
}

impl MarkdownDocument {
    /// An empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// A document with `source`.
    pub fn from_source(source: &str) -> Self {
        let document = Self::new();
        document.replace(source);
        document
    }

    /// Appends streamed text (re-parses only the last block).
    pub fn append(&self, text: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.parser.append(text);
        let stable = inner.parser.stable_prefix_blocks();
        inner.sync_generations(stable);
        inner.revision += 1;
    }

    /// Replaces the whole source.
    pub fn replace(&self, source: &str) {
        let mut inner = self.inner.borrow_mut();
        if inner.parser.source() == source {
            return;
        }
        inner.parser.replace(source);
        inner.sync_generations(0);
        inner.revision += 1;
    }

    /// Feeds `source` as the new full text: an append when it extends the stored text, a
    /// replacement otherwise.
    pub fn set_source(&self, source: &str) {
        let mut inner = self.inner.borrow_mut();
        let current = inner.parser.source();
        if current == source {
            return;
        }
        match source.strip_prefix(current) {
            Some(delta) if !current.is_empty() => {
                let delta = delta.to_owned();
                inner.parser.append(&delta);
                let stable = inner.parser.stable_prefix_blocks();
                inner.sync_generations(stable);
            }
            _ => {
                inner.parser.replace(source);
                inner.sync_generations(0);
            }
        }
        inner.revision += 1;
    }

    /// The exact stored source (whole-document copy).
    pub fn source(&self) -> String {
        self.inner.borrow().parser.source().to_owned()
    }

    /// The canonical tree.
    pub fn tree(&self) -> Vec<Block> {
        self.inner.borrow().parser.tree().to_vec()
    }

    /// Bytes the last append or replace parsed.
    pub fn last_parse_bytes(&self) -> usize {
        self.inner.borrow().parser.last_parse_bytes()
    }

    /// Leading blocks guaranteed unchanged by the last append.
    pub fn stable_prefix_blocks(&self) -> usize {
        self.inner.borrow().parser.stable_prefix_blocks()
    }

    /// The flatten cache of this document.
    pub fn flatten_cache(&self) -> Rc<RefCell<FlattenCache>> {
        self.inner.borrow().cache.clone()
    }

    /// How many canonical parses ran so far.
    pub fn parse_count(&self) -> u64 {
        self.inner.borrow().parser.parse_count()
    }

    /// The content revision.
    pub fn revision(&self) -> u64 {
        self.inner.borrow().revision
    }

    /// The 1-based source line of top-level block `block`.
    pub fn block_source_line(&self, block: usize) -> Option<usize> {
        let inner = self.inner.borrow();
        let block = inner.parser.tree().get(block)?;
        Some(source_line(inner.parser.source(), block.range.start))
    }

    fn display_blocks(&self, streaming: bool) -> (Vec<Block>, Rc<Vec<u64>>) {
        let inner = self.inner.borrow();
        let blocks = if streaming {
            inner.parser.display_tree().to_vec()
        } else {
            inner.parser.tree().to_vec()
        };
        (blocks, Rc::new(inner.generations.clone()))
    }
}

/// The Markdown view. Construct with [`Markdown::new`] (a source text kept by the view) or
/// [`Markdown::document`] (an owner-held streaming document).
#[derive(IntoElement)]
pub struct Markdown {
    id: ElementId,
    source: Option<SharedString>,
    document: Option<MarkdownDocument>,
    options: MarkdownOptions,
    root: Stateful<Div>,
}

impl Markdown {
    /// A view of `source`; the parse persists while the source is unchanged and appends reparse
    /// only the last block.
    pub fn new(id: impl Into<ElementId>, source: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self {
            root: div().id(id.clone()),
            id,
            source: Some(source.into()),
            document: None,
            options: MarkdownOptions::default(),
        }
    }

    /// A view of an owner-held document.
    pub fn document(id: impl Into<ElementId>, document: MarkdownDocument) -> Self {
        let id = id.into();
        Self {
            root: div().id(id.clone()),
            id,
            source: None,
            document: Some(document),
            options: MarkdownOptions::default(),
        }
    }

    /// Replaces all options.
    pub fn options(mut self, options: MarkdownOptions) -> Self {
        self.options = options;
        self
    }

    /// The theme.
    pub fn theme(mut self, theme: vespertine_theme::document::DocumentTheme) -> Self {
        self.options.theme = theme;
        self
    }

    /// Routes link activations (validated destinations only) to `callback`.
    pub fn on_link(
        mut self,
        callback: impl Fn(&vespertine_richtext::LinkActivation, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.options.link_options = self.options.link_options.clone().on_activate(callback);
        self
    }

    /// A stable prefix for element identifiers (defaults to a key unique to the document).
    pub fn row_key(mut self, key: impl Into<SharedString>) -> Self {
        self.options.row_key = Some(key.into());
        self
    }

    /// Shows the display tree (hanging syntax of the last block repaired).
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.options.streaming = streaming;
        self
    }

    /// Fades the last `tail_bytes` bytes of the streaming block to `alpha` (paint only).
    pub fn streaming_fade(mut self, tail_bytes: usize, alpha: f32) -> Self {
        self.options.fade = Some(super::flatten::StreamingFade { tail_bytes, alpha });
        self
    }

    /// Whether the document takes part in selection (links and search keep working when off).
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.options.selectable = selectable;
        self
    }
}

impl Styled for Markdown {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

impl InteractiveElement for Markdown {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl StatefulInteractiveElement for Markdown {}

impl RenderOnce for Markdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let document = match (&self.document, &self.source) {
            (Some(document), _) => document.clone(),
            (None, Some(source)) => {
                let state =
                    window.use_keyed_state((self.id.clone(), "markdown-document"), cx, |_, _| {
                        MarkdownDocument::new()
                    });
                let document = state.read(cx).clone();
                document.set_source(source);
                document
            }
            (None, None) => MarkdownDocument::new(),
        };
        let theme = self.options.theme.clone();
        let (blocks, generations) = document.display_blocks(self.options.streaming);
        let mut options = self.options.clone();
        if options.row_key.is_none() {
            options.row_key = Some(document.inner.borrow().key.0.clone());
        }
        options.cache = Some(document.flatten_cache());
        options.generations = generations;
        let elements = render_blocks(&blocks, 0, &options, window, cx);
        let body = self
            .root
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme.metrics.md_block_gap))
            .children(elements);
        if self.options.selectable {
            body.into_any_element()
        } else {
            user_select(UserSelect::None, body).into_any_element()
        }
    }
}
