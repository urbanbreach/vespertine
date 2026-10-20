//! Tests of views drawn inside deferred draws (PRF-001 R2, R6): such views are retained
//! independently of the view that owns the deferred draw. A dirty view inside the draw does not
//! rebuild the owner, and the records of the views inside the draw keep valid ranges across the
//! owner's replay.

use crate::{
    AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, deferred, div, point, px,
};

thread_local! {
    /// Names of the views rendered since the last `take_renders`.
    static RENDERS: std::cell::RefCell<Vec<(crate::WindowId, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn note_render(window: &Window, name: String) {
    RENDERS.with(|renders| {
        renders
            .borrow_mut()
            .push((window.window_handle().window_id(), name));
    });
}

/// The views `window` rendered since the last call (all windows' records are dropped).
fn take_renders(window: AnyWindowHandle) -> Vec<String> {
    RENDERS
        .with(|renders| std::mem::take(&mut *renders.borrow_mut()))
        .into_iter()
        .filter(|(id, _)| *id == window.window_id())
        .map(|(_, name)| name)
        .collect()
}

struct Leaf {
    ix: u32,
    value: u32,
}

impl Render for Leaf {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        note_render(window, format!("leaf{}", self.ix));
        div()
            .w(px(60.))
            .h(px(14.))
            .bg(crate::rgb(0xd0_e0_ff))
            .child(format!("leaf {:03}", self.value))
    }
}

/// A view drawn inside its owner's deferred draw. With `nested_deferred` its own leaf is drawn
/// inside a deferred draw of the popup as well.
struct Popup {
    ix: u32,
    value: u32,
    leaf: Entity<Leaf>,
    nested_deferred: bool,
}

impl Render for Popup {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        note_render(window, format!("popup{}", self.ix));
        let leaf = self.leaf.clone();
        let content = div()
            .w(px(100.))
            .h(px(40.))
            .child(format!("popup {:03}", self.value));
        if self.nested_deferred {
            content.child(deferred(div().child(leaf)))
        } else {
            content.child(leaf)
        }
    }
}

struct Panel {
    ix: u32,
    value: u32,
    popup: Entity<Popup>,
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        note_render(window, format!("panel{}", self.ix));
        div()
            .w(px(140.))
            .h(px(70.))
            .bg(crate::rgb(0xf6_f8_fa))
            .child(format!("panel {:03}", self.value))
            .child(deferred(div().child(self.popup.clone())))
    }
}

struct Board {
    panels: Vec<Entity<Panel>>,
}

impl Render for Board {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        note_render(window, "board".into());
        div()
            .size_full()
            .flex()
            .flex_wrap()
            .children(self.panels.iter().cloned())
    }
}

struct Tree {
    panels: Vec<Entity<Panel>>,
    popups: Vec<Entity<Popup>>,
    leaves: Vec<Entity<Leaf>>,
}

fn build_tree(cx: &mut TestAppContext, nested_deferred: bool) -> Tree {
    let mut tree = Tree {
        panels: Vec::new(),
        popups: Vec::new(),
        leaves: Vec::new(),
    };
    for ix in 0..4u32 {
        let leaf = cx.new(|_| Leaf { ix, value: ix });
        let popup = cx.new({
            let leaf = leaf.clone();
            move |_| Popup {
                ix,
                value: ix,
                leaf,
                nested_deferred,
            }
        });
        let panel = cx.new({
            let popup = popup.clone();
            move |_| Panel {
                ix,
                value: ix,
                popup,
            }
        });
        tree.leaves.push(leaf);
        tree.popups.push(popup);
        tree.panels.push(panel);
    }
    tree
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn stats(cx: &mut TestAppContext, window: AnyWindowHandle) -> crate::FrameStats {
    cx.update_window(window, |_, window, _| window.frame_stats())
        .unwrap()
}

fn reset(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, _| window.reset_frame_stats())
        .unwrap();
}

