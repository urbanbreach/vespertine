//! Declarative host events and focus routing (INP-006).
//!
//! A *host node* is a stable node of a declarative host tree (the tree an
//! application describes through atomic mutations, see APP-006) that exposes
//! normalized pointer, keyboard, focus, geometry and scroll notifications to
//! application callbacks — typically callbacks into a script runtime.
//!
//! [`HostNode`] is the persistent, mutable half: it owns the node's listener
//! table, its focus handle, its scroll handle and the state kept for geometry
//! and scroll reports, and keeps all of them for the node's lifetime however
//! often the tree is rendered. [`HostNode::element`] is the per-render half:
//! it builds the element for the current configuration, registering only the
//! element listeners the node's subscriptions need (a node that did not
//! subscribe to hover does no hover bookkeeping at all).
//!
//! All payloads use window-local logical pixels. Mouse buttons encode as 0
//! (left), 1 (middle) and 2 (right); further buttons follow (3 back, 4
//! forward). Replacing or removing a listener takes effect for the next
//! event; the node keeps its identity ([`HostNode::id`], its focus handle).
//!
//! Key callbacks of one keystroke share one [`KeyDispatch`], which also
//! reaches the trailing window-level callback registered with
//! [`HostContext::set_window_key_down`] / [`HostContext::set_window_key_up`].
//! The [`HostContext::window_hooks`] element must be part of the rendered tree
//! for that guarantee (it mints the dispatch before any other callback runs).

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::Div;
use crate::{
    A11ySubtreeBuilder, AnyElement, App, Bounds, DispatchPhase, Element, ElementId, ExternalPaths,
    FocusHandle, FocusId, GlobalElementId, InspectorElementId, InteractiveElement, Interactivity,
    IntoElement, KeyDownEvent, KeyUpEvent, LayoutId, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, ParentElement, Pixels, Point, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, Size, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled, Subscription, TouchPhase, Window, canvas, div,
};

/// Geometry and scroll-offset differences below this many logical pixels are
/// not reported, on either compared axis.
pub const REPORT_EPSILON: f32 = 0.01;

/// Modifier flags carried by host payloads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostModifiers {
    /// The Shift key.
    pub shift: bool,
    /// The Control key.
    pub ctrl: bool,
    /// The Alt/Option key.
    pub alt: bool,
    /// The platform key (Command / Super / Windows).
    pub cmd: bool,
}

impl From<Modifiers> for HostModifiers {
    fn from(modifiers: Modifiers) -> Self {
        Self {
            shift: modifiers.shift,
            ctrl: modifiers.control,
            alt: modifiers.alt,
            cmd: modifiers.platform,
        }
    }
}

/// The button encoding of host payloads: 0 left, 1 middle, 2 right, 3 back,
/// 4 forward.
pub fn encode_button(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::Navigate(NavigationDirection::Back) => 3,
        MouseButton::Navigate(NavigationDirection::Forward) => 4,
    }
}

/// Payload of pointer callbacks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostPointerEvent {
    /// Window-local horizontal position in logical pixels.
    pub x: f32,
    /// Window-local vertical position in logical pixels.
    pub y: f32,
    /// How many times the button was clicked in quick succession (0 for
    /// events that are not button events).
    pub click_count: usize,
    /// Modifier flags.
    pub modifiers: HostModifiers,
    /// The button of a button event; for move events the button held down,
    /// if any; `None` for hover events.
    pub button: Option<u8>,
}

/// The phase of a raw scroll gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostTouchPhase {
    /// The gesture started.
    Started,
    /// The gesture moved.
    Moved,
    /// The gesture ended (or was cancelled).
    Ended,
}

impl From<TouchPhase> for HostTouchPhase {
    fn from(phase: TouchPhase) -> Self {
        match phase {
            TouchPhase::Started => Self::Started,
            TouchPhase::Moved => Self::Moved,
            TouchPhase::Ended | TouchPhase::Cancelled => Self::Ended,
        }
    }
}

/// Payload of the raw scroll callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostScrollEvent {
    /// Window-local horizontal pointer position.
    pub x: f32,
    /// Window-local vertical pointer position.
    pub y: f32,
    /// Horizontal delta: pixels when [`Self::precise`], wheel lines otherwise.
    pub delta_x: f32,
    /// Vertical delta: pixels when [`Self::precise`], wheel lines otherwise.
    pub delta_y: f32,
    /// `true` when the deltas are exact pixels, `false` for wheel lines.
    pub precise: bool,
    /// The touch phase of the gesture.
    pub phase: HostTouchPhase,
    /// Modifier flags.
    pub modifiers: HostModifiers,
}

/// Payload of the file-drop callback.
#[derive(Clone, Debug, PartialEq)]
pub struct HostFileDropEvent {
    /// Absolute filesystem paths of the dropped files.
    pub paths: Vec<PathBuf>,
    /// Window-local horizontal pointer position.
    pub x: f32,
    /// Window-local vertical pointer position.
    pub y: f32,
}

/// Payload of the layout callback: the node's border-box in window-local
/// logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostLayoutEvent {
    /// Horizontal origin.
    pub x: f32,
    /// Vertical origin.
    pub y: f32,
    /// Border-box width.
    pub width: f32,
    /// Border-box height.
    pub height: f32,
}

/// Payload of the scroll-change callback: the new scroll offset, zero at the
/// start and negative as content moves toward the origin side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostScrollChangeEvent {
    /// Horizontal offset.
    pub x: f32,
    /// Vertical offset.
    pub y: f32,
}

#[derive(Debug)]
struct KeyDispatchFlags {
    default_prevented: AtomicBool,
    propagation_stopped: AtomicBool,
}

/// The identity of one keystroke dispatch, shared by every host key callback
/// of the keystroke and by the trailing window-level callback.
///
/// Preventing the default action or stopping propagation through any clone
/// is visible through all others; the flags are atomic, so a script runtime
/// that answers later and from another thread can still set them, and the
/// host applies whatever is set once each callback returns.
#[derive(Clone, Debug)]
pub struct KeyDispatch {
    id: u64,
    flags: Arc<KeyDispatchFlags>,
}

impl PartialEq for KeyDispatch {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl KeyDispatch {
    fn new(id: u64) -> Self {
        Self {
            id,
            flags: Arc::new(KeyDispatchFlags {
                default_prevented: AtomicBool::new(false),
                propagation_stopped: AtomicBool::new(false),
            }),
        }
    }

    /// The dispatch identity; unique per keystroke event within a host
    /// context.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Prevents the default action of the keystroke (key bindings after the
    /// callbacks, text input, keyboard activation).
    pub fn prevent_default(&self) {
        self.flags.default_prevented.store(true, Ordering::SeqCst);
    }

    /// Stops the keystroke from reaching later callbacks.
    pub fn stop_propagation(&self) {
        self.flags.propagation_stopped.store(true, Ordering::SeqCst);
    }

    /// Whether the default action has been prevented.
    pub fn default_prevented(&self) -> bool {
        self.flags.default_prevented.load(Ordering::SeqCst)
    }

    /// Whether propagation has been stopped.
    pub fn propagation_stopped(&self) -> bool {
        self.flags.propagation_stopped.load(Ordering::SeqCst)
    }

    fn apply(&self, window: &mut Window, cx: &mut App) {
        if self.default_prevented() {
            window.prevent_default();
        }
        if self.propagation_stopped() {
            cx.stop_propagation();
        }
    }
}

/// Payload of key callbacks.
#[derive(Clone, Debug)]
pub struct HostKeyEvent {
    /// The key name (`"a"`, `"enter"`, `"f10"`, ...).
    pub key: String,
    /// The character the keystroke produces, when available.
    pub key_char: Option<String>,
    /// Modifier flags.
    pub modifiers: HostModifiers,
    /// Whether the key is auto-repeating.
    pub held: bool,
    /// The dispatch identity shared by all callbacks of this keystroke.
    pub dispatch: KeyDispatch,
}

type PointerListener = Rc<dyn Fn(&HostPointerEvent, &mut Window, &mut App)>;
type ScrollListener = Rc<dyn Fn(&HostScrollEvent, &mut Window, &mut App)>;
type KeyListener = Rc<dyn Fn(&HostKeyEvent, &mut Window, &mut App)>;
type FocusListener = Rc<dyn Fn(&mut Window, &mut App)>;
type FileDropListener = Rc<dyn Fn(&HostFileDropEvent, &mut Window, &mut App)>;
type LayoutListener = Rc<dyn Fn(&HostLayoutEvent, &mut Window, &mut App)>;
type ScrollChangeListener = Rc<dyn Fn(&HostScrollChangeEvent, &mut Window, &mut App)>;

/// The subscriptions a host node offers, for removal with
/// [`HostNode::remove_listener`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// `pointerDown`
    PointerDown,
    /// `pointerUp`
    PointerUp,
    /// `pointerMove`
    PointerMove,
    /// `enter`
    Enter,
    /// `leave`
    Leave,
    /// `click`
    Click,
    /// `auxClick`
    AuxClick,
    /// `contextMenu`
    ContextMenu,
    /// `outsideDown`
    OutsideDown,
    /// Raw scroll (wheel or touch) input.
    Scroll,
    /// `keyDown`
    KeyDown,
    /// `keyUp`
    KeyUp,
    /// `focus`
    Focus,
    /// `blur`
    Blur,
    /// `fileDrop`
    FileDrop,
    /// `layout`
    Layout,
    /// `scrollChange`
    ScrollChange,
}

