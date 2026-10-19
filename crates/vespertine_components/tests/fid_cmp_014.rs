//! FID-CMP-014: the code block host scene against its reference image.

mod docs_support;

use vespertine::{AnyElement, IntoElement, ParentElement, Styled, div, px, rgb};
use vespertine_components::code::CodeBlock;
use vespertine_richtext::text_root;
use vespertine_test_host::{HostOptions, TestHost, require_backend};
use vespertine_theme::document::DocumentTheme;

fn scene() -> AnyElement {
    let mut theme = DocumentTheme::dark();
    theme.text = rgb(0xeeeeee);
    theme.font_mono = "Liberation Mono".into();
    let block = CodeBlock::new("code", "alpha\nbeta\ngamma")
        .theme(theme)
        .show_line_numbers(true)
        .font_family("Liberation Mono")
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(rgb(0xeeeeee));
    text_root(
        div().size_full().relative().bg(rgb(0x181818)).child(
            div()
                .absolute()
                .left(px(20.))
                .top(px(20.))
                .w(px(560.))
                .h(px(144.))
                .p(px(12.))
                .rounded(px(8.))
                .bg(rgb(0x202020))
                .child(block),
        ),
    )
    .into_any_element()
}

#[test]
fn fid_cmp_014_matches_the_reference_within_half_a_percent() {
    require_backend!();
    let mut host =
        TestHost::with_element(HostOptions::with_size(600., 220.), |_, _| scene()).unwrap();
    host.paint_frame();
    let image = host.screenshot_image().unwrap();
    docs_support::save_capture(&image, "FID-CMP-014");
    let Some(fraction) = docs_support::fidelity(&image, "FID-CMP-014") else {
        eprintln!("skipping: no reference image");
        return;
    };
    assert!(
        fraction <= 0.005,
        "{:.3}% of pixels differ",
        fraction * 100.
    );
}
