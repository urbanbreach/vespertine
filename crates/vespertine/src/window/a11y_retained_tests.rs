//! Tests of accessibility tree replay with retained frames (A11Y-002).

use crate::{
    AnyWindowHandle, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, Window, div, px,
};
use accesskit::{Action, ActionRequest, NodeId, Role, TreeId};
use std::{cell::Cell, rc::Rc};

/// How often each button was clicked through the accessibility action.
type Clicks = Rc<Cell<u32>>;

struct Button {
    index: usize,
    digit: u32,
    focus: FocusHandle,
    clicks: Clicks,
}

impl Render for Button {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div()
            .id(("button", self.index))
            .role(Role::Button)
            .aria_label(format!("Value {}", self.digit))
            .track_focus(&self.focus)
            .w(px(90.))
            .h(px(24.))
            .on_a11y_action(Action::Click, move |_, _, _| {
                clicks.set(clicks.get() + 1);
            })
            .child(format!("Value {}", self.digit))
    }
}

struct Panel {
    index: usize,
    buttons: Vec<Entity<Button>>,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(("panel", self.index))
            .role(Role::Group)
            .flex()
            .flex_wrap()
            .w(px(210.))
            .h(px(72.))
            .children(self.buttons.iter().cloned())
    }
}

struct Dashboard {
    panels: Vec<Entity<Panel>>,
}

impl Render for Dashboard {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .id("dashboard")
            .role(Role::Group)
            .flex_wrap()
            .flex()
            .children(self.panels.iter().cloned())
    }
}

struct Fixture {
    panels: Vec<Entity<Panel>>,
    buttons: Vec<Entity<Button>>,
    clicks: Vec<Clicks>,
}

impl Fixture {
    fn new(cx: &mut TestAppContext, panels: usize) -> Self {
        let mut buttons = Vec::new();
        let mut clicks = Vec::new();
        let mut panel_entities = Vec::new();
        for panel in 0..panels {
            let mut own = Vec::new();
            for button in 0..4 {
                let index = panel * 4 + button;
                let counter: Clicks = Rc::default();
                clicks.push(counter.clone());
                let entity = cx.new(|cx| Button {
                    index,
                    digit: 0,
                    focus: cx.focus_handle(),
                    clicks: counter,
                });
                buttons.push(entity.clone());
                own.push(entity);
            }
            panel_entities.push(cx.new(|_| Panel {
                index: panel,
                buttons: own,
            }));
        }
        Self {
            panels: panel_entities,
            buttons,
            clicks,
        }
    }

    fn window(&self, cx: &mut TestAppContext, retained: bool) -> AnyWindowHandle {
        let panels = self.panels.clone();
        let window: AnyWindowHandle = cx.add_window(move |_, _| Dashboard { panels }).into();
        cx.update_window(window, |_, window, _| {
            window.set_retention_enabled(retained);
            window.set_a11y_active_for_tests(true);
        })
        .unwrap();
        window
    }

    fn set_digit(&self, cx: &mut TestAppContext, button: usize, digit: u32) {
        self.buttons[button].update(cx, |button, cx| {
            button.digit = digit;
            cx.notify();
        });
    }

