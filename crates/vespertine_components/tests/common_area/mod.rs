//! Shared offscreen fixture of the editor integration tests.
#![allow(dead_code)]

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};
use vespertine::{
    AppContext as _, Context, Entity, EntityInputHandler as _, InteractiveElement, IntoElement,
    KeyBinding, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, PlatformInput, Point, Render, Styled, Window, div, point, px,
};
use vespertine_components::text::{TextArea, TextAreaEvent, actions::bind_default_keys};
use vespertine_richtext::{RichText, text_root};
use vespertine_test_host::{HostOptions, TestHost};

pub struct Root {
    pub area: Entity<TextArea>,
    pub wheel: Rc<Cell<u32>>,
    pub with_document: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wheel = self.wheel.clone();
        let holder = div().w_full().child(self.area.clone());
        let body = if self.with_document {
            div()
                .w_full()
                .child(RichText::new("doc", "Background document text to select"))
                .child(holder)
        } else {
            holder
        };
        let _ = cx;
        text_root(
            div()
                .id("root")
                .size_full()
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1))
                .child(body),
        )
    }
}

pub struct Fixture {
    pub host: TestHost,
    pub area: Entity<TextArea>,
    pub events: Rc<RefCell<Vec<TextAreaEvent>>>,
    pub wheel: Rc<Cell<u32>>,
    _subscription: vespertine::Subscription,
}

pub fn fixture(
    width: f64,
    height: f64,
    with_document: bool,
    configure: impl FnOnce(TextArea) -> TextArea + 'static,
) -> Fixture {
    let events = Rc::new(RefCell::new(Vec::new()));
    let wheel = Rc::new(Cell::new(0));
    let slot: Rc<RefCell<Option<(Entity<TextArea>, vespertine::Subscription)>>> = Rc::default();
    let (build_slot, build_events, build_wheel) = (slot.clone(), events.clone(), wheel.clone());
    let host = TestHost::with_view(HostOptions::with_size(width, height), move |window, cx| {
        bind_default_keys(cx);
        cx.bind_keys([KeyBinding::new("ctrl-k", GlobalShortcut, None)]);
        let area = cx.new(|cx| configure(TextArea::new(window, cx)));
        let subscription = cx.subscribe(&area, move |_, event: &TextAreaEvent, _| {
            build_events.borrow_mut().push(event.clone());
        });
        *build_slot.borrow_mut() = Some((area.clone(), subscription));
        cx.new(|_| Root {
            area,
            wheel: build_wheel,
            with_document,
        })
    })
    .unwrap();
    let (area, subscription) = slot.borrow_mut().take().unwrap();
    let mut fixture = Fixture {
        host,
        area,
        events,
        wheel,
        _subscription: subscription,
    };
    fixture.host.flush();
    fixture
}

vespertine::actions!(test_area_app, [GlobalShortcut]);

impl Fixture {
    pub fn with<R>(
        &mut self,
        f: impl FnOnce(&mut TextArea, &mut Window, &mut Context<TextArea>) -> R,
    ) -> R {
        let area = self.area.clone();
        let window = self.host.window();
        let result = self
            .host
            .app()
            .update_window(window, |_, window, cx| {
                area.update(cx, |area, cx| f(area, window, cx))
            })
            .unwrap();
        self.host.flush();
        result
    }

    pub fn read<R>(&mut self, f: impl FnOnce(&TextArea) -> R) -> R {
        let area = self.area.clone();
        self.host.app().update(|cx| f(area.read(cx)))
    }

    pub fn focus(&mut self) {
        self.with(|area, window, cx| area.focus(window, cx));
    }

    pub fn text(&mut self) -> String {
        self.read(|area| area.source().to_owned())
    }

    pub fn selection(&mut self) -> Range<usize> {
        self.read(|area| area.selected_range())
    }

    pub fn set_value(&mut self, value: &str) {
        let value = value.to_owned();
        self.with(|area, window, cx| area.set_value(&value, window, cx));
    }

    pub fn select(&mut self, range: Range<usize>) {
        self.with(|area, window, cx| area.set_selected_range(range, false, window, cx));
    }

    pub fn keys(&mut self, keys: &str) {
        self.host.simulate_keystrokes(keys, None).unwrap();
    }

    pub fn type_text(&mut self, text: &str) {
        let text = text.to_owned();
        self.with(|area, window, cx| area.replace_text_in_range(None, &text, window, cx));
    }

    pub fn take_events(&mut self) -> Vec<TextAreaEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    pub fn point_of(&mut self, index: usize) -> Point<vespertine::Pixels> {
        self.read(|area| area.point_for_index(index).unwrap())
    }

    pub fn raw(&mut self, input: PlatformInput) {
        let window = self.host.window();
        self.host.flush();
        self.host
            .app()
            .simulate_platform_input(window, input)
            .unwrap();
        self.host.flush();
    }

    pub fn press_at(&mut self, at: Point<vespertine::Pixels>, count: usize, shift: bool) {
        self.raw(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
            click_count: count,
            first_mouse: false,
        }));
    }

    pub fn drag_at(&mut self, at: Point<vespertine::Pixels>) {
        self.raw(PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        }));
    }

    pub fn release_at(&mut self, at: Point<vespertine::Pixels>) {
        self.raw(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
        }));
    }

    /// A window point inside the row of the caret cell at `index`, `dx` px right of its start.
    pub fn row_point(&mut self, index: usize, dx: f32) -> Point<vespertine::Pixels> {
        let at = self.point_of(index);
        point(at.x + px(dx), at.y + px(10.0))
    }

    pub fn clipboard_text(&mut self) -> Option<String> {
        self.host
            .app()
            .update(|cx| cx.read_from_clipboard())
            .and_then(|item| item.text())
    }
}
