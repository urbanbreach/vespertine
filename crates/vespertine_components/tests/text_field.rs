//! CMP-007 acceptance tests: the single-line text field in the offscreen test host.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    path::PathBuf,
    rc::Rc,
};
use vespertine::{
    App, AppContext as _, ClipboardEntry, ClipboardItem, Context, Entity, ExternalPaths,
    InteractiveElement, IntoElement, KeyBinding, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, PlatformInput, Render, Styled, Window, actions,
    div, point, px,
};
use vespertine_components::text::{
    Acknowledgement, FieldTheme, TextField, TextFieldEvent, actions::bind_default_keys,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

actions!(test_app, [GlobalShortcut]);

struct Root {
    field: Entity<TextField>,
    height: Option<f32>,
    shortcut: Rc<Cell<u32>>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shortcut = self.shortcut.clone();
        let mut holder = div().w_full();
        if let Some(height) = self.height {
            holder = holder.h(px(height));
        }
        div()
            .id("root")
            .size_full()
            .on_action(cx.listener(move |_, _: &GlobalShortcut, _, _| {
                shortcut.set(shortcut.get() + 1);
            }))
            .child(holder.child(self.field.clone()))
    }
}

struct Fixture {
    host: TestHost,
    root: Entity<Root>,
    field: Entity<TextField>,
    events: Rc<RefCell<Vec<TextFieldEvent>>>,
    shortcut: Rc<Cell<u32>>,
    _subscription: vespertine::Subscription,
}

fn fixture(width: f64, configure: impl FnOnce(TextField) -> TextField + 'static) -> Fixture {
    let events = Rc::new(RefCell::new(Vec::new()));
    let shortcut = Rc::new(Cell::new(0));
    let slot: Rc<RefCell<Option<(Entity<Root>, Entity<TextField>, vespertine::Subscription)>>> =
        Rc::default();
    let build_slot = slot.clone();
    let build_events = events.clone();
    let build_shortcut = shortcut.clone();
    let host = TestHost::with_view(HostOptions::with_size(width, 80.0), move |window, cx| {
        bind_default_keys(cx);
        cx.bind_keys([KeyBinding::new("ctrl-k", GlobalShortcut, None)]);
        let field = cx.new(|cx| configure(TextField::new(window, cx)));
        let subscription = cx.subscribe(&field, move |_, event: &TextFieldEvent, _| {
            build_events.borrow_mut().push(event.clone());
        });
        let root = cx.new(|_| Root {
            field: field.clone(),
            height: None,
            shortcut: build_shortcut,
        });
        *build_slot.borrow_mut() = Some((root.clone(), field, subscription));
        root
    })
    .unwrap();
    let (root, field, subscription) = slot.borrow_mut().take().unwrap();
    let mut fixture = Fixture {
        host,
        root,
        field,
        events,
        shortcut,
        _subscription: subscription,
    };
    fixture.host.flush();
    fixture
}

impl Fixture {
    fn with<R>(
        &mut self,
        f: impl FnOnce(&mut TextField, &mut Window, &mut Context<TextField>) -> R,
    ) -> R {
        let field = self.field.clone();
        let window = self.host.window();
        let result = self
            .host
            .app()
            .update_window(window, |_, window, cx| {
                field.update(cx, |field, cx| f(field, window, cx))
            })
            .unwrap();
        self.host.flush();
        result
    }

    fn read<R>(&mut self, f: impl FnOnce(&TextField, &App) -> R) -> R {
        let field = self.field.clone();
        self.host.app().update(|cx| f(field.read(cx), cx))
    }

    fn focus(&mut self) {
        self.with(|field, window, cx| field.focus(window, cx));
    }

    fn text(&mut self) -> String {
        self.read(|field, _| field.value_text().to_owned())
    }

    fn selection(&mut self) -> Range<usize> {
        self.read(|field, _| field.selected_range())
    }

    fn keys(&mut self, keys: &str) {
        self.host.simulate_keystrokes(keys, None).unwrap();
    }

