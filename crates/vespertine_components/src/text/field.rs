//! The single-line text field (CMP-007).

use super::{
    actions::{FIELD_CONTEXT, bind_default_keys},
    common::{
        AUTOSCROLL_INTERVAL_MS, Blink, CARET_WIDTH, DRAG_INSET, EditMenuState, FieldTheme,
        PastePreference, TextMetrics, autoscroll_step,
    },
    core::{Acknowledgement, DragAnchor, EditCore},
    editor::{EditorHost, now_ms},
};
use std::{ops::Range, path::PathBuf, rc::Rc, time::Duration};
use vespertine::{
    AnyElement, App, Bounds, ContentMask, Context, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, Image, InspectorElementId, InteractiveElement,
    IntoElement, KeyDownEvent, KeyUpEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, Role, ScrollWheelEvent, ShapedLine,
    SharedString, StatefulInteractiveElement, Style, Styled, Task, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, div, fill, point, px, relative, size,
};

/// Events of a [`TextField`] (CMP-007 R22).
#[derive(Clone, Debug)]
pub enum TextFieldEvent {
    /// The value changed through a user or input-method edit, undo or redo; emitted
    /// synchronously with the edit.
    Changed(String),
    /// Enter was pressed outside a composition.
    Submitted(String),
    /// Escape was pressed outside a composition.
    Cancelled,
    /// External file paths were pasted; nothing was inserted.
    PastedPaths(Vec<PathBuf>),
    /// Images were pasted; nothing was inserted.
    PastedImages(Vec<Image>),
    /// The field gained focus.
    Focused,
    /// The field lost focus.
    Blurred,
    /// A key went down while the field was focused.
    KeyDown(KeyDownEvent),
    /// A key was released while the field was focused.
    KeyUp(KeyUpEvent),
}

#[derive(Clone, PartialEq)]
struct ShapeKey {
    edit_epoch: u64,
    placeholder: SharedString,
    is_placeholder: bool,
    width: i32,
    font: Font,
    size: Pixels,
    color: Hsla,
    marked: Option<Range<usize>>,
}

struct Shaped {
    key: ShapeKey,
    line: ShapedLine,
}

struct Drag {
    anchor: DragAnchor,
    position: Point<Pixels>,
}

