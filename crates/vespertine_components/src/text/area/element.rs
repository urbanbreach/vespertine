//! The element and render function of the multiline editor.

use super::{TextArea, TextAreaEvent, WASH_INSET_Y, WASH_RADIUS, layout::painted_height};
use crate::{
    layout::paint_left_aligned,
    text::{actions::*, editor::EditorHost},
};
use vespertine::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Context, Corners, CursorStyle,
    DispatchPhase, Element, ElementId, ElementInputHandler, Entity, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Render, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Style, Styled, Window, div,
    fill, point, px, relative, size,
};

struct AreaElement {
    area: Entity<TextArea>,
}

struct Prepaint {
    hitbox: Hitbox,
}

impl IntoElement for AreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AreaElement {
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
        let font = window.text_style().font();
        self.area.update(cx, |area, _| {
            area.resolved_font = area.font_override().unwrap_or(font);
        });
        let area = self.area.clone();
        let mut style = Style::default();
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
        style.size.width = relative(1.).into();
        style.min_size.width = px(0.0).into();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.unwrap_or(match available.width {
                AvailableSpace::Definite(width) => width,
                AvailableSpace::MinContent | AvailableSpace::MaxContent => px(240.0),
            });
            let height = area.update(cx, |area, cx| area.measure(width, window, cx));
            size(width, known.height.unwrap_or(height))
        });
        (id, ())
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
        self.area
            .update(cx, |area, cx| area.prepare(bounds, window, cx));
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
        let area = self.area.clone();
        let (theme, disabled, focus_handle) = {
            let state = area.read(cx);
            (
                state.resolved_theme_for_paint(cx),
                state.is_disabled(),
                state.focus_ref().clone(),
            )
        };
        if !disabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, area.clone()),
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
        // The editor owns the selection started inside it, also inside a modal (R20).
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

        let state = area.read(cx);
        let layout = state.layout();
        let line_height = state.metrics().line_height;
        let origin = state.content_origin(bounds);
        let selection = state.selection_rects();
        let caret = state
            .caret_visible(window, cx)
            .then(|| state.caret_rect())
            .flatten();
        let wash = state.wash().zip(state.line_wash_rect(bounds));
        let span_washes = state.wash_quads();
        let empty = state.is_empty();
        let placeholder = state.placeholder_ref().clone();
        let font = state.resolved_font.clone();
        let size_px = state.metrics().size;
        let animating = state.settled_height_value().is_some_and(|settled| {
            (f32::from(settled) - f32::from(bounds.size.height)).abs() > 0.5
        });
        let scroll = state.scroll_y;
        let inset = state.content_inset_value();
        let clip_height = if animating {
            painted_height(scroll, bounds.size.height, line_height)
        } else {
            bounds.size.height
        };
        let clip = Bounds {
            origin: bounds.origin,
            size: size(bounds.size.width, clip_height),
        };

        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            if let Some((color, rect)) = wash {
                window.paint_quad(fill(rect, color));
            }
            for quad in span_washes {
                let rect = Bounds {
                    origin: point(
                        quad.bounds.origin.x,
                        quad.bounds.origin.y + px(WASH_INSET_Y),
                    ),
                    size: size(
                        quad.bounds.size.width,
                        (quad.bounds.size.height - px(2.0 * WASH_INSET_Y)).max(px(0.0)),
                    ),
                };
                let left = if quad.round_left {
                    px(WASH_RADIUS)
                } else {
                    px(0.0)
                };
                let right = if quad.round_right {
                    px(WASH_RADIUS)
                } else {
                    px(0.0)
                };
                window.paint_quad(fill(rect, quad.color).corner_radii(Corners {
                    top_left: left,
                    bottom_left: left,
                    top_right: right,
                    bottom_right: right,
                }));
            }
            for rect in selection {
                window.paint_quad(fill(rect, theme.selection));
            }
            if empty {
                if !placeholder.is_empty() {
                    let run = vespertine::TextRun {
                        len: placeholder.len(),
                        font,
                        color: theme.faint,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window.text_system().shape_line(
                        SharedString::from(placeholder.replace('\n', " ")),
                        size_px,
                        &[run],
                        None,
                    );
                    paint_left_aligned(&line, point(origin.x, origin.y), line_height, window, cx);
                }
            } else if let Some(layout) = layout {
                let top = bounds.top() - line_height;
                let bottom = bounds.bottom();
                for line in &layout.lines {
                    let rows = line.shape.row_count();
                    let line_top = origin.y + line.top;
                    if line_top + line_height * rows < top {
                        continue;
                    }
                    if line_top > bottom {
                        break;
                    }
                    for row in 0..rows {
                        let row_top = line_top + line_height * row;
                        if row_top + line_height < top || row_top > bottom {
                            continue;
                        }
                        let x = if row == 0 {
                            origin.x
                        } else {
                            origin.x + line.shape.indent
                        };
                        paint_left_aligned(
                            &line.shape.rows[row],
                            point(x, row_top),
                            line_height,
                            window,
                            cx,
                        );
                    }
                }
            }
            if let Some(caret) = caret {
                window.paint_quad(fill(caret, theme.caret));
            }
        });
        let _ = inset;

        let hitbox = prepaint.hitbox.clone();
        window.on_mouse_event({
            let area = area.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                match event.button {
                    MouseButton::Left => {
                        area.update(cx, |area, cx| area.on_pointer_down(event, window, cx));
                        cx.stop_propagation();
                    }
                    MouseButton::Right => {
                        area.update(cx, |_, cx| {
                            cx.emit(TextAreaEvent::ContextMenu(event.position))
                        });
                    }
                    _ => {}
                }
            }
        });
        window.on_mouse_event({
            let area = area.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    area.update(cx, |area, cx| area.on_pointer_move(event, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let area = area.clone();
            move |event: &MouseUpEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    area.update(cx, |area, cx| area.on_pointer_up(cx));
                }
            }
        });
        window.on_mouse_event({
            let area = area.clone();
            move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && hitbox.is_hovered(window)
                    && area.update(cx, |area, cx| area.on_wheel(event, cx))
                {
                    cx.stop_propagation();
                }
            }
        });
        area.update(cx, |area, cx| area.sync_blink(window, cx));
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_tab_stop();
        let theme = self.resolved_theme_for_paint(cx);
        let disabled = self.is_disabled();
        let focused = self.focus_ref().is_focused(window);
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
        let metrics = self.metrics();
        let mut root = div()
            .id(ElementId::from(("vespertine-text-area", cx.entity_id())))
            .key_context(self.mode_context().as_str())
            .flex()
            .flex_col()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(border)
            .bg(surface)
            .text_size(metrics.size)
            .line_height(metrics.line_height)
            .text_color(if disabled { theme.muted } else { theme.text })
            .role(self.role_value())
            .aria_value(SharedString::from(self.source().to_owned()))
            .aria_placeholder(self.placeholder_ref().clone())
            .aria_read_only(self.is_read_only())
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
                .track_focus(self.focus_ref())
                .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                    cx.emit(TextAreaEvent::KeyDown(event.clone()));
                }))
                .on_key_up(cx.listener(|_, event: &KeyUpEvent, _, cx| {
                    cx.emit(TextAreaEvent::KeyUp(event.clone()));
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        window.focus(this.focus_ref(), cx);
                    }),
                );
        }
        let root = crate::editor_action_listeners!(root, cx, TextArea)
            .on_action(cx.listener(|this: &mut TextArea, _: &Up, window, cx| {
                this.move_vertical(false, false, window, cx)
            }))
            .on_action(cx.listener(|this: &mut TextArea, _: &Down, window, cx| {
                this.move_vertical(true, false, window, cx)
            }))
            .on_action(
                cx.listener(|this: &mut TextArea, _: &SelectUp, window, cx| {
                    this.move_vertical(false, true, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this: &mut TextArea, _: &SelectDown, window, cx| {
                    this.move_vertical(true, true, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this: &mut TextArea, _: &DocumentStart, window, cx| {
                    this.move_with(window, cx, |core| core.move_document_start(false))
                }),
            )
            .on_action(
                cx.listener(|this: &mut TextArea, _: &DocumentEnd, window, cx| {
                    this.move_with(window, cx, |core| core.move_document_end(false))
                }),
            )
            .on_action(
                cx.listener(|this: &mut TextArea, _: &SelectDocumentStart, window, cx| {
                    this.move_with(window, cx, |core| core.move_document_start(true))
                }),
            )
            .on_action(
                cx.listener(|this: &mut TextArea, _: &SelectDocumentEnd, window, cx| {
                    this.move_with(window, cx, |core| core.move_document_end(true))
                }),
            )
            .on_action(cx.listener(|this: &mut TextArea, _: &Newline, window, cx| {
                this.insert_newline(window, cx)
            }))
            .on_action(
                cx.listener(|this: &mut TextArea, _: &ModifiedSubmit, window, cx| {
                    if !this.modified_submit(window, cx) {
                        cx.propagate();
                    }
                }),
            )
            .on_action(cx.listener(|this: &mut TextArea, _: &Indent, window, cx| {
                if !this.indent(false, window, cx) {
                    window.focus_next(cx);
                }
            }))
            .on_action(cx.listener(|this: &mut TextArea, _: &Outdent, window, cx| {
                if !this.indent(true, window, cx) {
                    window.focus_prev(cx);
                }
            }));
        root.child(
            div()
                .flex()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(AreaElement { area: cx.entity() }),
        )
    }
}

#[allow(dead_code)]
fn _assert_any_element(_: AnyElement) {}
