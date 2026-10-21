//! TXT-008 links against real shaping and input dispatch: exact hitboxes, activation rules,
//! keyboard and menu behavior, shortened labels and file destinations.

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use support::{Build, Harness, at};
use vespertine::{IntoElement, MouseButton, ParentElement, Styled, TextRun, div, font, px, rgb};
use vespertine_richtext::{
    FlatText, Link, LinkActionKind, LinkActivation, LinkOptions, OriginalText, RichText,
    fit_labels, text_root,
};

const WIDTH: f32 = 360.0;
const URL: &str = "https://example.com/some/very/long/path/to/a/page.html";

type Log = Rc<RefCell<Vec<LinkActivation>>>;

fn options(log: &Log) -> LinkOptions {
    let log = log.clone();
    LinkOptions::default()
        .on_activate(move |activation, _, _| log.borrow_mut().push(activation.clone()))
}

fn doc(width: f32, text: &'static str, link: std::ops::Range<usize>, log: &Log) -> Build {
    let log = log.clone();
    Rc::new(move |_, _| {
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181818))
                .font_family("Noto Sans")
                .text_size(px(16.))
                .line_height(px(24.))
                .text_color(rgb(0xeeeeee))
                .child(
                    div().w(px(width)).child(
                        RichText::new("t", text)
                            .links(vec![Link {
                                range: link.clone(),
                                destination: URL.to_string(),
                            }])
                            .link_options(options(&log)),
                    ),
                ),
        )
        .into_any_element()
    })
}

fn new_log() -> Log {
    Rc::default()
}

#[test]
fn movement_of_four_pixels_activates_and_five_does_not() {
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        100.,
        doc(300., "link text and more words", 0..9, &log),
    );
    harness.press(at(4., 12.), MouseButton::Left, 1);
    harness.release(at(8., 12.), MouseButton::Left, 1);
    assert_eq!(log.borrow().len(), 1);
    assert_eq!(log.borrow()[0].destination, URL);
    assert_eq!(log.borrow()[0].action, LinkActionKind::Open);
    harness.press(at(4., 12.), MouseButton::Left, 1);
    harness.release(at(9., 12.), MouseButton::Left, 1);
    assert_eq!(log.borrow().len(), 1, "5 px is a movement, not a click");
}

#[test]
fn double_click_secondary_button_and_drags_never_navigate() {
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        100.,
        doc(300., "link text and more words", 0..9, &log),
    );
    harness.press(at(4., 12.), MouseButton::Left, 2);
    harness.release(at(4., 12.), MouseButton::Left, 2);
    harness.press(at(4., 12.), MouseButton::Left, 3);
    harness.release(at(4., 12.), MouseButton::Left, 3);
    // Right button opens the menu without navigating.
    harness.press(at(4., 12.), MouseButton::Right, 1);
    harness.release(at(4., 12.), MouseButton::Right, 1);
    assert!(log.borrow().is_empty());
    harness.key("escape");
    // A drag that ends on a link is a selection.
    harness.press(at(4., 12.), MouseButton::Left, 1);
    harness.move_to(at(40., 12.), Some(MouseButton::Left));
    harness.release(at(40., 12.), MouseButton::Left, 1);
    assert!(
        log.borrow().is_empty(),
        "a drag ending on a link does not navigate"
    );
    assert!(harness.service().selected_text().is_some());
}

#[test]
fn blank_space_after_the_last_glyph_never_activates() {
    let log = new_log();
    let mut harness = Harness::new(WIDTH, 100., doc(340., "see the link", 8..12, &log));
    harness.click(at(300., 12.));
    assert!(log.borrow().is_empty(), "past the end of the line");
    // The glyphs themselves do.
    let activated = (60..120).step_by(2).any(|x| {
        harness.click(at(x as f32, 12.));
        !log.borrow().is_empty()
    });
    assert!(activated);
}

#[test]
fn a_link_wrapped_over_two_rows_has_two_hit_regions() {
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        140.,
        doc(100., "aaa bbb ccc ddd eee fff ggg hhh", 4..27, &log),
    );
    let mut rows_hit = 0;
    for row in 0..4 {
        let y = 12. + 24. * row as f32;
        let before = log.borrow().len();
        for x in (1..100).step_by(3) {
            harness.click(at(x as f32, y));
            if log.borrow().len() > before {
                rows_hit += 1;
                break;
            }
        }
    }
    assert!(
        rows_hit >= 2,
        "both rows of a wrapped link activate ({rows_hit})"
    );
}

