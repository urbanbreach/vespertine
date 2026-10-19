//! Window-level tests of the rounded clip scope, its wrapper and the opt-in curved overflow
//! clip (RND-002): A3, A8, A9 (scene structure), A10 (pure), A11 (retained replay) and the
//! R1, R9, R11 and R16 behaviors, through real element trees and the paint-scope code that
//! stamps every primitive.

use crate::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, CurvedClipShape, Entity,
    HitboxBehavior, InteractiveElement as _, IntoElement, ParentElement, Pixels, Render,
    ScaledPixels, Scene, ScopeParams, ScrollHandle, StatefulInteractiveElement as _,
    StyleRefinement, Styled, TestAppContext, Window, canvas, div, fill, linear_color_stop,
    linear_gradient, point, px, rgb, rounded_clip, scene::PaintOperation, size, white,
};
use std::{cell::Cell, cell::RefCell, rc::Rc};

/// Renders whatever element `build` returns, once per frame.
struct ElementView {
    build: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl Render for ElementView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

fn open(
    cx: &mut TestAppContext,
    build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> AnyWindowHandle {
    let build: Rc<dyn Fn(&mut Window, &mut App) -> AnyElement> = Rc::new(build);
    cx.add_window(move |_, _| ElementView { build }).into()
}

/// Draws a frame and runs `read` on the scene that frame produced.
fn with_scene<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    read: impl FnOnce(&Scene) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        read(&window.rendered_frame.scene)
    })
    .expect("draw")
}

fn logical(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

fn device(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

fn gradient_fill() -> crate::Background {
    linear_gradient(
        180.0,
        linear_color_stop(white(), 0.0),
        linear_color_stop(white(), 1.0),
    )
}

// ---- WebView hook: surfaces keep only the rectangular mask under an explicit scope ----

#[vespertine::test]
fn rounded_clip_active_reports_an_enclosing_explicit_scope(cx: &mut TestAppContext) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let window = open(cx, {
        let seen = seen.clone();
        move |_, _| {
            let seen = seen.clone();
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let mut seen = seen.borrow_mut();
                    seen.push(window.rounded_clip_active());
                    window.with_rounded_clip(logical(10.0, 10.0, 40.0, 40.0), px(8.0), |window| {
                        seen.push(window.rounded_clip_active());
                    });
                    // An empty scope has no rounded term.
                    window.with_rounded_clip(logical(10.0, 10.0, 0.0, 40.0), px(8.0), |window| {
                        seen.push(window.rounded_clip_active());
                    });
                    seen.push(window.rounded_clip_active());
                },
            )
            .size_full()
            .into_any_element()
        }
    });
    with_scene(cx, window, |_| ());
    // The window may paint more than one frame; every paint reports the same sequence.
    let seen = seen.borrow();
    assert!(!seen.is_empty() && seen.len() % 4 == 0);
    for paint in seen.chunks(4) {
        assert_eq!(paint, [false, true, false, false]);
    }
}

// ---- A3, R1, R8: nesting, rectangular intersection and exact restoration ----

