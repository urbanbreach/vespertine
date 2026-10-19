//! The editable code surface (CMP-014 R10-R14).
//!
//! A [`CodeEditor`] is an entity that wraps the multiline editor of CMP-008 ([`TextArea`]): the
//! caret, range selection (mouse, keyboard, double-click word, triple-click line), the editing
//! actions with undo and redo, native composition and the context-menu event come from it. The
//! code editor adds the code typography, theme-derived style tokens, an optional line-number
//! gutter (on by default, folding off), optional soft wrap (off by default: long lines scroll
//! horizontally), read-only mode, owner-supplied syntax spans that follow edits without
//! recomputation, baseline replacement that keeps the selection and scroll offset, the task
//! marker edit and the line-navigation and overlay hooks.
//!
//! Colors never change advances: a syntax run only carries a color, so a highlighter that
//! completes later cannot reflow rows.

use super::block::{GutterLine, LineBounds, LineClick};
use super::lines::gutter_width;
use super::spans::{CodeHighlighter, SpanHighlighter, TextEdit};
use super::typography::{CodeTypography, TypographyOverrides};
use crate::doc_theme::{alpha, hsla, ink_alpha, syntax_colors};
use crate::text::{EditMenuState, FieldTheme, TextArea, TextAreaEvent, area::SpanStyle};
use std::ops::Range;
use std::rc::Rc;
use vespertine::{
    AnyElement, App, AppContext, AvailableSpace, Bounds, ContentMask, Context, DispatchPhase,
    Entity, EventEmitter, Focusable, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, Render, ShapedLine, SharedString, Styled,
    Subscription, TextAlign, TextRun, WeakEntity, Window, canvas, div, fill, point, px, size,
};
use vespertine_richtext::syntax::{HighlightCache, HighlighterAdapter, LanguageHint};
use vespertine_theme::document::DocumentTheme;

/// Default height bounds of the editor in rows.
pub const DEFAULT_ROWS: (usize, usize) = (3, 30);

type GutterFn = Rc<dyn Fn(usize) -> GutterLine>;
type OverlayFn = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type ClickFn = Rc<dyn Fn(&LineClick, &mut Window, &mut App)>;
type MenuFn = Rc<dyn Fn(&ContextMenuRequest, &mut Window, &mut App)>;

/// Style tokens of the editor (R13). Every field overrides the theme-derived default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorStyle {
    /// Text color (default: the text token at alpha 0.93).
    pub foreground: Option<Hsla>,
    /// Muted text such as line numbers (default: the faint-text token).
    pub muted_foreground: Option<Hsla>,
    /// Surface fill (default: transparent).
    pub background: Option<Hsla>,
    /// Border (default: the border token).
    pub border: Option<Hsla>,
    /// Selection wash (default: the accent token at alpha 0.22).
    pub selection: Option<Hsla>,
    /// Caret (default: the caret token).
    pub caret: Option<Hsla>,
    /// Wash behind the line with the caret (default: ink at alpha 0.025).
    pub active_line: Option<Hsla>,
    /// Gutter fill (default: transparent).
    pub gutter_background: Option<Hsla>,
}

/// [`EditorStyle`] with every token resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedEditorStyle {
    /// Text color.
    pub foreground: Hsla,
    /// Muted text color.
    pub muted_foreground: Hsla,
    /// Surface fill.
    pub background: Hsla,
    /// Border.
    pub border: Hsla,
    /// Selection wash.
    pub selection: Hsla,
    /// Caret.
    pub caret: Hsla,
    /// Active-line wash.
    pub active_line: Hsla,
    /// Gutter fill.
    pub gutter_background: Hsla,
}

