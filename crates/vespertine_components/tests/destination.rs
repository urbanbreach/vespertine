//! CMP-004 A7: the link destination card.

use std::{cell::RefCell, rc::Rc};
use vespertine::AppContext as _;
use vespertine::{
    Context, IntoElement, Render, Size, Styled, TestAppContext, Window, div, px, size,
};
use vespertine_components::tokens::ComponentTokens;
use vespertine_components::tooltip::destination::{DestinationCard, card_limits, destination_card};
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