#[test]
fn keyboard_activation_menu_and_escape_return_focus() {
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        200.,
        doc(300., "link text and more words", 0..9, &log),
    );
    // Tab focuses the link (each link range is a tab stop); Enter and Space activate.
    harness.focus_next();
    harness.key("enter");
    harness.key("space");
    assert_eq!(log.borrow().len(), 2);
    // Shift+F10 opens the menu; Down moves to the second row; Enter chooses "open externally".
    harness.key("shift-f10");
    harness.key("down");
    harness.key("enter");
    assert_eq!(log.borrow().len(), 3);
    assert_eq!(log.borrow()[2].action, LinkActionKind::OpenExternally);
    // Escape closes the menu and returns focus to the link: Enter activates the link again.
    harness.key("shift-f10");
    harness.key("escape");
    harness.key("enter");
    assert_eq!(log.borrow().len(), 4);
    assert_eq!(log.borrow()[3].action, LinkActionKind::Open);
    // The menu cycles with wraparound: Up from the first row reaches copy.
    harness.key("shift-f10");
    harness.key("up");
    harness.key("enter");
    assert_eq!(harness.clipboard_text().as_deref(), Some(URL));
    assert_eq!(log.borrow().len(), 4, "copy is not a navigation");
}

#[test]
fn disallowed_schemes_disable_navigation_but_not_copy() {
    let log = new_log();
    let log2 = log.clone();
    let build: Build = Rc::new(move |_, _| {
        let log = log2.clone();
        text_root(
            div()
                .size_full()
                .font_family("Noto Sans")
                .text_size(px(16.))
                .line_height(px(24.))
                .child(
                    RichText::new("t", "click me")
                        .links(vec![Link {
                            range: 0..8,
                            destination: "javascript:alert(1)".to_string(),
                        }])
                        .link_options(options(&log)),
                ),
        )
        .into_any_element()
    });
    let mut harness = Harness::new(WIDTH, 100., build);
    harness.click(at(10., 12.));
    harness.focus_next();
    harness.key("enter");
    assert!(
        log.borrow().is_empty(),
        "navigation is refused for this scheme"
    );
    harness.key("shift-f10");
    harness.key("enter");
    assert_eq!(
        harness.clipboard_text().as_deref(),
        Some("javascript:alert(1)")
    );
    assert!(log.borrow().is_empty());
}

#[test]
fn a_shortened_link_copies_and_navigates_as_its_source() {
    let log = new_log();
    let log2 = log.clone();
    let source: &'static str = "visit https://example.com/some/very/long/path/to/a/page.html today";
    let build: Build = Rc::new(move |window, _| {
        let family = font("Noto Sans");
        let run = |len| TextRun {
            len,
            font: family.clone(),
            color: rgb(0xeeeeee).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let start = source.find("https").unwrap();
        let end = start + URL.len();
        let flat = FlatText {
            text: source.to_string(),
            runs: vec![
                (start, run(start)),
                (URL.len(), run(URL.len())),
                (source.len() - end, run(source.len() - end)),
            ],
            links: vec![start..end],
            code: vec![],
            slots: vec![],
        };
        let text_system = window.text_system().clone();
        let shortened = fit_labels(&flat, px(120.), |text, runs| {
            let runs: Vec<TextRun> = runs
                .iter()
                .map(|(len, style)| TextRun {
                    len: *len,
                    ..style.clone()
                })
                .collect();
            text_system
                .shape_line(text.to_string().into(), px(16.), &runs, None)
                .width
        });
        assert!(shortened.flat.text.contains('\u{2026}'));
        let original =
            OriginalText::plain(source).present(shortened.flat.text.clone(), shortened.map.clone());
        let link = shortened.flat.links[0].clone();
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181818))
                .font_family("Noto Sans")
                .text_size(px(16.))
                .line_height(px(24.))
                .text_color(rgb(0xeeeeee))
                .child(
                    RichText::with_original("t", original)
                        .links(vec![Link {
                            range: link,
                            destination: URL.to_string(),
                        }])
                        .link_options(options(&log2)),
                ),
        )
        .into_any_element()
    });
    let mut harness = Harness::new(WIDTH, 100., build);
    // Copying a selection that spans the shortened label returns the full source label.
    harness.drag(at(1., 12.), at(340., 12.));
    assert_eq!(harness.service().selected_text().as_deref(), Some(source));
    // The shortened link still navigates, to the complete destination.
    harness.click(at(150., 90.));
    harness.click(at(80., 12.));
    let log = log.borrow();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].destination, URL);
}

