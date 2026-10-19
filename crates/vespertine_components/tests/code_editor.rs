//! CMP-014 editor half: the editable code surface in the offscreen test host.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use vespertine::{
    App, AppContext as _, ClipboardItem, Context, Entity, EntityInputHandler as _,
    InteractiveElement, IntoElement, Modifiers, MouseButton, MouseDownEvent, ParentElement,
    PlatformInput, Point, Render, Styled, WeakEntity, Window, div, point, px, rgb,
};
use vespertine_components::code::{
    CodeEditor, CodeEditorEvent, CodeHighlighter, ContextMenuRequest, EditorStyle, GutterLine,
    LineClick, SpanHighlighter, StyledRun, TextEdit,
};
use vespertine_richtext::syntax::TokenKind;
use vespertine_richtext::text_root;
use vespertine_test_host::{HostOptions, TestHost, require_backend};

/// The window root: an owner that holds the editor and the context-menu requests it received.
struct Owner {
    editor: Option<Entity<CodeEditor>>,
    menus: Vec<ContextMenuRequest>,
}

impl Render for Owner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.editor {
            Some(editor) => div().w(px(420.)).child(editor.clone()).into_any_element(),
            None => div().into_any_element(),
        };
        text_root(
            div()
                .id("owner")
                .size_full()
                .bg(rgb(0x181818))
                .text_color(rgb(0xeeeeee))
                .p(px(10.))
                .child(body),
        )
    }
}

struct Fixture {
    host: TestHost,
    owner: Entity<Owner>,
    editor: WeakEntity<CodeEditor>,
    events: Rc<RefCell<Vec<CodeEditorEvent>>>,
}

fn fixture(
    value: &str,
    configure: impl FnOnce(&mut CodeEditor, &mut Context<CodeEditor>) + 'static,
) -> Fixture {
    let events: Rc<RefCell<Vec<CodeEditorEvent>>> = Rc::default();
    let slot: Rc<RefCell<Option<(Entity<Owner>, WeakEntity<CodeEditor>)>>> = Rc::default();
    let (build_events, build_slot, value, configure) = (
        events.clone(),
        slot.clone(),
        value.to_owned(),
        Rc::new(RefCell::new(Some(configure))),
    );
    let host = TestHost::with_view(HostOptions::with_size(480., 320.), move |window, cx| {
        let editor = cx.new(|cx| CodeEditor::new(window, cx));
        let sink = build_events.clone();
        let subscription = cx.subscribe(&editor, move |_, event: &CodeEditorEvent, _| {
            sink.borrow_mut().push(event.clone());
        });
        std::mem::forget(subscription);
        editor.update(cx, |editor, cx| {
            editor.set_value(&value, window, cx);
            if let Some(configure) = configure.borrow_mut().take() {
                configure(editor, cx);
            }
        });
        let owner = cx.new(|_| Owner {
            editor: Some(editor.clone()),
            menus: Vec::new(),
        });
        *build_slot.borrow_mut() = Some((owner.clone(), editor.downgrade()));
        owner
    })
    .unwrap();
    let (owner, editor) = slot.borrow_mut().take().unwrap();
    let mut fixture = Fixture {
        host,
        owner,
        editor,
        events,
    };
    fixture.host.paint_frame();
    fixture.events.borrow_mut().clear();
    fixture
}

impl Fixture {
    fn entity(&self) -> Entity<CodeEditor> {
        self.editor.upgrade().expect("the editor is alive")
    }

    fn with<R>(
        &mut self,
        f: impl FnOnce(&mut CodeEditor, &mut Window, &mut Context<CodeEditor>) -> R,
    ) -> R {
        let editor = self.entity();
        let window = self.host.window();
        let result = self
            .host
            .app()
            .update_window(window, |_, window, cx| {
                editor.update(cx, |editor, cx| f(editor, window, cx))
            })
            .unwrap();
        self.host.paint_frame();
        result
    }

    fn read<R>(&mut self, f: impl FnOnce(&CodeEditor, &App) -> R) -> R {
        let editor = self.entity();
        self.host.app().update(|cx| f(editor.read(cx), cx))
    }

    fn text(&mut self) -> String {
        self.read(|editor, cx| editor.value(cx))
    }

    fn selection(&mut self) -> Range<usize> {
        self.read(|editor, cx| editor.selected_range(cx))
    }

