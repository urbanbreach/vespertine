#[path = "example_support/fonts.rs"]
mod example_support;

use std::ops::Range;

use unicode_segmentation::*;
use vespertine::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, WindowBounds,
    WindowOptions, actions, black, div, fill, hsla,
    panes::{
        MovingSurface, MovingSurfaceState, PaneDescriptor, PaneEvent, PaneGroupState, pane_group,
    },
    point,
    prelude::*,
    px, relative, rgb, rgba, size, white, yellow,
};
use vespertine_platform::application;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

actions!(
    pane_group,
    [
        ToggleLeading,
        ToggleTrailing,
        ToggleBottom,
        ToggleDock,
        Quit
    ]
);

struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl TextInput {
    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), window, cx);
        } else {
            self.move_to(self.selected_range.start, window, cx)
        }
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), window, cx);
        } else {
            self.move_to(self.selected_range.end, window, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx)
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        window.request_virtual_keyboard();

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), window, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\n", " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

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

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
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
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
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
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start_utf16 + selected.start.min(inserted_length_utf16)
                        ..insertion_start_utf16 + selected.end.min(inserted_length_utf16)),
                )
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;

        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        if last_layout.text != self.content {
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: vespertine::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        if last_layout.text != self.content {
            return None;
        }
        let utf8_index = last_layout.closest_index_for_x(line_point.x);
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&vespertine::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&vespertine::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0., 0.2))
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    vespertine::blue(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x3311ff30),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&vespertine::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(
            bounds.origin,
            window.line_height(),
            vespertine::TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .bg(rgb(0xeeeeee))
            .line_height(px(30.))
            .text_size(px(24.))
            .child(
                div()
                    .h(px(30. + 4. * 2.))
                    .w_full()
                    .p(px(4.))
                    .bg(white())
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

const LEADING: &str = "leading";
const TRAILING: &str = "trailing";
const BOTTOM: &str = "bottom";

/// Where the retained input rests while docked, relative to its layout slot.
const DOCKED_ROUTE: (f32, f32) = (48., 400.);

struct PaneExample {
    panes: Entity<PaneGroupState>,
    text_input: Entity<TextInput>,
    surface: MovingSurfaceState,
    docked: bool,
    last_event: SharedString,
    focus_handle: FocusHandle,
}

impl PaneExample {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panes = cx.new(|_| {
            PaneGroupState::new([
                PaneDescriptor::leading(LEADING),
                PaneDescriptor::trailing(TRAILING).collapsed(true),
                PaneDescriptor::bottom(BOTTOM).collapsed(true),
            ])
        });
        cx.subscribe(&panes, |this, _, event: &PaneEvent, cx| {
            this.last_event = format!("{event:?}").into();
            cx.notify();
        })
        .detach();
        let text_input = cx.new(|cx| TextInput {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: "Type here, then open, resize and collapse panes...".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        });
        window.focus(&text_input.focus_handle(cx), cx);
        Self {
            panes,
            text_input,
            surface: MovingSurfaceState::default(),
            docked: false,
            last_event: "Drag a seam, double-click it to reset".into(),
            focus_handle: cx.focus_handle(),
        }
    }

    fn toggle(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.panes
            .update(cx, |panes, cx| panes.toggle_collapsed(&id.into(), cx));
    }

    fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        self.docked = !self.docked;
        cx.notify();
    }
}

impl Focusable for PaneExample {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2()
        .border_1()
        .border_color(black())
        .bg(yellow())
        .child(label)
        .hover(|style| style.cursor_pointer())
        .on_click(move |_, window, cx| on_click(window, cx))
}

fn panel(color: u32, label: &'static str) -> impl IntoElement {
    div().size_full().bg(rgb(color)).p_2().child(label)
}

impl Render for PaneExample {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let metrics = self.panes.read(cx).metrics();
        let route = if self.docked {
            point(px(DOCKED_ROUTE.0), px(DOCKED_ROUTE.1))
        } else {
            point(px(0.), px(0.))
        };
        let view = cx.entity();
        let toggle = |id: &'static str, label: &'static str| {
            let view = view.clone();
            button(id, label, move |_, cx| {
                view.update(cx, |this, cx| this.toggle(id, cx))
            })
        };
        let dock = {
            let view = view.clone();
            button("dock", "Dock input (ctrl-4)", move |_, cx| {
                view.update(cx, |this, cx| this.toggle_dock(cx))
            })
        };

