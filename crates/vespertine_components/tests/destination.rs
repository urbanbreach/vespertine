//! CMP-004 A7 and A8: the link destination card.

use std::{cell::RefCell, rc::Rc};
use vespertine::AppContext as _;
use vespertine::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render, Size,
    Styled, TestAppContext, Window, div, px, rgb, size,
};
use vespertine_components::tokens::ComponentTokens;
use vespertine_components::tooltip::card::HoverCardHost;
use vespertine_components::tooltip::destination::{DestinationCard, card_limits, destination_card};
use vespertine_richtext::{TextService, text_root};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

#[vespertine::test]
fn limits_follow_the_viewport() {
    for (viewport, expected) in [
        (size(px(320.), px(240.)), (296.0, 160.0)),
        (size(px(1000.), px(800.)), (360.0, 160.0)),
        (size(px(80.), px(80.)), (56.0, 1.0)),
    ] {
        assert_eq!(card_limits(viewport), expected);
        assert!(
            expected.0 <= f32::from(viewport.width) && expected.1 <= f32::from(viewport.height)
        );
    }
}

struct Empty;
impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn measure(
    cx: &mut TestAppContext,
    text: &str,
    viewport: Size<vespertine::Pixels>,
) -> DestinationCard {
    let (_, vcx) = cx.add_window_view(|_, _| Empty);
    let text = text.to_string();
    vcx.update(|window, _| destination_card(&text, viewport, ComponentTokens::neutral(), window))
}

#[vespertine::test]
fn removing_every_inserted_zero_width_space_reproduces_the_original(cx: &mut TestAppContext) {
    let destination = format!("{}?q=\u{1F600}&x=1", "\u{4E2D}\u{6587}".repeat(5000));
    let card = measure(cx, &destination, size(px(400.), px(300.)));
    assert_ne!(card.display_text(), destination);
    assert_eq!(card.display_text().replace('\u{200B}', ""), destination);
    assert_eq!(
        card.copied_text(),
        destination,
        "copying yields the original exactly"
    );
    assert_eq!(card.destination(), destination);
    // Wrapped but never above the maximum, and never above the viewport.
    let bounds = card.bounds();
    assert!(f32::from(bounds.width) <= 360.0);
    assert!(f32::from(bounds.height) <= 160.0);
    assert!(
        f32::from(bounds.height) > 100.0,
        "a long destination uses its scroll height"
    );
}

#[vespertine::test]
fn the_card_never_exceeds_a_tiny_viewport(cx: &mut TestAppContext) {
    let card = measure(cx, &"a".repeat(400), size(px(80.), px(80.)));
    assert!(f32::from(card.bounds().width) <= 56.0);
    assert!(f32::from(card.bounds().height) <= 1.0);
}

/// Measures cards in the production text system (the offscreen test host), where the break
/// opportunities are zero-width.
struct Measurer {
    out: Rc<RefCell<Vec<DestinationCard>>>,
}

impl Render for Measurer {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.out.borrow().is_empty() {
            let viewport = size(px(400.), px(300.));
            let mut cards = vec![
                destination_card(
                    "docs/readme.md",
                    viewport,
                    ComponentTokens::neutral(),
                    window,
                ),
                destination_card(
                    &"x".repeat(2000),
                    viewport,
                    ComponentTokens::neutral(),
                    window,
                ),
            ];
            self.out.borrow_mut().append(&mut cards);
        }
        div().size_full()
    }
}

#[test]
fn a_short_destination_stays_on_one_line_and_a_long_one_wraps_and_scrolls() {
    require_backend!();
    let out = Rc::new(RefCell::new(Vec::new()));
    let sink = out.clone();
    let mut host = TestHost::with_view(HostOptions::with_size(400.0, 300.0), move |_, cx| {
        cx.new(|_| Measurer { out: sink })
    })
    .expect("host");
    host.flush();
    let cards = out.borrow();
    let short = &cards[0];
    assert_eq!(
        f32::from(short.bounds().height),
        14.0 + 10.0,
        "one 14 px line plus the chrome"
    );
    assert_eq!(f32::from(short.bounds().width), short.limits().width);
    assert!(f32::from(short.bounds().width) < 200.0);
    let long = &cards[1];
    assert_eq!(
        f32::from(long.bounds().width),
        360.0,
        "capped at the maximum width"
    );
    assert_eq!(
        f32::from(long.bounds().height),
        160.0,
        "capped at the maximum height"
    );
}