    fn focus(&mut self) {
        self.with(|editor, window, cx| editor.focus(window, cx));
    }

    fn keys(&mut self, keys: &str) {
        self.host.simulate_keystrokes(keys, None).unwrap();
    }

    fn type_text(&mut self, text: &str) {
        let area = self.read(|editor, _| editor.area().clone());
        let window = self.host.window();
        let text = text.to_owned();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                area.update(cx, |area, cx| {
                    area.replace_text_in_range(None, &text, window, cx)
                })
            })
            .unwrap();
        self.host.paint_frame();
    }

    fn clipboard(&mut self) -> Option<String> {
        self.host
            .app()
            .update(|cx| cx.read_from_clipboard())
            .and_then(|item| item.text())
    }

    fn set_clipboard(&mut self, text: &str) {
        let text = text.to_owned();
        self.host
            .app()
            .update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
    }

    fn take_events(&mut self) -> Vec<CodeEditorEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    fn changes(&mut self) -> usize {
        self.take_events()
            .iter()
            .filter(|event| matches!(event, CodeEditorEvent::Changed(_)))
            .count()
    }

    fn raw(&mut self, input: PlatformInput) {
        let window = self.host.window();
        self.host.flush();
        self.host
            .app()
            .simulate_platform_input(window, input)
            .unwrap();
        self.host.paint_frame();
    }

    fn click(&mut self, at: Point<vespertine::Pixels>, button: MouseButton, count: usize) {
        self.raw(PlatformInput::MouseDown(MouseDownEvent {
            button,
            position: at,
            modifiers: Modifiers::default(),
            click_count: count,
            first_mouse: false,
        }));
        self.raw(PlatformInput::MouseUp(vespertine::MouseUpEvent {
            button,
            position: at,
            modifiers: Modifiers::default(),
            click_count: count,
        }));
    }

    /// The window point at `dx` px right of the text start on zero-based line `line`.
    fn on_line(&mut self, line: usize, dx: f32) -> Point<vespertine::Pixels> {
        let bounds = self.read(|editor, cx| editor.line_bounds(cx));
        let line = bounds
            .iter()
            .find(|l| l.index == line)
            .expect("line")
            .clone();
        point(
            line.bounds.left() + px(dx),
            line.bounds.top() + line.bounds.size.height / 2.,
        )
    }
}

const CODE: &str = "fn main() {\n    let answer = 42;\n    println!(\"{answer}\");\n}\n";

#[test]
fn r10_the_editor_edits_selects_and_undoes_like_the_multiline_surface() {
    require_backend!();
    let mut fx = fixture("hello world", |_, _| {});
    fx.focus();
    fx.keys("ctrl-a");
    assert_eq!(fx.selection(), 0..11);
    fx.type_text("fn");
    assert_eq!(fx.text(), "fn");
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), "hello world");
    fx.keys("ctrl-shift-z");
    assert_eq!(fx.text(), "fn");
    // Copy, cut and paste go through the system clipboard.
    fx.keys("ctrl-a ctrl-c");
    assert_eq!(fx.clipboard().as_deref(), Some("fn"));
    fx.keys("ctrl-x");
    assert_eq!(fx.text(), "");
    fx.keys("ctrl-v ctrl-v");
    assert_eq!(fx.text(), "fnfn");
    let changes = fx.changes();
    assert!(changes >= 4, "every edit emits a change event: {changes}");
}

#[test]
fn r10_double_click_selects_a_word_and_triple_click_a_line() {
    require_backend!();
    let mut fx = fixture(CODE, |_, _| {});
    fx.focus();
    let at = fx.on_line(1, 72.);
    fx.click(at, MouseButton::Left, 2);
    let word = fx.selection();
    assert_eq!(&CODE[word], "answer");
    fx.click(at, MouseButton::Left, 3);
    let line = fx.selection();
    assert_eq!(
        &CODE[line], "    let answer = 42;\n",
        "a line selection includes its terminator"
    );
}

