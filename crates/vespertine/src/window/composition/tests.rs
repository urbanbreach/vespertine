//! PLT-001 tests on the layered test platform: layered draw entry (A1), surface tree and
//! platform synchronization (A2), paint stream (A3), attachments (A4), focus (A7), device
//! recreation (A8).

use std::{cell::Cell, cell::RefCell, rc::Rc};

use crate::{AppContext as _, window::FluentBuilder as _};

use crate::{
    AnyView, AnyWindowHandle, Bounds, CompositionError, Context, FileDropEvent, InputEvent as _,
    IntoElement, ParentElement as _, Pixels, Render, Scene, StyleRefinement, Styled as _,
    SurfaceId, SurfaceKind, TestAppContext, UnbalancedScopes, Window, black, blue, canvas,
    deferred, div, fill, green, point, px, red, size,
};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(h)),
    }
}

/// What a scenery view paints this frame.
#[derive(Clone, Default)]
struct Plan {
    /// Paint a deferred draw (a popover) covering part of the window.
    deferred: bool,
    /// Request a tooltip while prepainting.
    tooltip: bool,
    /// An empty layer inside the deferred draw: scope markers without primitives.
    empty_layer_overlay: bool,
}

struct Scenery {
    plan: Rc<RefCell<Plan>>,
}

struct Plain;

impl Render for Plain {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(10.)).bg(blue())
    }
}

impl Render for Scenery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan.borrow().clone();
        let tooltip_view = cx.new(|_| Plain);
        div()
            .size_full()
            .bg(red())
            .child(div().size(px(50.)).bg(blue()))
            .when(plan.tooltip, move |parent| {
                let view: AnyView = tooltip_view.into();
                parent.child(
                    canvas(
                        move |_, window, _| {
                            window.set_tooltip(crate::AnyTooltip {
                                view,
                                mouse_position: point(px(5.), px(5.)),
                                check_visible_and_update: Rc::new(|_, _, _| true),
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .size(px(1.)),
                )
            })
            .when(plan.deferred, |parent| {
                parent.child(deferred(
                    div().absolute().top(px(20.)).size(px(40.)).bg(green()),
                ))
            })
            .when(plan.empty_layer_overlay, |parent| {
                parent.child(deferred(
                    canvas(
                        |_, _, _| (),
                        |bounds, _, window, _| window.paint_layer(bounds, |_| {}),
                    )
                    .size(px(10.)),
                ))
            })
    }
}

struct Harness {
    window: AnyWindowHandle,
    plan: Rc<RefCell<Plan>>,
}

impl Harness {
    fn new(cx: &mut TestAppContext, composed: bool) -> Self {
        let plan: Rc<RefCell<Plan>> = Default::default();
        let window = cx
            .add_window({
                let plan = plan.clone();
                move |_, _| Scenery { plan }
            })
            .into();
        let harness = Harness { window, plan };
        if composed {
            harness.with(cx, |window| {
                window
                    .platform_window
                    .as_test()
                    .unwrap()
                    .allow_composition();
                window.enable_window_composition().unwrap();
            });
        }
        harness
    }

    fn with<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Window) -> R) -> R {
        cx.update_window(self.window, |_, window, _| f(window))
            .unwrap()
    }

    /// Renders and presents one frame.
    fn frame(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.present(cx);
        })
        .unwrap();
    }

    fn test<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&crate::TestWindow) -> R) -> R {
        self.with(cx, |window| f(window.platform_window.as_test().unwrap()))
    }
}

// ---- A1: layered draw entry ----

#[crate::test]
fn a1_split_index_is_where_the_overlay_starts(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.plan.borrow_mut().deferred = true;
    harness.frame(cx);
    let draws = harness.test(cx, |window| window.layered_draws());
    let draw = draws.last().unwrap();
    assert!(draw.base.drawable && draw.overlay.drawable, "{draw:?}");
    assert_eq!(draw.overlay_start, draw.base.operations);
    assert_eq!(
        draw.base.operations + draw.overlay.operations,
        draw.scene_operations
    );
    assert_eq!(draw.requested_start, draw.overlay_start);
    assert!(draw.unbalanced.is_none());
}

#[crate::test]
fn a1_frames_without_deferred_content_have_an_empty_overlay(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.frame(cx);
    let draws = harness.test(cx, |window| window.layered_draws());
    let draw = draws.last().unwrap();
    assert!(draw.base.drawable);
    assert!(!draw.overlay.drawable);
    assert_eq!(draw.overlay_start, draw.scene_operations);
    assert!(!draw.capture);
    // The overlay's input region is empty too.
    let regions = harness.test(cx, |window| window.input_regions());
    assert!(regions.last().unwrap().is_empty());
}

#[crate::test]
fn a1_index_zero_and_out_of_range_indices(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.plan.borrow_mut().deferred = true;
    harness.frame(cx);
    harness.with(cx, |window| {
        let len = window.rendered_frame.scene.len();
        window
            .platform_window
            .draw_layered(&window.rendered_frame.scene, 0, false);
        window
            .platform_window
            .draw_layered(&window.rendered_frame.scene, len + 1000, false);
        window
            .platform_window
            .draw_layered(&window.rendered_frame.scene, usize::MAX, false);
    });
    let draws = harness.test(cx, |window| window.layered_draws());
    let all_overlay = &draws[draws.len() - 3];
    assert_eq!(all_overlay.overlay_start, 0);
    assert!(!all_overlay.base.drawable, "everything is in the overlay");
    assert_eq!(all_overlay.overlay.operations, all_overlay.scene_operations);
    for clamped in &draws[draws.len() - 2..] {
        assert_eq!(clamped.overlay_start, clamped.scene_operations);
        assert!(clamped.base.drawable);
        assert!(!clamped.overlay.drawable);
    }
}

