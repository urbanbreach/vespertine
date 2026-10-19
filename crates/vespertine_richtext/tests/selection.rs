//! TXT-008 against real shaping and input dispatch: drags, groups, chrome, virtualization,
//! the state machine's observable effects, scopes and the selection wash.

mod support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use support::{Build, Harness, at, near, rgb_at};
use unicode_segmentation::UnicodeSegmentation;
use vespertine::{
    AnyElement, FocusHandle, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled,
    deferred, div, px, rgb,
};
use vespertine_richtext::{
    RichText, RunKey, SelectionRequest, UserSelect, selection_color, surface, text_layer,
    text_root, user_select,
};

const WIDTH: f32 = 240.0;

fn page(children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    text_root(
        div()
            .size_full()
            .bg(rgb(0x181818))
            .font_family("Noto Sans")
            .text_size(px(16.))
            .line_height(px(24.))
            .text_color(rgb(0xeeeeee))
            .children(children),
    )
    .into_any_element()
}

fn lines(texts: &'static [&'static str]) -> Build {
    Rc::new(move |_, _| {
        page(
            texts
                .iter()
                .enumerate()
                .map(|(ix, text)| RichText::new(("line", ix), *text).into_any_element()),
        )
    })
}

#[test]
fn wrapped_unicode_text_selects_by_drag_in_both_directions() {
    const TEXT: &str = "Latin words e\u{301}cole \u{4e2d}\u{6587}\u{6587}\u{5b57} family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466} flag end of the line text";
    let build: Build = Rc::new(|_, _| {
        page([div()
            .w(px(120.))
            .child(RichText::new("wrapped", TEXT))
            .into_any_element()])
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    let boundaries: Vec<usize> = TEXT
        .grapheme_indices(true)
        .map(|(ix, _)| ix)
        .chain([TEXT.len()])
        .collect();

    // Whole text, forwards and backwards.
    harness.drag(at(1., 12.), at(119., 190.));
    assert_eq!(service.selected_text().as_deref(), Some(TEXT));
    // A press must start on a line: look for the lowest line whose end starts the reverse drag.
    let reverse = (0..190).rev().step_by(4).any(|y| {
        harness.drag(at(119., y as f32), at(1., 12.));
        service.selected_text().as_deref() == Some(TEXT)
    });
    assert!(
        reverse,
        "a reverse drag from the end of the last line selects everything"
    );

    // Every pointer position yields a selection that ends on a grapheme boundary.
    for y in (6..150).step_by(24) {
        for x in (2..118).step_by(3) {
            harness.drag(at(1., 12.), at(x as f32, y as f32));
            if let Some(text) = service.selected_text() {
                assert!(TEXT.starts_with(&text), "{text:?}");
                assert!(
                    boundaries.contains(&text.len()),
                    "cut inside a cluster at ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn drag_reverse_and_contract() {
    let mut harness = Harness::new(
        WIDTH,
        200.,
        lines(&["first line", "second line", "third line"]),
    );
    let service = harness.service();
    harness.drag(at(35., 12.), at(45., 60.));
    let forward = service.selected_text().unwrap();
    assert!(forward.contains("\nsecond line\nth"), "{forward:?}");
    assert!("first line".ends_with(forward.split('\n').next().unwrap()));
    assert!("third line".starts_with(forward.rsplit('\n').next().unwrap()));
    harness.drag(at(45., 60.), at(35., 12.));
    assert_eq!(service.selected_text().unwrap(), forward);
    // Contracting back into the anchor run.
    harness.press(at(45., 60.), MouseButton::Left, 1);
    harness.move_to(at(35., 12.), Some(MouseButton::Left));
    assert!(service.selected_text().unwrap().contains('\n'));
    harness.move_to(at(15., 60.), Some(MouseButton::Left));
    let contracted = service.selected_text().unwrap();
    assert!(
        !contracted.contains('\n') && "third line".contains(&contracted),
        "{contracted:?}"
    );
    harness.release(at(15., 60.), MouseButton::Left, 1);
}

#[test]
fn groups_join_without_newline_and_separate_groups_with_one() {
    let group = RunKey::new("group", 0);
    let build: Build = Rc::new(move |_, _| {
        page([
            div()
                .flex()
                .flex_row()
                .child(RichText::new("a", "Hello, ").group(group))
                .child(RichText::new("b", "world").group(group))
                .into_any_element(),
            RichText::new("c", "next paragraph").into_any_element(),
        ])
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    harness.drag(at(1., 12.), at(60., 36.));
    let text = service.selected_text().unwrap();
    assert!(text.starts_with("Hello, world\nnext"), "{text:?}");
}

#[test]
fn code_block_gutters_headers_and_chrome_never_enter_the_copy() {
    let build: Build = Rc::new(|_, _| {
        let row = |ix: u32, line: &'static str| {
            div()
                .flex()
                .flex_row()
                .child(RichText::new(("gutter", ix), format!("{}", ix + 1)).chrome())
                .child(RichText::new(("code", ix), line).sub_index(ix))
                .into_any_element()
        };
        page([
            RichText::new("header", "main.rs")
                .chrome()
                .into_any_element(),
            row(0, "fn main() {"),
            row(1, ""),
            row(2, "    run();"),
            row(3, "}"),
        ])
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    // Press in the first code row, drag to the last one.
    harness.drag(at(40., 36.), at(60., 108.));
    let text = service.selected_text().unwrap();
    assert_eq!(text, "main() {\n\n    run();\n}".get(..text.len()).unwrap());
    assert!(text.contains("\n\n"), "blank line survives: {text:?}");
    assert!(!text.contains("main.rs") && !text.contains('1'));
    // The chrome text is logged but never registered.
    let painted = service.painted_text();
    assert!(
        painted
            .iter()
            .any(|entry| entry.chrome && entry.text == "main.rs")
    );
    assert!(
        painted
            .iter()
            .any(|entry| !entry.chrome && entry.text == "fn main() {")
    );
    assert_eq!(service.registered_runs(), 4);
}

#[test]
fn rows_leaving_paint_keep_their_text_in_the_copy() {
    let first_row = Rc::new(Cell::new(0usize));
    let build: Build = {
        let first_row = first_row.clone();
        Rc::new(move |_, _| {
            let rows = ["row zero", "row one", "row two", "row three", "row four"];
            page(
                rows.iter()
                    .enumerate()
                    .skip(first_row.get())
                    .map(|(ix, text)| RichText::new(("row", ix), *text).into_any_element()),
            )
        })
    };
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    harness.press(at(1., 12.), MouseButton::Left, 1);
    harness.move_to(at(50., 60.), Some(MouseButton::Left));
    assert!(
        service
            .selected_text()
            .unwrap()
            .starts_with("row zero\nrow one\nrow t")
    );
    // Rows 0 and 1 are virtualized away while the drag continues.
    first_row.set(2);
    harness.redraw();
    harness.move_to(at(50., 36.), Some(MouseButton::Left));
    let text = service.selected_text().unwrap();
    assert!(
        text.starts_with("row zero\nrow one\nrow two\nrow th"),
        "{text:?}"
    );
    harness.release(at(50., 36.), MouseButton::Left, 1);
    assert_eq!(
        service.selected_text().unwrap(),
        text,
        "release ends the drag cleanly"
    );
}

#[test]
fn press_selects_nothing_and_does_not_blur_but_dragging_blurs() {
    let focus: Rc<RefCell<Option<FocusHandle>>> = Rc::default();
    let build: Build = {
        let focus = focus.clone();
        Rc::new(move |_, cx| {
            let handle = focus
                .borrow_mut()
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            text_root(
                div()
                    .size_full()
                    .font_family("Noto Sans")
                    .text_size(px(16.))
                    .line_height(px(24.))
                    .child(div().id("field").track_focus(&handle).size(px(1.)))
                    .child(RichText::new("text", "hello world")),
            )
            .into_any_element()
        })
    };
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    let handle = focus.borrow().clone().unwrap();
    harness
        .cx
        .update_window(harness.window.into(), |_, window, cx| {
            handle.focus(window, cx)
        })
        .unwrap();
    harness.draw();
    let focused = |harness: &mut Harness| {
        let handle = handle.clone();
        harness
            .cx
            .update_window(harness.window.into(), |_, window, _| {
                handle.is_focused(window)
            })
            .unwrap()
    };
    assert!(focused(&mut harness));
    harness.press(at(15., 20.), MouseButton::Left, 1);
    assert!(service.selected_text().is_none());
    assert!(focused(&mut harness), "a press alone does not blur");
    harness.move_to(at(70., 20.), Some(MouseButton::Left));
    assert!(!focused(&mut harness), "the first dragging move blurs");
    assert!(service.selected_text().is_some());
    harness.release(at(70., 20.), MouseButton::Left, 1);
}

#[test]
fn double_and_triple_click_and_change_events() {
    let mut harness = Harness::new(
        WIDTH,
        200.,
        lines(&["foo_bar            baz qux", "second"]),
    );
    let service = harness.service();
    let events: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
    {
        let events = events.clone();
        service.on_selection_change(move |text, _, _| events.borrow_mut().push(text));
    }
    harness.draw();
    assert!(
        events.borrow().is_empty(),
        "the initial empty state emits nothing"
    );
    harness.press(at(30., 12.), MouseButton::Left, 2);
    harness.release(at(30., 12.), MouseButton::Left, 2);
    assert_eq!(service.selected_text().as_deref(), Some("foo_bar"));
    // Repeated frames emit no duplicate event.
    harness.draw();
    harness.draw();
    assert_eq!(events.borrow().as_slice(), &[Some("foo_bar".to_string())]);
    harness.press(at(30., 12.), MouseButton::Left, 3);
    harness.release(at(30., 12.), MouseButton::Left, 3);
    assert_eq!(
        service.selected_text().as_deref(),
        Some("foo_bar            baz qux")
    );
    // Whitespace between two spaces selects nothing.
    harness.press(at(85., 12.), MouseButton::Left, 2);
    harness.release(at(85., 12.), MouseButton::Left, 2);
    assert!(service.selected_text().is_none());
    assert_eq!(
        events.borrow().last(),
        Some(&None),
        "became empty reports absent"
    );
    // Clearing emits an absent value once.
    harness.press(at(30., 12.), MouseButton::Left, 2);
    harness.release(at(30., 12.), MouseButton::Left, 2);
    let count = events.borrow().len();
    harness
        .cx
        .update_window(harness.window.into(), |_, window, cx| {
            service.clear_selection(window, cx)
        })
        .unwrap();
    harness.draw();
    assert_eq!(events.borrow().len(), count + 1);
    assert_eq!(events.borrow().last(), Some(&None));
    assert!(service.selected_text().is_none());
}

#[test]
fn copy_writes_the_joined_text_and_an_empty_selection_lets_the_key_propagate() {
    let propagated = Rc::new(Cell::new(0));
    let focus: Rc<RefCell<Option<FocusHandle>>> = Rc::default();
    let build: Build = {
        let propagated = propagated.clone();
        let focus = focus.clone();
        Rc::new(move |_, cx| {
            let propagated = propagated.clone();
            let handle = focus
                .borrow_mut()
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            text_root(
                div()
                    .id("host")
                    .track_focus(&handle)
                    .size_full()
                    .font_family("Noto Sans")
                    .text_size(px(16.))
                    .line_height(px(24.))
                    .on_key_down(move |_, _, _| propagated.set(propagated.get() + 1))
                    .child(RichText::new("a", "alpha beta"))
                    .child(RichText::new("b", "gamma")),
            )
            .into_any_element()
        })
    };
    let mut harness = Harness::new(WIDTH, 200., build);
    let handle = focus.borrow().clone().unwrap();
    harness
        .cx
        .update_window(harness.window.into(), |_, window, cx| {
            handle.focus(window, cx)
        })
        .unwrap();
    harness.draw();
    // Without a selection the chord propagates to the focused host.
    assert!(
        harness.key("ctrl-c"),
        "an empty selection lets the key propagate"
    );
    assert_eq!(harness.clipboard_text(), None);
    assert_eq!(propagated.get(), 1);
    harness.drag(at(1., 12.), at(40., 36.));
    // A drag blurs the focused control; refocus the host as an app would.
    harness
        .cx
        .update_window(harness.window.into(), |_, window, cx| {
            handle.focus(window, cx)
        })
        .unwrap();
    harness.draw();
    assert!(!harness.key("ctrl-c"), "the copy chord stops propagation");
    assert_eq!(harness.clipboard_text().as_deref(), Some("alpha beta\ngam"));
}

#[test]
fn user_select_none_is_painted_but_not_selectable_and_text_islands_reenable() {
    let build: Build = Rc::new(|_, _| {
        page([
            user_select(
                UserSelect::None,
                div()
                    .child(RichText::new("locked", "locked text"))
                    .child(user_select(
                        UserSelect::Text,
                        RichText::new("island", "island text"),
                    )),
            )
            .into_any_element(),
            RichText::new("free", "free text").into_any_element(),
        ])
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    assert_eq!(
        service.registered_runs(),
        2,
        "locked text is not registered"
    );
    assert!(
        service
            .painted_text()
            .iter()
            .any(|entry| entry.text == "locked text")
    );
    // A press on locked text does not start a selection.
    harness.press(at(10., 12.), MouseButton::Left, 2);
    harness.release(at(10., 12.), MouseButton::Left, 2);
    assert!(service.selected_text().is_none());
    // The text island can.
    harness.press(at(10., 36.), MouseButton::Left, 2);
    harness.release(at(10., 36.), MouseButton::Left, 2);
    assert_eq!(service.selected_text().as_deref(), Some("island"));
    // Dragging from the island across the locked row never copies the locked text.
    harness.drag(at(1., 36.), at(20., 60.));
    let text = service.selected_text().unwrap();
    assert!(text.starts_with("island text\nf"), "{text:?}");
}

#[test]
fn surfaces_with_equal_keys_never_share_a_selection() {
    let build: Build = Rc::new(|_, _| {
        page([
            surface(1, RichText::new("run", "main document")).into_any_element(),
            surface(2, RichText::new("run", "modal document")).into_any_element(),
        ])
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    let service = harness.service();
    harness.drag(at(1., 12.), at(120., 36.));
    assert_eq!(service.selected_text().as_deref(), Some("main document"));
    harness.drag(at(1., 36.), at(200., 40.));
    assert_eq!(service.selected_text().as_deref(), Some("modal document"));
}

fn wash_pixels(selection: Option<u32>) -> (Vec<[u8; 3]>, Harness) {
    let build: Build = Rc::new(move |_, _| {
        let content = div()
            .child(RichText::new("a", "wash row one"))
            .child(RichText::new("b", "wash row two"));
        let content = match selection {
            Some(color) => {
                let [r, g, b, a] = [
                    (color >> 24) & 0xff,
                    (color >> 16) & 0xff,
                    (color >> 8) & 0xff,
                    color & 0xff,
                ]
                .map(|c| c as f32 / 255.);
                selection_color(vespertine::Rgba { r, g, b, a }, content).into_any_element()
            }
            None => content.into_any_element(),
        };
        page([content])
    });
    let mut harness = Harness::new(WIDTH, 120., build);
    harness.drag(at(0., 12.), at(100., 36.));
    let image = harness.screenshot();
    let samples = vec![
        rgb_at(&image, WIDTH, 2., 3.),
        rgb_at(&image, WIDTH, 2., 27.),
        rgb_at(&image, WIDTH, 200., 12.),
    ];
    (samples, harness)
}

#[test]
fn selection_wash_composites_the_explicit_color_and_inherits() {
    let (samples, _harness) = wash_pixels(Some(0x4466cc80));
    // #4466cc at alpha 0.502 over #181818 is #2E3F72 (within one step).
    assert!(near(samples[0], [0x2e, 0x3f, 0x72], 2), "{samples:?}");
    assert!(near(samples[1], [0x2e, 0x3f, 0x72], 2), "{samples:?}");
    assert!(
        near(samples[2], [0x18, 0x18, 0x18], 1),
        "outside the span: {samples:?}"
    );
}

#[test]
fn default_selection_color_is_the_accent_at_alpha_035() {
    let (samples, _harness) = wash_pixels(None);
    // Accent #7C86FF at 0.35 over #181818.
    let expected = |a: u8, b: u8| (a as f32 * 0.35 + b as f32 * 0.65).round() as u8;
    let want = [
        expected(0x7c, 0x18),
        expected(0x86, 0x18),
        expected(0xff, 0x18),
    ];
    assert!(near(samples[0], want, 2), "{samples:?} vs {want:?}");
}

#[test]
fn programmatic_selection_applies_when_the_frame_finishes() {
    let build: Build = Rc::new(|_, _| {
        page([
            RichText::new("a", "alpha")
                .key(RunKey::new("k", 0))
                .into_any_element(),
            RichText::new("b", "beta")
                .key(RunKey::new("k", 1))
                .into_any_element(),
        ])
    });
    let mut harness = Harness::new(WIDTH, 120., build);
    let service = harness.service();
    service.select_runs(SelectionRequest {
        from: (RunKey::new("k", 0), 2),
        to: (RunKey::new("k", 1), 2),
    });
    harness.settle();
    assert_eq!(service.selected_text().as_deref(), Some("pha\nbe"));
}

#[test]
fn text_in_a_deferred_layer_registers_through_a_text_layer_and_takes_a_programmatic_selection() {
    let build: Build = Rc::new(|_, _| {
        page([
            RichText::new("a", "main")
                .key(RunKey::new("k", 0))
                .into_any_element(),
            // A deferred layer paints after the document root has finished: without the layer
            // scope its text would not register.
            deferred(div().absolute().top(px(60.)).child(text_layer(
                RichText::new("b", "layer text").key(RunKey::new("k", 1)),
            )))
            .into_any_element(),
            deferred(
                div()
                    .absolute()
                    .top(px(100.))
                    .child(RichText::new("c", "unscoped").key(RunKey::new("k", 2))),
            )
            .into_any_element(),
        ])
    });
    let mut harness = Harness::new(WIDTH, 160., build);
    let service = harness.service();
    harness.settle();
    assert_eq!(
        service.registered_runs(),
        2,
        "the main run and the layer's run register; the unscoped deferred run does not"
    );
    service.select_runs(SelectionRequest {
        from: (RunKey::new("k", 0), 1),
        to: (RunKey::new("k", 1), 5),
    });
    harness.settle();
    assert_eq!(service.selected_text().as_deref(), Some("ain\nlayer"));
}

#[test]
fn selection_over_a_soft_wrapped_run_washes_every_row_one_line_high() {
    let build: Build = Rc::new(|_, _| {
        page([selection_color(
            vespertine::Rgba {
                r: 0x44 as f32 / 255.,
                g: 0x66 as f32 / 255.,
                b: 0xcc as f32 / 255.,
                a: 0x80 as f32 / 255.,
            },
            div().w(px(100.)).child(RichText::new(
                "wrapped",
                "alpha beta gamma delta epsilon zeta eta theta",
            )),
        )
        .into_any_element()])
    });
    let mut harness = Harness::new(WIDTH, 200., build);
    harness.drag(at(0., 12.), at(99., 190.));
    let image = harness.screenshot();
    let mut washed_rows = 0;
    for row in 0..8 {
        // The first and last pixel line of every row are washed: rows are one line high, with no
        // vertical gaps between them.
        let top = rgb_at(&image, WIDTH, 2., row as f32 * 24. + 0.5);
        let bottom = rgb_at(&image, WIDTH, 2., row as f32 * 24. + 23.5);
        if near(top, [0x2e, 0x3f, 0x72], 2) && near(bottom, [0x2e, 0x3f, 0x72], 2) {
            washed_rows += 1;
        }
    }
    assert!(washed_rows >= 4, "{washed_rows} rows washed");
    // Below the text nothing is washed.
    assert!(near(
        rgb_at(&image, WIDTH, 2., 24. * 8. + 0.5),
        [0x18, 0x18, 0x18],
        1
    ));
}