#[derive(Default)]
struct Listeners {
    pointer_down: Option<PointerListener>,
    pointer_up: Option<PointerListener>,
    pointer_move: Option<PointerListener>,
    enter: Option<PointerListener>,
    leave: Option<PointerListener>,
    click: Option<PointerListener>,
    aux_click: Option<PointerListener>,
    context_menu: Option<PointerListener>,
    outside_down: Option<PointerListener>,
    scroll: Option<ScrollListener>,
    key_down: Option<KeyListener>,
    key_up: Option<KeyListener>,
    focus: Option<FocusListener>,
    blur: Option<FocusListener>,
    file_drop: Option<FileDropListener>,
    layout: Option<LayoutListener>,
    scroll_change: Option<ScrollChangeListener>,
}

/// Which axes a node scrolls on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostScroll {
    /// The node does not scroll. Its scroll handle is released.
    #[default]
    None,
    /// Both axes, with simultaneous diagonal movement.
    Both,
    /// Horizontal movement only; the vertical delta is ignored.
    Horizontal,
    /// Vertical movement only; the horizontal delta is ignored.
    Vertical,
}

// ---------------------------------------------------------------------------
// Context shared by all nodes of one host tree
// ---------------------------------------------------------------------------

struct Autofocus {
    node: FocusHandle,
    focus_generation: u64,
}

#[derive(Default)]
struct ContextInner {
    next_key_dispatch: Cell<u64>,
    current_key: RefCell<Option<KeyDispatch>>,
    window_key_down: RefCell<Option<KeyListener>>,
    window_key_up: RefCell<Option<KeyListener>>,
    autofocus: RefCell<Option<Autofocus>>,
    explicit_focus_seen: Cell<bool>,
    focus_evaluation_scheduled: Cell<bool>,
}

/// State shared by the nodes of one host tree: key dispatch identity, the
/// window-level key callbacks and focus request arbitration.
#[derive(Clone, Default)]
pub struct HostContext(Rc<ContextInner>);

impl HostContext {
    /// A new, empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets (or with `None` removes) the window-level key-down callback. It
    /// runs last for each key-down, with the dispatch identity shared with the
    /// node callbacks of the same keystroke.
    pub fn set_window_key_down(
        &self,
        listener: Option<impl Fn(&HostKeyEvent, &mut Window, &mut App) + 'static>,
    ) {
        *self.0.window_key_down.borrow_mut() = listener.map(|l| Rc::new(l) as KeyListener);
    }

    /// Sets (or with `None` removes) the window-level key-up callback.
    pub fn set_window_key_up(
        &self,
        listener: Option<impl Fn(&HostKeyEvent, &mut Window, &mut App) + 'static>,
    ) {
        *self.0.window_key_up.borrow_mut() = listener.map(|l| Rc::new(l) as KeyListener);
    }

    fn mint_key_dispatch(&self) -> KeyDispatch {
        let id = self.0.next_key_dispatch.get();
        self.0.next_key_dispatch.set(id + 1);
        let dispatch = KeyDispatch::new(id);
        *self.0.current_key.borrow_mut() = Some(dispatch.clone());
        dispatch
    }

    fn current_key_dispatch(&self) -> KeyDispatch {
        let current = self.0.current_key.borrow().clone();
        current.unwrap_or_else(|| self.mint_key_dispatch())
    }

    /// An element that registers the window-level key hooks: a window-root
    /// listener that mints each keystroke's [`KeyDispatch`] before any other
    /// callback runs and delivers the trailing window-level callback. Place it
    /// anywhere in the rendered host tree (it paints nothing).
    pub fn window_hooks(&self) -> impl IntoElement + use<> {
        let context = self.clone();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                context.register_root_listeners(window);
            },
        )
    }

    fn register_root_listeners(&self, window: &mut Window) {
        let context = self.clone();
        window.on_root_key_event(move |event: &KeyDownEvent, phase, window, cx| match phase {
            DispatchPhase::Capture => {
                context.mint_key_dispatch();
            }
            DispatchPhase::Bubble => {
                let listener = context.0.window_key_down.borrow().clone();
                if let Some(listener) = listener {
                    let dispatch = context.current_key_dispatch();
                    listener(&key_down_payload(event, dispatch.clone()), window, cx);
                    dispatch.apply(window, cx);
                }
            }
        });
        let context = self.clone();
        window.on_root_key_event(move |event: &KeyUpEvent, phase, window, cx| match phase {
            DispatchPhase::Capture => {
                context.mint_key_dispatch();
            }
            DispatchPhase::Bubble => {
                let listener = context.0.window_key_up.borrow().clone();
                if let Some(listener) = listener {
                    let dispatch = context.current_key_dispatch();
                    listener(&key_up_payload(event, dispatch.clone()), window, cx);
                    dispatch.apply(window, cx);
                }
            }
        });
    }

    fn note_explicit_focus(&self, window: &Window) {
        self.0.explicit_focus_seen.set(true);
        self.schedule_focus_evaluation(window);
    }

    fn schedule_focus_evaluation(&self, window: &Window) {
        if self.0.focus_evaluation_scheduled.replace(true) {
            return;
        }
        let context = self.clone();
        window.on_next_frame(move |window, cx| context.evaluate_autofocus(window, cx));
    }

    /// Applies the pending autofocus request, unless an explicit focus request
    /// was made meanwhile (it outranks autofocus) or focus moved since the
    /// request was registered (a user focus change is never overridden).
    fn evaluate_autofocus(&self, window: &mut Window, cx: &mut App) {
        self.0.focus_evaluation_scheduled.set(false);
        let explicit = self.0.explicit_focus_seen.replace(false);
        let Some(autofocus) = self.0.autofocus.borrow_mut().take() else {
            return;
        };
        if explicit || window.focus_generation != autofocus.focus_generation {
            return;
        }
        if window.is_focus_mounted(&autofocus.node) {
            window.focus(&autofocus.node, cx);
        }
    }
}

fn key_down_payload(event: &KeyDownEvent, dispatch: KeyDispatch) -> HostKeyEvent {
    HostKeyEvent {
        key: event.keystroke.key.clone(),
        key_char: event.keystroke.key_char.clone(),
        modifiers: event.keystroke.modifiers.into(),
        held: event.is_held,
        dispatch,
    }
}

fn key_up_payload(event: &KeyUpEvent, dispatch: KeyDispatch) -> HostKeyEvent {
    HostKeyEvent {
        key: event.keystroke.key.clone(),
        key_char: event.keystroke.key_char.clone(),
        modifiers: event.keystroke.modifiers.into(),
        held: false,
        dispatch,
    }
}

fn pointer_payload(
    position: Point<Pixels>,
    click_count: usize,
    modifiers: Modifiers,
    button: Option<MouseButton>,
) -> HostPointerEvent {
    HostPointerEvent {
        x: f32::from(position.x),
        y: f32::from(position.y),
        click_count,
        modifiers: modifiers.into(),
        button: button.map(encode_button),
    }
}

// ---------------------------------------------------------------------------
// Host node
// ---------------------------------------------------------------------------

const BUTTONS: [MouseButton; 5] = [
    MouseButton::Left,
    MouseButton::Middle,
    MouseButton::Right,
    MouseButton::Navigate(NavigationDirection::Back),
    MouseButton::Navigate(NavigationDirection::Forward),
];

struct NodeInner {
    listeners: Listeners,
    focus: FocusHandle,
    editor: bool,
    tab_index: Option<isize>,
    autofocus: bool,
    /// Autofocus was enabled and its request has not been registered yet.
    autofocus_pending: bool,
    scroll: HostScroll,
    scroll_handle: Option<ScrollHandle>,
    /// Border-box size last reported to the layout listener.
    reported_layout: Option<Size<Pixels>>,
    /// Scroll offset last reported to the scroll-change listener.
    reported_scroll: Option<Point<Pixels>>,
    /// The button pressed inside the node and not yet released.
    pressed: Option<u8>,
    /// The border box the node painted last, for deciding whether a captured release is a click.
    bounds: Option<Bounds<Pixels>>,
    bound: bool,
    subscriptions: Vec<Subscription>,
}

impl NodeInner {
    fn effective_tab_index(&self) -> Option<isize> {
        self.tab_index.or(self.editor.then_some(0))
    }

    fn sync_focus_handle(&self) {
        if let Some(index) = self.effective_tab_index() {
            // Negative indices make the node focusable programmatically but
            // exclude it from sequential (tab key) traversal.
            let _ = self.focus.clone().tab_index(index).tab_stop(index >= 0);
        }
    }
}

/// A stable node of the declarative host tree. See the [module
/// documentation](self).
#[derive(Clone)]
pub struct HostNode {
    id: ElementId,
    context: HostContext,
    inner: Rc<RefCell<NodeInner>>,
}

