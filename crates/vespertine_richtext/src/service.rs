//! The window-level text service: per-frame registry, selection state, the window listeners
//! (press, move, release, copy) and the scope elements (TXT-008 R1, R2, R5, R9, R12, R13, R15).
//!
//! The registry is built while the frame draws, but a retained frame (PRF-001) replays the output
//! of views that did not change instead of running their elements. Every update an element makes
//! to the service while it draws is therefore a replayable effect (`Window::prepaint_effect`,
//! `Window::paint_effect`): replaying a view performs its effects again, in order, so the registry,
//! the logs, the highlight numbering and the scope stacks a frame ends with are the same whether
//! its views were built or replayed.

use crate::autoscroll::{Autoscroll, RegisteredList, TICK, TickOutcome};
use crate::highlight::HostFrame;
use crate::registry::{
    PaintedHighlight, PaintedText, RegisteredRun, Registry, RunKey, StartRegion, SurfaceId,
};
use crate::selection::{Outcome, Selection, resolve_spans};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use vespertine::{
    AnyElement, App, Bounds, ClipboardItem, DispatchPhase, Element, ElementId, Global,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window, WindowId, rgb,
};

/// `userSelect` of a subtree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UserSelect {
    /// Text of the subtree is painted but neither registered nor selectable; presses do not start
    /// a selection.
    None,
    /// Selectable; an explicit island may override an ancestor `None`.
    Text,
    /// Selectable (the default).
    #[default]
    Auto,
}

/// Colors the text service needs from the theme.
#[derive(Clone, Copy, Debug)]
pub struct TextTheme {
    /// The accent: selection and search washes default to it with their own alpha.
    pub accent: Hsla,
}

impl Default for TextTheme {
    fn default() -> Self {
        Self {
            accent: rgb(0x7c86ff).into(),
        }
    }
}

impl Global for TextTheme {}

/// Default alpha of the selection wash.
pub const SELECTION_ALPHA: f32 = 0.35;

type ChangeCallback = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;
type DragEndCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// Where a programmatic selection starts and ends.
#[derive(Clone, Copy, Debug)]
pub struct SelectionRequest {
    /// First endpoint: run key and byte offset.
    pub from: (RunKey, usize),
    /// Second endpoint: run key and byte offset.
    pub to: (RunKey, usize),
}

#[derive(Default)]
struct State {
    registry: Registry,
    selection: Selection,
    accent: Option<Hsla>,
    surfaces: Vec<SurfaceId>,
    user_select: Vec<UserSelect>,
    selection_colors: Vec<Hsla>,
    highlights: Vec<Rc<HostFrame>>,
    lists: Vec<RegisteredList>,
    autoscroll: Autoscroll,
    on_change: Option<ChangeCallback>,
    on_drag_end: Option<DragEndCallback>,
    pending_request: Option<SelectionRequest>,
    frames: u64,
}

/// The text service of one window. Cheap to clone.
#[derive(Clone, Default)]
pub struct TextService(Rc<RefCell<State>>);