    fn type_text(&mut self, text: &str) {
        let text = text.to_owned();
        self.with(|field, window, cx| {
            use vespertine::EntityInputHandler as _;
            field.replace_text_in_range(None, &text, window, cx)
        });
    }

    fn clipboard(&mut self, item: ClipboardItem) {
        self.host.app().update(|cx| cx.write_to_clipboard(item));
    }

    fn take_events(&mut self) -> Vec<TextFieldEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    fn x_of(&mut self, index: usize) -> f64 {
        self.read(|field, _| f64::from(f32::from(field.window_x_for_index(index).unwrap())))
    }

    fn mid_y(&mut self) -> f64 {
        self.read(|field, _| {
            let bounds = field.text_bounds().unwrap();
            f64::from(f32::from(bounds.top() + bounds.size.height / 2.0))
        })
    }

    fn raw(&mut self, input: PlatformInput) {
        let window = self.host.window();
        self.host.flush();
        self.host
            .app()
            .simulate_platform_input(window, input)
            .unwrap();
        self.host.flush();
    }

    fn press(&mut self, x: f64, count: usize, shift: bool) {
        let y = self.mid_y();
        self.raw(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
            click_count: count,
            first_mouse: false,
        }));
    }

    fn drag_to(&mut self, x: f64, y: f64) {
        self.raw(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x as f32), px(y as f32)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        }));
    }

    fn release(&mut self, x: f64) {
        let y = self.mid_y();
        self.raw(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            modifiers: Modifiers::default(),
            click_count: 1,
        }));
    }
}

#[test]
fn a1_submit_cancel_select_all_paste_and_popup_refocus() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field.value("hello"));
    fx.focus();
    fx.take_events();
    fx.keys("enter");
    fx.keys("escape");
    let events = fx.take_events();
    assert!(matches!(&events[0], TextFieldEvent::Submitted(value) if value == "hello"));
    assert!(matches!(events[1], TextFieldEvent::Cancelled));
    fx.keys("ctrl-a");
    assert_eq!(fx.selection(), 0..5);
    fx.clipboard(ClipboardItem::new_string("café ☕ 日本語".into()));
    fx.keys("ctrl-v");
    assert_eq!(fx.text(), "café ☕ 日本語");
    // An anchored popup opens and closes; plain focus restores the exact selection.
    fx.with(|field, window, cx| field.set_selected_range(1..5, true, window, cx));
    let history = fx.read(|field, _| field.core().undo_len());
    for _ in 0..3 {
        fx.host.blur().unwrap();
        fx.focus();
    }
    assert_eq!(fx.text(), "café ☕ 日本語");
    assert_eq!(fx.selection(), 1..5);
    assert!(fx.read(|field, _| field.selection_reversed()));
    assert_eq!(fx.read(|field, _| field.core().undo_len()), history);
}

#[test]
fn a2_line_breaks_are_normalized_for_typing_paste_set_value_and_marked_text() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field);
    fx.focus();
    fx.type_text("a\r\nb\nc\rd");
    assert_eq!(fx.text(), "a b c d");
    fx.with(|field, window, cx| field.set_value("", window, cx));
    fx.clipboard(ClipboardItem::new_string("a\r\nb\nc\rd".into()));
    fx.keys("ctrl-v");
    assert_eq!(fx.text(), "a b c d");
    fx.with(|field, window, cx| field.set_value("a\r\nb\nc\rd", window, cx));
    assert_eq!(fx.text(), "a b c d");
    fx.with(|field, window, cx| {
        use vespertine::EntityInputHandler as _;
        field.replace_and_mark_text_in_range(None, "x\r\ny", None, window, cx)
    });
    assert_eq!(fx.text(), "a b c dx y");
    assert_eq!(fx.read(|field, _| field.marked_range()), Some(7..10));
}