macro_rules! listener_setters {
    ($($(#[$doc:meta])* $set:ident, $field:ident: $ty:ty, $event:ty;)*) => {
        $(
            $(#[$doc])*
            pub fn $set(&self, listener: impl Fn(&$event, &mut Window, &mut App) + 'static) {
                self.inner.borrow_mut().listeners.$field = Some(Rc::new(listener) as $ty);
            }
        )*
    };
}

impl HostNode {
    /// Creates a node with the given stable element identity. The node's focus
    /// handle lives as long as the node.
    pub fn new(context: &HostContext, id: impl Into<ElementId>, cx: &mut App) -> Self {
        Self {
            id: id.into(),
            context: context.clone(),
            inner: Rc::new(RefCell::new(NodeInner {
                listeners: Listeners::default(),
                focus: cx.focus_handle(),
                editor: false,
                tab_index: None,
                autofocus: false,
                autofocus_pending: false,
                scroll: HostScroll::None,
                scroll_handle: None,
                reported_layout: None,
                reported_scroll: None,
                pressed: None,
                bounds: None,
                bound: false,
                subscriptions: Vec::new(),
            })),
        }
    }

    /// The node's stable element identity.
    pub fn id(&self) -> &ElementId {
        &self.id
    }

    /// The node's persistent focus handle.
    pub fn focus_handle(&self) -> FocusHandle {
        self.inner.borrow().focus.clone()
    }

    /// The node's focus identity.
    pub fn focus_id(&self) -> FocusId {
        self.inner.borrow().focus.id()
    }

    listener_setters! {
        /// Replaces the `pointerDown` listener.
        on_pointer_down, pointer_down: PointerListener, HostPointerEvent;
        /// Replaces the `pointerUp` listener.
        on_pointer_up, pointer_up: PointerListener, HostPointerEvent;
        /// Replaces the `pointerMove` listener.
        on_pointer_move, pointer_move: PointerListener, HostPointerEvent;
        /// Replaces the `enter` listener; subscribing starts hover bookkeeping.
        on_enter, enter: PointerListener, HostPointerEvent;
        /// Replaces the `leave` listener; subscribing starts hover bookkeeping.
        on_leave, leave: PointerListener, HostPointerEvent;
        /// Replaces the `click` listener (left button, emitted on release after
        /// the pointer-up callback).
        on_click, click: PointerListener, HostPointerEvent;
        /// Replaces the `auxClick` listener (middle and other non-left,
        /// non-right buttons).
        on_aux_click, aux_click: PointerListener, HostPointerEvent;
        /// Replaces the `contextMenu` listener (right button).
        on_context_menu, context_menu: PointerListener, HostPointerEvent;
        /// Replaces the `outsideDown` listener: a press outside the node's
        /// bounds.
        on_outside_down, outside_down: PointerListener, HostPointerEvent;
        /// Replaces the raw scroll listener.
        on_scroll, scroll: ScrollListener, HostScrollEvent;
        /// Replaces the `keyDown` listener.
        on_key_down, key_down: KeyListener, HostKeyEvent;
        /// Replaces the `keyUp` listener.
        on_key_up, key_up: KeyListener, HostKeyEvent;
        /// Replaces the `fileDrop` listener.
        on_file_drop, file_drop: FileDropListener, HostFileDropEvent;
        /// Replaces the `layout` listener. It fires after the node's first
        /// paint with the border-box, then whenever the size changes by
        /// [`REPORT_EPSILON`] or more on either axis (never for a change of
        /// position alone). Callbacks run after the frame has been painted.
        on_layout, layout: LayoutListener, HostLayoutEvent;
        /// Replaces the `scrollChange` listener. The first paint only sets the
        /// baseline; afterwards it fires when the actual scroll offset
        /// changes by [`REPORT_EPSILON`] or more, also for programmatic
        /// scrolling. Callbacks run after the frame has been painted.
        on_scroll_change, scroll_change: ScrollChangeListener, HostScrollChangeEvent;
    }

    /// Replaces the `focus` listener (the node itself received focus).
    pub fn on_focus(&self, listener: impl Fn(&mut Window, &mut App) + 'static) {
        self.inner.borrow_mut().listeners.focus = Some(Rc::new(listener));
    }

    /// Replaces the `blur` listener (the node itself lost focus).
    pub fn on_blur(&self, listener: impl Fn(&mut Window, &mut App) + 'static) {
        self.inner.borrow_mut().listeners.blur = Some(Rc::new(listener));
    }

    /// Removes a listener. Its notifications stop with the next event; removing
    /// the layout or scroll-change listener also releases the retained report
    /// state (last reported geometry or offset).
    pub fn remove_listener(&self, event: HostEvent) {
        let mut inner = self.inner.borrow_mut();
        let listeners = &mut inner.listeners;
        match event {
            HostEvent::PointerDown => listeners.pointer_down = None,
            HostEvent::PointerUp => listeners.pointer_up = None,
            HostEvent::PointerMove => listeners.pointer_move = None,
            HostEvent::Enter => listeners.enter = None,
            HostEvent::Leave => listeners.leave = None,
            HostEvent::Click => listeners.click = None,
            HostEvent::AuxClick => listeners.aux_click = None,
            HostEvent::ContextMenu => listeners.context_menu = None,
            HostEvent::OutsideDown => listeners.outside_down = None,
            HostEvent::Scroll => listeners.scroll = None,
            HostEvent::KeyDown => listeners.key_down = None,
            HostEvent::KeyUp => listeners.key_up = None,
            HostEvent::Focus => listeners.focus = None,
            HostEvent::Blur => listeners.blur = None,
            HostEvent::FileDrop => listeners.file_drop = None,
            HostEvent::Layout => {
                listeners.layout = None;
                inner.reported_layout = None;
            }
            HostEvent::ScrollChange => {
                listeners.scroll_change = None;
                inner.reported_scroll = None;
            }
        }
    }

    /// Whether the node currently retains geometry or scroll-offset report
    /// state: `(layout, scroll)`.
    pub fn has_report_state(&self) -> (bool, bool) {
        let inner = self.inner.borrow();
        (
            inner.reported_layout.is_some(),
            inner.reported_scroll.is_some(),
        )
    }

    /// Marks the node as an editor: focusable by default at tab index 0.
    pub fn set_editor(&self, editor: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.editor = editor;
        inner.sync_focus_handle();
    }

    /// Sets the tab index. `None` restores the default (0 for editors, not
    /// focusable otherwise); a negative index permits programmatic focus but
    /// excludes the node from sequential traversal.
    pub fn set_tab_index(&self, tab_index: Option<isize>) {
        let mut inner = self.inner.borrow_mut();
        inner.tab_index = tab_index;
        inner.sync_focus_handle();
    }

    /// Whether the node can take focus at all.
    pub fn is_focusable(&self) -> bool {
        self.inner.borrow().effective_tab_index().is_some()
    }

    /// Requests initial focus when the node is first rendered. An explicit
    /// [`Self::focus`] request outranks it, and it never overrides a focus
    /// change made by the user before it applies.
    pub fn set_autofocus(&self, autofocus: bool) {
        let mut inner = self.inner.borrow_mut();
        if autofocus && !inner.autofocus {
            inner.autofocus_pending = true;
        }
        inner.autofocus = autofocus;
    }

    /// Sets which axes scroll. [`HostScroll::None`] releases the scroll
    /// handle; any other mode keeps one handle (and its offset) across
    /// rerenders and mode changes between scrolling modes.
    pub fn set_scroll(&self, scroll: HostScroll) {
        let mut inner = self.inner.borrow_mut();
        inner.scroll = scroll;
        match scroll {
            HostScroll::None => {
                inner.scroll_handle = None;
                inner.reported_scroll = None;
            }
            _ => {
                inner.scroll_handle.get_or_insert_with(ScrollHandle::new);
            }
        }
    }

    /// The node's scroll handle, while scrolling is enabled.
    pub fn scroll_handle(&self) -> Option<ScrollHandle> {
        self.inner.borrow().scroll_handle.clone()
    }

    /// Scrolls the node programmatically to `offset` (zero at the start,
    /// negative toward the far side). A scroll-change notification follows
    /// after the next paint.
    pub fn scroll_to(&self, offset: Point<Pixels>, window: &mut Window) {
        if let Some(handle) = self.scroll_handle() {
            handle.set_offset(offset);
            window.refresh();
        }
    }

    // ---- ref-like focus control -------------------------------------------------

    /// Focuses the node (an explicit request; it outranks pending autofocus).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let focus = self.focus_handle();
        self.context.note_explicit_focus(window);
        window.focus(&focus, cx);
    }

    /// Removes focus when the node (or one of its descendants) holds it.
    pub fn blur(&self, window: &mut Window, cx: &mut App) {
        if self.focus_handle().contains_focused(window, cx) {
            window.blur(cx);
        }
    }

    /// Moves focus to the next tab stop in the window.
    pub fn focus_next(&self, window: &mut Window, cx: &mut App) {
        self.context.note_explicit_focus(window);
        window.focus_next(cx);
    }

    /// Moves focus to the previous tab stop in the window.
    pub fn focus_prev(&self, window: &mut Window, cx: &mut App) {
        self.context.note_explicit_focus(window);
        window.focus_prev(cx);
    }

    /// Moves focus to the next tab stop among `scope` (a subtree of nodes),
    /// wrapping inside it.
    pub fn focus_next_in(scope: &[HostNode], window: &mut Window, cx: &mut App) {
        let ids = scope.iter().map(HostNode::focus_id).collect::<Vec<_>>();
        window.focus_next_among(cx, |id, _| ids.contains(&id));
    }

    /// Moves focus to the previous tab stop among `scope`, wrapping inside it.
    pub fn focus_prev_in(scope: &[HostNode], window: &mut Window, cx: &mut App) {
        let ids = scope.iter().map(HostNode::focus_id).collect::<Vec<_>>();
        window.focus_prev_among(cx, |id, _| ids.contains(&id));
    }

    // ---- element ---------------------------------------------------------------

    /// Builds the element for the node's current configuration. Call it on
    /// every render of the host tree; it registers only the element listeners
    /// the node's subscriptions need.
    pub fn element(&self, window: &mut Window, cx: &mut App) -> HostElement {
        self.bind(window, cx);
        self.request_autofocus(window);
        let node = self.inner.clone();
        let inner = self.inner.borrow();
        let listeners = &inner.listeners;

        let mut element = div().id(self.id.clone());

        if inner.effective_tab_index().is_some() {
            inner.sync_focus_handle();
            element = element.track_focus(&inner.focus);
        }

        match (inner.scroll, inner.scroll_handle.as_ref()) {
            (HostScroll::Both, Some(handle)) => {
                element = element.overflow_scroll().track_scroll(handle);
                element.style().allow_concurrent_scroll = Some(true);
            }
            (HostScroll::Horizontal, Some(handle)) => {
                element = element
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .track_scroll(handle);
            }
            (HostScroll::Vertical, Some(handle)) => {
                element = element
                    .overflow_y_scroll()
                    .restrict_scroll_to_axis()
                    .track_scroll(handle);
            }
            _ => {}
        }

        // A node that tracks a drag (press plus move or release callbacks) keeps the pointer
        // from the press on: moves and the release reach it outside its bounds and the window
        // (INP-001 capture, APP-012 R4).
        if listeners.pointer_down.is_some()
            && (listeners.pointer_move.is_some() || listeners.pointer_up.is_some())
        {
            element = element.capture_pointer();
        }
        let button_events = listeners.pointer_down.is_some()
            || listeners.pointer_up.is_some()
            || listeners.click.is_some()
            || listeners.aux_click.is_some()
            || listeners.context_menu.is_some();
        if button_events {
            for button in BUTTONS {
                let down_node = node.clone();
                element =
                    element.on_mouse_down(button, move |event: &MouseDownEvent, window, cx| {
                        let listener = {
                            let mut inner = down_node.borrow_mut();
                            inner.pressed = Some(encode_button(event.button));
                            inner.listeners.pointer_down.clone()
                        };
                        if let Some(listener) = listener {
                            listener(
                                &pointer_payload(
                                    event.position,
                                    event.click_count,
                                    event.modifiers,
                                    Some(event.button),
                                ),
                                window,
                                cx,
                            );
                        }
                    });
                let up_node = node.clone();
                element = element.on_mouse_up(button, move |event: &MouseUpEvent, window, cx| {
                    let payload = pointer_payload(
                        event.position,
                        event.click_count,
                        event.modifiers,
                        Some(event.button),
                    );
                    let (up, pressed) = {
                        let mut inner = up_node.borrow_mut();
                        let pressed = inner.pressed.take() == Some(encode_button(event.button));
                        (inner.listeners.pointer_up.clone(), pressed)
                    };
                    if let Some(up) = up {
                        up(&payload, window, cx);
                    }
                    // A release outside the node (reachable while the pointer is captured) is
                    // not a click.
                    let inside = up_node
                        .borrow()
                        .bounds
                        .is_none_or(|bounds| bounds.contains(&event.position));
                    if !pressed || !inside {
                        return;
                    }
                    // The click follows the pointer-up callback.
                    let click = {
                        let inner = up_node.borrow();
                        match event.button {
                            MouseButton::Left => inner.listeners.click.clone(),
                            MouseButton::Right => inner.listeners.context_menu.clone(),
                            _ => inner.listeners.aux_click.clone(),
                        }
                    };
                    if let Some(click) = click {
                        click(&payload, window, cx);
                    }
                });
                let released_outside = node.clone();
                element = element.on_mouse_up_out(button, move |_: &MouseUpEvent, _, _| {
                    released_outside.borrow_mut().pressed = None;
                });
            }
        }

        // Assistive technology activates the node through the accessible click action: a
        // primary click at the node's center, with no pointer press or release before it.
        if listeners.click.is_some() {
            let click_node = node.clone();
            element = element.on_a11y_action(accesskit::Action::Click, move |_, window, cx| {
                let (click, bounds) = {
                    let inner = click_node.borrow();
                    (inner.listeners.click.clone(), inner.bounds)
                };
                if let Some(click) = click {
                    let position = bounds.map_or_else(|| window.mouse_position(), |b| b.center());
                    let payload =
                        pointer_payload(position, 1, Modifiers::default(), Some(MouseButton::Left));
                    click(&payload, window, cx);
                }
            });
        }

        if listeners.pointer_move.is_some() {
            let move_node = node.clone();
            element = element.on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                let listener = move_node.borrow().listeners.pointer_move.clone();
                if let Some(listener) = listener {
                    listener(
                        &pointer_payload(event.position, 0, event.modifiers, event.pressed_button),
                        window,
                        cx,
                    );
                }
            });
        }

        // Hover bookkeeping exists only for nodes that subscribed to it.
        if listeners.enter.is_some() || listeners.leave.is_some() {
            let hover_node = node.clone();
            element = element.on_hover(move |hovered: &bool, window, cx| {
                let listener = {
                    let inner = hover_node.borrow();
                    if *hovered {
                        inner.listeners.enter.clone()
                    } else {
                        inner.listeners.leave.clone()
                    }
                };
                if let Some(listener) = listener {
                    listener(
                        &pointer_payload(window.mouse_position(), 0, window.modifiers(), None),
                        window,
                        cx,
                    );
                }
            });
        }

        if listeners.outside_down.is_some() {
            let outside_node = node.clone();
            element = element.on_mouse_down_out(move |event: &MouseDownEvent, window, cx| {
                let listener = outside_node.borrow().listeners.outside_down.clone();
                if let Some(listener) = listener {
                    listener(
                        &pointer_payload(
                            event.position,
                            event.click_count,
                            event.modifiers,
                            Some(event.button),
                        ),
                        window,
                        cx,
                    );
                }
            });
        }

        if listeners.scroll.is_some() {
            let scroll_node = node.clone();
            element = element.on_scroll_wheel(move |event: &ScrollWheelEvent, window, cx| {
                let listener = scroll_node.borrow().listeners.scroll.clone();
                if let Some(listener) = listener {
                    let (delta_x, delta_y, precise) = match event.delta {
                        ScrollDelta::Pixels(delta) => {
                            (f32::from(delta.x), f32::from(delta.y), true)
                        }
                        ScrollDelta::Lines(delta) => (delta.x, delta.y, false),
                    };
                    listener(
                        &HostScrollEvent {
                            x: f32::from(event.position.x),
                            y: f32::from(event.position.y),
                            delta_x,
                            delta_y,
                            precise,
                            phase: event.touch_phase.into(),
                            modifiers: event.modifiers.into(),
                        },
                        window,
                        cx,
                    );
                }
            });
        }

        if listeners.key_down.is_some() {
            let key_node = node.clone();
            let context = self.context.clone();
            element = element.on_key_down(move |event: &KeyDownEvent, window, cx| {
                let listener = key_node.borrow().listeners.key_down.clone();
                if let Some(listener) = listener {
                    let dispatch = context.current_key_dispatch();
                    listener(&key_down_payload(event, dispatch.clone()), window, cx);
                    dispatch.apply(window, cx);
                }
            });
        }

        if listeners.key_up.is_some() {
            let key_node = node.clone();
            let context = self.context.clone();
            element = element.on_key_up(move |event: &KeyUpEvent, window, cx| {
                let listener = key_node.borrow().listeners.key_up.clone();
                if let Some(listener) = listener {
                    let dispatch = context.current_key_dispatch();
                    listener(&key_up_payload(event, dispatch.clone()), window, cx);
                    dispatch.apply(window, cx);
                }
            });
        }

        if listeners.file_drop.is_some() {
            let drop_node = node;
            element = element.on_drop(move |paths: &ExternalPaths, window, cx| {
                let listener = drop_node.borrow().listeners.file_drop.clone();
                if let Some(listener) = listener {
                    let position = window.mouse_position();
                    let paths = paths
                        .paths()
                        .iter()
                        .map(|path| std::path::absolute(path).unwrap_or_else(|_| path.clone()))
                        .collect();
                    listener(
                        &HostFileDropEvent {
                            paths,
                            x: f32::from(position.x),
                            y: f32::from(position.y),
                        },
                        window,
                        cx,
                    );
                }
            });
        }

        HostElement {
            element,
            node: self.inner.clone(),
        }
    }

    /// First-render work: the persistent focus/blur listeners.
    fn bind(&self, window: &mut Window, cx: &mut App) {
        let focus = {
            let mut inner = self.inner.borrow_mut();
            if inner.bound {
                return;
            }
            inner.bound = true;
            inner.sync_focus_handle();
            inner.focus.clone()
        };

        let node = Rc::downgrade(&self.inner);
        let focus_id = focus.id();
        let (subscription, activate) =
            window.new_focus_listener(Box::new(move |event, window, cx| {
                let Some(node) = node.upgrade() else {
                    return false;
                };
                let was_focused = event.previous_focus_path.last() == Some(&focus_id);
                let is_focused = event.current_focus_path.last() == Some(&focus_id);
                if was_focused != is_focused {
                    let listener = {
                        let inner = node.borrow();
                        if is_focused {
                            inner.listeners.focus.clone()
                        } else {
                            inner.listeners.blur.clone()
                        }
                    };
                    if let Some(listener) = listener {
                        listener(window, cx);
                    }
                }
                true
            }));
        cx.defer(move |_| activate());
        self.inner.borrow_mut().subscriptions.push(subscription);
    }

    /// Registers a pending autofocus request, once per enabling of
    /// autofocus, for the next frame to evaluate.
    fn request_autofocus(&self, window: &mut Window) {
        let focus = {
            let mut inner = self.inner.borrow_mut();
            if !std::mem::take(&mut inner.autofocus_pending) {
                return;
            }
            inner.focus.clone()
        };
        *self.context.0.autofocus.borrow_mut() = Some(Autofocus {
            node: focus,
            focus_generation: window.focus_generation,
        });
        self.context.schedule_focus_evaluation(window);
    }
}