thread_local! {
    static CURRENT: RefCell<Vec<TextService>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
struct Services(HashMap<WindowId, TextService>);

impl Global for Services {}

impl TextService {
    /// The service of `window`, created on first use.
    pub fn of(window: &Window, cx: &mut App) -> TextService {
        let id = window.window_handle().window_id();
        if let Some(service) = cx.try_global::<Services>().and_then(|s| s.0.get(&id)) {
            return service.clone();
        }
        let service = TextService::default();
        cx.default_global::<Services>()
            .0
            .insert(id, service.clone());
        service
    }

    /// The service of the root being painted, if any.
    pub fn current() -> Option<TextService> {
        CURRENT.with(|stack| stack.borrow().last().cloned())
    }

    fn push_current(&self) {
        CURRENT.with(|stack| stack.borrow_mut().push(self.clone()));
    }

    fn pop_current() {
        CURRENT.with(|stack| stack.borrow_mut().pop());
    }

    /// Sets the callback invoked when the selection identity changes, with the joined selected
    /// text or `None` when the selection became empty.
    pub fn on_selection_change(
        &self,
        callback: impl Fn(Option<String>, &mut Window, &mut App) + 'static,
    ) {
        self.0.borrow_mut().on_change = Some(Rc::new(callback));
    }

    /// Sets the callback invoked when a live drag ends (never for a tap).
    pub fn on_drag_end(&self, callback: impl Fn(&mut Window, &mut App) + 'static) {
        self.0.borrow_mut().on_drag_end = Some(Rc::new(callback));
    }

    /// The joined selected text, or `None` when the selection is empty.
    pub fn selected_text(&self) -> Option<String> {
        self.0.borrow().selection.selected_text()
    }

    /// Whether a non-empty selection exists.
    pub fn selection_has_text(&self) -> bool {
        self.0.borrow().selection.has_text()
    }

    /// Identity of the selection (0 when empty).
    pub fn selection_identity(&self) -> u64 {
        self.0.borrow().selection.identity()
    }

    /// Removes the selection and its washes immediately.
    pub fn clear_selection(&self, window: &mut Window, cx: &mut App) {
        let changed = self.0.borrow_mut().selection.clear();
        if changed {
            window.refresh();
        }
        self.notify_change(window, cx);
    }

    /// Selects between two runs programmatically (select-all, find navigation). Applied when the
    /// current frame finishes painting, so the runs must paint in that frame.
    pub fn select_runs(&self, request: SelectionRequest) {
        self.0.borrow_mut().pending_request = Some(request);
    }

    /// Strings painted in the last frame, in paint order (chrome text included).
    pub fn painted_text(&self) -> Vec<PaintedText> {
        self.0.borrow().registry.painted_text().to_vec()
    }

    /// Washes painted in the last frame, in paint order.
    pub fn painted_highlights(&self) -> Vec<PaintedHighlight> {
        self.0.borrow().registry.painted_highlights().to_vec()
    }

    /// Number of registered runs in the last frame.
    pub fn registered_runs(&self) -> usize {
        self.0.borrow().registry.runs().len()
    }

    /// Registers a scrollable list so a selection drag can autoscroll it. Paint-time: performed
    /// now and again whenever the painting view is replayed.
    pub fn register_list(&self, list: RegisteredList, window: &mut Window, cx: &mut App) {
        let service = self.clone();
        window.paint_effect(
            cx,
            Rc::new(move |_, _| service.0.borrow_mut().lists.push(list.clone())),
        );
    }

    // --- paint-time API used by elements ---------------------------------------------------
    //
    // Each mutator exists twice: the public one performs the update as a paint effect (it is
    // performed again when the painting view is replayed, PRF-001) and the `*_now` one is for code
    // that already runs inside an effect.

    /// The surface being painted.
    pub fn surface(&self) -> SurfaceId {
        self.0.borrow().surfaces.last().copied().unwrap_or_default()
    }

    /// Whether text painted now may be selected (R2).
    pub fn selectable_now(&self) -> bool {
        self.0
            .borrow()
            .user_select
            .last()
            .is_none_or(|mode| *mode != UserSelect::None)
    }

    /// Registers a run (document order = paint order).
    pub fn register_run(&self, run: RegisteredRun, window: &mut Window, cx: &mut App) {
        let service = self.clone();
        window.paint_effect(
            cx,
            Rc::new(move |_, _| service.register_run_now(run.clone())),
        );
    }

    pub(crate) fn register_run_now(&self, run: RegisteredRun) {
        self.0.borrow_mut().registry.register(run);
    }

    /// Registers a selection start region.
    pub fn register_region(&self, region: StartRegion, window: &mut Window, cx: &mut App) {
        let service = self.clone();
        window.paint_effect(cx, Rc::new(move |_, _| service.register_region_now(region)));
    }

    pub(crate) fn register_region_now(&self, region: StartRegion) {
        self.0.borrow_mut().registry.register_region(region);
    }

    /// Logs a painted string.
    pub fn log_text(&self, entry: PaintedText, window: &mut Window, cx: &mut App) {
        let service = self.clone();
        window.paint_effect(cx, Rc::new(move |_, _| service.log_text_now(entry.clone())));
    }

    pub(crate) fn log_text_now(&self, entry: PaintedText) {
        self.0.borrow_mut().registry.log_text(entry);
    }

    /// Logs a painted wash.
    pub fn log_highlight(&self, entry: PaintedHighlight, window: &mut Window, cx: &mut App) {
        let service = self.clone();
        window.paint_effect(
            cx,
            Rc::new(move |_, _| service.log_highlight_now(entry.clone())),
        );
    }

    pub(crate) fn log_highlight_now(&self, entry: PaintedHighlight) {
        self.0.borrow_mut().registry.log_highlight(entry);
    }

    /// The selection wash range for a run painted now (R9, R14).
    pub fn selection_wash(&self, key: RunKey) -> Option<std::ops::Range<usize>> {
        let state = self.0.borrow();
        let surface = state.surfaces.last().copied().unwrap_or_default();
        state.selection.wash_range(surface, key)
    }

    /// Color of the selection wash: the inherited `selectionColor`, otherwise the accent with
    /// alpha 0.35 (R14).
    pub fn selection_color(&self) -> Hsla {
        let state = self.0.borrow();
        state.selection_colors.last().copied().unwrap_or_else(|| {
            let accent = state.accent.unwrap_or_else(|| TextTheme::default().accent);
            Hsla {
                a: SELECTION_ALPHA,
                ..accent
            }
        })
    }

    /// Monotonic frame counter; contexts that share a numbering use it to restart the sequence
    /// once per frame.
    pub fn frame_id(&self) -> u64 {
        self.0.borrow().frames
    }

    pub(crate) fn push_highlight(&self, frame: Rc<HostFrame>) {
        self.0.borrow_mut().highlights.push(frame);
    }

    pub(crate) fn pop_highlight(&self) {
        self.0.borrow_mut().highlights.pop();
    }

    /// The nearest declaring ancestor's highlight (an empty declaration owns its subtree too).
    pub(crate) fn highlight(&self) -> Option<Rc<HostFrame>> {
        self.0.borrow().highlights.last().cloned()
    }

    /// The accent color.
    pub fn accent(&self) -> Hsla {
        self.0
            .borrow()
            .accent
            .unwrap_or_else(|| TextTheme::default().accent)
    }

    // --- frame lifecycle ---------------------------------------------------------------------

    fn begin_frame(&self, cx: &App) {
        let mut state = self.0.borrow_mut();
        state.registry.reset();
        state.lists.clear();
        state.surfaces.clear();
        state.user_select.clear();
        state.selection_colors.clear();
        state.highlights.clear();
        state.accent = cx.try_global::<TextTheme>().map(|theme| theme.accent);
        state.frames += 1;
    }

    fn finish_frame(&self, window: &mut Window, cx: &mut App) {
        let request = self.0.borrow_mut().pending_request.take();
        if let Some(request) = request {
            let applied = {
                let mut state = self.0.borrow_mut();
                let state = &mut *state;
                let find =
                    |key: RunKey| state.registry.runs().iter().position(|run| run.key == key);
                match (find(request.from.0), find(request.to.0)) {
                    (Some(a), Some(b)) => {
                        let spans =
                            resolve_spans(&state.registry, (a, request.from.1), (b, request.to.1));
                        state.selection.set_spans(spans);
                        true
                    }
                    _ => false,
                }
            };
            if applied {
                window.refresh();
            }
        }
        self.notify_change(window, cx);
    }

    fn notify_change(&self, window: &mut Window, cx: &mut App) {
        let (change, callback) = {
            let mut state = self.0.borrow_mut();
            (state.selection.take_change(), state.on_change.clone())
        };
        if let (Some(text), Some(callback)) = (change, callback) {
            window.defer(cx, move |window, cx| callback(text, window, cx));
        }
    }

    fn apply(&self, outcome: Outcome, window: &mut Window, cx: &mut App) {
        if outcome.blur {
            window.blur(cx);
        }
        if outcome.refresh {
            window.refresh();
        }
        self.notify_change(window, cx);
    }

    fn install_listeners(&self, window: &mut Window) {
        let service = self.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            let outcome = {
                let mut state = service.0.borrow_mut();
                let state = &mut *state;
                state
                    .selection
                    .press(&state.registry, event.position, event.click_count)
            };
            service.apply(outcome, window, cx);
        });

        let service = self.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let dragging = event.pressed_button == Some(MouseButton::Left);
            if !dragging && service.0.borrow().selection.is_dragging() {
                // The button was released outside the window: end the drag.
                service.release(window, cx);
                return;
            }
            let outcome = {
                let mut state = service.0.borrow_mut();
                let state = &mut *state;
                state
                    .selection
                    .drag_move(&state.registry, event.position, dragging)
            };
            service.apply(outcome, window, cx);
            if dragging && service.0.borrow().selection.is_dragging() {
                service.track_pointer(event.position, window, cx);
            }
        });

        let service = self.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            service.release(window, cx);
        });

        let service = self.clone();
        window.on_key_event(move |event: &KeyDownEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let keystroke = &event.keystroke;
            if keystroke.key == "c"
                && keystroke.modifiers.secondary()
                && !keystroke.modifiers.shift
                && !keystroke.modifiers.alt
                && let Some(text) = service.selected_text()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.stop_propagation();
            }
        });
    }

    fn release(&self, window: &mut Window, cx: &mut App) {
        let (drag_ended, callback) = {
            let mut state = self.0.borrow_mut();
            let ended = state.selection.release();
            state.autoscroll.finish();
            (ended, state.on_drag_end.clone())
        };
        self.notify_change(window, cx);
        if drag_ended && let Some(callback) = callback {
            window.defer(cx, move |window, cx| callback(window, cx));
        }
    }

    /// Tracks the pointer of a live drag and starts the autoscroll loop when it is in an edge
    /// band (R15).
    fn track_pointer(&self, position: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let start = {
            let mut state = self.0.borrow_mut();
            let state = &mut *state;
            let step = state.autoscroll.track(&state.lists, position);
            step != 0.0 && state.autoscroll.begin_ticking()
        };
        if !start {
            return;
        }
        let service = self.clone();
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    let outcome = cx
                        .update(|window, cx| service.tick(window, cx))
                        .unwrap_or(TickOutcome::Stop);
                    if outcome == TickOutcome::Stop {
                        break;
                    }
                }
            })
            .detach();
    }

    /// One autoscroll tick (R15): re-resolve at the tracked pointer, scroll the owning list,
    /// refresh. Exposed so tests and hosts can drive it without a timer.
    pub fn tick(&self, window: &mut Window, cx: &mut App) -> TickOutcome {
        let outcome = {
            let mut state = self.0.borrow_mut();
            let state = &mut *state;
            let live = state.selection.is_dragging();
            let registry = &state.registry;
            let selection = &mut state.selection;
            state.autoscroll.tick(&state.lists, live, |pointer| {
                selection.update_head(registry, pointer)
            })
        };
        window.refresh();
        self.notify_change(window, cx);
        outcome
    }
}