#[test]
fn a3_controlled_value_acknowledgement_replacement_and_noop() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field);
    fx.focus();
    for ch in ["a", "b", "c"] {
        fx.type_text(ch);
    }
    let emitted: Vec<String> = fx
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            TextFieldEvent::Changed(value) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(emitted, ["a", "ab", "abc"]);
    let outcome = fx.with(|field, window, cx| field.acknowledge_or_apply("ab", window, cx));
    assert_eq!(outcome, Acknowledgement::Acknowledged);
    assert_eq!(fx.text(), "abc");
    let outcome = fx.with(|field, window, cx| field.acknowledge_or_apply("zzz", window, cx));
    assert_eq!(outcome, Acknowledgement::Replaced);
    assert_eq!(fx.text(), "zzz");
    assert_eq!(fx.selection(), 3..3);
    assert_eq!(fx.read(|field, _| field.core().undo_len()), 0);
    let outcome = fx.with(|field, window, cx| field.acknowledge_or_apply("zzz", window, cx));
    assert_eq!(outcome, Acknowledgement::Unchanged);
    // Replacements are not echoed back as local edits.
    assert!(fx.take_events().is_empty());
}

#[test]
fn keyboard_bindings_cover_ctrl_and_super_variants() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field.value("foo.bar baz"));
    fx.focus();
    fx.keys("home");
    assert_eq!(fx.selection(), 0..0);
    fx.keys("ctrl-right");
    assert_eq!(fx.selection(), 3..3);
    fx.keys("ctrl-right");
    assert_eq!(fx.selection(), 4..4);
    fx.keys("super-right");
    assert_eq!(fx.selection(), 7..7);
    fx.keys("shift-end");
    assert_eq!(fx.selection(), 7..11);
    fx.keys("ctrl-shift-left");
    assert_eq!(fx.selection(), 7..8);
    fx.keys("end");
    fx.keys("ctrl-backspace");
    assert_eq!(fx.text(), "foo.bar ");
    fx.keys("super-z");
    assert_eq!(fx.text(), "foo.bar baz");
    fx.keys("ctrl-shift-z");
    assert_eq!(fx.text(), "foo.bar ");
    fx.keys("backspace");
    assert_eq!(fx.text(), "foo.bar");
    fx.keys("shift-backspace");
    assert_eq!(fx.text(), "foo.ba");
    fx.keys("home delete");
    assert_eq!(fx.text(), "oo.ba");
}

#[test]
fn a4_backspace_removes_a_whole_cluster() {
    require_backend!();
    let mut fx = fixture(300.0, |field| {
        field.value("e\u{301}👩\u{200d}👩\u{200d}👧x")
    });
    fx.focus();
    fx.keys("left");
    fx.keys("left");
    assert_eq!(
        fx.selection(),
        3..3,
        "left crosses the ZWJ family as one cluster"
    );
    fx.keys("backspace");
    assert_eq!(fx.text(), "👩\u{200d}👩\u{200d}👧x");
    fx.keys("end");
    fx.keys("left");
    fx.keys("backspace");
    assert_eq!(fx.text(), "x");
}

#[test]
fn a5_click_counts_select_caret_extension_word_and_whole_value() {
    require_backend!();
    let mut fx = fixture(400.0, |field| field.value("alpha beta gamma"));
    fx.focus();
    let x2 = fx.x_of(2);
    fx.press(x2, 1, false);
    fx.release(x2);
    assert_eq!(fx.selection(), 2..2);
    let x8 = fx.x_of(8);
    fx.press(x8, 1, true);
    fx.release(x8);
    assert_eq!(fx.selection(), 2..8);
    fx.press(x8, 2, false);
    fx.release(x8);
    assert_eq!(
        fx.selection(),
        6..10,
        "double click selects the word under the pointer"
    );
    fx.press(x8, 3, false);
    fx.release(x8);
    assert_eq!(fx.selection(), 0..16);
    fx.press(x8, 4, false);
    fx.release(x8);
    assert_eq!(fx.selection(), 0..16);

    // A word-initiated drag extends by whole words and keeps the anchor word.
    fx.press(x8, 2, false);
    let x13 = fx.x_of(13);
    let y = fx.mid_y();
    fx.drag_to(x13, y);
    assert_eq!(fx.selection(), 6..16);
    let x1 = fx.x_of(1);
    fx.drag_to(x1, y);
    assert_eq!(fx.selection(), 0..10);
    assert!(fx.read(|field, _| field.selection_reversed()));
    fx.release(x1);
}

