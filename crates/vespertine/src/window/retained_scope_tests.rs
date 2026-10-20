//! Tests that a view rebuilt inside a clean, replayed ancestor still inherits what the
//! ancestor's wrappers establish (PRF-001): inertness (PRF-003) and the edge fade and rounded
//! clip paint scopes (RND-001, RND-002). Every scenario runs against a window with retention
//! disabled, which builds every view in place and so defines the expected output.

use super::a11y_retained_tests::tree;
use crate::{
    AnyElement, AnyWindowHandle, AppContext as _, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Quad, Render,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, TestAppContext, Window, div,
    edge_faded, motion::PaintScope, px, rgb, rounded_clip,
};
use accesskit::Role;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Scope {
    Inert,
    Fade,
    Rounded,
}

/// A view of fixed size: notifying it never changes layout.
struct Leaf {
    fill: u32,
    focus: FocusHandle,
    clicks: Rc<Cell<u32>>,
}

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div()
            .id("leaf")
            .role(Role::Button)
            .aria_label("Leaf")
            .track_focus(&self.focus)
            .w(px(60.))
            .h(px(40.))
            .bg(rgb(self.fill))
            .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
            .child("leaf")
    }
}

/// The clean ancestor: it wraps the leaf in the scope under test.
struct Wrapper {
    scope: Scope,
    amount: f32,
    leaf: Entity<Leaf>,
    /// Whether the leaf is an explicitly cached view, replayed in a rebuilt wrapper.
    cached: bool,
}

impl Render for Wrapper {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let leaf: AnyElement = if self.cached {
            let size = StyleRefinement::default().w(px(60.)).h(px(40.));
            self.leaf.clone().cached(size).into_any_element()
        } else {
            self.leaf.clone().into_any_element()
        };
        let body = div().w(px(100.)).h(px(80.)).bg(rgb(0x202020)).child(leaf);
        let wrapped: AnyElement = match self.scope {
            Scope::Inert => PaintScope::new(body).inert(true).into_any_element(),
            Scope::Fade => edge_faded(px(self.amount), true, true, body).into_any_element(),
            Scope::Rounded => rounded_clip(px(self.amount), body).into_any_element(),
        };
        div().size_full().child(wrapped)
    }
}

struct Page(Entity<Wrapper>);

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

/// The same views drawn in a window with retention and in one without.
struct Scenario {
    wrapper: Entity<Wrapper>,
    leaf: Entity<Leaf>,
    retained: AnyWindowHandle,
    oracle: AnyWindowHandle,
}

impl Scenario {
    fn new(cx: &mut TestAppContext, scope: Scope, cached: bool) -> Self {
        let leaf = cx.new(|cx| Leaf {
            fill: 0x3060c0,
            focus: cx.focus_handle(),
            clicks: Rc::default(),
        });
        let wrapper = cx.new(|_| Wrapper {
            scope,
            amount: 12.,
            leaf: leaf.clone(),
            cached,
        });
        let mut open = |retained: bool| {
            let page = wrapper.clone();
            let window: AnyWindowHandle = cx.add_window(move |_, _| Page(page)).into();
            cx.update_window(window, |_, window, _| {
                window.set_retention_enabled(retained);
                window.set_a11y_active_for_tests(true);
            })
            .unwrap();
            window
        };
        let (retained, oracle) = (open(true), open(false));
        let scenario = Self {
            wrapper,
            leaf,
            retained,
            oracle,
        };
        for _ in 0..2 {
            scenario.draw(cx);
        }
        scenario
    }

