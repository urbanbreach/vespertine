//! The native retained scene: turns the host tree (APP-006) into elements for each frame.
//!
//! The scene keeps one persistent [`HostNode`] per tree node (focus handle, scroll handle,
//! listener table, report state) and builds a fresh element tree from the retained tree on every
//! render. Styles go through [`HostStyle`] (ELM-006), listeners through [`HostNode`] (INP-006),
//! `img` and `svg` through the image hosts (ELM-004). The scene is shared by the live window view
//! and by the offscreen test host.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use vespertine::host_events::{
    HostContext, HostElement, HostEvent, HostKeyEvent, HostNode, HostPointerEvent, HostScroll,
    HostTouchPhase,
};
use vespertine::motion::{
    Easing, MotionDescription, MotionExt as _, MotionInitial, MotionProps, MotionTransition,
};
use vespertine::{
    AnyElement, App, Bounds, CustomElementRegistry, DeclarativeAria, ElementId, HostStyle,
    ImageHostState, InteractiveElement, IntoElement, ListAlignment, ListOffset, ListState,
    ParentElement, Pixels, Styled, SvgHostState, Window, div, list, point, px,
};
use vespertine::{HostId, KeyboardFocusPath, RenderContext, SharedString, Subscription};
use vespertine_host_tree::{HostTree, NodeId, NodeRef};
use vespertine_richtext::{
    RichHostText, RichText, RunKey, TextService, UserSelect, highlight,
    search::spec::{HighlightDeclaration, default_hex_color},
    selection_color, text_root, user_select,
};

/// Receives the events of the scene: `(app, element id, event type, payload fields)`. The
/// payload is a JSON object merged into the event the JavaScript side receives.
pub type EmitEvent = Rc<dyn Fn(&App, u64, &str, Value)>;

/// Called with the painted border-box of a node, after it painted.
pub type PaintedHook = Rc<dyn Fn(u64, Bounds<Pixels>, &mut App)>;

/// A search wash a text run painted: the owning node, the run's full text, the UTF-16 range and
/// one rectangle `(x, y, width, height)` per visual row.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneWash {
    pub element_id: NodeId,
    pub text: String,
    pub start_utf16: usize,
    pub end_utf16: usize,
    pub active: bool,
    pub rects: Vec<[f32; 4]>,
}

/// Called after a frame painted with the washes it painted, in paint order.
pub type WashesHook = Rc<dyn Fn(&[SceneWash], &mut App)>;

/// Element types that scroll their content by default.
const SCROLL_STYLE_KEYS: [(&str, &str); 3] =
    [("overflow", "both"), ("overflowX", "x"), ("overflowY", "y")];

/// Temporary read access to the retained tree. Lets a row of a virtual list be built again
/// during layout, when the tree is not otherwise borrowed.
pub trait TreeSource {
    /// Runs `read` with the tree.
    fn read(&self, read: &mut dyn FnMut(&HostTree));
}

impl TreeSource for Arc<std::sync::Mutex<HostTree>> {
    fn read(&self, read: &mut dyn FnMut(&HostTree)) {
        read(&self.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    }
}

impl TreeSource for Rc<RefCell<HostTree>> {
    fn read(&self, read: &mut dyn FnMut(&HostTree)) {
        read(&self.borrow());
    }
}

/// The configuration properties of a `virtual-list` node (ELM-001 host element).
#[derive(Clone, Debug, PartialEq)]
struct ListConfig {
    bottom: bool,
    follow_tail: bool,
    overdraw: f32,
    estimate: Option<f32>,
    item_count: Option<f64>,
    window_start: usize,
}

impl ListConfig {
    fn of(node: NodeRef<'_>) -> Self {
        let number = |name: &str| node.prop(name).and_then(Value::as_f64);
        Self {
            bottom: node.prop("alignment").and_then(Value::as_str) == Some("bottom"),
            follow_tail: node
                .prop("followTail")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            overdraw: number("overdraw")
                .filter(|value| value.is_finite())
                .map_or(512.0, |value| value.max(0.0) as f32),
            estimate: number("estimatedItemHeight")
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|value| value as f32),
            item_count: number("itemCount")
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(f64::trunc),
            window_start: number("windowStart")
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map_or(0, |value| value as usize),
        }
    }
}

