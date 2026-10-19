//! Plain data exchanged with the test host: geometry, tree nodes, events, modifiers.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use vespertine::{Bounds, Modifiers, MouseButton, Pixels, Point, Size, point, px, size};

/// A rectangle in logical pixels, window coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl Rect {
    /// Whether `(x, y)` lies inside the rectangle (left and top edges inclusive, right and
    /// bottom edges exclusive, like hit testing).
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// The center of the rectangle: `(x + width / 2, y + height / 2)`.
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

impl From<Bounds<Pixels>> for Rect {
    fn from(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: f64::from(f32::from(bounds.origin.x)),
            y: f64::from(f32::from(bounds.origin.y)),
            width: f64::from(f32::from(bounds.size.width)),
            height: f64::from(f32::from(bounds.size.height)),
        }
    }
}

/// The logical size of the offscreen window.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// Width in logical pixels.
    pub width: f64,
    /// Height in logical pixels.
    pub height: f64,
}

/// One retained element as serialized for inspection and automation.
///
/// The wire form (see [`TreeNode::to_json`]) uses the field names `id`, `type`, `text`,
/// `testId`, `style`, `events`, `customProps`, `bounds` and `children`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    /// Stable identity chosen by the sender.
    pub id: u64,
    /// The element type string.
    #[serde(rename = "type")]
    pub element_type: String,
    /// The element's own text, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The stable locator identity for automation, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_id: Option<String>,
    /// The element's resolved style, as a JSON object.
    #[serde(default)]
    pub style: Map<String, Value>,
    /// Event types the element has listeners for.
    #[serde(default)]
    pub events: Vec<String>,
    /// Custom properties (excluding the lifted `testId` and `autoFocus`).
    #[serde(default)]
    pub custom_props: Map<String, Value>,
    /// The border-box bounds of the element in the last painted frame; absent when the
    /// element was not painted (hidden, not yet painted, or only speculatively painted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Rect>,
    /// Children in document order.
    #[serde(default)]
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    /// The node as the JSON value of the automation protocol.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Visits this node and its descendants in document (pre-)order.
    pub fn walk<'a>(&'a self, visit: &mut impl FnMut(&'a TreeNode)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }

    /// Mutable pre-order visit.
    pub fn walk_mut(&mut self, visit: &mut impl FnMut(&mut TreeNode)) {
        visit(self);
        for child in &mut self.children {
            child.walk_mut(visit);
        }
    }

    /// The text content of the node: its own text followed by the text content of each
    /// descendant in document order, with no separators.
    pub fn text_content(&self) -> String {
        let mut content = String::new();
        self.walk(&mut |node| {
            if let Some(text) = &node.text {
                content.push_str(text);
            }
        });
        content
    }
}

/// An event produced by a native element and destined for the framework adapter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeEvent {
    /// Identity of the element the event originated from.
    pub element_id: u64,
    /// The event type (for example `click`, `input`, `keydown`).
    pub event_type: String,
    /// Event data.
    #[serde(default)]
    pub payload: Value,
}

/// A logical scroll anchor of a virtual list: the first mounted row at the viewport top.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAnchor {
    /// Item index; equal to the item count for the at-end sentinel.
    pub item_index: usize,
    /// Offset within the item in pixels.
    pub offset: f64,
    /// Viewport height in pixels.
    pub viewport_height: f64,
}

/// A painted highlight wash with its geometry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaintedWash {
    /// Identity of the element owning the run the wash belongs to.
    pub element_id: u64,
    /// Sub-index of the run within the element.
    pub run_index: usize,
    /// The run's full text.
    pub text: String,
    /// UTF-16 start of the wash within the text.
    pub start_utf16: usize,
    /// UTF-16 end of the wash within the text.
    pub end_utf16: usize,
    /// Whether this is the active match.
    pub active: bool,
    /// One painted rectangle per visual row.
    pub rects: Vec<Rect>,
}

/// Counters of the shared syntax coloration cache: hits, misses, cached documents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxCacheCounters {
    /// Lookups served from the cache.
    pub hits: u64,
    /// Lookups that had to compute a coloration.
    pub misses: u64,
    /// Documents currently cached.
    pub documents: u64,
}

/// Parses a modifier string such as `ctrl-shift`. Words are `alt`, `cmd` or `meta`, `ctrl` or
/// `control`, and `shift`, separated by hyphens; unknown words are ignored.
pub fn parse_modifiers(modifiers: &str) -> Modifiers {
    let mut parsed = Modifiers::none();
    for word in modifiers.split('-') {
        match word.trim().to_ascii_lowercase().as_str() {
            "alt" => parsed.alt = true,
            "cmd" | "meta" => parsed.platform = true,
            "ctrl" | "control" => parsed.control = true,
            "shift" => parsed.shift = true,
            _ => {}
        }
    }
    parsed
}

/// Maps a mouse button index (0 = left, 1 = middle, 2 = right) to a button. Other values are
/// not buttons.
pub fn mouse_button(index: u8) -> Option<MouseButton> {
    match index {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    }
}

pub(crate) fn logical_point(x: f64, y: f64) -> Point<Pixels> {
    point(px(x as f32), px(y as f32))
}

pub(crate) fn logical_size(width: f64, height: f64) -> Size<Pixels> {
    size(px(width as f32), px(height as f32))
}
