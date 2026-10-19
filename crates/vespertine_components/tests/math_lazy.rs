//! CMP-012 R11, R2: a face nobody uses is never parsed or registered. This file holds a single
//! test because it observes process-wide state.

mod math_support;

use math_support::*;
use vespertine_components::math::{FaceStatus, MathFonts, MathStyle, faces, lookup};

#[test]
fn a_face_is_parsed_and_registered_only_on_first_use() {
    assert!(!faces::is_parsed(0), "nothing parsed at start");
    // The unparsed engine knows nothing yet: even the constant tables are untouched.
    assert!(lookup(r"\frac{a}{b}", false, Default::default()).is_some());
    assert!(faces::is_parsed(0), "laying a formula out parses its face");

    let style = MathStyle::new();
    let text = paragraph(&[Piece::Text("a "), Piece::Math("x^2")], &style);
    let status = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = status.clone();
    let Some(mut page) = host(200.0, 60.0, move |window, cx| {
        // Before anything of math is built, no face is registered with this application.
        if seen.get().is_none() {
            assert_eq!(MathFonts::status(cx, 0), None);
        }
        let element = paragraph_page(&text, &style, 16.0, 24.0, 180.0, None, window, cx);
        seen.set(Some(MathFonts::status(cx, 0)));
        element
    }) else {
        return;
    };
    page.host.flush();
    assert!(matches!(status.get(), Some(Some(FaceStatus::Verified(_)))));
}
