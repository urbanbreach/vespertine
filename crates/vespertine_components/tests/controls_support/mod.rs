//! Shared harness of the CMP-001 tests: a window hosting a freshly built element tree each frame
//! (so the owner's state lives in `Rc` cells), Tab traversal bound like an application would,
//! accessibility-tree queries and pixel sampling of the painted frame.

#![allow(dead_code)]

use serde_json::Value;
use vespertine::{
    AnyElement, App, AppContext as _, Context, FocusHandle, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Render, Styled, Window, actions, div, rgb,
};
use vespertine_test_host::{HostOptions, TestHost};

actions!(controls_test, [Tab, TabPrev]);

type Build = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

struct Harness {
    focus: FocusHandle,
    build: Build,
}

impl Render for Harness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("app")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Tab, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &TabPrev, window, cx| window.focus_prev(cx)))
            .size_full()
            .bg(rgb(0x101114))
            .child((self.build)(window, cx))
    }
}

/// A host of `width` x `height` logical pixels (scale factor 2) rendering `build` each frame.
/// `None` when no GPU test backend exists (the caller skips).
pub fn host(
    width: f64,
    height: f64,
    build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> TestHost {
    let mut host = TestHost::with_view(HostOptions::with_size(width, height), move |window, cx| {
        let view = cx.new(|cx| Harness {
            focus: cx.focus_handle(),
            build: Box::new(build),
        });
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
        view
    })
    .expect("the test host starts");
    host.app().update(|cx| {
        cx.bind_keys([
            KeyBinding::new("tab", Tab, None),
            KeyBinding::new("shift-tab", TabPrev, None),
        ])
    });
    host
}

/// One node of the accessibility tree.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: u64,
    pub role: String,
    pub flags: u32,
    pub actions: u64,
    pub properties: Value,
}

const FLAG_BUSY: u32 = 1 << 4;
const FLAG_DISABLED: u32 = 1 << 9;
/// `accesskit::Action::Click` is the first action.
const ACTION_CLICK: u64 = 1 << 0;

impl Node {
    pub fn label(&self) -> Option<&str> {
        self.properties.get("label").and_then(Value::as_str)
    }

    pub fn disabled(&self) -> bool {
        self.flags & FLAG_DISABLED != 0
    }

    pub fn busy(&self) -> bool {
        self.flags & FLAG_BUSY != 0
    }

    pub fn can_click(&self) -> bool {
        self.actions & ACTION_CLICK != 0
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.properties.get(key).and_then(Value::as_f64)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.properties.get(key).and_then(Value::as_str)
    }

    /// Bounds in physical pixels: `(x0, y0, x1, y1)`.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let bounds = &self.properties["bounds"];
        (
            bounds["x0"].as_f64().unwrap(),
            bounds["y0"].as_f64().unwrap(),
            bounds["x1"].as_f64().unwrap(),
            bounds["y1"].as_f64().unwrap(),
        )
    }
}

/// Every node of the accessibility tree of the last frame.
pub fn nodes(host: &mut TestHost) -> Vec<Node> {
    let tree = host
        .accessibility_tree()
        .expect("the tree is built")
        .expect("accessibility is enabled");
    tree["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|entry| {
            let node = &entry[1];
            Node {
                id: entry[0].as_u64().unwrap_or(0),
                role: node["role"].as_str().unwrap_or_default().to_owned(),
                flags: node["flags"].as_u64().unwrap_or(0) as u32,
                actions: node["actions"].as_u64().unwrap_or(0),
                properties: node["properties"].clone(),
            }
        })
        .collect()
}

/// The node with `role`; with `label`, the one with that label.
pub fn node(host: &mut TestHost, role: &str, label: Option<&str>) -> Node {
    nodes(host)
        .into_iter()
        .find(|node| node.role == role && label.is_none_or(|label| node.label() == Some(label)))
        .unwrap_or_else(|| panic!("no {role} node {label:?}"))
}

/// The color of the painted pixel at logical `(x, y)`.
pub fn pixel(host: &mut TestHost, x: f64, y: f64) -> [u8; 3] {
    let scale = f64::from(host.scale_factor());
    let image = host.screenshot_image().expect("screenshot");
    let pixel = image.get_pixel((x * scale) as u32, (y * scale) as u32);
    [pixel[0], pixel[1], pixel[2]]
}

/// Whether `actual` is within `tolerance` of the hex color `expected` in every channel.
pub fn is_color(actual: [u8; 3], expected: u32, tolerance: u8) -> bool {
    let expected = [
        (expected >> 16) as u8,
        (expected >> 8) as u8,
        expected as u8,
    ];
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(e) <= tolerance)
}

/// Presses Tab (a key event, so the window enters keyboard modality, then traversal).
pub fn tab(host: &mut TestHost) {
    host.simulate_keystrokes("tab", None).expect("tab");
}

/// The window's center of the node's bounds in logical pixels.
pub fn center(host: &mut TestHost, node: &Node) -> (f64, f64) {
    let scale = f64::from(host.scale_factor());
    let (x0, y0, x1, y1) = node.bounds();
    ((x0 + x1) / 2.0 / scale, (y0 + y1) / 2.0 / scale)
}

/// Delivers the accessibility click action to `node`, as assistive technology would.
pub fn a11y_click(host: &mut TestHost, node: &Node) {
    let window = host.window();
    host.app()
        .simulate_accessibility_action(
            window,
            vespertine::accesskit::ActionRequest {
                action: vespertine::accesskit::Action::Click,
                target_tree: vespertine::accesskit::TreeId::ROOT,
                target_node: vespertine::accesskit::NodeId(node.id),
                data: None,
            },
        )
        .expect("the action is delivered");
    host.flush();
}

/// Delivers the next display frame the way the platform's frame loop does: runs the callbacks
/// scheduled with `on_next_frame` (frame requests of animations, per-frame coalescing) and
/// returns how many ran, then paints what they invalidated.
pub fn next_frame(host: &mut TestHost) -> usize {
    let window = host.window();
    let ran = host
        .app()
        .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
        .expect("the window is open");
    host.flush();
    ran
}
