//! The native retained scene: turns the host tree (APP-006) into elements for each frame.
//!
//! The scene keeps one persistent [`HostNode`] per tree node (focus handle, scroll handle,
//! listener table, report state) and builds a fresh element tree from the retained tree on every
//! render. Styles go through [`HostStyle`] (ELM-006), listeners through [`HostNode`] (INP-006),
//! `img` and `svg` through the image hosts (ELM-004). The scene is shared by the live window view
//! and by the offscreen test host.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use vespertine::host_events::{
    HostContext, HostEvent, HostKeyEvent, HostNode, HostPointerEvent, HostScroll, HostTouchPhase,
};
use vespertine::{
    AnyElement, App, Bounds, DeclarativeAria, ElementId, HostStyle, ImageHostState,
    InteractiveElement, IntoElement, ParentElement, Pixels, Styled, SvgHostState, Window, div,
    point, px,
};
use vespertine_host_tree::{HostTree, NodeId, NodeRef};

/// Receives the events of the scene: `(app, element id, event type, payload fields)`. The
/// payload is a JSON object merged into the event the JavaScript side receives.
pub type EmitEvent = Rc<dyn Fn(&App, u64, &str, Value)>;

/// Called with the painted border-box of a node, after it painted.
pub type PaintedHook = Rc<dyn Fn(u64, Bounds<Pixels>, &mut App)>;

/// Element types that scroll their content by default.
const SCROLL_STYLE_KEYS: [(&str, &str); 3] = [
    ("overflow", "both"),
    ("overflowX", "x"),
    ("overflowY", "y"),
];

struct SceneNode {
    host: HostNode,
    /// The events this node's listeners are currently registered for.
    registered: HashSet<&'static str>,
    image: Option<ImageHostState>,
    svg: Option<SvgHostState>,
}

struct ParsedStyle {
    /// Keeps the shared payload alive so its address is not reused while cached.
    _source: Arc<Value>,
    style: Option<Rc<HostStyle>>,
}

/// The scene of one host tree.
pub struct HostScene {
    context: HostContext,
    nodes: HashMap<NodeId, SceneNode>,
    emit: EmitEvent,
    generation: Arc<AtomicU64>,
    painted: Option<PaintedHook>,
    styles: HashMap<usize, ParsedStyle>,
    /// Style payloads that failed to decode, reported once.
    rejected: HashSet<usize>,
}