type Adornment = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A single-line text field: an editable value with placeholder, read-only and disabled modes,
/// selection, caret, native text input, clipboard, undo and controlled-value sync (CMP-007).
pub struct TextField {
    core: EditCore,
    focus_handle: FocusHandle,
    placeholder: SharedString,
    disabled: bool,
    tab_stop: bool,
    role: Role,
    metrics: TextMetrics,
    theme: Option<FieldTheme>,
    font: Option<Font>,
    paste_preference: PastePreference,
    leading: Option<Adornment>,
    trailing: Option<Adornment>,
    scroll_x: Pixels,
    follow: bool,
    blink: Blink,
    shaped: Option<Shaped>,
    shape_count: usize,
    bounds: Option<Bounds<Pixels>>,
    text_width: Pixels,
    drag: Option<Drag>,
    pointer_generation: u64,
    autoscroll: Option<Task<()>>,
    _subscriptions: Vec<vespertine::Subscription>,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextField {
    /// An empty, enabled, editable field with the default metrics (16 px text, 20 px lines).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_default_keys(cx);
        let focus_handle = cx.focus_handle().tab_stop(true);
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, window, cx| {
                this.blink.reset(cx);
                cx.emit(TextFieldEvent::Focused);
                cx.notify();
                let _ = window;
            }),
            cx.on_blur(&focus_handle, window, |this, _window, cx| {
                this.end_drag();
                this.core.cancel_marking();
                cx.emit(TextFieldEvent::Blurred);
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, _window, cx| {
                this.blink.reset(cx);
                cx.notify();
            }),
        ];
        Self {
            core: EditCore::new("", false),
            focus_handle,
            placeholder: SharedString::default(),
            disabled: false,
            tab_stop: true,
            role: Role::TextInput,
            metrics: TextMetrics::default(),
            theme: None,
            font: None,
            paste_preference: PastePreference::TextFirst,
            leading: None,
            trailing: None,
            scroll_x: px(0.0),
            follow: true,
            blink: Blink::new(cx),
            shaped: None,
            shape_count: 0,
            bounds: None,
            text_width: px(0.0),
            drag: None,
            pointer_generation: 0,
            autoscroll: None,
            _subscriptions: subscriptions,
        }
    }

    // ---- builders ---------------------------------------------------------------------------

    /// Sets the placeholder (line breaks become spaces).
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = single_line(placeholder.into());
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

    /// Whether the enabled field is a tab stop (default true).
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self.sync_tab_stop();
        self
    }

    /// Sets the accessibility role (default text input).
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

    /// An embedded control before the text (clear button, icon): a press on it never moves the
    /// caret, collapses the selection or starts a text drag (R25).
    pub fn leading(
        mut self,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.leading = Some(Rc::new(build));
        self
    }

    /// An embedded control after the text.
    pub fn trailing(
        mut self,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.trailing = Some(Rc::new(build));
        self
    }

    // ---- accessors ----------------------------------------------------------------------------

    /// The value.
    pub fn value_text(&self) -> &str {
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

    /// The placeholder.
    pub fn placeholder_text(&self) -> &SharedString {
        &self.placeholder
    }

    /// Whether the field is read-only.
    pub fn is_read_only(&self) -> bool {
        self.core.is_read_only()
    }

    /// Whether the field is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The editing core (selection model and history), for inspection.
    pub fn core(&self) -> &EditCore {
        &self.core
    }

    /// The horizontal scroll offset.
    pub fn scroll_offset(&self) -> Pixels {
        self.scroll_x
    }

    /// How many times the text was shaped; layout is reused across scroll, selection, caret blink
    /// and owner-driven size animation (R26).
    pub fn shape_count(&self) -> usize {
        self.shape_count
    }

    /// The text metrics.
    pub fn metrics(&self) -> TextMetrics {
        self.metrics
    }

    /// The bounds of the text element of the last frame.
    pub fn text_bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
    }

    /// The enablement of Cut, Copy, Paste and Select All (R24).
    pub fn edit_menu_state(&self, cx: &App) -> EditMenuState {
        self.menu_state(cx)
    }

    // ---- mutators -----------------------------------------------------------------------------

    /// Replaces the whole value: a new document, not an edit (R4). Emits `Changed` only when the
    /// value differs.
    pub fn set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.core.set_value(value);
        self.scroll_x = px(0.0);
        self.follow = true;
        if changed {
            cx.emit(TextFieldEvent::Changed(self.core.text().to_owned()));
        }
        self.blink.reset(cx);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    /// The owner's controlled-value push (R5): an acknowledgement of a value this field emitted
    /// changes nothing; any other value replaces the text.
    pub fn acknowledge_or_apply(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Acknowledgement {
        let outcome = self.core.acknowledge_or_apply(value);
        if outcome == Acknowledgement::Replaced {
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
        self.placeholder = single_line(placeholder.into());
        cx.notify();
    }

    /// Sets the selection (grapheme-snapped).
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

    /// Focuses the field, preserving the previous selection and caret (R23).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    /// Focuses the field and selects the whole value (address and search entries, R23).
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.select_all(window, cx);
    }

    /// Pastes a clipboard item exactly as the Paste action pastes the system clipboard (R16):
    /// plain text replaces the selection, paths and images are reported to the owner.
    pub fn paste_item(
        &mut self,
        item: &vespertine::ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled || self.core.is_read_only() {
            return;
        }
        self.paste_content(super::common::ClipboardContent::from_item(item), window, cx);
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

    fn sync_tab_stop(&self) {
        let _ = self
            .focus_handle
            .clone()
            .tab_stop(self.tab_stop && !self.disabled);
    }

    fn resolved_theme(&self, cx: &App) -> FieldTheme {
        self.theme.unwrap_or_else(|| FieldTheme::resolve(cx))
    }

    // ---- geometry --------------------------------------------------------------------------------

    fn is_placeholder(&self) -> bool {
        self.core.text().is_empty()
    }

    fn line(&self) -> Option<&ShapedLine> {
        self.shaped.as_ref().map(|shaped| &shaped.line)
    }

    fn visible_width(&self) -> Pixels {
        self.bounds
            .map(|bounds| (bounds.size.width - px(2.0)).max(px(0.0)))
            .unwrap_or_default()
    }

    fn max_scroll(&self) -> Pixels {
        (self.text_width - self.visible_width()).max(px(0.0))
    }

    fn text_top(&self, bounds: Bounds<Pixels>) -> Pixels {
        bounds.top() + ((bounds.size.height - self.metrics.line_height) / 2.0).max(px(0.0))
    }

    fn x_for_index(&self, index: usize) -> Pixels {
        match (self.line(), self.is_placeholder()) {
            (Some(line), false) => line.x_for_index(index),
            _ => px(0.0),
        }
    }

    /// The closest grapheme boundary for a window-coordinate x (R15): 0 against a placeholder.
    pub fn index_for_x(&self, x: Pixels) -> usize {
        let Some(bounds) = self.bounds else { return 0 };
        let Some(line) = self.line() else { return 0 };
        if self.is_placeholder() {
            return 0;
        }
        let local = x - bounds.left() + self.scroll_x;
        let index = line.closest_index_for_x(local);
        super::boundaries::snap_grapheme_boundary(self.core.text(), index)
    }

    /// The window x coordinate of the caret cell at a byte index, following the scroll offset.
    pub fn window_x_for_index(&self, index: usize) -> Option<Pixels> {
        let bounds = self.bounds?;
        Some(bounds.left() - self.scroll_x + self.x_for_index(index))
    }

    /// The caret rectangle in window coordinates: 2 px wide and one line tall (R14); `None`
    /// before the first frame.
    pub fn caret_rect(&self) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        let x = bounds.left() - self.scroll_x + self.x_for_index(self.core.cursor());
        Some(Bounds {
            origin: point(x, self.text_top(bounds)),
            size: size(px(CARET_WIDTH), self.metrics.line_height),
        })
    }

    /// Whether the caret is drawn right now: focused, window active, not disabled, collapsed
    /// selection and in the visible half of the blink cycle (R14).
    pub fn caret_visible(&self, window: &Window, cx: &App) -> bool {
        self.caret_eligible(window, cx) && self.blink.phase_visible(now_ms(cx))
    }

    fn caret_eligible(&self, window: &Window, _cx: &App) -> bool {
        !self.disabled
            && self.focus_handle.is_focused(window)
            && window.is_window_active()
            && self.core.selection().is_empty()
    }

    /// The selection rectangle in window coordinates (R15); `None` for an empty selection.
    pub fn selection_rect(&self) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        let selection = self.core.selection();
        if selection.is_empty() || self.is_placeholder() {
            return None;
        }
        let origin = bounds.left() - self.scroll_x;
        let left = origin + self.x_for_index(selection.start);
        let right = origin + self.x_for_index(selection.end);
        Some(Bounds {
            origin: point(left, self.text_top(bounds)),
            size: size(right - left, self.metrics.line_height),
        })
    }

    // ---- layout ------------------------------------------------------------------------------------

    fn prepare(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.resolved_theme(cx);
        let font = self
            .font
            .clone()
            .unwrap_or_else(|| window.text_style().font());
        let is_placeholder = self.is_placeholder();
        let color = if self.disabled {
            theme.muted
        } else if is_placeholder {
            theme.faint
        } else {
            theme.text
        };
        let key = ShapeKey {
            edit_epoch: self.core.edit_epoch(),
            placeholder: self.placeholder.clone(),
            is_placeholder,
            width: (f32::from(bounds.size.width) * 100.0).round() as i32,
            font: font.clone(),
            size: self.metrics.size,
            color,
            marked: self.core.marked(),
        };
        if self.shaped.as_ref().map(|shaped| &shaped.key) != Some(&key) {
            let display: SharedString = if is_placeholder {
                self.placeholder.clone()
            } else {
                SharedString::from(self.core.text().to_owned())
            };
            let runs = text_runs(
                &display,
                &font,
                color,
                key.marked.as_ref().filter(|_| !is_placeholder),
            );
            let line = window
                .text_system()
                .shape_line(display, self.metrics.size, &runs, None);
            self.shape_count += 1;
            self.shaped = Some(Shaped { key, line });
        }
        self.bounds = Some(bounds);
        let line = self.shaped.as_ref().map(|shaped| &shaped.line);
        self.text_width = match (line, is_placeholder) {
            (Some(line), false) => line.width(),
            _ => px(0.0),
        };
        let visible = self.visible_width();
        if self.follow && !is_placeholder {
            let caret_x = self.x_for_index(self.core.cursor());
            let low = caret_x - visible;
            self.scroll_x = self.scroll_x.max(low).min(caret_x);
        }
        self.scroll_x = self.scroll_x.max(px(0.0)).min(self.max_scroll());
        if is_placeholder {
            self.scroll_x = px(0.0);
        }
    }

    // ---- pointer ---------------------------------------------------------------------------------------

    fn clamp_position(&self, position: Point<Pixels>) -> Point<Pixels> {
        match self.bounds {
            Some(bounds) => point(
                position
                    .x
                    .max(bounds.left())
                    .min(bounds.right() - px(DRAG_INSET)),
                position.y,
            ),
            None => position,
        }
    }

    fn end_drag(&mut self) {
        self.drag = None;
        self.autoscroll = None;
        self.pointer_generation += 1;
    }

    fn on_pointer_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        window.focus(&self.focus_handle, cx);
        self.pointer_generation += 1;
        self.autoscroll = None;
        let hit = self.index_for_x(event.position.x);
        let anchor = self
            .core
            .press(hit, event.click_count, event.modifiers.shift, false);
        self.drag = Some(Drag {
            anchor,
            position: event.position,
        });
        self.follow = true;
        self.finish_move(window, cx);
    }

    fn on_pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.bounds else { return };
        if self.drag.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag();
            return;
        }
        if let Some(drag) = self.drag.as_mut() {
            drag.position = event.position;
        }
        let outside = event.position.x < bounds.left() || event.position.x > bounds.right();
        let clamped = self.clamp_position(event.position);
        let hit = self.index_for_x(clamped.x);
        if let Some(drag) = self.drag.as_ref() {
            let anchor = drag.anchor.clone();
            self.core.drag_to(&anchor, hit, false);
        }
        if outside {
            self.follow = false;
            self.ensure_autoscroll(window, cx);
        } else {
            self.autoscroll = None;
            self.follow = true;
        }
        window.notify_text_selection_changed(&self.focus_handle);
        self.blink.reset(cx);
        cx.notify();
    }

    fn on_pointer_up(&mut self, cx: &mut Context<Self>) {
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
                    .timer(Duration::from_millis(AUTOSCROLL_INTERVAL_MS))
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

    /// One autoscroll tick (R10); stale generations do nothing. Returns whether to continue.
    fn autoscroll_tick(
        &mut self,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.pointer_generation || self.drag.is_none() {
            return false;
        }
        let (Some(bounds), Some(drag)) = (self.bounds, self.drag.as_ref()) else {
            return false;
        };
        let position = drag.position;
        let (direction, distance) = if position.x < bounds.left() {
            (-1.0, f32::from(bounds.left() - position.x))
        } else if position.x > bounds.right() {
            (1.0, f32::from(position.x - bounds.right()))
        } else {
            self.autoscroll = None;
            return false;
        };
        let step = direction * autoscroll_step(distance, f32::from(self.metrics.line_height));
        let target = (self.scroll_x + px(step))
            .max(px(0.0))
            .min(self.max_scroll());
        if target == self.scroll_x {
            self.autoscroll = None;
            return false;
        }
        self.scroll_x = target;
        self.follow = false;
        let clamped = self.clamp_position(position);
        let hit = self.index_for_x(clamped.x);
        let anchor = drag.anchor.clone();
        self.core.drag_to(&anchor, hit, false);
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
        true
    }

    fn on_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) -> bool {
        let max = self.max_scroll();
        if max <= px(0.0) || self.disabled {
            return false;
        }
        let delta = event.delta.pixel_delta(self.metrics.line_height);
        let amount = if delta.x != px(0.0) { delta.x } else { delta.y };
        self.scroll_x = (self.scroll_x - amount).max(px(0.0)).min(max);
        self.follow = false;
        cx.notify();
        true
    }

    // ---- blink -----------------------------------------------------------------------------------------

    fn sync_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.caret_visible(window, cx);
        self.blink.painted_visible = visible;
        if self.blink.driver_running || !self.caret_eligible(window, cx) {
            return;
        }
        self.blink.driver_running = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let wait = this.update_in(cx, |this, window, cx| {
                    if !this.caret_eligible(window, cx) {
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
                            .timer(Duration::from_millis(ms))
                            .await
                    }
                    _ => break,
                }
            }
        })
        .detach();
    }
}

