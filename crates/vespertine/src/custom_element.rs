//! Persistent custom-element adapters (ELM-007).
//!
//! A [`CustomElementRegistry`] maps host type names to [`ElementFactory`]s and keeps exactly one
//! [`ElementAdapter`] per live host identity ([`HostId`]). Adapters keep their own retained state
//! (buffers, decoded images, scroll offsets) and build native elements every frame from a
//! [`RenderContext`]. The registry owns the adapter lifecycle (create on first render, destroy
//! on removal, on type change, and on teardown), property synchronization with explicit resets
//! (R5), event filtering (R6) and image-cache release (R3, R11).
//!
//! # Built-in types (R12)
//!
//! [`CustomElementRegistry::default`] contains the built-ins that exist natively in this tree:
//! currently only `anchored` ([`AnchoredFactory`](crate::AnchoredFactory), LAY-005). The
//! remaining names (`input`, `textarea`, `img`, `svg`, `code`, `diff`, `markdown`) belong to
//! other specs; their owners register them with [`CustomElementRegistry::register`].

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fmt,
    rc::Rc,
    sync::Arc,
};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value};

use crate::{
    AnyElement, App, Bounds, Div, Edges, ElementId, EmitEvent, Empty, EventSet, FocusHandle,
    HostStyle, Hsla, InteractiveElement, Interactivity, IntoElement, ParentElement, Pixels,
    RenderImage, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement, Styled,
    Subscription, TextRun, Window, apply_aria_properties, div,
    host_surface::{BoundsTracker, TrackBounds, track_bounds, wire_standard_events},
    host_text::{HostTextProvider, plain_text},
};

/// The stable identity of a host element: a JavaScript-safe nonnegative integer chosen by the
/// embedding side (APP-006).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostId(pub u64);

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for HostId {
    fn from(id: u64) -> Self {
        HostId(id)
    }
}

impl HostId {
    /// The stable element identity derived from this host identity. It keys the hover/active
    /// pseudo-style state and the accessibility node of the host's container.
    pub fn element_id(self) -> ElementId {
        ElementId::NamedInteger(SharedString::new_static("host"), self.0)
    }
}

/// The control holding keyboard focus and its ancestors, focused control first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyboardFocusPath {
    /// Identities from the focused control up to the root.
    pub ids: Vec<HostId>,
}

impl KeyboardFocusPath {
    /// The control holding keyboard focus.
    pub fn focused(&self) -> Option<HostId> {
        self.ids.first().copied()
    }

    /// Whether `id` is the focused control or one of its ancestors.
    pub fn contains(&self, id: HostId) -> bool {
        self.ids.contains(&id)
    }
}

/// Everything an adapter receives when rendering one host for one frame (R7).
pub struct RenderContext {
    /// The host identity.
    pub id: HostId,
    /// The host's retained custom properties, including role and accessibility properties.
    pub properties: Arc<Map<String, Value>>,
    /// The events the host subscribed to. The registry narrows this to the adapter's declared
    /// events before rendering.
    pub events: EventSet,
    /// Emits an event to the host's listener.
    pub emit: EmitEvent,
    /// The host's focus handle when it has keyboard or focus listeners.
    pub focus_handle: Option<FocusHandle>,
    /// The keyboard-focus path; absent when nothing holds keyboard focus or the focus is a text
    /// editor.
    pub keyboard_focus: Option<KeyboardFocusPath>,
    /// The host's style description (ELM-006).
    pub style: Arc<HostStyle>,
    /// The host's built children.
    pub children: Vec<AnyElement>,
    /// Builds selectable and chrome text and exposes the live text selection (TXT-008).
    pub text: Option<Rc<dyn HostTextProvider>>,
    /// Whether text is selectable here (false inside a subtree that disabled selection).
    pub selectable: bool,
    /// The inherited selection wash color.
    pub selection_color: Hsla,
    /// The unresolved search highlight declaration of the nearest ancestor (TXT-009), as sent.
    pub search_highlight: Option<Arc<Value>>,
    /// Records this host's last painted border box; set up by the registry per identity.
    pub bounds: BoundsTracker,
    /// Border widths of the parent (trigger) element, for hosts positioned relative to it.
    pub trigger_border: Edges<Pixels>,
}

