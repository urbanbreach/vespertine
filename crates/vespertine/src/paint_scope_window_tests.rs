//! Window- and element-level tests of the scoped edge fade (RND-001 A4, A8 conservative
//! primitives, A9, A11). They inspect the primitives the real paint path records: each quad
//! carries the [`ScopeParams`] that were active when it was painted.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use crate::{
    Bounds, BoxShadow, Context, Corners, EdgeFade, InteractiveElement as _, IntoElement, Modifiers,
    ParentElement as _, PathBuilder, Pixels, Quad, Render, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, UnderlineStyle,
    VisualTestContext, Window, canvas, div, edge_faded, fill, hsla, overflow_ends, point, px, rgb,
    size,
};

const OUTER: EdgeFade = EdgeFade {
    bounds: Bounds {
        origin: crate::Point {
            x: px(10.3),
            y: px(20.7),
        },
        size: crate::Size {
            width: px(200.4),
            height: px(150.2),
        },
    },
    band: px(13.3),
    band_top: None,
    band_bottom: None,
    band_left: None,
    band_right: None,
    top: true,
    bottom: true,
    left: false,
    right: false,
};

/// The device-space fade fields a scope must carry for `fade`: bands for the enabled edges
/// (override or common band, at least one logical pixel) and the edge coordinates of every
/// enabled edge, multiplied by the scale factor and never rounded.
fn expected_fade(fade: Option<&EdgeFade>, scale: f32) -> ([f32; 4], [f32; 4]) {
    let Some(fade) = fade else {
        return ([0.0; 4], [0.0; 4]);
    };
    let band = |enabled: bool, own: Option<Pixels>| {
        if enabled {
            own.unwrap_or(fade.band).0.max(1.0) * scale
        } else {
            0.0
        }
    };
    let bands = [
        band(fade.top, fade.band_top),
        band(fade.bottom, fade.band_bottom),
        band(fade.left, fade.band_left),
        band(fade.right, fade.band_right),
    ];
    let edges = [
        fade.bounds.top().0 * scale,
        fade.bounds.bottom().0 * scale,
        fade.bounds.left().0 * scale,
        fade.bounds.right().0 * scale,
    ];
    (edges, bands)
}

/// Asserts that `quad` carries exactly the fade of `fade` (or none) at `scale`.
fn assert_scope(quad: &Quad, fade: Option<&EdgeFade>, scale: f32, what: &str) {
    let (edges, bands) = expected_fade(fade, scale);
    assert_eq!(
        quad.scope.fade_bands, bands,
        "{what}: bands at scale {scale}"
    );
    for edge in 0..4 {
        if bands[edge] > 0.0 {
            assert_eq!(
                quad.scope.fade_edges[edge], edges[edge],
                "{what}: edge {edge} at scale {scale} (unsnapped device coordinate)"
            );
        }
    }
    assert!(!quad.scope.rounded_clip_active(), "{what}: no rounded clip");
}

struct PaintView(Rc<dyn Fn(&mut Window)>);

impl Render for PaintView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let paint = self.0.clone();
        canvas(|_, _, _| {}, move |_, _, window, _| paint(window)).size_full()
    }
}

