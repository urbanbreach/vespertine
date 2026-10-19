//! CMP-004 A8: the link destination card for a long Unicode destination near the right edge of
//! the window.
//!
//! The link at the right edge discloses its destination in a hover card after the 550 ms dwell. The
//! card shows the destination exactly as written (display-only zero-width break opportunities let
//! the long run of text wrap), stays inside the window, scrolls once the wrapped text is taller than
//! its 160 px maximum, never fetches or previews anything (it is plain text with no link behavior)
//! and copies the exact target: "Copy destination" puts the original string on the clipboard, and a
//! selection in the card copies the original text (the inserted U+200B never reaches the clipboard).
//!
//! Without arguments the example runs offscreen on the production test host: it hovers the link,
//! measures the card, scrolls it with the wheel, copies the destination and exits 0 when every
//! check holds (1 otherwise). With `--interactive` the same page opens in a window.

mod controls_common;

use std::{cell::RefCell, rc::Rc};

use controls_common::{Build, Checks, run_window, scripted_host};
use vespertine::{
    App, AppContext as _, ClipboardItem, Entity, InteractiveElement, IntoElement, ParentElement,
    Pixels, Size, Styled, div, px, rgb,
};
use vespertine_components::controls::Button;
use vespertine_components::tokens::ComponentTokens;
use vespertine_components::tooltip::card::HoverCardHost;
use vespertine_components::tooltip::destination::{DestinationCard, destination_card};
use vespertine_richtext::text_root;

const WIDTH: f32 = 720.0;
const HEIGHT: f32 = 480.0;
/// The link: left, top, width, height. It ends 8 px from the right edge of the window.
const LINK: (f32, f32, f32, f32) = (WIDTH - 8.0 - 150.0, 40.0, 150.0, 22.0);

/// A destination of non-Latin text, a query string and an emoji: one very long run with no natural
/// break.
fn destination() -> String {
    format!(
        "/\u{30C9}\u{30AD}\u{30E5}\u{30E1}\u{30F3}\u{30C8}/{}?\u{691C}\u{7D22}={}&e=\u{1F600}",
        "\u{9577}\u{3044}\u{30D1}\u{30B9}".repeat(60),
        "\u{30C6}\u{30B9}\u{30C8}".repeat(60),
    )
}

/// The card measured for the current window size, shared by the page and the card callback.
#[derive(Default)]
struct Measured(RefCell<Option<(Size<Pixels>, DestinationCard)>>);

impl Measured {
    fn card(
        &self,
        destination: &str,
        window: &mut vespertine::Window,
        cx: &App,
    ) -> DestinationCard {
        let viewport = window.viewport_size();
        if let Some((size, card)) = &*self.0.borrow()
            && *size == viewport
        {
            return card.clone();
        }
        let card = destination_card(destination, viewport, ComponentTokens::current(cx), window);
        *self.0.borrow_mut() = Some((viewport, card.clone()));
        card
    }
}

struct Example {
    cards: Rc<RefCell<Option<Entity<HoverCardHost<u8>>>>>,
    measured: Rc<Measured>,
}

fn build(example: &Rc<Example>) -> Build {
    let example = example.clone();
    Rc::new(move |window, cx| {
        let text = destination();
        let cards = example
            .cards
            .borrow_mut()
            .get_or_insert_with(|| {
                let measured = example.measured.clone();
                let text = text.clone();
                cx.new(|cx| {
                    HoverCardHost::new(
                        move |_: &u8, window, cx| {
                            measured
                                .card(&text, window, cx)
                                .element()
                                .into_any_element()
                        },
                        cx,
                    )
                    .interactive(true)
                    .bare(true)
                })
            })
            .clone();
        let card = example.measured.card(&text, window, cx);
        let (x, y, w, h) = LINK;
        let link = div()
            .id("destination-link")
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .text_color(rgb(0x7CB7FF))
            .underline()
            .child("the document link");
        let copy_text = card.copied_text().to_string();
        text_root(
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .absolute()
                        .left(px(24.))
                        .top(px(24.))
                        .w(px(440.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child("Hover the link at the right edge; the card opens after 550 ms.")
                        .child(format!(
                            "card: {} x {} px, wrapped text {} px tall ({})",
                            f32::from(card.bounds().width),
                            f32::from(card.bounds().height),
                            f32::from(card.content_height()),
                            if card.scrolls() {
                                "scrolls past its maximum height"
                            } else {
                                "fits"
                            },
                        ))
                        .child(format!(
                            "destination: {} characters, shown with U+200B between graphemes",
                            card.destination().chars().count(),
                        ))
                        .child("network access: none (the card is plain text, nothing is fetched)")
                        .child(
                            Button::new("copy-destination")
                                .label("Copy destination")
                                .w(px(180.))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_text.clone(),
                                    ));
                                }),
                        ),
                )
                .child(HoverCardHost::trigger(&cards, 0, link, cx))
                .child(cards),
        )
        .into_any_element()
    })
}

