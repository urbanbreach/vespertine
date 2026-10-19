//! A minimal retained tree that implements [`RetainedHost`], used by this crate's tests, the
//! automation fixture application and examples.
//!
//! This is a test fixture, not the retained host tree of the framework (APP-006): it understands
//! the wire format's mutation tuples (`createElement`, `destroyElement`, `appendChild`,
//! `insertBefore`, `setStyle`, `setText`, `setEventListener`, `setRoot`, `setCustomProp`) and
//! builds a small fixed set of elements from them so that the host and the automation layer can
//! be exercised end to end through real layout, painting, hit testing and input dispatch.
//!
//! Element types: `div` (container), `text` (a div whose own text is painted), `input` (a
//! focusable single-line field: printable keys append, `backspace` deletes, `ctrl-a` selects all
//! so the next key replaces the value, `enter` emits `submit`).
//!
//! Style keys: `width`, `height` (pixels, or `"100%"`), `backgroundColor`, `color`
//! (`#rrggbb` / `#rrggbbaa`), `flexDirection` (`row` | `column`), `gap`, `padding`, `fontSize`,
//! `position` (`absolute`) with `left` / `top`, `overflow` (`scroll`), `display` (`none`),
//! `visibility` (`hidden`). Events: listener types `click`, `mousedown`, `mouseup`, `mousemove`
//! queue a [`NativeEvent`] with the pointer position; `input` and `submit` are emitted by inputs.

use crate::{HostError, HostEvents, HostResult, NativeEvent, PaintLog, RetainedHost, TreeNode};
use serde_json::{Map, Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    rc::Rc,
};
use vespertine::{
    AnyElement, App, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, ScrollHandle, StatefulInteractiveElement, Styled, Window, canvas, div, point,
    px, relative, rgba,
};

const MAX_SAFE_ID: f64 = 9007199254740991.0;

#[derive(Default)]
struct Node {
    element_type: String,
    parent: Option<u64>,
    children: Vec<u64>,
    text: Option<String>,
    style: Map<String, Value>,
    listeners: BTreeSet<String>,
    props: Map<String, Value>,
    test_id: Option<String>,
    value: String,
    select_all: bool,
}

#[derive(Default)]
struct State {
    nodes: HashMap<u64, Node>,
    root: Option<u64>,
    focus: HashMap<u64, FocusHandle>,
    scroll: HashMap<u64, ScrollHandle>,
}

/// The fixture retained tree. Cloning shares the tree.
#[derive(Clone, Default)]
pub struct FixtureTree {
    state: Rc<RefCell<State>>,
}

enum Op {
    Create(u64, String),
    Destroy(u64),
    Append(u64, u64),
    InsertBefore(u64, u64, u64),
    Style(u64, Map<String, Value>),
    Text(u64, String),
    Listener(u64, String, bool),
    Root(u64),
    Prop(u64, String, Value),
}

fn id_arg(value: Option<&Value>) -> Option<u64> {
    let number = value?.as_f64()?;
    (number.is_finite() && number >= 0.0 && number.fract() == 0.0 && number <= MAX_SAFE_ID)
        .then_some(number as u64)
}

fn string_arg(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_owned)
}

fn decode(index: usize, tuple: &[Value]) -> Result<Op, String> {
    let fail = |what: &str| format!("operation {index}: {what}");
    let name = tuple
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| fail("the operation name must be a string"))?;
    let id =
        |position: usize| id_arg(tuple.get(position)).ok_or_else(|| fail("invalid identifier"));
    let string = |position: usize| {
        string_arg(tuple.get(position)).ok_or_else(|| fail("expected a string argument"))
    };
    Ok(match name {
        "createElement" => Op::Create(id(1)?, string(2)?),
        "destroyElement" => Op::Destroy(id(1)?),
        "appendChild" => Op::Append(id(1)?, id(2)?),
        "insertBefore" => Op::InsertBefore(id(1)?, id(2)?, id(3)?),
        "setStyle" => match tuple.get(2) {
            Some(Value::Object(style)) => Op::Style(id(1)?, style.clone()),
            _ => return Err(fail("the style must be an object")),
        },
        "setText" => Op::Text(id(1)?, string(2)?),
        "setEventListener" => match tuple.get(3) {
            Some(Value::Bool(on)) => Op::Listener(id(1)?, string(2)?, *on),
            _ => return Err(fail("hasHandler must be a boolean")),
        },
        "setRoot" => Op::Root(id(1)?),
        "setCustomProp" => Op::Prop(
            id(1)?,
            string(2)?,
            tuple.get(3).cloned().unwrap_or(Value::Null),
        ),
        other => return Err(fail(&format!("unknown operation {other:?}"))),
    })
}