#[vespertine::test]
fn nested_rounded_clips_replace_geometry_intersect_masks_and_restore_exactly(
    cx: &mut TestAppContext,
) {
    let results = Rc::new(RefCell::new(Vec::new()));
    let window = open(cx, {
        let results = results.clone();
        move |_, _| {
            let results = results.clone();
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let all = logical(0.0, 0.0, 300.0, 200.0);
                    let paint = |window: &mut Window| window.paint_quad(fill(all, gradient_fill()));
                    paint(window);
                    let value = window.with_rounded_clip(
                        logical(10.0, 10.0, 100.0, 80.0),
                        px(16.0),
                        |window| {
                            paint(window);
                            let inner = window.with_rounded_clip(
                                logical(20.0, 30.0, 60.0, 40.0),
                                px(8.0),
                                |window| {
                                    paint(window);
                                    7
                                },
                            );
                            paint(window);
                            // An inner scope that sticks out of the outer one on the right.
                            let overhang = window.with_rounded_clip(
                                logical(80.0, 50.0, 100.0, 60.0),
                                px(10.0),
                                |window| {
                                    paint(window);
                                    11
                                },
                            );
                            inner + overhang
                        },
                    );
                    paint(window);
                    results.borrow_mut().push(value);
                },
            )
            .size_full()
            .into_any_element()
        }
    });
    let quads = with_scene(cx, window, |scene| scene.quads.clone());
    // The closure's result is returned unchanged.
    // (The window drew once when it opened, and once more here.)
    assert!(!results.borrow().is_empty() && results.borrow().iter().all(|value| *value == 18));
    // Painted back to front over identical bounds, so scene order is paint order.
    assert_eq!(quads.len(), 6);

    // Scale factor 2 of the test window: logical (10, 10, 100 x 80) is (20, 20, 200 x 160).
    let outer_scope = [20.0, 20.0, 220.0, 180.0];
    let (before, in_outer, in_inner, after_inner, in_overhang, after_outer) =
        (quads[0], quads[1], quads[2], quads[3], quads[4], quads[5]);

    // Before any scope: neutral parameters.
    assert_eq!(before.scope, ScopeParams::default());
    // Outer scope: its geometry (unsnapped, scale factor applied) and its mask.
    assert_eq!(in_outer.scope.clip_bounds, outer_scope);
    assert_eq!(in_outer.scope.clip_radius, 32.0);
    assert_eq!(
        in_outer.content_mask.bounds,
        device(20.0, 20.0, 200.0, 160.0)
    );
    // Inner scope: only the inner geometry applies (the outer one is suspended), and the
    // rectangular mask is the intersection of both rectangles.
    assert_eq!(in_inner.scope.clip_bounds, [40.0, 60.0, 160.0, 140.0]);
    assert_eq!(in_inner.scope.clip_radius, 16.0);
    assert_eq!(
        in_inner.content_mask.bounds,
        device(40.0, 60.0, 120.0, 80.0)
    );
    // Closure return restores the outer geometry and the outer mask exactly.
    assert_eq!(after_inner.scope, in_outer.scope);
    assert_eq!(after_inner.content_mask, in_outer.content_mask);
    // An inner scope that sticks out: its own full geometry, the intersected mask.
    assert_eq!(in_overhang.scope.clip_bounds, [160.0, 100.0, 360.0, 220.0]);
    assert_eq!(in_overhang.scope.clip_radius, 20.0);
    assert_eq!(
        in_overhang.content_mask.bounds,
        device(160.0, 100.0, 60.0, 80.0)
    );
    // Leaving the outer scope restores the original state.
    assert_eq!(after_outer.scope, before.scope);
    assert_eq!(after_outer.content_mask, before.content_mask);
    // Fade fields are never touched by the rounded scope.
    assert_eq!(in_outer.scope.fade_bands, [0.0; 4]);
}

#[vespertine::test]
fn rounded_clip_is_usable_in_prepaint_and_adds_no_primitives_or_layers(cx: &mut TestAppContext) {
    let hitbox = Rc::new(RefCell::new(None));
    let scoped = open(cx, {
        let hitbox = hitbox.clone();
        move |_, _| {
            let hitbox = hitbox.clone();
            canvas(
                move |_, window, _| {
                    // Prepaint: the hitbox is clipped by the scope's rectangular mask.
                    let inserted = window.with_rounded_clip(
                        logical(20.0, 20.0, 80.0, 40.0),
                        px(12.0),
                        |window| {
                            window.insert_hitbox(
                                logical(0.0, 0.0, 200.0, 200.0),
                                HitboxBehavior::Normal,
                            )
                        },
                    );
                    *hitbox.borrow_mut() = Some(inserted);
                },
                |_, _, window, _| {
                    for index in 0..3 {
                        let area = logical(index as f32 * 10.0, 0.0, 200.0, 100.0);
                        window.with_rounded_clip(
                            logical(5.0, 5.0, 150.0, 90.0),
                            px(10.0),
                            |window| {
                                window.paint_quad(fill(area, gradient_fill()));
                            },
                        );
                    }
                },
            )
            .size_full()
            .into_any_element()
        }
    });
    let plain = open(cx, |_, _| {
        canvas(
            |_, _, _| (),
            |_, _, window, _| {
                for index in 0..3 {
                    let area = logical(index as f32 * 10.0, 0.0, 200.0, 100.0);
                    window.paint_quad(fill(area, gradient_fill()));
                }
            },
        )
        .size_full()
        .into_any_element()
    });
    let operations = |scene: &Scene| {
        scene
            .paint_operations
            .iter()
            .map(|operation| match operation {
                PaintOperation::Primitive(_) => "primitive",
                PaintOperation::StartLayer(_) | PaintOperation::EndLayer => "layer",
                PaintOperation::BackdropBlur(_) => "blur",
                PaintOperation::PushCurvedClip(_) | PaintOperation::PopCurvedClip => "curved",
            })
            .collect::<Vec<_>>()
    };
    let scoped_operations = with_scene(cx, scoped, |scene| {
        assert!(
            scene.curved_clips.is_empty(),
            "no curved-clip data for an explicit scope"
        );
        operations(scene)
    });
    let plain_operations = with_scene(cx, plain, operations);
    // The same primitives as without the scope: no corner covers, layers or surfaces.
    assert_eq!(scoped_operations, plain_operations);
    assert_eq!(scoped_operations, vec!["primitive"; 3]);
    let hitbox = hitbox
        .borrow()
        .clone()
        .expect("hitbox inserted in prepaint");
    assert_eq!(hitbox.content_mask.bounds, logical(20.0, 20.0, 80.0, 40.0));
}

