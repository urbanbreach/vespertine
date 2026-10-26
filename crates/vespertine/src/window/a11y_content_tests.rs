//! Tests of the accessibility tree of an ordinary window (A11Y-003): the window node covers the
//! visible window, text without an id is exposed as labels, and elements that handle clicks
//! without a role are buttons.

use crate::{
    AnyWindowHandle, AppContext as _, Context, DeclarativeAria as _, Decorations, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, TestAppContext, Text, Tiling, Window,
    div, motion::PaintScope, px,
};
use accesskit::{Action, ActionRequest, NodeId, Rect, Role, TreeId, TreeUpdate};
use std::{cell::Cell, rc::Rc};

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn activate(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, _| {
        window.set_a11y_active_for_tests(true)
    })
    .unwrap();
    draw(cx, window);
}

/// The tree update the platform received last.
fn update(cx: &mut TestAppContext, window: AnyWindowHandle) -> TreeUpdate {
    cx.update_window(window, |_, window, _| {
        window
            .platform_window
            .as_test()
            .expect("a test window")
            .last_accessibility_update()
            .expect("the platform received a tree update")
    })
    .unwrap()
}

fn node(update: &TreeUpdate, id: NodeId) -> &accesskit::Node {
    &update
        .nodes
        .iter()
        .find(|(node_id, _)| *node_id == id)
        .expect("the node is in the tree")
        .1
}

/// The id of the label holding `text`, which must be exactly one node.
fn text_node(update: &TreeUpdate, text: &str) -> NodeId {
    let found: Vec<NodeId> = update
        .nodes
        .iter()
        .filter(|(_, node)| node.value() == Some(text))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(found.len(), 1, "one node holds {text:?}: {update:#?}");
    assert_eq!(node(update, found[0]).role(), Role::Label);
    found[0]
}

fn holds_text(update: &TreeUpdate, text: &str) -> bool {
    update
        .nodes
        .iter()
        .any(|(_, node)| node.value() == Some(text))
}

fn parent(update: &TreeUpdate, id: NodeId) -> NodeId {
    update
        .nodes
        .iter()
        .find(|(_, node)| node.children().contains(&id))
        .expect("the node has a parent")
        .0
}

fn root(update: &TreeUpdate) -> NodeId {
    update.tree.as_ref().expect("the tree description").root
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// A1: the window node covers the visible window in device pixels, without the invisible
/// shadow margin of a client-decorated window, on edges that are not tiled; the tree names
/// its toolkit.
#[crate::test]
fn window_node_covers_the_visible_window_in_device_pixels(cx: &mut TestAppContext) {
    let window: AnyWindowHandle = cx.add_window(|_, _| Blank).into();
    cx.update_window(window, |_, window, _| window.set_scale_factor(2.))
        .unwrap();
    activate(cx, window);
    let size = cx
        .update_window(window, |_, window, _| window.viewport_size())
        .unwrap();
    let (width, height) = (f64::from(size.width.0), f64::from(size.height.0));
    let tree = update(cx, window);
    let description = tree.tree.as_ref().unwrap();
    assert_eq!(description.toolkit_name.as_deref(), Some("Vespertine"));
    assert_eq!(
        node(&tree, root(&tree)).bounds(),
        Some(Rect::new(0., 0., width * 2., height * 2.)),
        "a server-decorated window is all visible"
    );

    let decorate = |cx: &mut TestAppContext, tiling: Tiling| {
        cx.update_window(window, |_, window, _| {
            window
                .platform_window
                .as_test()
                .unwrap()
                .set_decorations(Decorations::Client { tiling });
            window.set_client_inset(px(10.));
            window.refresh();
        })
        .unwrap();
        draw(cx, window);
    };
    decorate(cx, Tiling::default());
    let tree = update(cx, window);
    assert_eq!(
        node(&tree, root(&tree)).bounds(),
        Some(Rect::new(20., 20., (width - 10.) * 2., (height - 10.) * 2.)),
        "the shadow margin is left out on every floating edge"
    );
    decorate(
        cx,
        Tiling {
            left: true,
            ..Tiling::default()
        },
    );
    let tree = update(cx, window);
    assert_eq!(
        node(&tree, root(&tree)).bounds(),
        Some(Rect::new(0., 20., (width - 10.) * 2., (height - 10.) * 2.)),
        "a tiled edge has no margin"
    );
}

struct Content {
    other: SharedString,
    lead: bool,
}

impl Render for Content {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("list").role(Role::List);
        if self.lead {
            list = list.child("lead");
        }
        div()
            .size_full()
            .child("Window text")
            .child(
                list.child(div().child("first"))
                    .child(StyledText::new("second"))
                    .child(Text::new_inaccessible("hidden".into()))
                    .child("  \n")
                    .child(PaintScope::new(div().child("inert")).inert(true)),
            )
            .child(div().id("other").child(self.other.clone()))
    }
}

