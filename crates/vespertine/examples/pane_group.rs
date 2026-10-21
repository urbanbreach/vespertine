#[path = "example_support/fonts.rs"]
mod example_support;
// This example never resets the editor, so part of the shared module goes unused here.
#[allow(dead_code)]
#[path = "example_support/text_input.rs"]
mod text_input;

use text_input::TextInput;
use vespertine::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, SharedString, Window,
    WindowBounds, WindowOptions, actions, black, div,
    panes::{
        MovingSurface, MovingSurfaceState, PaneDescriptor, PaneEvent, PaneGroupState, pane_group,
    },
    point,
    prelude::*,
    px, rgb, size, yellow,
};
use vespertine_platform::application;

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
        let text_input = cx
            .new(|cx| TextInput::new("", "Type here, then open, resize and collapse panes...", cx));
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
                    .p_2()
                    .gap_2()
                    .flex()
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
        cx.bind_keys(text_input::key_bindings());
        cx.bind_keys([
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
    use std::{ops::Range, time::Duration};
    use text_input::SelectLeft;
    use vespertine::{
        EntityInputHandler, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        Pixels, Point, TestAppContext, VisualTestContext, WindowHandle,
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
            self.vcx.update(|window, cx| {
                // Deliver the frame the panes asked for, as the platform loop would.
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx)
            });
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
            let focus = self.cx.read(|cx| self.input.focus_handle(cx));
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