#[test]
fn a6_drag_autoscroll_steps_and_follow_the_caret() {
    require_backend!();
    let long = "w".repeat(200);
    let mut fx = fixture(200.0, move |field| field.value(&long));
    fx.focus();
    fx.with(|field, window, cx| field.set_selected_range(0..0, false, window, cx));
    assert_eq!(fx.read(|field, _| field.scroll_offset()), px(0.0));
    let (right, y) = fx.read(|field, _| {
        let bounds = field.text_bounds().unwrap();
        (
            f64::from(f32::from(bounds.right())),
            f64::from(f32::from(bounds.top() + bounds.size.height / 2.0)),
        )
    });
    let start = fx.x_of(10);
    fx.press(start, 1, false);
    for (distance, expected) in [(10.0, 2.0), (100.0, 20.0), (1000.0, 20.0)] {
        fx.drag_to(right + distance, y);
        let before = fx.read(|field, _| f32::from(field.scroll_offset()));
        fx.host.advance_executor_time(16.0).unwrap();
        fx.host.flush();
        let after = fx.read(|field, _| f32::from(field.scroll_offset()));
        assert!(
            (after - before - expected).abs() < 0.01,
            "distance {distance}: {before} -> {after}, expected a step of {expected}"
        );
    }
    let selection = fx.selection();
    assert_eq!(selection.start, 10);
    assert!(
        selection.end > 10,
        "selection follows the pointer during autoscroll"
    );
    fx.release(right + 1000.0);
    let stopped = fx.read(|field, _| field.scroll_offset());
    fx.host.advance_executor_time(160.0).unwrap();
    assert_eq!(
        fx.read(|field, _| field.scroll_offset()),
        stopped,
        "release stops autoscroll"
    );

    // The caret stays inside [offset, offset + width - 2] (R12).
    fx.keys("end");
    let (offset, width, caret) = fx.read(|field, _| {
        let bounds = field.text_bounds().unwrap();
        (
            f32::from(field.scroll_offset()),
            f32::from(bounds.size.width),
            f32::from(
                field.window_x_for_index(field.cursor_offset()).unwrap() - bounds.left()
                    + field.scroll_offset(),
            ),
        )
    });
    assert!(
        caret >= offset && caret <= offset + width - 2.0 + 0.01,
        "{caret} {offset} {width}"
    );
}

#[test]
fn a7_caret_geometry_and_blink_cycle() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field.value("hi"));
    fx.focus();
    fx.host.motion_pause();
    fx.host.motion_set(10_000.0).unwrap();
    fx.keys("left");
    let rect = fx.read(|field, _| field.caret_rect().unwrap());
    assert_eq!(f32::from(rect.size.width), 2.0);
    assert_eq!(f32::from(rect.size.height), 20.0);
    let visible_at = |fx: &mut Fixture, at: f64| {
        fx.host.motion_set(at).unwrap();
        let window = fx.host.window();
        let field = fx.field.clone();
        fx.host
            .app()
            .update_window(window, |_, window, cx| {
                field.read(cx).caret_visible(window, cx)
            })
            .unwrap()
    };
    assert!(visible_at(&mut fx, 10_000.0));
    assert!(visible_at(&mut fx, 10_499.0));
    assert!(!visible_at(&mut fx, 10_500.0));
    assert!(!visible_at(&mut fx, 10_999.0));
    assert!(visible_at(&mut fx, 11_000.0));
    // Any edit restarts the cycle.
    fx.host.motion_set(11_700.0).unwrap();
    fx.type_text("x");
    assert!(visible_at(&mut fx, 11_700.0));
    assert!(visible_at(&mut fx, 12_199.0));
    assert!(!visible_at(&mut fx, 12_200.0));
    // Hidden when unfocused.
    fx.host.blur().unwrap();
    assert!(!visible_at(&mut fx, 11_700.0));
}