impl RenderContext {
    /// The default selection wash: a 35% opaque blue until the host supplies the theme accent.
    pub fn default_selection_color() -> Hsla {
        crate::blue().opacity(0.35)
    }

    /// A context for host `id` with no properties, events, focus, style or children, selectable
    /// text and the default selection wash.
    pub fn new(id: HostId, emit: EmitEvent) -> Self {
        Self {
            id,
            properties: Arc::default(),
            events: EventSet::new(),
            emit,
            focus_handle: None,
            keyboard_focus: None,
            style: Arc::default(),
            children: Vec::new(),
            text: None,
            selectable: true,
            selection_color: Self::default_selection_color(),
            search_highlight: None,
            bounds: BoundsTracker::new(),
            trigger_border: Edges::default(),
        }
    }

    /// The stable element identity of this host.
    pub fn element_id(&self) -> ElementId {
        self.id.element_id()
    }

    /// Selectable text (R10). The run is identified by this host plus `sub_index`, which the
    /// adapter MUST keep stable across frames for the same logical run. It registers with the
    /// window selection registry while painting; if selection is disabled for this subtree it is
    /// only painted and logged. Without a text provider it is plain, unselectable text.
    pub fn selectable_text(
        &self,
        sub_index: u32,
        text: impl Into<SharedString>,
        runs: Option<Vec<TextRun>>,
    ) -> AnyElement {
        let text = text.into();
        match &self.text {
            Some(provider) => {
                provider.selectable_run(self.id, sub_index, self.selectable, text, runs)
            }
            None => plain_text(text, runs),
        }
    }

    /// Chrome text (line numbers, language tags, file headers): painted and logged for tests but
    /// never part of a selection, so copied text never contains it. `sub_index` distinguishes
    /// chrome runs of one host in the painted-text log.
    pub fn chrome_text(
        &self,
        sub_index: u32,
        text: impl Into<SharedString>,
        runs: Option<Vec<TextRun>>,
    ) -> AnyElement {
        let text = text.into();
        match &self.text {
            Some(provider) => provider.chrome_run(self.id, sub_index, text, runs),
            None => plain_text(text, runs),
        }
    }

    /// Applies the shared plumbing to any stateful interactive element (R8, R9): the style with
    /// hover and active refinements, the accessibility properties, the focus handle, and a
    /// handler for every standard event the host subscribed to. Wrap the result with
    /// [`RenderContext::track`] to report its bounds. Leaf elements (images) use this directly.
    pub fn apply_to<E>(&self, element: E) -> E
    where
        E: Styled + StatefulInteractiveElement,
    {
        let mut element = self.style.apply(element);
        element = apply_aria_properties(element, &self.properties);
        if let Some(handle) = &self.focus_handle {
            element = element.track_focus(handle);
        }
        wire_standard_events(element, &self.events, &self.emit, &self.bounds)
    }

    /// Records the last painted border box of `element` for this host.
    pub fn track(&self, element: impl IntoElement) -> TrackBounds {
        track_bounds(&self.bounds, element)
    }

    /// The shared container surface (R8): a stable-id container holding the host's children,
    /// with [`RenderContext::apply_to`] applied and last-painted-bounds tracking. Takes the
    /// children out of the context. Add adapter content with the usual builders.
    pub fn surface(&mut self) -> HostSurface {
        let container = self.apply_to(div().id(self.element_id()));
        let mut surface = HostSurface {
            container,
            tracker: self.bounds.clone(),
        };
        surface.extend(std::mem::take(&mut self.children));
        surface
    }
}

/// A container built by [`RenderContext::surface`]. It is styled, interactive and a parent like
/// a stateful `div`, and converts into an element that tracks its painted bounds.
pub struct HostSurface {
    container: Stateful<Div>,
    tracker: BoundsTracker,
}

impl Styled for HostSurface {
    fn style(&mut self) -> &mut StyleRefinement {
        self.container.style()
    }
}

impl InteractiveElement for HostSurface {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.container.interactivity()
    }
}

impl StatefulInteractiveElement for HostSurface {}

impl ParentElement for HostSurface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.container.extend(elements);
    }
}

impl IntoElement for HostSurface {
    type Element = TrackBounds;