#[crate::test]
fn a1_capture_is_true_for_deferred_draws_prompts_and_drags_and_false_for_tooltips(
    cx: &mut TestAppContext,
) {
    // Deferred draw.
    let harness = Harness::new(cx, true);
    harness.plan.borrow_mut().deferred = true;
    harness.frame(cx);
    let capture = |harness: &Harness, cx: &mut TestAppContext| {
        harness
            .test(cx, |window| window.layered_draws())
            .last()
            .unwrap()
            .capture
    };
    assert!(capture(&harness, cx), "deferred draws capture");

    // Tooltip only.
    let tooltip_harness = Harness::new(cx, true);
    tooltip_harness.plan.borrow_mut().tooltip = true;
    tooltip_harness.frame(cx);
    let draws = tooltip_harness.test(cx, |window| window.layered_draws());
    let draw = draws.last().unwrap();
    assert!(
        draw.overlay.drawable,
        "the tooltip is painted into the overlay"
    );
    assert!(!draw.capture, "a passive tooltip does not capture");

    // Active drag.
    let drag_harness = Harness::new(cx, true);
    cx.update_window(drag_harness.window, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.dispatch_event(
            FileDropEvent::Entered {
                position: point(px(5.), px(5.)),
                paths: crate::ExternalPaths::default(),
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    drag_harness.frame(cx);
    assert!(capture(&drag_harness, cx), "active drags capture");

    // Prompt.
    let prompt_harness = Harness::new(cx, true);
    cx.update_window(prompt_harness.window, |_, window, cx| {
        drop(window.prompt(crate::PromptLevel::Info, "Hello", None, &["Ok"], cx));
    })
    .unwrap();
    prompt_harness.frame(cx);
    assert!(capture(&prompt_harness, cx), "prompts capture");
}

#[crate::test]
fn a1_windows_without_composition_present_one_plane(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, false);
    harness.plan.borrow_mut().deferred = true;
    harness.frame(cx);
    let (draw_count, layered) =
        harness.test(cx, |window| (window.draw_count(), window.layered_draws()));
    assert_eq!(draw_count, 1, "ordinary presentation");
    assert!(layered.is_empty());
    harness.with(cx, |window| {
        assert!(window.native_composition().is_none());
        assert!(window.composition().is_err());
        assert!(window.composition_stats().is_none());
    });
}

#[crate::test]
fn a1_layered_entry_flattens_to_ordinary_drawing_without_composition(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, false);
    harness.frame(cx);
    harness.with(cx, |window| {
        window
            .platform_window
            .draw_layered(&window.rendered_frame.scene, 3, true);
    });
    let (draw_count, layered) =
        harness.test(cx, |window| (window.draw_count(), window.layered_draws()));
    assert_eq!(draw_count, 2, "the whole scene is drawn as one plane");
    assert!(layered.is_empty());
}

#[crate::test]
fn a1_enabling_is_an_error_where_composition_is_unavailable_and_idempotent_elsewhere(
    cx: &mut TestAppContext,
) {
    let unavailable = Harness::new(cx, false);
    unavailable.with(cx, |window| {
        let error = window.enable_scene_overlay().unwrap_err().to_string();
        assert!(error.contains("not available"), "{error}");
        assert!(window.enable_window_composition().is_err());
    });
    // The window keeps drawing a normal single plane.
    unavailable.frame(cx);
    assert_eq!(unavailable.test(cx, |window| window.draw_count()), 1);

    let available = Harness::new(cx, true);
    available.with(cx, |window| {
        window.enable_window_composition().unwrap();
        window.enable_window_composition().unwrap();
        window.enable_scene_overlay().unwrap();
        assert!(window.native_composition().is_some());
    });
    // Only the first enabling reached the platform: still one base and one overlay.
    available.with(cx, |window| {
        let layout = window.composition().unwrap().layout();
        assert_eq!(layout.entries.len(), 2);
    });
}

#[crate::test]
fn a1_scene_overlay_alone_is_the_two_plane_subset(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, false);
    harness.with(cx, |window| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .allow_composition();
        window.enable_scene_overlay().unwrap();
        assert!(window.native_composition().is_some());
        // No surface tree until the general composition is enabled.
        assert!(matches!(
            window.composition().err(),
            Some(CompositionError::NotEnabled)
        ));
    });
    harness.plan.borrow_mut().deferred = true;
    harness.frame(cx);
    assert!(
        harness
            .test(cx, |window| window.layered_draws())
            .last()
            .unwrap()
            .overlay
            .drawable
    );
    harness.with(cx, |window| {
        window.enable_window_composition().unwrap();
        assert!(window.composition().is_ok());
    });
}

// ---- A2: surface tree through the controller ----

fn native(cx: &mut TestAppContext, harness: &Harness) -> crate::NativeSurface {
    harness.with(cx, |window| {
        window
            .composition()
            .unwrap()
            .create_native_surface()
            .unwrap()
    })
}

#[crate::test]
fn a2_controller_rejects_stale_self_cycle_and_cross_parent_ordering(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let first = native(cx, &harness);
    let second = native(cx, &harness);
    let third = native(cx, &harness);
    let (first_id, second_id, third_id) = (first.id(), second.id(), third.id());
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        assert!(matches!(
            controller.order_above(first_id, first_id),
            Err(CompositionError::SelfOrdering(_))
        ));
        controller.reparent(second_id, Some(first_id)).unwrap();
        controller.reparent(third_id, Some(second_id)).unwrap();
        assert!(matches!(
            controller.reparent(first_id, Some(third_id)),
            Err(CompositionError::Cycle(..))
        ));
        assert!(matches!(
            controller.order_above(first_id, third_id),
            Err(CompositionError::DifferentParents(..))
        ));
        assert!(matches!(
            controller.order_below(second_id, controller.overlay()),
            Err(CompositionError::DifferentParents(..))
        ));
        assert_eq!(controller.parent(third_id).unwrap(), Some(second_id));
        assert_eq!(controller.children(first_id).unwrap(), [second_id]);
        assert_eq!(controller.kind(first_id).unwrap(), SurfaceKind::Native);
        // A stale identity.
        controller.remove(third_id).unwrap();
        assert!(matches!(
            controller.kind(third_id),
            Err(CompositionError::Stale(_))
        ));
        assert!(matches!(
            controller.order_above(first_id, third_id),
            Err(CompositionError::Stale(_))
        ));
    });
    drop(third);
}

#[crate::test]
fn a2_platform_failure_restores_the_previous_tree(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let first = native(cx, &harness);
    let second = native(cx, &harness);
    let (first_id, second_id) = (first.id(), second.id());
    let before = harness.with(cx, |window| window.composition().unwrap().layout());
    harness.test(cx, |window| window.fail_next_order_syncs(1));
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        let error = controller.order_below(second_id, first_id).unwrap_err();
        assert!(matches!(error, CompositionError::Platform(_)), "{error}");
        assert_eq!(controller.layout(), before, "the tree is restored");
        assert_eq!(controller.parent(second_id).unwrap(), None);
        // The change works once the platform cooperates.
        controller.order_below(second_id, first_id).unwrap();
        assert_ne!(controller.layout(), before);
    });
    // Reparent and removal roll back too.
    let before = harness.with(cx, |window| window.composition().unwrap().layout());
    harness.test(cx, |window| window.fail_next_order_syncs(1));
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        assert!(controller.reparent(second_id, Some(first_id)).is_err());
        assert_eq!(controller.layout(), before);
    });
    harness.test(cx, |window| window.fail_next_order_syncs(1));
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        assert!(controller.remove(first_id).is_err());
        assert_eq!(controller.layout(), before);
        assert!(controller.kind(first_id).is_ok(), "still alive");
    });
    // A failed creation leaves nothing behind.
    harness.test(cx, |window| window.fail_next_order_syncs(1));
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        assert!(controller.create_native_surface().is_err());
        assert_eq!(controller.layout(), before);
    });
}