#[derive(Clone, Copy)]
enum ScopeKind {
    Root,
    Surface(SurfaceId),
    UserSelect(UserSelect),
    SelectionColor(Hsla),
}

/// An element that scopes text service state to its subtree.
pub struct Scope {
    kind: ScopeKind,
    child: AnyElement,
}

/// The root of a document: resets the registry and logs for the frame (as the first painted child
/// of the root) and installs the window-level press, move, release and copy listeners.
pub fn text_root(child: impl IntoElement) -> Scope {
    Scope {
        kind: ScopeKind::Root,
        child: child.into_any_element(),
    }
}

/// Paints `child` as a distinct surface: run keys are only unique within one (R9).
pub fn surface(id: u64, child: impl IntoElement) -> Scope {
    Scope {
        kind: ScopeKind::Surface(SurfaceId(id)),
        child: child.into_any_element(),
    }
}

/// Sets `userSelect` for `child`'s subtree (R2).
pub fn user_select(mode: UserSelect, child: impl IntoElement) -> Scope {
    Scope {
        kind: ScopeKind::UserSelect(mode),
        child: child.into_any_element(),
    }
}

/// Sets the inherited `selectionColor` for `child`'s subtree.
pub fn selection_color(color: impl Into<Hsla>, child: impl IntoElement) -> Scope {
    Scope {
        kind: ScopeKind::SelectionColor(color.into()),
        child: child.into_any_element(),
    }
}