// ---------------------------------------------------------------------------
// Element wrapper: geometry and scroll reports
// ---------------------------------------------------------------------------

/// The element of a [`HostNode`] for one render. It delegates to a stateful
/// [`Div`] (so it takes the usual style, interactivity and child builders)
/// and, after the node has painted, schedules the node's layout and
/// scroll-change reports.
pub struct HostElement {
    element: Stateful<Div>,
    node: Rc<RefCell<NodeInner>>,
}

impl Styled for HostElement {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl InteractiveElement for HostElement {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl StatefulInteractiveElement for HostElement {}

impl ParentElement for HostElement {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements);
    }
}

impl IntoElement for HostElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn differs(a: Pixels, b: Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() >= REPORT_EPSILON
}

impl HostElement {
    /// Compares the painted geometry and scroll offset against the retained
    /// report state and defers the notifications that are due until the frame
    /// has been painted.
    fn report(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let mut inner = self.node.borrow_mut();
        inner.bounds = Some(bounds);

        if inner.listeners.layout.is_some() {
            let size = bounds.size;
            let changed = match inner.reported_layout {
                None => true,
                Some(last) => differs(size.width, last.width) || differs(size.height, last.height),
            };
            if changed {
                inner.reported_layout = Some(size);
                let event = HostLayoutEvent {
                    x: f32::from(bounds.origin.x),
                    y: f32::from(bounds.origin.y),
                    width: f32::from(size.width),
                    height: f32::from(size.height),
                };
                let node = self.node.clone();
                window.defer(cx, move |window, cx| {
                    let listener = node.borrow().listeners.layout.clone();
                    if let Some(listener) = listener {
                        listener(&event, window, cx);
                    }
                });
            }
        } else {
            inner.reported_layout = None;
        }

        if inner.listeners.scroll_change.is_some()
            && let Some(handle) = inner.scroll_handle.clone()
        {
            let offset = handle.offset();
            match inner.reported_scroll {
                // The first paint only sets the baseline.
                None => inner.reported_scroll = Some(offset),
                Some(last) => {
                    if differs(offset.x, last.x) || differs(offset.y, last.y) {
                        inner.reported_scroll = Some(offset);
                        let event = HostScrollChangeEvent {
                            x: f32::from(offset.x),
                            y: f32::from(offset.y),
                        };
                        let node = self.node.clone();
                        window.defer(cx, move |window, cx| {
                            let listener = node.borrow().listeners.scroll_change.clone();
                            if let Some(listener) = listener {
                                listener(&event, window, cx);
                            }
                        });
                    }
                }
            }
        } else {
            inner.reported_scroll = None;
        }
    }
}