#[test]
fn a5_read_only_suppresses_every_edit_but_not_selection_copy_and_select_all() {
    require_backend!();
    let mut fx = fixture("alpha beta", |editor, cx| editor.set_read_only(true, cx));
    fx.focus();
    fx.set_clipboard("pasted");
    fx.type_text("x");
    fx.keys("ctrl-v ctrl-x backspace delete ctrl-z enter tab");
    assert_eq!(fx.text(), "alpha beta");
    assert_eq!(fx.changes(), 0);
    let replaced = fx.with(|editor, window, cx| editor.replace(0..5, "ALPHA", window, cx));
    assert!(!replaced, "the programmatic user-edit path is suppressed");
    assert!(!fx.with(|editor, window, cx| editor.undo(window, cx)));
    assert!(!fx.with(|editor, window, cx| editor.redo(window, cx)));
    // Select all and copy work.
    fx.keys("ctrl-a");
    assert_eq!(fx.selection(), 0..10);
    fx.keys("ctrl-c");
    assert_eq!(fx.clipboard().as_deref(), Some("alpha beta"));
    // Selection by keyboard works too.
    fx.keys("home shift-right shift-right");
    assert_eq!(fx.selection(), 0..2);
    // The owner's baseline replacement is not a user edit and works while read-only.
    fx.with(|editor, window, cx| editor.replace_contents("gamma", window, cx));
    assert_eq!(fx.text(), "gamma");
}

#[test]
fn a5_the_context_menu_matrix_follows_editability_selection_and_clipboard() {
    require_backend!();
    let mut fx = fixture("alpha beta", |_, _| {});
    fx.focus();
    fx.set_clipboard("text");
    // Editable with a selection: cut, copy, paste; select all always.
    fx.keys("ctrl-a");
    let menu = fx.read(|editor, cx| editor.menu_state(cx));
    assert!(menu.cut && menu.copy && menu.paste && menu.select_all);
    // Editable without a selection: only paste (and select all).
    fx.with(|editor, window, cx| editor.set_selected_range(3..3, false, window, cx));
    let menu = fx.read(|editor, cx| editor.menu_state(cx));
    assert!(!menu.cut && !menu.copy && menu.paste && menu.select_all);
    // Without clipboard text nothing pastes.
    // Read-only with a selection: only copy (and select all).
    fx.with(|editor, window, cx| {
        editor.set_read_only(true, cx);
        editor.set_selected_range(0..5, false, window, cx);
    });
    let menu = fx.read(|editor, cx| editor.menu_state(cx));
    assert!(!menu.cut && menu.copy && !menu.paste && menu.select_all);
    // A secondary press raises the request with the capabilities and the pointer position.
    let owner = fx.owner.clone();
    fx.with(|editor, _, _| {
        editor.on_context_menu(&owner, |owner, request, _, _| {
            owner.menus.push(request.clone());
        });
    });
    let at = fx.on_line(0, 20.);
    fx.click(at, MouseButton::Right, 1);
    let owner = fx.owner.clone();
    let requests = fx.host.app().update(|cx| owner.read(cx).menus.clone());
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].position, at);
    assert!(!requests[0].editable && requests[0].has_selection);
    assert!(fx.take_events().iter().any(
        |event| matches!(event, CodeEditorEvent::ContextMenu(request) if request.position == at)
    ));
}

#[test]
fn a6_replacing_content_keeps_selection_and_scroll_and_is_not_a_user_edit() {
    require_backend!();
    let lines: String = (0..60).map(|n| format!("line {n}\n")).collect();
    let mut fx = fixture(&lines, |editor, cx| editor.set_rows(1, 5, cx));
    fx.focus();
    fx.with(|editor, window, cx| {
        editor.set_selected_range(10..14, true, window, cx);
        editor.set_scroll_offset(point(px(0.), px(36.)), cx);
    });
    let (selection, scroll) = (
        fx.selection(),
        fx.read(|editor, cx| editor.scroll_offset(cx)),
    );
    assert_eq!(selection, 10..14);
    assert_eq!(scroll.y, px(36.));
    let replaced: String = (0..60).map(|n| format!("LINE {n}\n")).collect();
    fx.take_events();
    fx.with(|editor, window, cx| editor.replace_contents(&replaced, window, cx));
    assert_eq!(fx.text(), replaced);
    assert_eq!(fx.selection(), selection);
    assert_eq!(fx.read(|editor, cx| editor.scroll_offset(cx)), scroll);
    let events = fx.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CodeEditorEvent::Changed(_))),
        "a baseline is never a user edit: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CodeEditorEvent::BaselineReplaced(text) if *text == replaced))
    );
    // It is not in the undo history either.
    assert!(!fx.with(|editor, window, cx| editor.undo(window, cx)));
    assert_eq!(fx.text(), replaced);
    // set_value is a baseline too, and starts the selection over.
    fx.with(|editor, window, cx| editor.set_value("fresh", window, cx));
    assert_eq!(
        fx.take_events()
            .iter()
            .filter(|e| matches!(e, CodeEditorEvent::Changed(_)))
            .count(),
        0
    );
    assert_eq!(fx.selection(), 5..5);
}