/// A long destination of non-Latin text, a query string and an emoji.
fn long_destination() -> String {
    format!(
        "/\u{30C9}\u{30AD}\u{30E5}\u{30E1}\u{30F3}\u{30C8}/{}?\u{691C}\u{7D22}={}&e=\u{1F600}",
        "\u{9577}\u{3044}\u{30D1}\u{30B9}".repeat(60),
        "\u{30C6}\u{30B9}\u{30C8}".repeat(60),
    )
}

const WINDOW: (f64, f64) = (720., 480.);
/// The link trigger sits against the right edge of the window.
const LINK: (f32, f32, f32, f32) = (592., 40., 120., 22.);

struct Page {
    cards: Entity<HoverCardHost<u8>>,
    focus: FocusHandle,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (x, y, w, h) = LINK;
        let link = div()
            .id("link")
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .child("the document");
        text_root(
            div()
                .id("page")
                .track_focus(&self.focus)
                .size_full()
                .relative()
                .bg(rgb(0x181A1F))
                .text_color(rgb(0xF0F1F3))
                .child(HoverCardHost::trigger(&self.cards, 0, link, cx))
                .child(self.cards.clone()),
        )
    }
}

fn pixels_differing(
    a: &image::RgbaImage,
    b: &image::RgbaImage,
    region: (u32, u32, u32, u32),
) -> usize {
    let (x0, y0, w, h) = region;
    let mut count = 0;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            if a.get_pixel(x, y) != b.get_pixel(x, y) {
                count += 1;
            }
        }
    }
    count
}