fn content(cx: &mut TestAppContext) -> (Entity<Content>, AnyWindowHandle) {
    let mut entity = None;
    let window: AnyWindowHandle = cx
        .add_window(|_, cx| {
            let view = cx.new(|_| Content {
                other: "other".into(),
                lead: false,
            });
            entity = Some(view.clone());
            Holder { view }
        })
        .into();
    activate(cx, window);
    (entity.unwrap(), window)
}

struct Holder {
    view: Entity<Content>,
}

impl Render for Holder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.view.clone())
    }
}

/// A2: text of elements without an id is a label holding the text, a child of the nearest
/// enclosing node; inaccessible text, blank text and inert text are left out.
#[crate::test]
fn text_without_an_id_is_a_label_of_the_nearest_node(cx: &mut TestAppContext) {
    let (_, window) = content(cx);
    let tree = update(cx, window);
    let list = tree
        .nodes
        .iter()
        .find(|(_, node)| node.role() == Role::List)
        .expect("the list has a node")
        .0;
    let first = text_node(&tree, "first");
    let second = text_node(&tree, "second");
    assert_eq!(
        node(&tree, list).children(),
        [first, second],
        "the text of the list, through an element without a node, in order"
    );
    for text in ["Window text", "other"] {
        assert_eq!(
            parent(&tree, text_node(&tree, text)),
            root(&tree),
            "{text:?} has no enclosing node but the window's"
        );
    }
    for absent in ["hidden", "  \n", "inert"] {
        assert!(!holds_text(&tree, absent), "{absent:?} is not exposed");
    }
    let bounds = node(&tree, first).bounds().expect("the label has bounds");
    assert!(bounds.x1 > bounds.x0 && bounds.y1 > bounds.y0, "{bounds:?}");
}

/// A3: a label keeps its id when its text changes, and when text is added inside another
/// identified element.
#[crate::test]
fn text_ids_are_numbered_within_the_nearest_identified_element(cx: &mut TestAppContext) {
    let (view, window) = content(cx);
    let tree = update(cx, window);
    let ids = |tree: &TreeUpdate, texts: &[&str]| -> Vec<NodeId> {
        texts.iter().map(|text| text_node(tree, text)).collect()
    };
    let before = ids(&tree, &["Window text", "first", "second", "other"]);

    view.update(cx, |view, cx| {
        view.other = "changed".into();
        cx.notify();
    });
    draw(cx, window);
    let tree = update(cx, window);
    assert_eq!(
        ids(&tree, &["Window text", "first", "second", "changed"]),
        before,
        "changing a text keeps its node"
    );

    view.update(cx, |view, cx| {
        view.lead = true;
        cx.notify();
    });
    draw(cx, window);
    let tree = update(cx, window);
    let after = ids(
        &tree,
        &["Window text", "lead", "first", "second", "changed"],
    );
    assert_eq!(
        [after[0], after[4]],
        [before[0], before[3]],
        "text outside the list keeps its nodes"
    );
}

struct Clickable {
    clicks: Rc<Cell<u32>>,
}