#[test]
fn a8_read_only_keeps_selection_and_copy_and_rejects_mutation() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field.value("hello").read_only(true));
    fx.focus();
    fx.take_events();
    fx.keys("ctrl-a");
    assert_eq!(fx.selection(), 0..5);
    fx.keys("ctrl-c");
    assert_eq!(
        fx.host
            .app()
            .update(|cx| cx.read_from_clipboard())
            .and_then(|item| item.text()),
        Some("hello".into())
    );
    fx.clipboard(ClipboardItem::new_string("zzz".into()));
    for keys in [
        "ctrl-x",
        "ctrl-v",
        "backspace",
        "delete",
        "ctrl-z",
        "ctrl-backspace",
    ] {
        fx.keys(keys);
    }
    fx.type_text("q");
    fx.with(|field, window, cx| {
        use vespertine::EntityInputHandler as _;
        field.replace_and_mark_text_in_range(None, "に", None, window, cx)
    });
    assert_eq!(fx.text(), "hello");
    assert_eq!(fx.read(|field, _| field.core().undo_len()), 0);
    assert!(fx.take_events().is_empty(), "no events for rejected edits");
    // Cut leaves the clipboard alone.
    assert_eq!(
        fx.host
            .app()
            .update(|cx| cx.read_from_clipboard())
            .and_then(|item| item.text()),
        Some("zzz".into())
    );
}

#[test]
fn a8_disabled_is_not_in_the_tab_order_and_ignores_input() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field.value("x").disabled(true));
    fx.host.focus_next().unwrap();
    let focused = fx.read(|field, _| field.is_disabled());
    assert!(focused);
    let is_focused = {
        let window = fx.host.window();
        let field = fx.field.clone();
        fx.host
            .app()
            .update_window(window, |_, window, cx| {
                vespertine::Focusable::focus_handle(field.read(cx), cx).is_focused(window)
            })
            .unwrap()
    };
    assert!(!is_focused, "Tab never lands on a disabled field");
    let x = fx.x_of(0);
    fx.press(x, 1, false);
    assert_eq!(fx.selection(), 1..1, "pointer presses are ignored");
    let caret = fx.read(|field, _| field.core().selection());
    assert_eq!(caret, 1..1);
}

#[test]
fn a9_edit_menu_enablement_and_pasted_paths() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field.value("hello"));
    fx.focus();
    let paths_item = ClipboardItem {
        entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
            vec![PathBuf::from("/tmp/a.png")].into(),
        ))],
    };
    // (selection, clipboard) matrix against R24.
    fx.clipboard(ClipboardItem::new_string("t".into()));
    fx.with(|field, window, cx| field.set_selected_range(1..1, false, window, cx));
    let state = fx.read(|field, cx| field.edit_menu_state(cx));
    assert!(!state.cut && !state.copy && state.paste && state.select_all);
    fx.with(|field, window, cx| field.set_selected_range(1..3, false, window, cx));
    let state = fx.read(|field, cx| field.edit_menu_state(cx));
    assert!(state.cut && state.copy && state.paste && state.select_all);
    fx.clipboard(paths_item);
    let state = fx.read(|field, cx| field.edit_menu_state(cx));
    assert!(!state.paste, "paths are not insertable text");
    fx.with(|field, _, cx| field.set_read_only(true, cx));
    let state = fx.read(|field, cx| field.edit_menu_state(cx));
    assert!(!state.cut && state.copy && !state.paste);
    fx.with(|field, _, cx| field.set_read_only(false, cx));
    fx.with(|field, window, cx| field.set_value("", window, cx));
    let state = fx.read(|field, cx| field.edit_menu_state(cx));
    assert!(!state.select_all && !state.copy && !state.cut);

    // Pasting external paths inserts no text and reports them.
    fx.with(|field, window, cx| field.set_value("abc", window, cx));
    fx.take_events();
    fx.keys("ctrl-v");
    assert_eq!(fx.text(), "abc");
    let events = fx.take_events();
    assert!(
        matches!(&events[..], [TextFieldEvent::PastedPaths(paths)] if paths == &[PathBuf::from("/tmp/a.png")])
    );
}