impl EditorStyle {
    /// The tokens with `theme` supplying every default.
    pub fn resolve(&self, theme: &DocumentTheme) -> ResolvedEditorStyle {
        let transparent = alpha(hsla(theme.bg), 0.);
        ResolvedEditorStyle {
            foreground: self
                .foreground
                .unwrap_or_else(|| alpha(hsla(theme.text), 0.93)),
            muted_foreground: self
                .muted_foreground
                .unwrap_or_else(|| hsla(theme.text_faint)),
            background: self.background.unwrap_or(transparent),
            border: self.border.unwrap_or_else(|| hsla(theme.border)),
            selection: self
                .selection
                .unwrap_or_else(|| alpha(hsla(theme.accent), 0.22)),
            caret: self.caret.unwrap_or_else(|| hsla(theme.caret)),
            active_line: self.active_line.unwrap_or_else(|| ink_alpha(theme, 0.025)),
            gutter_background: self.gutter_background.unwrap_or(transparent),
        }
    }
}

/// What the context-menu callback receives (R13): the pointer position and the capabilities
/// the owner's menu needs.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuRequest {
    /// The pointer position in window coordinates.
    pub position: Point<Pixels>,
    /// Whether the editor accepts edits (not read-only).
    pub editable: bool,
    /// Whether a non-empty selection exists.
    pub has_selection: bool,
    /// Which menu commands are available.
    pub menu: EditMenuState,
}

/// Events of a [`CodeEditor`].
#[derive(Clone, Debug)]
pub enum CodeEditorEvent {
    /// The value changed through an edit (typing, paste, cut, delete, undo, redo, the task-marker
    /// edit). Replacing the content as a new baseline never emits it.
    Changed(String),
    /// The content was replaced as a new baseline ([`CodeEditor::set_value`],
    /// [`CodeEditor::replace_contents`]): not a user edit, neither undoable nor eligible for
    /// autosave.
    BaselineReplaced(String),
    /// The text changed: the old byte range replaced and the new byte length inserted.
    Edited {
        /// The replaced range of the previous text.
        range: Range<usize>,
        /// Length in bytes of the text that replaced it.
        inserted: usize,
    },
    /// The caret or selection moved.
    CursorMoved,
    /// The scroll offset, viewport or content size changed.
    ViewportChanged,
    /// A secondary press asks for a context menu.
    ContextMenu(ContextMenuRequest),
    /// A line or its number was clicked (R7).
    LineClick(LineClick),
    /// The editor gained focus.
    Focused,
    /// The editor lost focus.
    Blurred,
}

/// The editable code surface. Construct with [`CodeEditor::new`] inside `cx.new`.
pub struct CodeEditor {
    area: Entity<TextArea>,
    theme: DocumentTheme,
    typography: TypographyOverrides,
    style: EditorStyle,
    language: Option<String>,
    path: Option<String>,
    line_numbers: bool,
    soft_wrap: bool,
    highlighter: Option<Box<dyn CodeHighlighter>>,
    /// The edit epoch of the text the installed highlighter describes.
    highlighter_epoch: u64,
    gutter: Option<GutterFn>,
    overlays: Vec<(usize, OverlayFn)>,
    on_line_click: Option<ClickFn>,
    on_context_menu: Option<MenuFn>,
    /// Baseline replacements whose `Changed` event has not been seen yet.
    baseline_pending: usize,
    _subscription: Subscription,
}

impl EventEmitter<CodeEditorEvent> for CodeEditor {}