impl Element for HostElement {
    type RequestLayoutState = <Stateful<Div> as Element>::RequestLayoutState;
    type PrepaintState = <Stateful<Div> as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Element::id(&self.element)
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        self.element.source_location()
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.element.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.element.write_a11y_info(node);
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        Element::a11y_synthetic_children(&mut self.element, prepaint, builder);
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
        self.report(bounds, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Entity, Modifiers, TouchPhase};
    use crate::{
        FileDropEvent, KeyUpEvent, Keystroke, MouseButton, Render, ScrollWheelEvent,
        TestAppContext, VisualTestContext, div, point, px,
    };
    use std::cell::RefCell;

    type Log = Rc<RefCell<Vec<String>>>;

    #[derive(Clone, Copy)]
    struct Layout {
        width: f32,
        height: f32,
        margin_left: f32,
    }

    impl Default for Layout {
        fn default() -> Self {
            Self {
                width: 100.,
                height: 50.,
                margin_left: 0.,
            }
        }
    }

    struct HostView {
        context: HostContext,
        nodes: Vec<(HostNode, Layout)>,
        content: Option<(f32, f32)>,
    }

    impl Render for HostView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut root = div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.context.window_hooks());
            for (node, layout) in &self.nodes {
                let mut element = node
                    .element(window, cx)
                    .w(px(layout.width))
                    .h(px(layout.height))
                    .ml(px(layout.margin_left));
                if let Some((width, height)) = self.content {
                    element = element.child(div().w(px(width)).h(px(height)).flex_none());
                }
                root = root.child(element);
            }
            root
        }
    }

    fn host_view(
        cx: &mut TestAppContext,
        node_count: usize,
    ) -> (Entity<HostView>, &mut VisualTestContext, Vec<HostNode>) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            let context = HostContext::new();
            let nodes = (0..node_count)
                .map(|ix| {
                    (
                        HostNode::new(&context, ("host-node", ix), cx),
                        Layout::default(),
                    )
                })
                .collect();
            HostView {
                context,
                nodes,
                content: None,
            }
        });
        let nodes = view.read_with(cx, |view, _| {
            view.nodes.iter().map(|(node, _)| node.clone()).collect()
        });
        (view, cx, nodes)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, _| window.activate_window());
        // Let the platform's activation reach the window before painting.
        cx.run_until_parked();
        cx.update(|window, cx| {
            // Rebuild every view: node configuration changes take effect at
            // the next render of the host tree.
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    fn rerender(view: &Entity<HostView>, cx: &mut VisualTestContext) {
        view.update(cx, |_, cx| cx.notify());
        draw(cx);
    }

    fn logger() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    fn take(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    /// The center of node `ix` in a column of default-layout nodes.
    fn center(ix: usize) -> Point<Pixels> {
        point(px(50.), px(25. + 50. * ix as f32))
    }

    fn click_with(cx: &mut VisualTestContext, position: Point<Pixels>, button: MouseButton) {
        cx.simulate_mouse_move(position, None, Modifiers::none());
        cx.simulate_mouse_down(position, button, Modifiers::none());
        cx.simulate_mouse_up(position, button, Modifiers::none());
    }

    // ----- INP-006 A1: pointer events ------------------------------------------

    #[crate::test]
    fn pointer_buttons_map_to_click_aux_click_and_context_menu(cx: &mut TestAppContext) {
        let (_, cx, nodes) = host_view(cx, 1);
        let node = &nodes[0];
        let log = logger();
        let record = |name: &'static str| {
            let log = log.clone();
            move |event: &HostPointerEvent, _: &mut Window, _: &mut App| {
                log.borrow_mut().push(format!(
                    "{name}:{:?}:{}:{}:{}",
                    event.button, event.click_count, event.x, event.y
                ));
            }
        };
        node.on_pointer_down(record("down"));
        node.on_pointer_up(record("up"));
        node.on_click(record("click"));
        node.on_aux_click(record("aux"));
        node.on_context_menu(record("context"));
        draw(cx);

        click_with(cx, point(px(40.), px(20.)), MouseButton::Left);
        assert_eq!(
            take(&log),
            [
                "down:Some(0):1:40:20",
                "up:Some(0):1:40:20",
                "click:Some(0):1:40:20"
            ],
            "primary click follows the up callback"
        );
        click_with(cx, point(px(40.), px(20.)), MouseButton::Middle);
        assert_eq!(
            take(&log),
            [
                "down:Some(1):1:40:20",
                "up:Some(1):1:40:20",
                "aux:Some(1):1:40:20"
            ]
        );
        click_with(cx, point(px(40.), px(20.)), MouseButton::Right);
        assert_eq!(
            take(&log),
            [
                "down:Some(2):1:40:20",
                "up:Some(2):1:40:20",
                "context:Some(2):1:40:20"
            ]
        );

        // A release without a press inside the node is not a click.
        cx.simulate_mouse_move(point(px(300.), px(300.)), None, Modifiers::none());
        cx.simulate_mouse_down(
            point(px(300.), px(300.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_move(
            point(px(40.), px(20.)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        cx.simulate_mouse_up(
            point(px(40.), px(20.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(take(&log), ["up:Some(0):1:40:20"]);
    }

    #[crate::test]
    fn an_accessible_click_reaches_the_click_subscription(cx: &mut TestAppContext) {
        let (_, cx, nodes) = host_view(cx, 2);
        let log = logger();
        let record = |name: &'static str| {
            let log = log.clone();
            move |event: &HostPointerEvent, _: &mut Window, _: &mut App| {
                log.borrow_mut().push(format!(
                    "{name}:{:?}:{}:{}:{}",
                    event.button, event.click_count, event.x, event.y
                ));
            }
        };
        nodes[0].on_pointer_up(record("up"));
        // The second node subscribes to the pointer only: it has no click action.
        nodes[1].on_pointer_up(record("other-up"));
        cx.update(|window, _| window.a11y.set_active_for_test(true));
        draw(cx);
        nodes[0].on_click(record("click"));
        draw(cx);

        let click_targets = cx.update(|window, _| {
            window
                .rendered_frame
                .a11y_listeners
                .iter()
                .flatten()
                .filter(|listener| listener.action == accesskit::Action::Click)
                .map(|listener| listener.node_id)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            click_targets.len(),
            1,
            "only the node subscribed to click exposes the click action"
        );
        cx.update(|window, cx| {
            window.handle_a11y_action(
                accesskit::ActionRequest {
                    action: accesskit::Action::Click,
                    target_tree: accesskit::TreeId::ROOT,
                    target_node: click_targets[0],
                    data: None,
                },
                cx,
            );
        });
        assert_eq!(
            take(&log),
            ["click:Some(0):1:50:25"],
            "a primary click at the node's center, without a pointer release"
        );
    }

    #[crate::test]
    fn a_dragged_node_keeps_the_pointer_outside_its_bounds_and_a_far_release_is_no_click(
        cx: &mut TestAppContext,
    ) {
        let (_, cx, nodes) = host_view(cx, 1);
        let node = &nodes[0];
        let log = logger();
        let record = |name: &'static str| {
            let log = log.clone();
            move |event: &HostPointerEvent, _: &mut Window, _: &mut App| {
                log.borrow_mut()
                    .push(format!("{name}:{}:{}", event.x, event.y));
            }
        };
        node.on_pointer_down(record("down"));
        node.on_pointer_move(record("move"));
        node.on_pointer_up(record("up"));
        node.on_click(record("click"));
        draw(cx);

        let inside = point(px(40.), px(20.));
        let far = point(px(900.), px(700.));
        cx.simulate_mouse_move(inside, None, Modifiers::none());
        cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::none());
        take(&log);
        cx.simulate_mouse_move(far, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(far, MouseButton::Left, Modifiers::none());
        assert_eq!(
            take(&log),
            ["move:900:700", "up:900:700"],
            "the captured node gets the moves and the release outside, and no click"
        );

        // The capture ended with the release: the pointer is the window's again.
        cx.simulate_mouse_move(far, None, Modifiers::none());
        cx.simulate_mouse_move(point(px(300.), px(300.)), None, Modifiers::none());
        assert_eq!(take(&log), Vec::<String>::new());

        // A press and release inside is a click, as before (the press first moves the pointer in).
        click_with(cx, inside, MouseButton::Left);
        assert_eq!(
            take(&log),
            ["move:40:20", "down:40:20", "up:40:20", "click:40:20"]
        );
    }

    #[crate::test]
    fn listener_replacement_and_removal_take_effect_for_the_next_event(cx: &mut TestAppContext) {
        let (view, cx, nodes) = host_view(cx, 1);
        let node = &nodes[0];
        let log = logger();
        let identity = (node.id().clone(), node.focus_id());
        node.on_pointer_down({
            let log = log.clone();
            move |_, _, _| log.borrow_mut().push("old".into())
        });
        draw(cx);
        click_with(cx, center(0), MouseButton::Left);
        assert_eq!(take(&log), ["old"]);

        // Replacement: no rerender needed, the node keeps its identity.
        node.on_pointer_down({
            let log = log.clone();
            move |_, _, _| log.borrow_mut().push("new".into())
        });
        click_with(cx, center(0), MouseButton::Left);
        assert_eq!(take(&log), ["new"]);
        assert_eq!(identity, (node.id().clone(), node.focus_id()));

        // Removal stops notifications immediately, even before a rerender.
        node.remove_listener(HostEvent::PointerDown);
        click_with(cx, center(0), MouseButton::Left);
        assert!(take(&log).is_empty());
        rerender(&view, cx);
        click_with(cx, center(0), MouseButton::Left);
        assert!(take(&log).is_empty());
    }

    #[crate::test]
    fn hover_enter_and_leave_exist_only_for_subscribers(cx: &mut TestAppContext) {
        let (_, cx, nodes) = host_view(cx, 2);
        let log = logger();
        let subscribed = &nodes[0];
        let silent = &nodes[1];
        subscribed.on_enter({
            let log = log.clone();
            move |event, _, _| {
                log.borrow_mut()
                    .push(format!("enter:{}:{}", event.x, event.y))
            }
        });
        subscribed.on_leave({
            let log = log.clone();
            move |_, _, _| log.borrow_mut().push("leave".into())
        });
        silent.on_pointer_down(|_, _, _| {});

        // No hover bookkeeping unless a node subscribed to enter or leave.
        cx.update(|window, cx| {
            assert!(
                subscribed
                    .element(window, cx)
                    .interactivity()
                    .hover_listener
                    .is_some()
            );
            assert!(
                silent
                    .element(window, cx)
                    .interactivity()
                    .hover_listener
                    .is_none()
            );
        });
        draw(cx);

        cx.simulate_hover_status_change(true);
        // The pointer starts at the window origin, inside the first node.
        cx.simulate_mouse_move(point(px(500.), px(400.)), None, Modifiers::none());
        draw(cx);
        take(&log);
        cx.simulate_mouse_move(center(0), None, Modifiers::none());
        draw(cx);
        cx.simulate_mouse_move(center(1), None, Modifiers::none());
        draw(cx);
        assert_eq!(take(&log), ["enter:50:25", "leave"]);
        // Moving across the unsubscribed node produced nothing further.
        cx.simulate_mouse_move(point(px(500.), px(400.)), None, Modifiers::none());
        draw(cx);
        assert!(take(&log).is_empty());
    }

    #[crate::test]
    fn outside_down_fires_for_presses_outside_the_node(cx: &mut TestAppContext) {
        let (_, cx, nodes) = host_view(cx, 1);
        let log = logger();
        nodes[0].on_outside_down({
            let log = log.clone();
            move |event, _, _| {
                log.borrow_mut().push(format!(
                    "outside:{:?}:{}:{}",
                    event.button, event.x, event.y
                ))
            }
        });
        draw(cx);
        cx.simulate_mouse_down(center(0), MouseButton::Left, Modifiers::none());
        assert!(take(&log).is_empty(), "inside the bounds");
        cx.simulate_mouse_up(center(0), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_down(
            point(px(400.), px(300.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(take(&log), ["outside:Some(0):400:300"]);
    }

    #[crate::test]
    fn file_drops_carry_absolute_paths_and_coordinates(cx: &mut TestAppContext) {
        let (_, cx, nodes) = host_view(cx, 1);
        let log = logger();
        nodes[0].on_file_drop({
            let log = log.clone();
            move |event, _, _| {
                log.borrow_mut()
                    .push(format!("{:?}@{}:{}", event.paths, event.x, event.y))
            }
        });
        draw(cx);
        let paths = || {
            ExternalPaths(
                [PathBuf::from("/tmp/a.txt"), PathBuf::from("relative/b.txt")]
                    .into_iter()
                    .collect(),
            )
        };
        let position = point(px(30.), px(20.));
        cx.simulate_event(FileDropEvent::Entered {
            position,
            paths: paths(),
        });
        cx.simulate_event(FileDropEvent::Submit { position });
        let events = take(&log);
        assert_eq!(events.len(), 1, "{events:?}");
        let absolute_second = std::path::absolute("relative/b.txt").expect("absolute");
        assert_eq!(
            events[0],
            format!("{:?}@30:20", [PathBuf::from("/tmp/a.txt"), absolute_second])
        );
    }

    #[crate::test]
    fn key_callbacks_share_one_dispatch_identity_with_the_window_callback(cx: &mut TestAppContext) {
        let (view, cx, nodes) = host_view(cx, 1);
        let node = &nodes[0];
        let context = view.read_with(cx, |view, _| view.context.clone());
        let log = logger();
        let ids: Rc<RefCell<Vec<(&'static str, u64)>>> = Default::default();
        node.set_tab_index(Some(0));
        node.on_key_down({
            let (log, ids) = (log.clone(), ids.clone());
            move |event, _, _| {
                ids.borrow_mut().push(("node", event.dispatch.id()));
                log.borrow_mut().push(format!(
                    "node:{}:{:?}:{}:{}",
                    event.key, event.key_char, event.modifiers.shift, event.held
                ));
                event.dispatch.prevent_default();
            }
        });
        context.set_window_key_down(Some({
            let (log, ids) = (log.clone(), ids.clone());
            move |event: &HostKeyEvent, _: &mut Window, _: &mut App| {
                ids.borrow_mut().push(("window", event.dispatch.id()));
                // Preventing the default through the node callback is visible
                // here.
                log.borrow_mut().push(format!(
                    "window:prevented={}",
                    event.dispatch.default_prevented()
                ));
            }
        }));
        node.on_key_up({
            let ids = ids.clone();
            move |event, _, _| ids.borrow_mut().push(("node-up", event.dispatch.id()))
        });
        context.set_window_key_up(Some({
            let ids = ids.clone();
            move |event: &HostKeyEvent, _: &mut Window, _: &mut App| {
                ids.borrow_mut().push(("window-up", event.dispatch.id()))
            }
        }));
        draw(cx);
        let focus = node.focus_handle();
        cx.update(|window, cx| window.focus(&focus, cx));
        draw(cx);

        cx.simulate_keystrokes("shift-a");
        assert_eq!(
            take(&log),
            ["node:a:Some(\"A\"):true:false", "window:prevented=true"]
        );
        let first = ids.borrow().clone();
        assert_eq!(first.len(), 2);
        assert_eq!(
            first[0].1, first[1].1,
            "one dispatch identity per keystroke"
        );

        // The next keystroke has a new identity; key-up callbacks share theirs.
        ids.borrow_mut().clear();
        cx.simulate_keystrokes("shift-a");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("a").expect("keystroke"),
        });
        let second = ids.borrow().clone();
        assert_eq!(second.len(), 4);
        assert_ne!(second[0].1, first[0].1);
        assert_eq!(second[0].1, second[1].1);
        assert_eq!(second[2].1, second[3].1);

        // Stopping propagation through the node applies to the window-level
        // callback of the same keystroke, even when set from another thread.
        take(&log);
        node.on_key_down({
            let log = log.clone();
            move |event, _, _| {
                let dispatch = event.dispatch.clone();
                std::thread::spawn(move || dispatch.stop_propagation())
                    .join()
                    .expect("thread");
                log.borrow_mut().push("node-stopped".into());
            }
        });
        cx.simulate_keystrokes("b");
        assert_eq!(take(&log), ["node-stopped"]);
    }

    // ----- INP-006 A2: layout notifications -------------------------------------

    fn set_layout(view: &Entity<HostView>, cx: &mut VisualTestContext, layout: Layout) {
        view.update(cx, |view, cx| {
            view.nodes[0].1 = layout;
            cx.notify();
        });
        draw(cx);
    }

    #[crate::test]
    fn layout_events_follow_border_box_size_changes_only(cx: &mut TestAppContext) {
        let (view, cx, nodes) = host_view(cx, 1);
        let node = &nodes[0];
        let events: Rc<RefCell<Vec<HostLayoutEvent>>> = Default::default();
        node.on_layout({
            let events = events.clone();
            move |event, _, _| events.borrow_mut().push(*event)
        });
        assert_eq!(node.has_report_state(), (false, false));

        // Exactly one event at mount, supplying x, y, width and height; it is
        // delivered after painting, not from inside the paint.
        cx.update(|window, cx| {
            window.activate_window();
            // The node was already drawn without the listener; an unchanged view replays its
            // retained output (PRF-001), so the frame is refreshed to paint the node again.
            window.refresh();
            let _arena = window.draw(cx);
            assert!(events.borrow().is_empty(), "never invoked inside paint");
            _arena.clear(cx);
        });
        assert_eq!(
            *events.borrow(),
            [HostLayoutEvent {
                x: 0.,
                y: 0.,
                width: 100.,
                height: 50.
            }]
        );
        assert_eq!(node.has_report_state().0, true);
        events.borrow_mut().clear();

        // Position alone does not emit.
        set_layout(
            &view,
            cx,
            Layout {
                margin_left: 30.,
                ..Layout::default()
            },
        );
        assert!(events.borrow().is_empty());
        rerender(&view, cx);
        assert!(events.borrow().is_empty());

        // A size change emits once, with the current position.
        set_layout(
            &view,
            cx,
            Layout {
                width: 140.,
                margin_left: 30.,
                ..Layout::default()
            },
        );
        assert_eq!(
            *events.borrow(),
            [HostLayoutEvent {
                x: 30.,
                y: 0.,
                width: 140.,
                height: 50.
            }]
        );
        events.borrow_mut().clear();

        // Differences below 0.01 px on both axes are suppressed, and keep
        // being compared against the last reported size.
        set_layout(
            &view,
            cx,
            Layout {
                width: 140.005,
                height: 50.004,
                margin_left: 30.,
            },
        );
        assert!(events.borrow().is_empty());
        set_layout(
            &view,
            cx,
            Layout {
                width: 140.5,
                margin_left: 30.,
                ..Layout::default()
            },
        );
        assert_eq!(events.borrow().len(), 1);
        events.borrow_mut().clear();
        set_layout(
            &view,
            cx,
            Layout {
                width: 140.5,
                height: 60.,
                margin_left: 30.,
            },
        );
        assert_eq!(events.borrow().len(), 1, "a height change alone emits");
        events.borrow_mut().clear();

        // After the listener is removed nothing is emitted and the retained
        // report state is gone.
        node.remove_listener(HostEvent::Layout);
        assert_eq!(node.has_report_state(), (false, false));
        set_layout(
            &view,
            cx,
            Layout {
                width: 200.,
                ..Layout::default()
            },
        );
        assert!(events.borrow().is_empty());
        assert_eq!(node.has_report_state(), (false, false));
    }

    // ----- INP-006 A3 / A5: scrolling -----------------------------------------

    fn wheel(cx: &mut VisualTestContext, position: Point<Pixels>, dx: f32, dy: f32) {
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        draw(cx);
    }

    fn scrolling_view(
        cx: &mut TestAppContext,
        scroll: HostScroll,
    ) -> (Entity<HostView>, &mut VisualTestContext, HostNode) {
        let (view, cx, nodes) = host_view(cx, 1);
        view.update(cx, |view, _| {
            view.nodes[0].1 = Layout {
                width: 200.,
                height: 200.,
                margin_left: 0.,
            };
            view.content = Some((1000., 1000.));
        });
        nodes[0].set_scroll(scroll);
        draw(cx);
        (view, cx, nodes[0].clone())
    }

    #[crate::test]
    fn scroll_change_reports_offset_changes_not_wheel_intent(cx: &mut TestAppContext) {
        let (_, cx, node) = scrolling_view(cx, HostScroll::Both);
        let raw: Rc<RefCell<Vec<HostScrollEvent>>> = Default::default();
        let changes: Rc<RefCell<Vec<HostScrollChangeEvent>>> = Default::default();
        node.on_scroll({
            let raw = raw.clone();
            move |event, _, _| raw.borrow_mut().push(*event)
        });
        node.on_scroll_change({
            let changes = changes.clone();
            move |event, _, _| changes.borrow_mut().push(*event)
        });
        // The element listeners are registered at render time.
        draw(cx);
        cx.update(|window, _| window.refresh());
        draw(cx);
        // The first paint is the baseline: nothing was emitted.
        assert!(changes.borrow().is_empty());
        assert_eq!(node.has_report_state().1, true);

        // A wheel event moves the content and reports the new offset; the raw
        // event is reported separately with its precise flag and phase.
        wheel(cx, point(px(100.), px(100.)), 0., -50.);
        assert_eq!(changes.borrow().len(), 1);
        assert_eq!(
            changes.borrow()[0],
            HostScrollChangeEvent { x: 0., y: -50. }
        );
        assert_eq!(raw.borrow().len(), 1);
        assert!(raw.borrow()[0].precise);
        assert_eq!(raw.borrow()[0].phase, HostTouchPhase::Moved);
        assert_eq!(
            (raw.borrow()[0].delta_x, raw.borrow()[0].delta_y),
            (0., -50.)
        );
        changes.borrow_mut().clear();
        raw.borrow_mut().clear();

        // Programmatic scrolling reports too.
        cx.update(|window, _| node.scroll_to(point(px(0.), px(-800.)), window));
        draw(cx);
        assert_eq!(
            *changes.borrow(),
            [HostScrollChangeEvent { x: 0., y: -800. }]
        );
        changes.borrow_mut().clear();

        // A wheel event at the clamped limit is a raw scroll but changes nothing.
        wheel(cx, point(px(100.), px(100.)), 0., -50.);
        assert_eq!(raw.borrow().len(), 1);
        assert!(changes.borrow().is_empty(), "{:?}", changes.borrow());
        raw.borrow_mut().clear();

        // Offsets below 0.01 px are suppressed.
        cx.update(|window, _| node.scroll_to(point(px(0.), px(-799.995)), window));
        draw(cx);
        assert!(changes.borrow().is_empty());

        // Removing the listener releases the retained offset.
        node.remove_listener(HostEvent::ScrollChange);
        assert_eq!(node.has_report_state().1, false);
    }

    #[crate::test]
    fn wheel_lines_are_reported_as_imprecise(cx: &mut TestAppContext) {
        let (_, cx, node) = scrolling_view(cx, HostScroll::Vertical);
        let raw: Rc<RefCell<Vec<HostScrollEvent>>> = Default::default();
        node.on_scroll({
            let raw = raw.clone();
            move |event, _, _| raw.borrow_mut().push(*event)
        });
        draw(cx);
        cx.update(|window, cx| {
            let _ = (window, cx);
        });
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(100.), px(100.)),
            delta: ScrollDelta::Lines(point(0., -3.)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Started,
        });
        let raw = raw.borrow();
        assert_eq!(raw.len(), 1);
        assert!(!raw[0].precise);
        assert_eq!((raw[0].delta_x, raw[0].delta_y), (0., -3.));
        assert_eq!(raw[0].phase, HostTouchPhase::Started);
    }

    #[crate::test]
    fn scroller_axes_rerenders_and_handle_lifetime(cx: &mut TestAppContext) {
        // Two-axis scrollers move diagonally with one wheel event.
        let (view, cx, node) = scrolling_view(cx, HostScroll::Both);
        wheel(cx, point(px(100.), px(100.)), -30., -40.);
        let handle = node.scroll_handle().expect("handle");
        assert_eq!(handle.offset(), point(px(-30.), px(-40.)));

        // A rerender keeps the offset and the handle.
        rerender(&view, cx);
        assert_eq!(handle.offset(), point(px(-30.), px(-40.)));
        // The node still hands out the very same handle (it shares state).
        handle.set_offset(point(px(-5.), px(-6.)));
        assert_eq!(
            node.scroll_handle().expect("handle").offset(),
            point(px(-5.), px(-6.))
        );

        // Disabling scrolling releases the handle; re-enabling starts at zero.
        node.set_scroll(HostScroll::None);
        assert!(node.scroll_handle().is_none());
        rerender(&view, cx);
        node.set_scroll(HostScroll::Both);
        rerender(&view, cx);
        assert_eq!(
            node.scroll_handle().expect("handle").offset(),
            point(px(0.), px(0.))
        );
    }

    #[crate::test]
    fn horizontal_scrollers_ignore_vertical_deltas(cx: &mut TestAppContext) {
        let (_, cx, node) = scrolling_view(cx, HostScroll::Horizontal);
        wheel(cx, point(px(100.), px(100.)), 0., -40.);
        assert_eq!(
            node.scroll_handle().expect("handle").offset(),
            point(px(0.), px(0.)),
            "vertical delta ignored"
        );
        // The vertical gesture ends; a new, diagonal gesture is locked to its
        // dominant (horizontal) axis.
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(100.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(0.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Ended,
        });
        wheel(cx, point(px(100.), px(100.)), -40., -30.);
        assert_eq!(
            node.scroll_handle().expect("handle").offset(),
            point(px(-40.), px(0.))
        );
    }

    // ----- INP-006 A4: focus -------------------------------------------------------

    fn focused_node(cx: &mut VisualTestContext, nodes: &[HostNode]) -> Option<usize> {
        let focused = cx.update(|window, cx| window.focused(cx).map(|handle| handle.id()));
        nodes
            .iter()
            .position(|node| Some(node.focus_id()) == focused)
    }

    #[crate::test]
    fn editors_are_tab_stops_at_index_zero_and_negative_indices_are_programmatic(
        cx: &mut TestAppContext,
    ) {
        let (_, cx, nodes) = host_view(cx, 4);
        // Node 0: editor (default index 0). Node 1: negative index. Node 2: a
        // plain node with an explicit index. Node 3: a plain node that is not
        // focusable at all.
        nodes[0].set_editor(true);
        nodes[1].set_editor(true);
        nodes[1].set_tab_index(Some(-1));
        nodes[2].set_tab_index(Some(0));
        assert!(nodes[0].is_focusable() && nodes[1].is_focusable() && nodes[2].is_focusable());
        assert!(!nodes[3].is_focusable());
        draw(cx);

        // Sequential traversal: editor, then the explicit node, skipping the
        // negative-index node, wrapping around.
        cx.update(|window, cx| nodes[0].focus_next(window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(0));
        cx.update(|window, cx| nodes[0].focus_next(window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(2));
        cx.update(|window, cx| nodes[0].focus_next(window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(0));
        cx.update(|window, cx| nodes[0].focus_prev(window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(2));
        cx.update(|window, cx| nodes[0].focus_prev(window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(0));

        // The negative-index node is focusable by ref, and blur releases it.
        cx.update(|window, cx| nodes[1].focus(window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(1));
        draw(cx);
        assert_eq!(focused_node(cx, &nodes), Some(1));
        cx.update(|window, cx| nodes[1].blur(window, cx));
        assert_eq!(focused_node(cx, &nodes), None);
        // Blurring a node that is not focused does nothing.
        cx.update(|window, cx| nodes[0].focus(window, cx));
        cx.update(|window, cx| nodes[2].blur(window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(0));
    }

    #[crate::test]
    fn scoped_traversal_wraps_inside_the_scope(cx: &mut TestAppContext) {
        let (_, cx, nodes) = host_view(cx, 4);
        for node in &nodes {
            node.set_tab_index(Some(0));
        }
        draw(cx);
        let scope = [nodes[1].clone(), nodes[3].clone()];

        // Nothing focused: forward starts at the first accepted stop.
        cx.update(|window, cx| HostNode::focus_next_in(&scope, window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(1));
        cx.update(|window, cx| HostNode::focus_next_in(&scope, window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(3));
        cx.update(|window, cx| HostNode::focus_next_in(&scope, window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(1), "wraps");
        cx.update(|window, cx| HostNode::focus_prev_in(&scope, window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(3), "wraps backward");
        cx.update(|window, cx| HostNode::focus_prev_in(&scope, window, cx));
        assert_eq!(focused_node(cx, &nodes), Some(1));
    }

    #[crate::test]
    fn focus_and_blur_callbacks_follow_the_persistent_handle(cx: &mut TestAppContext) {
        let (view, cx, nodes) = host_view(cx, 2);
        let log = logger();
        for (ix, node) in nodes.iter().enumerate() {
            node.set_tab_index(Some(0));
            node.on_focus({
                let log = log.clone();
                move |_, _| log.borrow_mut().push(format!("focus {ix}"))
            });
            node.on_blur({
                let log = log.clone();
                move |_, _| log.borrow_mut().push(format!("blur {ix}"))
            });
        }
        draw(cx);
        let handle_before = nodes[0].focus_id();
        cx.update(|window, cx| nodes[0].focus(window, cx));
        draw(cx);
        cx.update(|window, cx| nodes[1].focus(window, cx));
        draw(cx);
        assert_eq!(take(&log), ["focus 0", "blur 0", "focus 1"]);

        // The handle outlives rerenders; replaced callbacks apply immediately.
        rerender(&view, cx);
        assert_eq!(handle_before, nodes[0].focus_id());
        nodes[1].on_blur({
            let log = log.clone();
            move |_, _| log.borrow_mut().push("blur 1 (replaced)".into())
        });
        cx.update(|window, cx| nodes[0].focus(window, cx));
        draw(cx);
        assert_eq!(take(&log), ["focus 0", "blur 1 (replaced)"]);
        nodes[0].remove_listener(HostEvent::Blur);
        cx.update(|window, cx| nodes[1].focus(window, cx));
        draw(cx);
        assert_eq!(take(&log), ["focus 1"]);
    }

    fn next_frame(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        draw(cx);
    }

    #[crate::test]
    fn autofocus_applies_unless_an_explicit_request_or_user_focus_change_came_first(
        cx: &mut TestAppContext,
    ) {
        // Autofocus alone applies after mount.
        let (_, cx, nodes) = host_view(cx, 2);
        for node in &nodes {
            node.set_tab_index(Some(0));
        }
        nodes[0].set_autofocus(true);
        draw(cx);
        assert_eq!(focused_node(cx, &nodes), None);
        next_frame(cx);
        assert_eq!(focused_node(cx, &nodes), Some(0));
    }

    #[crate::test]
    fn explicit_focus_request_outranks_autofocus(cx: &mut TestAppContext) {
        let (view, cx, nodes) = host_view(cx, 2);
        for node in &nodes {
            node.set_tab_index(Some(0));
        }
        // Node 0 autofocuses; node 1 is focused explicitly during mount (from
        // the render of the host tree, before the first paint).
        nodes[0].set_autofocus(true);
        let explicit = nodes[1].clone();
        view.update_in(cx, |_, window, cx| explicit.focus(window, cx));
        draw(cx);
        next_frame(cx);
        assert_eq!(focused_node(cx, &nodes), Some(1));

        // An explicit request to the node that already holds focus still
        // outranks the autofocus of another node.
        let (_, cx, nodes) = host_view(cx, 2);
        for node in &nodes {
            node.set_tab_index(Some(0));
        }
        draw(cx);
        cx.update(|window, cx| nodes[1].focus(window, cx));
        draw(cx);
        next_frame(cx);
        nodes[0].set_autofocus(true);
        let explicit = nodes[1].clone();
        cx.update(|window, cx| explicit.focus(window, cx));
        draw(cx);
        next_frame(cx);
        assert_eq!(focused_node(cx, &nodes), Some(1));
    }

    #[crate::test]
    fn user_focus_change_is_not_overridden_by_pending_autofocus(cx: &mut TestAppContext) {
        let (_, cx, nodes) = host_view(cx, 3);
        for node in &nodes {
            node.set_tab_index(Some(0));
        }
        nodes[0].set_autofocus(true);
        draw(cx);
        // The user moves focus (for example by clicking) before the pending
        // autofocus applies.
        let user_choice = nodes[2].focus_handle();
        cx.update(|window, cx| window.focus(&user_choice, cx));
        draw(cx);
        next_frame(cx);
        assert_eq!(focused_node(cx, &nodes), Some(2));
    }
}
