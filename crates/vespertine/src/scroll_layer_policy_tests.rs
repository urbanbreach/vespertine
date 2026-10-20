//! Tests of the policy of adaptive tiled scroll-content reuse (PRF-004 A3 through A8):
//! promotion, the fallbacks of R8, demotion and cooldown (R9, R10), snapping (R3), the retained
//! region (R12), the overlay split (R6) and the identity of tiles (R11).

use crate::{
    AnyElement, AnyWindowHandle, AppContext as _, AtlasTextureId, Bounds, ContentMask, Context,
    DevicePixels, Div, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    InputEvent as _, InteractiveElement as _, IntoElement, LAYER_IDENTITY_LIMIT, LayerContent,
    LayerFrame, LayerId, MAX_TILE_COORDINATE, MIN_TILE_COORDINATE, Modifiers, ParentElement as _,
    Pixels, Point, Quad, Render, ScaledPixels, Scene, ScrollDelta, ScrollWheelEvent, Stateful,
    StatefulInteractiveElement as _, Styled as _, SurfaceSource, TILE_BUDGET_TILES, TILE_POOL_SIZE,
    TestAppContext, TileCache, TileCoord, TileNeed, TouchPhase, UTF16Selection, Window, anchored,
    black, canvas, deferred, div, edge_faded, is_tile_texture, point, px, rgb, rounded_clip, size,
    snap_path_coordinate, surface, tile_identity, tile_sprite, tile_texture_id, white,
    window::scroll_layers::ScrollLayerProbe,
};
use std::{cell::Cell, ops::Range, rc::Rc, sync::Arc};

const VIEWPORT: (f32, f32) = (300.0, 200.0);
const ROW_HEIGHT: f32 = 40.0;
const ROWS: usize = 120;

pub(crate) struct Lab {
    build: Box<dyn Fn(&mut Window, &mut Context<Lab>) -> AnyElement>,
}

impl Render for Lab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

fn plain_row(ix: usize) -> Stateful<Div> {
    div()
        .id(("row", ix))
        .h(px(ROW_HEIGHT))
        .w_full()
        .bg(if ix.is_multiple_of(2) {
            white()
        } else {
            crate::Hsla::from(rgb(0xf7f7f7))
        })
        .cursor_pointer()
        .child(format!("Item {ix}"))
}

fn scroller() -> Stateful<Div> {
    div()
        .id("scroller")
        .w(px(VIEWPORT.0))
        .h(px(VIEWPORT.1))
        .bg(white())
        .overflow_y_scroll()
}

fn page(scroller: impl IntoElement) -> AnyElement {
    div()
        .size_full()
        .bg(rgb(0xeeeeee))
        .child(scroller)
        .into_any_element()
}

fn rows(count: usize) -> Stateful<Div> {
    scroller().children((0..count).map(plain_row))
}

pub(crate) struct Rig {
    handle: AnyWindowHandle,
}

impl Rig {
    fn new(
        cx: &mut TestAppContext,
        build: impl Fn(&mut Window, &mut Context<Lab>) -> AnyElement + 'static,
    ) -> Self {
        let handle = cx.add_window(|_, _| Lab {
            build: Box::new(build),
        });
        let rig = Self {
            handle: handle.into(),
        };
        rig.draw(cx);
        rig.draw(cx);
        rig
    }

    /// A page with the plain rows of [`rows`].
    fn plain(cx: &mut TestAppContext) -> Self {
        Self::new(cx, |_, _| page(rows(ROWS)))
    }