impl CodeEditor {
    /// An empty, editable editor with line numbers, no soft wrap and the dark document theme.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let area = cx.new(|cx| {
            TextArea::new(window, cx)
                .wrap(false)
                .rows(DEFAULT_ROWS.0, DEFAULT_ROWS.1)
                .insert_tabs(true)
        });
        let subscription = cx.subscribe_in(
            &area,
            window,
            |this, _, event: &TextAreaEvent, window, cx| {
                this.on_area_event(event, window, cx);
            },
        );
        let mut editor = Self {
            area,
            theme: DocumentTheme::dark(),
            typography: TypographyOverrides::default(),
            style: EditorStyle::default(),
            language: None,
            path: None,
            line_numbers: true,
            soft_wrap: false,
            highlighter: None,
            highlighter_epoch: 0,
            gutter: None,
            overlays: Vec::new(),
            on_line_click: None,
            on_context_menu: None,
            baseline_pending: 0,
            _subscription: subscription,
        };
        editor.sync_area(cx);
        editor
    }

    // ---- configuration ------------------------------------------------------------------------

    /// The wrapped multiline editor (focus, actions and geometry queries).
    pub fn area(&self) -> &Entity<TextArea> {
        &self.area
    }

    /// Sets the theme: the tokens, typography defaults and syntax palette derive from it.
    pub fn set_theme(&mut self, theme: DocumentTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        self.sync_area(cx);
        self.refresh_spans(cx);
    }

    /// The theme.
    pub fn theme(&self) -> &DocumentTheme {
        &self.theme
    }

    /// Overrides the typography (family, weight, size, line height, plain color; R4).
    pub fn set_typography(&mut self, typography: TypographyOverrides, cx: &mut Context<Self>) {
        self.typography = typography;
        self.sync_area(cx);
        self.refresh_spans(cx);
    }

    /// The resolved typography.
    pub fn typography(&self) -> CodeTypography {
        CodeTypography::resolve(&self.typography, &self.theme)
    }

    /// Overrides style tokens (R13).
    pub fn set_style(&mut self, style: EditorStyle, cx: &mut Context<Self>) {
        self.style = style;
        self.sync_area(cx);
    }

    /// The resolved style tokens.
    pub fn resolved_style(&self) -> ResolvedEditorStyle {
        self.style.resolve(&self.theme)
    }

    /// Sets the language tag used by [`CodeEditor::highlight_from_language`].
    pub fn set_language(&mut self, language: Option<String>) {
        self.language = language;
    }

    /// The language tag.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Sets the path, the fallback of the language tag.
    pub fn set_path(&mut self, path: Option<String>) {
        self.path = path;
    }

    /// The path.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// Shows or hides the line-number gutter (default: shown).
    pub fn set_line_numbers(&mut self, show: bool, cx: &mut Context<Self>) {
        self.line_numbers = show;
        self.sync_area(cx);
    }

    /// Whether the gutter is shown.
    pub fn line_numbers(&self) -> bool {
        self.line_numbers
    }

    /// Sets soft wrap (default: off; long lines then scroll horizontally).
    pub fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.soft_wrap = wrap;
        self.area.update(cx, |area, cx| area.set_wrap(wrap, cx));
        cx.notify();
    }

    /// Whether long lines wrap.
    pub fn soft_wrap(&self) -> bool {
        self.soft_wrap
    }

    /// Sets the height bounds in rows (the editor grows with its content within them).
    pub fn set_rows(&mut self, min_rows: usize, max_rows: usize, cx: &mut Context<Self>) {
        self.area
            .update(cx, |area, cx| area.set_rows(min_rows, max_rows, cx));
        cx.notify();
    }

    /// Sets read-only mode: every edit action and programmatic user-edit path is suppressed,
    /// selection, select-all and copy keep working (R10).
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.area
            .update(cx, |area, cx| area.set_read_only(read_only, cx));
        cx.notify();
    }

    /// Whether the editor is read-only.
    pub fn is_read_only(&self, cx: &App) -> bool {
        self.area.read(cx).is_read_only()
    }

    /// Per-line gutter content and metadata (R7): a number override and a marker element.
    pub fn set_gutter(
        &mut self,
        gutter: impl Fn(usize) -> GutterLine + 'static,
        cx: &mut Context<Self>,
    ) {
        self.gutter = Some(Rc::new(gutter));
        cx.notify();
    }

    /// Anchors an overlay element (a review card, a navigation target, a decoration) to the line
    /// with zero-based index `line`: it is laid out over the line's bounds inside the viewport
    /// and never changes text advances.
    pub fn add_overlay(
        &mut self,
        line: usize,
        overlay: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) {
        self.overlays.push((line, Rc::new(overlay)));
        cx.notify();
    }

    /// Removes every overlay.
    pub fn clear_overlays(&mut self, cx: &mut Context<Self>) {
        self.overlays.clear();
        cx.notify();
    }

    /// Called when a line or its number is clicked with its source line (R7).
    pub fn set_on_line_click(
        &mut self,
        handler: impl Fn(&LineClick, &mut Window, &mut App) + 'static,
    ) {
        self.on_line_click = Some(Rc::new(handler));
    }

    /// The context-menu callback (R13): it receives the capabilities and the pointer position;
    /// the owner supplies the menu surface. It reaches the owner through a weak handle, so
    /// registering it creates no reference cycle and dropping the editor releases it.
    pub fn on_context_menu<T: 'static>(
        &mut self,
        owner: &Entity<T>,
        handler: impl Fn(&mut T, &ContextMenuRequest, &mut Window, &mut Context<T>) + 'static,
    ) {
        let owner: WeakEntity<T> = owner.downgrade();
        self.on_context_menu = Some(Rc::new(move |request, window, cx| {
            if let Some(owner) = owner.upgrade() {
                owner.update(cx, |owner, cx| handler(owner, request, window, cx));
            }
        }));
    }

    // ---- content ------------------------------------------------------------------------------

    /// The current text.
    pub fn value(&self, cx: &App) -> String {
        self.area.read(cx).source().to_owned()
    }

    /// The selection as byte offsets.
    pub fn selected_range(&self, cx: &App) -> Range<usize> {
        self.area.read(cx).selected_range()
    }

    /// Selects `range` (snapped to boundaries).
    pub fn set_selected_range(
        &mut self,
        range: Range<usize>,
        reversed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.area.update(cx, |area, cx| {
            area.set_selected_range(range, reversed, window, cx)
        });
    }

    /// The scroll offset: x is 0 while lines wrap.
    pub fn scroll_offset(&self, cx: &App) -> Point<Pixels> {
        let area = self.area.read(cx);
        point(area.scroll_offset_x(), area.scroll_offset())
    }

    /// Sets the scroll offset (clamped to the content).
    pub fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) {
        self.area.update(cx, |area, cx| {
            area.set_scroll_offset(offset.y, cx);
            area.set_scroll_offset_x(offset.x, cx);
        });
    }

    /// Replaces the whole document with `text` as a new baseline: not recorded as a user edit,
    /// so neither undoable nor eligible for autosave, and the selection and scroll start over.
    pub fn set_value(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.area.read(cx).source() != text;
        if changed {
            self.baseline_pending += 1;
        }
        self.area
            .update(cx, |area, cx| area.set_value(text, window, cx));
    }

    /// Replaces the content because the owner learned of a clean external change (R12): a new
    /// baseline (not a user edit, so neither undoable nor eligible for autosave) that keeps the
    /// selection and scroll offset: both are re-applied after the replacement.
    pub fn replace_contents(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (previous, reversed, scroll) = {
            let area = self.area.read(cx);
            (
                area.selected_range(),
                area.selection_reversed(),
                (area.scroll_offset_x(), area.scroll_offset()),
            )
        };
        let changed = self.area.read(cx).source() != text;
        if changed {
            self.baseline_pending += 1;
        }
        self.area.update(cx, |area, cx| {
            area.set_value(text, window, cx);
            let range = clamp_range(text, previous);
            area.set_selected_range(range, reversed, window, cx);
            area.set_scroll_offset(scroll.1, cx);
            area.set_scroll_offset_x(scroll.0, cx);
        });
    }

    /// Replaces `range` with `text` as one recorded edit (the user-edit path): refused (returns
    /// `false`) while the editor is read-only.
    pub fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.area.read(cx).is_read_only() {
            return false;
        }
        self.area
            .update(cx, |area, cx| area.replace_range(range, text, window, cx))
    }

    /// Toggles a task marker of a Markdown preview (R12): only when the editor is editable and
    /// its current text is identical to `snapshot`, the source the preview was parsed from, and
    /// the text at `marker` is exactly `[ ]` when it was reported unchecked or `[x]`/`[X]` when
    /// it was reported checked. The single inner character is replaced by `x` or a space through
    /// the ordinary atomic edit and change-event path, and the previous selection and scroll
    /// offset are restored afterwards. Returns whether the marker was edited.
    pub fn toggle_task(
        &mut self,
        snapshot: &str,
        marker: Range<usize>,
        checked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (previous, reversed, scroll) = {
            let area = self.area.read(cx);
            if area.is_read_only() || area.source() != snapshot {
                return false;
            }
            (
                area.selected_range(),
                area.selection_reversed(),
                (area.scroll_offset_x(), area.scroll_offset()),
            )
        };
        let Some(text) = snapshot.get(marker.clone()) else {
            return false;
        };
        let matches = if checked {
            text == "[x]" || text == "[X]"
        } else {
            text == "[ ]"
        };
        if !matches {
            return false;
        }
        let inner = marker.start + 1..marker.end - 1;
        let replacement = if checked { " " } else { "x" };
        self.area.update(cx, |area, cx| {
            let changed = area.replace_range(inner, replacement, window, cx);
            area.set_selected_range(previous, reversed, window, cx);
            area.set_scroll_offset(scroll.1, cx);
            area.set_scroll_offset_x(scroll.0, cx);
            changed
        })
    }

    /// Undo (a no-op while read-only).
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.area.update(cx, |area, cx| area.undo(window, cx))
    }

    /// Redo (a no-op while read-only).
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.area.update(cx, |area, cx| area.redo(window, cx))
    }

    /// Selects the whole text (also while read-only).
    pub fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.area.update(cx, |area, cx| area.select_all(window, cx));
    }

    /// Focuses the editor.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.area.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    // ---- syntax spans ---------------------------------------------------------------------------

    /// Installs `highlighter` only when `source`, the text it was computed from, equals the
    /// editor's current text (R11); returns whether it was installed.
    pub fn install_highlighter(
        &mut self,
        source: &str,
        highlighter: impl CodeHighlighter + 'static,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.area.read(cx).source() != source {
            return false;
        }
        self.highlighter = Some(Box::new(highlighter));
        self.highlighter_epoch = self.area.read(cx).core().edit_epoch();
        self.refresh_spans(cx);
        true
    }

    /// Removes the highlighter: the text shows plain.
    pub fn clear_highlighter(&mut self, cx: &mut Context<Self>) {
        self.highlighter = None;
        self.refresh_spans(cx);
    }

    /// Whether a highlighter is installed.
    pub fn has_highlighter(&self) -> bool {
        self.highlighter.is_some()
    }

    /// Computes spans for the current text from the shared highlight cache (TXT-007) with the
    /// language tag, else the path, and installs them. An unknown language, a missing grammar or
    /// an exceeded limit leaves plain text; returns whether spans were installed.
    pub fn highlight_from_language(&mut self, cx: &mut Context<Self>) -> bool {
        let source = self.area.read(cx).source().to_owned();
        let hint = LanguageHint {
            fence_label: self.language.as_deref(),
            path: self.path.as_deref(),
        };
        match HighlightCache::global().document_for(&source, hint) {
            Some(document) => {
                let highlighter = SpanHighlighter::from_document(&source, &document);
                self.install_highlighter(&source, highlighter, cx)
            }
            None => {
                self.clear_highlighter(cx);
                false
            }
        }
    }

    fn refresh_spans(&mut self, cx: &mut Context<Self>) {
        let spans = match &self.highlighter {
            None => Vec::new(),
            Some(highlighter) => {
                let typography = self.typography();
                let colors = syntax_colors(&self.theme).with_plain(typography.color);
                let resolver = |kind| colors.color_for(kind);
                let text_len = self.area.read(cx).source().len();
                highlighter
                    .styles(0..text_len, &resolver, typography.color)
                    .into_iter()
                    .filter(|run| run.color != typography.color)
                    .map(|run| {
                        (
                            run.range(),
                            SpanStyle {
                                color: Some(run.color),
                                ..SpanStyle::default()
                            },
                        )
                    })
                    .collect()
            }
        };
        self.area
            .update(cx, |area, cx| area.set_style_spans(spans, cx));
    }

    // ---- geometry -------------------------------------------------------------------------------

    /// The bounds of every laid-out line in window coordinates with its source line (R7, R14):
    /// the same terms as the code block's line bounds. Empty before the first frame.
    pub fn line_bounds(&self, cx: &App) -> Vec<LineBounds> {
        let area = self.area.read(cx);
        let Some(viewport) = area.viewport_bounds() else {
            return Vec::new();
        };
        let inset = area.content_inset_value();
        let left = viewport.left() + inset - area.scroll_offset_x();
        let width = (viewport.size.width - inset).max(px(0.));
        let mut lines: Vec<LineBounds> = Vec::new();
        for row in area.visual_rows() {
            let top = viewport.top() - area.scroll_offset() + row.top;
            if row.first_of_line {
                lines.push(LineBounds {
                    index: row.source_line,
                    source_line: self.source_line(row.source_line),
                    bounds: Bounds {
                        origin: point(left, top),
                        size: size(width, row.height),
                    },
                });
            } else if let Some(last) = lines.last_mut() {
                last.bounds.size.height = top + row.height - last.bounds.origin.y;
            }
        }
        lines
    }

    /// The source-line number at a window position: the line the position falls on (the text or
    /// the gutter), mapped through the gutter's number overrides.
    pub fn source_line_at(&self, position: Point<Pixels>, cx: &App) -> Option<usize> {
        let area = self.area.read(cx);
        let offset = area.index_for_point(position)?;
        let text = area.source();
        let line = text[..offset.min(text.len())].matches('\n').count();
        Some(self.source_line(line))
    }

    /// The availability of the context-menu commands (R10): cut = editable and a selection
    /// exists; copy = a selection exists; paste = editable and clipboard text exists; select-all
    /// always.
    pub fn menu_state(&self, cx: &App) -> EditMenuState {
        EditMenuState {
            select_all: true,
            ..self.area.read(cx).edit_menu_state(cx)
        }
    }

    fn source_line(&self, index: usize) -> usize {
        self.gutter
            .as_ref()
            .and_then(|gutter| gutter(index).number)
            .unwrap_or(index + 1)
    }

    // ---- internals --------------------------------------------------------------------------------

    fn line_count(&self, cx: &App) -> usize {
        self.area.read(cx).source().matches('\n').count() + 1
    }

    fn gutter_inset(&self, cx: &App) -> Pixels {
        if self.line_numbers {
            px(gutter_width(self.line_count(cx), &self.theme.metrics))
        } else {
            px(0.)
        }
    }

    /// Pushes typography, tokens and the gutter inset into the wrapped editor.
    fn sync_area(&mut self, cx: &mut Context<Self>) {
        let typography = self.typography();
        let style = self.resolved_style();
        let inset = self.gutter_inset(cx);
        let field = FieldTheme {
            text: style.foreground,
            faint: style.muted_foreground,
            muted: style.muted_foreground,
            selection: style.selection,
            caret: style.caret,
            surface: style.background,
            border: style.border,
            accent: hsla(self.theme.accent),
            disabled_surface: style.background,
        };
        let font = typography.font();
        self.area.update(cx, |area, cx| {
            area.set_theme(Some(field), cx);
            area.set_font(Some(font), cx);
            area.set_text_metrics(typography.size, typography.line_height, cx);
            area.set_content_inset(inset, cx);
            area.set_active_line_wash(Some(style.active_line), cx);
        });
        cx.notify();
    }

    fn on_area_event(
        &mut self,
        event: &TextAreaEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextAreaEvent::Changed(value) => {
                if self.baseline_pending > 0 {
                    self.baseline_pending -= 1;
                    cx.emit(CodeEditorEvent::BaselineReplaced(value.clone()));
                } else {
                    cx.emit(CodeEditorEvent::Changed(value.clone()));
                }
            }
            TextAreaEvent::Edited {
                range,
                inserted,
                epoch,
            } => {
                // Events arrive after the update that made the edit: an edit the installed
                // spans were already computed for is not applied again.
                if let Some(highlighter) = self.highlighter.as_mut()
                    && *epoch > self.highlighter_epoch
                {
                    highlighter.update(&TextEdit {
                        range: range.clone(),
                        inserted: *inserted,
                    });
                    self.highlighter_epoch = *epoch;
                    self.refresh_spans(cx);
                }
                let inset = self.gutter_inset(cx);
                if self.area.read(cx).content_inset_value() != inset {
                    self.area
                        .update(cx, |area, cx| area.set_content_inset(inset, cx));
                }
                cx.emit(CodeEditorEvent::Edited {
                    range: range.clone(),
                    inserted: *inserted,
                });
                cx.notify();
            }
            TextAreaEvent::CursorMoved => cx.emit(CodeEditorEvent::CursorMoved),
            TextAreaEvent::ViewportChanged => {
                cx.emit(CodeEditorEvent::ViewportChanged);
                cx.notify();
            }
            TextAreaEvent::ContextMenu(position) => {
                let area = self.area.read(cx);
                let request = ContextMenuRequest {
                    position: *position,
                    editable: !area.is_read_only(),
                    has_selection: !area.selected_range().is_empty(),
                    menu: self.menu_state(cx),
                };
                if let Some(handler) = self.on_context_menu.clone() {
                    handler(&request, window, cx);
                }
                cx.emit(CodeEditorEvent::ContextMenu(request));
            }
            TextAreaEvent::Focused => cx.emit(CodeEditorEvent::Focused),
            TextAreaEvent::Blurred => cx.emit(CodeEditorEvent::Blurred),
            _ => {}
        }
    }

    fn line_clicked(&mut self, click: LineClick, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handler) = self.on_line_click.clone() {
            handler(&click, window, cx);
        }
        cx.emit(CodeEditorEvent::LineClick(click));
    }
}