// ---- A8: the wrapper is layout-transparent and clips hit testing like paint ----

fn flex_row(wrapped: bool) -> impl Fn(&mut Window, &mut App) -> AnyElement {
    move |_, _| {
        let item = || {
            div()
                .flex_1()
                .h(px(30.0))
                .debug_selector(|| "item".to_string())
        };
        let item: AnyElement = if wrapped {
            rounded_clip(px(8.0), item()).into_any_element()
        } else {
            item().into_any_element()
        };
        div()
            .size_full()
            .child(
                div()
                    .flex()
                    .w(px(300.0))
                    .h(px(60.0))
                    .child(
                        div()
                            .w(px(50.0))
                            .h(px(60.0))
                            .debug_selector(|| "left".to_string()),
                    )
                    .child(item)
                    .child(
                        div()
                            .w(px(70.0))
                            .h(px(60.0))
                            .debug_selector(|| "right".to_string()),
                    ),
            )
            .into_any_element()
    }
}

#[vespertine::test]
fn wrapping_a_flex_item_leaves_its_measured_size_unchanged(cx: &mut TestAppContext) {
    let mut measured = Vec::new();
    for wrapped in [false, true] {
        let window = open(cx, flex_row(wrapped));
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw");
        let bounds: Vec<_> = ["left", "item", "right"]
            .into_iter()
            .map(|selector| {
                cx.update_window(window, |_, window, _| {
                    window.rendered_frame.debug_bounds.get(selector).copied()
                })
                .expect("window")
                .unwrap_or_else(|| panic!("no bounds for {selector}"))
            })
            .collect();
        measured.push(bounds);
    }
    assert_eq!(measured[0], measured[1]);
    // The flexible item really grew: 300 - 50 - 70.
    assert_eq!(measured[0][1].size.width, px(180.0));
}

#[vespertine::test]
fn wrapping_a_scroll_container_child_leaves_scroll_bounds_unchanged(cx: &mut TestAppContext) {
    let mut max_offsets = Vec::new();
    for wrapped in [false, true] {
        let handle = ScrollHandle::new();
        let window = open(cx, {
            let handle = handle.clone();
            move |_, _| {
                let content = div().w(px(180.0)).h(px(320.0));
                let content: AnyElement = if wrapped {
                    rounded_clip(px(12.0), content).into_any_element()
                } else {
                    content.into_any_element()
                };
                div()
                    .id("scroller")
                    .w(px(200.0))
                    .h(px(100.0))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .child(content)
                    .into_any_element()
            }
        });
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        })
        .expect("draw");
        max_offsets.push((handle.max_offset(), handle.bounds()));
    }
    assert_eq!(max_offsets[0], max_offsets[1]);
    assert_eq!(max_offsets[0].0.y, px(220.0));
}

