//! The multiline text editor (CMP-008): wrapped source text with exact caret, selection and hit
//! geometry, auto-growing height followed by internal scrolling, native composition, a
//! source/display projection layer and a transactional path for external progressive text.

mod element;
pub mod external;
pub mod layers;
pub mod layout;
pub mod projection;
mod references;
pub mod runs;

pub use external::{ExternalOutcome, ExternalPhase, ExternalReplacement};
pub use layers::{AreaLayer, LayerEdit, LayerEnv, LayerIndent};
pub use layout::OverflowEdges;
pub use projection::{Projection, Replacement};
pub use references::{ChipHit, ChipPill, ChipRecord, HoverPopup, PasteRecord, icon_asset_path};
pub use runs::SpanStyle;

use super::{
    actions::{AREA_CONTEXT, bind_default_keys},
    boundaries::line_start,
    common::{Blink, ClipboardContent, EditMenuState, FieldTheme, PastePreference, TextMetrics},
    core::{Acknowledgement, Affinity, DragAnchor, EditCore, EditKind},
    editor::{EditorHost, now_ms},
};
use layout::{AreaLayout, LineShape, auto_height, clamp_scroll, follow_scroll, row_bounds};
use runs::{BaseStyle, SpanSweep, cut_spans, line_runs};
use std::{collections::HashMap, ops::Range, path::PathBuf, rc::Rc};
use vespertine::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    Font, Hsla, Image, KeyDownEvent, KeyUpEvent, Pixels, Point, Role, SharedString, Task, TextRun,
    UTF16Selection, Window, point, px, size,
};

/// Vertical space the chrome (padding and border) takes around the content: 2 x (4 + 1) px.
pub const CHROME_PADDING_Y: f32 = 10.0;
/// Horizontal space the chrome takes: 2 x (8 + 1) px.
pub const CHROME_PADDING_X: f32 = 18.0;

/// How Enter behaves (CMP-008 R7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubmitMode {
    /// Enter inserts a newline; the owner submits some other way.
    #[default]
    Enter,
    /// "Enter submits" mode: Enter emits Submitted, Shift+Enter inserts a newline.
    EnterSubmits,
    /// "Modified Enter submits" mode: Enter inserts a newline, Ctrl+Enter emits Submitted.
    ModifiedEnter,
}

/// One row segment of a span wash (window coordinates): painted as a rounded quad whose outer
/// corners are rounded where the span begins or ends and square where it continues on another row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WashQuad {
    /// The row segment's box.
    pub bounds: Bounds<Pixels>,
    /// The wash color.
    pub color: Hsla,
    /// Whether the left corners are rounded (the span begins here).
    pub round_left: bool,
    /// Whether the right corners are rounded (the span ends here).
    pub round_right: bool,
}

/// Corner radius of the code wash.
pub const WASH_RADIUS: f32 = 3.0;
/// Vertical inset of the code wash inside its row.
pub const WASH_INSET_Y: f32 = 1.0;

/// A visual row of the editor for gutters and overlays (content coordinates, before scrolling).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowInfo {
    /// Source byte offset of the row start.
    pub source_start: usize,
    /// Zero-based logical line of the row.
    pub source_line: usize,
    /// Whether this is the first row of its logical line.
    pub first_of_line: bool,
    /// Top of the row.
    pub top: Pixels,
    /// Height of the row.
    pub height: Pixels,
}

/// Events of a [`TextArea`] (CMP-007 R22 and CMP-008 R21).
#[derive(Clone, Debug)]
pub enum TextAreaEvent {
    /// The value changed through a user or input-method edit, undo or redo.
    Changed(String),
    /// The text changed: the old byte range replaced and the new byte length inserted.
    Edited {
        /// The replaced range of the previous text.
        range: Range<usize>,
        /// Length in bytes of the text that replaced it.
        inserted: usize,
        /// The core's edit epoch after the edit: events are delivered after the update that
        /// made them, so a listener that learned the text some other way compares epochs to
        /// tell the edits it has already seen.
        epoch: u64,
    },
    /// The caret or selection moved.
    CursorMoved,
    /// The scroll offset, viewport or content size changed.
    ViewportChanged,
    /// Enter (in a submit mode) or the submit chord was pressed outside a composition.
    Submitted(String),
    /// Escape was pressed outside a composition.
    Cancelled,
    /// External file paths were pasted; nothing was inserted.
    PastedPaths(Vec<PathBuf>),
    /// Images were pasted; nothing was inserted.
    PastedImages(Vec<Image>),
    /// The editor gained focus.
    Focused,
    /// The editor lost focus.
    Blurred,
    /// A key went down while the editor was focused.
    KeyDown(KeyDownEvent),
    /// A key was released while the editor was focused.
    KeyUp(KeyUpEvent),
    /// Enter was pressed while an anchored completion popup owns it (R7).
    AcceptCompletion,
    /// The external-replacement phase changed (R18g).
    ExternalPhaseChanged(ExternalPhase),
    /// A secondary press asks for a context menu at the window position.
    ContextMenu(Point<Pixels>),
    /// Text was pasted; the record lets the owner amend the pasted text inside the paste's own
    /// undo step with [`TextArea::amend_paste`] (CMP-010 R16).
    Pasted(PasteRecord),
    /// A press and release on an image chip without dragging activated it (CMP-010 R9); the
    /// payload is the chip's attachment number.
    ActivateReference(Option<u32>),
}

struct Drag {
    anchor: DragAnchor,
    position: Point<Pixels>,
}

#[derive(Clone)]
struct CachedLine {
    text: String,
    runs: Vec<TextRun>,
    width_bits: i32,
    indent_bits: i32,
    size_bits: i32,
    shape: Rc<LineShape>,
    /// The chips of the line as ranges of the line's display text.
    chips: Vec<Range<usize>>,
}

#[derive(Clone, PartialEq)]
struct LayoutKey {
    projection_epoch: u64,
    width_bits: i32,
    font: Font,
    size: Pixels,
    line_height: Pixels,
    color: Hsla,
    monospace: SharedString,
    padding: Font,
    marked: Option<Range<usize>>,
    edit_epoch: u64,
    wrap: bool,
}

#[derive(Clone, PartialEq)]
struct ProjectionKey {
    edit_epoch: u64,
    active: Range<usize>,
    marked: Option<Range<usize>>,
    layers: u64,
    manual: u64,
    env: LayerEnv,
}

/// The multiline text editor entity.
pub struct TextArea {
    pub(crate) core: EditCore,
    pub(crate) focus_handle: FocusHandle,
    placeholder: SharedString,
    disabled: bool,
    tab_stop: bool,
    role: Role,
    metrics: TextMetrics,
    theme: Option<FieldTheme>,
    font: Option<Font>,
    pub(crate) resolved_font: Font,
    min_rows: usize,
    max_rows: usize,
    pixel_bounds: Option<(Pixels, Pixels)>,
    submit_mode: SubmitMode,
    triple_click_selects_all: bool,
    copies_background_selection: bool,
    paste_preference: PastePreference,
    content_inset: Pixels,
    active_line_wash: Option<Hsla>,
    completion_open: bool,
    layers: Vec<Rc<dyn AreaLayer>>,
    manual_replacements: Vec<Replacement>,
    manual_spans: Vec<(Range<usize>, SpanStyle)>,
    manual_epoch: u64,
    projection: Projection,
    projection_key: Option<ProjectionKey>,
    projection_epoch: u64,
    display: String,
    display_spans: Vec<(Range<usize>, SpanStyle)>,
    layout: Option<Rc<AreaLayout>>,
    layout_key: Option<LayoutKey>,
    line_cache: HashMap<u64, Vec<Rc<CachedLine>>>,
    shape_count: usize,
    pub(crate) scroll_y: Pixels,
    /// Whether long lines wrap (the default); without wrapping the editor scrolls horizontally.
    wrap: bool,
    pub(crate) scroll_x: Pixels,
    content_width: Pixels,
    follow: bool,
    preferred_x: Option<Pixels>,
    settled_height: Option<Pixels>,
    pub(crate) bounds: Option<Bounds<Pixels>>,
    measured_height: Pixels,
    last_viewport: Option<(Pixels, Pixels, Pixels, Pixels)>,
    pub(crate) blink: Blink,
    drag: Option<Drag>,
    pointer_generation: u64,
    autoscroll: Option<Task<()>>,
    external: ExternalReplacement,
    refs: references::ReferenceState,
    _subscriptions: Vec<vespertine::Subscription>,
}