fn signature(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
) -> (Vec<String>, Vec<Vec<crate::Bounds<crate::Pixels>>>) {
    cx.update_window(window, |_, window, _| {
        let hits = (0..16)
            .map(|ix| {
                window.hit_test_bounds(point(
                    px(5. + 40. * (ix % 8) as f32),
                    px(5. + 30. * (ix / 8) as f32),
                ))
            })
            .collect();
        (window.painted_primitives(), hits)
    })
    .unwrap()
}

/// What to notify in one step: the view level and the panel index.
#[derive(Clone, Copy, Debug)]
enum Touch {
    Leaf(usize),
    Popup(usize),
    Panel(usize),
}

impl Touch {
    fn view(self) -> String {
        match self {
            Touch::Leaf(ix) => format!("leaf{ix}"),
            Touch::Popup(ix) => format!("popup{ix}"),
            Touch::Panel(ix) => format!("panel{ix}"),
        }
    }
}

fn touch(cx: &mut TestAppContext, tree: &Tree, touch: Touch, round: u32) {
    match touch {
        Touch::Leaf(ix) => tree.leaves[ix].update(cx, |leaf, cx| {
            leaf.value = 100 + round;
            cx.notify();
        }),
        Touch::Popup(ix) => tree.popups[ix].update(cx, |popup, cx| {
            popup.value = 200 + round;
            cx.notify();
        }),
        Touch::Panel(ix) => tree.panels[ix].update(cx, |panel, cx| {
            panel.value = 300 + round;
            cx.notify();
        }),
    }
}

fn assert_agree(
    cx: &mut TestAppContext,
    retained: AnyWindowHandle,
    full: AnyWindowHandle,
    context: &str,
) {
    let (retained_scene, retained_hits) = signature(cx, retained);
    let (full_scene, full_hits) = signature(cx, full);
    assert_eq!(retained_scene, full_scene, "scene differs: {context}");
    assert_eq!(retained_hits, full_hits, "hit tests differ: {context}");
}

/// A retained window and a window that rebuilds everything, each over its own entities: the
/// renders of one window update its entities, which the other would count as changes.
fn windows(cx: &mut TestAppContext, nested_deferred: bool) -> Windows {
    let retained_tree = build_tree(cx, nested_deferred);
    let full_tree = build_tree(cx, nested_deferred);
    let open = |cx: &mut TestAppContext, tree: &Tree| -> AnyWindowHandle {
        let panels = tree.panels.clone();
        cx.add_window(move |_, _| Board { panels }).into()
    };
    let retained = open(cx, &retained_tree);
    let full = open(cx, &full_tree);
    cx.update_window(full, |_, window, _| window.set_retention_enabled(false))
        .unwrap();
    for window in [retained, full] {
        draw(cx, window);
        draw(cx, window);
    }
    Windows {
        retained,
        full,
        retained_tree,
        full_tree,
    }
}

struct Windows {
    retained: AnyWindowHandle,
    full: AnyWindowHandle,
    retained_tree: Tree,
    full_tree: Tree,
}

impl Windows {
    /// Notifies the same view in both windows' trees and draws both.
    fn touch(&self, cx: &mut TestAppContext, which: Touch, round: u32) {
        for window in [self.retained, self.full] {
            reset(cx, window);
        }
        take_renders(self.retained);
        touch(cx, &self.retained_tree, which, round);
        touch(cx, &self.full_tree, which, round);
        draw(cx, self.retained);
        draw(cx, self.full);
    }

    fn agree(&self, cx: &mut TestAppContext, context: &str) {
        assert_agree(cx, self.retained, self.full, context);
    }
}