    fn draw(&self, cx: &mut TestAppContext) {
        for window in [self.retained, self.oracle] {
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
    }

    fn reset_stats(&self, cx: &mut TestAppContext) {
        cx.update_window(self.retained, |_, window, _| window.reset_frame_stats())
            .unwrap();
    }

    fn stats(&self, cx: &mut TestAppContext) -> crate::FrameStats {
        cx.update_window(self.retained, |_, window, _| window.frame_stats())
            .unwrap()
    }

    fn set_fill(&self, cx: &mut TestAppContext, fill: u32) {
        self.leaf.update(cx, |leaf, cx| {
            leaf.fill = fill;
            cx.notify();
        });
    }

    fn set_amount(&self, cx: &mut TestAppContext, amount: f32) {
        self.wrapper.update(cx, |wrapper, cx| {
            wrapper.amount = amount;
            cx.notify();
        });
    }

    /// What the frame drawn last registered and painted, in comparable form.
    fn observe(&self, cx: &mut TestAppContext, window: AnyWindowHandle) -> String {
        let focus = self.leaf.read_with(cx, |leaf, _| leaf.focus.clone());
        let (registered, quads) = cx
            .update_window(window, |_, window, _| {
                let hitboxes: Vec<_> = window
                    .rendered_frame
                    .hitboxes
                    .iter()
                    .map(|hitbox| (hitbox.bounds, hitbox.content_mask, hitbox.behavior))
                    .collect();
                let registered = format!(
                    "hitboxes {hitboxes:?}\nmounted {}\nmouse listeners {}\n",
                    focus.is_mounted(window),
                    window.rendered_frame.mouse_listeners.len(),
                );
                (registered, window.painted_quads())
            })
            .unwrap();
        format!("{registered}quads {quads:#?}\ntree {}", tree(cx, window))
    }

    fn assert_agree(&self, cx: &mut TestAppContext, what: &str) {
        let retained = self.observe(cx, self.retained);
        let oracle = self.observe(cx, self.oracle);
        assert_eq!(retained, oracle, "{what}");
    }

    fn quads(&self, cx: &mut TestAppContext) -> Vec<Quad> {
        cx.update_window(self.retained, |_, window, _| window.painted_quads())
            .unwrap()
    }

    fn scale(&self, cx: &mut TestAppContext) -> f32 {
        cx.update_window(self.retained, |_, window, _| window.scale_factor())
            .unwrap()
    }
}

/// Notifies the leaf, which is then built again inside its replayed ancestors, and checks that
/// the frame is that of the window built in full.
fn rebuild_leaf_in_a_splice(cx: &mut TestAppContext, scope: Scope) -> Scenario {
    let scenario = Scenario::new(cx, scope, false);
    scenario.assert_agree(cx, "before the leaf changes");
    scenario.reset_stats(cx);
    scenario.set_fill(cx, 0xc03060);
    scenario.draw(cx);
    let stats = scenario.stats(cx);
    assert_eq!(stats.views_built, 1, "only the leaf is built: {stats:?}");
    assert!(stats.views_spliced >= 1, "the leaf is spliced: {stats:?}");
    scenario.assert_agree(cx, "after the leaf was rebuilt in a splice");
    scenario
}

#[crate::test]
fn rebuilt_leaf_in_an_inert_wrapper_registers_nothing(cx: &mut TestAppContext) {
    let scenario = rebuild_leaf_in_a_splice(cx, Scope::Inert);
    let focus = scenario.leaf.read_with(cx, |leaf, _| leaf.focus.clone());
    let mounted = cx
        .update_window(scenario.retained, |_, window, _| focus.is_mounted(window))
        .unwrap();
    assert!(!mounted, "an inert subtree registers no focus handle");
    let text = tree(cx, scenario.retained);
    assert!(!text.contains("Leaf"), "no accessibility output: {text}");
    assert!(!scenario.quads(cx).is_empty(), "inert content still paints");
}

#[crate::test]
fn rebuilt_leaf_in_an_edge_faded_wrapper_keeps_the_fade(cx: &mut TestAppContext) {
    let scenario = rebuild_leaf_in_a_splice(cx, Scope::Fade);
    let quads = scenario.quads(cx);
    assert!(quads.len() >= 2, "the wrapper's and the leaf's fills");
    let band = 12. * scenario.scale(cx);
    for quad in quads {
        assert_eq!(quad.scope.fade_bands[..2], [band, band], "{quad:?}");
    }
}

#[crate::test]
fn rebuilt_leaf_in_a_rounded_clip_keeps_the_clip(cx: &mut TestAppContext) {
    let scenario = rebuild_leaf_in_a_splice(cx, Scope::Rounded);
    let quads = scenario.quads(cx);
    assert!(quads.len() >= 2, "the wrapper's and the leaf's fills");
    let radius = 12. * scenario.scale(cx);
    for quad in quads {
        assert!(quad.scope.rounded_clip_active(), "{quad:?}");
        assert_eq!(quad.scope.clip_radius, radius, "{quad:?}");
    }
}

/// A replayed view carries the scope it was painted in: a wrapper that changes its scope
/// around a clean view makes that view build again.
#[crate::test]
fn changed_wrapper_scope_builds_the_clean_leaf_again(cx: &mut TestAppContext) {
    for scope in [Scope::Fade, Scope::Rounded] {
        let scenario = Scenario::new(cx, scope, true);
        scenario.assert_agree(cx, "before the wrapper changes");
        scenario.reset_stats(cx);
        scenario.set_amount(cx, 20.);
        scenario.draw(cx);
        let stats = scenario.stats(cx);
        assert!(
            stats.views_built >= 2,
            "{scope:?}: the leaf is built again: {stats:?}"
        );
        scenario.assert_agree(cx, &format!("{scope:?} after the wrapper changed"));
        let expected = 20. * scenario.scale(cx);
        for quad in scenario.quads(cx) {
            let applied = match scope {
                Scope::Fade => quad.scope.fade_bands[0],
                _ => quad.scope.clip_radius,
            };
            assert_eq!(applied, expected, "{scope:?}: {quad:?}");
        }
    }
}