    /// Moves focus without a window refresh, as a focus change inside a frame does.
    fn focus_without_refresh(
        &self,
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        button: Option<usize>,
    ) {
        let id = button.map(|button| self.buttons[button].read_with(cx, |b, _| b.focus.id));
        cx.update_window(window, |_, window, _| window.focus = id)
            .unwrap();
    }
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn views_rendered(cx: &mut TestAppContext, window: AnyWindowHandle) -> u64 {
    cx.update_window(window, |_, window, _| window.frame_stats().views_built)
        .unwrap()
}

fn reset(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, _| window.reset_frame_stats())
        .unwrap();
}
/// The tree update the platform received last, in comparable form: node ids are replaced by
/// the order of their first appearance, since two windows hold different root entities.
pub(super) fn tree(cx: &mut TestAppContext, window: AnyWindowHandle) -> String {
    cx.update_window(window, |_, window, _| {
        let update = window
            .platform_window
            .as_test()
            .expect("a test window")
            .last_accessibility_update()
            .expect("the platform received a tree update");
        let mut text = format!("{update:?}");
        let mut ids: Vec<String> = Vec::new();
        let mut out = String::new();
        let mut rest = text.as_str();
        while let Some(at) = rest.find('#') {
            out.push_str(&rest[..at]);
            let digits: String = rest[at + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            rest = &rest[at + 1 + digits.len()..];
            let ordinal = ids.iter().position(|id| *id == digits).unwrap_or_else(|| {
                ids.push(digits.clone());
                ids.len() - 1
            });
            out.push_str(&format!("#n{ordinal}"));
        }
        out.push_str(rest);
        text = out;
        text
    })
    .unwrap()
}

fn node_of(cx: &mut TestAppContext, window: AnyWindowHandle, button: usize) -> NodeId {
    cx.update_window(window, |_, window, _| {
        let update = window
            .platform_window
            .as_test()
            .unwrap()
            .last_accessibility_update()
            .unwrap();
        update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::Button)
            .map(|(id, _)| *id)
            .nth(button)
            .expect("the button is in the tree")
    })
    .unwrap()
}

fn act(cx: &mut TestAppContext, window: AnyWindowHandle, target_node: NodeId) {
    cx.update_window(window, |_, window, cx| {
        window.handle_a11y_action(
            ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node,
                data: None,
            },
            cx,
        );
    })
    .unwrap();
}

fn focused_node(cx: &mut TestAppContext, window: AnyWindowHandle) -> NodeId {
    cx.update_window(window, |_, window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .last_accessibility_update()
            .unwrap()
            .focus
    })
    .unwrap()
}

/// A1: a retained and a nonretained window, both accessible, produce the identical tree,
/// focus and action behavior on every frame of a seeded history.
#[crate::test]
fn retained_and_full_trees_agree_on_every_frame(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, 6);
    let retained = fixture.window(cx, true);
    let full = fixture.window(cx, false);
    draw(cx, retained);
    draw(cx, full);
    assert_eq!(tree(cx, retained), tree(cx, full));

    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    let mut replayed = 0;
    for step in 0..150 {
        reset(cx, retained);
        match next(4) {
            0 | 1 => {
                let button = next(24) as usize;
                fixture.set_digit(cx, button, next(10) as u32);
            }
            2 => {
                let button = (next(5) != 0).then_some(next(24) as usize);
                for window in [retained, full] {
                    fixture.focus_without_refresh(cx, window, button);
                }
            }
            _ => {}
        }
        draw(cx, retained);
        draw(cx, full);
        replayed += cx
            .update_window(retained, |_, window, _| window.frame_stats().views_reused)
            .unwrap();
        assert_eq!(
            tree(cx, retained),
            tree(cx, full),
            "the trees differ at step {step}"
        );
        let button = next(24) as usize;
        let (target_retained, target_full) =
            (node_of(cx, retained, button), node_of(cx, full, button));
        act(cx, retained, target_retained);
        act(cx, full, target_full);
        assert_eq!(
            fixture.clicks[button].get() % 2,
            0,
            "each action reaches its listener once per window"
        );
    }
    assert!(replayed > 0, "the history never replayed a view");
}

/// A2: one child rebuilt inside a retained parent, and a focus change with no visual change:
/// the changed node is updated and the focused identity is re-evaluated, not copied.
#[crate::test]
fn replayed_views_reevaluate_focus_and_splice_rebuilt_children(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, 3);
    let window = fixture.window(cx, true);
    draw(cx, window);
    draw(cx, window);
    let initial_focus = focused_node(cx, window);

    fixture.focus_without_refresh(cx, window, Some(5));
    reset(cx, window);
    draw(cx, window);
    assert_eq!(
        views_rendered(cx, window),
        0,
        "a focus change alone builds no view"
    );
    let focused = focused_node(cx, window);
    assert_ne!(focused, initial_focus);
    assert_eq!(focused, node_of(cx, window, 5), "button 5 holds the focus");

    fixture.focus_without_refresh(cx, window, Some(6));
    // The notification redraws the window when the update ends.
    reset(cx, window);
    fixture.set_digit(cx, 1, 7);
    draw(cx, window);
    assert_eq!(
        views_rendered(cx, window),
        1,
        "only the changed button is built"
    );
    assert_eq!(focused_node(cx, window), node_of(cx, window, 6));
    let text = tree(cx, window);
    assert!(
        text.contains("Value 7"),
        "the rebuilt child's label is updated"
    );

    fixture.focus_without_refresh(cx, window, None);
    draw(cx, window);
    assert_eq!(
        focused_node(cx, window),
        NodeId(0),
        "dropping the focus moves it back to the window node"
    );
}