        let main = div().size_full().bg(rgb(0xf4f4f4)).p_2().child(
            MovingSurface::new(
                &self.surface,
                div().w(px(360.)).child(self.text_input.clone()),
            )
            .route(route)
            .clear_bottom(metrics),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ToggleLeading, _, cx| this.toggle(LEADING, cx)))
            .on_action(cx.listener(|this, _: &ToggleTrailing, _, cx| this.toggle(TRAILING, cx)))
            .on_action(cx.listener(|this, _: &ToggleBottom, _, cx| this.toggle(BOTTOM, cx)))
            .on_action(cx.listener(|this, _: &ToggleDock, _, cx| this.toggle_dock(cx)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_2()
                    .bg(rgb(0xaaaaaa))
                    .child(toggle("t-leading", "Sidebar (ctrl-1)"))
                    .child(toggle("t-trailing", "Detail (ctrl-2)"))
                    .child(toggle("t-bottom", "Bottom (ctrl-3)"))
                    .child(dock)
                    .child(self.last_event.clone()),
            )
            .child(
                div().flex_1().min_h_0().child(
                    pane_group("panes", &self.panes)
                        .main(main)
                        .pane(LEADING, panel(0xdde6f0, "Sidebar"))
                        .pane(TRAILING, panel(0xf0e6dd, "Detail"))
                        .pane(BOTTOM, panel(0xe0f0dd, "Bottom")),
                ),
            )
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(1100.0), px(700.0)), cx);
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", Delete, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("shift-left", SelectLeft, None),
            KeyBinding::new("shift-right", SelectRight, None),
            KeyBinding::new("cmd-a", SelectAll, None),
            KeyBinding::new("cmd-v", Paste, None),
            KeyBinding::new("cmd-c", Copy, None),
            KeyBinding::new("cmd-x", Cut, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, None),
            KeyBinding::new("ctrl-1", ToggleLeading, None),
            KeyBinding::new("ctrl-2", ToggleTrailing, None),
            KeyBinding::new("ctrl-3", ToggleBottom, None),
            KeyBinding::new("ctrl-4", ToggleDock, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| PaneExample::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}

