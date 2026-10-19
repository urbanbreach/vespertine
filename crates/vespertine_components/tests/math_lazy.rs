//! CMP-012 R11, R2: a face nobody uses is never parsed or registered. This file holds a single
//! test because it observes process-wide state.

mod math_support;

use math_support::*;
use vespertine_components::math::{
    FACE_COUNT, FaceStatus, MathFace, MathFonts, MathStyle, faces, lookup,
};

#[test]
fn a_face_is_parsed_and_registered_only_on_first_use() {
    for index in 0..FACE_COUNT {
        assert!(
            !faces::is_parsed(index),
            "nothing parsed at start: face {index}"
        );
    }
    // The unparsed engine knows nothing yet: even the constant tables are untouched.
    assert!(lookup(r"\frac{a}{b}", false, Default::default()).is_some());
    assert!(faces::is_parsed(0), "laying a formula out parses its face");
    for index in 1..FACE_COUNT {
        assert!(
            !faces::is_parsed(index),
            "a face nobody uses is never parsed: face {index}"
        );
    }

    // Selecting another face parses that face and no other.
    assert!(lookup(r"\frac{a}{b}+\sqrt{x^2}", false, MathFace::EULER_MATH).is_some());
    assert!(faces::is_parsed(MathFace::EULER_MATH.index()));
    for face in [
        MathFace::STIX_TWO_MATH,
        MathFace::LATIN_MODERN_MATH,
        MathFace::LIBERTINUS_MATH,
        MathFace::TEX_GYRE_PAGELLA_MATH,
    ] {
        assert!(!faces::is_parsed(face.index()), "{}", face.label());
    }

    let style = MathStyle::new();
    let text = paragraph(&[Piece::Text("a "), Piece::Math("x^2")], &style);
    let status = std::rc::Rc::new(std::cell::RefCell::new(None));
    let seen = status.clone();
    let Some(mut page) = host(200.0, 60.0, move |window, cx| {
        // Before anything of math is built, no face is registered with this application.
        if seen.borrow().is_none() {
            for index in 0..FACE_COUNT {
                assert_eq!(MathFonts::status(cx, index), None);
            }
        }
        let element = paragraph_page(&text, &style, 16.0, 24.0, 180.0, None, window, cx);
        *seen.borrow_mut() = Some(
            (0..FACE_COUNT)
                .map(|index| MathFonts::status(cx, index))
                .collect::<Vec<_>>(),
        );
        element
    }) else {
        return;
    };
    page.host.flush();
    let statuses = status.borrow_mut().take().expect("the page was built");
    assert!(matches!(statuses[0], Some(FaceStatus::Verified(_))));
    assert!(
        statuses[1..].iter().all(Option::is_none),
        "a face nobody paints is never registered: {statuses:?}"
    );
}
