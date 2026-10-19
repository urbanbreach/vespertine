//! The segmented navigation control (CMP-017 R14, R15): a row of mutually exclusive segments
//! with stable ids. The owner passes the selected id on every render (the control is
//! controlled and never changes its own selection) and routes content itself.

use crate::{
    nav_theme::NavTheme,
    tabs::{
        TabIcon,
        chip::{HOVER_WASH, SELECTED_WASH},
    },
};
use std::rc::Rc;
use vespertine::{
    App, ClickEvent, ElementId, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, svg,
};

/// One segment of a [`SegmentedControl`].
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// The stable id passed to `on_activate`.
    pub id: SharedString,
    /// The label.
    pub label: SharedString,
    /// An optional leading icon.
    pub icon: Option<TabIcon>,
}

impl Segment {
    /// A segment with an id and a label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
        }
    }

    /// The same segment with a leading icon.
    pub fn icon(mut self, icon: TabIcon) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// The size of the segments: the tab chip styling of R7 at the segment's own size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentSize {
    /// Segment height.
    pub height: f32,
    /// Corner radius.
    pub radius: f32,
    /// Gap between segments.
    pub gap: f32,
    /// Label text size.
    pub text: f32,
    /// Horizontal padding.
    pub padding: f32,
}

impl Default for SegmentSize {
    fn default() -> Self {
        Self {
            height: 24.0,
            radius: 6.0,
            gap: 4.0,
            text: 12.0,
            padding: 10.0,
        }
    }
}

struct SegmentFocus(FocusHandle);

type ActivateCallback = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// A controlled row of segments. `id` scopes the focus state of the segments, so give every
/// control of a window its own id.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    segments: Vec<Segment>,
    selected: Option<SharedString>,
    on_activate: Option<ActivateCallback>,
    size: SegmentSize,
    theme: Option<NavTheme>,
}

impl SegmentedControl {
    /// An empty control.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
            selected: None,
            on_activate: None,
            size: SegmentSize::default(),
            theme: None,
        }
    }

    /// The segments, in order.
    pub fn segments(mut self, segments: Vec<Segment>) -> Self {
        self.segments = segments;
        self
    }

    /// The selected segment id. Passed on every render: the control never changes it.
    pub fn selected(mut self, id: impl Into<Option<SharedString>>) -> Self {
        self.selected = id.into();
        self
    }

    /// Called for every activation (click, or Enter/Space on a focused segment), including the
    /// already-selected segment. The owner decides what changes.
    pub fn on_activate(
        mut self,
        callback: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Rc::new(callback));
        self
    }

    /// The segment size.
    pub fn size(mut self, size: SegmentSize) -> Self {
        self.size = size;
        self
    }

    /// Fixes the theme roles; by default they follow the active theme.
    pub fn theme(mut self, theme: NavTheme) -> Self {
        self.theme = Some(theme);
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme.clone().unwrap_or_else(|| NavTheme::of(cx));
        let size = self.size;
        let control_id = self.id.clone();
        let mut segments = Vec::with_capacity(self.segments.len());
        for segment in &self.segments {
            let selected = self.selected.as_ref() == Some(&segment.id);
            let focus = window.with_element_namespace(control_id.clone(), |window| {
                window
                    .use_keyed_state(
                        ElementId::Name(format!("segment-focus:{}", segment.id).into()),
                        cx,
                        |_, cx| SegmentFocus(cx.focus_handle().tab_stop(true)),
                    )
                    .read(cx)
                    .0
                    .clone()
            });
            let id = segment.id.clone();
            let on_activate = self.on_activate.clone();
            let color = if selected { theme.text } else { theme.muted };
            let hover_wash = theme.wash(HOVER_WASH);
            let accent = theme.accent;
            let icon = segment.icon.clone();
            segments.push(
                div()
                    .id(ElementId::Name(format!("segment:{id}").into()))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("segment:{id}")
                    })
                    .role(Role::Tab)
                    .aria_label(segment.label.clone())
                    .aria_selected(selected)
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap(px(4.))
                    .h(px(size.height))
                    .px(px(size.padding))
                    .rounded(px(size.radius))
                    .text_size(px(size.text))
                    .text_color(color)
                    .cursor_pointer()
                    .when(selected, |segment| segment.bg(theme.wash(SELECTED_WASH)))
                    .when(!selected, |segment| {
                        segment.hover(move |style| style.bg(hover_wash))
                    })
                    .track_focus(&focus)
                    .focus_visible(move |style| style.border_2().border_color(accent))
                    .when_some(icon, |segment, icon| match icon {
                        TabIcon::Svg(path) => {
                            segment.child(svg().path(path).size(px(12.)).text_color(color))
                        }
                        TabIcon::Glyph(glyph) => segment.child(glyph),
                    })
                    .child(segment.label.clone())
                    .on_click(move |_: &ClickEvent, window, cx| {
                        if let Some(callback) = &on_activate {
                            callback(&id, window, cx);
                        }
                    }),
            );
        }
        div()
            .id(control_id)
            .role(Role::TabList)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(size.gap))
            .children(segments)
    }
}