impl Render for Clickable {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div()
            .size_full()
            .child(
                div()
                    .id("open")
                    .w(px(80.))
                    .h(px(20.))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    .child("Open file"),
            )
            .child(
                div()
                    .id("link")
                    .role(Role::Link)
                    .on_click(|_, _, _| {})
                    .child("Docs"),
            )
            .child(
                div()
                    .id("backdrop")
                    .dom_role("presentation")
                    .on_click(|_, _, _| {})
                    .child("Decoration"),
            )
            .child(div().id("plain").child("Plain"))
    }
}

/// A4: an element that handles clicks and has no role is a button whose text labels it and
/// whose click action reaches the click listener; an explicit role wins and a suppressing role
/// token keeps the element out.
#[crate::test]
fn clickable_elements_without_a_role_are_buttons(cx: &mut TestAppContext) {
    let clicks = Rc::new(Cell::new(0));
    let window: AnyWindowHandle = cx
        .add_window({
            let clicks = clicks.clone();
            move |_, _| Clickable { clicks }
        })
        .into();
    activate(cx, window);
    let tree = update(cx, window);
    let role_of_text = |text: &str| node(&tree, parent(&tree, text_node(&tree, text))).role();
    assert_eq!(role_of_text("Open file"), Role::Button);
    assert_eq!(role_of_text("Docs"), Role::Link, "the explicit role wins");
    assert_eq!(
        role_of_text("Decoration"),
        Role::Window,
        "suppressed: no node"
    );
    assert_eq!(role_of_text("Plain"), Role::Window, "no click, no node");
    let button = parent(&tree, text_node(&tree, "Open file"));
    assert!(node(&tree, button).supports_action(Action::Click));

    cx.update_window(window, |_, window, cx| {
        window.handle_a11y_action(
            ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: button,
                data: None,
            },
            cx,
        );
    })
    .unwrap();
    assert_eq!(clicks.get(), 1, "the click action reaches the listener");
}

struct Leaf {
    index: usize,
    digit: u32,
}

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(format!("leaf {} shows {}", self.index, self.digit))
            .child(
                div()
                    .id("act")
                    .on_click(|_, _, _| {})
                    .child(format!("act on {}", self.index)),
            )
    }
}

struct Stack {
    leaves: Vec<Entity<Leaf>>,
}

impl Render for Stack {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut stack = div().size_full().child("before");
        for (index, leaf) in self.leaves.iter().enumerate() {
            stack = stack.child(leaf.clone()).child(format!("between {index}"));
        }
        stack.child("after")
    }
}

/// A5: a retained frame, which replays unchanged views and rebuilds changed ones in place,
/// delivers the same tree, ids included, as a frame that builds every view.
#[crate::test]
fn replayed_text_keeps_the_ids_of_a_full_build(cx: &mut TestAppContext) {
    let leaves: Vec<Entity<Leaf>> = (0..6)
        .map(|index| cx.new(|_| Leaf { index, digit: 0 }))
        .collect();
    let window: AnyWindowHandle = cx
        .add_window({
            let leaves = leaves.clone();
            move |_, _| Stack { leaves }
        })
        .into();
    activate(cx, window);
    draw(cx, window);
    let mut reused = 0;
    for step in 0..12 {
        let leaf = &leaves[(step * 5) % leaves.len()];
        leaf.update(cx, |leaf, cx| {
            leaf.digit = step as u32;
            cx.notify();
        });
        cx.update_window(window, |_, window, _| window.reset_frame_stats())
            .unwrap();
        draw(cx, window);
        reused += cx
            .update_window(window, |_, window, _| window.frame_stats().views_reused)
            .unwrap();
        let retained = format!("{:?}", update(cx, window));

        cx.update_window(window, |_, window, _| {
            window.set_retention_enabled(false);
            window.refresh();
        })
        .unwrap();
        draw(cx, window);
        let full = format!("{:?}", update(cx, window));
        assert_eq!(retained, full, "step {step}");

        cx.update_window(window, |_, window, _| {
            window.set_retention_enabled(true);
            window.refresh();
        })
        .unwrap();
        draw(cx, window);
    }
    assert!(reused > 0, "the retained frames replayed views");
}
