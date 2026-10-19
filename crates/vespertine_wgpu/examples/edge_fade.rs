//! RND-001 A3: measured alpha of scoped edge fades against the specification's fragment
//! factor, through the real element path (the `edge_faded` wrapper), the real renderer and
//! an offscreen target, at scale factors 1 and 2.
//!
//! A white box over a black window fades toward the enabled edges of its own bounds, so the
//! red channel of every pixel is the measured edge coverage E. Every pixel of the window is
//! compared with R6: u = min(1, active ramps), E = u², with each device band the effective
//! logical band (an enabled edge's band raised to at least 1 logical pixel) times the scale;
//! disabled edges never attenuate. Prints a table and exits non-zero on any mismatch.
//!
//! Run with: `cargo run -p vespertine_wgpu --features test-support --example edge_fade`

#[path = "edge_fade_support/mod.rs"]
mod support;

use support::{DeviceFade, Host};
use vespertine::{IntoElement as _, ParentElement as _, Styled as _, div, edge_faded, px, white};

const WINDOW: (f32, f32) = (200.0, 160.0);
/// Fade region in logical pixels (integral so device edges sit on pixel boundaries).
const REGION: (f32, f32, f32, f32) = (20.0, 20.0, 160.0, 120.0);
/// Largest accepted deviation, in 8-bit steps: one blend and one rounding.
const TOLERANCE_STEPS: f32 = 3.0;

struct Case {
    name: &'static str,
    /// Enabled edges: top, bottom, left, right.
    enabled: [bool; 4],
    band: f32,
    /// Per-edge overrides: top, bottom, left, right.
    overrides: [Option<f32>; 4],
}

fn cases() -> Vec<Case> {
    let none = [None; 4];
    vec![
        Case {
            name: "top only, band 20",
            enabled: [true, false, false, false],
            band: 20.0,
            overrides: none,
        },
        Case {
            name: "bottom only, band 20",
            enabled: [false, true, false, false],
            band: 20.0,
            overrides: none,
        },
        Case {
            name: "left only, band 20",
            enabled: [false, false, true, false],
            band: 20.0,
            overrides: none,
        },
        Case {
            name: "right only, band 20",
            enabled: [false, false, false, true],
            band: 20.0,
            overrides: none,
        },
        Case {
            name: "asymmetric, band 12 with 30/6/-/24",
            enabled: [true, true, true, true],
            band: 12.0,
            overrides: [Some(30.0), Some(6.0), None, Some(24.0)],
        },
        Case {
            name: "four edges, band 20",
            enabled: [true, true, true, true],
            band: 20.0,
            overrides: none,
        },
        Case {
            name: "top only, band 0 (1 px)",
            enabled: [true, false, false, false],
            band: 0.0,
            overrides: none,
        },
        Case {
            name: "bottom only, band -7 (1 px)",
            enabled: [false, true, false, false],
            band: -7.0,
            overrides: none,
        },
        Case {
            name: "top+left, band 20, left override 0",
            enabled: [true, false, true, false],
            band: 20.0,
            overrides: [None, None, Some(0.0), None],
        },
        Case {
            name: "four edges, band -3 (1 px)",
            enabled: [true, true, true, true],
            band: -3.0,
            overrides: none,
        },
        Case {
            name: "top only, band 20, disabled overrides ignored",
            enabled: [true, false, false, false],
            band: 20.0,
            overrides: [None, Some(50.0), Some(50.0), Some(50.0)],
        },
    ]
}

fn main() {
    let mut host = Host::new();
    let mut failures = 0;
    println!(
        "{:<48} {:>5} {:>10} {:>12} {:>7}",
        "case", "scale", "max error", "partial px", "result"
    );
    for scale in [1.0f32, 2.0] {
        for case in cases() {
            let image = host.render(WINDOW.0, WINDOW.1, scale, {
                let enabled = case.enabled;
                let overrides = case.overrides;
                let band = case.band;
                move |_, _| {
                    let mut fade = edge_faded(
                        px(band),
                        enabled[0],
                        enabled[1],
                        div().size_full().bg(white()),
                    )
                    .fade_left(enabled[2])
                    .fade_right(enabled[3]);
                    if let Some(value) = overrides[0] {
                        fade = fade.band_top(px(value));
                    }
                    if let Some(value) = overrides[1] {
                        fade = fade.band_bottom(px(value));
                    }
                    if let Some(value) = overrides[2] {
                        fade = fade.band_left(px(value));
                    }
                    if let Some(value) = overrides[3] {
                        fade = fade.band_right(px(value));
                    }
                    div()
                        .size_full()
                        .bg(vespertine::black())
                        .child(
                            div()
                                .absolute()
                                .left(px(REGION.0))
                                .top(px(REGION.1))
                                .w(px(REGION.2))
                                .h(px(REGION.3))
                                .child(fade),
                        )
                        .into_any_element()
                }
            });
            let effective = |index: usize| {
                if case.enabled[index] {
                    case.overrides[index].unwrap_or(case.band).max(1.0) * scale
                } else {
                    0.0
                }
            };
            let expected = DeviceFade {
                top: REGION.1 * scale,
                bottom: (REGION.1 + REGION.3) * scale,
                left: REGION.0 * scale,
                right: (REGION.0 + REGION.2) * scale,
                bands: [effective(0), effective(1), effective(2), effective(3)],
            };
            let (width, height) = image.dimensions();
            assert_eq!(
                (width, height),
                ((WINDOW.0 * scale) as u32, (WINDOW.1 * scale) as u32)
            );
            let mut worst = 0.0f32;
            let mut partial = 0usize;
            for y in 0..height {
                for x in 0..width {
                    let inside = (x as f32 + 0.5) > expected.left
                        && (x as f32 + 0.5) < expected.right
                        && (y as f32 + 0.5) > expected.top
                        && (y as f32 + 0.5) < expected.bottom;
                    let want = if inside { expected.coverage(x, y) } else { 0.0 };
                    let pixel = image.get_pixel(x, y).0;
                    for channel in 0..3 {
                        let got = f32::from(pixel[channel]) / 255.0;
                        worst = worst.max((got - want).abs() * 255.0);
                    }
                    if want > 0.02 && want < 0.98 {
                        partial += 1;
                    }
                }
            }
            let ok = worst <= TOLERANCE_STEPS && partial > 0;
            if !ok {
                failures += 1;
            }
            println!(
                "{:<48} {:>5} {:>8.2}/255 {:>12} {:>7}",
                case.name,
                scale,
                worst,
                partial,
                if ok { "ok" } else { "FAILED" }
            );
        }
    }
    if failures > 0 {
        eprintln!("{failures} case(s) deviate from RND-001 R6");
        std::process::exit(1);
    }
    println!("all cases match RND-001 R6");
}