fn main() {
    run_example();
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use std::time::Duration;
    use vespertine::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, TestAppContext,
        VisualTestContext, WindowHandle,
    };

    #[derive(Clone, Debug, PartialEq)]
    struct InputSnapshot {
        content: SharedString,
        selected: Range<usize>,
        marked: Option<Range<usize>>,
    }

    struct Fixture {
        cx: TestAppContext,
        vcx: VisualTestContext,
        window: WindowHandle<PaneExample>,
        input: Entity<TextInput>,
        panes: Entity<PaneGroupState>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut cx = TestAppContext::single();
            let window = cx.open_window(size(px(1100.), px(700.)), PaneExample::new);
            let vcx = VisualTestContext::from_window(window.into(), &cx);
            let (input, panes) = cx.read(|cx| {
                let view = window.read(cx).unwrap();
                (view.text_input.clone(), view.panes.clone())
            });
            let mut fixture = Self {
                cx,
                vcx,
                window,
                input,
                panes,
            };
            fixture.frame(0);
            fixture
        }

        fn frame(&mut self, millis: u64) {
            self.cx
                .executor()
                .advance_clock(Duration::from_millis(millis));
            self.vcx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn settle(&mut self) {
            for _ in 0..12 {
                self.frame(50);
            }
        }

        fn snapshot(&self) -> InputSnapshot {
            self.cx.read(|cx| {
                let input = self.input.read(cx);
                InputSnapshot {
                    content: input.content.clone(),
                    selected: input.selected_range.clone(),
                    marked: input.marked_range.clone(),
                }
            })
        }

        fn focused(&mut self) -> bool {
            let focus = self.cx.read(|cx| self.input.read(cx).focus_handle.clone());
            self.vcx.update(|window, _| focus.is_focused(window))
        }

        fn input_origin(&self) -> Point<Pixels> {
            self.cx
                .read(|cx| self.input.read(cx).last_bounds.unwrap().origin)
        }

        fn pane_update(
            &mut self,
            f: impl FnOnce(&mut PaneGroupState, &mut Context<PaneGroupState>),
        ) {
            let panes = self.panes.clone();
            self.cx.update(|cx| panes.update(cx, f));
        }

        fn mouse(&mut self, x: f32, y: f32, down: Option<bool>, pressed: bool) {
            let position = point(px(x), px(y));
            match down {
                Some(true) => self.vcx.simulate_event(MouseDownEvent {
                    position,
                    modifiers: Modifiers::default(),
                    button: MouseButton::Left,
                    click_count: 1,
                    first_mouse: false,
                }),
                Some(false) => self.vcx.simulate_event(MouseUpEvent {
                    position,
                    modifiers: Modifiers::default(),
                    button: MouseButton::Left,
                    click_count: 1,
                }),
                None => self.vcx.simulate_event(MouseMoveEvent {
                    position,
                    modifiers: Modifiers::default(),
                    pressed_button: pressed.then_some(MouseButton::Left),
                }),
            }
        }
    }

    /// LAY-004 A1: typing into the retained input while a side pane and a
    /// bottom pane open, resize, collapse and restore, and the input travels.
    #[test]
    fn input_survives_pane_open_resize_collapse_restore() {
        let mut f = Fixture::new();
        assert!(f.focused());

        // Type, select two characters and start a composition over them.
        f.vcx.simulate_input("hello");
        assert_eq!(f.snapshot().content, "hello");
        f.vcx.dispatch_action(SelectLeft);
        f.vcx.dispatch_action(SelectLeft);
        let window = f.window;
        let input = f.input.clone();
        f.cx.update(|cx| {
            window
                .update(cx, |_, window, cx| {
                    input.update(cx, |input, cx| {
                        input.replace_and_mark_text_in_range(None, "ab", Some(1..2), window, cx)
                    })
                })
                .unwrap()
        });
        f.frame(16);
        let expected = f.snapshot();
        assert_eq!(expected.content, "helab");
        assert_eq!(expected.marked, Some(3..5));
        assert_eq!(expected.selected, 4..5);

        let check = |f: &mut Fixture, what: &str| {
            assert_eq!(
                f.snapshot(),
                expected,
                "{what}: text, selection, composition"
            );
            assert!(f.focused(), "{what}: focus");
        };

        // Open the bottom and trailing panes (animated), then collapse the sidebar.
        f.pane_update(|panes, cx| panes.set_collapsed(&BOTTOM.into(), false, cx));
        for _ in 0..4 {
            f.frame(20);
            check(&mut f, "bottom opening");
        }
        f.pane_update(|panes, cx| panes.set_collapsed(&TRAILING.into(), false, cx));
        for _ in 0..4 {
            f.frame(20);
            check(&mut f, "trailing opening");
        }
        f.settle();
        check(&mut f, "opened");
        let origin_open = f.input_origin();

        // Reverse mid-flight.
        f.pane_update(|panes, cx| panes.set_collapsed(&LEADING.into(), true, cx));
        f.frame(60);
        f.pane_update(|panes, cx| panes.set_collapsed(&LEADING.into(), false, cx));
        f.frame(60);
        check(&mut f, "reversal");
        f.settle();

        // Drag the bottom seam with the pointer: the input keeps everything.
        let seam = f.cx.read(|cx| {
            f.panes
                .read(cx)
                .metrics()
                .frame()
                .seams
                .iter()
                .find(|s| s.pane == BOTTOM.into())
                .unwrap()
                .bounds
        });
        let (x, y) = (f32::from(seam.center().x), f32::from(seam.center().y));
        f.mouse(x, y, None, false);
        f.mouse(x, y, Some(true), false);
        f.frame(0);
        for dy in [-40., -80., 30.] {
            f.mouse(x, y + dy, None, true);
            f.frame(0);
            check(&mut f, "bottom seam drag");
        }
        f.mouse(x, y + 30., Some(false), false);
        f.frame(0);

        // Dock the input: it travels toward the bottom reservation.
        let window = f.window;
        f.cx.update(|cx| {
            window
                .update(cx, |view, _, cx| view.toggle_dock(cx))
                .unwrap()
        });
        f.frame(0);
        for _ in 0..4 {
            f.frame(40);
            check(&mut f, "docking");
        }
        f.settle();
        check(&mut f, "docked");
        let origin_docked = f.input_origin();
        assert!(
            origin_docked.y > origin_open.y,
            "the input's bounds (caret, IME) travel with it"
        );

        // Collapse everything, shrink the window until fitting hides panes, restore.
        for id in [LEADING, TRAILING, BOTTOM] {
            f.pane_update(|panes, cx| panes.set_collapsed(&id.into(), true, cx));
            f.frame(80);
            check(&mut f, "collapsing");
        }
        f.settle();
        for id in [LEADING, TRAILING, BOTTOM] {
            f.pane_update(|panes, cx| panes.set_collapsed(&id.into(), false, cx));
        }
        f.vcx.simulate_resize(size(px(700.), px(500.)));
        f.frame(16);
        check(&mut f, "narrow window");
        f.vcx.simulate_resize(size(px(1100.), px(700.)));
        f.settle();
        check(&mut f, "restored");

        // Typing continues into the same composition.
        f.vcx.simulate_input("c");
        assert!(f.snapshot().content.contains('c'));
        assert!(f.focused());
    }
}