impl EventEmitter<TextAreaEvent> for TextArea {}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn display_width_bits(width: Pixels) -> i32 {
    (f32::from(width) * 100.0).round() as i32
}

fn hash_line(text: &str, width: i32, indent: i32, size: i32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    width.hash(&mut hasher);
    indent.hash(&mut hasher);
    size.hash(&mut hasher);
    hasher.finish()
}

impl TextArea {
    /// An empty, enabled, editable multiline editor (1 to 10 rows, 16 px text, 20 px lines).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        Self::with_focus_handle(focus_handle, window, cx)
    }

    /// Like [`TextArea::new`] but focused through `focus_handle`, so an owner whose own node
    /// already holds the handle (a retained host element) shares one focus identity with the
    /// editor.
    pub fn with_focus_handle(
        focus_handle: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        bind_default_keys(cx);
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _window, cx| {
                this.blink.reset(cx);
                cx.emit(TextAreaEvent::Focused);
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |this, _window, cx| {
                this.end_drag();
                this.core.cancel_marking();
                cx.emit(TextAreaEvent::Blurred);
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, _window, cx| {
                this.blink.reset(cx);
                cx.notify();
            }),
        ];
        Self {
            core: EditCore::new("", true),
            focus_handle,
            placeholder: SharedString::default(),
            disabled: false,
            tab_stop: true,
            role: Role::MultilineTextInput,
            metrics: TextMetrics::default(),
            theme: None,
            font: None,
            resolved_font: Font::default(),
            min_rows: 1,
            max_rows: 10,
            pixel_bounds: None,
            submit_mode: SubmitMode::Enter,
            triple_click_selects_all: false,
            copies_background_selection: false,
            paste_preference: PastePreference::TextFirst,
            content_inset: px(0.0),
            active_line_wash: None,
            completion_open: false,
            layers: Vec::new(),
            manual_replacements: Vec::new(),
            manual_spans: Vec::new(),
            manual_epoch: 0,
            projection: Projection::identity(),
            projection_key: None,
            projection_epoch: 0,
            display: String::new(),
            display_spans: Vec::new(),
            layout: None,
            layout_key: None,
            line_cache: HashMap::new(),
            shape_count: 0,
            scroll_y: px(0.0),
            wrap: true,
            scroll_x: px(0.0),
            content_width: px(0.0),
            follow: true,
            preferred_x: None,
            settled_height: None,
            bounds: None,
            measured_height: px(0.0),
            last_viewport: None,
            blink: Blink::new(cx),
            drag: None,
            pointer_generation: 0,
            autoscroll: None,
            external: ExternalReplacement::new(),
            refs: references::ReferenceState::default(),
            _subscriptions: subscriptions,
        }
    }

    // ---- builders ---------------------------------------------------------------------------

    /// Whether long lines wrap to the width (the default). Without wrapping every logical line is
    /// one row and the editor scrolls horizontally, following the caret.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Sets the placeholder shown while the value is empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Sets the initial value; the caret ends at its end.
    pub fn value(mut self, value: &str) -> Self {
        self.core.set_value(value);
        self
    }

    /// Sets the read-only flag.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.core.set_read_only(read_only);
        self
    }

    /// Sets the disabled flag.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.sync_tab_stop();
        self
    }

    /// Sets the text size and line height.
    pub fn text_metrics(mut self, size: Pixels, line_height: Pixels) -> Self {
        self.metrics = TextMetrics { size, line_height };
        self
    }

    /// Sets the height bounds in rows: `min_rows` is clamped to at least 1 and `max_rows` to at
    /// least `min_rows` (R11).
    pub fn rows(mut self, min_rows: usize, max_rows: usize) -> Self {
        self.min_rows = min_rows.max(1);
        self.max_rows = max_rows.max(self.min_rows);
        self
    }

    /// Sets the minimum rows (at least 1).
    pub fn min_rows(mut self, min_rows: usize) -> Self {
        self.min_rows = min_rows.max(1);
        self.max_rows = self.max_rows.max(self.min_rows);
        self
    }

    /// Sets the maximum rows (at least the minimum).
    pub fn max_rows(mut self, max_rows: usize) -> Self {
        self.max_rows = max_rows.max(self.min_rows);
        self
    }

    /// Sets explicit pixel bounds of the editor's CONTENT height (below the chrome).
    pub fn height_bounds(mut self, min: Pixels, max: Pixels) -> Self {
        self.pixel_bounds = Some((min, max.max(min)));
        self
    }

    /// Sets the Enter behavior (R7).
    pub fn submit_mode(mut self, mode: SubmitMode) -> Self {
        self.submit_mode = mode;
        self
    }

    /// Makes a third click select the entire value instead of the logical line (R8).
    pub fn triple_click_selects_all(mut self, all: bool) -> Self {
        self.triple_click_selects_all = all;
        self
    }

    /// Lets Copy fall back to the document selection when the editor's own selection is empty
    /// (R20); the default is no fallback.
    pub fn copies_background_selection(mut self, copies: bool) -> Self {
        self.copies_background_selection = copies;
        self
    }

    /// Whether the enabled editor is a tab stop (default true).
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self.sync_tab_stop();
        self
    }

    /// Inserts a tab character when Tab arrives as text (code editors); by default Tab traverses
    /// focus.
    pub fn insert_tabs(mut self, insert: bool) -> Self {
        self.core.set_allow_tab_text(insert);
        self
    }

    /// Sets the accessibility role (default multiline text input).
    pub fn accessibility_role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    /// Overrides the theme roles.
    pub fn theme(mut self, theme: FieldTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Overrides the font; by default the inherited text font.
    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Chooses between text and media on paste (PLT-009 R4).
    pub fn paste_preference(mut self, preference: PastePreference) -> Self {
        self.paste_preference = preference;
        self
    }

    /// Reserves `left` pixels at the left of the content for a gutter the owner paints.
    pub fn content_inset(mut self, left: Pixels) -> Self {
        self.content_inset = left;
        self
    }

    /// A wash painted behind the logical line holding the caret.
    pub fn active_line_wash(mut self, wash: Option<Hsla>) -> Self {
        self.active_line_wash = wash;
        self
    }

    /// Turns on source-preserving Markdown editing (CMP-009): styling, display decorations,
    /// fenced-code coloring, list continuation on Enter and list indent on Tab.
    pub fn with_markdown(self, options: crate::markdown::MarkdownOptions) -> Self {
        self.with_layer(Rc::new(crate::markdown::MarkdownLayer::new(options)))
    }

    /// Attaches a layer (Markdown, chips): its replacements, style spans and editing hooks join
    /// the editor.
    pub fn with_layer(mut self, layer: Rc<dyn AreaLayer>) -> Self {
        self.layers.push(layer);
        self
    }

    // ---- accessors ---------------------------------------------------------------------------

    /// The source text (R1).
    pub fn source(&self) -> &str {
        self.core.text()
    }

    /// Whether the value is empty.
    pub fn is_empty(&self) -> bool {
        self.core.text().is_empty()
    }

    /// The selection as UTF-8 byte offsets.
    pub fn selected_range(&self) -> Range<usize> {
        self.core.selection()
    }

    /// The caret (active selection end).
    pub fn cursor_offset(&self) -> usize {
        self.core.cursor()
    }

    /// Whether the caret sits at the selection start.
    pub fn selection_reversed(&self) -> bool {
        self.core.is_reversed()
    }

    /// The marked (composing) range in UTF-8.
    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.core.marked()
    }

    /// The editing core (selection model and history), for inspection.
    pub fn core(&self) -> &EditCore {
        &self.core
    }

    /// Whether the editor is read-only.
    pub fn is_read_only(&self) -> bool {
        self.core.is_read_only()
    }

    /// Whether the editor is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The text metrics.
    pub fn metrics(&self) -> TextMetrics {
        self.metrics
    }

    /// The placeholder.
    pub fn placeholder_text(&self) -> &SharedString {
        &self.placeholder
    }

    /// The current projection.
    pub fn projection(&self) -> &Projection {
        &self.projection
    }

    /// The projected display text.
    pub fn display_text(&self) -> &str {
        &self.display
    }

    /// How many logical lines were reshaped so far (retained layout reuse).
    pub fn shape_count(&self) -> usize {
        self.shape_count
    }

    /// The vertical scroll offset.
    pub fn scroll_offset(&self) -> Pixels {
        self.scroll_y
    }

    /// The height of all wrapped rows, at least one line (R3).
    pub fn content_height(&self) -> Pixels {
        self.layout
            .as_ref()
            .map(|layout| layout.content_height)
            .unwrap_or(self.metrics.line_height)
    }

    /// The width of the widest laid-out row.
    pub fn measured_text_width(&self) -> Pixels {
        self.layout
            .as_ref()
            .map(|layout| {
                layout
                    .lines
                    .iter()
                    .map(|line| line.shape.max_width())
                    .fold(px(0.0), Pixels::max)
            })
            .unwrap_or_default()
    }

    /// The generation of the current pointer gesture: a drag autoscroll tick with an older
    /// generation does nothing (R9).
    pub fn pointer_generation(&self) -> u64 {
        self.pointer_generation
    }

    /// The height the editor asked its parent for at the last layout (R11).
    pub fn measured_height(&self) -> Pixels {
        self.measured_height
    }

    /// The content box of the last frame, in window coordinates.
    pub fn viewport_bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
    }

    /// The enablement of Cut, Copy, Paste and Select All (CMP-007 R24).
    pub fn edit_menu_state(&self, cx: &App) -> EditMenuState {
        self.menu_state(cx)
    }

    /// The height bounds (min, max) of the content box in pixels (R11).
    pub fn content_height_bounds(&self) -> (Pixels, Pixels) {
        match self.pixel_bounds {
            Some(bounds) => bounds,
            None => row_bounds(self.min_rows, self.max_rows, self.metrics.line_height),
        }
    }

    /// The viewport height scrolling is computed against: the settled height an owner animates
    /// toward, else the current one (R13).
    fn settled_viewport(&self) -> Pixels {
        self.settled_height
            .or_else(|| self.bounds.map(|bounds| bounds.size.height))
            .unwrap_or(self.measured_height)
    }

    /// Sets the settled (target) viewport height of an owner-driven height animation; `None`
    /// ends it (R13).
    pub fn set_settled_viewport_height(&mut self, height: Option<Pixels>, cx: &mut Context<Self>) {
        self.settled_height = height;
        cx.notify();
    }

    /// Whether content is hidden past the top or bottom edge, judged against the SETTLED height
    /// (R13).
    pub fn overflow_edges(&self) -> OverflowEdges {
        layout::overflow_edges(
            self.scroll_y,
            self.content_height(),
            self.settled_viewport(),
        )
    }

    /// Every visual row with its source start, logical line and content-space geometry.
    pub fn visual_rows(&self) -> Vec<RowInfo> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        layout
            .rows()
            .into_iter()
            .map(|row| RowInfo {
                source_start: self
                    .projection
                    .display_to_source(self.core.text(), row.start),
                source_line: row.line,
                first_of_line: row.row == 0,
                top: row.top,
                height: layout.line_height,
            })
            .collect()
    }

    fn resolved_theme(&self, cx: &App) -> FieldTheme {
        self.theme.unwrap_or_else(|| FieldTheme::resolve(cx))
    }

    fn sync_tab_stop(&self) {
        let _ = self
            .focus_handle
            .clone()
            .tab_stop(self.tab_stop && !self.disabled);
    }

    // ---- mutators ----------------------------------------------------------------------------

    /// Replaces the whole value: a new document (CMP-007 R4). Any external replacement is
    /// cancelled and late results of the previous document are rejected (R19).
    pub fn set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_external(cx);
        let changed = self.core.set_value(value);
        let edit = self.core.take_edit();
        self.text_changed(cx);
        self.scroll_y = px(0.0);
        self.scroll_x = px(0.0);
        self.follow = true;
        self.preferred_x = None;
        if changed {
            cx.emit(TextAreaEvent::Changed(self.core.text().to_owned()));
            if let Some((range, inserted)) = edit {
                cx.emit(TextAreaEvent::Edited {
                    range,
                    inserted,
                    epoch: self.core.edit_epoch(),
                });
            }
        }
        self.blink.reset(cx);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    /// The owner's controlled-value push (CMP-007 R5).
    pub fn acknowledge_or_apply(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Acknowledgement {
        let outcome = self.core.acknowledge_or_apply(value);
        if outcome == Acknowledgement::Replaced {
            self.cancel_external(cx);
            let _ = self.core.take_edit();
            self.text_changed(cx);
            self.scroll_y = px(0.0);
            self.scroll_x = px(0.0);
            self.follow = true;
            self.blink.reset(cx);
            window.notify_text_input_changed(&self.focus_handle);
            cx.notify();
        }
        outcome
    }

    /// Sets the placeholder.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Sets the selection (snapped to grapheme boundaries and atomic ranges).
    pub fn set_selected_range(
        &mut self,
        range: Range<usize>,
        reversed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.core.set_selection(range, reversed);
        self.finish_move(window, cx);
    }

    /// Selects the whole value.
    pub fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.core.select_all();
        self.finish_move(window, cx);
    }

    /// Focuses the editor, preserving the previous selection and caret.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    /// Focuses the editor and selects the whole value.
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.select_all(window, cx);
    }

    /// Sets the read-only flag.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.core.set_read_only(read_only);
        cx.notify();
    }

    /// Sets the disabled flag.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        self.sync_tab_stop();
        if disabled {
            self.end_drag();
        }
        cx.notify();
    }

    /// Sets the text size and line height.
    pub fn set_text_metrics(&mut self, size: Pixels, line_height: Pixels, cx: &mut Context<Self>) {
        self.metrics = TextMetrics { size, line_height };
        cx.notify();
    }

    /// Sets the height bounds in rows (see [`TextArea::rows`]).
    pub fn set_rows(&mut self, min_rows: usize, max_rows: usize, cx: &mut Context<Self>) {
        self.min_rows = min_rows.max(1);
        self.max_rows = max_rows.max(self.min_rows);
        cx.notify();
    }

    /// Overrides the theme roles (`None` follows the application theme).
    pub fn set_theme(&mut self, theme: Option<FieldTheme>, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Sets whether the enabled editor is a tab stop.
    pub fn set_tab_stop(&mut self, tab_stop: bool, cx: &mut Context<Self>) {
        self.tab_stop = tab_stop;
        self.sync_tab_stop();
        cx.notify();
    }

    /// Sets the Enter behavior.
    pub fn set_submit_mode(&mut self, mode: SubmitMode, cx: &mut Context<Self>) {
        self.submit_mode = mode;
        cx.notify();
    }

    /// Tells the editor that an anchored completion popup owns Enter (R7).
    pub fn set_completion_open(&mut self, open: bool) {
        self.completion_open = open;
    }

    /// Replaces a source range as one recorded, never-coalesced undo step.
    pub fn replace_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.disabled {
            return false;
        }
        let now = now_ms(cx);
        let changed = self.core.apply(range, text, EditKind::Other, now);
        self.finish_edit(changed, window, cx);
        changed
    }

    /// Replaces a source range without history and without a `Changed` event: a baseline sync
    /// of text the owner already knows about. `Edited` still fires so highlights can shift.
    pub fn replace_range_baseline(
        &mut self,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.cancel_external(cx);
        let changed = self.core.replace_unrecorded(range, text);
        self.text_changed(cx);
        if let Some((range, inserted)) = self.core.take_edit() {
            cx.emit(TextAreaEvent::Edited {
                range,
                inserted,
                epoch: self.core.edit_epoch(),
            });
        }
        self.follow = true;
        self.preferred_x = None;
        self.blink.reset(cx);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
        changed
    }

    /// Undo; a no-op when read-only or when there is nothing to undo.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let changed = self.core.undo();
        self.finish_edit(changed, window, cx);
        changed
    }

    /// Redo.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let changed = self.core.redo();
        self.finish_edit(changed, window, cx);
        changed
    }

    /// Pastes a clipboard item exactly as the Paste action pastes the system clipboard.
    pub fn paste_item(
        &mut self,
        entry: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled || self.core.is_read_only() {
            return;
        }
        self.paste_content(ClipboardContent::from_item(entry), window, cx);
    }

    /// Installs a projection directly (R16): an ordered list of non-overlapping replacements.
    /// Replacements from layers are added to it.
    pub fn set_projection(&mut self, replacements: Vec<Replacement>, cx: &mut Context<Self>) {
        self.manual_replacements = replacements;
        self.manual_epoch += 1;
        cx.notify();
    }

    /// Installs style spans over SOURCE byte offsets (syntax colors, faces).
    pub fn set_style_spans(
        &mut self,
        mut spans: Vec<(Range<usize>, SpanStyle)>,
        cx: &mut Context<Self>,
    ) {
        spans.sort_by_key(|(range, _)| range.start);
        self.manual_spans = spans;
        self.manual_epoch += 1;
        cx.notify();
    }

    /// Attaches a layer after construction.
    pub fn add_layer(&mut self, layer: Rc<dyn AreaLayer>, cx: &mut Context<Self>) {
        self.layers.push(layer);
        self.manual_epoch += 1;
        cx.notify();
    }

    /// Overrides the theme roles (`None` returns to the application theme).
    pub fn set_theme(&mut self, theme: Option<FieldTheme>, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Overrides the font (`None` returns to the inherited text font).
    pub fn set_font(&mut self, font: Option<Font>, cx: &mut Context<Self>) {
        self.font = font;
        cx.notify();
    }

    /// Sets the text size and line height.
    pub fn set_text_metrics(&mut self, size: Pixels, line_height: Pixels, cx: &mut Context<Self>) {
        self.metrics = TextMetrics { size, line_height };
        cx.notify();
    }

    /// Sets the width reserved at the left of the content for an owner-painted gutter.
    pub fn set_content_inset(&mut self, left: Pixels, cx: &mut Context<Self>) {
        self.content_inset = left;
        cx.notify();
    }

    /// Sets the wash painted behind the logical line holding the caret.
    pub fn set_active_line_wash(&mut self, wash: Option<Hsla>, cx: &mut Context<Self>) {
        self.active_line_wash = wash;
        cx.notify();
    }

    /// Sets the height bounds in rows (see [`TextArea::rows`]).
    pub fn set_rows(&mut self, min_rows: usize, max_rows: usize, cx: &mut Context<Self>) {
        self.min_rows = min_rows.max(1);
        self.max_rows = max_rows.max(self.min_rows);
        cx.notify();
    }

    /// Switches line wrapping on or off.
    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.scroll_x = px(0.0);
            self.follow = true;
            cx.notify();
        }
    }

    /// Whether long lines wrap.
    pub fn is_wrap(&self) -> bool {
        self.wrap
    }

    /// The horizontal scroll offset (always 0 while lines wrap).
    pub fn scroll_offset_x(&self) -> Pixels {
        self.scroll_x
    }

    /// Width of the widest line as of the last layout (0 before the first one).
    pub fn content_width(&self) -> Pixels {
        self.content_width
    }

    /// Sets the horizontal scroll offset (clamped; ignored while lines wrap); suspends
    /// caret-follow like wheel scrolling.
    pub fn set_scroll_offset_x(&mut self, offset: Pixels, cx: &mut Context<Self>) {
        if self.wrap {
            return;
        }
        let viewport = self.horizontal_viewport();
        self.scroll_x = clamp_scroll(offset, self.content_width, viewport);
        self.follow = false;
        cx.notify();
    }

    fn horizontal_viewport(&self) -> Pixels {
        self.bounds.map_or(px(0.0), |bounds| {
            (bounds.size.width - self.content_inset).max(px(0.0))
        })
    }

    /// Sets the vertical scroll offset (clamped); suspends caret-follow like wheel scrolling.
    pub fn set_scroll_offset(&mut self, offset: Pixels, cx: &mut Context<Self>) {
        let viewport = self.settled_viewport();
        self.scroll_y = clamp_scroll(offset, self.content_height(), viewport);
        self.refs.invalidate_hover();
        self.follow = false;
        cx.notify();
    }

    // ---- projection and layout -----------------------------------------------------------------------

    /// The union of the logical lines intersecting the selection and any marked range, line
    /// terminators excluded; a non-empty range ending right after a line feed does not include
    /// the following line (CMP-009 R4).
    fn active_range(&self) -> Range<usize> {
        let text = self.core.text();
        let selection = self.core.selection();
        let mut start = selection.start;
        let mut end = selection.end;
        if let Some(marked) = self.core.marked() {
            start = start.min(marked.start);
            end = end.max(marked.end);
        }
        let tail = if end > start && text[..end].ends_with('\n') {
            end - 1
        } else {
            end
        };
        line_start(text, start)..super::boundaries::line_end(text, tail)
    }

    fn layers_version(&self) -> u64 {
        self.layers.iter().fold(0, |acc, layer| {
            acc.wrapping_mul(31).wrapping_add(layer.version())
        })
    }

    /// Rebuilds the projection, display text and mapped spans when the source, the active
    /// lines, the marked range or a layer changed (R17). Rebuilding is not an edit.
    fn refresh_projection(&mut self, cx: &App) {
        let key = ProjectionKey {
            edit_epoch: self.core.edit_epoch(),
            active: self.active_range(),
            marked: self.core.marked(),
            layers: self.layers_version(),
            manual: self.manual_epoch,
            env: LayerEnv::resolve(self.resolved_theme(cx), cx),
        };
        if self.projection_key.as_ref() == Some(&key) {
            return;
        }
        let source = self.core.text();
        let mut replacements = self.manual_replacements.clone();
        for layer in &self.layers {
            replacements.extend(layer.replacements(source, key.active.clone(), key.marked.clone()));
        }
        replacements.retain(|replacement| replacement.range.end <= source.len());
        // A decoration that overlaps a chip is dropped (CMP-009 R5).
        let mut chip_ranges: Vec<Range<usize>> = replacements
            .iter()
            .filter(|replacement| replacement.atomic)
            .map(|replacement| replacement.range.clone())
            .collect();
        if !chip_ranges.is_empty() {
            chip_ranges.sort_by_key(|range| range.start);
            replacements.retain(|replacement| {
                replacement.atomic || {
                    let at =
                        chip_ranges.partition_point(|chip| chip.end <= replacement.range.start);
                    !chip_ranges
                        .get(at)
                        .is_some_and(|chip| chip.start < replacement.range.end)
                }
            });
        }
        self.projection = if replacements.is_empty() {
            Projection::identity()
        } else {
            Projection::new(source, replacements)
        };
        let atomics = self.projection.atomic_ranges();
        self.display = if self.projection.is_identity() {
            source.to_owned()
        } else {
            self.projection.display_text(source)
        };
        let mut spans = self.manual_spans.clone();
        for layer in &self.layers {
            spans.extend(layer.style_spans(source, &key.env));
        }
        spans.sort_by_key(|(range, _)| range.start);
        let mut display_spans: Vec<(Range<usize>, SpanStyle)> = spans
            .into_iter()
            .filter(|(range, _)| range.start < range.end && range.end <= source.len())
            .filter_map(|(range, style)| {
                let start = self.projection.source_to_display(range.start);
                let end = self.projection.source_to_display(range.end);
                (start < end).then_some((start..end, style))
            })
            .collect();
        self.refs.chips = self.surviving_chips(&atomics);
        if !self.refs.chips.is_empty() {
            // Within chips no face applies, and the padding is shaped in its fixed face (R2).
            let holes: Vec<Range<usize>> = self
                .refs
                .chips
                .iter()
                .map(|chip| chip.display.clone())
                .collect();
            display_spans = cut_spans(display_spans, &holes);
            let padding = SpanStyle {
                padding_face: true,
                ..SpanStyle::default()
            };
            for chip in &self.refs.chips {
                let label = &chip.info.layout.label;
                let start = chip.display.start;
                display_spans.push((start..start + label.start, padding));
                display_spans.push((start + label.end..chip.display.end, padding));
            }
            display_spans.sort_by_key(|(range, _)| range.start);
        }
        self.display_spans = display_spans;
        self.core.set_atomic_ranges(atomics);
        self.projection_key = Some(key);
        self.projection_epoch += 1;
        self.refs.projection_rebuilt();
    }

    fn display_of(&self, source_offset: usize) -> usize {
        self.projection.source_to_display(source_offset)
    }

    fn source_of(&self, display_offset: usize) -> usize {
        self.projection
            .display_to_source(self.core.text(), display_offset)
    }

    /// Lays out the display text for a content width, reusing every line whose text, runs,
    /// width, indent and size are unchanged (R3).
    pub(crate) fn ensure_layout(
        &mut self,
        width: Pixels,
        window: &mut Window,
        cx: &App,
    ) -> Rc<AreaLayout> {
        self.refresh_projection(cx);
        let theme = self.resolved_theme(cx);
        let color = if self.disabled {
            theme.muted
        } else {
            theme.text
        };
        let monospace: SharedString = vespertine_theme::appearance::try_theme_of(cx)
            .map(|theme| theme.typography.code_family.clone())
            .unwrap_or_else(|| "DejaVu Sans Mono".into());
        let marked_source = self.core.marked();
        let padding_font = self.padding_font(cx);
        let key = LayoutKey {
            projection_epoch: self.projection_epoch,
            width_bits: display_width_bits(width),
            font: self.resolved_font.clone(),
            size: self.metrics.size,
            line_height: self.metrics.line_height,
            color,
            monospace: monospace.clone(),
            padding: padding_font.clone(),
            marked: marked_source.clone(),
            edit_epoch: self.core.edit_epoch(),
            wrap: self.wrap,
        };
        if let (Some(layout), Some(previous)) = (&self.layout, &self.layout_key)
            && *previous == key
        {
            return layout.clone();
        }
        let base = BaseStyle {
            font: self.resolved_font.clone(),
            monospace_family: monospace,
            color,
            padding_font,
        };
        let marked_display =
            marked_source.map(|marked| self.display_of(marked.start)..self.display_of(marked.end));
        let mut sweep = SpanSweep::new(&self.display_spans);
        let mut old = std::mem::take(&mut self.line_cache);
        let mut fresh: HashMap<u64, Vec<Rc<CachedLine>>> = HashMap::new();
        let mut shapes = Vec::new();
        let mut start = 0;
        let mut chip_cursor = 0;
        let source = self.core.text();
        let mut source_lines = source.split('\n');
        let size = self.metrics.size;
        let size_bits = display_width_bits(size);
        for raw in self.display.split('\n') {
            let full_len = raw.len();
            let text = raw.strip_suffix('\r').unwrap_or(raw);
            let range = start..start + text.len();
            let spans = sweep.for_line(range.clone());
            let runs = line_runs(&base, range.clone(), &spans, marked_display.as_ref());
            let mut line_chips: Vec<Range<usize>> = Vec::new();
            while let Some(chip) = self.refs.chips.get(chip_cursor) {
                if chip.display.end > range.end {
                    break;
                }
                if chip.display.start >= range.start {
                    line_chips
                        .push(chip.display.start - range.start..chip.display.end - range.start);
                }
                chip_cursor += 1;
            }
            let source_line = source_lines.next().unwrap_or("");
            let indent = if self.wrap {
                self.hanging_indent(source, source_line, text, &runs, width, window)
            } else {
                px(0.0)
            };
            let indent_bits = display_width_bits(indent);
            // Without wrapping the shaped rows do not depend on the width.
            let width_bits = if self.wrap {
                display_width_bits((width - indent).max(px(layout::MIN_SHAPE_WIDTH)))
            } else {
                i32::MIN
            };
            let hash = hash_line(text, width_bits, indent_bits, size_bits);
            let reused = old.get(&hash).and_then(|bucket| {
                bucket
                    .iter()
                    .find(|line| {
                        line.text == text
                            && line.runs == runs
                            && line.width_bits == width_bits
                            && line.indent_bits == indent_bits
                            && line.size_bits == size_bits
                            && line.chips == line_chips
                    })
                    .cloned()
            });
            let cached = match reused {
                Some(line) => line,
                None => {
                    self.shape_count += 1;
                    let wrap_width = (width - indent).max(px(layout::MIN_SHAPE_WIDTH));
                    let breaks = if !self.wrap {
                        Some(Vec::new())
                    } else {
                        (!line_chips.is_empty()).then(|| {
                            layout::chip_breaks(window, text, &runs, size, wrap_width, &line_chips)
                        })
                    };
                    let shape =
                        LineShape::shape(window, text, &runs, size, wrap_width, indent, breaks);
                    Rc::new(CachedLine {
                        text: text.to_owned(),
                        runs,
                        width_bits,
                        indent_bits,
                        size_bits,
                        chips: line_chips,
                        shape: Rc::new(shape),
                    })
                }
            };
            shapes.push((cached.shape.clone(), full_len));
            fresh.entry(hash).or_default().push(cached);
            start += full_len + 1;
        }
        old.clear();
        self.line_cache = fresh;
        let layout = Rc::new(AreaLayout::new(shapes, self.metrics.line_height, width));
        self.layout = Some(layout.clone());
        self.layout_key = Some(key);
        layout
    }

    /// The continuation indent of a line (CMP-009 R7): the shaped width of its list prefix,
    /// capped at 40% of the width.
    fn hanging_indent(
        &self,
        source: &str,
        source_line: &str,
        display_line: &str,
        runs: &[TextRun],
        width: Pixels,
        window: &mut Window,
    ) -> Pixels {
        if self.layers.is_empty() || display_line.is_empty() {
            return px(0.0);
        }
        let line_start = source_line.as_ptr() as usize - source.as_ptr() as usize;
        let line_source = line_start..line_start + source_line.len();
        let Some(prefix) = self
            .layers
            .iter()
            .find_map(|layer| layer.hanging_prefix_len(source, line_source.clone(), display_line))
        else {
            return px(0.0);
        };
        let prefix = prefix.min(display_line.len());
        if prefix == 0 || !display_line.is_char_boundary(prefix) {
            return px(0.0);
        }
        let mut covered = 0;
        let mut sliced = Vec::new();
        for run in runs {
            if covered >= prefix {
                break;
            }
            let len = run.len.min(prefix - covered);
            let mut run = run.clone();
            run.len = len;
            covered += len;
            sliced.push(run);
        }
        let shaped = window.text_system().shape_line(
            SharedString::from(display_line[..prefix].to_owned()),
            self.metrics.size,
            &sliced,
            None,
        );
        shaped.width().min(width * 0.4)
    }

    /// Measures the editor for a width: lays out and returns the height clamped to the row
    /// bounds (R11).
    pub(crate) fn measure(&mut self, width: Pixels, window: &mut Window, cx: &App) -> Pixels {
        let inner = (width - self.content_inset).max(px(layout::MIN_SHAPE_WIDTH));
        let layout = self.ensure_layout(inner, window, cx);
        let (min, max) = self.content_height_bounds();
        self.measured_height = auto_height(layout.content_height, px(0.0), min, max);
        self.measured_height
    }

    /// Lays out inside the final bounds, clamps and follows the scroll, and records the
    /// geometry later queries use.
    pub(crate) fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inner = (bounds.size.width - self.content_inset).max(px(layout::MIN_SHAPE_WIDTH));
        let layout = self.ensure_layout(inner, window, cx);
        self.bounds = Some(bounds);
        let viewport = self.settled_viewport();
        let content = layout.content_height;
        let mut scroll = self.scroll_y;
        if self.follow && !self.core.text().is_empty() {
            let cursor = self.display_of(self.core.cursor());
            let point = layout.point_for(cursor, self.core.affinity());
            scroll = follow_scroll(scroll, point.y, layout.line_height, content, viewport);
        }
        self.scroll_y = clamp_scroll(scroll, content, viewport);
        self.content_width = layout
            .lines
            .iter()
            .map(|line| line.shape.max_width())
            .fold(px(0.0), Pixels::max);
        if self.wrap {
            self.scroll_x = px(0.0);
        } else {
            let mut scroll_x = self.scroll_x;
            if self.follow && !self.core.text().is_empty() {
                let cursor = self.display_of(self.core.cursor());
                let point = layout.point_for(cursor, self.core.affinity());
                scroll_x = follow_scroll(
                    scroll_x,
                    point.x,
                    self.metrics.size,
                    self.content_width + self.metrics.size,
                    inner,
                );
            }
            self.scroll_x = clamp_scroll(scroll_x, self.content_width + self.metrics.size, inner);
        }
        let snapshot = (self.scroll_y, bounds.size.height, content, self.scroll_x);
        if self.last_viewport != Some(snapshot) {
            let first = self.last_viewport.is_none();
            self.last_viewport = Some(snapshot);
            if !first {
                let entity = cx.entity();
                window.defer(cx, move |_, cx| {
                    entity.update(cx, |_, cx| cx.emit(TextAreaEvent::ViewportChanged));
                });
            }
        }
    }

    pub(crate) fn layout(&self) -> Option<Rc<AreaLayout>> {
        self.layout.clone()
    }

    pub(crate) fn resolved_theme_for_paint(&self, cx: &App) -> FieldTheme {
        self.resolved_theme(cx)
    }

    pub(crate) fn content_origin(&self, bounds: Bounds<Pixels>) -> Point<Pixels> {
        point(
            bounds.left() + self.content_inset - self.scroll_x,
            bounds.top() - self.scroll_y,
        )
    }

    // ---- geometry queries ---------------------------------------------------------------------------

    fn content_point(&self, position: Point<Pixels>) -> Option<Point<Pixels>> {
        let bounds = self.bounds?;
        Some(point(
            position.x - bounds.left() - self.content_inset + self.scroll_x,
            position.y - bounds.top() + self.scroll_y,
        ))
    }

    /// The window-coordinate position of the caret cell at a source offset (R14). `None`
    /// before the first layout.
    pub fn point_for_index(&self, source_offset: usize) -> Option<Point<Pixels>> {
        let layout = self.layout.as_ref()?;
        let bounds = self.bounds?;
        let affinity = if source_offset == self.core.cursor() {
            self.core.affinity()
        } else {
            Affinity::Downstream
        };
        let at = layout.point_for(self.display_of(source_offset), affinity);
        Some(point(
            bounds.left() + self.content_inset - self.scroll_x + at.x,
            bounds.top() - self.scroll_y + at.y,
        ))
    }

    /// The source offset at a window position (R4).
    pub fn index_for_point(&self, position: Point<Pixels>) -> Option<usize> {
        self.hit_source(position).map(|(offset, _)| offset)
    }

    fn hit_display(&self, position: Point<Pixels>) -> Option<(usize, Affinity)> {
        let layout = self.layout.as_ref()?;
        let at = self.content_point(position)?;
        Some(layout.hit(at))
    }

    /// The unnormalized source offset and affinity of a window position.
    fn hit_source(&self, position: Point<Pixels>) -> Option<(usize, Affinity)> {
        let (display, affinity) = self.hit_display(position)?;
        Some((self.source_of(display), affinity))
    }

    /// The box (2 px wide, one line tall) at the start of a source range, translated by the
    /// scroll (R14).
    pub fn bounds_for_range(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        let origin = self.point_for_index(range.start)?;
        Some(Bounds {
            origin,
            size: size(px(super::common::CARET_WIDTH), self.metrics.line_height),
        })
    }

    /// The caret rectangle in window coordinates; `None` before the first frame.
    pub fn caret_rect(&self) -> Option<Bounds<Pixels>> {
        self.bounds_for_range(self.core.cursor()..self.core.cursor())
    }

    /// Selection rectangles in window coordinates (R5).
    pub fn selection_rects(&self) -> Vec<Bounds<Pixels>> {
        let (Some(layout), Some(bounds)) = (&self.layout, self.bounds) else {
            return Vec::new();
        };
        let selection = self.core.selection();
        let range = self.display_of(selection.start)..self.display_of(selection.end);
        let origin = self.content_origin(bounds);
        layout
            .selection_rects(range, (bounds.size.width - self.content_inset).max(px(0.0)))
            .into_iter()
            .map(|rect| Bounds {
                origin: origin + rect.origin,
                size: rect.size,
            })
            .collect()
    }

    /// The washes behind spans that carry a background (inline and fenced code, CMP-009 R2), one
    /// quad per visual row, in window coordinates. Adjacent spans of one color are one run.
    pub fn wash_quads(&self) -> Vec<WashQuad> {
        let (Some(layout), Some(bounds)) = (&self.layout, self.bounds) else {
            return Vec::new();
        };
        let mut runs: Vec<(Range<usize>, Hsla)> = Vec::new();
        for (range, style) in &self.display_spans {
            let Some(color) = style.background else {
                continue;
            };
            match runs.last_mut() {
                Some((last, last_color)) if last.end == range.start && *last_color == color => {
                    last.end = range.end;
                }
                _ => runs.push((range.clone(), color)),
            }
        }
        let origin = self.content_origin(bounds);
        let mut quads = Vec::new();
        for (range, color) in runs {
            for segment in layout.range_segments(range) {
                quads.push(WashQuad {
                    bounds: Bounds {
                        origin: origin + segment.bounds.origin,
                        size: segment.bounds.size,
                    },
                    color,
                    round_left: segment.starts,
                    round_right: segment.ends,
                });
            }
        }
        quads
    }

    /// Whether the caret is drawn right now (CMP-007 R14).
    pub fn caret_visible(&self, window: &Window, cx: &App) -> bool {
        self.caret_eligible(window) && self.blink.phase_visible(now_ms(cx))
    }

    pub(crate) fn caret_eligible(&self, window: &Window) -> bool {
        !self.disabled
            && self.focus_handle.is_focused(window)
            && window.is_window_active()
            && self.core.selection().is_empty()
    }

    // ---- external progressive text (R18, R19) --------------------------------------------------------------

    /// The phase of the external replacement transaction.
    pub fn external_phase(&self) -> ExternalPhase {
        self.external.phase()
    }

    /// The generation counter, incremented on every cancellation; producers that run their own
    /// tasks pass it to the `_for_generation` methods so stale callbacks are rejected.
    pub fn external_generation(&self) -> u64 {
        self.external.generation()
    }

    fn external_phase_events(&mut self, cx: &mut Context<Self>) {
        for phase in self.external.drain_phase_changes() {
            cx.emit(TextAreaEvent::ExternalPhaseChanged(phase));
        }
    }

    fn cancel_external(&mut self, cx: &mut Context<Self>) {
        if self.external.is_active() {
            self.external.cancel();
        }
        self.external_phase_events(cx);
    }

    /// Cancels the transaction; the text already inserted stays committed as its own undo step.
    pub fn cancel_external_replacement(&mut self, cx: &mut Context<Self>) {
        self.cancel_external(cx);
    }

    /// Starts a transaction replacing `selection` (default: the current selection, normalized
    /// through the projection) with provisional text (R18).
    pub fn begin_external_replacement(
        &mut self,
        selection: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if self.disabled || self.core.is_read_only() {
            return;
        }
        let selection = selection.unwrap_or_else(|| self.core.selection());
        let selection = self.core.expand_atomic(selection);
        self.external.begin(&mut self.core, selection);
        self.external_phase_events(cx);
    }

    fn after_external(
        &mut self,
        outcome: ExternalOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ExternalOutcome {
        if outcome == ExternalOutcome::Applied {
            self.text_changed(cx);
            if let Some((range, inserted)) = self.core.take_edit() {
                cx.emit(TextAreaEvent::Changed(self.core.text().to_owned()));
                cx.emit(TextAreaEvent::Edited {
                    range,
                    inserted,
                    epoch: self.core.edit_epoch(),
                });
            }
            self.follow = true;
            self.preferred_x = None;
            self.blink.reset(cx);
            window.notify_text_input_changed(&self.focus_handle);
            cx.notify();
        }
        self.external_phase_events(cx);
        outcome
    }

    /// Replaces the transaction's target with provisional text.
    pub fn apply_provisional(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ExternalOutcome {
        let outcome = self.external.apply_provisional(&mut self.core, text);
        self.after_external(outcome, window, cx)
    }

    /// Applies the final text and ends the transaction as one undo edit.
    pub fn finalize(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ExternalOutcome {
        let outcome = self.external.finalize(&mut self.core, text);
        self.after_external(outcome, window, cx)
    }

    /// `apply_provisional` for a producer that captured the generation at `begin`.
    pub fn apply_provisional_for_generation(
        &mut self,
        generation: u64,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ExternalOutcome {
        let outcome =
            self.external
                .apply_provisional_for_generation(generation, &mut self.core, text);
        self.after_external(outcome, window, cx)
    }

    /// `finalize` for a producer that captured the generation at `begin`.
    pub fn finalize_for_generation(
        &mut self,
        generation: u64,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ExternalOutcome {
        let outcome = self
            .external
            .finalize_for_generation(generation, &mut self.core, text);
        self.after_external(outcome, window, cx)
    }

    // ---- newline, indent -------------------------------------------------------------------------------------

    /// Inserts a newline through the layers' newline hook first (list continuation), else a
    /// plain line terminator: CRLF when the broken line already ends in CRLF (R1, R7).
    pub fn insert_newline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.core.is_read_only() {
            return;
        }
        let now = now_ms(cx);
        let selection = self.core.selection();
        let text = self.core.text();
        if selection.is_empty() && self.core.marked().is_none() {
            let hook = self
                .layers
                .iter()
                .find_map(|layer| layer.newline_edit(text, selection.start));
            if let Some(edit) = hook {
                let changed = self
                    .core
                    .apply(edit.range, &edit.text, EditKind::Other, now);
                self.finish_edit(changed, window, cx);
                return;
            }
        }
        let crlf = {
            let end = super::boundaries::raw_line_end(text, selection.start);
            end > 0
                && text.as_bytes().get(end - 1) == Some(&b'\r')
                && end > line_start(text, selection.start)
        };
        let terminator = if crlf { "\r\n" } else { "\n" };
        let changed = self.core.apply(selection, terminator, EditKind::Other, now);
        self.finish_edit(changed, window, cx);
    }

    fn indent(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.disabled
            || self.core.is_read_only()
            || self.core.marked().is_some()
            || self.completion_open
        {
            return false;
        }
        let selection = self.core.selection();
        let reversed = self.core.is_reversed();
        let text = self.core.text();
        let Some(indent) = self
            .layers
            .iter()
            .find_map(|layer| layer.indent_edit(text, selection.clone(), reversed, outdent))
        else {
            return false;
        };
        let now = now_ms(cx);
        let changed = if indent.edit.range.is_empty() && indent.edit.text.is_empty() {
            false
        } else {
            self.core
                .apply(indent.edit.range, &indent.edit.text, EditKind::Other, now)
        };
        self.core.set_selection(indent.selection, indent.reversed);
        self.finish_edit(changed, window, cx);
        true
    }

    // ---- vertical movement ------------------------------------------------------------------------------------

    fn move_vertical(
        &mut self,
        down: bool,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let Some(layout) = self.layout.clone() else {
            return;
        };
        let cursor = self.display_of(self.core.cursor());
        let affinity = self.core.affinity();
        let row = layout.row_at(cursor, affinity);
        let x = self
            .preferred_x
            .unwrap_or_else(|| layout.point_for(cursor, affinity).x);
        let target = if down {
            layout.row_below(row)
        } else {
            layout.row_above(row)
        };
        match target {
            None if down => {
                self.core.move_to(self.core.text().len(), extend);
                self.core.set_affinity(Affinity::Upstream);
            }
            None => {
                self.core.move_to(0, extend);
            }
            Some(target) => {
                let (display, affinity) = layout.offset_on_row(target, x);
                let source = self.source_of(display);
                self.core.move_to(source, extend);
                self.core.set_affinity(affinity);
            }
        }
        self.after_move(window, cx);
        self.preferred_x = Some(x);
    }

    fn after_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.external.sync(&self.core) {
            self.external_phase_events(cx);
        }
        self.follow = true;
        self.preferred_x = None;
        self.blink.reset(cx);
        window.notify_text_selection_changed(&self.focus_handle);
        cx.emit(TextAreaEvent::CursorMoved);
        cx.notify();
    }

    // ---- pointer ---------------------------------------------------------------------------------------------------

    pub(crate) fn end_drag(&mut self) {
        self.drag = None;
        self.autoscroll = None;
        self.pointer_generation += 1;
    }

    pub(crate) fn on_pointer_down(
        &mut self,
        event: &vespertine::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        window.focus(&self.focus_handle, cx);
        self.pointer_generation += 1;
        self.autoscroll = None;
        self.chip_pointer_down(event.position, event.modifiers.shift, event.click_count);
        let Some((hit, affinity)) = self.hit_source(event.position) else {
            return;
        };
        let anchor = self.core.press(
            hit,
            event.click_count,
            event.modifiers.shift,
            self.triple_click_selects_all,
        );
        if event.click_count == 2
            && !event.modifiers.shift
            && let Some(hit) = self.chip_at(event.position)
        {
            // A double click on a chip selects the whole chip (CMP-010 R2).
            self.core.set_selection(hit.descriptor.range, false);
        }
        if event.click_count <= 1 && self.core.selection().is_empty() {
            self.core.set_affinity(affinity);
        }
        self.drag = Some(Drag {
            anchor,
            position: event.position,
        });
        self.after_move(window, cx);
    }

    pub(crate) fn on_pointer_move(
        &mut self,
        event: &vespertine::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.bounds else { return };
        if self.drag.is_none() {
            return;
        }
        if event.pressed_button != Some(vespertine::MouseButton::Left) {
            self.end_drag();
            return;
        }
        if let Some(drag) = self.drag.as_mut() {
            drag.position = event.position;
        }
        let outside = event.position.y < bounds.top() || event.position.y > bounds.bottom();
        self.drag_selection(event.position, window, cx);
        if outside {
            self.follow = false;
            self.ensure_autoscroll(window, cx);
        } else {
            self.autoscroll = None;
        }
    }

    fn clamp_position(&self, position: Point<Pixels>) -> Point<Pixels> {
        match self.bounds {
            Some(bounds) => point(
                position.x,
                position
                    .y
                    .max(bounds.top())
                    .min(bounds.bottom() - px(super::common::DRAG_INSET)),
            ),
            None => position,
        }
    }

    fn drag_selection(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clamped = self.clamp_position(position);
        let Some((hit, _)) = self.hit_source(clamped) else {
            return;
        };
        let Some(anchor) = self.drag.as_ref().map(|drag| drag.anchor.clone()) else {
            return;
        };
        self.core
            .drag_to(&anchor, hit, self.triple_click_selects_all);
        self.blink.reset(cx);
        window.notify_text_selection_changed(&self.focus_handle);
        cx.emit(TextAreaEvent::CursorMoved);
        cx.notify();
    }

    pub(crate) fn on_pointer_up(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.chip_pointer_up(position, cx);
        if self.drag.is_some() {
            self.end_drag();
            cx.notify();
        }
    }

    fn ensure_autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.autoscroll.is_some() {
            return;
        }
        let generation = self.pointer_generation;
        self.autoscroll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(
                        super::common::AUTOSCROLL_INTERVAL_MS,
                    ))
                    .await;
                let keep_going = this
                    .update_in(cx, |this, window, cx| {
                        this.autoscroll_tick(generation, window, cx)
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    /// One drag autoscroll tick (R9); a stale generation does nothing.
    #[doc(hidden)]
    pub fn autoscroll_tick(
        &mut self,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.pointer_generation || self.drag.is_none() {
            return false;
        }
        let (Some(bounds), Some(position)) =
            (self.bounds, self.drag.as_ref().map(|drag| drag.position))
        else {
            return false;
        };
        let (direction, distance) = if position.y < bounds.top() {
            (-1.0, f32::from(bounds.top() - position.y))
        } else if position.y > bounds.bottom() {
            (1.0, f32::from(position.y - bounds.bottom()))
        } else {
            self.autoscroll = None;
            return false;
        };
        let step = direction
            * super::common::autoscroll_step(distance, f32::from(self.metrics.line_height));
        let viewport = self.settled_viewport();
        let target = clamp_scroll(self.scroll_y + px(step), self.content_height(), viewport);
        if target == self.scroll_y {
            self.autoscroll = None;
            return false;
        }
        self.scroll_y = target;
        self.refs.invalidate_hover();
        self.follow = false;
        self.drag_selection(position, window, cx);
        true
    }

    /// Wheel scrolling (R10): returns whether the editor consumed the event. A scrollable editor
    /// consumes it even at a scroll limit so the gesture does not chain into an ancestor.
    pub(crate) fn on_wheel(
        &mut self,
        event: &vespertine::ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.refs.invalidate_hover();
        let viewport = self.settled_viewport();
        let content = self.content_height();
        let delta = event.delta.pixel_delta(self.metrics.line_height);
        let horizontal_content = self.content_width + self.metrics.size;
        let horizontal = !self.wrap && horizontal_content > self.horizontal_viewport();
        let vertical = content > viewport;
        // A vertical gesture over an editor that only scrolls horizontally passes through.
        if self.disabled || !(vertical || horizontal && delta.x != px(0.0)) {
            return false;
        }
        if horizontal {
            self.scroll_x = clamp_scroll(
                self.scroll_x - delta.x,
                horizontal_content,
                self.horizontal_viewport(),
            );
        }
        if vertical {
            self.scroll_y = clamp_scroll(self.scroll_y - delta.y, content, viewport);
        }
        self.follow = false;
        cx.notify();
        true
    }

    pub(crate) fn sync_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.caret_visible(window, cx);
        self.blink.painted_visible = visible;
        if self.blink.driver_running || !self.caret_eligible(window) {
            return;
        }
        self.blink.driver_running = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let wait = this.update_in(cx, |this, window, cx| {
                    if !this.caret_eligible(window) {
                        this.blink.driver_running = false;
                        return None;
                    }
                    let now = now_ms(cx);
                    if this.blink.phase_visible(now) != this.blink.painted_visible {
                        cx.notify();
                    }
                    Some(this.blink.next_flip_ms(now))
                });
                match wait {
                    Ok(Some(ms)) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(ms))
                            .await
                    }
                    _ => break,
                }
            }
        })
        .detach();
    }

    pub(crate) fn line_wash_rect(&self, bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let cursor = self.display_of(self.core.cursor());
        let row = layout.row_at(cursor, self.core.affinity());
        let line = &layout.lines[row.line];
        let origin = self.content_origin(bounds);
        Some(Bounds {
            origin: point(bounds.left(), origin.y + line.top),
            size: size(
                bounds.size.width,
                layout.line_height * line.shape.row_count(),
            ),
        })
    }

    pub(crate) fn wash(&self) -> Option<Hsla> {
        self.active_line_wash
    }

    pub(crate) fn role_value(&self) -> Role {
        self.role
    }

    pub(crate) fn placeholder_ref(&self) -> &SharedString {
        &self.placeholder
    }

    pub(crate) fn mode_context(&self) -> String {
        if self.layers.iter().any(|layer| layer.handles_tab()) {
            format!("{AREA_CONTEXT} Markdown")
        } else {
            AREA_CONTEXT.to_owned()
        }
    }

    pub(crate) fn font_override(&self) -> Option<Font> {
        self.font.clone()
    }

    pub(crate) fn settled_height_value(&self) -> Option<Pixels> {
        self.settled_height
    }

    /// The submit chord of "modified Enter submits" mode; returns whether it was handled.
    fn modified_submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.disabled || self.core.marked().is_some() {
            return true;
        }
        if self.submit_mode == SubmitMode::ModifiedEnter {
            cx.emit(TextAreaEvent::Submitted(self.core.text().to_owned()));
            true
        } else {
            false
        }
    }

    pub(crate) fn focus_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub(crate) fn content_inset_value(&self) -> Pixels {
        self.content_inset
    }
}