fn marker(window: &mut Window, index: usize) {
    let x = 10.0 + 30.0 * index as f32;
    window.paint_quad(fill(
        Bounds::new(point(px(x), px(10.0)), size(px(20.0), px(20.0))),
        rgb(0xff0000),
    ));
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

fn painted_quads(cx: &mut VisualTestContext) -> Vec<Quad> {
    cx.update(|window, _| window.painted_quads())
}

#[vespertine::test]
fn a4_nested_replacement_pass_through_and_restoration(cx: &mut TestAppContext) {
    let inner = EdgeFade {
        bounds: Bounds::new(point(px(30.1), px(40.9)), size(px(100.6), px(80.2))),
        band: px(7.7),
        band_left: Some(px(0.0)),
        top: false,
        bottom: false,
        left: true,
        right: true,
        ..OUTER
    };
    let disabled = EdgeFade {
        top: false,
        bottom: false,
        ..OUTER
    };
    let result = Rc::new(Cell::new(0));
    let (_view, cx) = cx.add_window_view({
        let result = result.clone();
        move |_, _| {
            PaintView(Rc::new(move |window| {
                marker(window, 0); // no fade
                let value = window.with_edge_fade(Some(OUTER), |window| {
                    marker(window, 1); // outer
                    window.with_edge_fade(Some(inner), |window| {
                        marker(window, 2); // inner replaces the outer entirely
                        window.with_edge_fade(Some(disabled), |window| {
                            marker(window, 3); // disabled nested: inner stays active
                        });
                        window.with_edge_fade(None, |window| {
                            marker(window, 4); // none: inner stays active
                        });
                        marker(window, 5);
                    });
                    marker(window, 6); // outer restored exactly
                    17
                });
                result.set(value);
                marker(window, 7); // restored to no fade
            }))
        }
    });
    for scale in [1.0, 1.5, 2.0] {
        cx.simulate_scale_factor_change(scale);
        draw(cx);
        let quads = painted_quads(cx);
        assert_eq!(quads.len(), 8, "scale {scale}");
        let expectations: [(Option<&EdgeFade>, &str); 8] = [
            (None, "before any fade"),
            (Some(&OUTER), "outer"),
            (Some(&inner), "active nested replacement"),
            (Some(&inner), "disabled nested pass-through"),
            (Some(&inner), "None nested pass-through"),
            (Some(&inner), "after nested closures"),
            (Some(&OUTER), "restoration of the outer fade"),
            (None, "after the outermost closure"),
        ];
        for (quad, (fade, what)) in quads.iter().zip(expectations) {
            assert_scope(quad, fade, scale, what);
        }
        // Replacement is not a merge: the inner scope has no top or bottom band even
        // though the outer fade enables them.
        assert_eq!(quads[2].scope.fade_bands[0], 0.0);
        assert_eq!(quads[2].scope.fade_bands[1], 0.0);
        // A band of 0 on an enabled edge behaves as one logical pixel.
        assert_eq!(quads[2].scope.fade_bands[2], scale);
        assert_eq!(
            result.get(),
            17,
            "with_edge_fade returns the closure's result"
        );
    }
}

#[vespertine::test]
fn fade_scope_is_neutral_without_fade_and_never_changes_mask_or_opacity(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(move |_, _| {
        PaintView(Rc::new(move |window| {
            let mask = window.content_mask();
            window.with_edge_fade(Some(OUTER), |window| {
                assert_eq!(window.content_mask(), mask, "fading is not clipping");
                window.with_element_opacity(Some(0.5), |window| marker(window, 0));
            });
            window.with_element_opacity(Some(0.5), |window| marker(window, 1));
        }))
    });
    draw(cx);
    let quads = painted_quads(cx);
    // Opacity multiplies colors exactly as before; the fade factor is an additional multiplier
    // carried in the scope, not folded into the color.
    for quad in &quads {
        assert_eq!(quad.background.solid.a, 0.5);
    }
    assert!(quads[0].scope.fade_active());
    assert!(!quads[1].scope.fade_active());
    // The rectangular content mask is identical for scoped and unscoped quads.
    assert_eq!(quads[0].content_mask, quads[1].content_mask);
}

#[vespertine::test]
fn conservative_fade_of_paths_shadows_and_decorations(cx: &mut TestAppContext) {
    // Top edge at y = 20 (band 40), bottom edge at y = 220 (band 40).
    let fade = EdgeFade {
        bounds: Bounds::new(point(px(0.0), px(20.0)), size(px(400.0), px(200.0))),
        band: px(40.0),
        top: true,
        bottom: true,
        left: false,
        right: false,
        band_top: None,
        band_bottom: None,
        band_left: None,
        band_right: None,
    };
    let (_view, cx) = cx.add_window_view(move |_, _| {
        PaintView(Rc::new(move |window| {
            let shape = |top: f32, bottom: f32| {
                let mut builder = PathBuilder::fill();
                builder.move_to(point(px(50.0), px(top)));
                builder.line_to(point(px(80.0), px(top)));
                builder.line_to(point(px(50.0), px(bottom)));
                builder.close();
                builder.build().expect("path")
            };
            let shadow = [BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 1.0),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(4.0),
                spread_radius: px(0.0),
                inset: false,
            }];
            let underline = UnderlineStyle {
                thickness: px(2.0),
                color: Some(hsla(0.0, 1.0, 0.5, 1.0)),
                wavy: false,
            };
            window.with_edge_fade(Some(fade), |window| {
                // Paths: box touching the top edge (0), half way into the band (0.25),
                // clear of both bands (1), half way into the bottom band (0.25).
                window.paint_path(shape(20.0, 60.0), rgb(0x00ff00));
                window.paint_path(shape(40.0, 80.0), rgb(0x00ff00));
                window.paint_path(shape(100.0, 120.0), rgb(0x00ff00));
                window.paint_path(shape(180.0, 200.0), rgb(0x00ff00));
                // Shadows of the element's box (before offset, spread and blur).
                for top in [20.0, 40.0, 100.0] {
                    window.paint_drop_shadows(
                        Bounds::new(point(px(100.0), px(top)), size(px(50.0), px(30.0))),
                        Corners::all(px(0.0)),
                        &shadow,
                    );
                }
                // Decorations evaluate at their unsnapped origin.
                for y in [20.0, 40.0, 100.0, 220.0] {
                    window.paint_underline(point(px(200.0), px(y)), px(30.0), &underline);
                }
                window.with_element_opacity(Some(0.5), |window| {
                    window.paint_underline(point(px(200.0), px(40.0)), px(30.0), &underline);
                });
            });
        }))
    });
    for scale in [1.0, 2.0] {
        cx.simulate_scale_factor_change(scale);
        draw(cx);
        cx.update(|window, _| {
            let scene = &window.rendered_frame.scene;
            // Primitives are identified by their position, not their index: draw order
            // within the scene is a batching detail.
            let mut paths: Vec<(f32, f32)> = scene
                .paths
                .iter()
                .map(|path| (path.bounds.top().0 / scale, path.color.solid.a))
                .collect();
            let mut shadows: Vec<(f32, f32)> = scene
                .shadows
                .iter()
                .map(|shadow| (shadow.element_bounds.top().0 / scale, shadow.color.a))
                .collect();
            let mut underlines: Vec<(f32, f32)> = scene
                .underlines
                .iter()
                .map(|underline| (underline.bounds.top().0 / scale, underline.color.a))
                .collect();
            for list in [&mut paths, &mut shadows, &mut underlines] {
                list.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)));
            }
            let expected_paths = [(20.0, 0.0), (40.0, 0.25), (100.0, 1.0), (180.0, 0.25)];
            let expected_shadows = [(20.0, 0.0), (40.0, 0.25), (100.0, 1.0)];
            let expected_underlines = [
                (20.0, 0.0),
                (40.0, 0.25),
                (40.0, 0.125),
                (100.0, 1.0),
                (220.0, 0.0),
            ];
            for (what, actual, expected) in [
                ("path", &paths[..], &expected_paths[..]),
                ("shadow", &shadows[..], &expected_shadows[..]),
                ("underline", &underlines[..], &expected_underlines[..]),
            ] {
                assert_eq!(actual.len(), expected.len(), "{what} count");
                for (actual, expected) in actual.iter().zip(expected) {
                    assert!(
                        (actual.0 - expected.0).abs() < 1e-3
                            && (actual.1 - expected.1).abs() < 1e-6,
                        "{what} at scale {scale}: {actual:?} vs {expected:?}"
                    );
                }
            }
        });
    }
}

