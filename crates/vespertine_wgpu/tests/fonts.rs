//! Runtime font registration (TXT-003) and typography roles (TXT-004) against the real Linux text
//! backend. The same scenarios back `examples/registered_font.rs` and `examples/typography.rs`.

use std::sync::Arc;
use vespertine::{FontCatalogSource, HeadlessAppContext};
use vespertine_wgpu::CosmicTextSystem;

#[path = "support/registered_font_scenario.rs"]
mod registered_font_scenario;
#[path = "support/typography_scenario.rs"]
mod typography_scenario;

/// TXT-003 A2.
#[test]
fn registered_font_width_follows_registration() {
    registered_font_scenario::run();
}

/// TXT-004 A1.
#[test]
fn interface_size_scales_only_the_interface_sample() {
    typography_scenario::run();
}

/// TXT-004 R3: fixed width is decided by measuring advances, on the installed system fonts.
#[test]
fn installed_system_fonts_are_classified_by_measured_advances() {
    let cx = HeadlessAppContext::new(Arc::new(CosmicTextSystem::new("Noto Sans")));
    let text_system = cx.text_system();
    let mut checked = 0;
    for (family, fixed) in [("Liberation Mono", true), ("Noto Sans", false)] {
        let faces = text_system.family_faces(family);
        if faces.is_empty() {
            // The font is not installed on this machine.
            continue;
        }
        checked += 1;
        assert_eq!(vespertine::faces_are_fixed_width(&faces), fixed, "{family}");
    }
    eprintln!("classified {checked} installed families");
}
