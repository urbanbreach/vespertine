//! CMP-017 pixel tests on the offscreen test host: the close slot's stacked layers (R6) and the
//! overflow fade (R9).

use std::{cell::RefCell, rc::Rc};
use vespertine::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use vespertine_components::{
    nav_theme::NavTheme,
    tabs::{TabId, TabSpec, TabStrip},
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

struct StripView {
    strip: Entity<TabStrip>,
}

impl Render for StripView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(NavTheme::neutral().background)
            .child(div().w(px(300.)).child(self.strip.clone()))
    }
}

fn host(specs: Vec<TabSpec>, selected: &str) -> (TestHost, Rc<RefCell<Option<Entity<TabStrip>>>>) {
    let slot: Rc<RefCell<Option<Entity<TabStrip>>>> = Rc::default();
    let captured = slot.clone();
    let selected = TabId::from(selected);
    let mut host = TestHost::with_view(
        HostOptions {
            scale_factor: 1.0,
            ..HostOptions::with_size(300.0, 40.0)
        },
        move |_, cx| {
            let strip = cx.new(|_| {
                TabStrip::new("host")
                    .tabs(specs)
                    .selected(selected)
                    .theme(NavTheme::neutral())
            });
            *captured.borrow_mut() = Some(strip.clone());
            cx.new(|_| StripView { strip })
        },
    )
    .expect("host");
    host.flush();
    (host, slot)
}

fn pixel(host: &mut TestHost, x: u32, y: u32) -> [u8; 4] {
    host.screenshot_image()
        .expect("screenshot")
        .get_pixel(x, y)
        .0
}

fn specs(count: usize) -> Vec<TabSpec> {
    (0..count)
        .map(|ix| TabSpec::new(format!("t{ix}"), format!("Tab {ix}")))
        .collect()
}

/// Number of pixels in the box whose brightest channel is at least `threshold`.
fn bright(host: &mut TestHost, x: u32, y: u32, size: u32, threshold: u8) -> usize {
    let image = host.screenshot_image().expect("screenshot");
    let mut count = 0;
    for yy in y..y + size {
        for xx in x..x + size {
            let [r, g, b, _] = image.get_pixel(xx, yy).0;
            if r.max(g).max(b) >= threshold {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn a_dirty_tab_shows_the_dot_and_hover_swaps_it_for_the_glyph_in_the_same_slot() {
    require_backend!();
    let mut tabs = specs(2);
    tabs[0].dirty = true;
    let (mut host, _) = host(tabs, "t1");
    // Chip t0 spans x 0..112, y 0..24; its close slot is the 18 px box at x 90..108, y 3..21.
    let (slot_x, slot_y) = (90, 3);
    host.mouse_move(250.0, 35.0, None, "")
        .expect("park the pointer");
    host.flush();
    let dot_probe = pixel(&mut host, 101, 12);
    let muted = NavTheme::neutral().muted;
    let muted_rgb = [
        (muted.to_rgb().r * 255.0).round() as u8,
        (muted.to_rgb().g * 255.0).round() as u8,
        (muted.to_rgb().b * 255.0).round() as u8,
    ];
    for (got, want) in dot_probe[..3].iter().zip(muted_rgb) {
        assert!(
            got.abs_diff(want) <= 10,
            "the dot is painted in the muted colour: {dot_probe:?}"
        );
    }
    let dot_pixels = bright(&mut host, slot_x, slot_y, 18, 0x70);
    assert!(
        (20..=40).contains(&dot_pixels),
        "a 6 px disc is about 28 pixels: {dot_pixels}"
    );

    // Hovering the title (not the slot) hides the dot and shows the glyph.
    host.mouse_move(40.0, 12.0, None, "").expect("hover");
    host.flush();
    let hovered_probe = pixel(&mut host, 101, 12);
    for (got, want) in hovered_probe[..3].iter().zip(muted_rgb) {
        assert!(
            got.abs_diff(want) > 40,
            "the dot is gone while the tab is hovered: {hovered_probe:?}"
        );
    }
    let glyph_pixels = bright(&mut host, slot_x, slot_y, 18, 0x50);
    assert!(
        glyph_pixels >= 6,
        "the close glyph is drawn in the same slot: {glyph_pixels}"
    );
    // The glyph is not a disc: pixels at the dot's right edge are background.
    assert_ne!(dot_probe, hovered_probe);

    // A clean tab shows nothing in its slot until hovered.
    host.mouse_move(250.0, 30.0, None, "").expect("leave");
    host.flush();
    let clean_idle = bright(&mut host, 116 + slot_x, slot_y, 18, 0x50);
    assert_eq!(clean_idle, 0, "a clean tab has an empty slot");
    host.mouse_move(116.0 + 40.0, 12.0, None, "")
        .expect("hover");
    host.flush();
    let clean_hovered = bright(&mut host, 116 + slot_x, slot_y, 18, 0x50);
    assert!(clean_hovered >= 6, "hover shows the glyph: {clean_hovered}");
}

#[test]
fn the_edge_fade_follows_the_hidden_edges() {
    require_backend!();
    // Two tabs fit: no edge is hidden, so nothing fades.
    let (mut host_fit, _) = host(specs(2), "t0");
    let edge = pixel(&mut host_fit, 2, 12);
    let inside = pixel(&mut host_fit, 50, 12);
    assert_eq!(edge, inside, "no fade without hidden tabs");
    assert_ne!(
        edge,
        pixel(&mut host_fit, 2, 35),
        "the selected wash is painted"
    );

    // Ten tabs overflow: scrolled to the start only the end fades.
    let (mut host_scroll, strip) = host(specs(10), "t0");
    let background = pixel(&mut host_scroll, 2, 35);
    assert_eq!(
        pixel(&mut host_scroll, 2, 12),
        pixel(&mut host_scroll, 50, 12)
    );
    let strip = strip.borrow().clone().expect("strip");
    let scroll = host_scroll
        .app()
        .update(|cx| strip.read(cx).scroll().clone());
    // Select t2 so the tab under the right edge carries a wash.
    host_scroll.app().update(|cx| {
        strip.update(cx, |strip, cx| strip.select(&TabId::from("t2"), cx));
    });
    host_scroll.flush();
    host_scroll.flush();
    let offset = scroll.offset().x;
    // Revealing t2 (content 232..344) in a 300 px strip scrolls by 44 px.
    assert_eq!(offset, px(-44.), "t2 is revealed with the minimal scroll");
    let (start, end) = host_scroll.app().update(|cx| strip.read(cx).edge_hidden());
    assert!(start && end);
    // t2 now spans window x 188..300: its right 36 px fade out toward the edge.
    let solid = pixel(&mut host_scroll, 220, 12);
    let faded = pixel(&mut host_scroll, 298, 12);
    let distance = |a: [u8; 4]| -> i32 {
        a[..3]
            .iter()
            .zip(background[..3].iter())
            .map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs())
            .sum()
    };
    assert!(
        distance(solid) > 20,
        "the selected wash is visible: {solid:?}"
    );
    assert!(
        distance(faded) < distance(solid) / 2,
        "the right edge fades: {faded:?} vs {solid:?}"
    );
}