// --- wrapper gating (A9) --- //

#[derive(Clone)]
struct ScrollFixture {
    handle: ScrollHandle,
    content: Rc<Cell<(f32, f32)>>,
    top: bool,
    bottom: bool,
    left: bool,
    right: bool,
    vertical_gate: bool,
    horizontal_gate: bool,
    predicate: Rc<Cell<Option<(bool, bool)>>>,
}

impl ScrollFixture {
    fn new(top: bool, bottom: bool, left: bool, right: bool) -> Self {
        Self {
            handle: ScrollHandle::new(),
            content: Rc::new(Cell::new((100.0, 300.0))),
            top,
            bottom,
            left,
            right,
            vertical_gate: true,
            horizontal_gate: true,
            predicate: Rc::new(Cell::new(None)),
        }
    }
}

impl Render for ScrollFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (width, height) = self.content.get();
        let mut fade = edge_faded(
            px(20.0),
            self.top,
            self.bottom,
            div()
                .id("scroller")
                .w(px(100.0))
                .h(px(100.0))
                .overflow_scroll()
                .track_scroll(&self.handle)
                .child(
                    div()
                        .flex_none()
                        .w(px(width))
                        .h(px(height))
                        .bg(rgb(0xff0000)),
                ),
        )
        .fade_left(self.left)
        .fade_right(self.right);
        if self.vertical_gate {
            fade = fade.fade_overflow_y(&self.handle);
        }
        if self.horizontal_gate {
            fade = fade.fade_overflow_x(&self.handle);
        }
        if let Some(gate) = self.predicate.get() {
            fade = fade.fade_overflow_y_with(move |_, _| gate);
        }
        div().size_full().child(fade)
    }
}

