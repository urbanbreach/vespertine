//! The diff viewer element: the native unified patch viewer (R9-R19) and the split, annotated
//! presentation (R20-R29) over one parsed model.

use super::highlight::FileDocuments;
use super::render::{self, Palette};
use super::rows::{Annotation, DiffMode, RowMetrics, Side, SourceRef, scaled_text_metrics};
use super::state::{DiffViewState, Layout, Model, SyncInput};
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, ClickEvent, ElementId, Entity, ExternalPaths, FocusHandle, InteractiveElement,
    IntoElement, ListState, ParentElement, Refineable, RenderOnce, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, canvas, div, font, list, px,
};
use vespertine_richtext::syntax::{HighlightCache, SyntaxColors};
use vespertine_theme::document::DocumentTheme;

/// A click on a code line (R16, R19): the line text and the old and/or new numbers present, and
/// for the annotated surface the side-aware source reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineClick {
    /// Display path of the file.
    pub path: String,
    /// The line text.
    pub text: String,
    /// The old-side number, when the line has one.
    pub old: Option<u32>,
    /// The new-side number, when the line has one.
    pub new: Option<u32>,
    /// The side-aware source reference (`None` for meta lines).
    pub source: Option<SourceRef>,
}

/// The add callback payload (R28).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationAdd {
    /// Display path of the file.
    pub path: String,
    /// The side of the anchor.
    pub side: Side,
    /// The line number on that side.
    pub line: u32,
    /// The trimmed, non-empty body.
    pub body: String,
    /// The pre-rename path when the file was renamed.
    pub old_path: Option<String>,
}

type Callback<A> = Rc<dyn Fn(A, &mut Window, &mut App)>;

/// What the owner's draft editor needs (R28): the anchor, the initial body (when editing an
/// existing note), the focus handle the owner attaches to its editor (the viewer focuses it when
/// the draft opens) and the commit and cancel actions.
#[derive(Clone)]
pub struct DraftEditor {
    /// Display path of the file.
    pub path: String,
    /// The side of the anchor.
    pub side: Side,
    /// The line number on that side.
    pub line: u32,
    /// The existing note being edited, if any.
    pub editing: Option<String>,
    /// The body to start from.
    pub initial_body: String,
    /// Focus handle for the owner's editor element.
    pub focus: FocusHandle,
    pub(super) commit: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    pub(super) cancel: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl DraftEditor {
    /// Commits `body`: it is trimmed and an empty body is discarded; otherwise the add (or edit)
    /// callback runs. The draft closes either way.
    pub fn commit(&self, body: &str, window: &mut Window, cx: &mut App) {
        (self.commit)(body, window, cx);
    }

    /// Discards the draft.
    pub fn cancel(&self, window: &mut Window, cx: &mut App) {
        (self.cancel)(window, cx);
    }
}

pub(super) type DraftEditorFn = Rc<dyn Fn(&DraftEditor, &mut Window, &mut App) -> AnyElement>;

/// Shared properties and callbacks of one render.
pub(super) struct Props {
    pub theme: Arc<DocumentTheme>,
    pub word_diff: bool,
    pub collapsed: Option<Vec<String>>,
    pub wrap: bool,
    pub annotations: Vec<Annotation>,
    pub sticky: bool,
    pub frosted: bool,
    pub syntax: bool,
    pub on_toggle_file: Option<Callback<String>>,
    pub on_show_more: Option<Callback<usize>>,
    pub on_line_click: Option<Callback<LineClick>>,
    pub on_add: Option<Callback<AnnotationAdd>>,
    pub on_edit: Option<Rc<dyn Fn(&str, &str, &mut Window, &mut App)>>,
    pub on_remove: Option<Callback<String>>,
    pub draft_editor: Option<DraftEditorFn>,
}

/// What the rows of one render read.
pub(super) struct Frame {
    pub props: Rc<Props>,
    pub state: Entity<DiffViewState>,
    pub model: Rc<Model>,
    pub flat: Rc<super::rows::Flat>,
    pub metrics: RowMetrics,
    pub text_size: f32,
    pub palette: Palette,
    pub syntax: SyntaxColors,
    pub font: vespertine::Font,
    pub radius: Option<vespertine::Pixels>,
}

/// The diff viewer: parses a unified Git patch and shows it as virtual rows, unified or split.
///
/// The viewer is controlled: `toggleFile`, `showMore` and `lineClick` are reported through
/// callbacks and the owner decides whether to change `collapsed_paths` or `max_lines` (R19).
/// Text selection and search need a [`vespertine_richtext::text_root`] ancestor.
#[derive(IntoElement)]
pub struct DiffView {
    state: Entity<DiffViewState>,
    patch: Arc<str>,
    theme: Arc<DocumentTheme>,
    word_diff: bool,
    scroll: bool,
    max_lines: Option<usize>,
    file_line_cap: Option<usize>,
    collapsed: Option<Vec<String>>,
    mode: DiffMode,
    wrap: bool,
    code_font_size: Option<f32>,
    annotations: Vec<Annotation>,
    draft_key: Option<u64>,
    sticky: bool,
    frosted: bool,
    syntax: bool,
    syntax_colors: Option<SyntaxColors>,
    documents: Vec<FileDocuments>,
    cache: Arc<HighlightCache>,
    on_toggle_file: Option<Callback<String>>,
    on_show_more: Option<Callback<usize>>,
    on_line_click: Option<Callback<LineClick>>,
    on_add: Option<Callback<AnnotationAdd>>,
    on_edit: Option<Rc<dyn Fn(&str, &str, &mut Window, &mut App)>>,
    on_remove: Option<Callback<String>>,
    draft_editor: Option<DraftEditorFn>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
    on_hover: Option<Rc<dyn Fn(&bool, &mut Window, &mut App)>>,
    on_file_drop: Option<Rc<dyn Fn(&ExternalPaths, &mut Window, &mut App)>>,
    style: StyleRefinement,
}

impl DiffView {
    /// A viewer of an (empty) patch over `state`.
    pub fn new(state: &Entity<DiffViewState>) -> Self {
        Self {
            state: state.clone(),
            patch: Arc::from(""),
            theme: Arc::new(DocumentTheme::dark()),
            word_diff: false,
            scroll: false,
            max_lines: None,
            file_line_cap: None,
            collapsed: None,
            mode: DiffMode::Unified,
            wrap: false,
            code_font_size: None,
            annotations: Vec::new(),
            draft_key: None,
            sticky: false,
            frosted: false,
            syntax: true,
            syntax_colors: None,
            documents: Vec::new(),
            cache: HighlightCache::global(),
            on_toggle_file: None,
            on_show_more: None,
            on_line_click: None,
            on_add: None,
            on_edit: None,
            on_remove: None,
            draft_editor: None,
            on_click: None,
            on_hover: None,
            on_file_drop: None,
            style: StyleRefinement::default(),
        }
    }