/// A4: the first frame after activation builds every view to seed the log; following frames
/// replay the log produced by a replay for at least 100 frames with an identical tree.
#[crate::test]
fn replay_of_a_replay_keeps_the_tree_identical(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, 4);
    let panels = fixture.panels;
    let window: AnyWindowHandle = cx.add_window(move |_, _| Dashboard { panels }).into();
    draw(cx, window);
    draw(cx, window);
    // Raw activation: unlike `set_a11y_active_for_tests` it does not refresh the window, so the
    // frame drawn below is the first one after activation.
    cx.update_window(window, |_, window, _| window.a11y.set_active_for_test(true))
        .unwrap();
    reset(cx, window);
    draw(cx, window);
    assert_eq!(
        views_rendered(cx, window),
        1 + 4 + 16,
        "the first accessible frame builds the dashboard, every panel and every button"
    );
    let seeded = tree(cx, window);
    for frame in 0..120 {
        reset(cx, window);
        draw(cx, window);
        assert_eq!(views_rendered(cx, window), 0, "frame {frame} replays");
        assert_eq!(tree(cx, window), seeded, "frame {frame} keeps the tree");
    }
}

/// A5: an action delivered to a node inside a retained view reaches the right listener after
/// repeated reuse and child splicing.
#[crate::test]
fn actions_reach_listeners_through_reuse_and_splicing(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, 3);
    let window = fixture.window(cx, true);
    for round in 0..40 {
        if round % 3 == 0 {
            fixture.set_digit(cx, round % 12, round as u32 % 10);
        }
        draw(cx, window);
        let target = round % 12;
        let node = node_of(cx, window, target);
        act(cx, window, node);
        assert_eq!(
            fixture.clicks[target].get(),
            (round / 12 + 1) as u32,
            "round {round}"
        );
    }
    let total: u32 = fixture.clicks.iter().map(|clicks| clicks.get()).sum();
    assert_eq!(total, 40);
}

/// A6: the platform receives a full tree update on every frame on which accessibility is
/// active, and a focus change appears in the update of the frame it was made.
#[crate::test]
fn every_accessible_frame_delivers_a_full_tree_with_current_focus(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, 2);
    let window = fixture.window(cx, true);
    draw(cx, window);
    let node_count = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window
                .platform_window
                .as_test()
                .unwrap()
                .last_accessibility_update()
                .map(|update| update.nodes.len())
        })
        .unwrap()
    };
    let full = node_count(cx).expect("an update was delivered");
    assert_eq!(
        full,
        1 + 2 + 2 * 4 * 2 + 1,
        "root, dashboard, panels, buttons and the buttons' text"
    );
    for frame in 0..10 {
        fixture.focus_without_refresh(cx, window, Some(frame % 8));
        let before = cx
            .update_window(window, |_, window, _| {
                window
                    .platform_window
                    .as_test()
                    .unwrap()
                    .accessibility_updates()
            })
            .unwrap();
        draw(cx, window);
        let after = cx
            .update_window(window, |_, window, _| {
                window
                    .platform_window
                    .as_test()
                    .unwrap()
                    .accessibility_updates()
            })
            .unwrap();
        assert_eq!(after, before + 1, "frame {frame} delivers one update");
        assert_eq!(node_count(cx), Some(full), "the update is complete");
        assert_eq!(
            focused_node(cx, window),
            node_of(cx, window, frame % 8),
            "the update of frame {frame} carries that frame's focus"
        );
    }
}
