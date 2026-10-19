//! The window's root view: turns the retained host tree into elements for each frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use std::sync::Mutex;
use vespertine::{
    AnyElement, Context, Hsla, IntoElement, ParentElement, Render, Rgba, Styled, Window, div, px,
};
use vespertine_host_tree::{HostTree, NodeId};

/// The retained tree shared between the JavaScript thread (which mutates it) and the UI
/// thread (which draws it).
pub type SharedTree = Arc<Mutex<HostTree>>;

pub struct HostView {
    tree: SharedTree,
    renders: Arc<AtomicU64>,
}

impl HostView {
    /// `renders` is incremented every time the view builds its elements for a frame.
    pub fn new(tree: SharedTree, renders: Arc<AtomicU64>) -> Self {
        Self { tree, renders }
    }
}

impl Render for HostView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.fetch_add(1, Ordering::Relaxed);
        let tree = self
            .tree
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match tree.root() {
            Some(root) => build(&tree, root),
            None => div().size_full().into_any_element(),
        }
    }
}

fn build(tree: &HostTree, id: NodeId) -> AnyElement {
    let Some(node) = tree.node(id) else {
        return div().into_any_element();
    };
    let mut element = div();
    if let Some(style) = node.style() {
        element = apply_style(element, style);
    }
    if let Some(text) = node.text() {
        element = element.child(text.to_string());
    }
    for child in node.children() {
        element = element.child(build(tree, child));
    }
    element.into_any_element()
}

fn number(style: &Value, key: &str) -> Option<f32> {
    style.get(key)?.as_f64().map(|value| value as f32)
}

fn color(style: &Value, key: &str) -> Option<Hsla> {
    let text = style.get(key)?.as_str()?.trim().strip_prefix('#')?;
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(text.get(range)?, 16).ok();
    let (r, g, b, a) = match text.len() {
        3 => {
            let expand = |i: usize| channel(i..i + 1).map(|v| v * 17);
            (expand(0)?, expand(1)?, expand(2)?, 255)
        }
        6 => (channel(0..2)?, channel(2..4)?, channel(4..6)?, 255),
        8 => (
            channel(0..2)?,
            channel(2..4)?,
            channel(4..6)?,
            channel(6..8)?,
        ),
        _ => return None,
    };
    Some(
        Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
        .into(),
    )
}

fn apply_style(mut element: vespertine::Div, style: &Value) -> vespertine::Div {
    element = element.flex();
    if style.get("flexDirection").and_then(Value::as_str) == Some("column") {
        element = element.flex_col();
    }
    if let Some(width) = number(style, "width") {
        element = element.w(px(width));
    }
    if let Some(height) = number(style, "height") {
        element = element.h(px(height));
    }
    if let Some(gap) = number(style, "gap") {
        element = element.gap(px(gap));
    }
    if let Some(padding) = number(style, "padding") {
        element = element.p(px(padding));
    }
    if let Some(grow) = number(style, "flexGrow") {
        element = element.flex_grow(grow);
    }
    if let Some(radius) = number(style, "borderRadius") {
        element = element.rounded(px(radius));
    }
    if let Some(size) = number(style, "fontSize") {
        element = element.text_size(px(size));
    }
    if let Some(background) = color(style, "backgroundColor") {
        element = element.bg(background);
    }
    if let Some(text) = color(style, "color") {
        element = element.text_color(text);
    }
    element
}
