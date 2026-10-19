//! The single-line address text field (CMP-022 R10) with real text editing: cursor, selection,
//! word movement, clipboard, input-method composition (through [`EntityInputHandler`]) and
//! horizontal scrolling. It is deliberately small and owns no key bindings: the keys it handles
//! are matched in its key listener, so it never competes with the embedder's keymap.

use super::theme::BrowserTheme;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use vespertine::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels,
    Point, Render, Role, ShapedLine, SharedString, StatefulInteractiveElement, Style, Styled,
    TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, div, fill, point, px, relative,
    size,
};

/// Notifications of the address field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressEvent {
    /// The user changed the text (typing, deleting, pasting, composition).
    Edited,
    /// Enter without shift was pressed (R10).
    Submit,
    /// Escape was pressed (R10).
    Escape,
}

/// The address field entity.
pub struct AddressInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_x: Pixels,
    is_selecting: bool,
    /// An explicit override of the colours; without one the active theme is read at paint time.
    theme: Option<BrowserTheme>,
    label: SharedString,
    select_all_on_focus: bool,
    _subscriptions: Vec<vespertine::Subscription>,
}

impl EventEmitter<AddressEvent> for AddressInput {}

impl Focusable for AddressInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AddressInput {
    /// An empty field. Gaining focus selects the whole text (R10).
    pub fn new(
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![cx.on_focus(&focus_handle, window, |this, window, cx| {
            if this.select_all_on_focus {
                this.select_all(window, cx);
            }
        })];
        Self {
            focus_handle,
            content: SharedString::default(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_x: px(0.),
            is_selecting: false,
            theme: None,
            label: SharedString::new_static("Address"),
            select_all_on_focus: true,
            _subscriptions: subscriptions,
        }
    }

    /// The text of the field.
    pub fn text(&self) -> &str {
        &self.content
    }

    /// The selected byte range of the text.
    pub fn selection(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    /// Whether the input method has marked (uncommitted) text in the field.
    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    /// Whether the field has keyboard focus.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// Replaces the text programmatically (no edited notification), placing the cursor at the end.
    pub fn set_text(&mut self, text: &str, window: &Window, cx: &mut Context<Self>) {
        self.content = sanitize(text).into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_x = px(0.);
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    /// Sets the placeholder shown while the field is empty.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Replaces the colours with an explicit set, or `None` to follow the active theme again.
    pub fn set_theme(&mut self, theme: Option<BrowserTheme>, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Sets the accessible label (default "Address").
    pub fn set_label(&mut self, label: impl Into<SharedString>) {
        self.label = label.into();
    }

    /// Whether gaining focus selects the whole text (default `true`). Embedders that place the
    /// selection themselves, such as an inline rename, turn it off.
    pub fn set_select_all_on_focus(&mut self, select_all: bool) {
        self.select_all_on_focus = select_all;
    }

    /// Selects the byte `range` of the text (clamped to the text and to character boundaries).
    pub fn set_selection(&mut self, range: Range<usize>, window: &Window, cx: &mut Context<Self>) {
        let clamp = |offset: usize| {
            let mut offset = offset.min(self.content.len());
            while !self.content.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        let (start, end) = (clamp(range.start), clamp(range.end));
        self.selected_range = start.min(end)..start.max(end);
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    /// Selects the whole text.
    pub fn select_all(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    // ---- selection and cursor ---------------------------------------------------------------

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn previous_word(&self, offset: usize) -> usize {
        let mut result = 0;
        for (index, segment) in self.content.split_word_bound_indices() {
            if index >= offset {
                break;
            }
            if !segment.trim().is_empty() {
                result = index;
            }
        }
        result
    }

    fn next_word(&self, offset: usize) -> usize {
        for (index, segment) in self.content.split_word_bound_indices() {
            let end = index + segment.len();
            if end > offset && !segment.trim().is_empty() {
                return end;
            }
        }
        self.content.len()
    }

    fn word_range_at(&self, offset: usize) -> Range<usize> {
        for (index, segment) in self.content.split_word_bound_indices() {
            let end = index + segment.len();
            if offset < end || end == self.content.len() {
                return index..end;
            }
        }
        0..self.content.len()
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        line.closest_index_for_x(position.x - bounds.left() + self.scroll_x)
            .min(self.content.len())
    }

    // ---- editing ----------------------------------------------------------------------------

    fn edit(&mut self, range: Range<usize>, text: &str, window: &Window, cx: &mut Context<Self>) {
        let text = sanitize(text);
        self.content =
            (self.content[0..range.start].to_owned() + &text + &self.content[range.end..]).into();
        let cursor = range.start + text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(AddressEvent::Edited);
        cx.notify();
    }

    fn delete_selection_or(
        &mut self,
        range: impl FnOnce(&Self) -> Range<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selected_range.is_empty() {
            range(self)
        } else {
            self.selected_range.clone()
        };
        if !range.is_empty() {
            self.edit(range, "", window, cx);
        }
    }

    fn copy(&self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
        }
    }

    fn paste_clipboard(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let range = self
                .marked_range
                .clone()
                .unwrap_or(self.selected_range.clone());
            self.edit(range, &text, window, cx);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let primary = modifiers.control;
        let plain = !primary && !modifiers.alt && !modifiers.platform;
        let mut handled = true;
        match keystroke.key.as_str() {
            "enter" if plain && !modifiers.shift => cx.emit(AddressEvent::Submit),
            "escape" if plain => cx.emit(AddressEvent::Escape),
            "enter" | "escape" => {}
            "left" | "right" | "home" | "end" if !modifiers.alt && !modifiers.platform => {
                let cursor = self.cursor_offset();
                let target = match (keystroke.key.as_str(), primary) {
                    ("left", false) if self.selected_range.is_empty() || modifiers.shift => {
                        self.previous_boundary(cursor)
                    }
                    ("left", false) => self.selected_range.start,
                    ("left", true) => self.previous_word(cursor),
                    ("right", false) if self.selected_range.is_empty() || modifiers.shift => {
                        self.next_boundary(cursor)
                    }
                    ("right", false) => self.selected_range.end,
                    ("right", true) => self.next_word(cursor),
                    ("home", _) => 0,
                    _ => self.content.len(),
                };
                if modifiers.shift {
                    self.select_to(target, window, cx);
                } else {
                    self.move_to(target, window, cx);
                }
            }
            "backspace" if !modifiers.alt && !modifiers.platform => {
                self.delete_selection_or(
                    |this| {
                        let cursor = this.cursor_offset();
                        let start = if primary {
                            this.previous_word(cursor)
                        } else {
                            this.previous_boundary(cursor)
                        };
                        start..cursor
                    },
                    window,
                    cx,
                );
            }
            "delete" if !modifiers.alt && !modifiers.platform => {
                self.delete_selection_or(
                    |this| {
                        let cursor = this.cursor_offset();
                        let end = if primary {
                            this.next_word(cursor)
                        } else {
                            this.next_boundary(cursor)
                        };
                        cursor..end
                    },
                    window,
                    cx,
                );
            }
            "a" if primary && !modifiers.alt => self.select_all(window, cx),
            "c" if primary && !modifiers.alt => self.copy(cx),
            "x" if primary && !modifiers.alt => {
                self.copy(cx);
                self.delete_selection_or(|_| 0..0, window, cx);
            }
            "v" if primary && !modifiers.alt => self.paste_clipboard(window, cx),
            _ => handled = false,
        }
        if handled {
            cx.stop_propagation();
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.request_virtual_keyboard();
        if !self.focus_handle.is_focused(window) {
            // A click focuses the field and selects the whole text (R10).
            window.focus(&self.focus_handle, cx);
            self.select_all(window, cx);
            return;
        }
        match event.click_count {
            0 | 1 => {
                self.is_selecting = true;
                let index = self.index_for_position(event.position);
                if event.modifiers.shift {
                    self.select_to(index, window, cx);
                } else {
                    self.move_to(index, window, cx);
                }
            }
            2 => {
                let range = self.word_range_at(self.index_for_position(event.position));
                self.selected_range = range;
                self.selection_reversed = false;
                window.notify_text_selection_changed(&self.focus_handle);
                cx.notify();
            }
            _ => self.select_all(window, cx),
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let index = self.index_for_position(event.position);
            self.select_to(index, window, cx);
        }
    }

    // ---- UTF-16 conversion for the platform text-input handler ------------------------------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

/// Removes control characters: the field is a single line and R3 rejects them anyway.
fn sanitize(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

impl EntityInputHandler for AddressInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.edit(range, new_text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let new_text = sanitize(new_text);
        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
                .into();
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let insertion_start = self.offset_to_utf16(range.start);
        let inserted_length = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start + selected.start.min(inserted_length)
                        ..insertion_start + selected.end.min(inserted_length)),
                )
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(AddressEvent::Edited);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        if layout.text != self.content {
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + layout.x_for_index(range.start) - self.scroll_x,
                bounds.top(),
            ),
            point(
                bounds.left() + layout.x_for_index(range.end) - self.scroll_x,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.last_layout.as_ref()?;
        if layout.text != self.content {
            return None;
        }
        let local = self.last_bounds?.localize(&point)?;
        let index = layout.closest_index_for_x(local.x + self.scroll_x);
        Some(self.offset_to_utf16(index))
    }
}

impl Render for AddressInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("browser-address-input")
            .role(Role::TextInput)
            .aria_label(self.label.clone())
            .aria_placeholder(self.placeholder.clone())
            .aria_value(self.content.clone())
            .key_context("BrowserAddress")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(AddressElement { input: cx.entity() })
    }
}

/// The element that shapes, scrolls and paints the text, the selection and the caret.
struct AddressElement {
    input: Entity<AddressInput>,
}

struct AddressPrepaint {
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for AddressElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for AddressElement {
    type RequestLayoutState = ();
    type PrepaintState = AddressPrepaint;

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
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AddressPrepaint {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let marked_range = input.marked_range.clone();
        let theme = input
            .theme
            .clone()
            .unwrap_or_else(|| BrowserTheme::current(cx));
        let mut scroll_x = input.scroll_x;
        let style = window.text_style();

        let (display_text, color) = if content.is_empty() {
            (input.placeholder.clone(), theme.faint)
        } else {
            (content, style.color)
        };
        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match marked_range {
            Some(marked) if !input.content.is_empty() => [
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            _ => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        // Keep the cursor inside the field.
        let cursor_x = line.x_for_index(cursor_offset);
        if cursor_x - scroll_x > bounds.size.width - px(2.) {
            scroll_x = cursor_x - bounds.size.width + px(2.);
        }
        if cursor_x < scroll_x {
            scroll_x = cursor_x;
        }
        scroll_x = scroll_x.clamp(
            px(0.),
            (line.width - bounds.size.width + px(2.)).max(px(0.)),
        );
        if input.content.is_empty() {
            scroll_x = px(0.);
        }

        let origin_x = bounds.left() - scroll_x;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(origin_x + cursor_x, bounds.top()),
                        size(px(1.), bounds.size.height),
                    ),
                    theme.text,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            origin_x + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            origin_x + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    theme.strong_border.opacity(0.5),
                )),
                None,
            )
        };
        AddressPrepaint {
            line,
            cursor,
            selection,
            scroll_x,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut AddressPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let focused = focus_handle.is_focused(window);
        let line_height = window.line_height();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if focused && let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }
            let origin = point(bounds.left() - prepaint.scroll_x, bounds.top());
            let _ = prepaint
                .line
                .paint(origin, line_height, TextAlign::Left, None, window, cx);
            if focused && let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });
        let line = prepaint.line.clone();
        let scroll_x = prepaint.scroll_x;
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll_x = scroll_x;
        });
    }
}