/// Device bands (top, bottom, left, right) of the last quad, the scroller's content,
/// at scale factor 2 (the test platform's default): the enabled bands are 20 × 2.
fn content_bands(cx: &mut VisualTestContext) -> [f32; 4] {
    draw(cx);
    let quads = painted_quads(cx);
    quads.last().expect("content quad").scope.fade_bands
}

const ON: f32 = 40.0;

#[vespertine::test]
fn a9_vertical_gating_follows_scroll_offset_with_one_pixel_hysteresis(cx: &mut TestAppContext) {
    let fixture = ScrollFixture::new(true, true, false, false);
    let handle = fixture.handle.clone();
    let (_view, cx) = cx.add_window_view({
        let fixture = fixture.clone();
        move |_, _| fixture
    });
    // Content 300 in a 100 box: maximum offset 200.
    let cases: [(f32, [f32; 4], &str); 7] = [
        (
            0.0,
            [0.0, ON, 0.0, 0.0],
            "offset 0: only the bottom has hidden content",
        ),
        (
            -1.0,
            [0.0, ON, 0.0, 0.0],
            "offset exactly 1.0: not beyond the hysteresis",
        ),
        (-1.5, [ON, ON, 0.0, 0.0], "offset beyond 1.0"),
        (-100.0, [ON, ON, 0.0, 0.0], "middle"),
        (
            -198.5,
            [ON, ON, 0.0, 0.0],
            "bottom more than 1.0 from the end",
        ),
        (
            -199.0,
            [ON, 0.0, 0.0, 0.0],
            "bottom exactly 1.0 from the end",
        ),
        (-200.0, [ON, 0.0, 0.0, 0.0], "at the end"),
    ];
    for (offset, expected, what) in cases {
        handle.set_offset(point(px(0.0), px(offset)));
        assert_eq!(content_bands(cx), expected, "{what}");
    }
    assert_eq!(handle.max_offset().y, px(200.0));

    // Content shrink after scrolling: the offset is clamped during the child's prepaint and
    // the gating reads the clamped state in the same frame, not the previous frame's.
    handle.set_offset(point(px(0.0), px(-150.0)));
    assert_eq!(content_bands(cx), [ON, ON, 0.0, 0.0]);
    fixture.content.set((100.0, 120.0));
    assert_eq!(
        content_bands(cx),
        [ON, 0.0, 0.0, 0.0],
        "clamped to the new end in the same frame"
    );
    assert_eq!(handle.offset().y, px(-20.0));
    // Content now fits: no overflow, no edge is enabled, and no scope is installed at all.
    fixture.content.set((100.0, 100.0));
    let bands = content_bands(cx);
    assert_eq!(bands, [0.0; 4]);
    let quads = painted_quads(cx);
    assert_eq!(
        quads.last().unwrap().scope,
        crate::ScopeParams::default(),
        "no scope when every edge is gated off"
    );
}