fn single_line(text: SharedString) -> SharedString {
    if text.contains(['\r', '\n']) {
        text.replace("\r\n", " ").replace(['\r', '\n'], " ").into()
    } else {
        text
    }
}

fn text_runs(text: &str, font: &Font, color: Hsla, marked: Option<&Range<usize>>) -> Vec<TextRun> {
    let base = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs: Vec<TextRun> = match marked {
        Some(marked) if marked.end <= text.len() => vec![
            TextRun {
                len: marked.start,
                ..base.clone()
            },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.0),
                    wavy: false,
                }),
                ..base.clone()
            },
            TextRun {
                len: text.len() - marked.end,
                ..base
            },
        ],
        _ => vec![base],
    };
    runs.into_iter().filter(|run| run.len > 0).collect()
}

impl EditorHost for TextField {
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
            cx.emit(TextFieldEvent::Changed(self.core.text().to_owned()));
        }
        self.follow = true;
        self.blink.reset(cx);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn finish_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow = true;
        self.blink.reset(cx);
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.core.marked().is_some() {
            return;
        }
        cx.emit(TextFieldEvent::Submitted(self.core.text().to_owned()));
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.core.marked().is_some() {
            return;
        }
        cx.emit(TextFieldEvent::Cancelled);
    }

    fn paste_media(&mut self, paths: Vec<PathBuf>, images: Vec<Image>, cx: &mut Context<Self>) {
        if !paths.is_empty() {
            cx.emit(TextFieldEvent::PastedPaths(paths));
        }
        if !images.is_empty() {
            cx.emit(TextFieldEvent::PastedImages(images));
        }
    }
}