/// The native state of a `virtual-list` node: a persistent [`ListState`] that follows the
/// node's keyed children through splices, so measurements, hints and the scroll anchor survive
/// rerenders (ELM-001 R13, R15).
struct ListHost {
    state: ListState,
    /// The keys (child identities) of the rows the state currently describes.
    keys: Vec<NodeId>,
    config: Option<ListConfig>,
    /// Whether the node subscribed to `visibleRange`; shared with the scroll handler.
    reports_range: Rc<Cell<bool>>,
}

impl ListHost {
    fn new(emit: &EmitEvent, id: NodeId) -> Self {
        let state = ListState::new(0, ListAlignment::Top, px(512.));
        let reports_range = Rc::new(Cell::new(false));
        let enabled = reports_range.clone();
        let emit = emit.clone();
        state.set_scroll_handler(move |event, _, cx| {
            if enabled.get() {
                emit(
                    cx,
                    id,
                    "visibleRange",
                    json!({ "start": event.visible_range.start, "end": event.visible_range.end, "count": event.count }),
                );
            }
        });
        Self {
            state,
            keys: Vec::new(),
            config: None,
            reports_range,
        }
    }

    /// Brings the state in line with the node: configuration changes first (each only when it
    /// changed), then the keyed row sequence.
    fn sync(&mut self, node: NodeRef<'_>) {
        let config = ListConfig::of(node);
        if self.config.as_ref() != Some(&config) {
            let previous = self.config.as_ref();
            if previous.is_none_or(|old| old.bottom != config.bottom) {
                self.state.set_alignment(if config.bottom {
                    ListAlignment::Bottom
                } else {
                    ListAlignment::Top
                });
            }
            if previous.is_none_or(|old| old.overdraw != config.overdraw) {
                self.state.set_overdraw(px(config.overdraw));
            }
            if previous.is_none_or(|old| old.estimate != config.estimate) {
                self.state
                    .set_estimated_item_height(config.estimate.map(px));
            }
            if previous.is_none_or(|old| old.follow_tail != config.follow_tail) {
                self.state.set_follow_tail(config.follow_tail);
            }
            if previous.is_none_or(|old| old.item_count != config.item_count) {
                self.state.set_logical_item_count(config.item_count);
            }
        }
        let keys: Vec<NodeId> = node.children().collect();
        if self.state.is_windowed() {
            self.state
                .reconcile_keys_in_window(config.window_start, &self.keys, &keys);
        } else {
            self.state.reconcile_keys(&self.keys, &keys);
        }
        self.keys = keys;
        self.config = Some(config);
    }
}

struct SceneNode {
    host: HostNode,
    /// The events this node's listeners are currently registered for.
    registered: HashSet<&'static str>,
    image: Option<ImageHostState>,
    svg: Option<SvgHostState>,
    list: Option<ListHost>,
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
    /// The window's text service (selection, search washes), known after the first render.
    service: Option<TextService>,
    /// Whether selection changes are reported as `windowSelectionChange` events.
    selection_notifications: Rc<Cell<bool>>,
    /// The node each text run (identified by its hashed owner) belongs to.
    run_owners: Rc<RefCell<HashMap<u64, NodeId>>>,
    washes: Option<WashesHook>,
    /// This scene, for rows of virtual lists that are built again during layout.
    me: std::rc::Weak<RefCell<HostScene>>,
    tree: Rc<dyn TreeSource>,
    /// The persistent adapters of the registry types (ELM-007), one per live host identity.
    registry: Rc<RefCell<CustomElementRegistry>>,
    /// Destroys every adapter when the application quits, while its entities still exist.
    quit: Option<Subscription>,
    /// The control holding keyboard focus and its ancestors; `None` when nothing is focused or
    /// the focus is a text editor.
    focus_path: Option<KeyboardFocusPath>,
}