#[test]
fn a10_native_input_utf16_mapping_and_bounds_follow_scroll() {
    require_backend!();
    use vespertine::EntityInputHandler as _;
    let mut fx = fixture(300.0, |field| field.value("a🌍b"));
    fx.focus();
    let (text, adjusted) = fx.with(|field, window, cx| {
        let mut adjusted = None;
        let text = field.text_for_range(2..3, &mut adjusted, window, cx);
        (text, adjusted)
    });
    assert_eq!(text.as_deref(), Some("🌍"));
    assert_eq!(adjusted, Some(1..3));
    let selection =
        fx.with(|field, window, cx| field.selected_text_range(false, window, cx).unwrap());
    assert_eq!(selection.range, 4..4);
    fx.with(|field, window, cx| {
        field.replace_and_mark_text_in_range(Some(1..3), "に日本", Some(1..2), window, cx)
    });
    assert_eq!(fx.text(), "aに日本b");
    assert_eq!(fx.read(|field, _| field.marked_range()), Some(1..10));
    assert_eq!(fx.selection(), 4..7);
    let marked = fx.with(|field, window, cx| field.marked_text_range(window, cx));
    assert_eq!(marked, Some(1..4));
    let bounds = fx.with(|field, window, cx| {
        let element = field.text_bounds().unwrap();
        field.bounds_for_range(1..1, element, window, cx).unwrap()
    });
    assert_eq!(f32::from(bounds.size.width), 2.0);
    assert_eq!(f32::from(bounds.size.height), 20.0);
    let index = fx.with(|field, window, cx| {
        let x = field.window_x_for_index(1).unwrap();
        let y = field.text_bounds().unwrap().top();
        field.character_index_for_point(point(x, y), window, cx)
    });
    assert_eq!(index, Some(1));
}

#[test]
fn a11_composition_is_one_undo_unit_and_redo_restores_the_commit() {
    require_backend!();
    use vespertine::EntityInputHandler as _;
    let mut fx = fixture(300.0, |field| field.value("abc"));
    fx.focus();
    fx.with(|field, window, cx| field.set_selected_range(1..2, false, window, cx));
    for marked in ["に", "にほ", "にほん", "にほんご"] {
        let marked = marked.to_owned();
        fx.with(|field, window, cx| {
            field.replace_and_mark_text_in_range(None, &marked, None, window, cx)
        });
    }
    fx.with(|field, window, cx| field.replace_text_in_range(None, "日本語", window, cx));
    assert_eq!(fx.text(), "a日本語c");
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), "abc");
    assert_eq!(fx.selection(), 1..2);
    fx.keys("ctrl-shift-z");
    assert_eq!(fx.text(), "a日本語c");
}

#[test]
fn a12_history_cap_and_paste_cut_steps_through_the_field() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field);
    fx.focus();
    fx.clipboard(ClipboardItem::new_string("x".into()));
    for _ in 0..210 {
        fx.keys("ctrl-v");
    }
    assert_eq!(fx.read(|field, _| field.core().undo_len()), 200);
    fx.keys("ctrl-a ctrl-x");
    assert_eq!(fx.text(), "");
    fx.keys("ctrl-z");
    assert_eq!(fx.text().len(), 210);
}