impl State {
    fn unlink(&mut self, child: u64) {
        let parent = self
            .nodes
            .get_mut(&child)
            .and_then(|node| node.parent.take());
        if let Some(parent) = parent.and_then(|parent| self.nodes.get_mut(&parent)) {
            parent.children.retain(|existing| *existing != child);
        }
    }

    fn destroy(&mut self, id: u64, destroyed: &mut Vec<u64>) {
        if !self.nodes.contains_key(&id) {
            return;
        }
        self.unlink(id);
        self.destroy_subtree(id, destroyed);
    }

    fn destroy_subtree(&mut self, id: u64, destroyed: &mut Vec<u64>) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        destroyed.push(id);
        self.focus.remove(&id);
        self.scroll.remove(&id);
        if self.root == Some(id) {
            self.root = None;
        }
        for child in node.children {
            self.destroy_subtree(child, destroyed);
        }
    }

    fn apply(&mut self, op: Op, destroyed: &mut Vec<u64>) {
        match op {
            Op::Create(id, element_type) => {
                self.nodes.insert(
                    id,
                    Node {
                        element_type,
                        ..Node::default()
                    },
                );
            }
            Op::Destroy(id) => self.destroy(id, destroyed),
            Op::Append(parent, child) => {
                if self.nodes.contains_key(&child) {
                    self.unlink(child);
                    if let Some(parent_node) = self.nodes.get_mut(&parent) {
                        parent_node.children.push(child);
                        self.nodes.get_mut(&child).expect("checked above").parent = Some(parent);
                    }
                }
            }
            Op::InsertBefore(parent, child, before) => {
                if self.nodes.contains_key(&child) {
                    self.unlink(child);
                    if let Some(parent_node) = self.nodes.get_mut(&parent) {
                        let at = parent_node
                            .children
                            .iter()
                            .position(|existing| *existing == before)
                            .unwrap_or(parent_node.children.len());
                        parent_node.children.insert(at, child);
                        self.nodes.get_mut(&child).expect("checked above").parent = Some(parent);
                    }
                }
            }
            Op::Style(id, style) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.style = style;
                }
            }
            Op::Text(id, text) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.text = Some(text);
                }
            }
            Op::Listener(id, event_type, on) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    if on {
                        node.listeners.insert(event_type);
                    } else {
                        node.listeners.remove(&event_type);
                    }
                }
            }
            Op::Root(id) => self.root = Some(id),
            Op::Prop(id, key, value) => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    match key.as_str() {
                        "testId" => node.test_id = value.as_str().map(str::to_owned),
                        _ if value.is_null() => {
                            node.props.remove(&key);
                        }
                        _ => {
                            node.props.insert(key, value);
                        }
                    }
                }
            }
        }
    }

    fn snapshot(&self, id: u64) -> Option<TreeNode> {
        let node = self.nodes.get(&id)?;
        let mut custom_props = node.props.clone();
        if node.element_type == "input" {
            custom_props.insert("value".to_owned(), Value::String(node.value.clone()));
        }
        Some(TreeNode {
            id,
            element_type: node.element_type.clone(),
            text: node.text.clone(),
            test_id: node.test_id.clone(),
            style: node.style.clone(),
            events: node.listeners.iter().cloned().collect(),
            custom_props,
            bounds: None,
            children: node
                .children
                .iter()
                .filter_map(|child| self.snapshot(*child))
                .collect(),
        })
    }
}

fn color(value: &Value) -> Option<vespertine::Rgba> {
    let hex = value.as_str()?.strip_prefix('#')?;
    let parsed = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some(rgba(parsed << 8 | 0xff)),
        8 => Some(rgba(parsed)),
        _ => None,
    }
}

fn number(value: Option<&Value>) -> Option<f32> {
    value?.as_f64().map(|number| number as f32)
}