    /// The unified patch text.
    pub fn patch(mut self, patch: impl Into<Arc<str>>) -> Self {
        self.patch = patch.into();
        self
    }

    /// The theme: color tokens, fonts and the diff metrics (THM-002).
    pub fn theme(mut self, theme: Arc<DocumentTheme>) -> Self {
        self.theme = theme;
        self
    }

    /// Word-level change washes (default off, R13).
    pub fn word_diff(mut self, enabled: bool) -> Self {
        self.word_diff = enabled;
        self
    }

    /// Internal virtualized scrolling (default off: rows are stacked and an enclosing element
    /// scrolls, R17). The viewer must be given a height.
    pub fn scroll(mut self, enabled: bool) -> Self {
        self.scroll = enabled;
        self
    }

    /// Caps line rows globally across files (R7); a "show more" row follows.
    pub fn max_lines(mut self, max_lines: Option<usize>) -> Self {
        self.max_lines = max_lines;
        self
    }

    /// Caps every file's hunks at this many diff lines (R5).
    pub fn file_line_cap(mut self, cap: Option<usize>) -> Self {
        self.file_line_cap = cap;
        self
    }

    /// Controls the collapsed files by display path (R6, R26). Without it the viewer tracks the
    /// collapsed state itself from header activation.
    pub fn collapsed_paths(mut self, paths: Vec<String>) -> Self {
        self.collapsed = Some(paths);
        self
    }

    /// Unified or split presentation (R20).
    pub fn mode(mut self, mode: DiffMode) -> Self {
        self.mode = mode;
        self
    }

