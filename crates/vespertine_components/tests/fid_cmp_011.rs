//! FID-CMP-011: the Markdown document scene against its reference image.

mod docs_support;

use vespertine::{AnyElement, IntoElement, ParentElement, Styled, div, px, rgb};
use vespertine_components::markdown_doc::Markdown;
use vespertine_richtext::text_root;
use vespertine_test_host::{HostOptions, TestHost, require_backend};
use vespertine_theme::document::DocumentTheme;

fn scene() -> AnyElement {
    let mut theme = DocumentTheme::dark();
    theme.text = rgb(0xeeeeee);
    theme.accent = rgb(0x8080ff);
    theme.border = vespertine::Rgba {
        r: 1.,
        g: 1.,
        b: 1.,
        a: 0x14 as f32 / 255.,
    };
    theme.font_sans = "Noto Sans".into();
    theme.font_mono = "Liberation Mono".into();
    text_root(
        div().size_full().relative().bg(rgb(0x181818)).child(
            div()
                .absolute()
                .left(px(20.))
                .top(px(20.))
                .w(px(600.))
                .child(
                    Markdown::new("doc", "# Document\n\nA short paragraph.\n\n> Quoted text\n")
                        .theme(theme),
                ),
        ),
    )
    .into_any_element()
}

#[test]
fn fid_cmp_011_matches_the_reference_within_half_a_percent() {
    require_backend!();
    let mut host =
        TestHost::with_element(HostOptions::with_size(640., 460.), |_, _| scene()).unwrap();
    host.paint_frame();
    let image = host.screenshot_image().unwrap();
    docs_support::save_capture(&image, "FID-CMP-011");
    let Some(fraction) = docs_support::fidelity(&image, "FID-CMP-011") else {
        eprintln!("skipping: no reference image");
        return;
    };
    assert!(
        fraction <= 0.005,
        "{:.3}% of pixels differ",
        fraction * 100.
    );
}