/// A motion property value from JSON. A value that is not a number becomes NaN, which the track
/// validation rejects (ANM-002 R5), so a malformed description shows the ordinary style.
fn motion_number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

fn motion_props(value: &Value) -> MotionProps {
    let mut props = MotionProps::new();
    let Some(map) = value.as_object() else {
        return MotionProps::new().width(f64::NAN);
    };
    for (name, value) in map {
        let number = motion_number(value);
        props = match name.as_str() {
            "width" => props.width(number),
            "height" => props.height(number),
            "top" => props.top(number),
            "right" => props.right(number),
            "bottom" => props.bottom(number),
            "left" => props.left(number),
            "borderRadius" => props.border_radius(number),
            "opacity" => props.opacity(number),
            _ => props,
        };
    }
    props
}

fn motion_easing(value: &Value) -> Easing {
    match value {
        Value::String(name) => Easing::named(name),
        Value::Array(points) if points.len() == 4 => {
            let point = |index: usize| motion_number(&points[index]) as f32;
            Easing::cubic_bezier(point(0), point(1), point(2), point(3))
        }
        _ => Easing::cubic_bezier(f32::NAN, 0.0, 0.0, 0.0),
    }
}

/// The motion description of a node's `motion` property (ANM-002 public API): `initial`
/// (false, or a style), `animate`, `exit`, `transition` (`duration` and `delay` in seconds, and
/// `ease` as a name or four Bezier coordinates), plus an optional completion `generation` and
/// `present` flag.
fn motion_description(value: &Value) -> Option<MotionDescription> {
    let map = value.as_object()?;
    let mut description = MotionDescription::new(
        map.get("animate")
            .map(motion_props)
            .unwrap_or_else(MotionProps::new),
    );
    match map.get("initial") {
        None | Some(Value::Null) => {}
        Some(Value::Bool(false)) => description = description.initial(MotionInitial::False),
        Some(Value::Object(_)) => {
            description = description.initial(MotionInitial::Style(motion_props(&map["initial"])))
        }
        Some(_) => description = description.initial(MotionInitial::True),
    }
    if let Some(exit) = map.get("exit").filter(|exit| !exit.is_null()) {
        description = description.exit(motion_props(exit));
    }
    if let Some(Value::Object(transition)) = map.get("transition") {
        let mut timing = MotionTransition::new();
        if let Some(duration) = transition.get("duration") {
            timing = timing.duration(motion_number(duration));
        }
        if let Some(delay) = transition.get("delay") {
            timing = timing.delay(motion_number(delay));
        }
        if let Some(ease) = transition.get("ease") {
            timing = timing.easing(motion_easing(ease));
        }
        description = description.transition(timing);
    }
    if let Some(generation) = map.get("generation").and_then(Value::as_u64) {
        description = description.generation(generation);
    }
    if let Some(present) = map.get("present").and_then(Value::as_bool) {
        description = description.present(present);
    }
    Some(description)
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

/// The wire name and payload of an event an adapter emitted (ELM-007 R9).
fn host_event_payload(event: &vespertine::HostEvent) -> (String, Value) {
    use vespertine::HostEvent as Adapter;
    let modifiers = |payload: &mut Value, modifiers: &vespertine::Modifiers| {
        payload["shift"] = Value::Bool(modifiers.shift);
        payload["ctrl"] = Value::Bool(modifiers.control);
        payload["alt"] = Value::Bool(modifiers.alt);
        payload["cmd"] = Value::Bool(modifiers.platform);
    };
    let name = event.name().to_owned();
    let payload = match event {
        Adapter::Click(click) => {
            let mut payload = json!({
                "x": f32::from(click.position.x),
                "y": f32::from(click.position.y),
                "button": click.button,
                "clickCount": click.click_count,
            });
            modifiers(&mut payload, &click.modifiers);
            payload
        }
        Adapter::Enter(hover) | Adapter::Leave(hover) => {
            let mut payload = json!({
                "x": f32::from(hover.position.x),
                "y": f32::from(hover.position.y),
            });
            modifiers(&mut payload, &hover.modifiers);
            payload
        }
        Adapter::FileDrop(drop) => json!({
            "paths": drop.paths,
            "x": f32::from(drop.position.x),
            "y": f32::from(drop.position.y),
        }),
        Adapter::Custom { payload, .. } => payload.clone(),
    };
    (name, payload)
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
    /// A shared scene delivering events through `emit`. `generation` is the identity of the
    /// mounted root's window-level key events (announced by `setWindowGeneration`); `tree`
    /// gives rows of virtual lists read access to the retained tree during layout.
    pub fn new(
        emit: EmitEvent,
        generation: Arc<AtomicU64>,
        tree: Rc<dyn TreeSource>,
    ) -> Rc<RefCell<Self>> {
        let context = HostContext::new();
        for key_up in [false, true] {
            let emit = emit.clone();
            let generation = generation.clone();
            let listener = move |event: &HostKeyEvent, _: &mut Window, cx: &mut App| {
                let name = if key_up {
                    "windowKeyUp"
                } else {
                    "windowKeyDown"
                };
                emit(
                    cx,
                    generation.load(Ordering::SeqCst),
                    name,
                    key_payload(event),
                );
            };
            if key_up {
                context.set_window_key_up(Some(listener));
            } else {
                context.set_window_key_down(Some(listener));
            }
        }
        Rc::new_cyclic(|me| {
            RefCell::new(Self {
                context,
                nodes: HashMap::new(),
                emit,
                generation,
                painted: None,
                styles: HashMap::new(),
                rejected: HashSet::new(),
                service: None,
                selection_notifications: Rc::new(Cell::new(false)),
                run_owners: Rc::new(RefCell::new(HashMap::new())),
                washes: None,
                me: me.clone(),
                tree,
                registry: Rc::new(RefCell::new(CustomElementRegistry::default())),
                quit: None,
                focus_path: None,
            })
        })
    }

    /// The registry of persistent element adapters (ELM-007). Host types with a registered
    /// factory render through it; register the factory of a type to enable it.
    pub fn registry(&self) -> Rc<RefCell<CustomElementRegistry>> {
        self.registry.clone()
    }

    /// Installs the hook that learns each node's painted bounds.
    pub fn set_painted_hook(&mut self, hook: Option<PaintedHook>) {
        self.painted = hook;
    }

    /// Installs the hook that learns the search washes each frame painted.
    pub fn set_washes_hook(&mut self, hook: Option<WashesHook>) {
        self.washes = hook;
    }

    /// Enables or disables `windowSelectionChange` events (identified by the window generation).
    pub fn set_selection_notifications(&self, enabled: bool) {
        self.selection_notifications.set(enabled);
    }

    /// The flag behind [`Self::set_selection_notifications`], for owners that outlive access to
    /// the scene (the test host owns the tree it renders).
    pub fn selection_notifications(&self) -> Rc<Cell<bool>> {
        self.selection_notifications.clone()
    }

    /// The selected document text, `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        self.service.as_ref()?.selected_text()
    }

    /// Clears the document selection.
    pub fn clear_selection(&self, window: &mut Window, cx: &mut App) {
        if let Some(service) = &self.service {
            service.clear_selection(window, cx);
        }
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
        self.install_text_service(window, cx);
        self.run_owners.borrow_mut().clear();
        self.focus_path = self.keyboard_focus_path(tree, window, cx);
        let content = match tree.root() {
            Some(root) => self.build(tree, root, window, cx),
            None => div().into_any_element(),
        };
        let service = self.service.clone();
        let washes = self.washes.clone();
        let owners = self.run_owners.clone();
        // Painted last: reports the washes the frame's text runs painted.
        let report = vespertine::canvas(
            |_, _, _| (),
            move |_, _, _, cx| {
                let (Some(service), Some(hook)) = (service, washes) else {
                    return;
                };
                let owners = owners.borrow();
                let painted: Vec<SceneWash> = service
                    .painted_highlights()
                    .into_iter()
                    .filter_map(|wash| {
                        Some(SceneWash {
                            element_id: *owners.get(&wash.key.owner)?,
                            text: wash.text,
                            start_utf16: wash.start,
                            end_utf16: wash.end,
                            active: wash.active,
                            rects: wash.rects,
                        })
                    })
                    .collect();
                hook(&painted, cx);
            },
        );
        text_root(
            div()
                .size_full()
                .child(self.context.window_hooks())
                .child(content)
                .child(report),
        )
        .into_any_element()
    }

    /// Looks up the window's text service and reports selection changes through the emitter.
    fn install_text_service(&mut self, window: &Window, cx: &mut App) {
        if self.service.is_some() {
            return;
        }
        self.quit = Some(CustomElementRegistry::destroy_on_quit(&self.registry, cx));
        let service = TextService::of(window, cx);
        let emit = self.emit.clone();
        let generation = self.generation.clone();
        let enabled = self.selection_notifications.clone();
        service.on_selection_change(move |text, _, cx| {
            if enabled.get() {
                emit(
                    cx,
                    generation.load(Ordering::SeqCst),
                    "windowSelectionChange",
                    json!({ "text": text }),
                );
            }
        });
        self.service = Some(service);
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
            self.registry.borrow_mut().destroy(HostId(id), window, cx);
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
                list: None,
            });
            Self::configure(&self.emit, id, scene_node, node);
        }
        // An identity that now has a type without an adapter (or none of the registry's types)
        // loses its adapter; a type change between adapter types is handled when it renders.
        let registry = self.registry.clone();
        let mut registry = registry.borrow_mut();
        let live_adapters: HashSet<NodeId> = tree
            .node_ids()
            .filter(|id| {
                tree.node(*id)
                    .is_some_and(|node| Self::adapter_type(&registry, node.element_type()))
            })
            .collect();
        registry.prune_missing(|host| live_adapters.contains(&host.0), window, cx);
    }

    /// Whether `element_type` renders through the adapter registry: it has a factory and is not
    /// one of the types the scene renders itself.
    fn adapter_type(registry: &CustomElementRegistry, element_type: &str) -> bool {
        !matches!(
            element_type,
            "img" | "svg" | "virtual-list" | "div" | "text"
        ) && registry.has_factory(element_type)
    }

    /// The path from the focused node up to the root; `None` when nothing is focused or the focus
    /// is a text editor (ELM-007 R7).
    fn keyboard_focus_path(
        &self,
        tree: &HostTree,
        window: &Window,
        cx: &App,
    ) -> Option<KeyboardFocusPath> {
        let mut at = Some(self.focused(window, cx)?);
        let focused = tree.node(at?)?;
        if matches!(focused.element_type(), "input" | "textarea") {
            return None;
        }
        let mut ids = Vec::new();
        while let Some(id) = at {
            ids.push(HostId(id));
            at = tree.node(id).and_then(|node| node.parent());
        }
        Some(KeyboardFocusPath { ids })
    }

    /// The render context of an adapter node: its properties, events, style, built children and
    /// the focus, selection and highlight context inherited from its ancestors (ELM-007 R7).
    fn adapter_context(
        &mut self,
        tree: &HostTree,
        node: NodeRef<'_>,
        style: Option<Rc<HostStyle>>,
        children: Vec<AnyElement>,
    ) -> RenderContext {
        let id = node.id();
        let emit = self.emit.clone();
        let sink: vespertine::EmitEvent = Rc::new(move |event, _, cx| {
            let (name, payload) = host_event_payload(&event);
            emit(cx, id, &name, payload);
        });
        let mut context = RenderContext::new(HostId(id), sink);
        context.properties = Arc::new(
            node.props()
                .map(|(key, value)| (key.to_owned(), value.clone()))
                .collect(),
        );
        context.events = node.listeners().map(SharedString::from).collect();
        context.focus_handle = self.focus_handle(id);
        context.keyboard_focus = self.focus_path.clone();
        context.style = style
            .map(|style| Arc::new((*style).clone()))
            .unwrap_or_default();
        context.children = children;
        context.text = Some(Rc::new(RichHostText));
        let mut ancestor = node.parent();
        let (mut selectable, mut color, mut highlight) = (None, None, None);
        while let Some(parent) = ancestor.and_then(|parent| tree.node(parent)) {
            if let Some(Value::Object(style)) = parent.style().map(|style| &**style) {
                if selectable.is_none() {
                    selectable = match style.get("userSelect").and_then(Value::as_str) {
                        Some("none") => Some(false),
                        Some("text") => Some(true),
                        _ => None,
                    };
                }
                if color.is_none() {
                    color = style
                        .get("selectionColor")
                        .and_then(Value::as_str)
                        .and_then(vespertine::css_color_to_rgba)
                        .map(vespertine::Hsla::from);
                }
            }
            if highlight.is_none() {
                highlight = parent
                    .prop("highlight")
                    .map(|value| Arc::new(value.clone()));
            }
            ancestor = parent.parent();
        }
        context.selectable = selectable.unwrap_or(true);
        if let Some(color) = color {
            context.selection_color = color;
        }
        context.search_highlight = highlight;
        context
    }

    fn configure(emit: &EmitEvent, id: NodeId, scene_node: &mut SceneNode, node: NodeRef<'_>) {
        let host = &scene_node.host;
        let desired: HashSet<&'static str> = node
            .listeners()
            .filter_map(|name| host_event(name).map(|(name, _)| name))
            .collect();
        for name in scene_node
            .registered
            .difference(&desired)
            .copied()
            .collect::<Vec<_>>()
        {
            if let Some((_, event)) = host_event(name) {
                host.remove_listener(event);
            }
        }
        for name in desired
            .difference(&scene_node.registered)
            .copied()
            .collect::<Vec<_>>()
        {
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
            "virtual-list" => {
                let list = scene_node
                    .list
                    .get_or_insert_with(|| ListHost::new(emit, id));
                list.reports_range
                    .set(node.listeners().any(|name| name == "visibleRange"));
                list.sync(node);
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
            .filter_map(|id| {
                tree.node(id)?
                    .style()
                    .map(|style| Arc::as_ptr(style) as usize)
            })
            .collect();
        self.styles.retain(|key, _| live.contains(key));
        self.rejected.retain(|key| live.contains(key));
    }

    fn build(
        &mut self,
        tree: &HostTree,
        id: NodeId,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.sweep_styles(tree);
        let Some(node) = tree.node(id) else {
            return div().into_any_element();
        };
        let style = self.style_of(node);
        let element_type = node.element_type();
        let painted = self.painted.clone();

        if Self::adapter_type(&self.registry.borrow(), element_type) {
            let children: Vec<NodeId> = node.children().collect();
            let children = children
                .into_iter()
                .map(|child| self.build(tree, child, window, cx))
                .collect();
            let context = self.adapter_context(tree, node, style, children);
            let registry = self.registry.clone();
            let mut registry = registry.borrow_mut();
            // An adapter's content may be painted after the rest of the frame (deferred), so its
            // bounds are reported when it paints rather than at the end of the frame.
            let observer: Option<vespertine::BoundsObserver> = painted.map(|painted| {
                Rc::new(move |bounds: Bounds<Pixels>, cx: &mut App| painted(id, bounds, cx)) as _
            });
            let element = registry.render(element_type, context, window, cx);
            registry.observe_bounds(HostId(id), observer);
            return element;
        }

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
                    let refinement = style
                        .as_ref()
                        .map(|style| style.to_refinement())
                        .unwrap_or_default();
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
                    let refinement = style
                        .as_ref()
                        .map(|style| style.to_refinement())
                        .unwrap_or_default();
                    return svg.render(ElementId::Integer(id), refinement, cx);
                }
            }
            "virtual-list" => {
                if let Some(list_host) = scene_node.list.as_ref() {
                    let source = self.tree.clone();
                    let scene = self.me.clone();
                    let rows = list(list_host.state.clone(), move |ix, window, cx| {
                        let mut row = None;
                        if let Some(scene) = scene.upgrade() {
                            source.read(&mut |tree| {
                                let child = tree.node(id).and_then(|list| list.children().nth(ix));
                                row = child
                                    .map(|child| scene.borrow_mut().build(tree, child, window, cx));
                            });
                        }
                        row.unwrap_or_else(|| div().into_any_element())
                    });
                    return self.with_motion(id, node, host.child(rows.w_full().h_full()));
                }
            }
            _ => {}
        }

        if let Some(text) = node.text() {
            // The run is selectable and searchable; its key addresses it for the wash report.
            let key = RunKey::new(id, 0);
            self.run_owners.borrow_mut().insert(key.owner, id);
            host = host.child(
                RichText::new(ElementId::NamedInteger("text".into(), id), text.to_string())
                    .key(key),
            );
        }
        let children: Vec<NodeId> = node.children().collect();
        for child in children {
            let element = self.build(tree, child, window, cx);
            host = host.child(element);
        }
        let mut element = self.with_motion(id, node, host);
        // `userSelect` and `selectionColor` are inherited by the subtree's text runs.
        if let Some(Value::Object(style)) = node.style().map(|style| &**style) {
            if let Some(color) = style
                .get("selectionColor")
                .and_then(Value::as_str)
                .and_then(vespertine::css_color_to_rgba)
            {
                element =
                    selection_color(vespertine::Hsla::from(color), element).into_any_element();
            }
            match style.get("userSelect").and_then(Value::as_str) {
                Some("none") => element = user_select(UserSelect::None, element).into_any_element(),
                Some("text") => element = user_select(UserSelect::Text, element).into_any_element(),
                _ => {}
            }
        }
        let Some(declaration) = node.prop("highlight") else {
            return element;
        };
        let declaration = HighlightDeclaration::from_value(declaration, &default_hex_color);
        let mut scope = highlight(
            ElementId::NamedInteger("highlight".into(), id),
            declaration,
            element,
        );
        if node.listeners().any(|name| name == "highlight") {
            let emit = self.emit.clone();
            scope = scope.on_match_count(move |count, _, cx| {
                emit(cx, id, "highlight", json!({ "matchCount": count }))
            });
        }
        scope.into_any_element()
    }

    /// Finishes a node's host element: a node with a `motion` property has its style driven by
    /// a declarative motion track (ANM-002), keyed by the node identity so the track survives
    /// rerenders. Completion reports reach a `motionComplete` listener.
    fn with_motion(&self, id: NodeId, node: NodeRef<'_>, host: HostElement) -> AnyElement {
        let Some(description) = node.prop("motion").and_then(motion_description) else {
            return host.into_any_element();
        };
        let mut element =
            host.with_motion(ElementId::NamedInteger("motion".into(), id), description);
        if node.listeners().any(|name| name == "motionComplete") {
            let emit = self.emit.clone();
            element = element.on_motion_complete(move |completion, _, cx| {
                emit(
                    cx,
                    id,
                    "motionComplete",
                    json!({ "generation": completion.generation, "exit": completion.exit }),
                )
            });
        }
        element.into_any_element()
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
    pub fn focus_traverse(
        &self,
        tree: &HostTree,
        scope: Option<NodeId>,
        forward: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
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
        let node = self.nodes.get(&id)?;
        if let Some(list) = &node.list {
            let offset = list.state.scroll_px_offset_for_scrollbar();
            return Some((f32::from(offset.x) + 0.0, f32::from(offset.y) + 0.0));
        }
        let offset = node.host.scroll_handle()?.offset();
        // `+ 0.0` turns a negative zero (an unscrolled axis) into a plain zero.
        let (x, y): (f32, f32) = (offset.x.into(), offset.y.into());
        Some((x + 0.0, y + 0.0))
    }

    /// Scrolls a scrolling node to an absolute offset (positive numbers scroll toward the far side).
    pub fn scroll_to(&self, id: NodeId, x: f32, y: f32, window: &mut Window) -> bool {
        let Some(node) = self.nodes.get(&id) else {
            return false;
        };
        if let Some(list) = &node.list {
            list.state.set_offset_from_scrollbar(point(px(-x), px(-y)));
            window.refresh();
            return true;
        }
        if node.host.scroll_handle().is_some() {
            node.host.scroll_to(point(px(-x), px(-y)), window);
            return true;
        }
        false
    }

    /// The logical scroll anchor of a virtual list: row index, offset within the row and the
    /// viewport height. `None` for nodes that are not lists.
    pub fn list_anchor(&self, id: NodeId) -> Option<(usize, f32, f32)> {
        let list = self.nodes.get(&id)?.list.as_ref()?;
        let top = list.state.logical_scroll_top_and_extent().offset;
        let viewport = list.state.viewport_bounds().size.height;
        Some((
            top.item_ix,
            f32::from(top.offset_in_item),
            f32::from(viewport),
        ))
    }

    /// Scrolls a virtual list so that logical row `index` is at the top, `offset` pixels into
    /// the row (a negative offset places the row below the top). Returns whether `id` is a list.
    pub fn scroll_to_item(
        &self,
        id: NodeId,
        index: usize,
        offset: f32,
        window: &mut Window,
    ) -> bool {
        let Some(list) = self.nodes.get(&id).and_then(|node| node.list.as_ref()) else {
            return false;
        };
        list.state.scroll_to(ListOffset {
            item_ix: index,
            offset_in_item: px(offset),
        });
        window.refresh();
        true
    }

    /// Scrolls the nearest scrolling ancestor of `id` so that the branch containing the node is
    /// visible. Returns whether an ancestor scrolled.
    pub fn scroll_into_view(&self, tree: &HostTree, id: NodeId, window: &mut Window) -> bool {
        let mut child = id;
        let mut parent = tree.node(id).and_then(|node| node.parent());
        while let Some(current) = parent {
            let Some(node) = tree.node(current) else {
                return false;
            };
            let scene = self.nodes.get(&current);
            if let Some(list) = scene.and_then(|scene| scene.list.as_ref()) {
                if let Some(index) = node.children().position(|candidate| candidate == child) {
                    // Rows are indexed logically: the mounted window starts at `window_start`.
                    list.state
                        .scroll_to_reveal_item(list.state.window_start() + index);
                    window.refresh();
                    return true;
                }
            }
            if let Some(handle) = scene.and_then(|scene| scene.host.scroll_handle()) {
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
    pub fn set_image_pixels(
        &mut self,
        id: NodeId,
        width: f64,
        height: f64,
        bytes: &[u8],
        format: Option<&str>,
        window: &mut Window,
    ) -> Result<(), String> {
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
    pub fn set_image(
        &mut self,
        id: NodeId,
        bytes: &[u8],
        window: &mut Window,
        cx: &App,
    ) -> Result<(), String> {
        let image = self
            .nodes
            .get_mut(&id)
            .and_then(|node| node.image.as_mut())
            .ok_or_else(|| "the element is not an image".to_string())?;
        image
            .set_image(bytes, cx)
            .map_err(|error| error.to_string())?;
        window.refresh();
        Ok(())
    }
}

/// The scroll mode a style asks for through `overflow`, `overflowX` and `overflowY`.
fn scroll_mode(style: Option<&Value>) -> HostScroll {
    let Some(Value::Object(style)) = style else {
        return HostScroll::None;
    };
    let scrolls = |key: &str| {
        matches!(
            style.get(key).and_then(Value::as_str),
            Some("scroll" | "auto")
        )
    };
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
                let paths: Vec<String> = event
                    .paths
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                emit(
                    cx,
                    id,
                    "fileDrop",
                    json!({ "paths": paths, "x": event.x, "y": event.y }),
                );
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
                emit(
                    cx,
                    id,
                    "scrollChange",
                    json!({ "x": event.x + 0.0, "y": event.y + 0.0 }),
                );
            });
        }
        _ => {}
    }
}