#[crate::test]
fn a2_removal_rehomes_children_at_the_former_sibling_position(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let left = native(cx, &harness);
    let parent = native(cx, &harness);
    let right = native(cx, &harness);
    let child = native(cx, &harness);
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        controller.reparent(child.id(), Some(parent.id())).unwrap();
        controller.remove(parent.id()).unwrap();
        let layout = controller.layout();
        let ids: Vec<SurfaceId> = layout.entries.iter().map(|entry| entry.id).collect();
        assert_eq!(
            ids,
            [
                layout.base,
                left.id(),
                child.id(),
                right.id(),
                layout.overlay
            ]
        );
        assert_eq!(controller.parent(child.id()).unwrap(), None);
    });
    // The platform learned the new arrangement and dropped the removed attachment.
    let last = harness.test(cx, |window| window.last_layout()).unwrap();
    assert!(last.entry(parent.id()).is_none());
    assert!(
        harness
            .test(cx, |window| window.attachment(parent.id()))
            .is_none()
    );
    drop(parent);
}

#[crate::test]
fn a2_base_and_overlay_cannot_be_removed(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        for id in [controller.base(), controller.overlay()] {
            assert!(matches!(
                controller.remove(id),
                Err(CompositionError::DefaultSurface(_))
            ));
            assert!(matches!(
                controller.reparent(id, None),
                Err(CompositionError::DefaultSurface(_))
            ));
        }
        assert_eq!(controller.layout().entries.len(), 2);
        assert_eq!(
            controller.kind(controller.base()).unwrap(),
            SurfaceKind::Framework
        );
    });
}

#[crate::test]
fn a2_dropping_the_owning_handle_removes_the_surface(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    let surface = native(cx, &harness);
    let id = surface.id();
    assert!(harness.test(cx, |window| window.attachment(id)).is_some());
    drop(surface);
    // The platform learns about the removal at the next presentation.
    harness.frame(cx);
    harness.with(cx, |window| {
        let controller = window.composition().unwrap();
        assert!(matches!(
            controller.kind(id),
            Err(CompositionError::Stale(_))
        ));
        assert_eq!(controller.layout().entries.len(), 2);
    });
    assert!(harness.test(cx, |window| window.attachment(id)).is_none());
    assert!(
        harness
            .test(cx, |window| window.last_layout())
            .unwrap()
            .entry(id)
            .is_none()
    );
}

// ---- A3: paint stream ----

fn quad_scene(colors: &[crate::Hsla]) -> Scene {
    let mut scene = Scene::default();
    for (index, color) in colors.iter().enumerate() {
        scene.insert_primitive(crate::Quad {
            bounds: Bounds {
                origin: crate::Point::new(
                    crate::ScaledPixels(index as f32),
                    crate::ScaledPixels(0.0),
                ),
                size: crate::Size {
                    width: crate::ScaledPixels(20.0),
                    height: crate::ScaledPixels(20.0),
                },
            },
            content_mask: crate::ContentMask {
                bounds: Bounds {
                    origin: crate::Point::default(),
                    size: crate::Size {
                        width: crate::ScaledPixels(1000.0),
                        height: crate::ScaledPixels(1000.0),
                    },
                },
            },
            background: (*color).into(),
            ..Default::default()
        });
    }
    scene
}

fn scaled(x: f32, y: f32, w: f32, h: f32) -> Bounds<crate::ScaledPixels> {
    Bounds {
        origin: crate::Point::new(crate::ScaledPixels(x), crate::ScaledPixels(y)),
        size: crate::Size {
            width: crate::ScaledPixels(w),
            height: crate::ScaledPixels(h),
        },
    }
}

/// A scene's paint operations are balanced: every scope closes, none closes twice.
fn is_balanced(scene: &Scene) -> bool {
    let mut depth = 0i32;
    for operation in &scene.paint_operations {
        match operation {
            crate::PaintOperation::StartLayer(_) => depth += 1,
            crate::PaintOperation::EndLayer => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn quad_colors(scene: &Scene) -> Vec<crate::Hsla> {
    scene
        .quads
        .iter()
        .map(|quad| quad.background.as_solid().unwrap())
        .collect()
}

#[test]
fn a3_repeated_separated_stretches_keep_their_relative_order() {
    let colors = [red(), green(), blue(), black()];
    let scene = quad_scene(&colors);
    // Stream: red -> A, green -> B, blue -> A, black -> B.
    let scenes = scene
        .split_ranges(&[(0, 0..1), (1, 1..2), (0, 2..3), (1, 3..4)], 2)
        .unwrap();
    assert_eq!(quad_colors(&scenes[0]), [red(), blue()]);
    assert_eq!(quad_colors(&scenes[1]), [green(), black()]);
}

#[test]
fn a3_a_range_boundary_inside_a_layer_scope_reopens_and_closes_the_scope() {
    let mut scene = Scene::default();
    scene.push_layer(scaled(0.0, 0.0, 100.0, 100.0));
    let quad = |x: f32| crate::Quad {
        bounds: scaled(x, 0.0, 10.0, 10.0),
        content_mask: crate::ContentMask {
            bounds: scaled(0.0, 0.0, 1000.0, 1000.0),
        },
        background: red().into(),
        ..Default::default()
    };
    scene.insert_primitive(quad(0.0));
    scene.insert_primitive(quad(20.0));
    scene.pop_layer();
    // StartLayer, quad, quad, EndLayer: split between the quads.
    let (base, overlay) = scene.split_at_overlay(2).unwrap();
    for plane in [&base, &overlay] {
        assert!(is_balanced(plane));
        assert_eq!(plane.len(), 3, "start, one quad, end");
        assert!(!plane.is_empty());
    }
    // The same through the single-range entry point.
    let mut tail = Scene::default();
    tail.replay_balanced(2..4, &scene).unwrap();
    assert!(is_balanced(&tail));
    assert_eq!(tail.len(), 3);
}

#[test]
fn a3_empty_scope_markers_do_not_make_a_scene_drawable() {
    let mut scene = Scene::default();
    scene.push_layer(scaled(0.0, 0.0, 10.0, 10.0));
    scene.pop_layer();
    assert!(scene.is_empty());
    assert_eq!(scene.len(), 2);
    let (base, overlay) = scene.split_at_overlay(1).unwrap();
    assert!(base.is_empty() && overlay.is_empty());
    assert!(is_balanced(&base) && is_balanced(&overlay));
    assert!(!quad_scene(&[red()]).is_empty());
}

#[test]
fn a3_unbalanced_replay_is_reported_not_drawn() {
    let mut scene = quad_scene(&[red()]);
    scene.pop_layer(); // closes a scope that was never opened
    let error = scene.split_at_overlay(1);
    assert!(matches!(
        error,
        Err(UnbalancedScopes { operation_index: 1 })
    ));
    assert!(scene.split_ranges(&[(0, 0..2)], 1).is_err());
    let mut target = Scene::default();
    assert!(target.replay_balanced(0..2, &scene).is_err());
    assert!(target.is_empty(), "nothing was replayed");
    // Out-of-range replays fail instead of panicking.
    assert!(target.replay_balanced(0..10, &scene).is_err());
}

#[crate::test]
fn a3_an_unbalanced_frame_is_not_drawn_by_the_layered_platform(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.frame(cx);
    let mut bad = quad_scene(&[red()]);
    bad.pop_layer();
    harness.with(cx, |window| {
        window.platform_window.draw_layered(&bad, 1, false);
    });
    let draws = harness.test(cx, |window| window.layered_draws());
    let draw = draws.last().unwrap();
    assert!(draw.unbalanced.is_some());
    assert!(!draw.base.drawable && !draw.overlay.drawable, "{draw:?}");
}

#[crate::test]
fn a3_empty_scope_markers_in_the_overlay_do_not_draw(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.plan.borrow_mut().empty_layer_overlay = true;
    harness.frame(cx);
    let draws = harness.test(cx, |window| window.layered_draws());
    let draw = draws.last().unwrap();
    assert!(draw.overlay.operations > 0, "the markers were recorded");
    assert!(!draw.overlay.drawable);
}

/// Paints into an additional framework surface between base content.
struct MultiSurface {
    surface: Rc<Cell<Option<SurfaceId>>>,
}

impl Render for MultiSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let surface = self.surface.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let Some(surface) = surface.get() else {
                    return;
                };
                window.paint_quad(fill(bounds, red()));
                window
                    .with_composition_surface(surface, |window| {
                        window.paint_quad(fill(bounds, green()));
                        window.paint_quad(fill(bounds, blue()));
                    })
                    .ok();
                window.paint_quad(fill(bounds, black()));
                window
                    .with_composition_surface(surface, |window| {
                        window.paint_quad(fill(bounds, red()));
                    })
                    .ok();
            },
        )
        .size_full()
    }
}