    fn draw(&self, cx: &mut TestAppContext) {
        cx.update_window(self.handle, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    /// One wheel event; the animation frames content requested come first, in the same frame.
    fn wheel(&self, cx: &mut TestAppContext, dy: f32) {
        cx.update_window(self.handle, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.dispatch_event(
                ScrollWheelEvent {
                    position: point(px(50.), px(50.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
    }

    /// Wheel frames of `dy` each.
    fn scroll(&self, cx: &mut TestAppContext, frames: usize, dy: f32) {
        for _ in 0..frames {
            self.wheel(cx, dy);
        }
    }

    /// Invalidates the owner as an unrelated notification would; the window draws the frame
    /// after the update.
    fn notify(&self, cx: &mut TestAppContext) {
        cx.update_window(self.handle, |view, _, cx| cx.notify(view.entity_id()))
            .unwrap();
    }

    fn probe(&self, cx: &mut TestAppContext) -> ScrollLayerProbe {
        cx.update_window(self.handle, |_, window, _| window.scroll_layer_probe())
            .unwrap()
    }

    fn stats(&self, cx: &mut TestAppContext) -> crate::FrameStats {
        cx.update_window(self.handle, |_, window, _| window.frame_stats())
            .unwrap()
    }

    /// The bounds of the hitboxes under `position`, topmost first.
    fn hits(&self, cx: &mut TestAppContext, x: f32, y: f32) -> Vec<Bounds<Pixels>> {
        cx.update_window(self.handle, |_, window, _| {
            window.hit_test_bounds(point(px(x), px(y)))
        })
        .unwrap()
    }

    /// Scrolls with `dy` per frame until the window holds a layer; returns the frames used.
    fn promote(&self, cx: &mut TestAppContext, dy: f32) -> usize {
        for frame in 1..=6 {
            self.wheel(cx, dy);
            if self.probe(cx).layer {
                return frame;
            }
        }
        panic!("no layer after six scrolling frames: {:?}", self.probe(cx));
    }
}

// ---------------------------------------------------------------------------------------------
// A3: promotion and the fallbacks of R8
// ---------------------------------------------------------------------------------------------

#[crate::test]
fn a3_a_block_is_promoted_after_exactly_two_consecutive_scrolling_frames(cx: &mut TestAppContext) {
    let rig = Rig::plain(cx);
    assert!(!rig.probe(cx).layer);
    rig.wheel(cx, -3.0);
    assert!(!rig.probe(cx).layer, "one scrolling frame");
    rig.wheel(cx, -3.0);
    let probe = rig.probe(cx);
    assert!(probe.layer, "two consecutive scrolling frames: {probe:?}");
    assert_eq!(probe.generation, 1);
    assert_eq!(probe.demotions, 0);
}

#[crate::test]
fn a3_frames_without_scrolling_between_wheel_frames_restart_the_count(cx: &mut TestAppContext) {
    let rig = Rig::plain(cx);
    rig.wheel(cx, -3.0);
    rig.draw(cx);
    rig.wheel(cx, -3.0);
    assert!(
        !rig.probe(cx).layer,
        "the scrolling frames were not consecutive"
    );
    rig.wheel(cx, -3.0);
    assert!(rig.probe(cx).layer, "two consecutive scrolling frames now");
}

#[crate::test]
fn a3_a_wheel_that_moves_nothing_is_no_scrolling_frame(cx: &mut TestAppContext) {
    let rig = Rig::plain(cx);
    // Scrolling up at the top moves nothing.
    for _ in 0..4 {
        rig.wheel(cx, 5.0);
    }
    assert!(!rig.probe(cx).layer);
}

/// Content of rows that no layer may carry, built by `ineligible` for the row `ix`.
fn ineligible_page(
    ineligible: impl Fn(usize) -> Option<AnyElement> + 'static,
) -> impl Fn(&mut Window, &mut Context<Lab>) -> AnyElement + 'static {
    move |_, _| {
        page(scroller().children((0..ROWS).map(|ix| {
            let row = plain_row(ix);
            match ineligible(ix) {
                Some(extra) => row.child(extra),
                None => row,
            }
        })))
    }
}

/// Scrolls a page whose content cannot be cached and returns the probe after every frame.
fn scroll_ineligible(
    cx: &mut TestAppContext,
    build: impl Fn(&mut Window, &mut Context<Lab>) -> AnyElement + 'static,
    frames: usize,
) -> Vec<ScrollLayerProbe> {
    let rig = Rig::new(cx, build);
    (0..frames)
        .map(|_| {
            rig.scroll(cx, 1, -3.0);
            rig.probe(cx)
        })
        .collect()
}

fn assert_never_cached(probes: &[ScrollLayerProbe], what: &str) {
    assert!(
        probes.iter().all(|probe| !probe.layer),
        "{what}: a layer was kept: {probes:#?}"
    );
}

#[crate::test]
fn a3_r8_deferred_content_bypasses_layers(cx: &mut TestAppContext) {
    let probes = scroll_ineligible(
        cx,
        ineligible_page(|ix| {
            ix.is_multiple_of(3)
                .then(|| deferred(div().child("popover")).into_any_element())
        }),
        12,
    );
    assert_never_cached(&probes, "deferred content");
    // The recording failed at the promotion frame: the block waits before it tries again.
    assert!(probes.iter().any(|probe| probe.retry > 0), "{probes:#?}");
}

#[crate::test]
fn a3_r8_anchored_content_bypasses_layers(cx: &mut TestAppContext) {
    let probes = scroll_ineligible(
        cx,
        ineligible_page(|ix| {
            ix.is_multiple_of(4).then(|| {
                deferred(anchored().child(div().w(px(30.)).h(px(10.)).child("menu")))
                    .into_any_element()
            })
        }),
        12,
    );
    assert_never_cached(&probes, "anchored content");
    assert!(probes.iter().any(|probe| probe.retry > 0), "{probes:#?}");
}

#[crate::test]
fn a3_r8_platform_surfaces_bypass_layers(cx: &mut TestAppContext) {
    let probes = scroll_ineligible(
        cx,
        ineligible_page(|ix| {
            ix.is_multiple_of(5).then(|| {
                surface(SurfaceSource::gpu_texture(
                    Arc::new(()),
                    size(DevicePixels(4), DevicePixels(4)),
                ))
                .w(px(20.))
                .h(px(20.))
                .into_any_element()
            })
        }),
        12,
    );
    assert_never_cached(&probes, "a platform surface");
    assert!(probes.iter().any(|probe| probe.retry > 0), "{probes:#?}");
}

#[crate::test]
fn a3_r8_child_prepaint_callbacks_bypass_layers(cx: &mut TestAppContext) {
    let seen = Rc::new(Cell::new(0usize));
    let counter = seen.clone();
    let probes = scroll_ineligible(
        cx,
        move |_, _| {
            let counter = counter.clone();
            page(
                div()
                    .on_children_prepainted(move |bounds, _, _| {
                        counter.set(counter.get() + bounds.len());
                    })
                    .id("scroller")
                    .w(px(VIEWPORT.0))
                    .h(px(VIEWPORT.1))
                    .bg(white())
                    .overflow_y_scroll()
                    .children((0..ROWS).map(plain_row)),
            )
        },
        12,
    );
    assert_never_cached(&probes, "a child-prepaint callback");
    assert!(seen.get() > 0, "the callback ran");
}

#[crate::test]
fn a3_r8_content_that_requests_animation_frames_bypasses_layers(cx: &mut TestAppContext) {
    let probes = scroll_ineligible(
        cx,
        ineligible_page(|ix| {
            (ix == 3).then(|| {
                canvas(
                    |_, _, _| {},
                    |_, _, window, _| window.request_animation_frame(),
                )
                .w(px(10.))
                .h(px(10.))
                .into_any_element()
            })
        }),
        12,
    );
    assert_never_cached(&probes, "content animation");
}

struct Editor {
    focus: FocusHandle,
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let focus = self.focus.clone();
        page(scroller().children((0..ROWS).map(move |ix| {
            let row = plain_row(ix);
            if ix != 4 {
                return row;
            }
            let view = view.clone();
            let focus = focus.clone();
            row.track_focus(&focus.clone()).child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, view.clone()),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
        })))
    }
}

/// The editor takes no part in text services; only the input handler it registers matters.
impl EntityInputHandler for Editor {
    fn character_index_for_point(
        &mut self,
        _position: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn bounds_for_range(
        &mut self,
        _utf16_range: Range<usize>,
        _element: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _replaced: Option<Range<usize>>,
        _text: &str,
        _selection: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn replace_text_in_range(
        &mut self,
        _replaced: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    fn text_for_range(
        &mut self,
        _utf16_range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
}

#[crate::test]
fn a3_r8_a_focused_text_input_in_the_content_bypasses_layers(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, cx| Editor {
        focus: cx.focus_handle(),
    });
    let handle: AnyWindowHandle = window.into();
    let rig = Rig { handle };
    rig.draw(cx);
    // Unfocused, the content has no input handler and is cached.
    rig.scroll(cx, 4, -3.0);
    assert!(
        rig.probe(cx).layer,
        "an unfocused editor is ordinary content"
    );
    let focus: Entity<Editor> = window.root(cx).unwrap();
    cx.update_window(handle, |_, window, cx| {
        let handle = focus.read(cx).focus.clone();
        window.focus(&handle, cx);
    })
    .unwrap();
    let probes: Vec<_> = (0..12)
        .map(|_| {
            rig.scroll(cx, 1, -3.0);
            rig.probe(cx)
        })
        .collect();
    assert_never_cached(&probes, "a focused text input");
}

#[crate::test]
fn a3_r8_inherited_opacity_bypasses_layers(cx: &mut TestAppContext) {
    let probes = scroll_ineligible(
        cx,
        |_, _| {
            div()
                .size_full()
                .bg(rgb(0xeeeeee))
                .child(div().opacity(0.5).child(rows(ROWS)))
                .into_any_element()
        },
        10,
    );
    assert_never_cached(&probes, "an element opacity inherited from an ancestor");
    assert!(
        probes.iter().all(|probe| probe.retry == 0),
        "no failed recording either"
    );
}

#[crate::test]
fn a3_r8_an_inherited_curved_clip_bypasses_layers(cx: &mut TestAppContext) {
    let probes = scroll_ineligible(
        cx,
        |_, _| {
            div()
                .size_full()
                .bg(rgb(0xeeeeee))
                .child(
                    div()
                        .w(px(320.))
                        .h(px(220.))
                        .rounded(px(24.))
                        .overflow_hidden()
                        .curved_overflow_clip()
                        .child(rows(ROWS)),
                )
                .into_any_element()
        },
        10,
    );
    assert_never_cached(&probes, "a curved overflow clip of an ancestor");
}

#[crate::test]
fn a3_r8_a_paint_scope_of_an_ancestor_bypasses_layers(cx: &mut TestAppContext) {
    // A rounded clip and an edge fade cut or fade every primitive on its own. The wrappers
    // declare their scope while prepainting, so the block is never recorded, and no
    // recording fails either.
    let probes = scroll_ineligible(cx, |_, _| page(rounded_clip(px(12.), rows(ROWS))), 10);
    assert_never_cached(&probes, "a rounded clip scope");
    assert!(probes.iter().all(|probe| probe.retry == 0), "{probes:#?}");
    let probes = scroll_ineligible(
        cx,
        |_, _| page(edge_faded(px(16.), true, true, rows(ROWS))),
        10,
    );
    assert_never_cached(&probes, "an edge fade scope");
    assert!(probes.iter().all(|probe| probe.retry == 0), "{probes:#?}");
}

#[crate::test]
fn a3_r8_a_block_without_an_opaque_background_bypasses_layers(cx: &mut TestAppContext) {
    for background in [None, Some(rgb(0xffffff).opacity(0.5)), Some(rgb(0xffffff))] {
        let probes = scroll_ineligible(
            cx,
            move |_, _| {
                let mut scroller = div()
                    .id("scroller")
                    .w(px(VIEWPORT.0))
                    .h(px(VIEWPORT.1))
                    .overflow_y_scroll();
                if let Some(background) = background {
                    scroller = scroller.bg(background);
                }
                page(scroller.children((0..ROWS).map(plain_row)))
            },
            8,
        );
        let opaque = background.is_some_and(|color| color.a >= 1.0);
        assert_eq!(
            probes.last().unwrap().layer,
            opaque,
            "background {background:?}: {probes:#?}"
        );
    }
}

#[crate::test]
fn a3_r8_rounded_blocks_bypass_layers(cx: &mut TestAppContext) {
    let probes = scroll_ineligible(
        cx,
        |_, _| {
            page(
                scroller()
                    .rounded(px(8.))
                    .children((0..ROWS).map(plain_row)),
            )
        },
        8,
    );
    assert_never_cached(&probes, "rounded corners");
}

#[crate::test]
fn a3_r8_an_animation_in_the_owner_outside_the_content_drops_the_layer_without_demoting(
    cx: &mut TestAppContext,
) {
    let animate = Rc::new(Cell::new(false));
    let rig = Rig::new(cx, {
        let animate = animate.clone();
        move |window, _| {
            if animate.get() {
                window.request_animation_frame();
            }
            page(rows(ROWS))
        }
    });
    rig.promote(cx, -3.0);
    animate.set(true);
    rig.notify(cx);
    for _ in 0..6 {
        rig.scroll(cx, 1, -3.0);
        let probe = rig.probe(cx);
        assert!(!probe.layer, "the owner animates: {probe:?}");
        assert_eq!(
            (probe.demotions, probe.cooldown),
            (0, 0),
            "dropped, not demoted"
        );
    }
    animate.set(false);
    rig.notify(cx);
    rig.scroll(cx, 4, -3.0);
    assert!(
        rig.probe(cx).layer,
        "the layer returns once the owner stops animating"
    );
}

#[crate::test]
fn a3_r8_an_animation_in_an_outer_ancestor_keeps_the_layer(cx: &mut TestAppContext) {
    let inner = cx.new(|_| Lab {
        build: Box::new(|_, _| rows(ROWS).into_any_element()),
    });
    let rig = Rig::new(cx, move |window, _| {
        window.request_animation_frame();
        page(inner.clone())
    });
    for _ in 0..12 {
        rig.scroll(cx, 1, -3.0);
    }
    let probe = rig.probe(cx);
    assert!(
        probe.layer,
        "an animation only in the outer view: {probe:?}"
    );
    assert!(rig.stats(cx).layer_composites > 4, "{:?}", rig.stats(cx));
}

#[crate::test]
fn a3_r8_a_failed_recording_waits_eight_frames_before_a_fresh_layer(cx: &mut TestAppContext) {
    let ineligible = Rc::new(Cell::new(true));
    let rig = Rig::new(cx, {
        let ineligible = ineligible.clone();
        move |_, _| {
            let ineligible = ineligible.clone();
            page(scroller().children((0..ROWS).map(move |ix| {
                let row = plain_row(ix);
                if ix != 2 {
                    return row;
                }
                let ineligible = ineligible.clone();
                row.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            if ineligible.get() {
                                window.paint_surface(
                                    bounds,
                                    SurfaceSource::gpu_texture(
                                        Arc::new(()),
                                        size(DevicePixels(2), DevicePixels(2)),
                                    ),
                                );
                            }
                        },
                    )
                    .w(px(10.))
                    .h(px(10.)),
                )
            })))
        }
    });
    // Frame 1 scrolls; frame 2 records and fails.
    rig.scroll(cx, 2, -1.0);
    let failed = rig.probe(cx);
    assert!(!failed.layer);
    assert_eq!(failed.retry, 8, "{failed:?}");
    ineligible.set(false);
    // Eight frames of waiting, then a fresh layer on the ninth.
    for waited in 1..=8 {
        rig.scroll(cx, 1, -1.0);
        let probe = rig.probe(cx);
        assert!(!probe.layer, "frame {waited} after the failure: {probe:?}");
    }
    rig.scroll(cx, 1, -1.0);
    let probe = rig.probe(cx);
    assert!(probe.layer, "the ninth frame retries: {probe:?}");
    assert_eq!(probe.retry, 0);
}

#[crate::test]
fn a3_r8_a_jump_of_a_viewport_per_frame_drops_the_layer_and_inhibits_promotion(
    cx: &mut TestAppContext,
) {
    let rig = Rig::plain(cx);
    rig.promote(cx, -3.0);
    // A jump of one viewport (200 logical px) in one frame drops the layer.
    rig.wheel(cx, -200.0);
    let probe = rig.probe(cx);
    assert!(!probe.layer, "{probe:?}");
    assert_eq!(
        (probe.demotions, probe.cooldown),
        (0, 0),
        "a drop is no demotion"
    );
    // While the motion stays that fast, no layer is made.
    for _ in 0..6 {
        rig.wheel(cx, -210.0);
        assert!(!rig.probe(cx).layer);
    }
    // 199 px per frame is not a jump: promotion needs two slow consecutive frames.
    rig.wheel(cx, -199.0);
    assert!(!rig.probe(cx).layer, "the first slow frame");
    rig.wheel(cx, -199.0);
    assert!(rig.probe(cx).layer, "the second slow frame");
}

#[crate::test]
fn a3_a_resize_tears_the_layer_down(cx: &mut TestAppContext) {
    let rig = Rig::new(cx, |_, _| {
        page(
            div()
                .id("scroller")
                .size_full()
                .bg(white())
                .overflow_y_scroll()
                .children((0..ROWS).map(plain_row)),
        )
    });
    rig.promote(cx, -3.0);
    let before = rig.probe(cx);
    // The retained region is five viewports tall; a viewport of another size shows other tiles.
    cx.simulate_window_resize(rig.handle, size(px(500.), px(300.)));
    rig.draw(cx);
    let after = rig.probe(cx);
    assert!(after.layer, "scrolling goes on in the new size: {after:?}");
    assert_ne!(
        after.instance, before.instance,
        "a fresh layer, not the old one resized"
    );
    assert_eq!(after.generation, 1, "its content generation starts over");
    assert_eq!(
        (after.demotions, after.cooldown),
        (0, 0),
        "a resize is no demotion"
    );
}

#[crate::test]
fn a3_a_scale_factor_change_tears_the_layer_down(cx: &mut TestAppContext) {
    let rig = Rig::plain(cx);
    rig.promote(cx, -3.0);
    let before = rig.probe(cx);
    cx.simulate_window_scale_factor_change(rig.handle, 1.5);
    rig.draw(cx);
    let after = rig.probe(cx);
    assert!(after.layer);
    assert_ne!(after.instance, before.instance);
    assert_eq!(after.generation, 1);
    assert_eq!((after.demotions, after.cooldown), (0, 0));
}

#[crate::test]
fn a3_a_layer_whose_block_left_the_tree_is_dropped_and_its_tiles_retired(cx: &mut TestAppContext) {
    let shown = Rc::new(Cell::new(true));
    let rig = Rig::new(cx, {
        let shown = shown.clone();
        move |_, _| {
            if shown.get() {
                page(rows(ROWS))
            } else {
                page(div())
            }
        }
    });
    rig.promote(cx, -3.0);
    let live = cx
        .update_window(rig.handle, |_, window, _| {
            window
                .painted_scene()
                .layers
                .frames()
                .first()
                .map(|frame| (frame.id(), frame.instance()))
        })
        .unwrap()
        .expect("a layer frame");
    shown.set(false);
    rig.notify(cx);
    assert!(!rig.probe(cx).layer);
    let retired = cx
        .update_window(rig.handle, |_, window, _| {
            window.painted_scene().layers.retired().to_vec()
        })
        .unwrap();
    assert_eq!(retired, [live], "the renderer is told to release the tiles");
}

#[test]
fn a3_a_layer_not_composited_for_120_frames_is_inactive() {
    use crate::window::scroll_layers::inactive;
    assert!(!inactive(119, 0));
    assert!(inactive(120, 0));
    assert!(inactive(1000, 10));
    assert!(!inactive(129, 10));
}

/// A page whose scroller is `width` x `height` logical px, with rows as wide as the scroller
/// and 20 times the usual height.
fn huge_page(width: f32, height: f32) -> impl Fn(&mut Window, &mut Context<Lab>) -> AnyElement {
    move |_, _| {
        let tall = (0..60).map(|ix| plain_row(ix).h(px(ROW_HEIGHT * 20.)).w(px(width)));
        let big = scroller().w(px(width)).h(px(height)).children(tall);
        div().size_full().child(big).into_any_element()
    }
}

#[crate::test]
fn a3_budget_a_visible_tile_set_over_64_tiles_bypasses_the_layer(cx: &mut TestAppContext) {
    // 4,100 x 1,100 logical px at scale factor 2 (8,200 x 2,200 device px): 17 x 5 = 85 visible
    // tiles, more than the 64 the memory budget holds.
    let rig = Rig::new(cx, huge_page(4100., 1100.));
    let mut probes = Vec::new();
    for _ in 0..12 {
        rig.scroll(cx, 1, -3.0);
        probes.push(rig.probe(cx));
    }
    assert_never_cached(&probes, "a visible set over the budget");
    assert!(probes.iter().any(|probe| probe.retry > 0), "{probes:#?}");
    // The same content in a window that fits the budget is cached.
    let rig = Rig::new(cx, huge_page(2000., 600.));
    rig.promote(cx, -3.0);
}

#[crate::test]
fn a3_budget_two_blocks_share_the_window_budget(cx: &mut TestAppContext) {
    // Each block alone shows 8 x 4 = 32 tiles at most (plus a straddling row and column);
    // the sum of two exceeds 64, so only one of them is cached.
    let rig = Rig::new(cx, |_, _| {
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("left")
                    .w(px(1900.))
                    .h(px(500.))
                    .bg(white())
                    .overflow_y_scroll()
                    .children((0..40).map(|ix| plain_row(ix).w(px(1900.)))),
            )
            .child(
                div()
                    .id("right")
                    .w(px(1900.))
                    .h(px(500.))
                    .bg(white())
                    .overflow_y_scroll()
                    .children((0..40).map(|ix| plain_row(ix).w(px(1900.)))),
            )
            .into_any_element()
    });
    let mut layers = 0;
    for _ in 0..8 {
        rig.wheel(cx, -3.0);
        layers = cx
            .update_window(rig.handle, |_, window, _| window.scroll_layer_count())
            .unwrap();
    }
    assert!(
        layers <= 1,
        "the budget of 64 tiles holds one of them: {layers}"
    );
    assert!(TILE_BUDGET_TILES == 64);
}

// R9 and R10 through whole frames --------------------------------------------------------------

fn counted_page(value: Rc<Cell<u32>>) -> impl Fn(&mut Window, &mut Context<Lab>) -> AnyElement {
    move |_, _| {
        let value = value.get();
        page(scroller().children((0..ROWS).map(move |ix| {
            div()
                .id(("row", ix))
                .h(px(ROW_HEIGHT))
                .w_full()
                .bg(white())
                .child(format!("Item {ix} v{}", value + ix as u32 % 3))
        })))
    }
}

/// Writes a new value into the content and draws the frame it invalidates.
fn write(cx: &mut TestAppContext, rig: &Rig, value: &Rc<Cell<u32>>) {
    value.set(value.get() + 1);
    rig.notify(cx);
}

#[crate::test]
fn a3_r9_two_expensive_refreshes_within_120_frames_demote(cx: &mut TestAppContext) {
    let value = Rc::new(Cell::new(0));
    let rig = Rig::new(cx, counted_page(value.clone()));
    rig.promote(cx, -3.0);
    write(cx, &rig, &value);
    assert!(rig.probe(cx).layer, "one refresh keeps the layer");
    for _ in 0..20 {
        rig.draw(cx);
    }
    write(cx, &rig, &value);
    let probe = rig.probe(cx);
    assert!(
        !probe.layer,
        "the second refresh within 120 frames demotes: {probe:?}"
    );
    assert_eq!((probe.demotions, probe.cooldown), (1, 60));
}

#[crate::test]
fn a3_r9_expensive_refreshes_more_than_120_frames_apart_do_not_demote(cx: &mut TestAppContext) {
    let value = Rc::new(Cell::new(0));
    let rig = Rig::new(cx, counted_page(value.clone()));
    rig.promote(cx, -3.0);
    for _ in 0..3 {
        write(cx, &rig, &value);
        for _ in 0..125 {
            rig.draw(cx);
            // Keep the layer alive: composite it now and then.
            if rig.probe(cx).layer {
                rig.wheel(cx, -0.1);
            }
        }
        assert!(rig.probe(cx).layer, "{:?}", rig.probe(cx));
    }
}

#[crate::test]
fn a3_r9_scrolling_that_repaints_the_region_too_often_demotes(cx: &mut TestAppContext) {
    // 37.3 logical px per frame leaves the retained region every few frames: each repaint is
    // worth ten direct draws, and the cost window demotes the layer.
    let rig = Rig::plain(cx);
    let mut demoted_at = None;
    for frame in 1..=120 {
        rig.wheel(cx, -37.3);
        let probe = rig.probe(cx);
        if probe.demotions > 0 {
            demoted_at = Some((frame, probe));
            break;
        }
    }
    let (frame, probe) = demoted_at.expect("the layer was demoted");
    assert!(frame < 60, "demoted after {frame} frames");
    assert!(!probe.layer);
    assert_eq!(probe.cooldown, 60);
    assert!(rig.stats(cx).layer_repaints >= 3, "{:?}", rig.stats(cx));
}

#[crate::test]
fn a3_r10_the_cooldown_doubles_with_every_demotion_and_layer_identities_are_recycled(
    cx: &mut TestAppContext,
) {
    let value = Rc::new(Cell::new(0));
    let rig = Rig::new(cx, counted_page(value.clone()));
    rig.promote(cx, -3.0);
    let first = rig.probe(cx);
    let mut identities = vec![first.instance];
    for (round, cooldown) in [60u32, 120, 240].into_iter().enumerate() {
        // Two broad updates in a row demote the layer.
        write(cx, &rig, &value);
        write(cx, &rig, &value);
        let demoted = rig.probe(cx);
        assert!(!demoted.layer, "round {round}: {demoted:?}");
        assert_eq!(demoted.cooldown, cooldown, "round {round}: {demoted:?}");
        // Scrolling all the while does not bring the layer back before the cooldown ends ...
        for frame in 1..cooldown {
            rig.scroll(cx, 1, if frame.is_multiple_of(2) { -3.0 } else { 3.0 });
            assert!(
                !rig.probe(cx).layer,
                "round {round}: a layer after {frame} of {cooldown} frames"
            );
        }
        // ... and it does exactly when it ends.
        rig.scroll(cx, 1, -3.0);
        let promoted = rig.probe(cx);
        assert!(
            promoted.layer,
            "round {round}: no layer after the cooldown: {promoted:?}"
        );
        identities.push(promoted.instance);
    }
    identities.sort();
    identities.dedup();
    assert_eq!(identities.len(), 4, "every layer is a new instance");
}

#[crate::test]
fn a3_r10_an_update_during_the_cooldown_extends_it_by_sixty_frames(cx: &mut TestAppContext) {
    let value = Rc::new(Cell::new(0));
    let rig = Rig::new(cx, counted_page(value.clone()));
    rig.promote(cx, -3.0);
    write(cx, &rig, &value);
    write(cx, &rig, &value);
    assert_eq!(rig.probe(cx).cooldown, 60);
    for _ in 0..10 {
        rig.scroll(cx, 1, -3.0);
    }
    write(cx, &rig, &value);
    let extended = rig.probe(cx);
    assert_eq!(extended.cooldown, 60 - 10 + 60 - 1, "{extended:?}");
    for _ in 0..extended.cooldown - 1 {
        rig.scroll(cx, 1, -3.0);
        assert!(!rig.probe(cx).layer);
    }
    rig.scroll(cx, 1, -3.0);
    assert!(rig.probe(cx).layer);
}

#[crate::test]
fn a3_r10_1920_quiet_frames_reset_the_backoff(cx: &mut TestAppContext) {
    let value = Rc::new(Cell::new(0));
    let rig = Rig::new(cx, counted_page(value.clone()));
    rig.promote(cx, -3.0);
    for _ in 0..2 {
        write(cx, &rig, &value);
    }
    assert_eq!(rig.probe(cx).demotions, 1);
    for _ in 0..1900 {
        rig.draw(cx);
    }
    assert_eq!(rig.probe(cx).demotions, 1, "not yet");
    for _ in 0..30 {
        rig.draw(cx);
    }
    assert_eq!(rig.probe(cx).demotions, 0, "reset");
    assert_eq!(rig.probe(cx).cooldown, 0);
}

// ---------------------------------------------------------------------------------------------
// A3 (generation skips, budget), A8 (tile identity) on the tile cache
// ---------------------------------------------------------------------------------------------

fn block_content(width: f32, height: f32, shade: f32) -> Arc<LayerContent> {
    let mut scene = Scene::default();
    let bounds = Bounds {
        origin: point(ScaledPixels(0.), ScaledPixels(0.)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    };
    scene.insert_primitive(Quad {
        bounds,
        content_mask: ContentMask { bounds },
        background: crate::hsla(0., 0., shade, 1.).into(),
        ..Quad::default()
    });
    let region = Bounds {
        origin: point(DevicePixels(0), DevicePixels(0)),
        size: size(DevicePixels(width as i32), DevicePixels(height as i32)),
    };
    let zero = point(ScaledPixels(0.), ScaledPixels(0.));
    Arc::new(LayerContent::from_recording(&scene, zero, region).expect("plain content"))
}

fn tiles(width: i32, height: i32) -> Vec<TileCoord> {
    let mut all = Vec::new();
    for y in 0..height {
        for x in 0..width {
            all.push(TileCoord::new(x, y).unwrap());
        }
    }
    all
}

fn frame_of(
    id: u32,
    instance: u64,
    generation: u64,
    content: &Arc<LayerContent>,
    shown: &[TileCoord],
    dirty: &[TileCoord],
) -> LayerFrame {
    LayerFrame::new(
        LayerId::new(id).unwrap(),
        instance,
        generation,
        white().into(),
        content.clone(),
        shown.to_vec(),
        dirty.to_vec(),
    )
}

/// Rasterizes what `cache` needs for `frame` into numbered textures; returns the tiles rasterized.
fn show(cache: &mut TileCache<u32>, frame: &LayerFrame, next_texture: &mut u32) -> Vec<TileCoord> {
    cache.begin_frame();
    let mut rasterized = Vec::new();
    for coord in frame.shown() {
        if cache.need(frame, *coord) == TileNeed::Rasterize {
            let texture = cache.take_pooled().unwrap_or_else(|| {
                *next_texture += 1;
                *next_texture
            });
            cache.store(frame, *coord, texture);
            rasterized.push(*coord);
        }
    }
    cache.end_frame(&[]);
    rasterized
}

#[test]
fn a3_a_cache_that_missed_a_generation_rasterizes_again() {
    let content = block_content(1024., 1024., 0.5);
    let shown = tiles(2, 2);
    let mut cache = TileCache::<u32>::default();
    let mut textures = 0;
    // Generation 1 rasterizes every tile.
    let first = frame_of(0, 1, 1, &content, &shown, &shown);
    assert_eq!(show(&mut cache, &first, &mut textures).len(), 4);
    // The next generation with the same content and no dirty tile reuses all of them.
    let second = frame_of(0, 1, 2, &content, &shown, &[]);
    assert!(show(&mut cache, &second, &mut textures).is_empty());
    // A cache entry two generations behind is not provably current: it rasterizes again, though
    // the frame reports nothing dirty and the content is the same.
    let fourth = frame_of(0, 1, 4, &content, &shown, &[]);
    assert_eq!(show(&mut cache, &fourth, &mut textures).len(), 4);
    // Generation 5 follows generation 4: reuse again.
    let fifth = frame_of(0, 1, 5, &content, &shown, &[]);
    assert!(show(&mut cache, &fifth, &mut textures).is_empty());
}

#[test]
fn a3_a_tile_whose_content_changed_rasterizes_and_the_rest_reuse() {
    let shown = tiles(2, 1);
    let mut cache = TileCache::<u32>::default();
    let mut textures = 0;
    let first = block_content(1024., 512., 0.5);
    show(
        &mut cache,
        &frame_of(0, 1, 1, &first, &shown, &shown),
        &mut textures,
    );
    // A different shade changes both tiles: the hash differs, so they rasterize although the
    // frame says nothing was dirty.
    let second = block_content(1024., 512., 0.25);
    let changed = show(
        &mut cache,
        &frame_of(0, 1, 2, &second, &shown, &[]),
        &mut textures,
    );
    assert_eq!(changed, shown);
    // Dirty tiles of the same generation are rasterized once per generation.
    let third = frame_of(0, 1, 3, &second, &shown, &shown[..1]);
    assert_eq!(show(&mut cache, &third, &mut textures), shown[..1].to_vec());
    assert!(
        show(&mut cache, &third, &mut textures).is_empty(),
        "the same frame drawn again"
    );
}

#[test]
fn a3_tiles_the_frame_needs_are_never_evicted_even_over_the_budget() {
    let shown = tiles(4, 2);
    let content = block_content(2048., 1024., 0.5);
    let mut cache = TileCache::<u32>::new(3);
    let mut textures = 0;
    let frame = frame_of(0, 1, 1, &content, &shown, &shown);
    assert_eq!(show(&mut cache, &frame, &mut textures).len(), 8);
    assert_eq!(
        cache.len(),
        8,
        "the visible set alone exceeds the budget and stays"
    );
    // The next frame shows other tiles: the unused ones are evicted down to the budget.
    let other = tiles(2, 1);
    let next = frame_of(0, 1, 2, &content, &other, &[]);
    show(&mut cache, &next, &mut textures);
    assert_eq!(cache.len(), 3);
    for coord in &other {
        assert!(
            cache.texture(LayerId::new(0).unwrap(), *coord).is_some(),
            "{coord:?}"
        );
    }
}

#[test]
fn a3_the_default_budget_is_64_mebibytes_of_tiles() {
    assert_eq!(crate::TILE_BYTES, 512 * 512 * 4);
    assert_eq!(crate::TILE_BUDGET_BYTES, 64 * 1024 * 1024);
    assert_eq!(TILE_BUDGET_TILES, 64);
    let cache = TileCache::<u32>::default();
    assert_eq!(cache.bytes(), 0);
}

#[test]
fn a8_tiles_are_keyed_by_layer_identity_and_tile_coordinate() {
    let content = block_content(1024., 512., 0.5);
    let shown = tiles(2, 1);
    let mut cache = TileCache::<u32>::default();
    let mut textures = 0;
    show(
        &mut cache,
        &frame_of(0, 1, 1, &content, &shown, &shown),
        &mut textures,
    );
    show(
        &mut cache,
        &frame_of(1, 2, 1, &content, &shown, &shown),
        &mut textures,
    );
    assert_eq!(
        textures, 4,
        "two layers do not share textures for equal coordinates"
    );
    let (a, b) = (LayerId::new(0).unwrap(), LayerId::new(1).unwrap());
    let mut seen = Vec::new();
    for layer in [a, b] {
        for coord in &shown {
            seen.push(*cache.texture(layer, *coord).expect("cached"));
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 4);
    assert!(cache.texture(a, TileCoord::new(5, 5).unwrap()).is_none());
}

#[test]
fn a8_a_recycled_layer_identity_does_not_inherit_stale_textures_or_generations() {
    let content = block_content(1024., 512., 0.5);
    let shown = tiles(2, 1);
    let mut cache = TileCache::<u32>::default();
    let mut textures = 0;
    show(
        &mut cache,
        &frame_of(7, 1, 1, &content, &shown, &shown),
        &mut textures,
    );
    show(
        &mut cache,
        &frame_of(7, 1, 2, &content, &shown, &[]),
        &mut textures,
    );
    let id = LayerId::new(7).unwrap();
    assert_eq!(cache.generation_of(id, shown[0]), Some(2));
    // The same identity, another layer: generation 3 would follow generation 2 of the old
    // layer with an equal hash, which must not be mistaken for a reusable tile.
    let recycled = frame_of(7, 2, 3, &content, &shown, &[]);
    let rasterized = show(&mut cache, &recycled, &mut textures);
    assert_eq!(
        rasterized, shown,
        "nothing carries over to the recycled identity"
    );
    assert_eq!(cache.generation_of(id, shown[0]), Some(3));
    // The old instance retiring afterwards must not drop the new layer's tiles.
    cache.begin_frame();
    cache.end_frame(&[(id, 1)]);
    assert!(cache.texture(id, shown[0]).is_some());
    cache.begin_frame();
    cache.end_frame(&[(id, 2)]);
    assert!(cache.texture(id, shown[0]).is_none());
}

#[test]
fn a8_at_most_eight_freed_textures_are_pooled() {
    let content = block_content(4096., 1024., 0.5);
    let shown = tiles(8, 2);
    let mut cache = TileCache::<u32>::default();
    let mut textures = 0;
    show(
        &mut cache,
        &frame_of(3, 1, 1, &content, &shown, &shown),
        &mut textures,
    );
    assert_eq!(cache.len(), 16);
    assert_eq!(cache.pooled(), 0);
    cache.begin_frame();
    cache.end_frame(&[(LayerId::new(3).unwrap(), 1)]);
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.pooled(), TILE_POOL_SIZE);
    assert_eq!(TILE_POOL_SIZE, 8);
    // New tiles take pooled textures before new ones are made.
    let before = textures;
    show(
        &mut cache,
        &frame_of(4, 2, 1, &content, &shown[..8], &shown[..8]),
        &mut textures,
    );
    assert_eq!(textures, before, "eight pooled textures served eight tiles");
    assert_eq!(cache.pooled(), 0);
}

#[test]
fn a8_tile_sprites_carry_a_texture_identity_that_is_never_an_atlas_texture() {
    let mask = ContentMask {
        bounds: Bounds {
            origin: point(ScaledPixels(0.), ScaledPixels(0.)),
            size: size(ScaledPixels(100.), ScaledPixels(100.)),
        },
    };
    let bounds = mask.bounds;
    let a = LayerId::new(5).unwrap();
    let b = LayerId::new(6).unwrap();
    let coord = TileCoord::new(-3, 4).unwrap();
    let sprite = tile_sprite(a, coord, bounds, mask);
    assert!(is_tile_texture(sprite.tile.texture_id));
    assert_eq!(tile_identity(&sprite.tile), Some((a, coord)));
    assert_ne!(sprite.tile, tile_sprite(b, coord, bounds, mask).tile);
    assert_ne!(
        sprite.tile,
        tile_sprite(a, TileCoord::new(-3, 5).unwrap(), bounds, mask).tile
    );
    assert!(!is_tile_texture(AtlasTextureId {
        index: 5,
        kind: crate::AtlasTextureKind::Polychrome,
    }));
    assert!(is_tile_texture(tile_texture_id(a)));
}

#[test]
fn a8_identities_and_tile_coordinates_stay_in_their_domains() {
    assert!(LayerId::new(LAYER_IDENTITY_LIMIT - 1).is_some());
    assert!(LayerId::new(LAYER_IDENTITY_LIMIT).is_none());
    assert_eq!(LAYER_IDENTITY_LIMIT, 16_777_216);
    assert_eq!((MIN_TILE_COORDINATE, MAX_TILE_COORDINATE), (-2048, 2047));
    assert!(TileCoord::new(-2048, 2047).is_some());
    assert!(TileCoord::new(-2049, 0).is_none());
    assert!(TileCoord::new(0, 2048).is_none());
    for (x, y) in [(-2048, -2048), (-1, 0), (0, -1), (2047, 2047), (17, -300)] {
        let coord = TileCoord::new(x, y).unwrap();
        assert_eq!(TileCoord::unpack(coord.pack()), coord);
        assert!(coord.pack() < 1 << 24);
    }
}

// ---------------------------------------------------------------------------------------------
// A4: a click right after scrolling
// ---------------------------------------------------------------------------------------------

#[crate::test]
fn a4_a_click_right_after_scrolling_reaches_the_item_under_the_pointer(cx: &mut TestAppContext) {
    use crate::scroll_layer_tests::Fixture;
    let layered = Fixture::new(cx, 60, true, false, false);
    let direct = Fixture::new(cx, 60, false, false, false);
    for fixture in [&layered, &direct] {
        fixture.move_pointer(cx, 100., 100.);
    }
    // A run of wheel frames, the pointer unmoved: the layer composites at the new offsets.
    for _ in 0..9 {
        layered.wheel(cx, -7.3);
        direct.wheel(cx, -7.3);
    }
    let before = layered.stats(cx);
    assert!(
        before.layer_composites >= 5,
        "the layer composited: {before:?}"
    );
    for fixture in [&layered, &direct] {
        fixture.click(cx, 100., 100.);
    }
    let stats = layered.stats(cx);
    assert!(
        stats.input_layer_rebuilds > before.input_layer_rebuilds,
        "the layer was repainted at current coordinates before the click: {stats:?}"
    );
    let clicked = layered.log.borrow().clone();
    let expected = direct.log.borrow().clone();
    assert!(!expected.is_empty(), "the click reached an item");
    assert_eq!(
        clicked, expected,
        "the same item and coordinates as direct drawing"
    );
    // The pointer is over the row whose top the layer reports at current coordinates.
    let under_pointer = layered.hits(cx);
    assert!(!under_pointer.is_empty());
}

#[crate::test]
fn a4_scroll_bound_getters_report_current_window_coordinates(cx: &mut TestAppContext) {
    use crate::scroll_layer_tests::Fixture;
    let layered = Fixture::new(cx, 60, true, false, false);
    let direct = Fixture::new(cx, 60, false, false, false);
    for _ in 0..8 {
        layered.wheel(cx, -9.1);
        direct.wheel(cx, -9.1);
        assert_eq!(layered.hits(cx), direct.hits(cx));
    }
    assert!(layered.stats(cx).layer_composites > 3);
}

// ---------------------------------------------------------------------------------------------
// A5: snapping
// ---------------------------------------------------------------------------------------------

/// The top of the row under the top edge of the viewport, in logical px: the content's
/// placement.
fn row_top(rig: &Rig, cx: &mut TestAppContext) -> f32 {
    let hits = rig.hits(cx, 50., 1.);
    // Hitboxes topmost first: the row, then the scroller.
    hits.iter()
        .find(|bounds| bounds.size.height == px(ROW_HEIGHT))
        .map(|bounds| bounds.origin.y.0)
        .expect("a row under the pointer")
}

#[crate::test]
fn a5_scroll_offsets_snap_to_whole_device_pixels_with_layers_on_and_off(cx: &mut TestAppContext) {
    // 0.4 logical px at scale factor 2 is 0.8 device px per frame: the unsnapped accumulation
    // is 0.8, 1.6, 2.4, 3.2, 4.0 device px and the content sits at 1, 2, 2, 3, 4.
    for layers in [true, false] {
        let handle = crate::ScrollHandle::new();
        let rig = {
            let handle = handle.clone();
            Rig::new(cx, move |_, _| page(rows(ROWS).track_scroll(&handle)))
        };
        cx.update_window(rig.handle, |_, window, _| {
            window.set_scroll_layers_enabled(layers)
        })
        .unwrap();
        let mut placed = Vec::new();
        let mut stored = Vec::new();
        for _ in 0..5 {
            rig.wheel(cx, -0.4);
            placed.push((-row_top(&rig, cx) * 2.0).round() as i32);
            stored.push(handle.offset().y.0 * 2.0);
        }
        assert_eq!(placed, [1, 2, 2, 3, 4], "layers {layers}");
        for (step, (stored, expected)) in stored
            .iter()
            .zip([-0.8, -1.6, -2.4, -3.2, -4.0])
            .enumerate()
        {
            assert!(
                (stored - expected).abs() < 1e-4,
                "layers {layers}: the stored offset keeps sub-pixel motion at step {step}: {stored}"
            );
        }
    }
}

#[crate::test]
fn a5_halves_snap_toward_zero(cx: &mut TestAppContext) {
    for layers in [true, false] {
        // 0.25 logical px is 0.5 device px: it snaps to 0; 0.75 logical px (1.5 device px)
        // snaps to 1 (one device pixel is half a logical pixel).
        for (wheel, device) in [(-0.25f32, 0), (-0.75, 1), (-1.25, 2), (-1.75, 3)] {
            let rig = Rig::plain(cx);
            cx.update_window(rig.handle, |_, window, _| {
                window.set_scroll_layers_enabled(layers)
            })
            .unwrap();
            rig.wheel(cx, wheel);
            let placed = (-row_top(&rig, cx) * 2.0).round() as i32;
            // 0.5 -> 0, 1.5 -> 1, 2.5 -> 2, 3.5 -> 3: halves go toward zero.
            assert_eq!(placed, device, "layers {layers}, wheel {wheel}");
        }
    }
}

#[test]
fn a5_path_coordinates_snap_to_1_256_device_pixel_with_ties_to_even() {
    let step = 1.0 / 256.0;
    assert_eq!(snap_path_coordinate(0.0), 0.0);
    assert_eq!(snap_path_coordinate(7.0 * step), 7.0 * step);
    assert_eq!(snap_path_coordinate(7.3 * step), 7.0 * step);
    assert_eq!(snap_path_coordinate(7.7 * step), 8.0 * step);
    // Exact ties go to the even step.
    for (tie, even) in [
        (0.5, 0.0),
        (1.5, 2.0),
        (2.5, 2.0),
        (3.5, 4.0),
        (254.5, 254.0),
        (255.5, 256.0),
    ] {
        assert_eq!(snap_path_coordinate(tie * step), even * step, "tie {tie}");
    }
    for (tie, even) in [(-0.5, 0.0), (-1.5, -2.0), (-2.5, -2.0), (-3.5, -4.0)] {
        assert_eq!(snap_path_coordinate(tie * step), even * step, "tie {tie}");
    }
    // A whole device pixel offset leaves the grid position unchanged.
    for offset in [1.0, 81.0, -3.0, 4096.0] {
        let moved = snap_path_coordinate(5.37 + offset);
        assert_eq!(moved, snap_path_coordinate(5.37) + offset);
    }
}

#[crate::test]
fn a5_path_vertices_bounds_and_masks_land_on_the_grid(cx: &mut TestAppContext) {
    for layers in [true, false] {
        let rig = Rig::new(cx, |_, _| {
            page(
                scroller()
                    .child(
                        canvas(
                            |_, _, _| {},
                            |bounds, _, window, _| {
                                let mut stroke = crate::PathBuilder::stroke(px(1.));
                                stroke.move_to(point(
                                    bounds.origin.x + px(10.3),
                                    bounds.origin.y + px(5.7),
                                ));
                                stroke.line_to(point(
                                    bounds.origin.x + px(40.123),
                                    bounds.origin.y + px(9.31),
                                ));
                                stroke.line_to(point(
                                    bounds.origin.x + px(60.77),
                                    bounds.origin.y + px(3.9),
                                ));
                                window.paint_path(stroke.build().expect("path"), black());
                            },
                        )
                        .w(px(100.))
                        .h(px(30.)),
                    )
                    .children((0..ROWS).map(plain_row)),
            )
        });
        cx.update_window(rig.handle, |_, window, _| {
            window.set_scroll_layers_enabled(layers);
        })
        .unwrap();
        for _ in 0..4 {
            rig.wheel(cx, -0.9);
        }
        for scale in [1.0, 1.5, 2.0] {
            cx.simulate_window_scale_factor_change(rig.handle, scale);
            rig.draw(cx);
            rig.wheel(cx, -0.37);
            let paths = cx
                .update_window(rig.handle, |_, window, _| {
                    window.painted_scene().paths.clone()
                })
                .unwrap();
            assert!(!paths.is_empty(), "layers {layers}, scale {scale}");
            let on_grid = |value: f32| (value * 256.0 - (value * 256.0).round()).abs() < 1e-3;
            for path in paths {
                for vertex in &path.vertices {
                    assert!(
                        on_grid(vertex.xy_position.x.0) && on_grid(vertex.xy_position.y.0),
                        "layers {layers}, scale {scale}: {vertex:?}"
                    );
                }
                for edge in [
                    path.bounds.origin.x.0,
                    path.bounds.origin.y.0,
                    path.bounds.origin.x.0 + path.bounds.size.width.0,
                    path.bounds.origin.y.0 + path.bounds.size.height.0,
                ] {
                    assert!(
                        on_grid(edge),
                        "layers {layers}, scale {scale}: bounds edge {edge}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A6: the retained region
// ---------------------------------------------------------------------------------------------

#[crate::test]
fn a6_no_repaint_while_the_padded_viewport_stays_inside_the_retained_region(
    cx: &mut TestAppContext,
) {
    // Scale factor 2: the viewport is 400 device px tall, so the margin is 100 and the
    // overscan 800 above and below; 120 rows are 9,600 device px of content.
    let rig = Rig::plain(cx);
    rig.promote(cx, -10.0);
    let promoted = rig.probe(cx);
    let (_, top, _, bottom) = promoted.region.expect("region");
    let (mut recorded, mut bottom) = (-promoted.recorded_scroll.expect("scroll").1, bottom);
    assert_eq!(recorded, 40, "two 10 px frames at scale factor 2");
    assert_eq!(
        (top, bottom),
        (0, recorded + 400 + 800),
        "the viewport with two viewports below it, clamped at the top of the content"
    );
    let mut repaints = rig.stats(cx).layer_repaints;
    let mut offset = recorded;
    let mut painted_at = Vec::new();
    for _ in 0..3 {
        for frame in 0.. {
            assert!(frame < 80, "no repaint in 80 frames");
            rig.wheel(cx, -10.0);
            offset += 20;
            let probe = rig.probe(cx);
            let now = -probe.recorded_scroll.expect("scroll").1;
            if rig.stats(cx).layer_repaints > repaints {
                // The first frame the padded viewport does not fit repaints, around the
                // current viewport with the full overscan.
                assert_eq!(now, offset, "recorded at the current offset");
                assert!(
                    offset + 400 + 100 > bottom,
                    "{offset} did not need a repaint"
                );
                let (_, new_top, _, new_bottom) = probe.region.expect("region");
                assert_eq!(new_top, (offset - 800).max(0), "two viewports above");
                assert_eq!(new_bottom, offset + 400 + 800, "two viewports below");
                recorded = now;
                bottom = new_bottom;
                repaints += 1;
                painted_at.push(offset);
                break;
            }
            assert_eq!(now, recorded, "composite frames keep the recorded content");
            assert!(
                offset + 400 + 100 <= bottom,
                "{offset} fits the region, no repaint"
            );
        }
    }
    // Each repaint came one margin short of the region's lower edge.
    assert!(
        painted_at.windows(2).all(|pair| pair[1] > pair[0] + 500),
        "{painted_at:?}"
    );
}

#[crate::test]
fn a6_regions_clamp_at_the_content_edges(cx: &mut TestAppContext) {
    // 12 rows of 40 px at scale factor 2: 960 device px of content in a 400 px viewport.
    let rig = Rig::new(cx, |_, _| page(rows(12)));
    rig.promote(cx, -10.0);
    let probe = rig.probe(cx);
    let (left, top, right, bottom) = probe.region.expect("region");
    assert_eq!(
        (left, top, right),
        (0, 0, 600),
        "a vertical scroller keeps its width"
    );
    assert_eq!(
        bottom, 960,
        "clamped to the content: the overscan would reach 1,640"
    );
    // Scrolled to the end, the padded viewport clamps at the content and fits: no repaint.
    let repaints = rig.stats(cx).layer_repaints;
    for _ in 0..40 {
        rig.wheel(cx, -10.0);
    }
    let end = rig.probe(cx);
    assert_eq!(
        rig.stats(cx).layer_repaints,
        repaints,
        "a layer spanning the content never repaints for scrolling"
    );
    assert!(end.layer);
}

#[crate::test]
fn a6_a_repaint_around_the_viewport_clamps_the_overscan_to_the_content(cx: &mut TestAppContext) {
    // 60 rows: 4,800 device px of content. Scrolled near the end, the lower overscan is cut.
    let rig = Rig::new(cx, |_, _| page(rows(60)));
    rig.promote(cx, -10.0);
    let mut clamped = None;
    for _ in 0..300 {
        rig.wheel(cx, -10.0);
        let probe = rig.probe(cx);
        let (_, _, _, bottom) = probe.region.expect("a layer all along");
        if bottom == 4800 && probe.recorded_scroll.unwrap().1 != 0 {
            clamped = Some(probe);
            break;
        }
    }
    let probe = clamped.expect("a repaint reached the end of the content");
    let (_, top, _, bottom) = probe.region.expect("region");
    let scroll = -probe.recorded_scroll.expect("scroll").1;
    assert_eq!(bottom, 4800, "clamped at the end of the content");
    assert_eq!(top, scroll - 800, "the upper overscan is whole");
    assert!(
        scroll + 400 + 800 > 4800,
        "the unclamped region would reach past the content"
    );
}

// ---------------------------------------------------------------------------------------------
// A7: the overlay split
// ---------------------------------------------------------------------------------------------

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

fn quad_at(bounds: Bounds<ScaledPixels>, shade: f32) -> Quad {
    Quad {
        bounds,
        content_mask: ContentMask {
            bounds: rect(-10000., -10000., 20000., 20000.),
        },
        background: crate::hsla(0., 0., shade, 1.).into(),
        ..Quad::default()
    }
}

fn path_over(bounds: Bounds<ScaledPixels>) -> crate::Path<ScaledPixels> {
    let mut builder = crate::PathBuilder::stroke(px(1.));
    builder.move_to(point(px(bounds.origin.x.0), px(bounds.origin.y.0)));
    builder.line_to(point(
        px(bounds.origin.x.0 + bounds.size.width.0),
        px(bounds.origin.y.0 + bounds.size.height.0),
    ));
    let mut path = builder.build().expect("path").scale(1.0);
    path.content_mask = ContentMask {
        bounds: rect(-10000., -10000., 20000., 20000.),
    };
    path
}

fn shades(scene: &Scene) -> Vec<f32> {
    scene
        .paint_operations
        .iter()
        .filter_map(|operation| match operation {
            crate::scene::PaintOperation::Primitive(crate::scene::Primitive::Quad(quad)) => {
                Some(quad.background.solid.l)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a7_paths_and_every_later_overlapping_primitive_are_replayed_in_order() {
    let mut scene = Scene::default();
    // Shade 0.1: a quad before the path; 0.2: the path's neighbour that it does not touch.
    scene.insert_primitive(quad_at(rect(0., 0., 100., 100.), 0.1));
    scene.insert_primitive(quad_at(rect(400., 0., 50., 50.), 0.2));
    scene.insert_primitive(path_over(rect(50., 50., 100., 100.)));
    // 0.3 overlaps the path; 0.4 overlaps only 0.3 (transitively); 0.5 overlaps nothing in
    // the overlay but the first quad; 0.6 is far away.
    scene.insert_primitive(quad_at(rect(120., 120., 100., 100.), 0.3));
    scene.insert_primitive(quad_at(rect(210., 210., 100., 100.), 0.4));
    scene.insert_primitive(quad_at(rect(0., 0., 20., 20.), 0.5));
    scene.insert_primitive(quad_at(rect(800., 800., 20., 20.), 0.6));
    let region = Bounds {
        origin: point(DevicePixels(0), DevicePixels(0)),
        size: size(DevicePixels(1000), DevicePixels(1000)),
    };
    let content =
        LayerContent::from_recording(&scene, point(ScaledPixels(0.), ScaledPixels(0.)), region)
            .expect("plain content");
    assert_eq!(content.overlay_len(), 3, "the path, 0.3 and 0.4");
    assert_eq!(
        shades(content.overlay()),
        [0.3, 0.4],
        "the quads in original order"
    );
    assert_eq!(content.overlay().paths.len(), 1);
    assert_eq!(shades(content.baked()), [0.1, 0.2, 0.5, 0.6]);
    assert!(content.baked().paths.is_empty(), "no path in a tile");
    assert_eq!(content.baked_len(), 4);
}

#[test]
fn a7_no_overlay_primitive_precedes_an_overlapping_baked_one() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    for round in 0..120 {
        let mut scene = Scene::default();
        let mut recorded: Vec<(Bounds<ScaledPixels>, bool)> = Vec::new();
        for ix in 0..30 {
            let bounds = rect(
                next(300) as f32,
                next(300) as f32,
                10. + next(90) as f32,
                10. + next(90) as f32,
            );
            let is_path = next(6) == 0;
            if is_path {
                let path = path_over(bounds);
                recorded.push((path.bounds, true));
                scene.insert_primitive(path);
            } else {
                scene.insert_primitive(quad_at(bounds, 0.01 * (ix + 1) as f32));
                recorded.push((bounds, false));
            }
        }
        let region = Bounds {
            origin: point(DevicePixels(0), DevicePixels(0)),
            size: size(DevicePixels(500), DevicePixels(500)),
        };
        let content =
            LayerContent::from_recording(&scene, point(ScaledPixels(0.), ScaledPixels(0.)), region)
                .expect("plain content");
        // Every primitive is in exactly one list.
        assert_eq!(
            content.baked_len() + content.overlay_len(),
            30,
            "round {round}"
        );
        // Rebuild which recorded primitive went where: quads by shade, the paths by count.
        let overlay_shades = shades(content.overlay());
        let baked_shades = shades(content.baked());
        let in_overlay = |ix: usize| {
            let (_, is_path) = recorded[ix];
            !is_path && overlay_shades.contains(&(0.01 * (ix + 1) as f32))
        };
        let _ = baked_shades;
        // Paths are overlay; a quad is overlay when it overlaps an earlier overlay primitive.
        let mut overlay_bounds: Vec<Bounds<ScaledPixels>> = Vec::new();
        for (ix, (bounds, is_path)) in recorded.iter().enumerate() {
            let expected = *is_path || overlay_bounds.iter().any(|other| other.intersects(bounds));
            if !is_path {
                assert_eq!(in_overlay(ix), expected, "round {round}, primitive {ix}");
            }
            if expected {
                overlay_bounds.push(*bounds);
            }
        }
        // Overlay quads keep their original order.
        let order: Vec<usize> = overlay_shades
            .iter()
            .map(|shade| (shade / 0.01).round() as usize - 1)
            .collect();
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "round {round}: {order:?}"
        );
        // The invariant tiles rely on: no baked primitive that overlaps an overlay primitive
        // comes after it.
        for (b_ix, (baked_bounds, is_path)) in recorded.iter().enumerate() {
            if *is_path || in_overlay(b_ix) {
                continue;
            }
            for (o_ix, (overlay_bounds, o_is_path)) in recorded.iter().enumerate() {
                if o_ix < b_ix && (*o_is_path || in_overlay(o_ix)) {
                    assert!(
                        !overlay_bounds.intersects(baked_bounds),
                        "round {round}: overlay {o_ix} precedes the overlapping baked {b_ix}"
                    );
                }
            }
        }
    }
}

#[crate::test]
fn a7_a_layered_window_replays_paths_over_tiles_in_original_order(cx: &mut TestAppContext) {
    use crate::scroll_layer_tests::Fixture;
    let layered = Fixture::new(cx, 60, true, true, false);
    let direct = Fixture::new(cx, 60, false, true, false);
    for _ in 0..6 {
        layered.wheel(cx, -7.7);
        direct.wheel(cx, -7.7);
    }
    assert!(layered.stats(cx).layer_composites > 0);
    let (expected, actual) = (direct.content(cx), layered.content(cx));
    crate::painted_content_agrees(&expected, &actual).expect("same primitives, same order");
    // The scene of the layered window holds the paths live, after the tile sprites.
    let paths = cx
        .update_window(layered.handle, |_, window, _| {
            let scene = window.painted_scene();
            (
                scene.layers.len(),
                scene.paths.len(),
                scene.polychrome_sprites.len(),
            )
        })
        .unwrap();
    assert_eq!(paths.0, 1);
    assert!(paths.1 >= 5, "paths replayed live: {paths:?}");
    assert!(paths.2 >= 1, "tile sprites: {paths:?}");
}