#[vespertine::test]
fn a9_horizontal_gating_follows_scroll_offset(cx: &mut TestAppContext) {
    let mut fixture = ScrollFixture::new(false, false, true, true);
    fixture.content.set((300.0, 100.0));
    let handle = fixture.handle.clone();
    let (_view, cx) = cx.add_window_view({
        let fixture = fixture.clone();
        move |_, _| fixture
    });
    let cases: [(f32, [f32; 4]); 6] = [
        (0.0, [0.0, 0.0, 0.0, ON]),
        (-1.0, [0.0, 0.0, 0.0, ON]),
        (-1.5, [0.0, 0.0, ON, ON]),
        (-198.5, [0.0, 0.0, ON, ON]),
        (-199.0, [0.0, 0.0, ON, 0.0]),
        (-200.0, [0.0, 0.0, ON, 0.0]),
    ];
    for (offset, expected) in cases {
        handle.set_offset(point(px(offset), px(0.0)));
        assert_eq!(content_bands(cx), expected, "x offset {offset}");
    }
    assert_eq!(handle.max_offset().x, px(200.0));
    fixture.content.set((100.0, 100.0));
    assert_eq!(
        content_bands(cx),
        [0.0; 4],
        "content shrink removes the stale fade"
    );
    assert_eq!(handle.offset().x, px(0.0));
}

#[vespertine::test]
fn a9_only_requested_and_overflowing_bands_apply(cx: &mut TestAppContext) {
    // Overflow in both axes with only top and right requested: the bottom and left bands
    // never appear even though content overflows there.
    let fixture = ScrollFixture::new(true, false, false, true);
    fixture.content.set((300.0, 300.0));
    let handle = fixture.handle.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| fixture);
    handle.set_offset(point(px(-50.0), px(-50.0)));
    assert_eq!(content_bands(cx), [ON, 0.0, 0.0, ON]);
    // Requested but not overflowing: nothing at the start of each axis for top.
    handle.set_offset(point(px(0.0), px(0.0)));
    assert_eq!(content_bands(cx), [0.0, 0.0, 0.0, ON]);
    handle.set_offset(point(px(-200.0), px(-200.0)));
    assert_eq!(content_bands(cx), [ON, 0.0, 0.0, 0.0]);
}

#[vespertine::test]
fn a9_predicate_gates_vertical_edges_by_logical_and(cx: &mut TestAppContext) {
    let mut fixture = ScrollFixture::new(true, true, false, false);
    fixture.vertical_gate = false;
    fixture.horizontal_gate = false;
    let (_view, cx) = cx.add_window_view({
        let fixture = fixture.clone();
        move |_, _| fixture
    });
    // No gating source: the enable flags apply unconditionally.
    assert_eq!(content_bands(cx), [ON, ON, 0.0, 0.0]);
    for (gate, expected) in [
        ((true, true), [ON, ON, 0.0, 0.0]),
        ((true, false), [ON, 0.0, 0.0, 0.0]),
        ((false, true), [0.0, ON, 0.0, 0.0]),
        ((false, false), [0.0; 4]),
    ] {
        fixture.predicate.set(Some(gate));
        assert_eq!(content_bands(cx), expected, "predicate {gate:?}");
    }
    // The predicate ANDs with a disabled flag.
    let mut top_only = ScrollFixture::new(true, false, false, false);
    top_only.vertical_gate = false;
    top_only.horizontal_gate = false;
    top_only.predicate.set(Some((true, true)));
    let (_view, cx2) = cx.add_window_view(move |_, _| top_only);
    assert_eq!(content_bands(cx2), [ON, 0.0, 0.0, 0.0]);
}

#[vespertine::test]
fn a9_gated_off_wrapper_keeps_the_outer_fade_and_active_wrapper_replaces_it(
    cx: &mut TestAppContext,
) {
    struct Nested {
        inner_enabled: bool,
    }
    impl Render for Nested {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let inner = edge_faded(
                px(8.0),
                self.inner_enabled,
                false,
                div().w(px(60.0)).h(px(60.0)).bg(rgb(0x00ff00)),
            )
            .fade_overflow_y_with(move |_, _| (false, false));
            let inner = if self.inner_enabled {
                // An enabled, ungated wrapper replaces the outer fade.
                edge_faded(
                    px(8.0),
                    true,
                    false,
                    div().w(px(60.0)).h(px(60.0)).bg(rgb(0x00ff00)),
                )
                .into_any_element()
            } else {
                inner.into_any_element()
            };
            div().size_full().child(edge_faded(
                px(10.0),
                true,
                true,
                div().w(px(100.0)).h(px(100.0)).child(inner),
            ))
        }
    }

    for inner_enabled in [false, true] {
        let (_view, cx) = cx.add_window_view(move |_, _| Nested { inner_enabled });
        draw(cx);
        let quad = *painted_quads(cx).last().unwrap();
        if inner_enabled {
            // Inner top only, 8 px band, replaces the outer top+bottom fade.
            assert_eq!(quad.scope.fade_bands, [16.0, 0.0, 0.0, 0.0]);
        } else {
            // Gated off: the outer top + bottom fade (10 px band) is still active.
            assert_eq!(quad.scope.fade_bands, [20.0, 20.0, 0.0, 0.0]);
        }
    }
}