impl IntoElement for Scope {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl ScopeKind {
    fn enter(self, service: Option<&TextService>) {
        let Some(service) = service else { return };
        let mut state = service.0.borrow_mut();
        match self {
            ScopeKind::Root => {}
            ScopeKind::Surface(id) => state.surfaces.push(id),
            ScopeKind::UserSelect(mode) => state.user_select.push(mode),
            ScopeKind::SelectionColor(color) => state.selection_colors.push(color),
        }
    }

    fn exit(self, service: Option<&TextService>) {
        let Some(service) = service else { return };
        let mut state = service.0.borrow_mut();
        match self {
            ScopeKind::Root => {}
            ScopeKind::Surface(_) => {
                state.surfaces.pop();
            }
            ScopeKind::UserSelect(_) => {
                state.user_select.pop();
            }
            ScopeKind::SelectionColor(_) => {
                state.selection_colors.pop();
            }
        }
    }

    /// The token views nested in the scope are replayed under: a nested view is replayed only
    /// while the scope it inherits is unchanged (its output depends on the inherited setting).
    fn retention_token(self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match self {
            ScopeKind::Root => 0u8.hash(&mut hasher),
            ScopeKind::Surface(id) => (1u8, id.0).hash(&mut hasher),
            ScopeKind::UserSelect(mode) => (2u8, mode as u8).hash(&mut hasher),
            ScopeKind::SelectionColor(color) => (
                3u8,
                color.h.to_bits(),
                color.s.to_bits(),
                color.l.to_bits(),
                color.a.to_bits(),
            )
                .hash(&mut hasher),
        }
        hasher.finish()
    }
}

/// State a scope remembers between prepaint and paint.
pub struct ScopeState {
    service: Option<TextService>,
}

impl Element for Scope {
    type RequestLayoutState = ();
    type PrepaintState = ScopeState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = match self.kind {
            ScopeKind::Root => self.child.request_layout(window, cx),
            kind => window.with_retention_context(kind.retention_token(), |window| {
                self.child.request_layout(window, cx)
            }),
        };
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let kind = self.kind;
        if matches!(kind, ScopeKind::Root) {
            let service = TextService::of(window, cx);
            let begin = service.clone();
            window.prepaint_effect(
                cx,
                Rc::new(move |_, cx| {
                    begin.begin_frame(cx);
                    begin.push_current();
                }),
            );
            self.child.prepaint(window, cx);
            window.prepaint_effect(cx, Rc::new(|_, _| TextService::pop_current()));
            return ScopeState {
                service: Some(service),
            };
        }
        let service = TextService::current();
        let entering = service.clone();
        window.prepaint_effect(cx, Rc::new(move |_, _| kind.enter(entering.as_ref())));
        window.with_retention_context(kind.retention_token(), |window| {
            self.child.prepaint(window, cx)
        });
        let exiting = service.clone();
        window.prepaint_effect(cx, Rc::new(move |_, _| kind.exit(exiting.as_ref())));
        ScopeState { service }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let kind = self.kind;
        let service = prepaint.service.clone();
        if matches!(kind, ScopeKind::Root) {
            let Some(service) = service else {
                self.child.paint(window, cx);
                return;
            };
            service.install_listeners(window);
            let entering = service.clone();
            window.paint_effect(cx, Rc::new(move |_, _| entering.push_current()));
            self.child.paint(window, cx);
            window.paint_effect(cx, Rc::new(|_, _| TextService::pop_current()));
            window.paint_effect(
                cx,
                Rc::new(move |window, cx| service.finish_frame(window, cx)),
            );
            return;
        }
        if let (Some(service), ScopeKind::UserSelect(mode)) = (&service, kind)
            && mode != UserSelect::Auto
        {
            let service = service.clone();
            let region = StartRegion {
                bounds,
                selectable: mode == UserSelect::Text,
            };
            window.paint_effect(cx, Rc::new(move |_, _| service.register_region_now(region)));
        }
        let entering = service.clone();
        window.paint_effect(cx, Rc::new(move |_, _| kind.enter(entering.as_ref())));
        self.child.paint(window, cx);
        window.paint_effect(cx, Rc::new(move |_, _| kind.exit(service.as_ref())));
    }
}