/// `range` clamped to `text` and moved onto character boundaries.
fn clamp_range(text: &str, range: Range<usize>) -> Range<usize> {
    let snap = |mut at: usize| {
        at = at.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    snap(range.start)..snap(range.end)
}

/// One gutter number prepared for painting.
struct Number {
    origin: Point<Pixels>,
    line: ShapedLine,
    marker: Option<AnyElement>,
}

/// What the gutter and overlay layer prepares for a frame.
#[derive(Default)]
struct LayerFrame {
    gutter: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    line_height: Pixels,
    numbers: Vec<Number>,
    overlays: Vec<AnyElement>,
    /// The visible lines: bounds in window coordinates, zero-based index, source line.
    lines: Vec<(Bounds<Pixels>, usize, usize)>,
}

impl Render for CodeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.resolved_style();
        let typography = self.typography();
        let area = self.area.clone();
        let editor = cx.weak_entity();
        let gutter = self.gutter.clone();
        let overlays = self.overlays.clone();
        let show_numbers = self.line_numbers;
        let padding_right = self.theme.metrics.code_gutter_padding_right;
        let layer_area = area.clone();
        let layer = canvas(
            move |_, window, cx| {
                prepare_layer(
                    &layer_area,
                    show_numbers,
                    gutter.as_deref(),
                    &typography,
                    style,
                    padding_right,
                    &overlays,
                    window,
                    cx,
                )
            },
            move |_, frame, window, cx| paint_layer(frame, editor, style, window, cx),
        )
        .absolute()
        .top_0()
        .left_0()
        .size_0();
        div()
            .id("vespertine-code-editor")
            .relative()
            .w_full()
            .child(area)
            .child(layer)
    }
}