impl EditorHost for TextArea {
    fn core(&self) -> &EditCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut EditCore {
        &mut self.core
    }

    fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    fn is_disabled(&self) -> bool {
        self.disabled
    }

    fn paste_preference(&self) -> PastePreference {
        self.paste_preference
    }

    fn finish_edit(&mut self, changed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if changed {
            self.text_changed(cx);
            if self.external.sync(&self.core) {
                self.external_phase_events(cx);
            }
            let edit = self.core.take_edit();
            cx.emit(TextAreaEvent::Changed(self.core.text().to_owned()));
            if let Some((range, inserted)) = edit {
                cx.emit(TextAreaEvent::Edited {
                    range,
                    inserted,
                    epoch: self.core.edit_epoch(),
                });
            }
        }
        self.follow = true;
        self.preferred_x = None;
        self.blink.reset(cx);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn finish_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.after_move(window, cx);
    }

    fn act_line_end(&mut self, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        self.core.move_line_end(extend);
        // At an exact wrap boundary the caret stays on the row the line ends on (R4).
        if let Some(layout) = self.layout.clone() {
            let display = self.display_of(self.core.cursor());
            let row = layout.row_at(display, Affinity::Downstream);
            if row.row > 0 && row.start == display {
                self.core.set_affinity(Affinity::Upstream);
            }
        }
        self.after_move(window, cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.core.marked().is_some() {
            return;
        }
        if self.completion_open {
            cx.emit(TextAreaEvent::AcceptCompletion);
            return;
        }
        match self.submit_mode {
            SubmitMode::EnterSubmits => {
                cx.emit(TextAreaEvent::Submitted(self.core.text().to_owned()));
            }
            SubmitMode::Enter | SubmitMode::ModifiedEnter => self.insert_newline(window, cx),
        }
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.core.marked().is_some() {
            return;
        }
        cx.emit(TextAreaEvent::Cancelled);
    }

    fn paste_media(&mut self, paths: Vec<PathBuf>, images: Vec<Image>, cx: &mut Context<Self>) {
        if !paths.is_empty() {
            cx.emit(TextAreaEvent::PastedPaths(paths));
        }
        if !images.is_empty() {
            cx.emit(TextAreaEvent::PastedImages(images));
        }
    }

    fn copy_falls_through(&self) -> bool {
        self.copies_background_selection && self.core.selection().is_empty()
    }

    fn copy_item(&mut self, _cx: &App) -> Option<ClipboardItem> {
        if self.core.selection().is_empty() {
            return None;
        }
        self.scoped_copy_item().or_else(|| {
            let text = self.core.selected_text();
            (!text.is_empty()).then(|| ClipboardItem::new_string(text.to_owned()))
        })
    }

    fn restore_pasted_text(&self, text: String, metadata: Option<&str>) -> String {
        self.restored_paste_text(text, metadata)
    }

    fn finish_paste(&mut self, start: usize, cx: &mut Context<Self>) {
        self.emit_pasted(start, cx);
    }
}

impl EntityInputHandler for TextArea {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        self.ime_text_for_range(range, adjusted_range)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.ime_selected()
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.core.marked_range_utf16()
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ime_unmark(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_replace_text(range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_replace_and_mark(range, new_text, new_selected_range, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let range = self.core.range_from_utf16(&range_utf16);
        let affinity = if range.start == self.core.cursor() {
            self.core.affinity()
        } else {
            Affinity::Downstream
        };
        let at = layout.point_for(self.display_of(range.start), affinity);
        Some(Bounds {
            origin: point(
                element_bounds.left() + self.content_inset + at.x,
                element_bounds.top() - self.scroll_y + at.y,
            ),
            size: size(px(super::common::CARET_WIDTH), self.metrics.line_height),
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (source, _) = self.hit_source(point)?;
        let source = self.core.snap_caret(source);
        Some(self.core.range_to_utf16(&(source..source)).start)
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        !self.disabled && !self.core.is_read_only()
    }
}