#[test]
fn a6_toggling_a_task_marker_edits_only_the_marker() {
    require_backend!();
    let source = "- [ ] first\n- [x] second\n- [X] third\n- [?] odd\n";
    let mut fx = fixture(source, |_, _| {});
    fx.focus();
    fx.with(|editor, window, cx| editor.set_selected_range(14..18, false, window, cx));
    fx.with(|editor, _, cx| editor.set_scroll_offset(point(px(0.), px(0.)), cx));
    fx.take_events();
    // Unchecked -> checked: only the inner character changes.
    let edited = fx.with(|editor, window, cx| editor.toggle_task(source, 2..5, false, window, cx));
    assert!(edited);
    let toggled = source.replacen("[ ]", "[x]", 1);
    assert_eq!(fx.text(), toggled);
    assert_eq!(fx.selection(), 14..18, "the selection is restored");
    assert_eq!(fx.changes(), 1, "one ordinary change event");
    // The edit is an ordinary, undoable step.
    fx.keys("ctrl-z");
    assert_eq!(fx.text(), source);
    // Checked -> unchecked, for both `x` and `X`.
    assert!(fx.with(|editor, window, cx| editor.toggle_task(source, 14..17, true, window, cx)));
    assert_eq!(fx.text(), source.replacen("[x] second", "[ ] second", 1));
    fx.with(|editor, window, cx| editor.replace_contents(source, window, cx));
    assert!(fx.with(|editor, window, cx| editor.toggle_task(source, 27..30, true, window, cx)));
    assert_eq!(fx.text(), source.replacen("[X] third", "[ ] third", 1));
    fx.with(|editor, window, cx| editor.replace_contents(source, window, cx));
    // A marker that does not match what was reported, or an unknown marker, is refused.
    assert!(!fx.with(|editor, window, cx| editor.toggle_task(source, 2..5, true, window, cx)));
    assert!(!fx.with(|editor, window, cx| editor.toggle_task(source, 14..17, false, window, cx)));
    assert!(!fx.with(|editor, window, cx| editor.toggle_task(source, 40..43, false, window, cx)));
    assert!(!fx.with(|editor, window, cx| editor.toggle_task(source, 40..43, true, window, cx)));
    // A stale preview never applies offsets to a different revision.
    fx.type_text("// ");
    let stale = fx.text();
    assert!(!fx.with(|editor, window, cx| editor.toggle_task(source, 2..5, false, window, cx)));
    assert_eq!(fx.text(), stale);
    // Read-only editors refuse too.
    fx.with(|editor, window, cx| editor.replace_contents(source, window, cx));
    fx.with(|editor, _, cx| editor.set_read_only(true, cx));
    assert!(!fx.with(|editor, window, cx| editor.toggle_task(source, 2..5, false, window, cx)));
    assert_eq!(fx.text(), source);
}

/// A highlighter that shares its spans with the test.
struct Shared(Rc<RefCell<SpanHighlighter>>);

impl CodeHighlighter for Shared {
    fn update(&mut self, edit: &TextEdit) {
        self.0.borrow_mut().update(edit);
    }

    fn styles(
        &self,
        range: Range<usize>,
        resolver: &dyn Fn(TokenKind) -> vespertine::Hsla,
        default: vespertine::Hsla,
    ) -> Vec<StyledRun> {
        self.0.borrow().styles(range, resolver, default)
    }
}