    fn into_element(self) -> Self::Element {
        track_bounds(&self.tracker, self.container)
    }
}

/// Creates the adapter of one host type.
pub trait ElementFactory: 'static {
    /// The host type name this factory serves.
    fn type_name(&self) -> &'static str;

    /// Creates the adapter for host `id`.
    fn create(&self, id: HostId) -> Box<dyn ElementAdapter>;
}

/// A persistent native widget behind a host identity.
///
/// `render` is called every frame and the adapter keeps its state across calls; work that must
/// not repeat per frame (decoding, parsing) is cached in the adapter keyed by the property
/// values that produced it (R13).
pub trait ElementAdapter: 'static {
    /// Builds the native element for this frame.
    fn render(&mut self, context: RenderContext, window: &mut Window, cx: &mut App) -> AnyElement;

    /// Receives a changed property. `Value::Null` resets the property to its default.
    fn set_property(&mut self, name: &str, value: &Value);

    /// The properties this adapter understands (immutable per adapter type).
    fn supported_properties(&self) -> &'static [&'static str];

    /// The events this adapter can emit (immutable per adapter type).
    fn supported_events(&self) -> &'static [&'static str];

    /// Releases entity handles and subscriptions. Called exactly once, while the application
    /// context still exists. Images need not be released here: the registry releases the
    /// adapter's live and replaced images from the window's cache after this call.
    fn destroy(&mut self, cx: &mut App);

    /// The image currently shown, if this adapter holds one (R11).
    fn live_image(&self) -> Option<Arc<RenderImage>> {
        None
    }

    /// Takes one image that was replaced and still awaits release (R11).
    fn take_replaced_image(&mut self) -> Option<Arc<RenderImage>> {
        None
    }

    /// Installs a new live image and returns the previous one so the renderer can update its
    /// uploaded copy in place (R11).
    fn replace_live_image(&mut self, image: Arc<RenderImage>) -> Result<Option<Arc<RenderImage>>> {
        drop(image);
        Err(anyhow!("this adapter does not hold a live image"))
    }
}

/// Severity of a registry diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostDiagnosticLevel {
    /// Something is wrong but rendering continued.
    Warning,
}

/// A diagnostic recorded by the registry, also logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDiagnostic {
    /// Severity.
    pub level: HostDiagnosticLevel,
    /// The host the diagnostic concerns.
    pub host: HostId,
    /// Human-readable message.
    pub message: String,
}

struct Entry {
    type_name: SharedString,
    adapter: Box<dyn ElementAdapter>,
    /// Last value applied per property (never holds `Null`).
    applied: BTreeMap<String, Value>,
    bounds: BoundsTracker,
}

/// Maps host type names to factories and owns one adapter per live host identity.
pub struct CustomElementRegistry {
    factories: HashMap<SharedString, Rc<dyn ElementFactory>>,
    entries: HashMap<HostId, Entry>,
    diagnostics: Vec<HostDiagnostic>,
}

impl Default for CustomElementRegistry {
    /// A registry pre-populated with the built-in types that exist natively in this tree (R12);
    /// see the module documentation.
    fn default() -> Self {
        let mut registry = Self::empty();
        registry.register(crate::AnchoredFactory);
        registry
    }
}