fn main() {
    let example = Rc::new(Example {
        cards: Rc::default(),
        measured: Rc::default(),
    });
    if std::env::args().any(|argument| argument == "--interactive") {
        run_window(WIDTH, HEIGHT, build(&example));
        return;
    }

    let mut host = match scripted_host(f64::from(WIDTH), f64::from(HEIGHT), build(&example)) {
        Ok(host) => host,
        Err(reason) => {
            println!("destination_card: no offscreen host here ({reason}); run with --interactive");
            return;
        }
    };
    let mut checks = Checks::default();
    let destination = destination();
    host.flush();
    let cards = example.cards.borrow().clone().expect("the card host");
    let shown = |host: &mut vespertine_test_host::TestHost| {
        let cards = cards.clone();
        host.app().update(|cx| cards.read(cx).shown().copied())
    };

    // Real hover events over the link open the card after the dwell.
    let (lx, ly, lw, lh) = LINK;
    host.mouse_move(f64::from(lx + lw / 2.), f64::from(ly + lh / 2.), None, "")
        .unwrap();
    host.flush();
    host.advance_executor_time(549.).unwrap();
    host.flush();
    checks.check(
        shown(&mut host).is_none(),
        "the card waits the 550 ms dwell",
    );
    host.advance_executor_time(1.).unwrap();
    host.flush();
    checks.check(
        shown(&mut host) == Some(0),
        "the card opens after the dwell",
    );
    let bounds = {
        let cards = cards.clone();
        host.app().update(|cx| cards.read(cx).card_bounds())
    }
    .expect("the card is painted");
    let card = example
        .measured
        .0
        .borrow()
        .as_ref()
        .map(|(_, card)| card.clone())
        .expect("the card was measured");
    println!(
        "   link {lx},{ly} {lw}x{lh}; card {:?} x {:?} at {:?},{:?}; content {:?}",
        bounds.size.width,
        bounds.size.height,
        bounds.origin.x,
        bounds.origin.y,
        card.content_height()
    );

    // Inside the window with the 8 px containment inset although the link touches the right edge.
    let inside = f32::from(bounds.left()) >= 8.0
        && f32::from(bounds.right()) <= WIDTH - 8.0
        && f32::from(bounds.top()) >= 8.0
        && f32::from(bounds.bottom()) <= HEIGHT - 8.0;
    checks.check(inside, "the card stays inside the window");
    checks.check(
        f32::from(bounds.left()) < lx,
        "the card shifted left of the link, which ends at the window edge",
    );
    checks.check(
        (f32::from(bounds.size.width) - f32::from(card.bounds().width)).abs() < 1.0
            && (f32::from(bounds.size.height) - f32::from(card.bounds().height)).abs() < 1.0,
        "the painted card has its measured size",
    );
    checks.check(
        card.display_text().contains('\u{200B}')
            && card.display_text().replace('\u{200B}', "") == destination
            && f32::from(card.content_height()) > 3.0 * 14.0,
        "the text wraps over many rows through the display-only break opportunities",
    );
    checks.check(
        card.scrolls() && f32::from(bounds.size.height) == 160.0,
        "the card is capped at its 160 px maximum height and scrolls beyond it",
    );

    // A wheel over the card scrolls its content and keeps it open: the painted card changes.
    let center = bounds.center();
    let (px_, py_) = (
        f64::from(f32::from(center.x)),
        f64::from(f32::from(center.y)),
    );
    host.mouse_move(px_, py_, None, "").unwrap();
    host.flush();
    host.advance_executor_time(300.).unwrap();
    host.flush();
    let scale = host.scale_factor();
    let region = (
        (f32::from(bounds.left()) * scale) as u32,
        (f32::from(bounds.top()) * scale) as u32,
        (f32::from(bounds.size.width) * scale) as u32,
        (f32::from(bounds.size.height) * scale) as u32,
    );
    let before = host.screenshot_image().unwrap();
    host.scroll_wheel(px_, py_, 0.0, -60.0, "").unwrap();
    host.flush();
    let after = host.screenshot_image().unwrap();
    let moved = (region.1..region.1 + region.3)
        .flat_map(|y| (region.0..region.0 + region.2).map(move |x| (x, y)))
        .filter(|(x, y)| before.get_pixel(*x, *y) != after.get_pixel(*x, *y))
        .count();
    checks.check(
        shown(&mut host) == Some(0) && moved > 200,
        format!(
            "a wheel over the card scrolls its text ({moved} pixels changed) and keeps it open"
        ),
    );

    // Copy the exact target.
    host.simulate_keystrokes("tab", None).unwrap();
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    host.flush();
    let copied = host
        .app()
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    checks.check(
        copied.as_deref() == Some(destination.as_str()),
        "Copy destination puts the exact original string on the clipboard",
    );
    checks.check(
        card.copied_text() == destination && !card.copied_text().contains('\u{200B}'),
        "a copy of the card's text yields the original with no inserted U+200B",
    );
    checks.finish("destination_card");
}