impl EntityInputHandler for TextField {
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
        let range = self.core.range_from_utf16(&range_utf16);
        let x = element_bounds.left() - self.scroll_x + self.x_for_index(range.start);
        Some(Bounds {
            origin: point(x, self.text_top(element_bounds)),
            size: size(px(CARET_WIDTH), self.metrics.line_height),
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.bounds?;
        let index = self.index_for_x(point.x);
        Some(self.core.range_to_utf16(&(index..index)).start)
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        !self.disabled && !self.core.is_read_only()
    }
}

struct TextElement {
    field: Entity<TextField>,
}

struct Prepaint {
    hitbox: Hitbox,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_height = self.field.read(cx).metrics.line_height;
        let mut style = Style::default();
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
        style.size.width = relative(1.).into();
        style.min_size.width = px(0.0).into();
        style.min_size.height = line_height.into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.field
            .update(cx, |field, cx| field.prepare(bounds, window, cx));
        Prepaint {
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let field = self.field.clone();
        let (theme, disabled, focus_handle) = {
            let state = field.read(cx);
            (
                state.resolved_theme(cx),
                state.disabled,
                state.focus_handle.clone(),
            )
        };
        if !disabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, field.clone()),
                cx,
            );
        }
        window.set_cursor_style(
            if disabled {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            },
            &prepaint.hitbox,
        );
        // The field owns the selection started inside it (R11).
        if let Some(service) = vespertine_richtext::TextService::current() {
            service.register_region(
                vespertine_richtext::StartRegion {
                    bounds,
                    selectable: false,
                },
                window,
                cx,
            );
        }

        let (line, scroll, selection, caret, line_height, text_top) = {
            let state = field.read(cx);
            (
                state.line().cloned(),
                state.scroll_x,
                state.selection_rect(),
                state
                    .caret_visible(window, cx)
                    .then(|| state.caret_rect())
                    .flatten(),
                state.metrics.line_height,
                state.text_top(bounds),
            )
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = selection {
                window.paint_quad(fill(selection, theme.selection));
            }
            if let Some(line) = line {
                let origin = point(bounds.left() - scroll, text_top);
                let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
            }
            if let Some(caret) = caret {
                window.paint_quad(fill(caret, theme.caret));
            }
        });

        // Pointer gestures.
        let hitbox = prepaint.hitbox.clone();
        window.on_mouse_event({
            let field = field.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                field.update(cx, |field, cx| field.on_pointer_down(event, window, cx));
                cx.stop_propagation();
            }
        });
        window.on_mouse_event({
            let field = field.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    field.update(cx, |field, cx| field.on_pointer_move(event, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let field = field.clone();
            move |event: &MouseUpEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    field.update(cx, |field, cx| field.on_pointer_up(cx));
                }
            }
        });
        window.on_mouse_event({
            let field = field.clone();
            move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && hitbox.is_hovered(window)
                    && field.update(cx, |field, cx| field.on_wheel(event, cx))
                {
                    cx.stop_propagation();
                }
            }
        });

        field.update(cx, |field, cx| field.sync_blink(window, cx));
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_tab_stop();
        let theme = self.resolved_theme(cx);
        let disabled = self.disabled;
        let focused = self.focus_handle.is_focused(window);
        let surface = if disabled {
            theme.disabled_surface
        } else {
            theme.surface
        };
        let border = if focused && !disabled {
            theme.accent
        } else {
            theme.border
        };
        let mut root = div()
            .id(ElementId::from(("vespertine-text-field", cx.entity_id())))
            .key_context(FIELD_CONTEXT)
            .flex()
            .flex_row()
            .items_stretch()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(border)
            .bg(surface)
            .text_size(self.metrics.size)
            .line_height(self.metrics.line_height)
            .text_color(if disabled { theme.muted } else { theme.text })
            .role(self.role)
            .aria_value(SharedString::from(self.core.text().to_owned()))
            .aria_placeholder(self.placeholder.clone())
            .aria_read_only(self.core.is_read_only())
            .aria_disabled(disabled)
            .focus_visible(move |style| {
                style
                    .border_color(theme.accent)
                    .shadow(vec![vespertine::BoxShadow {
                        color: theme.accent,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(1.0),
                        inset: false,
                    }])
            });
        if !disabled {
            root = root
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                    cx.emit(TextFieldEvent::KeyDown(event.clone()));
                }))
                .on_key_up(cx.listener(|_, event: &KeyUpEvent, _, cx| {
                    cx.emit(TextFieldEvent::KeyUp(event.clone()));
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        // A press on the chrome around the text focuses without moving the caret.
                        window.focus(&this.focus_handle, cx);
                    }),
                );
        }
        let mut root = crate::editor_action_listeners!(root, cx, TextField);
        if let Some(leading) = self.leading.clone() {
            root = root.child(adornment(leading(window, cx)));
        }
        root = root.child(
            div()
                .flex()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(TextElement { field: cx.entity() }),
        );
        if let Some(trailing) = self.trailing.clone() {
            root = root.child(adornment(trailing(window, cx)));
        }
        root
    }
}

/// Wraps an embedded control so a press on it stops at the control (R25).
fn adornment(element: AnyElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(element)
}