#[crate::test]
fn a3_surface_switches_derive_one_scene_per_surface_in_tree_order(cx: &mut TestAppContext) {
    let surface_id: Rc<Cell<Option<SurfaceId>>> = Default::default();
    let window: AnyWindowHandle = cx
        .add_window({
            let surface = surface_id.clone();
            move |_, _| MultiSurface { surface }
        })
        .into();
    let handle = cx
        .update_window(window, |_, window, _| {
            window
                .platform_window
                .as_test()
                .unwrap()
                .allow_composition();
            window.enable_window_composition().unwrap();
            let handle = window
                .composition()
                .unwrap()
                .create_framework_surface()
                .unwrap();
            surface_id.set(Some(handle.id()));
            handle
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.present(cx);
        // Base paints red, black; the extra surface green, blue, red (two separated
        // stretches); nothing reaches the overlay.
        let composed_draw = window
            .platform_window
            .as_test()
            .unwrap()
            .composed_draws()
            .pop()
            .unwrap();
        let ops: Vec<usize> = composed_draw
            .layers
            .iter()
            .map(|(_, plane, _)| plane.operations)
            .collect();
        let controller = window.composition().unwrap();
        let order: Vec<SurfaceId> = composed_draw.layers.iter().map(|(id, ..)| *id).collect();
        assert_eq!(
            order,
            [controller.base(), handle.id(), controller.overlay()]
        );
        assert_eq!(ops, [2, 3, 0]);
        assert!(composed_draw.layers[1].1.drawable);
        assert!(!composed_draw.layers[2].1.drawable);
    })
    .unwrap();
    // Draws through the layered entry did not happen: the tree has an extra surface.
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .platform_window
                .as_test()
                .unwrap()
                .layered_draws()
                .is_empty()
        );
    })
    .unwrap();
}

#[test]
fn a3_composed_scenes_expose_ordered_layers_hitboxes_and_drawable_extraction() {
    use crate::{ComposedScene, SurfaceSwitch, SurfaceTree};
    let mut tree = SurfaceTree::new();
    let extra = tree.create_below_overlay(SurfaceKind::Framework);
    let (base, overlay) = (tree.base(), tree.overlay());
    let scene = quad_scene(&[red(), green(), blue()]);
    let switches = [
        SurfaceSwitch {
            scene_index: 1,
            surface: extra,
        },
        SurfaceSwitch {
            scene_index: 2,
            surface: overlay,
        },
    ];
    let hitbox = rect(1.0, 2.0, 3.0, 4.0);
    let composed = ComposedScene::derive(
        &scene,
        base,
        &switches,
        &tree.framework_surfaces(),
        &[(overlay, hitbox)],
        true,
    )
    .unwrap();
    let surfaces: Vec<SurfaceId> = composed.layers.iter().map(|layer| layer.surface).collect();
    assert_eq!(surfaces, [base, extra, overlay]);
    assert_eq!(quad_colors(&composed.layer(base).unwrap().scene), [red()]);
    assert_eq!(
        quad_colors(&composed.layer(extra).unwrap().scene),
        [green()]
    );
    assert_eq!(
        quad_colors(&composed.layer(overlay).unwrap().scene),
        [blue()]
    );
    assert_eq!(composed.layer(overlay).unwrap().hitboxes, [hitbox]);
    assert!(composed.layer(base).unwrap().hitboxes.is_empty());
    assert_eq!(composed.drawable_layers().count(), 3);

    // A surface that was removed after painting drops its operations; the rest stays.
    let composed =
        ComposedScene::derive(&scene, base, &switches, &[base, overlay], &[], false).unwrap();
    assert_eq!(composed.layers.len(), 2);
    assert_eq!(quad_colors(&composed.layer(base).unwrap().scene), [red()]);

    // Layers holding only empty scope markers are not drawable.
    let mut markers = Scene::default();
    markers.push_layer(scaled(0.0, 0.0, 1.0, 1.0));
    markers.pop_layer();
    let composed =
        ComposedScene::derive(&markers, base, &[], &[base, overlay], &[], false).unwrap();
    assert_eq!(composed.drawable_layers().count(), 0);
}

struct RetainedChild {
    surface: Rc<Cell<Option<SurfaceId>>>,
    renders: Rc<Cell<u32>>,
}