/// A8: the card of a long Unicode destination, opened from a link at the right edge of the
/// window through real hover events, stays inside the window, wraps, scrolls past its maximum
/// height, copies the exact destination and touches no network (the card is plain text with no
/// link behavior).
#[test]
fn a_long_unicode_destination_card_stays_inside_the_window_wraps_scrolls_and_copies_exactly() {
    require_backend!();
    let destination = long_destination();
    let measured: Rc<RefCell<Option<DestinationCard>>> = Rc::new(RefCell::new(None));
    let service: Rc<RefCell<Option<TextService>>> = Rc::new(RefCell::new(None));
    let cards_slot: Rc<RefCell<Option<Entity<HoverCardHost<u8>>>>> = Rc::new(RefCell::new(None));
    let page_slot: Rc<RefCell<Option<Entity<Page>>>> = Rc::new(RefCell::new(None));
    let page_sink = page_slot.clone();
    let (sink, service_sink, cards_sink, text) = (
        measured.clone(),
        service.clone(),
        cards_slot.clone(),
        destination.clone(),
    );
    let mut host = TestHost::with_view(
        HostOptions::with_size(WINDOW.0, WINDOW.1),
        move |window, cx| {
            *service_sink.borrow_mut() = Some(TextService::of(window, cx));
            let cards = cx.new(|cx| {
                HoverCardHost::new(
                    move |_: &u8, window, cx| {
                        let card = destination_card(
                            &text,
                            window.viewport_size(),
                            ComponentTokens::current(cx),
                            window,
                        );
                        let element = card.element().into_any_element();
                        *sink.borrow_mut() = Some(card);
                        element
                    },
                    cx,
                )
                .interactive(true)
                .bare(true)
            });
            *cards_sink.borrow_mut() = Some(cards.clone());
            let page = cx.new(|cx| Page {
                cards,
                focus: cx.focus_handle(),
            });
            let focus = page.read(cx).focus.clone();
            window.focus(&focus, cx);
            *page_sink.borrow_mut() = Some(page.clone());
            page
        },
    )
    .expect("host");
    let cards = cards_slot.borrow().clone().expect("cards");
    let page = page_slot.borrow().clone().expect("page");
    let service = service.borrow().clone().expect("service");
    host.flush();

    // Real hover events over the link open the card after the dwell.
    let (lx, ly, lw, lh) = LINK;
    host.mouse_move(f64::from(lx + lw / 2.), f64::from(ly + lh / 2.), None, "")
        .unwrap();
    host.flush();
    host.advance_executor_time(550.).unwrap();
    host.flush();
    let bounds = host
        .app()
        .update(|cx| cards.read(cx).card_bounds())
        .expect("the card is painted");
    let card = measured.borrow().clone().expect("the card was measured");

    // Inside the window (the 8 px containment inset included), although the link ends at the
    // right edge and the card is wider than the link.
    let (left, right) = (f32::from(bounds.left()), f32::from(bounds.right()));
    let (top, bottom) = (f32::from(bounds.top()), f32::from(bounds.bottom()));
    assert!(
        left >= 8.0 && right <= WINDOW.0 as f32 - 8.0,
        "{left}..{right}"
    );
    assert!(
        top >= 8.0 && bottom <= WINDOW.1 as f32 - 8.0,
        "{top}..{bottom}"
    );
    assert!(
        left < lx,
        "the card shifted left of the link to stay inside the window"
    );
    assert!(top >= ly + lh, "the card sits below the link");

    // The painted card has the measured size: the maximum width and the maximum height.
    assert!((f32::from(bounds.size.width) - f32::from(card.bounds().width)).abs() < 1.0);
    assert!((f32::from(bounds.size.height) - f32::from(card.bounds().height)).abs() < 1.0);
    assert_eq!(f32::from(card.bounds().width), 360.0);
    assert_eq!(f32::from(card.bounds().height), 160.0);

    // It wraps (many rows, break opportunities between graphemes) and scrolls past its maximum.
    assert!(card.display_text().contains('\u{200B}'));
    assert!(
        f32::from(card.content_height()) > 160.0 + 14.0,
        "{:?}",
        card.content_height()
    );
    assert!(card.scrolls());

    // Pointing at the card keeps it open; a wheel over it scrolls its content instead of
    // dismissing it.
    let center = bounds.center();
    let (cx_, cy_) = (
        f64::from(f32::from(center.x)),
        f64::from(f32::from(center.y)),
    );
    host.mouse_move(cx_, cy_, None, "").unwrap();
    host.flush();
    host.advance_executor_time(300.).unwrap();
    host.flush();
    let shown = |host: &mut TestHost| {
        let cards = cards.clone();
        host.app().update(|cx| cards.read(cx).shown().copied())
    };
    assert_eq!(shown(&mut host), Some(0));
    let scale = host.scale_factor();
    let region = (
        (left * scale) as u32,
        (top * scale) as u32,
        (f32::from(bounds.size.width) * scale) as u32,
        (f32::from(bounds.size.height) * scale) as u32,
    );
    let before = host.screenshot_image().unwrap();
    host.scroll_wheel(cx_, cy_, 0.0, -60.0, "").unwrap();
    host.flush();
    assert_eq!(
        shown(&mut host),
        Some(0),
        "scrolling the card keeps it open"
    );
    let after = host.screenshot_image().unwrap();
    assert!(
        pixels_differing(&before, &after, region) > 200,
        "the card's text moved: the content scrolled"
    );

    // A drag over the card selects a contiguous piece of the destination without the inserted
    // zero-width spaces.
    host.scroll_wheel(cx_, cy_, 0.0, 600.0, "").unwrap();
    host.flush();
    host.mouse_down(f64::from(left + 7.5), f64::from(top + 6.), 0, "")
        .unwrap();
    host.paint_frame();
    host.mouse_move(f64::from(right - 12.), f64::from(top + 30.), Some(0), "")
        .unwrap();
    host.paint_frame();
    host.mouse_up(f64::from(right - 12.), f64::from(top + 30.), 0, "")
        .unwrap();
    host.paint_frame();
    let dragged = service.selected_text().expect("a selection");
    assert!(!dragged.contains('\u{200B}'));
    assert!(destination.contains(&dragged), "{dragged:?}");
    assert!(dragged.chars().count() > 20);

    // Selecting the whole card and copying yields the exact destination.
    card.select_all(&service);
    host.invalidate();
    for _ in 0..3 {
        host.paint_frame();
    }
    assert_eq!(
        service.selected_text().as_deref(),
        Some(destination.as_str())
    );
    // Pressing on the occluding card moved focus off the page (nothing under the pointer was
    // focusable); the owner's focus root takes it back before the copy chord.
    let window = host.window();
    host.app()
        .update_window(window, |_, window, cx| {
            let focus = page.read(cx).focus.clone();
            window.focus(&focus, cx);
        })
        .unwrap();
    host.key_down("ctrl-c", false, None).unwrap();
    let copied = host
        .app()
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some(destination.as_str()));
    assert_eq!(card.copied_text(), destination);

    // The card is text only: the painted run is the display string, and it has no link
    // behavior (the card never opens or fetches its destination).
    assert!(
        host.painted_text()
            .iter()
            .any(|text| text == card.display_text()),
        "{:?}",
        host.painted_text()
    );

    // Leaving the card closes it after the 80 ms grace.
    host.mouse_move(40., 440., None, "").unwrap();
    host.flush();
    host.advance_executor_time(80.).unwrap();
    host.flush();
    assert_eq!(shown(&mut host), None);
}