#[vespertine::test]
fn wrapper_clips_hitboxes_beyond_its_bounds_but_never_at_the_rounded_corners(
    cx: &mut TestAppContext,
) {
    let hitbox = Rc::new(RefCell::new(None));
    let window = open(cx, {
        let hitbox = hitbox.clone();
        move |_, _| {
            let hitbox = hitbox.clone();
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px(50.0))
                        .top(px(50.0))
                        .w(px(100.0))
                        .h(px(60.0))
                        .child(rounded_clip(
                            px(20.0),
                            canvas(
                                move |bounds, window, _| {
                                    // A hitbox 40 pixels larger than the wrapper on every side.
                                    let larger = Bounds {
                                        origin: bounds.origin - point(px(40.0), px(40.0)),
                                        size: bounds.size + size(px(80.0), px(80.0)),
                                    };
                                    *hitbox.borrow_mut() =
                                        Some(window.insert_hitbox(larger, HitboxBehavior::Normal));
                                },
                                |_, _, _, _| {},
                            )
                            .size_full(),
                        )),
                )
                .into_any_element()
        }
    });
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .expect("draw");
    let hitbox = hitbox.borrow().clone().expect("hitbox");
    let hovered_at = |cx: &mut TestAppContext, x: f32, y: f32| {
        cx.update_window(window, |_, window, _| {
            hitbox.is_hovered_at(point(px(x), px(y)), window)
        })
        .expect("window")
    };
    // Inside the wrapper, including the cut-away top-left corner: the rounded shape never
    // cuts hit testing (R1).
    assert!(hovered_at(cx, 100.0, 80.0));
    assert!(hovered_at(cx, 51.0, 51.0));
    assert!(hovered_at(cx, 149.0, 109.0));
    // The hitbox extends 40 pixels past the wrapper but the narrowed mask clips it.
    assert!(!hovered_at(cx, 30.0, 80.0));
    assert!(!hovered_at(cx, 100.0, 30.0));
    assert!(!hovered_at(cx, 170.0, 80.0));
    assert!(!hovered_at(cx, 100.0, 130.0));
}

// ---- A9, R11, R12: which elements opt in and the shapes they produce ----

fn container(flag: bool, overflow: &str, radius: f32) -> AnyElement {
    let mut element = div()
        .absolute()
        .left(px(20.0))
        .top(px(20.0))
        .w(px(100.0))
        .h(px(60.0))
        .rounded(px(radius));
    element = match overflow {
        "both" => element.overflow_hidden(),
        "x" => element.overflow_x_hidden(),
        "y" => element.overflow_y_hidden(),
        _ => element,
    };
    if flag {
        element = element.curved_overflow_clip();
    }
    div()
        .size_full()
        .child(element.child(div().size_full().bg(rgb(0x2c6de8))))
        .into_any_element()
}

#[vespertine::test]
fn div_opts_in_only_with_the_flag_hidden_overflow_on_both_axes_and_a_positive_radius(
    cx: &mut TestAppContext,
) {
    let shapes = |cx: &mut TestAppContext, flag: bool, overflow: &'static str, radius: f32| {
        let window = open(cx, move |_, _| container(flag, overflow, radius));
        with_scene(cx, window, |scene| scene.curved_clips.clone())
    };
    // Opted in: one shape, the padding edge (no border here) at scale factor 2.
    let on = shapes(cx, true, "both", 12.0);
    assert_eq!(
        on,
        vec![CurvedClipShape {
            bounds: [40.0, 40.0, 240.0, 160.0],
            radii_x: [24.0; 4],
            radii_y: [24.0; 4],
        }]
    );
    // Flag off: rectangular overflow masking only, no curved-clip data at all.
    assert!(shapes(cx, false, "both", 12.0).is_empty());
    // Overflow visible on either axis: not effective.
    assert!(shapes(cx, true, "x", 12.0).is_empty());
    assert!(shapes(cx, true, "y", 12.0).is_empty());
    assert!(shapes(cx, true, "visible", 12.0).is_empty());
    // No positive radius: not effective.
    assert!(shapes(cx, true, "both", 0.0).is_empty());
    // Radii clamp to the quad like the background does: 1000 becomes half the height.
    let clamped = shapes(cx, true, "both", 1000.0);
    assert_eq!(clamped[0].radii_x, [60.0; 4]);
    assert_eq!(clamped[0].radii_y, [60.0; 4]);
}