impl Render for RetainedChild {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let surface = self.surface.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.paint_quad(fill(bounds, red()));
                let Some(surface) = surface.get() else {
                    return;
                };
                window
                    .with_composition_surface(surface, |window| {
                        window.paint_quad(fill(bounds, green()));
                    })
                    .ok();
            },
        )
        .size(px(20.))
    }
}

struct RetainedParent {
    child: crate::Entity<RetainedChild>,
    tint: Rc<Cell<u32>>,
}

impl Render for RetainedParent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let _ = self.tint.get();
        div()
            .child(div().size(px(5.)).bg(blue()))
            .child(AnyView::from(self.child.clone()).cached(StyleRefinement::default()))
            .child(div().size(px(5.)).bg(black()))
    }
}

#[crate::test]
fn a3_retained_subtree_replay_carries_surface_switches(cx: &mut TestAppContext) {
    let surface_id: Rc<Cell<Option<SurfaceId>>> = Default::default();
    let renders = Rc::new(Cell::new(0));
    let tint = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let (surface, renders, tint) = (surface_id.clone(), renders.clone(), tint);
        move |_, cx| RetainedParent {
            child: cx.new(|_| RetainedChild { surface, renders }),
            tint,
        }
    });
    let handle: AnyWindowHandle = window.into();
    let extra = cx
        .update_window(handle, |_, window, _| {
            window
                .platform_window
                .as_test()
                .unwrap()
                .allow_composition();
            window.enable_window_composition().unwrap();
            let surface = window
                .composition()
                .unwrap()
                .create_framework_surface()
                .unwrap();
            surface_id.set(Some(surface.id()));
            surface
        })
        .unwrap();
    let frame = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.present(cx);
            window
                .platform_window
                .as_test()
                .unwrap()
                .composed_draws()
                .pop()
                .unwrap()
        })
        .unwrap()
    };
    let first = frame(cx);
    let renders_after_first = renders.get();
    let extra_layer = |draw: &crate::ComposedDraw| {
        draw.layers
            .iter()
            .find(|(surface, ..)| *surface == extra.id())
            .cloned()
            .map(|(_, plane, _)| plane)
            .unwrap()
    };
    assert!(extra_layer(&first).drawable);

    // Only the parent changes: the child's paint is replayed, not re-recorded.
    window
        .update(cx, |parent, _, cx| {
            parent.tint.set(1);
            cx.notify();
        })
        .unwrap();
    let second = frame(cx);
    assert_eq!(
        renders.get(),
        renders_after_first,
        "the cached child was not rendered again"
    );
    assert_eq!(
        extra_layer(&second),
        extra_layer(&first),
        "the replayed range still paints into its surface"
    );
    // The base plane did not receive the child's switched-out quad.
    let base_ops = |draw: &crate::ComposedDraw| draw.layers[0].1.operations;
    assert_eq!(base_ops(&first), base_ops(&second));
}

// ---- A4: native attachments ----

/// An element that positions `surface` in base content, or inside a deferred overlay, and
/// optionally blocks pointer events over `blocker` the way a framework control would.
fn positioned_element(
    surface: Rc<crate::NativeSurface>,
    bounds: Bounds<Pixels>,
    visible: bool,
    in_overlay: bool,
    blocker: Option<Bounds<Pixels>>,
) -> impl IntoElement {
    let position = move |_, _, window: &mut Window, _: &mut crate::App| {
        surface.position(window, bounds, visible)
    };
    let block = canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let Some(blocker) = blocker else {
                return;
            };
            window.on_mouse_event(
                move |event: &crate::MouseMoveEvent, phase, _, cx: &mut crate::App| {
                    if phase == crate::DispatchPhase::Capture && blocker.contains(&event.position) {
                        cx.stop_propagation();
                    }
                },
            );
            window.on_mouse_event(
                move |event: &crate::MouseDownEvent, phase, _, cx: &mut crate::App| {
                    if phase == crate::DispatchPhase::Capture && blocker.contains(&event.position) {
                        cx.stop_propagation();
                    }
                },
            );
        },
    )
    .size(px(1.));
    let element = div().size_full().bg(red()).child(block);
    if in_overlay {
        element.child(deferred(
            div().child(canvas(|_, _, _| (), position).size(px(1.))),
        ))
    } else {
        element.child(canvas(|_, _, _| (), position).size(px(1.)))
    }
}

struct PositionFixture {
    harness_window: AnyWindowHandle,
    surface: Rc<crate::NativeSurface>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    visible: Rc<Cell<bool>>,
    in_overlay: Rc<Cell<bool>>,
    /// Framework content over part of the window that stops propagation of pointer events.
    blocker: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl PositionFixture {
    fn new(cx: &mut TestAppContext) -> Self {
        let bounds = Rc::new(Cell::new(rect(180.0, 40.0, 520.0, 340.0)));
        let visible = Rc::new(Cell::new(true));
        let in_overlay = Rc::new(Cell::new(false));
        let blocker = Rc::new(Cell::new(None));
        // The window exists first; the surface is created once composition is enabled.
        let slot: Rc<RefCell<Option<Rc<crate::NativeSurface>>>> = Default::default();
        let window: AnyWindowHandle = cx
            .add_window({
                let (slot, bounds, visible, in_overlay, blocker) = (
                    slot.clone(),
                    bounds.clone(),
                    visible.clone(),
                    in_overlay.clone(),
                    blocker.clone(),
                );
                move |_, _| LazyPositioned {
                    slot,
                    base_bounds: bounds,
                    visible,
                    in_overlay,
                    blocker,
                }
            })
            .into();
        let surface = cx
            .update_window(window, |_, window, _| {
                window
                    .platform_window
                    .as_test()
                    .unwrap()
                    .allow_composition();
                window.enable_window_composition().unwrap();
                Rc::new(
                    window
                        .composition()
                        .unwrap()
                        .create_native_surface()
                        .unwrap(),
                )
            })
            .unwrap();
        *slot.borrow_mut() = Some(surface.clone());
        PositionFixture {
            harness_window: window,
            surface,
            bounds,
            visible,
            in_overlay,
            blocker,
        }
    }

    fn frame(&self, cx: &mut TestAppContext) {
        cx.update_window(self.harness_window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.present(cx);
        })
        .unwrap();
    }

    fn attachment(&self, cx: &mut TestAppContext) -> Rc<RefCell<crate::AttachmentState>> {
        let id = self.surface.id();
        cx.update_window(self.harness_window, |_, window, _| {
            window
                .platform_window
                .as_test()
                .unwrap()
                .attachment(id)
                .unwrap()
        })
        .unwrap()
    }

    fn stats(&self, cx: &mut TestAppContext) -> crate::CompositionStats {
        cx.update_window(self.harness_window, |_, window, _| {
            window.composition_stats().unwrap()
        })
        .unwrap()
    }