#[test]
fn a13_enter_and_escape_during_composition_emit_nothing_and_unbound_keys_propagate() {
    require_backend!();
    use vespertine::EntityInputHandler as _;
    let mut fx = fixture(300.0, |field| field);
    fx.focus();
    fx.with(|field, window, cx| field.replace_and_mark_text_in_range(None, "に", None, window, cx));
    fx.take_events();
    fx.keys("enter");
    fx.keys("escape");
    assert!(!fx.take_events().iter().any(|event| matches!(
        event,
        TextFieldEvent::Submitted(_) | TextFieldEvent::Cancelled
    )));
    fx.with(|field, window, cx| field.replace_text_in_range(None, "日", window, cx));
    fx.keys("ctrl-k");
    assert_eq!(
        fx.shortcut.get(),
        1,
        "an unbound key reaches the application handler"
    );
    fx.keys("tab");
    assert_eq!(fx.text(), "日", "Tab is not consumed or inserted");
}

#[test]
fn a14_layout_is_reused_across_caret_selection_and_height_animation() {
    require_backend!();
    let mut fx = fixture(300.0, |field| field.value("hello world"));
    fx.focus();
    fx.host.flush();
    let baseline = fx.read(|field, _| field.shape_count());
    assert!(baseline >= 1);
    fx.host.motion_pause();
    for at in [100.0, 600.0, 1100.0] {
        fx.host.motion_set(at).unwrap();
        fx.host.flush();
    }
    fx.keys("left shift-left home");
    fx.keys("ctrl-a");
    assert_eq!(
        fx.read(|field, _| field.shape_count()),
        baseline,
        "caret and selection never reshape"
    );
    for height in [30.0, 40.0, 50.0, 60.0] {
        let root = fx.root.clone();
        fx.host.app().update(|cx| {
            root.update(cx, |root, cx| {
                root.height = Some(height);
                cx.notify();
            })
        });
        fx.host.flush();
    }
    assert_eq!(
        fx.read(|field, _| field.shape_count()),
        baseline,
        "height animation never reshapes"
    );
    fx.type_text("!");
    assert_eq!(
        fx.read(|field, _| field.shape_count()),
        baseline + 1,
        "a value change reshapes once"
    );
    fx.host.flush();
    assert_eq!(fx.read(|field, _| field.shape_count()), baseline + 1);
}

#[test]
fn vertical_centering_theme_roles_and_accessibility() {
    require_backend!();
    let theme = FieldTheme::default();
    let mut fx = fixture(300.0, move |field| field.placeholder("Search").theme(theme));
    let root = fx.root.clone();
    fx.host.app().update(|cx| {
        root.update(cx, |root, cx| {
            root.height = Some(60.0);
            cx.notify();
        })
    });
    fx.host.flush();
    let (top, text_bounds) = fx.read(|field, _| {
        let bounds = field.text_bounds().unwrap();
        (field.caret_rect().unwrap().top(), bounds)
    });
    let expected = text_bounds.top() + (text_bounds.size.height - px(20.0)) / 2.0;
    assert!(
        (f32::from(top - expected)).abs() < 0.01,
        "text is centered in the surplus height"
    );
    assert!(fx.host.painted_text().contains(&"Search".to_owned()));
    let tree = fx.host.accessibility_tree().unwrap().unwrap().to_string();
    assert!(
        tree.contains("TextInput") || tree.contains("textInput"),
        "{tree}"
    );
}

#[test]
fn r25_a_press_on_an_embedded_control_leaves_caret_and_selection_alone() {
    require_backend!();
    let mut fx = fixture(300.0, |field| {
        field
            .value("hello world")
            .trailing(|_, _| div().id("clear").w(px(20.0)).h(px(20.0)).into_any_element())
    });
    fx.focus();
    fx.with(|field, window, cx| field.set_selected_range(1..3, false, window, cx));
    let y = fx.mid_y();
    let x = 300.0 - 9.0 - 10.0;
    fx.raw(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: point(px(x as f32), px(y as f32)),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    }));
    assert_eq!(fx.selection(), 1..3);
    fx.drag_to(x - 100.0, y);
    assert_eq!(fx.selection(), 1..3, "no text drag starts on the control");
    fx.release(x);
}