    /// Wraps long lines instead of scrolling each file horizontally (R22).
    pub fn wrap_lines(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// The owner's shared code font size: the diff text size becomes
    /// `clamp(size x 12 / 12.5)` and the line height `text size x 21 / 12` (R23).
    pub fn code_font_size(mut self, size: f32) -> Self {
        self.code_font_size = Some(size);
        self
    }

    /// The review annotations (R27).
    pub fn annotations(mut self, annotations: Vec<Annotation>) -> Self {
        self.annotations = annotations;
        self
    }

    /// The owner's draft context: a changed key discards the draft (R28).
    pub fn draft_context_key(mut self, key: impl std::hash::Hash) -> Self {
        let mut hasher = std::hash::DefaultHasher::new();
        key.hash(&mut hasher);
        self.draft_key = Some(std::hash::Hasher::finish(&hasher));
        self
    }

    /// Sticky file headers (R25; scrolling viewers only).
    pub fn sticky_headers(mut self, enabled: bool) -> Self {
        self.sticky = enabled;
        self
    }

    /// Paints the sticky header with the translucent (frost) treatment: a 16 px backdrop blur and
    /// a tint of the background (R25). Opaque themes get the flattened fills.
    pub fn frosted(mut self, frosted: bool) -> Self {
        self.frosted = frosted;
        self
    }

    /// Syntax coloration on or off (default on, R14).
    pub fn syntax(mut self, enabled: bool) -> Self {
        self.syntax = enabled;
        self
    }

    /// A syntax palette instead of the theme's.
    pub fn syntax_colors(mut self, colors: SyntaxColors) -> Self {
        self.syntax_colors = Some(colors);
        self
    }

    /// Full old and new documents per file for whole-document highlighting (R15).
    pub fn documents(mut self, documents: Vec<FileDocuments>) -> Self {
        self.documents = documents;
        self
    }

    /// A highlight cache instead of the global one.
    pub fn cache(mut self, cache: Arc<HighlightCache>) -> Self {
        self.cache = cache;
        self
    }

    /// `toggleFile`: a header was activated; the value is the display path (R19).
    pub fn on_toggle_file(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle_file = Some(Rc::new(move |path: String, window, cx| {
            f(&path, window, cx)
        }));
        self
    }

    /// `showMore`: the show-more row was activated; the value is the remaining count (R19).
    pub fn on_show_more(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_show_more = Some(Rc::new(f));
        self
    }

    /// `lineClick`: a line row was activated (R16, R19).
    pub fn on_line_click(
        mut self,
        f: impl Fn(&LineClick, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_line_click = Some(Rc::new(move |event: LineClick, window, cx| {
            f(&event, window, cx)
        }));
        self
    }

    /// Annotation callback `add` (R28). With [`DiffView::draft_editor`] it enables the add
    /// affordance.
    pub fn on_add_annotation(
        mut self,
        f: impl Fn(&AnnotationAdd, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_add = Some(Rc::new(move |add: AnnotationAdd, window, cx| {
            f(&add, window, cx)
        }));
        self
    }

    /// Annotation callback `edit(id, new body)` (R28).
    pub fn on_edit_annotation(
        mut self,
        f: impl Fn(&str, &str, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_edit = Some(Rc::new(f));
        self
    }

    /// Annotation callback `remove(id)` (R28).
    pub fn on_remove_annotation(
        mut self,
        f: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_remove = Some(Rc::new(move |id: String, window, cx| f(&id, window, cx)));
        self
    }

    /// The owner-supplied editor of a draft row (R28): a callback returning an element. It
    /// attaches `DraftEditor::focus` to its editor (the viewer focuses it when the draft
    /// opens) and calls [`DraftEditor::commit`] or [`DraftEditor::cancel`]. Escape cancels the
    /// draft before any focused descendant sees the key.
    pub fn draft_editor(
        mut self,
        f: impl Fn(&DraftEditor, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.draft_editor = Some(Rc::new(f));
        self
    }

    /// Host event: click on the viewer surface.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// Host event: the pointer entered (`true`) or left (`false`) the viewer.
    pub fn on_hover(mut self, f: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(f));
        self
    }

    /// Host event: files were dropped on the viewer.
    pub fn on_file_drop(
        mut self,
        f: impl Fn(&ExternalPaths, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_file_drop = Some(Rc::new(f));
        self
    }
}

impl Styled for DiffView {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

fn corner_radius(style: &StyleRefinement, window: &Window) -> Option<vespertine::Pixels> {
    let top_left = style.corner_radii.top_left?;
    let radius = top_left.to_pixels(window.rem_size());
    (radius > px(0.)).then_some(radius)
}

impl RenderOnce for DiffView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme.clone();
        let (text_size, line_height) = match self.code_font_size {
            Some(size) => scaled_text_metrics(size),
            None => (theme.metrics.diff_text_size, theme.metrics.diff_line_height),
        };
        let metrics = RowMetrics::from_theme(&theme.metrics).with_line_height(line_height);
        let mono = font(theme.font_mono.clone());
        let layout = Layout {
            mode: self.mode,
            wrap: self.wrap,
            text_size,
            font: mono.clone(),
            metrics,
        };
        let state = self.state.clone();
        let owner = window.current_view();
        let collapsed = self.collapsed.clone();
        let input = SyncInput {
            patch: &self.patch,
            file_cap: self.file_line_cap,
            documents: &self.documents,
            cache: &self.cache,
            word_diff: self.word_diff,
            max_lines: self.max_lines,
            collapsed: collapsed.as_deref(),
            mode: self.mode,
            wrap: self.wrap,
            annotations: &self.annotations,
            draft_key: self.draft_key,
            layout,
            scroll: self.scroll,
        };
        let scroll = self.scroll;
        state.update(cx, |state, cx| {
            state.owner = Some(owner);
            state.sync(&input, window, cx);
            if scroll && !state.scroll_wired {
                state.scroll_wired = true;
                let weak = cx.weak_entity();
                state.list.set_scroll_handler(move |_, _, cx| {
                    weak.update(cx, |state, cx| state.changed(cx)).ok();
                });
            }
        });

        let radius = corner_radius(&self.style, window);
        let palette = Palette::new(&theme);
        let plain = palette.text.alpha(0.92);
        let syntax = self
            .syntax_colors
            .map(|colors| colors.with_plain(plain))
            .unwrap_or_else(|| SyntaxColors::from_palette(&theme.syntax, plain));
        let props = Rc::new(Props {
            theme: theme.clone(),
            word_diff: self.word_diff,
            collapsed: self.collapsed,
            wrap: self.wrap,
            annotations: self.annotations,
            sticky: self.sticky,
            frosted: self.frosted,
            syntax: self.syntax,
            on_toggle_file: self.on_toggle_file,
            on_show_more: self.on_show_more,
            on_line_click: self.on_line_click,
            on_add: self.on_add,
            on_edit: self.on_edit,
            on_remove: self.on_remove,
            draft_editor: self.draft_editor,
        });
        let (model, flat, focus_draft, draft_focus) = {
            let s = state.read(cx);
            (
                s.model.clone(),
                s.flat.clone(),
                s.focus_draft,
                s.draft_focus.clone(),
            )
        };
        if focus_draft {
            state.update(cx, |s, _| s.focus_draft = false);
            window.on_next_frame(move |window, cx| draft_focus.focus(window, cx));
        }
        let frame = Rc::new(Frame {
            props,
            state: state.clone(),
            model,
            flat,
            metrics,
            text_size,
            palette,
            syntax,
            font: mono,
            radius,
        });

        let mut root = div()
            .id(ElementId::from(("diff-view", state.entity_id())))
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(frame.palette.bg)
            .font_family(theme.font_sans.clone())
            .text_size(px(text_size))
            .text_color(frame.palette.text)
            .line_height(px(line_height));
        root.style().refine(&self.style);
        let escape_state = state.clone();
        root = root.capture_key_down(move |event, _window, cx| {
            let key = &event.keystroke;
            if key.key == "escape"
                && !key.modifiers.modified()
                && escape_state.read(cx).draft.is_some()
            {
                escape_state.update(cx, |state, cx| state.cancel_draft(cx));
                cx.stop_propagation();
            }
        });
        if let Some(handler) = self.on_click {
            root = root.on_click(move |event, window, cx| handler(event, window, cx));
        }
        if let Some(handler) = self.on_hover {
            root = root.on_hover(move |hovered, window, cx| handler(hovered, window, cx));
        }
        if let Some(handler) = self.on_file_drop {
            root =
                root.on_drop(move |paths: &ExternalPaths, window, cx| handler(paths, window, cx));
        }

        let width_state = state.clone();
        let tracker = canvas(
            move |bounds, _window, cx| {
                let width = f32::from(bounds.size.width);
                if (width_state.read(cx).width - width).abs() > 0.01 {
                    width_state.update(cx, |state, _| state.width = width);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        root = root.child(tracker);

        if frame.flat.rows.is_empty() {
            return root
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(frame.palette.text_faint)
                        .child("No changes"),
                )
                .into_any_element();
        }

        if self.scroll {
            let list_state: ListState = state.read(cx).list.clone();
            let row_frame = frame.clone();
            let rows = list(list_state, move |ix, window, cx| {
                render::render_row(&row_frame, ix, window, cx)
            })
            .size_full();
            root = root.child(rows);
            if frame.props.sticky {
                if let Some(sticky) = render::sticky_header(&frame, window, cx) {
                    root = root.child(sticky);
                }
            }
        } else {
            let mut column = div().flex().flex_col().w_full();
            for ix in 0..frame.flat.rows.len() {
                column = column.child(render::render_row(&frame, ix, window, cx));
            }
            root = root.child(column);
        }
        root.into_any_element()
    }
}