/// Notifies each view of `sequence` in turn and checks that exactly that view is rebuilt and
/// that the retained window still draws what the retention-disabled window draws.
fn assert_each_builds_alone(cx: &mut TestAppContext, sequence: &[Touch]) {
    for nested_deferred in [false, true] {
        let windows = windows(cx, nested_deferred);
        windows.agree(cx, "warm");
        reset(cx, windows.retained);
        take_renders(windows.retained);
        draw(cx, windows.retained);
        let idle = stats(cx, windows.retained);
        assert_eq!(idle.views_built, 0, "nested {nested_deferred}: {idle:?}");

        for (round, which) in sequence.iter().copied().enumerate() {
            // Several replayed frames in a row: every record keeps a valid range across them.
            for _ in 0..3 {
                reset(cx, windows.retained);
                draw(cx, windows.retained);
                draw(cx, windows.full);
                let idle = stats(cx, windows.retained);
                assert_eq!(idle.views_built, 0, "nested {nested_deferred}: {idle:?}");
            }
            windows.agree(
                cx,
                &format!("nested {nested_deferred} idle before round {round}"),
            );
            windows.touch(cx, which, round as u32);
            let built = stats(cx, windows.retained);
            assert_eq!(
                take_renders(windows.retained),
                vec![which.view()],
                "nested {nested_deferred}, {which:?}, round {round}: {built:?}"
            );
            assert!(
                built.views_reused > 0,
                "nested {nested_deferred}, {which:?}, round {round}: {built:?}"
            );
            windows.agree(
                cx,
                &format!("nested {nested_deferred} {which:?} round {round}"),
            );
        }
    }
}

/// A notified view drawn inside its owner's deferred draw is the only view built; the owner and
/// the panels around it replay. Four panels notified one after the other, then a panel twice.
#[crate::test]
fn dirty_view_in_a_deferred_draw_does_not_rebuild_its_owner(cx: &mut TestAppContext) {
    assert_each_builds_alone(
        cx,
        &[
            Touch::Popup(0),
            Touch::Popup(1),
            Touch::Popup(2),
            Touch::Popup(3),
            Touch::Popup(1),
            Touch::Popup(1),
        ],
    );
}

/// A view nested two levels deep inside a deferred draw builds alone, over successive
/// notifications of different panels.
#[crate::test]
fn nested_view_in_a_deferred_draw_builds_alone(cx: &mut TestAppContext) {
    assert_each_builds_alone(
        cx,
        &[
            Touch::Leaf(0),
            Touch::Leaf(1),
            Touch::Leaf(2),
            Touch::Leaf(3),
            Touch::Leaf(0),
            Touch::Leaf(0),
        ],
    );
}

/// Owners notified themselves rebuild alone as well, and their popups and leaves are replayed.
#[crate::test]
fn notified_owner_replays_the_views_inside_its_deferred_draw(cx: &mut TestAppContext) {
    assert_each_builds_alone(
        cx,
        &[
            Touch::Panel(2),
            Touch::Panel(0),
            Touch::Panel(3),
            Touch::Panel(1),
            Touch::Panel(2),
        ],
    );
}

/// Owners, their popups and leaves notified in mixed order, several at a time, keep agreeing
/// with the retention-disabled window while still replaying what is clean.
#[crate::test]
fn successive_mixed_notifications_agree_with_the_full_window(cx: &mut TestAppContext) {
    for nested_deferred in [false, true] {
        let windows = windows(cx, nested_deferred);
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move |bound: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % bound
        };
        let mut replayed = 0;
        for round in 0..80u32 {
            for window in [windows.retained, windows.full] {
                reset(cx, window);
            }
            take_renders(windows.retained);
            let mut log = Vec::new();
            for _ in 0..next(4) {
                let ix = next(4) as usize;
                let which = match next(3) {
                    0 => Touch::Leaf(ix),
                    1 => Touch::Popup(ix),
                    _ => Touch::Panel(ix),
                };
                log.push(which);
                touch(cx, &windows.retained_tree, which, round);
                touch(cx, &windows.full_tree, which, round);
            }
            draw(cx, windows.retained);
            draw(cx, windows.full);
            let built = stats(cx, windows.retained);
            replayed += built.views_reused;
            let mut rendered = take_renders(windows.retained);
            rendered.sort();
            rendered.dedup();
            let mut expected: Vec<String> = log.iter().map(|which| which.view()).collect();
            expected.sort();
            expected.dedup();
            assert_eq!(
                rendered, expected,
                "nested {nested_deferred}, round {round} {log:?}: {built:?}"
            );
            windows.agree(
                cx,
                &format!("nested {nested_deferred} round {round} {log:?}"),
            );
        }
        assert!(replayed > 0);
    }
}