fn emit(cx: &App, id: u64, event_type: &str, payload: Value) {
    HostEvents::emit(
        cx,
        NativeEvent {
            element_id: id,
            event_type: event_type.to_owned(),
            payload,
        },
    );
}

/// Appends the event type to the element's `received` property (an array of event types, the
/// last 256 kept) so that automation can observe what the element received.
fn record(state: &Rc<RefCell<State>>, id: u64, event_type: &str) {
    let mut state = state.borrow_mut();
    let Some(node) = state.nodes.get_mut(&id) else {
        return;
    };
    let received = node
        .props
        .entry("received".to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(received) = received {
        received.push(Value::String(event_type.to_owned()));
        if received.len() > 256 {
            received.remove(0);
        }
    }
}

fn button_index(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => 3,
    }
}

impl FixtureTree {
    /// An empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    fn build(&self, id: u64, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let (node, children) = {
            let state = self.state.borrow();
            let node = state.nodes.get(&id)?;
            if node.style.get("display").and_then(Value::as_str) == Some("none") {
                return None;
            }
            (
                (
                    node.element_type.clone(),
                    node.style.clone(),
                    node.listeners.clone(),
                    node.text.clone(),
                    node.value.clone(),
                    node.props.contains_key("tabIndex"),
                ),
                node.children.clone(),
            )
        };
        let (element_type, style, listeners, text, value, has_tab_index) = node;
        let is_input = element_type == "input";
        let scrolls = style.get("overflow").and_then(Value::as_str) == Some("scroll");

        let mut element = div().id(("fixture", id as usize));
        match style.get("width") {
            Some(Value::String(width)) if width == "100%" => element = element.w(relative(1.0)),
            width => {
                if let Some(width) = number(width) {
                    element = element.w(px(width));
                }
            }
        }
        match style.get("height") {
            Some(Value::String(height)) if height == "100%" => element = element.h(relative(1.0)),
            height => {
                if let Some(height) = number(height) {
                    element = element.h(px(height));
                }
            }
        }
        if let Some(background) = style.get("backgroundColor").and_then(color) {
            element = element.bg(background);
        }
        if let Some(text_color) = style.get("color").and_then(color) {
            element = element.text_color(text_color);
        }
        if let Some(size) = number(style.get("fontSize")) {
            element = element.text_size(px(size));
        }
        element = element.flex().flex_shrink_0();
        if style.get("flexDirection").and_then(Value::as_str) == Some("column") {
            element = element.flex_col();
        }
        if let Some(gap) = number(style.get("gap")) {
            element = element.gap(px(gap));
        }
        if let Some(padding) = number(style.get("padding")) {
            element = element.p(px(padding));
        }
        if style.get("position").and_then(Value::as_str) == Some("absolute") {
            element = element.absolute();
            if let Some(left) = number(style.get("left")) {
                element = element.left(px(left));
            }
            if let Some(top) = number(style.get("top")) {
                element = element.top(px(top));
            }
        }
        if style.get("visibility").and_then(Value::as_str) == Some("hidden") {
            element = element.invisible();
        }
        if scrolls {
            let handle = self
                .state
                .borrow_mut()
                .scroll
                .entry(id)
                .or_insert_with(ScrollHandle::new)
                .clone();
            element = element.overflow_scroll().track_scroll(&handle);
        }

        if is_input || has_tab_index {
            let handle = self
                .state
                .borrow_mut()
                .focus
                .entry(id)
                .or_insert_with(|| cx.focus_handle())
                .clone();
            element = element.track_focus(&handle).tab_stop(true);
        }
        if is_input {
            let state = self.state.clone();
            element = element.on_key_down(move |event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                let mut state = state.borrow_mut();
                let Some(node) = state.nodes.get_mut(&id) else {
                    return;
                };
                let changed = match keystroke.key.as_str() {
                    "a" if keystroke.modifiers.control || keystroke.modifiers.platform => {
                        node.select_all = true;
                        false
                    }
                    "backspace" => {
                        if std::mem::take(&mut node.select_all) {
                            node.value.clear();
                        } else {
                            node.value.pop();
                        }
                        true
                    }
                    "enter" => {
                        let value = node.value.clone();
                        drop(state);
                        emit(cx, id, "submit", json!({ "value": value }));
                        return;
                    }
                    _ => match keystroke.key_char.as_deref() {
                        Some(text)
                            if !keystroke.modifiers.control && !keystroke.modifiers.platform =>
                        {
                            if std::mem::take(&mut node.select_all) {
                                node.value.clear();
                            }
                            node.value.push_str(text);
                            true
                        }
                        _ => false,
                    },
                };
                if changed {
                    let value = node.value.clone();
                    drop(state);
                    emit(cx, id, "input", json!({ "value": value }));
                }
                window.refresh();
            });
        }
        for listener in &listeners {
            let state = self.state.clone();
            element = match listener.as_str() {
                "click" => element.on_click(move |_event, _window, cx| {
                    record(&state, id, "click");
                    emit(cx, id, "click", Value::Null)
                }),
                "mousedown" => {
                    element.on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                        record(&state, id, "mousedown");
                        emit(
                            cx,
                            id,
                            "mousedown",
                            json!({
                                "x": f32::from(event.position.x),
                                "y": f32::from(event.position.y),
                                "button": button_index(event.button),
                                "shift": event.modifiers.shift,
                                "ctrl": event.modifiers.control,
                            }),
                        )
                    })
                }
                "mouseup" => element.on_mouse_up(MouseButton::Left, move |event, _window, cx| {
                    record(&state, id, "mouseup");
                    emit(
                        cx,
                        id,
                        "mouseup",
                        json!({
                            "x": f32::from(event.position.x),
                            "y": f32::from(event.position.y),
                            "shift": event.modifiers.shift,
                            "ctrl": event.modifiers.control,
                        }),
                    )
                }),
                "mousemove" => element.on_mouse_move(move |event, _window, cx| {
                    record(&state, id, "mousemove");
                    emit(
                        cx,
                        id,
                        "mousemove",
                        json!({
                            "x": f32::from(event.position.x),
                            "y": f32::from(event.position.y),
                            "pressed": event.pressed_button.map(button_index),
                            "shift": event.modifiers.shift,
                            "ctrl": event.modifiers.control,
                        }),
                    )
                }),
                _ => element,
            };
        }

        let mut element = element.child(
            canvas(
                |_bounds, _window, _cx| (),
                move |bounds, _, _window, cx| PaintLog::record_bounds(cx, id, bounds),
            )
            .absolute()
            .size_full(),
        );
        if is_input {
            element = element.child(value);
        } else if let Some(text) = text {
            element = element.child(text);
        }
        for child in children {
            if let Some(child) = self.build(child, window, cx) {
                element = element.child(child);
            }
        }
        Some(element.into_any_element())
    }
}