/// The FID-RND-002-4 tree with fills and shadows, so that every primitive class of the
/// containers shows where it sits relative to the clip scopes.
fn nested_containers(flag: bool) -> AnyElement {
    let outer = div()
        .absolute()
        .left(px(40.0))
        .top(px(40.0))
        .w(px(420.0))
        .h(px(280.0))
        .bg(white())
        .shadow_md()
        .rounded(px(40.0))
        .border_t(px(4.0))
        .border_r(px(12.0))
        .border_b(px(20.0))
        .border_l(px(8.0))
        .border_color(rgb(0x202020))
        .overflow_hidden();
    let inner = div()
        .absolute()
        .left(px(72.0))
        .top(px(36.0))
        .w(px(400.0))
        .h(px(230.0))
        .bg(rgb(0x2c6de8))
        .shadow_md()
        .rounded(px(12.0))
        .border(px(6.0))
        .border_color(rgb(0x163878))
        .overflow_hidden();
    let red = div()
        .absolute()
        .left(px(-26.0))
        .top(px(-26.0))
        .w(px(500.0))
        .h(px(260.0))
        .bg(rgb(0xe53a40));
    let (outer, inner) = if flag {
        (outer.curved_overflow_clip(), inner.curved_overflow_clip())
    } else {
        (outer, inner)
    };
    div()
        .size_full()
        .child(outer.child(inner.child(red)))
        .into_any_element()
}

#[vespertine::test]
fn nested_containers_produce_independent_shapes_and_keep_own_borders_outside(
    cx: &mut TestAppContext,
) {
    let window = open(cx, |_, _| nested_containers(true));
    with_scene(cx, window, |scene| {
        // R12 at scale factor 2: padding edges of the outer block (border box 80,80
        // 840 x 560, borders 4/12/20/8 logical) and the inner block (border box 240,160
        // 800 x 460, border 6).
        assert_eq!(
            scene.curved_clips,
            vec![
                CurvedClipShape {
                    bounds: [96.0, 88.0, 896.0, 600.0],
                    radii_x: [64.0, 56.0, 56.0, 64.0],
                    radii_y: [72.0, 72.0, 40.0, 40.0],
                },
                CurvedClipShape {
                    bounds: [252.0, 172.0, 1028.0, 608.0],
                    radii_x: [12.0; 4],
                    radii_y: [12.0; 4],
                },
            ]
        );
        // The inner shape hangs below the outer one in the chain; the outer has no parent.
        assert_eq!(scene.curved_clip_parents, vec![0, 1]);
        let chain_of = |bounds: Bounds<ScaledPixels>| {
            scene
                .quads
                .iter()
                .filter(|quad| quad.bounds == bounds)
                .map(|quad| quad.scope.curved_clip)
                .collect::<Vec<_>>()
        };
        // The outer block's own fill and border (80,80 840 x 560) stay outside every shape.
        // (A transparent-fill border quad is painted as four strips with the same bounds.)
        let outer_chains = chain_of(device(80.0, 80.0, 840.0, 560.0));
        assert!(outer_chains.len() >= 2 && outer_chains.iter().all(|chain| *chain == 0));
        // The inner block's fill and border are descendants of the outer block.
        let inner_chains = chain_of(device(240.0, 160.0, 800.0, 460.0));
        assert!(inner_chains.len() >= 2 && inner_chains.iter().all(|chain| *chain == 1));
        // The red rectangle (200,120 1000 x 520) is under both shapes.
        assert_eq!(chain_of(device(200.0, 120.0, 1000.0, 520.0)), vec![2]);
        // Drop shadows: the outer block's outside, the inner block's under the outer shape.
        let shadow_chains: Vec<_> = scene
            .shadows
            .iter()
            .map(|shadow| (shadow.element_bounds, shadow.curved_clip))
            .collect();
        assert!(shadow_chains.contains(&(device(80.0, 80.0, 840.0, 560.0), 0)));
        assert!(shadow_chains.contains(&(device(240.0, 160.0, 800.0, 460.0), 1)));
    });

    // Flag off: no curved-clip data, every primitive on chain 0.
    let window = open(cx, |_, _| nested_containers(false));
    with_scene(cx, window, |scene| {
        assert!(scene.curved_clips.is_empty() && scene.curved_clip_parents.is_empty());
        assert!(scene.quads.iter().all(|quad| quad.scope.curved_clip == 0));
        assert!(scene.shadows.iter().all(|shadow| shadow.curved_clip == 0));
    });
}

// ---- A11: retained replay through Window::reuse_paint ----

struct CachedChild {
    renders: Rc<Cell<usize>>,
}

impl Render for CachedChild {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        nested_containers(true)
    }
}

struct CachedRoot {
    child: Entity<CachedChild>,
}