#[test]
fn overflow_ends_hysteresis() {
    assert_eq!(overflow_ends(px(0.0), px(50.0)), (false, true));
    assert_eq!(overflow_ends(px(1.0), px(50.0)), (false, true));
    assert_eq!(overflow_ends(px(1.01), px(50.0)), (true, true));
    assert_eq!(overflow_ends(px(49.0), px(50.0)), (true, false));
    assert_eq!(overflow_ends(px(48.99), px(50.0)), (true, true));
    assert_eq!(overflow_ends(px(0.0), px(0.0)), (false, false));
}

// --- layout and hit testing (A11) --- //

struct HitFixture {
    wrapped: bool,
    clicks: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for HitFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        let content = div()
            .debug_selector(|| "content".to_string())
            .flex()
            .flex_col()
            .w(px(200.0))
            .h(px(120.0))
            .child(
                div()
                    .id("top")
                    .debug_selector(|| "top".to_string())
                    .h(px(30.0))
                    .w_full()
                    .bg(rgb(0x3060c0))
                    .on_click({
                        let clicks = clicks.clone();
                        move |_, _, _| clicks.borrow_mut().push("top")
                    }),
            )
            .child(
                div()
                    .id("rest")
                    .debug_selector(|| "rest".to_string())
                    .flex_1()
                    .w_full()
                    .bg(rgb(0x60c030))
                    .on_click(move |_, _, _| clicks.borrow_mut().push("rest")),
            );
        if self.wrapped {
            div()
                .size_full()
                .child(
                    edge_faded(px(40.0), true, true, content)
                        .fade_left(true)
                        .fade_right(true),
                )
                .into_any_element()
        } else {
            div().size_full().child(content).into_any_element()
        }
    }
}

#[vespertine::test]
fn a11_layout_and_hit_testing_are_unchanged_by_the_scope(cx: &mut TestAppContext) {
    let mut layouts = Vec::new();
    let mut hitboxes = Vec::new();
    let mut clicks_seen = Vec::new();
    for wrapped in [false, true] {
        let clicks = Rc::new(RefCell::new(Vec::new()));
        let (_view, cx) = cx.add_window_view({
            let clicks = clicks.clone();
            move |_, _| HitFixture { wrapped, clicks }
        });
        draw(cx);
        layouts.push(
            ["content", "top", "rest"].map(|selector| cx.debug_bounds(selector).expect(selector)),
        );
        hitboxes.push(cx.update(|window, _| {
            window
                .rendered_frame
                .hitboxes
                .iter()
                .map(|hitbox| (hitbox.bounds, hitbox.content_mask, hitbox.behavior))
                .collect::<Vec<_>>()
        }));
        // Points deep inside the fade band (the content is nearly transparent there) and in
        // the opaque centre both reach their handlers.
        for position in [
            point(px(100.0), px(1.0)),
            point(px(2.0), px(100.0)),
            point(px(100.0), px(60.0)),
        ] {
            cx.simulate_click(position, Modifiers::default());
        }
        clicks_seen.push(clicks.borrow().clone());
    }
    assert_eq!(layouts[0], layouts[1], "layout of the scoped subtree");
    assert_eq!(
        hitboxes[0], hitboxes[1],
        "hitboxes (bounds, mask, behavior)"
    );
    assert!(!hitboxes[0].is_empty());
    assert_eq!(clicks_seen[0], clicks_seen[1], "click routing");
    assert_eq!(clicks_seen[1], vec!["top", "rest", "rest"]);
}