#[test]
fn a7_spans_follow_edits_and_a_highlighter_for_other_text_is_not_installed() {
    require_backend!();
    let source = "let x = 1;\nlet y = 2;\n";
    let mut fx = fixture(source, |_, _| {});
    fx.focus();
    let spans = Rc::new(RefCell::new(SpanHighlighter::new(
        source,
        [
            (0..3, TokenKind::Keyword),
            (8..9, TokenKind::Number),
            (11..14, TokenKind::Keyword),
            (19..20, TokenKind::Number),
        ],
    )));
    // Computed for different text: ignored.
    let rejected = fx.with(|editor, _, cx| {
        editor.install_highlighter("let x = 1;\n", Shared(spans.clone()), cx)
    });
    assert!(!rejected && !fx.read(|editor, _| editor.has_highlighter()));
    let installed =
        fx.with(|editor, _, cx| editor.install_highlighter(source, Shared(spans.clone()), cx));
    assert!(installed && fx.read(|editor, _| editor.has_highlighter()));
    let ranges = |spans: &Rc<RefCell<SpanHighlighter>>| -> Vec<Range<usize>> {
        spans
            .borrow()
            .spans()
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    };
    // Events arrive after the update that made the edit: installing in the same update as the
    // replacement must not apply that replacement to the spans a second time.
    let fresh = Rc::new(RefCell::new(SpanHighlighter::new(
        "let z = 3;\n",
        [(0..3, TokenKind::Keyword), (8..9, TokenKind::Number)],
    )));
    let installed = fx.with(|editor, window, cx| {
        editor.set_value("let z = 3;\n", window, cx);
        editor.install_highlighter("let z = 3;\n", Shared(fresh.clone()), cx)
    });
    assert!(installed);
    assert_eq!(ranges(&fresh), [0..3, 8..9]);
    fx.with(|editor, window, cx| editor.set_value(source, window, cx));
    let installed =
        fx.with(|editor, _, cx| editor.install_highlighter(source, Shared(spans.clone()), cx));
    assert!(installed);
    // An edit in the middle of line one: spans before are unchanged, after are shifted by the
    // delta and the span under the edit is removed.
    fx.with(|editor, window, cx| {
        editor.set_selected_range(8..9, false, window, cx);
    });
    fx.type_text("100");
    assert_eq!(fx.text(), "let x = 100;\nlet y = 2;\n");
    assert_eq!(ranges(&spans), [0..3, 13..16, 21..22]);
    // Positions after the edit moved by +2 (3 inserted, 1 removed).
    fx.with(|editor, window, cx| editor.set_selected_range(0..0, false, window, cx));
    fx.type_text("// ");
    assert_eq!(ranges(&spans), [3..6, 16..19, 24..25]);
}

#[test]
fn a4_syntax_colors_never_move_glyphs() {
    require_backend!();
    let source = "let x = 1;\nlet y = \"text\";\n";
    let mut fx = fixture(source, |_, _| {});
    let positions = |fx: &mut Fixture| -> Vec<Point<vespertine::Pixels>> {
        fx.read(|editor, cx| {
            let area = editor.area().read(cx);
            (0..=source.len())
                .filter(|i| source.is_char_boundary(*i))
                .map(|i| area.point_for_index(i).unwrap())
                .collect()
        })
    };
    let before = positions(&mut fx);
    let installed = fx.with(|editor, _, cx| {
        editor.install_highlighter(
            source,
            SpanHighlighter::new(
                source,
                [(0..3, TokenKind::Keyword), (8..9, TokenKind::Number)],
            ),
            cx,
        )
    });
    assert!(installed);
    assert_eq!(before, positions(&mut fx), "installing colors");
    // A theme change recolors the same text with the same advances.
    fx.with(|editor, _, cx| {
        let mut theme = editor.theme().clone();
        theme.syntax.keyword = rgb(0xff0000);
        editor.set_theme(theme, cx);
    });
    assert_eq!(before, positions(&mut fx), "changing theme colors");
    // An unknown language shows plain text: no spans are installed.
    fx.with(|editor, _, cx| {
        editor.clear_highlighter(cx);
        editor.set_language(Some("no-such-language".into()));
        assert!(!editor.highlight_from_language(cx));
        assert!(!editor.has_highlighter());
    });
    assert_eq!(before, positions(&mut fx));
    fx.with(|editor, _, cx| {
        editor.set_language(Some("rust".into()));
        assert!(editor.highlight_from_language(cx));
    });
    assert_eq!(before, positions(&mut fx), "highlighting by language tag");
}