/// Lays out the gutter numbers, markers and overlays over the lines the wrapped editor laid out
/// in this frame (the layer prepaints after the editor, so its geometry is current).
#[allow(clippy::too_many_arguments)]
fn prepare_layer(
    area: &Entity<TextArea>,
    show_numbers: bool,
    gutter: Option<&dyn Fn(usize) -> GutterLine>,
    typography: &CodeTypography,
    style: ResolvedEditorStyle,
    padding_right: f32,
    overlays: &[(usize, OverlayFn)],
    window: &mut Window,
    cx: &mut App,
) -> LayerFrame {
    let (viewport, inset, scroll_x, scroll_y, rows) = {
        let area = area.read(cx);
        let Some(viewport) = area.viewport_bounds() else {
            return LayerFrame::default();
        };
        (
            viewport,
            area.content_inset_value(),
            area.scroll_offset_x(),
            area.scroll_offset(),
            area.visual_rows(),
        )
    };
    let mut frame = LayerFrame {
        gutter: Bounds {
            origin: viewport.origin,
            size: size(inset, viewport.size.height),
        },
        viewport,
        line_height: typography.line_height,
        ..LayerFrame::default()
    };
    let font = typography.font();
    let text_left = viewport.left() + inset - scroll_x;
    let text_width = (viewport.size.width - inset).max(px(0.));
    // Rows of one logical line, merged: (line, top, height).
    let mut line_tops: Vec<(usize, Pixels, Pixels)> = Vec::new();
    for row in &rows {
        let top = viewport.top() - scroll_y + row.top;
        if row.first_of_line {
            line_tops.push((row.source_line, top, row.height));
        } else if let Some(last) = line_tops.last_mut() {
            last.2 = top + row.height - last.1;
        }
    }
    for (index, top, height) in line_tops {
        if top + height <= viewport.top() || top >= viewport.bottom() {
            continue;
        }
        let GutterLine { number, marker } = gutter.map(|gutter| gutter(index)).unwrap_or_default();
        let source_line = number.unwrap_or(index + 1);
        frame.lines.push((
            Bounds {
                origin: point(text_left, top),
                size: size(text_width, height),
            },
            index,
            source_line,
        ));
        if !show_numbers {
            continue;
        }
        let label: SharedString = source_line.to_string().into();
        let run = TextRun {
            len: label.len(),
            font: font.clone(),
            color: style.muted_foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(label, typography.size, &[run], None);
        let right = viewport.left() + inset - px(padding_right);
        let mut marker = marker;
        if let Some(marker) = marker.as_mut() {
            marker.prepaint_as_root(
                point(viewport.left(), top),
                size(
                    AvailableSpace::Definite(inset),
                    AvailableSpace::Definite(typography.line_height),
                ),
                window,
                cx,
            );
        }
        frame.numbers.push(Number {
            origin: point(right - line.width(), top),
            line,
            marker,
        });
    }
    for (line, overlay) in overlays {
        let Some((bounds, _, _)) = frame.lines.iter().find(|(_, index, _)| index == line) else {
            continue;
        };
        let mut element = overlay(window, cx);
        element.prepaint_as_root(
            bounds.origin,
            size(
                AvailableSpace::Definite(bounds.size.width),
                AvailableSpace::Definite(bounds.size.height),
            ),
            window,
            cx,
        );
        frame.overlays.push(element);
    }
    frame
}

fn paint_layer(
    mut frame: LayerFrame,
    editor: WeakEntity<CodeEditor>,
    style: ResolvedEditorStyle,
    window: &mut Window,
    cx: &mut App,
) {
    if frame.viewport.size.width <= px(0.) {
        return;
    }
    let line_height = frame.line_height;
    window.with_content_mask(
        Some(ContentMask {
            bounds: frame.viewport,
        }),
        |window| {
            if style.gutter_background.a > 0. && frame.gutter.size.width > px(0.) {
                window.paint_quad(fill(frame.gutter, style.gutter_background));
            }
            for number in &mut frame.numbers {
                let _ = number.line.paint(
                    number.origin,
                    line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                if let Some(marker) = number.marker.as_mut() {
                    marker.paint(window, cx);
                }
            }
            for overlay in &mut frame.overlays {
                overlay.paint(window, cx);
            }
        },
    );
    let (gutter, viewport, lines) = (frame.gutter, frame.viewport, frame.lines);
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || event.button != MouseButton::Left
            || event.click_count != 1
            || !viewport.contains(&event.position)
        {
            return;
        }
        let Some((_, index, source_line)) = lines
            .iter()
            .find(|(bounds, _, _)| {
                event.position.y >= bounds.top() && event.position.y < bounds.bottom()
            })
            .cloned()
        else {
            return;
        };
        let on_number = gutter.contains(&event.position);
        let Some(editor) = editor.upgrade() else {
            return;
        };
        editor.update(cx, |editor, cx| {
            let text: SharedString = editor
                .area
                .read(cx)
                .source()
                .split('\n')
                .nth(index)
                .unwrap_or("")
                .to_owned()
                .into();
            editor.line_clicked(
                LineClick {
                    index,
                    source_line,
                    text,
                    on_number,
                },
                window,
                cx,
            );
        });
    });
}