    fn layout(&self, cx: &mut TestAppContext) -> crate::CompositionLayout {
        cx.update_window(self.harness_window, |_, window, _| {
            window.composition().unwrap().layout()
        })
        .unwrap()
    }
}

struct LazyPositioned {
    slot: Rc<RefCell<Option<Rc<crate::NativeSurface>>>>,
    base_bounds: Rc<Cell<Bounds<Pixels>>>,
    visible: Rc<Cell<bool>>,
    in_overlay: Rc<Cell<bool>>,
    blocker: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Render for LazyPositioned {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.slot.borrow().clone() {
            Some(surface) => positioned_element(
                surface,
                self.base_bounds.get(),
                self.visible.get(),
                self.in_overlay.get(),
                self.blocker.get(),
            )
            .into_any_element(),
            None => div().into_any_element(),
        }
    }
}

#[crate::test]
fn a4_identical_bounds_and_visibility_cause_no_platform_write(cx: &mut TestAppContext) {
    let fixture = PositionFixture::new(cx);
    fixture.frame(cx);
    let attachment = fixture.attachment(cx);
    assert_eq!(attachment.borrow().geometry_writes.len(), 1);
    assert_eq!(attachment.borrow().visibility_writes, [true]);
    // Device pixels: scale factor 2 of the test window.
    assert_eq!(
        attachment.borrow().geometry_writes[0].0,
        rect(180.0, 40.0, 520.0, 340.0).to_device_pixels(2.0)
    );
    for _ in 0..5 {
        fixture.frame(cx);
    }
    assert_eq!(
        attachment.borrow().geometry_writes.len(),
        1,
        "unchanged bounds"
    );
    assert_eq!(
        attachment.borrow().visibility_writes.len(),
        1,
        "unchanged visibility"
    );
    let stats = fixture.stats(cx);
    assert_eq!((stats.bounds_writes, stats.visibility_writes), (1, 1));

    // A move plus resize is exactly one write; no order synchronization.
    let syncs = stats.order_syncs;
    fixture.bounds.set(rect(181.0, 40.0, 519.0, 340.0));
    fixture.frame(cx);
    assert_eq!(attachment.borrow().geometry_writes.len(), 2);
    assert_eq!(fixture.stats(cx).order_syncs, syncs);
    // Visibility changes are written once, in both directions.
    fixture.visible.set(false);
    fixture.frame(cx);
    fixture.frame(cx);
    assert_eq!(attachment.borrow().visibility_writes, [true, false]);
    fixture.visible.set(true);
    fixture.frame(cx);
    assert_eq!(attachment.borrow().visibility_writes, [true, false, true]);
}

#[crate::test]
fn a4_a_failed_geometry_write_is_retried_next_frame(cx: &mut TestAppContext) {
    let fixture = PositionFixture::new(cx);
    let attachment = fixture.attachment(cx);
    attachment.borrow_mut().fail_geometry_writes = 1;
    let dirty = cx
        .update_window(fixture.harness_window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.present(cx);
            window.invalidator.is_dirty()
        })
        .unwrap();
    assert!(
        attachment.borrow().geometry_writes.is_empty(),
        "the write failed"
    );
    assert!(
        attachment.borrow().visibility_writes.is_empty(),
        "an attachment whose geometry failed is not shown at a stale place"
    );
    assert_eq!(fixture.stats(cx).failed_writes, 1);
    assert!(dirty, "the window schedules the retry");
    fixture.frame(cx);
    assert_eq!(
        attachment.borrow().geometry_writes.len(),
        1,
        "retried and applied"
    );
    assert_eq!(attachment.borrow().visibility_writes, [true]);
    fixture.frame(cx);
    assert_eq!(attachment.borrow().geometry_writes.len(), 1);
}

#[crate::test]
fn a4_positioning_in_a_deferred_overlay_hosts_above_it_and_base_content_returns_below(
    cx: &mut TestAppContext,
) {
    let fixture = PositionFixture::new(cx);
    let id = fixture.surface.id();
    fixture.frame(cx);
    let layout = fixture.layout(cx);
    assert!(layout.position(id).unwrap() < layout.position(layout.overlay).unwrap());
    let syncs_below = fixture.stats(cx).order_syncs;

    fixture.in_overlay.set(true);
    fixture.frame(cx);
    let layout = fixture.layout(cx);
    assert!(
        layout.position(id).unwrap() > layout.position(layout.overlay).unwrap(),
        "positioned while prepainting a deferred overlay: above the overlay"
    );
    assert_eq!(fixture.stats(cx).order_syncs, syncs_below + 1);
    // Staying in the overlay does not synchronize again.
    fixture.frame(cx);
    fixture.frame(cx);
    assert_eq!(fixture.stats(cx).order_syncs, syncs_below + 1);

    fixture.in_overlay.set(false);
    fixture.frame(cx);
    let layout = fixture.layout(cx);
    assert!(
        layout.position(id).unwrap() < layout.position(layout.overlay).unwrap(),
        "positioned in base content again: below the overlay"
    );
    assert_eq!(fixture.stats(cx).order_syncs, syncs_below + 2);
}

// ---- A11: cooperating windowless producers ----

fn dispatch(
    cx: &mut TestAppContext,
    fixture: &PositionFixture,
    event: impl crate::InputEvent,
) -> bool {
    cx.update_window(fixture.harness_window, |_, window, cx| {
        window
            .dispatch_event(event.to_platform_input(), cx)
            .propagate
    })
    .unwrap()
}

fn pointer_move(x: f32, y: f32, pressed: Option<crate::MouseButton>) -> crate::MouseMoveEvent {
    crate::MouseMoveEvent {
        position: point(px(x), px(y)),
        pressed_button: pressed,
        modifiers: Default::default(),
    }
}

fn pointer_down(x: f32, y: f32) -> crate::MouseDownEvent {
    crate::MouseDownEvent {
        button: crate::MouseButton::Left,
        position: point(px(x), px(y)),
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    }
}

fn pointer_up(x: f32, y: f32) -> crate::MouseUpEvent {
    crate::MouseUpEvent {
        button: crate::MouseButton::Left,
        position: point(px(x), px(y)),
        modifiers: Default::default(),
        click_count: 1,
    }
}