#[test]
fn r13_the_style_tokens_derive_from_the_theme_and_can_be_overridden() {
    require_backend!();
    let mut fx = fixture("x", |_, _| {});
    let style = fx.read(|editor, _| editor.resolved_style());
    let theme = vespertine_theme::document::DocumentTheme::dark();
    let hsla = |c: vespertine::Rgba| vespertine::Hsla::from(c);
    assert_eq!(style.foreground.a, 0.93);
    assert_eq!(style.foreground.l, hsla(theme.text).l);
    assert_eq!(style.muted_foreground, hsla(theme.text_faint));
    assert_eq!(style.background.a, 0.0);
    assert_eq!(style.border, hsla(theme.border));
    assert_eq!(style.selection.a, 0.22);
    assert_eq!(style.selection.h, hsla(theme.accent).h);
    assert_eq!(style.caret, hsla(theme.caret));
    assert_eq!(style.active_line.a, 0.025);
    assert_eq!(style.gutter_background.a, 0.0);
    let caret = vespertine::Hsla {
        h: 0.3,
        s: 1.,
        l: 0.5,
        a: 1.,
    };
    fx.with(|editor, _, cx| {
        editor.set_style(
            EditorStyle {
                caret: Some(caret),
                ..EditorStyle::default()
            },
            cx,
        )
    });
    assert_eq!(fx.read(|editor, _| editor.resolved_style().caret), caret);
}

#[test]
fn r10_soft_wrap_is_off_by_default_and_long_lines_scroll_horizontally() {
    require_backend!();
    let long = "x".repeat(200);
    let source = format!("{long}\nshort\n");
    let mut fx = fixture(&source, |_, _| {});
    fx.focus();
    let rows = fx.read(|editor, cx| editor.area().read(cx).visual_rows().len());
    assert_eq!(
        rows, 3,
        "one row per logical line without soft wrap: {rows}"
    );
    assert!(!fx.read(|editor, _| editor.soft_wrap()));
    // The caret at the end of the long line scrolls it into view.
    fx.with(|editor, window, cx| editor.set_selected_range(200..200, false, window, cx));
    let scrolled = fx.read(|editor, cx| editor.scroll_offset(cx));
    assert!(scrolled.x > px(100.), "{scrolled:?}");
    let bounds = fx.read(|editor, cx| editor.area().read(cx).viewport_bounds().unwrap());
    let caret = fx.read(|editor, cx| editor.area().read(cx).caret_rect().unwrap());
    assert!(
        caret.right() <= bounds.right() + px(1.),
        "{caret:?} {bounds:?}"
    );
    // Soft wrap shows the same text on more rows and needs no horizontal scroll.
    fx.with(|editor, _, cx| editor.set_soft_wrap(true, cx));
    let rows = fx.read(|editor, cx| editor.area().read(cx).visual_rows().len());
    assert!(rows > 3, "{rows}");
    assert_eq!(fx.read(|editor, cx| editor.scroll_offset(cx)).x, px(0.));
}

#[test]
fn r6_the_gutter_width_follows_the_digit_rule_and_numbers_are_not_text() {
    require_backend!();
    let width_of = |lines: usize| {
        let source = "a\n".repeat(lines - 1);
        let mut fx = fixture(&source, |editor, cx| editor.set_rows(1, 4, cx));
        fx.host.paint_frame();
        let viewport = fx.read(|editor, cx| editor.area().read(cx).viewport_bounds().unwrap());
        let first = fx.read(|editor, cx| editor.line_bounds(cx))[0]
            .bounds
            .left();
        (first - viewport.left()).as_f32()
    };
    assert_eq!(width_of(1), 28.);
    assert_eq!(width_of(9), 28.);
    assert_eq!(width_of(99), 28.);
    assert_eq!(width_of(100), 33.); // 3 x 7 + 12
    assert_eq!(width_of(1000), 40.);
    // Numbers are painted by the gutter and are not part of the value or the selection.
    let mut fx = fixture("a\nb\nc", |_, _| {});
    fx.focus();
    fx.keys("ctrl-a ctrl-c");
    assert_eq!(fx.clipboard().as_deref(), Some("a\nb\nc"));
    // Hiding the gutter gives the text the whole width.
    let with = fx.read(|editor, cx| editor.line_bounds(cx))[0]
        .bounds
        .left();
    fx.with(|editor, _, cx| editor.set_line_numbers(false, cx));
    let without = fx.read(|editor, cx| editor.line_bounds(cx))[0]
        .bounds
        .left();
    assert_eq!((with - without).as_f32(), 28.);
}