fn mods(event: &HostPointerEvent) -> [(&'static str, bool); 4] {
    [
        ("shift", event.modifiers.shift),
        ("ctrl", event.modifiers.ctrl),
        ("alt", event.modifiers.alt),
        ("cmd", event.modifiers.cmd),
    ]
}

fn pointer_payload(event: &HostPointerEvent) -> Value {
    let mut payload = json!({
        "x": event.x,
        "y": event.y,
        "clickCount": event.click_count,
        "button": event.button,
    });
    for (name, value) in mods(event) {
        payload[name] = Value::Bool(value);
    }
    payload
}

fn key_payload(event: &HostKeyEvent) -> Value {
    json!({
        "key": event.key,
        "char": event.key_char,
        "held": event.held,
        "keystroke": event.dispatch.id(),
        "shift": event.modifiers.shift,
        "ctrl": event.modifiers.ctrl,
        "alt": event.modifiers.alt,
        "cmd": event.modifiers.cmd,
    })
}

/// The wire event types that map to host listeners, with their [`HostEvent`].
fn host_event(name: &str) -> Option<(&'static str, HostEvent)> {
    Some(match name {
        "pointerDown" => ("pointerDown", HostEvent::PointerDown),
        "pointerUp" => ("pointerUp", HostEvent::PointerUp),
        "pointerMove" => ("pointerMove", HostEvent::PointerMove),
        "enter" => ("enter", HostEvent::Enter),
        "leave" => ("leave", HostEvent::Leave),
        "click" => ("click", HostEvent::Click),
        "auxClick" => ("auxClick", HostEvent::AuxClick),
        "contextMenu" => ("contextMenu", HostEvent::ContextMenu),
        "outsideDown" => ("outsideDown", HostEvent::OutsideDown),
        "wheel" => ("wheel", HostEvent::Scroll),
        "keyDown" => ("keyDown", HostEvent::KeyDown),
        "keyUp" => ("keyUp", HostEvent::KeyUp),
        "focus" => ("focus", HostEvent::Focus),
        "blur" => ("blur", HostEvent::Blur),
        "fileDrop" => ("fileDrop", HostEvent::FileDrop),
        "layout" => ("layout", HostEvent::Layout),
        "scrollChange" => ("scrollChange", HostEvent::ScrollChange),
        _ => return None,
    })
}

impl HostScene {
    /// A scene delivering events through `emit`. `generation` is the identity of the mounted
    /// root's window-level key events (announced by `setWindowGeneration`).
    pub fn new(emit: EmitEvent, generation: Arc<AtomicU64>) -> Self {
        let context = HostContext::new();
        for key_up in [false, true] {
            let emit = emit.clone();
            let generation = generation.clone();
            let listener = move |event: &HostKeyEvent, _: &mut Window, cx: &mut App| {
                let name = if key_up { "windowKeyUp" } else { "windowKeyDown" };
                emit(cx, generation.load(Ordering::SeqCst), name, key_payload(event));
            };
            if key_up {
                context.set_window_key_up(Some(listener));
            } else {
                context.set_window_key_down(Some(listener));
            }
        }
        Self {
            context,
            nodes: HashMap::new(),
            emit,
            generation,
            painted: None,
            styles: HashMap::new(),
            rejected: HashSet::new(),
        }
    }

    /// Installs the hook that learns each node's painted bounds.
    pub fn set_painted_hook(&mut self, hook: Option<PaintedHook>) {
        self.painted = hook;
    }

    /// The identity window-level events currently carry.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The node's focus handle, when it is focusable.
    pub fn focus_handle(&self, id: NodeId) -> Option<vespertine::FocusHandle> {
        let node = &self.nodes.get(&id)?.host;
        node.is_focusable().then(|| node.focus_handle())
    }

    /// The persistent host node of a tree node, when it has been rendered.
    pub fn node(&self, id: NodeId) -> Option<&HostNode> {
        self.nodes.get(&id).map(|node| &node.host)
    }

    // ---- frame ---------------------------------------------------------------------------

    /// Builds the elements of one frame from `tree`.
    pub fn render(&mut self, tree: &HostTree, window: &mut Window, cx: &mut App) -> AnyElement {
        self.sync(tree, window, cx);
        let content = match tree.root() {
            Some(root) => self.build(tree, root, window, cx),
            None => div().into_any_element(),
        };
        div()
            .size_full()
            .child(self.context.window_hooks())
            .child(content)
            .into_any_element()
    }

    /// Creates host nodes for new tree nodes, drops those of destroyed nodes and brings every
    /// node's listeners, focus configuration and scroll mode in line with the tree.
    fn sync(&mut self, tree: &HostTree, window: &mut Window, cx: &mut App) {
        let live: HashSet<NodeId> = tree.node_ids().collect();
        let dead: Vec<NodeId> = self
            .nodes
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in dead {
            if let Some(mut node) = self.nodes.remove(&id) {
                if let Some(image) = node.image.as_mut() {
                    image.release(window, cx);
                }
            }
        }
        for id in live {
            let Some(node) = tree.node(id) else { continue };
            let scene_node = self.nodes.entry(id).or_insert_with(|| SceneNode {
                host: HostNode::new(&self.context, ElementId::Integer(id), cx),
                registered: HashSet::new(),
                image: None,
                svg: None,
            });
            Self::configure(&self.emit, id, scene_node, node);
        }
    }

    fn configure(emit: &EmitEvent, id: NodeId, scene_node: &mut SceneNode, node: NodeRef<'_>) {
        let host = &scene_node.host;
        let desired: HashSet<&'static str> = node.listeners().filter_map(|name| host_event(name).map(|(name, _)| name)).collect();
        for name in scene_node.registered.difference(&desired).copied().collect::<Vec<_>>() {
            if let Some((_, event)) = host_event(name) {
                host.remove_listener(event);
            }
        }
        for name in desired.difference(&scene_node.registered).copied().collect::<Vec<_>>() {
            register(emit, id, host, name);
        }
        scene_node.registered = desired;

        host.set_editor(matches!(node.element_type(), "input" | "textarea"));
        host.set_tab_index(
            node.prop("tabIndex")
                .and_then(Value::as_i64)
                .map(|index| index as isize),
        );
        host.set_autofocus(node.auto_focus());
        host.set_scroll(scroll_mode(node.style().map(|style| &**style)));

        match node.element_type() {
            "img" => {
                let image = scene_node.image.get_or_insert_with(ImageHostState::new);
                image.set_src(node.prop("src").and_then(Value::as_str));
                image.set_object_fit(node.prop("objectFit").and_then(Value::as_str));
                image.set_alt(node.prop("alt").and_then(Value::as_str));
            }
            "svg" => {
                let svg = scene_node.svg.get_or_insert_with(SvgHostState::new);
                svg.set_src(node.prop("src").and_then(Value::as_str));
                svg.set_source(node.prop("source").and_then(Value::as_str));
            }
            _ => {}
        }
    }

    /// The parsed style of a node. A payload that does not decode is treated as no style (the
    /// tree rejects such a payload before it is stored when it validates with the same schema).
    fn style_of(&mut self, node: NodeRef<'_>) -> Option<Rc<HostStyle>> {
        let shared = node.style()?;
        let key = Arc::as_ptr(shared) as usize;
        if let Some(parsed) = self.styles.get(&key) {
            return parsed.style.clone();
        }
        let style = match HostStyle::from_json(shared, "setStyle") {
            Ok(style) => Some(Rc::new(style)),
            Err(error) => {
                if self.rejected.insert(key) {
                    log::warn!("host style rejected: {error}");
                }
                None
            }
        };
        self.styles.insert(
            key,
            ParsedStyle {
                _source: shared.clone(),
                style: style.clone(),
            },
        );
        style
    }

    /// Releases parsed styles the tree no longer references.
    fn sweep_styles(&mut self, tree: &HostTree) {
        if self.styles.len() <= 2 * tree.len().max(32) {
            return;
        }
        let live: HashSet<usize> = tree
            .node_ids()
            .filter_map(|id| tree.node(id)?.style().map(|style| Arc::as_ptr(style) as usize))
            .collect();
        self.styles.retain(|key, _| live.contains(key));
        self.rejected.retain(|key| live.contains(key));
    }

    fn build(&mut self, tree: &HostTree, id: NodeId, window: &mut Window, cx: &mut App) -> AnyElement {
        self.sweep_styles(tree);
        let Some(node) = tree.node(id) else {
            return div().into_any_element();
        };
        let style = self.style_of(node);
        let element_type = node.element_type();
        let painted = self.painted.clone();

        let Some(scene_node) = self.nodes.get_mut(&id) else {
            return div().into_any_element();
        };
        let mut host = scene_node.host.element(window, cx);
        if let Some(style) = &style {
            host = style.apply(host);
        }
        if let Some(role) = node.prop("role").and_then(Value::as_str) {
            host = host.dom_role(role);
        }
        for (key, value) in node.props() {
            if key.starts_with("aria-") {
                host = host.aria_attribute(key, value);
            }
        }
        if let Some(painted) = painted {
            host = host.on_painted(move |bounds, _, cx| painted(id, bounds, cx));
        }

        match element_type {
            "img" => {
                if let Some(image) = scene_node.image.as_mut() {
                    let refinement = style.as_ref().map(|style| style.to_refinement()).unwrap_or_default();
                    let mut element = image.element(ElementId::Integer(id), refinement, window, cx);
                    // The image element carries the host's identity for bounds tracking.
                    if let Some(painted) = self.painted.clone() {
                        element = element.on_painted(move |bounds, _, cx| painted(id, bounds, cx));
                    }
                    return element.into_any_element();
                }
            }
            "svg" => {
                if let Some(svg) = scene_node.svg.as_mut() {
                    let refinement = style.as_ref().map(|style| style.to_refinement()).unwrap_or_default();
                    return svg.render(ElementId::Integer(id), refinement, cx);
                }
            }
            _ => {}
        }

        if let Some(text) = node.text() {
            host = host.child(text.to_string());
        }
        let children: Vec<NodeId> = node.children().collect();
        for child in children {
            let element = self.build(tree, child, window, cx);
            host = host.child(element);
        }
        host.into_any_element()
    }

    // ---- ref-like operations --------------------------------------------------------------

    /// Focuses a node. Returns whether the node exists and can take focus.
    pub fn focus(&self, id: NodeId, window: &mut Window, cx: &mut App) -> bool {
        match self.nodes.get(&id) {
            Some(node) if node.host.is_focusable() => {
                node.host.focus(window, cx);
                true
            }
            _ => false,
        }
    }

    /// Removes focus when the node holds it.
    pub fn blur(&self, id: NodeId, window: &mut Window, cx: &mut App) {
        if let Some(node) = self.nodes.get(&id) {
            node.host.blur(window, cx);
        }
    }

    /// The node that holds keyboard focus, if it is a node of this scene.
    pub fn focused(&self, window: &Window, cx: &App) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|(_, node)| node.host.focus_handle().is_focused(window))
            .map(|(id, _)| *id)
            .or_else(|| {
                // A descendant of a focusable node can hold the focus handle's focus path.
                let _ = cx;
                None
            })
    }

    /// Moves focus to the next (`forward`) or previous tab stop, optionally restricted to the
    /// subtree of `scope`.
    pub fn focus_traverse(&self, tree: &HostTree, scope: Option<NodeId>, forward: bool, window: &mut Window, cx: &mut App) {
        match scope {
            None => {
                if let Some(node) = self.nodes.values().next() {
                    if forward {
                        node.host.focus_next(window, cx);
                    } else {
                        node.host.focus_prev(window, cx);
                    }
                }
            }
            Some(root) => {
                let mut members = Vec::new();
                let mut stack = vec![root];
                while let Some(id) = stack.pop() {
                    if let Some(node) = tree.node(id) {
                        if let Some(scene) = self.nodes.get(&id) {
                            if scene.host.is_focusable() {
                                members.push(scene.host.clone());
                            }
                        }
                        stack.extend(node.children());
                    }
                }
                if forward {
                    HostNode::focus_next_in(&members, window, cx);
                } else {
                    HostNode::focus_prev_in(&members, window, cx);
                }
            }
        }
    }

    /// The scroll offset of a scrolling node `(x, y)`, zero at the start and negative toward the
    /// far side.
    pub fn scroll_offset(&self, id: NodeId) -> Option<(f32, f32)> {
        let handle = self.nodes.get(&id)?.host.scroll_handle()?;
        let offset = handle.offset();
        Some((offset.x.into(), offset.y.into()))
    }

    /// Scrolls a scrolling node to an absolute offset (positive numbers scroll toward the far side).
    pub fn scroll_to(&self, id: NodeId, x: f32, y: f32, window: &mut Window) -> bool {
        match self.nodes.get(&id) {
            Some(node) if node.host.scroll_handle().is_some() => {
                node.host.scroll_to(point(px(-x), px(-y)), window);
                true
            }
            _ => false,
        }
    }

    /// Scrolls the nearest scrolling ancestor of `id` so that the branch containing the node is
    /// visible. Returns whether an ancestor scrolled.
    pub fn scroll_into_view(&self, tree: &HostTree, id: NodeId, window: &mut Window) -> bool {
        let mut child = id;
        let mut parent = tree.node(id).and_then(|node| node.parent());
        while let Some(current) = parent {
            let Some(node) = tree.node(current) else { return false };
            if let Some(handle) = self.nodes.get(&current).and_then(|scene| scene.host.scroll_handle()) {
                let index = node.children().position(|candidate| candidate == child);
                if let Some(index) = index {
                    handle.scroll_to_item(index);
                    window.refresh();
                    return true;
                }
            }
            child = current;
            parent = node.parent();
        }
        false
    }

    /// Replaces an `img` node's image with packed pixels.
    pub fn set_image_pixels(&mut self, id: NodeId, width: f64, height: f64, bytes: &[u8], format: Option<&str>, window: &mut Window) -> Result<(), String> {
        let image = self
            .nodes
            .get_mut(&id)
            .and_then(|node| node.image.as_mut())
            .ok_or_else(|| "the element is not an image".to_string())?;
        image
            .set_image_pixels(width, height, bytes, format)
            .map_err(|error| error.to_string())?;
        window.refresh();
        Ok(())
    }

    /// Replaces an `img` node's image with encoded bytes.
    pub fn set_image(&mut self, id: NodeId, bytes: &[u8], window: &mut Window, cx: &App) -> Result<(), String> {
        let image = self
            .nodes
            .get_mut(&id)
            .and_then(|node| node.image.as_mut())
            .ok_or_else(|| "the element is not an image".to_string())?;
        image.set_image(bytes, cx).map_err(|error| error.to_string())?;
        window.refresh();
        Ok(())
    }
}