#[crate::test]
fn a11_forwarding_follows_framework_capture_bounds_ownership_and_visibility(
    cx: &mut TestAppContext,
) {
    let fixture = PositionFixture::new(cx);
    let log: Rc<RefCell<Vec<String>>> = Default::default();
    fixture
        .surface
        .set_input_forwarding(crate::InputForwarding::Framework);
    fixture.surface.set_pointer_handler({
        let log = log.clone();
        move |event, _, _| {
            let (name, position) = match event {
                crate::NativePointerEvent::Enter(e) => ("enter", e.position),
                crate::NativePointerEvent::Move(e) => ("move", e.position),
                crate::NativePointerEvent::Leave(e) => ("leave", e.position),
                crate::NativePointerEvent::Down(e) => ("down", e.position),
                crate::NativePointerEvent::Up(e) => ("up", e.position),
            };
            log.borrow_mut()
                .push(format!("{name} {} {}", position.x.0, position.y.0));
            matches!(event, crate::NativePointerEvent::Down(_))
        }
    });
    fixture.surface.set_wheel_handler({
        let log = log.clone();
        move |event, _, _| {
            log.borrow_mut()
                .push(format!("wheel {}", event.position.x.0));
            true
        }
    });
    // Framework content blocks a corner of the child's area.
    fixture.blocker.set(Some(rect(600.0, 300.0, 100.0, 80.0)));
    fixture.frame(cx);
    assert_eq!(
        fixture.attachment(cx).borrow().forwarding,
        crate::InputForwarding::Framework
    );
    let take = |log: &Rc<RefCell<Vec<String>>>| std::mem::take(&mut *log.borrow_mut());

    // Outside the child's current bounds nothing is forwarded.
    dispatch(cx, &fixture, pointer_move(10.0, 10.0, None));
    assert!(take(&log).is_empty());
    // Inside: enter and move, in window-content logical coordinates.
    dispatch(cx, &fixture, pointer_move(200.0, 60.0, None));
    assert_eq!(take(&log), ["enter 200 60", "move 200 60"]);
    // Framework capture runs first: the covered pointer leaves the child and nothing else.
    dispatch(cx, &fixture, pointer_move(650.0, 350.0, None));
    assert_eq!(take(&log), ["leave 650 350"]);
    assert!(
        !dispatch(cx, &fixture, pointer_down(650.0, 350.0)),
        "the blocker stopped it"
    );
    assert!(
        take(&log).is_empty(),
        "no press reaches the child under framework content"
    );

    // A press keeps its recipient through the release, wherever the pointer goes.
    let propagated = dispatch(cx, &fixture, pointer_down(200.0, 60.0));
    assert_eq!(take(&log), ["enter 200 60", "down 200 60"]);
    assert!(!propagated, "a handled press stops propagation");
    dispatch(
        cx,
        &fixture,
        pointer_move(10.0, 10.0, Some(crate::MouseButton::Left)),
    );
    assert_eq!(
        take(&log),
        ["move 10 10"],
        "the owner keeps receiving moves"
    );
    dispatch(cx, &fixture, pointer_up(10.0, 10.0));
    assert_eq!(take(&log), ["up 10 10", "leave 10 10"]);

    // Wheel events go to the topmost surface under the pointer after capture.
    cx.update_window(fixture.harness_window, |_, window, cx| {
        window.dispatch_event(
            crate::ScrollWheelEvent {
                position: point(px(300.0), px(100.0)),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    assert_eq!(take(&log), ["wheel 300"]);

    // The surface's current bounds apply: after moving the child away the old area is dead.
    fixture.bounds.set(rect(0.0, 400.0, 100.0, 100.0));
    fixture.frame(cx);
    dispatch(cx, &fixture, pointer_move(200.0, 60.0, None));
    assert!(take(&log).is_empty());
    fixture.bounds.set(rect(180.0, 40.0, 520.0, 340.0));
    fixture.frame(cx);
    dispatch(cx, &fixture, pointer_move(200.0, 60.0, None));
    assert_eq!(take(&log), ["enter 200 60", "move 200 60"]);

    // A hidden attachment stops participating immediately.
    fixture.visible.set(false);
    fixture.frame(cx);
    dispatch(cx, &fixture, pointer_move(201.0, 60.0, None));
    dispatch(cx, &fixture, pointer_down(201.0, 60.0));
    assert!(take(&log).is_empty(), "nothing reaches a hidden surface");
    fixture.visible.set(true);
    fixture.frame(cx);
    dispatch(cx, &fixture, pointer_move(202.0, 60.0, None));
    assert_eq!(take(&log), ["enter 202 60", "move 202 60"]);

    // Input forwarding is a preference: with the default native path nothing is forwarded.
    fixture
        .surface
        .set_input_forwarding(crate::InputForwarding::Native);
    dispatch(cx, &fixture, pointer_move(203.0, 60.0, None));
    assert!(take(&log).is_empty());
}

#[crate::test]
fn a11_nothing_is_forwarded_after_the_attachment_is_destroyed(cx: &mut TestAppContext) {
    let fixture = PositionFixture::new(cx);
    let log: Rc<RefCell<Vec<String>>> = Default::default();
    fixture
        .surface
        .set_input_forwarding(crate::InputForwarding::Framework);
    fixture.surface.set_pointer_handler({
        let log = log.clone();
        move |_, _, _| {
            log.borrow_mut().push("event".into());
            false
        }
    });
    fixture.frame(cx);
    dispatch(cx, &fixture, pointer_move(200.0, 60.0, None));
    assert!(!log.borrow().is_empty());
    log.borrow_mut().clear();
    let id = fixture.surface.id();
    cx.update_window(fixture.harness_window, |_, window, _| {
        window.composition().unwrap().remove(id).unwrap();
    })
    .unwrap();
    dispatch(cx, &fixture, pointer_move(201.0, 60.0, None));
    dispatch(cx, &fixture, pointer_down(201.0, 60.0));
    assert!(log.borrow().is_empty());
}

// ---- A7: focus ----

#[crate::test]
fn a7_focus_moves_to_the_framework_when_capture_starts_and_is_not_restored(
    cx: &mut TestAppContext,
) {
    let harness = Harness::new(cx, true);
    harness.test(cx, |window| window.set_native_focus(true));
    harness.frame(cx);
    assert_eq!(
        harness.test(cx, |window| window.framework_focus_requests()),
        0,
        "no capture, no transfer"
    );

    // A passive tooltip does not transfer focus.
    harness.plan.borrow_mut().tooltip = true;
    harness.frame(cx);
    assert_eq!(
        harness.test(cx, |window| window.framework_focus_requests()),
        0
    );
    assert!(harness.test(cx, |window| window.native_focus()));

    // Capture starts over the focused native child: focus moves.
    {
        let mut plan = harness.plan.borrow_mut();
        plan.tooltip = false;
        plan.deferred = true;
    }
    harness.frame(cx);
    assert_eq!(
        harness.test(cx, |window| window.framework_focus_requests()),
        1
    );
    assert!(!harness.test(cx, |window| window.native_focus()));
    // Capture continuing does not move it again.
    harness.frame(cx);
    assert_eq!(
        harness.test(cx, |window| window.framework_focus_requests()),
        1
    );

    // Capture ends: focus is not given back to the native content.
    harness.plan.borrow_mut().deferred = false;
    harness.frame(cx);
    assert_eq!(
        harness.test(cx, |window| window.framework_focus_requests()),
        1
    );
    assert!(!harness.test(cx, |window| window.native_focus()));
}

#[crate::test]
fn a7_pressing_on_framework_content_takes_focus_from_a_native_child(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.frame(cx);
    harness.test(cx, |window| window.set_native_focus(true));
    cx.update_window(harness.window, |_, window, cx| {
        window.dispatch_event(
            crate::MouseDownEvent {
                button: crate::MouseButton::Left,
                position: point(px(30.), px(30.)),
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    assert_eq!(
        harness.test(cx, |window| window.framework_focus_requests()),
        1
    );
    assert!(!harness.test(cx, |window| window.native_focus()));
    // Without a focused native child a press transfers nothing.
    cx.update_window(harness.window, |_, window, cx| {
        window.dispatch_event(
            crate::MouseDownEvent {
                button: crate::MouseButton::Left,
                position: point(px(30.), px(30.)),
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
    })
    .unwrap();
    assert_eq!(
        harness.test(cx, |window| window.framework_focus_requests()),
        1
    );
}

// ---- A8: device recreation ----

#[crate::test]
fn a8_recreation_changes_the_generation_and_both_planes_redraw(cx: &mut TestAppContext) {
    let fixture = PositionFixture::new(cx);
    // Overlay content over a native child.
    fixture.frame(cx);
    let attachment = fixture.attachment(cx);
    let recreations: Rc<RefCell<Vec<u64>>> = Default::default();
    fixture.surface.on_recreated({
        let recreations = recreations.clone();
        move |composition| recreations.borrow_mut().push(composition.generation)
    });
    let generation = |cx: &mut TestAppContext| {
        cx.update_window(fixture.harness_window, |_, window, _| {
            window.native_composition().unwrap().generation
        })
        .unwrap()
    };
    assert_eq!(generation(cx), 0);
    let layered_before = cx
        .update_window(fixture.harness_window, |_, window, _| {
            window
                .platform_window
                .as_test()
                .unwrap()
                .layered_draws()
                .len()
        })
        .unwrap();

    // The device is lost: nothing is drawn until the surface is recreated.
    cx.update_window(fixture.harness_window, |_, window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .simulate_device_loss();
    })
    .unwrap();
    fixture.frame(cx);
    let test = |cx: &mut TestAppContext, f: &dyn Fn(&crate::TestWindow) -> usize| {
        cx.update_window(fixture.harness_window, |_, window, _| {
            f(window.platform_window.as_test().unwrap())
        })
        .unwrap()
    };
    assert_eq!(
        test(cx, &|window| window.layered_draws().len()),
        layered_before
    );

    cx.update_window(fixture.harness_window, |_, window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .simulate_composition_recreation();
        assert!(
            window.invalidator.is_dirty(),
            "the recreation requests a redraw"
        );
    })
    .unwrap();
    assert_eq!(generation(cx), 1, "the generation changed");
    assert_eq!(*recreations.borrow(), [1], "embedders were told to remount");
    assert_eq!(attachment.borrow().generation_value(), 1);
    // The renderer created on the lost device is not reused.
    assert_eq!(test(cx, &|window| window.overlay_renderer_creations()), 1);

    // Both planes redraw and the child geometry is re-applied.
    fixture.in_overlay.set(false);
    let writes_before = attachment.borrow().geometry_writes.len();
    fixture.frame(cx);
    let draws = cx
        .update_window(fixture.harness_window, |_, window, _| {
            window.platform_window.as_test().unwrap().layered_draws()
        })
        .unwrap();
    let redraw = draws.last().unwrap();
    assert_eq!(draws.len(), layered_before + 1);
    assert_eq!(redraw.generation, 1);
    assert!(redraw.base.drawable, "the base plane redrew");
    assert_eq!(
        test(cx, &|window| window.overlay_renderer_creations()),
        2,
        "the overlay renderer was recreated from the new device"
    );
    assert_eq!(
        attachment.borrow().geometry_writes.len(),
        writes_before + 1,
        "the attachment reapplied its geometry after recreation"
    );
}

#[crate::test]
fn a8_the_overlay_plane_redraws_after_recreation(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.plan.borrow_mut().deferred = true;
    harness.frame(cx);
    harness.test(cx, |window| window.simulate_device_loss());
    harness.test(cx, |window| window.simulate_composition_recreation());
    harness.frame(cx);
    let draws = harness.test(cx, |window| window.layered_draws());
    let draw = draws.last().unwrap();
    assert_eq!(draw.generation, 1);
    assert!(
        draw.base.drawable && draw.overlay.drawable,
        "both planes redraw: {draw:?}"
    );
    assert_eq!(
        harness.test(cx, |window| window.overlay_renderer_creations()),
        2
    );
    assert_eq!(
        harness.test(cx, |window| window.overlay_renderer_generation()),
        Some(1)
    );
}

// ---- layered input region (R14, R20) ----

#[test]
fn overlay_input_region_is_the_hitbox_union_only_while_capturing() {
    let hitboxes = [
        rect(0.0, 0.0, 10.0, 10.0),
        rect(20.0, 0.0, 10.0, 10.0),
        rect(0.0, 0.0, 0.0, 5.0),
    ];
    assert_eq!(
        crate::overlay_input_region(true, &hitboxes),
        [hitboxes[0], hitboxes[1]]
    );
    assert!(crate::overlay_input_region(false, &hitboxes).is_empty());
    assert!(crate::overlay_input_region(true, &[]).is_empty());
}

#[crate::test]
fn the_window_passes_the_overlay_input_region_to_the_platform(cx: &mut TestAppContext) {
    let harness = Harness::new(cx, true);
    harness.frame(cx);
    // No overlay content: empty region.
    assert!(
        harness
            .test(cx, |window| window.input_regions())
            .last()
            .unwrap()
            .is_empty()
    );
    // A tooltip overlay captures nothing: still empty even if it had hitboxes.
    harness.plan.borrow_mut().tooltip = true;
    harness.frame(cx);
    assert!(
        harness
            .test(cx, |window| window.input_regions())
            .last()
            .unwrap()
            .is_empty()
    );
    // A deferred draw without a hitbox leaves the region empty as well.
    {
        let mut plan = harness.plan.borrow_mut();
        plan.tooltip = false;
        plan.deferred = true;
    }
    harness.frame(cx);
    assert!(
        harness
            .test(cx, |window| window.input_regions())
            .last()
            .unwrap()
            .is_empty()
    );
}