impl Render for CachedRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.child
            .clone()
            .cached(StyleRefinement::default().size_full())
    }
}

#[vespertine::test]
fn retained_frames_replay_curved_clip_shapes_and_chains_identically(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let renders = renders.clone();
        move |_, cx| CachedRoot {
            child: cx.new(|_| CachedChild { renders }),
        }
    });
    let window: AnyWindowHandle = window.into();
    let snapshot = |scene: &Scene| {
        (
            scene.curved_clips.clone(),
            scene.curved_clip_parents.clone(),
            scene
                .quads
                .iter()
                .map(|quad| (quad.bounds, quad.scope.curved_clip))
                .collect::<Vec<_>>(),
            scene
                .shadows
                .iter()
                .map(|shadow| (shadow.element_bounds, shadow.curved_clip))
                .collect::<Vec<_>>(),
        )
    };
    // The window drew at open; draws without a refresh reuse the cached subtree's paint.
    let renders_at_open = renders.get();
    let direct = with_scene(cx, window, snapshot);
    assert_eq!(direct.0.len(), 2);
    assert_eq!(renders.get(), renders_at_open);
    for _ in 0..2 {
        let replayed = with_scene(cx, window, snapshot);
        // The cached subtree was not re-rendered: its paint was replayed (`reuse_paint`).
        assert_eq!(renders.get(), renders_at_open);
        assert_eq!(replayed, direct);
    }
}

// ---- A10 (pure): coverage of one shape ----

#[test]
fn shape_of_extent_64_with_radii_24_follows_r13_at_scale_factor_one() {
    let shape = CurvedClipShape {
        bounds: [0.0, 0.0, 64.0, 64.0],
        radii_x: [24.0; 4],
        radii_y: [24.0; 4],
    };
    let tolerance = 1e-4;
    // Equal radii reproduce the exact circular distance in every corner box.
    for corner in 0..4 {
        let center = [
            if corner == 1 || corner == 2 {
                40.0f32
            } else {
                24.0
            },
            if corner >= 2 { 40.0f32 } else { 24.0 },
        ];
        let toward = [
            if corner == 1 || corner == 2 {
                1.0f32
            } else {
                -1.0
            },
            if corner >= 2 { 1.0f32 } else { -1.0 },
        ];
        for step_x in 0..48 {
            for step_y in 0..48 {
                let p = [
                    center[0] + toward[0] * (step_x as f32 * 0.5 + 0.1),
                    center[1] + toward[1] * (step_y as f32 * 0.5 + 0.1),
                ];
                let distance =
                    ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt() - 24.0;
                let expected = (0.5 - distance).clamp(0.0, 1.0);
                assert!(
                    (shape.coverage(p) - expected).abs() < tolerance,
                    "corner {corner} at {p:?}: {} vs {expected}",
                    shape.coverage(p)
                );
            }
        }
    }
    // Points just inside and outside the straight edges.
    for (inside, outside) in [
        ([32.0, 0.25], [32.0, -0.75]),
        ([32.0, 63.75], [32.0, 64.75]),
        ([0.25, 32.0], [-0.75, 32.0]),
        ([63.75, 32.0], [64.75, 32.0]),
    ] {
        assert!(shape.coverage(inside) > 0.7);
        assert_eq!(shape.coverage(outside), 0.0);
    }
    // Along the arc: 0.5 on the arc, a linear one pixel ramp either side.
    let arc = |offset: f32| {
        let along = 24.0 - (24.0 + offset) * std::f32::consts::FRAC_1_SQRT_2;
        shape.coverage([along, along])
    };
    for (offset, expected) in [(-1.0, 1.0), (-0.5, 1.0), (0.0, 0.5), (0.5, 0.0), (1.0, 0.0)] {
        assert!(
            (arc(offset) - expected).abs() < tolerance,
            "offset {offset}"
        );
    }
    // An empty shape covers zero everywhere.
    for bounds in [
        [0.0; 4],
        [5.0, 5.0, 5.0, 9.0],
        [5.0, 5.0, 9.0, 5.0],
        [9.0, 5.0, 5.0, 9.0],
    ] {
        let empty = CurvedClipShape { bounds, ..shape };
        assert_eq!(empty.coverage([6.0, 6.0]), 0.0);
        assert_eq!(empty.coverage([7.0, 7.0]), 0.0);
    }
}