impl RetainedHost for FixtureTree {
    fn apply_batch(&mut self, batch: &str) -> Result<Vec<u64>, String> {
        let tuples: Vec<Vec<Value>> =
            serde_json::from_str(batch).map_err(|error| format!("invalid batch: {error}"))?;
        let ops = tuples
            .iter()
            .enumerate()
            .map(|(index, tuple)| decode(index, tuple))
            .collect::<Result<Vec<_>, _>>()?;
        let mut destroyed = Vec::new();
        let mut state = self.state.borrow_mut();
        for op in ops {
            state.apply(op, &mut destroyed);
        }
        Ok(destroyed)
    }

    fn render(&mut self, window: &mut Window, cx: &mut App) -> AnyElement {
        let root = self.state.borrow().root;
        root.and_then(|root| self.build(root, window, cx))
            .unwrap_or_else(|| div().into_any_element())
    }

    fn root(&self) -> Option<u64> {
        self.state.borrow().root
    }

    fn element_count(&self) -> usize {
        self.state.borrow().nodes.len()
    }

    fn snapshot(&self) -> Option<TreeNode> {
        let state = self.state.borrow();
        state.snapshot(state.root?)
    }

    fn focus_handle(&self, id: u64, _cx: &App) -> Option<FocusHandle> {
        self.state.borrow().focus.get(&id).cloned()
    }

    fn focused_element(&self, window: &Window, _cx: &App) -> Option<u64> {
        let state = self.state.borrow();
        let mut focused: Vec<u64> = state
            .focus
            .iter()
            .filter(|(_, handle)| handle.is_focused(window))
            .map(|(id, _)| *id)
            .collect();
        focused.sort_unstable();
        focused.first().copied()
    }