impl CustomElementRegistry {
    /// A registry with no registered types.
    pub fn empty() -> Self {
        Self {
            factories: HashMap::new(),
            entries: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Registers `factory` for its type name, replacing any earlier factory of that name (R1).
    /// Live adapters created by the replaced factory keep running until their identity is
    /// destroyed or changes type.
    pub fn register(&mut self, factory: impl ElementFactory) {
        self.factories.insert(
            SharedString::new_static(factory.type_name()),
            Rc::new(factory),
        );
    }

    /// Whether a factory is registered for `type_name`.
    pub fn has_factory(&self, type_name: &str) -> bool {
        self.factories.contains_key(type_name)
    }

    /// The registered type names, sorted.
    pub fn type_names(&self) -> Vec<SharedString> {
        let mut names: Vec<_> = self.factories.keys().cloned().collect();
        names.sort();
        names
    }

    /// The number of live adapters.
    pub fn adapter_count(&self) -> usize {
        self.entries.len()
    }

    /// Whether host `id` currently has an adapter.
    pub fn contains(&self, id: HostId) -> bool {
        self.entries.contains_key(&id)
    }

    /// The type of the adapter of host `id`.
    pub fn adapter_type(&self, id: HostId) -> Option<&str> {
        self.entries.get(&id).map(|entry| &*entry.type_name)
    }

    /// Installs `observer` to be called with the border box of host `id` right after each paint
    /// that records it, so a host's bounds are known within the frame that painted it even when
    /// its content is deferred. Does nothing when the host has no adapter.
    pub fn observe_bounds(&mut self, id: HostId, observer: Option<crate::BoundsObserver>) {
        if let Some(entry) = self.entries.get(&id) {
            entry.bounds.observe(observer);
        }
    }

    /// The last painted border box of host `id`, once it has painted.
    pub fn last_bounds(&self, id: HostId) -> Option<Bounds<Pixels>> {
        self.entries.get(&id)?.bounds.get()
    }

    /// Drains the diagnostics recorded since the last call.
    pub fn take_diagnostics(&mut self) -> Vec<HostDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Returns the adapter of host `id`, creating it on first use (R1). If the identity exists
    /// with a different type, the prior adapter is destroyed first (R2). An unknown type yields
    /// `None`, records a warning and retains no adapter (R4).
    pub fn create_or_get(
        &mut self,
        id: HostId,
        type_name: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<&mut dyn ElementAdapter> {
        if self.ensure(id, type_name, window, cx) {
            Some(&mut *self.entries.get_mut(&id)?.adapter)
        } else {
            None
        }
    }

    fn ensure(&mut self, id: HostId, type_name: &str, window: &mut Window, cx: &mut App) -> bool {
        if self
            .entries
            .get(&id)
            .is_some_and(|entry| &*entry.type_name != type_name)
        {
            self.destroy(id, window, cx);
        }
        if self.entries.contains_key(&id) {
            return true;
        }
        let Some(factory) = self.factories.get(type_name).cloned() else {
            let message = format!("unknown host element type {type_name:?}");
            log::warn!("host {id}: {message}");
            self.diagnostics.push(HostDiagnostic {
                level: HostDiagnosticLevel::Warning,
                host: id,
                message,
            });
            return false;
        };
        self.entries.insert(
            id,
            Entry {
                type_name: SharedString::new_static(factory.type_name()),
                adapter: factory.create(id),
                applied: BTreeMap::new(),
                bounds: BoundsTracker::new(),
            },
        );
        true
    }

    /// Renders host `context.id` of `type_name` (R5-R7): synchronizes changed properties,
    /// filters the events to the adapter's declared ones, and returns the adapter's element. An
    /// unknown type renders an empty element and records a warning.
    pub fn render(
        &mut self,
        type_name: &str,
        mut context: RenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let id = context.id;
        if !self.ensure(id, type_name, window, cx) {
            return Empty.into_any_element();
        }
        let entry = self.entries.get_mut(&id).expect("ensured above");
        sync_properties(entry, &context.properties);
        let supported = entry.adapter.supported_events();
        context.events.retain(|event| supported.contains(&&**event));
        context.bounds = entry.bounds.clone();
        entry.adapter.render(context, window, cx)
    }

    /// Installs `image` as the live image of host `id`, creating the image adapter (type `img`)
    /// when the host has none, and returns the image it replaced (R11). Fails when no image
    /// factory is registered or the adapter does not hold images.
    pub fn set_live_image(
        &mut self,
        id: HostId,
        image: Arc<RenderImage>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Option<Arc<RenderImage>>> {
        if !self.entries.contains_key(&id) && !self.has_factory(IMAGE_TYPE) {
            return Err(anyhow!(
                "cannot set a live image on host {id}: no factory is registered for the image type {IMAGE_TYPE:?}"
            ));
        }
        if !self.ensure(id, IMAGE_TYPE, window, cx) {
            return Err(anyhow!("cannot create the image adapter of host {id}"));
        }
        let entry = self.entries.get_mut(&id).expect("ensured above");
        entry.adapter.replace_live_image(image)
    }

    /// Destroys the adapter of host `id` and releases its images from the window's cache (R3).
    /// Returns whether an adapter existed.
    pub fn destroy(&mut self, id: HostId, window: &mut Window, cx: &mut App) -> bool {
        self.destroy_entry(id, Some(window), cx)
    }

    /// Destroys every adapter whose identity `is_live` rejects (R3) and returns their identities
    /// in ascending order. Call after every applied tree change.
    pub fn prune_missing(
        &mut self,
        is_live: impl Fn(HostId) -> bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<HostId> {
        let mut gone: Vec<HostId> = self
            .entries
            .keys()
            .copied()
            .filter(|id| !is_live(*id))
            .collect();
        gone.sort();
        for id in &gone {
            self.destroy_entry(*id, Some(&mut *window), cx);
        }
        gone
    }

    /// Destroys every adapter on application teardown, while the application context still
    /// exists (adapters may hold entity handles). Without a window, images are released from
    /// every window's cache.
    pub fn destroy_all(&mut self, mut window: Option<&mut Window>, cx: &mut App) {
        let mut ids: Vec<HostId> = self.entries.keys().copied().collect();
        ids.sort();
        for id in ids {
            self.destroy_entry(id, window.as_deref_mut(), cx);
        }
    }

    /// Destroys every adapter of the shared `registry` when the application quits, while the
    /// application context still exists (R3). Keep the returned subscription alive for the
    /// lifetime of the application.
    pub fn destroy_on_quit(registry: &Rc<RefCell<Self>>, cx: &App) -> Subscription {
        let registry = registry.clone();
        cx.on_app_quit(move |cx| {
            registry.borrow_mut().destroy_all(None, cx);
            std::future::ready(())
        })
    }

    fn destroy_entry(&mut self, id: HostId, window: Option<&mut Window>, cx: &mut App) -> bool {
        let Some(mut entry) = self.entries.remove(&id) else {
            return false;
        };
        entry.adapter.destroy(cx);
        let mut images = Vec::new();
        images.extend(entry.adapter.live_image());
        while let Some(replaced) = entry.adapter.take_replaced_image() {
            images.push(replaced);
        }
        entry.bounds.clear();
        drop(entry);
        match window {
            Some(window) => {
                for image in images {
                    release_image(image, window, cx);
                }
            }
            None => {
                for image in images {
                    cx.drop_image(image, None);
                }
            }
        }
        true
    }
}

/// The host type used for images by [`CustomElementRegistry::set_live_image`].
pub const IMAGE_TYPE: &str = "img";

fn release_image(image: Arc<RenderImage>, window: &mut Window, cx: &mut App) {
    cx.drop_image(image, Some(window));
}

/// Property synchronization (R5).
fn sync_properties(entry: &mut Entry, properties: &Map<String, Value>) {
    let supported = entry.adapter.supported_properties();
    let current = |name: &str| properties.get(name).filter(|value| !value.is_null());

    for name in supported {
        match (current(name), entry.applied.get(*name)) {
            (Some(value), Some(last)) if json_equal(value, last) => {}
            (Some(value), _) => {
                entry.adapter.set_property(name, value);
                entry.applied.insert((*name).to_owned(), value.clone());
            }
            (None, Some(_)) => {
                entry.adapter.set_property(name, &Value::Null);
                entry.applied.remove(*name);
            }
            (None, None) => {}
        }
    }

    for (name, value) in properties {
        if value.is_null() || supported.contains(&name.as_str()) {
            continue;
        }
        match entry.applied.get(name) {
            Some(last) if json_equal(value, last) => {}
            _ => {
                entry.adapter.set_property(name, value);
                entry.applied.insert(name.clone(), value.clone());
            }
        }
    }

    let removed: Vec<String> = entry
        .applied
        .keys()
        .filter(|name| !supported.contains(&name.as_str()) && current(name).is_none())
        .cloned()
        .collect();
    for name in removed {
        entry.adapter.set_property(&name, &Value::Null);
        entry.applied.remove(&name);
    }
}

/// Deep JSON equality where numbers compare by value (`1` equals `1.0`) and objects ignore key
/// order.
pub fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => a == b,
            _ => match (a.as_u64(), b.as_u64()) {
                (Some(a), Some(b)) => a == b,
                _ => a.as_f64() == b.as_f64(),
            },
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| json_equal(a, b)))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests;