/// The scroll mode a style asks for through `overflow`, `overflowX` and `overflowY`.
fn scroll_mode(style: Option<&Value>) -> HostScroll {
    let Some(Value::Object(style)) = style else {
        return HostScroll::None;
    };
    let scrolls = |key: &str| matches!(style.get(key).and_then(Value::as_str), Some("scroll" | "auto"));
    let (mut x, mut y) = (false, false);
    for (key, axis) in SCROLL_STYLE_KEYS {
        if scrolls(key) {
            match axis {
                "both" => (x, y) = (true, true),
                "x" => x = true,
                _ => y = true,
            }
        }
    }
    match (x, y) {
        (true, true) => HostScroll::Both,
        (true, false) => HostScroll::Horizontal,
        (false, true) => HostScroll::Vertical,
        (false, false) => HostScroll::None,
    }
}

/// Registers the listener of event `name` on a host node; it reports through `emit`.
fn register(emit: &EmitEvent, id: NodeId, host: &HostNode, name: &'static str) {
    let pointer = |name: &'static str| {
        let emit = emit.clone();
        move |event: &HostPointerEvent, _: &mut Window, cx: &mut App| {
            emit(cx, id, name, pointer_payload(event));
        }
    };
    match name {
        "pointerDown" => host.on_pointer_down(pointer("pointerDown")),
        "pointerUp" => host.on_pointer_up(pointer("pointerUp")),
        "pointerMove" => host.on_pointer_move(pointer("pointerMove")),
        "enter" => host.on_enter(pointer("enter")),
        "leave" => host.on_leave(pointer("leave")),
        "click" => host.on_click(pointer("click")),
        "auxClick" => host.on_aux_click(pointer("auxClick")),
        "contextMenu" => host.on_context_menu(pointer("contextMenu")),
        "outsideDown" => host.on_outside_down(pointer("outsideDown")),
        "wheel" => {
            let emit = emit.clone();
            host.on_scroll(move |event, _, cx| {
                let phase = match event.phase {
                    HostTouchPhase::Started => "started",
                    HostTouchPhase::Moved => "moved",
                    HostTouchPhase::Ended => "ended",
                };
                emit(
                    cx,
                    id,
                    "wheel",
                    json!({
                        "x": event.x,
                        "y": event.y,
                        "deltaX": event.delta_x,
                        "deltaY": event.delta_y,
                        "precise": event.precise,
                        "phase": phase,
                        "shift": event.modifiers.shift,
                        "ctrl": event.modifiers.ctrl,
                        "alt": event.modifiers.alt,
                        "cmd": event.modifiers.cmd,
                    }),
                );
            });
        }
        "keyDown" | "keyUp" => {
            let emit = emit.clone();
            let listener = move |event: &HostKeyEvent, _: &mut Window, cx: &mut App| {
                emit(cx, id, name, key_payload(event));
            };
            if name == "keyDown" {
                host.on_key_down(listener);
            } else {
                host.on_key_up(listener);
            }
        }
        "focus" | "blur" => {
            let emit = emit.clone();
            let listener = move |_: &mut Window, cx: &mut App| emit(cx, id, name, json!({}));
            if name == "focus" {
                host.on_focus(listener);
            } else {
                host.on_blur(listener);
            }
        }
        "fileDrop" => {
            let emit = emit.clone();
            host.on_file_drop(move |event, _, cx| {
                let paths: Vec<String> = event.paths.iter().map(|path| path.to_string_lossy().into_owned()).collect();
                emit(cx, id, "fileDrop", json!({ "paths": paths, "x": event.x, "y": event.y }));
            });
        }
        "layout" => {
            let emit = emit.clone();
            host.on_layout(move |event, _, cx| {
                emit(cx, id, "layout", json!({ "x": event.x, "y": event.y, "width": event.width, "height": event.height }));
            });
        }
        "scrollChange" => {
            let emit = emit.clone();
            host.on_scroll_change(move |event, _, cx| {
                emit(cx, id, "scrollChange", json!({ "x": event.x, "y": event.y }));
            });
        }
        _ => {}
    }
}