#[test]
fn r7_lines_report_source_lines_and_bounds_and_clicks_raise_navigation() {
    require_backend!();
    let clicks: Rc<RefCell<Vec<LineClick>>> = Rc::default();
    let sink = clicks.clone();
    let mut fx = fixture(CODE, |editor, cx| {
        editor.set_gutter(
            |index| GutterLine {
                number: Some(index + 100),
                marker: None,
            },
            cx,
        );
        editor.set_on_line_click(move |click, _, _| sink.borrow_mut().push(click.clone()));
    });
    let bounds = fx.read(|editor, cx| editor.line_bounds(cx));
    assert_eq!(bounds.len(), 5, "{bounds:?}");
    assert_eq!(bounds[0].source_line, 100);
    assert_eq!(bounds[2].source_line, 102);
    assert_eq!(bounds[0].bounds.size.height, px(18.), "code line height 18");
    assert!(bounds[1].bounds.top() > bounds[0].bounds.top());
    // A click on a line (text) reports its source line; the position maps back to it.
    let at = fx.on_line(2, 40.);
    assert_eq!(
        fx.read(|editor, cx| editor.source_line_at(at, cx)),
        Some(102)
    );
    fx.click(at, MouseButton::Left, 1);
    // A click on the number reports on_number.
    let viewport = fx.read(|editor, cx| editor.area().read(cx).viewport_bounds().unwrap());
    let number = point(viewport.left() + px(10.), at.y);
    fx.click(number, MouseButton::Left, 1);
    let clicks = clicks.borrow();
    assert_eq!(clicks.len(), 2, "{clicks:?}");
    assert_eq!(
        (clicks[0].index, clicks[0].source_line, clicks[0].on_number),
        (2, 102, false)
    );
    assert_eq!(clicks[0].text, "    println!(\"{answer}\");");
    assert_eq!((clicks[1].source_line, clicks[1].on_number), (102, true));
}

#[test]
fn r7_overlays_anchor_to_a_line_without_changing_advances() {
    require_backend!();
    let seen: Rc<RefCell<Option<vespertine::Bounds<vespertine::Pixels>>>> = Rc::default();
    let mut fx = fixture(CODE, |_, _| {});
    let positions = |fx: &mut Fixture| -> Vec<Point<vespertine::Pixels>> {
        fx.read(|editor, cx| {
            let area = editor.area().read(cx);
            (0..CODE.len())
                .filter(|i| CODE.is_char_boundary(*i))
                .map(|i| area.point_for_index(i).unwrap())
                .collect()
        })
    };
    let before = positions(&mut fx);
    let sink = seen.clone();
    fx.with(|editor, _, cx| {
        editor.add_overlay(
            1,
            move |_, _| {
                let sink = sink.clone();
                div()
                    .size_full()
                    .child(
                        vespertine::canvas(
                            move |bounds, _, _| *sink.borrow_mut() = Some(bounds),
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .into_any_element()
            },
            cx,
        )
    });
    fx.host.paint_frame();
    assert_eq!(before, positions(&mut fx), "an overlay never moves glyphs");
    let line = fx.read(|editor, cx| editor.line_bounds(cx))[1].clone();
    let overlay = seen.borrow().expect("the overlay was laid out");
    assert_eq!(overlay.origin, line.bounds.origin);
    assert_eq!(overlay.size, line.bounds.size);
}

#[test]
fn a9_dropping_an_editor_with_a_context_menu_callback_releases_it() {
    require_backend!();
    let mut fx = fixture("alpha", |_, _| {});
    let owner = fx.owner.clone();
    fx.with(|editor, _, _| {
        editor.on_context_menu(&owner, |owner, request, _, _| {
            owner.menus.push(request.clone())
        });
    });
    let weak = fx.editor.clone();
    assert!(weak.upgrade().is_some());
    // The owner drops its handle: the callback refers back to the owner, yet the editor goes.
    let owner = fx.owner.clone();
    let window = fx.host.window();
    fx.host
        .app()
        .update_window(window, |_, _, cx| {
            owner.update(cx, |owner, cx| {
                owner.editor = None;
                cx.notify();
            })
        })
        .unwrap();
    fx.host.paint_frame();
    fx.host.paint_frame();
    assert!(weak.upgrade().is_none(), "the editor entity is released");
}