#[test]
fn a_keyboard_focused_link_discloses_its_destination_until_escape() {
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        200.,
        doc(300., "link text and more words", 0..9, &log),
    );
    let before = harness.screenshot();
    harness.focus_next();
    // Focus by keyboard: any key press sets the keyboard modality.
    harness.key("shift");
    let shown = harness.screenshot();
    let card = |image: &image::RgbaImage| support::rgb_at(image, WIDTH, 40., 33.);
    assert_ne!(
        card(&before),
        card(&shown),
        "the card appears below the link"
    );
    harness.key("escape");
    let dismissed = harness.screenshot();
    assert_eq!(
        card(&before),
        card(&dismissed),
        "Escape dismisses the disclosure"
    );
    assert!(log.borrow().is_empty());
}

#[test]
fn touch_taps_activate() {
    use vespertine::{PlatformInput, TouchEvent, TouchId, TouchPhase};
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        100.,
        doc(300., "link text and more words", 0..9, &log),
    );
    for phase in [TouchPhase::Started, TouchPhase::Ended] {
        let event = PlatformInput::Touch(TouchEvent {
            id: TouchId(1),
            phase,
            position: at(10., 12.),
            predicted_position: None,
            force: None,
        });
        harness
            .cx
            .update_window(harness.window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .unwrap();
        harness.draw();
    }
    harness
        .cx
        .advance_clock(std::time::Duration::from_millis(400));
    harness.draw();
    assert_eq!(log.borrow().len(), 1, "a tap on a link activates it");
}

#[test]
fn a_press_outside_dismisses_the_focused_disclosure() {
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        200.,
        doc(300., "link text and more words", 0..9, &log),
    );
    let before = harness.screenshot();
    harness.focus_next();
    harness.key("shift");
    let card = |image: &image::RgbaImage| support::rgb_at(image, WIDTH, 40., 33.);
    assert_ne!(
        card(&before),
        card(&harness.screenshot()),
        "the card is shown"
    );
    harness.click(at(250., 150.));
    assert_eq!(
        card(&before),
        card(&harness.screenshot()),
        "an outside press dismisses it"
    );
}

/// `text` with one link per `(range, destination)`.
fn two_links(
    text: &'static str,
    links: [(std::ops::Range<usize>, &'static str); 2],
    log: &Log,
) -> Build {
    let log = log.clone();
    Rc::new(move |_, _| {
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181818))
                .font_family("Noto Sans")
                .text_size(px(16.))
                .line_height(px(24.))
                .text_color(rgb(0xeeeeee))
                .child(
                    div().w(px(300.)).child(
                        RichText::new("t", text)
                            .links(
                                links
                                    .iter()
                                    .map(|(range, destination)| Link {
                                        range: range.clone(),
                                        destination: destination.to_string(),
                                    })
                                    .collect(),
                            )
                            .link_options(options(&log)),
                    ),
                ),
        )
        .into_any_element()
    })
}

#[test]
fn a_release_on_an_adjacent_link_activates_neither() {
    let log = new_log();
    let mut harness = Harness::new(
        WIDTH,
        100.,
        two_links(
            "mm",
            [
                (0..1, "https://example.com/alpha"),
                (1..2, "https://example.com/beta"),
            ],
            &log,
        ),
    );
    // Each "m" is about 13.6 px wide: press near the end of the first, release 3.5 px later
    // inside the second.
    harness.press(at(12., 12.), MouseButton::Left, 1);
    harness.release(at(15.5, 12.), MouseButton::Left, 1);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    // A click inside the second link still activates it.
    harness.click(at(20., 12.));
    assert_eq!(log.borrow().len(), 1);
    assert_eq!(log.borrow()[0].destination, "https://example.com/beta");
}