    fn scroll_offset(&self, id: u64, _cx: &App) -> Option<(f64, f64)> {
        let state = self.state.borrow();
        let offset = state.scroll.get(&id)?.offset();
        Some((
            -f64::from(f32::from(offset.x)),
            -f64::from(f32::from(offset.y)),
        ))
    }

    fn scroll_to(
        &mut self,
        id: u64,
        x: f64,
        y: f64,
        _window: &mut Window,
        _cx: &mut App,
    ) -> HostResult<()> {
        let state = self.state.borrow();
        let handle = state.scroll.get(&id).ok_or(HostError::NoSuchElement(id))?;
        handle.set_offset(point(px(-(x as f32)), px(-(y as f32))));
        Ok(())
    }
}

/// The standard automation scenario: a 400 x 300 column holding a title, a text input
/// (`name`), a button (`save`), a scrolling list of ten items (`list`), a drag area (`drag`)
/// and a hidden element (`ghost`). When `echo` is given a text element `env` shows it.
///
/// Both the in-process tests and the launchable fixture application build their tree from this
/// batch, so the same scenario runs against both backends.
pub fn scenario_batch(echo: Option<String>) -> Value {
    let mut ops: Vec<Value> = vec![
        json!(["createElement", 1, "div"]),
        json!(["setStyle", 1, {"width": 400, "height": 300, "flexDirection": "column", "backgroundColor": "#ffffff", "gap": 4, "fontSize": 14, "color": "#111111"}]),
        json!(["createElement", 2, "text"]),
        json!(["setText", 2, "Fixture"]),
        json!(["setCustomProp", 2, "testId", "title"]),
        json!(["createElement", 3, "input"]),
        json!(["setStyle", 3, {"width": 200, "height": 28, "backgroundColor": "#eeeeee"}]),
        json!(["setCustomProp", 3, "testId", "name"]),
        json!(["createElement", 4, "div"]),
        json!(["setStyle", 4, {"width": 80, "height": 30, "backgroundColor": "#3355ff", "color": "#ffffff"}]),
        json!(["setText", 4, "Save"]),
        json!(["setCustomProp", 4, "testId", "save"]),
        json!(["setEventListener", 4, "click", true]),
        json!(["setEventListener", 4, "mousedown", true]),
        json!(["setEventListener", 4, "mouseup", true]),
        json!(["createElement", 5, "div"]),
        json!(["setStyle", 5, {"width": 200, "height": 90, "flexDirection": "column", "overflow": "scroll", "backgroundColor": "#f4f4f4"}]),
        json!(["setCustomProp", 5, "testId", "list"]),
    ];
    for item in 0..10u64 {
        let id = 10 + item;
        ops.push(json!(["createElement", id, "text"]));
        ops.push(json!(["setStyle", id, {"height": 30, "width": "100%"}]));
        ops.push(json!(["setText", id, format!("Item {item}")]));
        ops.push(json!(["appendChild", 5, id]));
    }
    ops.extend([
        json!(["createElement", 6, "div"]),
        json!(["setStyle", 6, {"width": 200, "height": 50, "backgroundColor": "#ffddaa"}]),
        json!(["setCustomProp", 6, "testId", "drag"]),
        json!(["setEventListener", 6, "mousedown", true]),
        json!(["setEventListener", 6, "mousemove", true]),
        json!(["setEventListener", 6, "mouseup", true]),
        json!(["createElement", 7, "text"]),
        json!(["setStyle", 7, {"visibility": "hidden"}]),
        json!(["setText", 7, "ghost"]),
        json!(["setCustomProp", 7, "testId", "ghost"]),
    ]);
    let mut children = vec![2, 3, 4, 5, 6, 7];
    if let Some(echo) = echo {
        ops.push(json!(["createElement", 8, "text"]));
        ops.push(json!(["setText", 8, echo]));
        ops.push(json!(["setCustomProp", 8, "testId", "env"]));
        children.push(8);
    }
    for child in children {
        ops.push(json!(["appendChild", 1, child]));
    }
    ops.push(json!(["setRoot", 1]));
    Value::Array(ops)
}
