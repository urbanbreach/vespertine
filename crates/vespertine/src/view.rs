use crate::key_dispatch::DispatchNodeId;
use crate::window::read_tracker::ViewKey;
use crate::window::retained::{AmbientPhase, SpliceMap};
use crate::window::retained::{RetainedViews, ViewKind};
use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, AvailableSpace, Bounds, Context, Element, ElementId,
    Entity, EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintIndex,
    Pixels, PrepaintStateIndex, Render, RenderOnce, Size, Style, StyleRefinement, WeakEntity,
};
use crate::{Empty, Window};
use anyhow::Result;
use std::mem;
use std::ops::Range;
use std::{
    any::{TypeId, type_name},
    fmt,
};
use vespertine_refineable::Refineable;

/// A dynamically-typed view handle that can be downcast to a specific `Entity<V>`.
///
/// This is the type-erased counterpart to [`ViewElement`]: it holds an entity plus
/// a function pointer to its render, and is itself a [`View`], so embedding it as an
/// element goes through the same [`ViewElement`] machinery as any other view.
#[derive(Clone, Debug)]
pub struct AnyView {
    entity: AnyEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    view_type_name: &'static str,
}

impl<V: Render> From<Entity<V>> for AnyView {
    fn from(value: Entity<V>) -> Self {
        AnyView {
            entity: value.into_any(),
            render: any_view::render::<V>,
            view_type_name: type_name::<V>(),
        }
    }
}

impl AnyView {
    /// Embed this view as a cached [`ViewElement`] laid out at `style`.
    ///
    /// The rendered subtree is recycled from the previous frame unless
    /// [Context::notify] was called on the backing entity since it was rendered
    /// (or [Window::refresh] is called, which ignores caching).
    pub fn cached(self, style: StyleRefinement) -> ViewElement<AnyView> {
        self.into_element().cached(style)
    }

    /// Convert this to a weak handle.
    pub fn downgrade(&self) -> AnyWeakView {
        AnyWeakView {
            entity: self.entity.downgrade(),
            render: self.render,
            view_type_name: self.view_type_name,
        }
    }

    /// Convert this to a [Entity] of a specific type.
    /// If this handle does not contain a view of the specified type, returns itself in an `Err` variant.
    pub fn downcast<T: 'static>(self) -> Result<Entity<T>, Self> {
        match self.entity.downcast() {
            Ok(entity) => Ok(entity),
            Err(entity) => Err(Self {
                entity,
                render: self.render,
                view_type_name: self.view_type_name,
            }),
        }
    }

    /// Gets the [TypeId] of the underlying view.
    pub fn entity_type(&self) -> TypeId {
        self.entity.entity_type
    }

    /// The [`EntityId`] of this view.
    pub fn entity_id(&self) -> EntityId {
        self.entity.entity_id()
    }
}

impl PartialEq for AnyView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl Eq for AnyView {}

/// `AnyView` is the type-erased [`View`]: its `render` is a function pointer rather
/// than a concrete type, but it participates in the reactive graph exactly like any
/// other view via [`ViewElement`].
impl View for AnyView {
    fn entity_id(&self) -> Option<EntityId> {
        Some(self.entity.entity_id())
    }

    fn erased(&self) -> Option<AnyView> {
        Some(self.clone())
    }

    #[cfg(not(vespertine_hot_reload))]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        (self.render)(&self, window, cx)
    }

    // The stored function pointer was captured from the original binary, so it must be looked up
    // in the jump table to reach the patched render.
    #[cfg(vespertine_hot_reload)]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        subsecond::HotFn::current(self.render).call((&self, window, cx))
    }
}

impl<V: 'static + Render> IntoElement for Entity<V> {
    type Element = ViewElement<Entity<V>>;

    fn into_element(self) -> Self::Element {
        ViewElement::with_type_name(self, type_name::<V>())
    }

    #[inline(never)]
    fn into_any_element(self) -> AnyElement {
        self.into_element().into_any()
    }
}

impl IntoElement for AnyView {
    type Element = ViewElement<AnyView>;

    fn into_element(self) -> Self::Element {
        let view_type_name = self.view_type_name;
        ViewElement::with_type_name(self, view_type_name)
    }
}

/// A weak, dynamically-typed view handle.
pub struct AnyWeakView {
    entity: AnyWeakEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    view_type_name: &'static str,
}

impl AnyWeakView {
    /// Upgrade to a strong `AnyView` handle, if the view is still alive.
    pub fn upgrade(&self) -> Option<AnyView> {
        let entity = self.entity.upgrade()?;
        Some(AnyView {
            entity,
            render: self.render,
            view_type_name: self.view_type_name,
        })
    }
}

impl<V: 'static + Render> From<WeakEntity<V>> for AnyWeakView {
    fn from(view: WeakEntity<V>) -> Self {
        AnyWeakView {
            entity: view.into(),
            render: any_view::render::<V>,
            view_type_name: type_name::<V>(),
        }
    }
}

impl PartialEq for AnyWeakView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl std::fmt::Debug for AnyWeakView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyWeakView")
            .field("entity_id", &self.entity.entity_id)
            .finish_non_exhaustive()
    }
}

mod any_view {
    use crate::{AnyElement, AnyView, App, IntoElement, Render, Window};

    pub(crate) fn render<V: 'static + Render>(
        view: &AnyView,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let view = view.clone().downcast::<V>().unwrap();
        // Record the view's Render type name so the accessibility debug dump can
        // attribute nodes to the view that produced them.
        #[cfg(debug_assertions)]
        window
            .a11y
            .view_type_names
            .insert(view.entity_id(), std::any::type_name::<V>());
        view.update(cx, |view, cx| view.render(window, cx).into_any_element())
    }
}

/// A renderable that participates in VESPERTINE's reactive graph — the unifying model
/// behind [`Render`] and [`RenderOnce`].
///
/// When `entity_id()` returns `Some`, that id becomes the view's identity: it gets
/// a unique element-id space (so internal `use_state` / `.id(..)` never collide
/// across siblings) and `cx.notify()` on that entity re-renders only this view's
/// subtree. `None` behaves like a stateless component.
///
/// You rarely implement `View` directly. `Entity<T: Render>` and any `T: RenderOnce`
/// get a blanket impl below; implement it by hand only when a component needs both
/// parent-supplied props *and* a backing entity for identity.
pub trait View: 'static + Sized {
    /// This view's identity, if it has one. A view typically holds the backing
    /// entity as a field and returns its [`EntityId`] here.
    ///
    /// The id becomes this view's [`ElementId`], so two views keyed on the same
    /// entity must not be rendered at the same position in the element tree
    /// (e.g. as siblings under the same parent): their internal element state
    /// (`use_state`, scroll offsets, etc.) would silently collide. Nesting is
    /// fine — the id is scoped by the parent path.
    fn entity_id(&self) -> Option<EntityId>;

    /// A type-erased handle with which the framework can render this view again
    /// without the element that embedded it (for views that are entity-backed).
    fn erased(&self) -> Option<AnyView> {
        None
    }

    /// Render this view into an element tree, consuming `self`.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement;
}

/// A stateless component (`RenderOnce`) is a `View` with no identity.
impl<T: RenderOnce> View for T {
    fn entity_id(&self) -> Option<EntityId> {
        None
    }

    #[inline]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        RenderOnce::render(self, window, cx)
    }
}

/// An entity that renders itself (`Render`) is a `View` keyed on its own id.
impl<T: Render> View for Entity<T> {
    fn entity_id(&self) -> Option<EntityId> {
        Some(Entity::entity_id(self))
    }

    fn erased(&self) -> Option<AnyView> {
        Some(AnyView::from(self.clone()))
    }

    #[inline]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.update(cx, |this, cx| {
            Render::render(this, window, cx).into_any_element()
        })
    }
}

impl<T: Render> Entity<T> {
    /// Embed this entity as a cached [`ViewElement`] laid out at `style`.
    ///
    /// The rendered subtree is reused until the entity is notified (or the
    /// cached bounds / text style change). Caching requires a definite size:
    /// a cached view is laid out from `style` and is *not* measured from its
    /// contents. Use [`ViewElement::new`] (or `.child(entity)`) for the
    /// uncached case.
    #[track_caller]
    pub fn cached(self, style: StyleRefinement) -> ViewElement<Entity<T>> {
        ViewElement::with_type_name(self, type_name::<T>()).cached(style)
    }
}

/// The element type for [`View`] implementations. Wraps a `View` and hooks it
/// into layout, prepaint, and paint. Constructed via [`ViewElement::new`].
#[doc(hidden)]
pub struct ViewElement<V: View> {
    view: Option<V>,
    entity_id: Option<EntityId>,
    cached_style: Option<StyleRefinement>,
    #[cfg(feature = "profiler")]
    view_type_name: &'static str,
    #[cfg(debug_assertions)]
    source: &'static core::panic::Location<'static>,
}

impl<V: View> ViewElement<V> {
    /// Wrap a [`View`] as an element.
    #[track_caller]
    pub fn new(view: V) -> Self {
        Self::with_type_name(view, type_name::<V>())
    }

    /// Wraps a view whose draw time profiling attributes to `view_type_name`,
    /// such as an entity's type rather than [`Entity`]'s.
    #[track_caller]
    #[cfg_attr(not(feature = "profiler"), expect(unused_variables))]
    pub(crate) fn with_type_name(view: V, view_type_name: &'static str) -> Self {
        let entity_id = view.entity_id();
        ViewElement {
            entity_id,
            cached_style: None,
            #[cfg(feature = "profiler")]
            view_type_name,
            view: Some(view),
            #[cfg(debug_assertions)]
            source: core::panic::Location::caller(),
        }
    }

    /// Enable caching of this view's rendered subtree, laid out at `style`.
    /// The composer supplies the layout style because caching skips rendering
    /// the contents to measure them.
    ///
    /// Crate-private on purpose: caching is only sound for entity-backed views,
    /// where [`Context::notify`] is the contract that busts the cache. A stateless
    /// view has no such contract, so a frozen subtree could never be invalidated.
    /// Reach this through [`Entity::cached`] or [`AnyView::cached`], which are
    /// entity-backed by construction.
    pub(crate) fn cached(mut self, style: StyleRefinement) -> Self {
        self.cached_style = Some(style);
        self
    }
}

impl<V: View> IntoElement for ViewElement<V> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Opaque, framework-owned per-frame state of a view element (PRF-001).
///
/// It carries either the element built for this frame or the identity of the
/// retained record whose previous output may be replayed.
#[doc(hidden)]
pub struct ViewState(ViewStateInner);

enum ViewStateInner {
    /// A stateless component, or a view drawn without retention: the element
    /// of this frame, if it was already built.
    Plain(Option<AnyElement>),
    /// A tracked view. `element` is `Some` when the view was built in this
    /// frame and `None` when the previous output may be replayed.
    Tracked {
        key: ViewKey,
        element: Option<AnyElement>,
        /// Nested views rebuilt in this frame inside a clean view that is
        /// replayed around them (splice).
        splice: Vec<SplicedChild>,
    },
}

/// A view nested in a replayed view that was built again in this frame.
struct SplicedChild {
    key: ViewKey,
    layout_id: LayoutId,
    element: Option<AnyElement>,
    wrapper_node: Option<DispatchNodeId>,
    state: Option<ViewState>,
}

#[cfg(not(vespertine_hot_reload))]
#[inline]
fn render_view<V: View>(view: V, window: &mut Window, cx: &mut App) -> AnyElement {
    view.render(window, cx).into_any_element()
}

/// Every view and component render passes through this hot-patchable boundary, so the
/// patched code for that render and everything it calls runs on the next frame.
#[cfg(vespertine_hot_reload)]
fn render_view<V: View>(view: V, window: &mut Window, cx: &mut App) -> AnyElement {
    #[inline(never)]
    fn render_view_impl<V: View>(view: V, window: &mut Window, cx: &mut App) -> AnyElement {
        view.render(window, cx).into_any_element()
    }

    subsecond::HotFn::current(render_view_impl::<V>).call((view, window, cx))
}

impl<V: View> Element for ViewElement<V> {
    type RequestLayoutState = ViewState;
    type PrepaintState = ViewState;

    fn id(&self) -> Option<ElementId> {
        self.entity_id.map(ElementId::View)
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        #[cfg(debug_assertions)]
        return Some(self.source);

        #[cfg(not(debug_assertions))]
        return None;
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path: create a reactive boundary.
            #[cfg(feature = "profiler")]
            let start = window.window_profiler.begin_view();
            let erased = self.view.as_ref().and_then(View::erased);
            let view = &mut self.view;
            let layout = request_layout_view(
                entity_id,
                erased,
                id.expect("views have element ids"),
                self.cached_style.as_ref(),
                window,
                cx,
                &mut |window, cx| render_view(view.take().unwrap(), window, cx),
            );
            #[cfg(feature = "profiler")]
            window.window_profiler.end_view(
                start,
                self.view_type_name,
                crate::profiler::ViewPhase::RequestLayout,
            );
            layout
        } else {
            // Stateless path: isolate subtree via type name (no entity identity).
            let (layout_id, element) =
                request_layout_component(type_name::<V>(), window, cx, &mut |window, cx| {
                    render_view(self.view.take().unwrap(), window, cx)
                });
            (layout_id, ViewState(ViewStateInner::Plain(element)))
        }
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> ViewState {
        if let Some(entity_id) = self.entity_id {
            // Stateful path.
            #[cfg(feature = "profiler")]
            let start = window.window_profiler.begin_view();
            let prepainted = prepaint_view(
                entity_id,
                global_id.expect("views have element ids"),
                bounds,
                state,
                window,
                cx,
                &mut |window, cx| render_view(self.view.take().unwrap(), window, cx),
            );
            #[cfg(feature = "profiler")]
            window.window_profiler.end_view(
                start,
                self.view_type_name,
                crate::profiler::ViewPhase::Prepaint,
            );
            prepainted
        } else {
            // Stateless path: just prepaint the element.
            prepaint_component(type_name::<V>(), state, window, cx)
        }
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path.
            #[cfg(feature = "profiler")]
            let start = window.window_profiler.begin_view();
            paint_view(entity_id, state, window, cx);
            #[cfg(feature = "profiler")]
            window.window_profiler.end_view(
                start,
                self.view_type_name,
                crate::profiler::ViewPhase::Paint,
            );
        } else {
            // Stateless path: just paint the element.
            paint_component(std::any::type_name::<V>(), state, window, cx);
        }
    }
}

/// A view that renders nothing
pub struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// How much of a view's previous output a frame may keep.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Everything is replayed.
    Clean,
    /// The view itself is clean but views nested in it are not: it is replayed
    /// around freshly built children.
    Splice,
    /// The view is built again.
    Rebuild,
}

fn verdict(window: &Window, key: ViewKey, kind: ViewKind) -> Verdict {
    let Some(record) = window.retention.views.get(key) else {
        return Verdict::Rebuild;
    };
    if record.dirty || record.prepaint.is_none() || record.paint.is_none() {
        return Verdict::Rebuild;
    }
    // Ordinary descendants of a notified ancestor re-evaluate updates that were
    // never notified; explicitly cached views keep the upstream rule.
    if kind == ViewKind::Retained
        && window.retention.notified_ancestors > 0
        && RetainedViews::reads_stale(&window.retention.tracker, record)
    {
        return Verdict::Rebuild;
    }
    if record.dirty_descendants {
        // Explicitly cached views are rebuilt when anything inside them changed.
        if kind == ViewKind::Retained {
            Verdict::Splice
        } else {
            Verdict::Rebuild
        }
    } else {
        Verdict::Clean
    }
}

#[inline(never)]
fn request_layout_view(
    entity_id: EntityId,
    erased: Option<AnyView>,
    path: &GlobalElementId,
    cached_style: Option<&StyleRefinement>,
    window: &mut Window,
    cx: &mut App,
    render: &mut dyn FnMut(&mut Window, &mut App) -> AnyElement,
) -> (LayoutId, ViewState) {
    window.with_rendered_view(entity_id, |window| {
        let picking = window.is_inspector_picking(cx);
        let tracking = window.retention_tracking();
        match cached_style {
            Some(style) if !picking => {
                let mut root_style = Style::default();
                root_style.refine(style);
                let layout_id = window.request_layout(root_style, None, cx);
                let state = if tracking {
                    let key = tracked_record(window, entity_id, path, ViewKind::Cached);
                    ViewStateInner::Tracked {
                        key,
                        element: None,
                        splice: Vec::new(),
                    }
                } else {
                    ViewStateInner::Plain(None)
                };
                (layout_id, ViewState(state))
            }
            _ if !tracking => {
                let mut element = render(window, cx);
                window.retention.views.stats.views_rendered += 1;
                let layout_id = element.request_layout(window, cx);
                (layout_id, ViewState(ViewStateInner::Plain(Some(element))))
            }
            _ => {
                let key = tracked_record(window, entity_id, path, ViewKind::Retained);
                if window.retention_reuse_allowed()
                    && let Some(layout_id) = window
                        .retention
                        .views
                        .get(key)
                        .and_then(|record| record.layout_root)
                {
                    if let Some((layout_id, element)) = window.retention.adopted.remove(&key) {
                        // Built already for a splice that had to be abandoned.
                        let parent = window.retention.stack.last().copied();
                        window.retention.views.adopt(key, parent);
                        return (
                            layout_id,
                            ViewState(ViewStateInner::Tracked {
                                key,
                                element: Some(element),
                                splice: Vec::new(),
                            }),
                        );
                    }
                    match verdict(window, key, ViewKind::Retained) {
                        Verdict::Clean => {
                            let parent = window.retention.stack.last().copied();
                            window.retention.views.adopt(key, parent);
                            window.touch_layout_subtree(key);
                            return (
                                layout_id,
                                ViewState(ViewStateInner::Tracked {
                                    key,
                                    element: None,
                                    splice: Vec::new(),
                                }),
                            );
                        }
                        Verdict::Splice => {
                            if let Some(splice) = splice_layout(window, cx, key) {
                                let parent = window.retention.stack.last().copied();
                                window.retention.views.adopt(key, parent);
                                window.touch_layout_subtree(key);
                                return (
                                    layout_id,
                                    ViewState(ViewStateInner::Tracked {
                                        key,
                                        element: None,
                                        splice,
                                    }),
                                );
                            }
                        }
                        Verdict::Rebuild => {}
                    }
                } else if let Some((layout_id, element)) = window.retention.adopted.remove(&key) {
                    let parent = window.retention.stack.last().copied();
                    window.retention.views.adopt(key, parent);
                    return (
                        layout_id,
                        ViewState(ViewStateInner::Tracked {
                            key,
                            element: Some(element),
                            splice: Vec::new(),
                        }),
                    );
                }
                let parent = window.retention.stack.last().copied();
                let (layout_id, element) = build_view(window, cx, key, parent, erased, render);
                (
                    layout_id,
                    ViewState(ViewStateInner::Tracked {
                        key,
                        element: Some(element),
                        splice: Vec::new(),
                    }),
                )
            }
        }
    })
}

/// Finds or creates the record of the view at `path`.
fn tracked_record(
    window: &mut Window,
    entity_id: EntityId,
    path: &GlobalElementId,
    kind: ViewKind,
) -> ViewKey {
    match window.retention.views.lookup(path) {
        Some(key) => key,
        None => window.retention.views.create(path.clone(), entity_id, kind),
    }
}

/// Renders the view and requests layout for it as part of the enclosing
/// layout; returns the layout root and the element.
fn build_view(
    window: &mut Window,
    cx: &mut App,
    key: ViewKey,
    parent: Option<ViewKey>,
    erased: Option<AnyView>,
    render: &mut dyn FnMut(&mut Window, &mut App) -> AnyElement,
) -> (LayoutId, AnyElement) {
    window.retention.views.begin_rebuild(key, parent);
    window.retention.views.reset_reads(key);
    let layout_key = window.layout_key_context();
    let mut ambient = window.capture_ambient();
    // The prepaint-phase context of an earlier build stays valid until the view is
    // prepainted again; a view built out of band for a splice is prepainted in it.
    ambient.prepaint = window
        .retention
        .views
        .get(key)
        .and_then(|record| record.ambient.as_ref())
        .and_then(|ambient| ambient.prepaint.clone());
    window.begin_view_phase(key);
    window.begin_layout_owner();
    window.retention.layout_state_sink.push(Vec::new());
    let mut element = render(window, cx);
    window.retention.views.stats.views_rendered += 1;
    let layout_id = element.request_layout(window, cx);
    let layout_states = window.retention.layout_state_sink.pop().unwrap_or_default();
    let owned = window.end_layout_owner();
    window.end_view_phase(key);
    if let Some(record) = window.retention.views.get_mut(key) {
        record.layout = owned;
        record.layout_states = layout_states;
        record.layout_root = Some(layout_id);
        record.layout_key = Some(layout_key);
        if erased.is_some() {
            record.view = erased;
        }
        record.ambient = Some(std::rc::Rc::new(ambient));
    }
    (layout_id, element)
}

/// Builds the views nested in the clean view `key` that are dirty, out of band,
/// so that the view can be replayed around them. Returns `None` (after handing
/// the built children over for adoption) if building them changed layout.
fn splice_layout(window: &mut Window, cx: &mut App, key: ViewKey) -> Option<Vec<SplicedChild>> {
    // A view nested here was built for a splice that had to be abandoned because it changed
    // layout: this view is affected as well and builds again to adopt it.
    if window
        .retention
        .adopted
        .keys()
        .any(|adopted| window.retention.views.is_nested_in(*adopted, key))
    {
        return None;
    }
    let frontier = window.retention.views.dirty_frontier(key)?;
    if frontier.is_empty() {
        window.retention.views.clear_dirty_descendants(key);
        return Some(Vec::new());
    }
    let writes_before = window.layout_write_count();
    let mut built = Vec::with_capacity(frontier.len());
    for child in frontier {
        let record = window.retention.views.get(child)?;
        let (ambient, view, layout_key, parent) = (
            record.ambient.clone()?,
            record.view.clone()?,
            record.layout_key?,
            record.parent,
        );
        let (layout_id, element) = window.with_ambient(&ambient, AmbientPhase::Layout, |window| {
            window.with_layout_key_context(layout_key, |window| {
                build_view(
                    window,
                    cx,
                    child,
                    parent,
                    Some(view.clone()),
                    &mut |window, cx| render_view(view.clone(), window, cx),
                )
            })
        });
        built.push(SplicedChild {
            key: child,
            layout_id,
            element: Some(element),
            wrapper_node: None,
            state: None,
        });
    }
    if window.layout_write_count() != writes_before {
        // A child changed what its parent's layout depends on: the parent is
        // built again and adopts the children built here.
        window.retention.views.stats.splice_failures += 1;
        for child in built {
            if let Some(element) = child.element {
                window
                    .retention
                    .adopted
                    .insert(child.key, (child.layout_id, element));
            }
        }
        return None;
    }
    window.retention.views.clear_dirty_descendants(key);
    Some(built)
}

#[inline(never)]
fn request_layout_component(
    name: &'static str,
    window: &mut Window,
    cx: &mut App,
    render: &mut dyn FnMut(&mut Window, &mut App) -> AnyElement,
) -> (LayoutId, Option<AnyElement>) {
    window.with_id(ElementId::from(name), |window| {
        let mut element = render(window, cx);
        let layout_id = element.request_layout(window, cx);
        (layout_id, Some(element))
    })
}

#[inline(never)]
fn prepaint_view(
    entity_id: EntityId,
    path: &GlobalElementId,
    bounds: Bounds<Pixels>,
    state: &mut ViewState,
    window: &mut Window,
    cx: &mut App,
    render: &mut dyn FnMut(&mut Window, &mut App) -> AnyElement,
) -> ViewState {
    window.set_view_id(entity_id);
    window.with_rendered_view(entity_id, |window| {
        match mem::replace(&mut state.0, ViewStateInner::Plain(None)) {
            ViewStateInner::Plain(Some(mut element)) => {
                element.prepaint(window, cx);
                ViewState(ViewStateInner::Plain(Some(element)))
            }
            ViewStateInner::Plain(None) => {
                // A cached view drawn without retention: build it here.
                let mut element = render(window, cx);
                window.retention.views.stats.views_rendered += 1;
                element.layout_as_root(Size::<AvailableSpace>::from(bounds.size), window, cx);
                element.prepaint_at(bounds.origin, window, cx);
                ViewState(ViewStateInner::Plain(Some(element)))
            }
            ViewStateInner::Tracked {
                key,
                element: Some(mut element),
                ..
            } => {
                let prepaint_ambient = window.capture_prepaint_ambient();
                window.begin_view_phase(key);
                let start = window.prepaint_index();
                element.prepaint(window, cx);
                let end = window.prepaint_index();
                window.end_view_phase(key);
                let cache_key = window.ambient_cache_key(bounds);
                if let Some(record) = window.retention.views.get_mut(key) {
                    record.prepaint = Some(start..end);
                    record.cache_key = Some(cache_key);
                    if let Some(ambient) = record.ambient.as_mut() {
                        std::rc::Rc::make_mut(ambient).prepaint = Some(prepaint_ambient);
                    }
                }
                ViewState(ViewStateInner::Tracked {
                    key,
                    element: Some(element),
                    splice: Vec::new(),
                })
            }
            ViewStateInner::Tracked {
                key,
                element: None,
                mut splice,
            } => {
                let kind = window
                    .retention
                    .views
                    .get(key)
                    .map_or(ViewKind::Retained, |record| record.kind);
                let cache_key = window.ambient_cache_key(bounds);
                let ambient_matches = window.retention_reuse_allowed()
                    && window
                        .retention
                        .views
                        .get(key)
                        .is_some_and(|record| record.cache_key.as_ref() == Some(&cache_key));
                if ambient_matches
                    && splice.is_empty()
                    && verdict(window, key, kind) == Verdict::Clean
                {
                    let old = window
                        .retention
                        .views
                        .get(key)
                        .and_then(|record| record.prepaint.clone())
                        .expect("replayable views have prepaint output");
                    let parent = window.retention.stack.last().copied();
                    window.retention.views.adopt(key, parent);
                    let start = window.prepaint_index();
                    window.reuse_prepaint(old.clone());
                    let end = window.prepaint_index();
                    window.rebase_prepaint_subtree(key, old, start..end);
                    if kind == ViewKind::Cached {
                        window.touch_layout_subtree(key);
                    }
                    window.retention.views.stats.views_reused += 1;
                    return ViewState(ViewStateInner::Tracked {
                        key,
                        element: None,
                        splice: Vec::new(),
                    });
                }
                if ambient_matches && !splice.is_empty() {
                    let spliceable = window
                        .retention
                        .views
                        .get(key)
                        .is_some_and(|record| !record.dirty && record.prepaint.is_some());
                    if spliceable {
                        prepaint_spliced(window, cx, key, &mut splice);
                        window.retention.views.stats.splice_successes += 1;
                        window.retention.views.stats.views_reused += 1;
                        return ViewState(ViewStateInner::Tracked {
                            key,
                            element: None,
                            splice,
                        });
                    }
                }
                if !splice.is_empty() {
                    // The clean view cannot be replayed after all (it moved or its
                    // inputs changed): it is built again below, and adopts the
                    // children that were built for the splice instead of rendering
                    // them a second time.
                    window.retention.views.stats.splice_failures += 1;
                    for child in splice {
                        if let Some(element) = child.element {
                            window
                                .retention
                                .adopted
                                .insert(child.key, (child.layout_id, element));
                        }
                    }
                }

                // The view moved, changed size or became invalid after its layout
                // was requested: build it again here.
                let parent = window.retention.stack.last().copied();
                window.retention.views.begin_rebuild(key, parent);
                window.retention.views.reset_reads(key);
                let layout_key = window
                    .retention
                    .views
                    .get(key)
                    .and_then(|record| record.layout_key)
                    .filter(|_| kind == ViewKind::Retained);
                let prepaint_ambient = window.capture_prepaint_ambient();
                window.begin_view_phase(key);
                window.begin_layout_owner();
                let start = window.prepaint_index();
                let mut element = render(window, cx);
                window.retention.views.stats.views_rendered += 1;
                if let Some(layout_key) = layout_key {
                    // Request layout again in place: the view claims the very nodes the
                    // enclosing layout already computed, so nothing is laid out again
                    // and the view is prepainted where its layout node sits.
                    let writes = window.layout_write_count();
                    let layout_id = window.with_layout_key_context(layout_key, |window| {
                        element.request_layout(window, cx)
                    });
                    if window.layout_write_count() != writes {
                        // Something the layout depends on changed (for example a
                        // measured leaf that could not keep its measurement).
                        window.relayout_subtree_in_place(layout_id, bounds.size, cx);
                    }
                    element.prepaint(window, cx);
                } else {
                    element.layout_as_root(Size::<AvailableSpace>::from(bounds.size), window, cx);
                    element.prepaint_at(bounds.origin, window, cx);
                }
                let end = window.prepaint_index();
                let owned = window.end_layout_owner();
                window.end_view_phase(key);
                if let Some(record) = window.retention.views.get_mut(key) {
                    if layout_key.is_some() {
                        record.layout = owned;
                    } else {
                        record.layout.extend(&owned);
                    }
                    record.prepaint = Some(start..end);
                    record.cache_key = Some(cache_key);
                    if let Some(ambient) = record.ambient.as_mut() {
                        std::rc::Rc::make_mut(ambient).prepaint = Some(prepaint_ambient);
                    }
                }
                let _ = path;
                ViewState(ViewStateInner::Tracked {
                    key,
                    element: Some(element),
                    splice: Vec::new(),
                })
            }
        }
    })
}

/// Replays the prepaint output of the clean view `key` in stretches around the
/// views nested in it that were built again, and prepaints those in place.
fn prepaint_spliced(window: &mut Window, cx: &mut App, key: ViewKey, splice: &mut [SplicedChild]) {
    let old = window
        .retention
        .views
        .get(key)
        .and_then(|record| record.prepaint.clone())
        .expect("spliced views have prepaint output");
    // Gaps in the order their output appears.
    splice.sort_by_key(|child| {
        window
            .retention
            .views
            .get(child.key)
            .and_then(|record| record.prepaint.as_ref())
            .map_or(0, |range| range.start.order_key())
    });
    let gaps: Vec<Range<PrepaintStateIndex>> = splice
        .iter()
        .map(|child| {
            window
                .retention
                .views
                .get(child.key)
                .and_then(|record| record.prepaint.clone())
                .expect("rebuilt children have prepaint output")
        })
        .collect();
    let parent = window.retention.stack.last().copied();
    window.retention.views.adopt(key, parent);

    let attachment = window.next_frame.dispatch_tree.active_node_id();
    let whole_start = window.prepaint_index();
    let mut map = SpliceMap::default();
    let mut stretches: Vec<(Range<PrepaintStateIndex>, PrepaintStateIndex)> = Vec::new();
    let mut cursor = old.start.clone();
    for ix in 0..=splice.len() {
        let stretch_end = gaps
            .get(ix)
            .map_or_else(|| old.end.clone(), |gap| gap.start.clone());
        let stretch = cursor.clone()..stretch_end;
        let new_start = window.prepaint_index();
        window.reuse_prepaint_mapped(stretch.clone(), Some(&map));
        map.push(
            stretch.start.dispatch_tree_index()..stretch.end.dispatch_tree_index(),
            new_start.dispatch_tree_index(),
        );
        stretches.push((stretch, new_start));
        let Some(child) = splice.get_mut(ix) else {
            break;
        };
        cursor = gaps[ix].end.clone();

        let record = window
            .retention
            .views
            .get(child.key)
            .expect("rebuilt children have records");
        let (ambient, path, entity_id, view) = (
            record
                .ambient
                .clone()
                .expect("rebuilt children have ambient state"),
            record.path.clone(),
            record.entity_id,
            record
                .view
                .clone()
                .expect("rebuilt children have view handles"),
        );
        let old_parent = ambient
            .prepaint
            .as_ref()
            .and_then(|prepaint| prepaint.dispatch_parent)
            .map(DispatchNodeId::from_index);
        let new_parent = old_parent.and_then(|old| map.resolve(old)).or(attachment);
        let layout_id = child.layout_id;
        let element = child.element.take();
        let (node, state) = window.with_ambient(&ambient, AmbientPhase::Prepaint, |window| {
            if let Some(new_parent) = new_parent {
                window.next_frame.dispatch_tree.set_active_node(new_parent);
            }
            let bounds = window.layout_bounds(layout_id);
            let node = window.next_frame.dispatch_tree.push_node();
            let mut state = ViewState(ViewStateInner::Tracked {
                key: child.key,
                element,
                splice: Vec::new(),
            });
            let state = prepaint_view(
                entity_id,
                &path,
                bounds,
                &mut state,
                window,
                cx,
                &mut |window, cx| render_view(view.clone(), window, cx),
            );
            window.next_frame.dispatch_tree.pop_node();
            (node, state)
        });
        if let Some(attachment) = attachment {
            window.next_frame.dispatch_tree.set_active_node(attachment);
        }
        child.wrapper_node = Some(node);
        child.state = Some(state);
    }
    let whole_end = window.prepaint_index();
    let rebuilt: Vec<ViewKey> = splice.iter().map(|child| child.key).collect();
    window.rebase_spliced_prepaint(key, whole_start..whole_end, &stretches, &rebuilt);
}

#[inline(never)]
fn prepaint_component(
    name: &'static str,
    state: &mut ViewState,
    window: &mut Window,
    cx: &mut App,
) -> ViewState {
    let ViewStateInner::Plain(element) = mem::replace(&mut state.0, ViewStateInner::Plain(None))
    else {
        unreachable!("components are never tracked");
    };
    let mut element = element.unwrap();
    window.with_id(ElementId::from(name), |window| {
        element.prepaint(window, cx);
    });
    ViewState(ViewStateInner::Plain(Some(element)))
}

#[inline(never)]
fn paint_view(entity_id: EntityId, state: &mut ViewState, window: &mut Window, cx: &mut App) {
    window.with_rendered_view(entity_id, |window| match &mut state.0 {
        ViewStateInner::Plain(element) => element.as_mut().unwrap().paint(window, cx),
        ViewStateInner::Tracked {
            key,
            element,
            splice,
        } => {
            let key = *key;
            let start = window.paint_index();
            if let Some(element) = element {
                window.begin_view_phase(key);
                element.paint(window, cx);
                window.end_view_phase(key);
            } else if !splice.is_empty() {
                paint_spliced(window, cx, key, splice);
                return;
            } else {
                let old = window
                    .retention
                    .views
                    .get(key)
                    .and_then(|record| record.paint.clone())
                    .expect("replayed views have paint output");
                window.reuse_paint(old.clone());
                let end = window.paint_index();
                window.rebase_paint_subtree(key, old, start..end);
                return;
            }
            let end = window.paint_index();
            if let Some(record) = window.retention.views.get_mut(key) {
                record.paint = Some(start..end);
            }
        }
    });
}

/// Replays the paint output of the clean view `key` in stretches around the
/// views nested in it that were built again, and paints those in place.
fn paint_spliced(window: &mut Window, cx: &mut App, key: ViewKey, splice: &mut [SplicedChild]) {
    let old = window
        .retention
        .views
        .get(key)
        .and_then(|record| record.paint.clone())
        .expect("spliced views have paint output");
    let gaps: Vec<Range<PaintIndex>> = splice
        .iter()
        .map(|child| {
            window
                .retention
                .views
                .get(child.key)
                .and_then(|record| record.paint.clone())
                .expect("rebuilt children have paint output")
        })
        .collect();
    let whole_start = window.paint_index();
    let mut stretches: Vec<(Range<PaintIndex>, PaintIndex)> = Vec::new();
    let mut cursor = old.start.clone();
    for ix in 0..=splice.len() {
        let stretch_end = gaps
            .get(ix)
            .map_or_else(|| old.end.clone(), |gap| gap.start.clone());
        let stretch = cursor.clone()..stretch_end;
        let new_start = window.paint_index();
        window.reuse_paint(stretch.clone());
        stretches.push((stretch, new_start));
        let Some(child) = splice.get_mut(ix) else {
            break;
        };
        cursor = gaps[ix].end.clone();
        let record = window
            .retention
            .views
            .get(child.key)
            .expect("rebuilt children have records");
        let ambient = record
            .ambient
            .clone()
            .expect("rebuilt children have ambient state");
        let entity_id = record.entity_id;
        let (Some(node), Some(mut child_state)) = (child.wrapper_node, child.state.take()) else {
            continue;
        };
        window.with_ambient(&ambient, AmbientPhase::Prepaint, |window| {
            window.next_frame.dispatch_tree.set_active_node(node);
            paint_view(entity_id, &mut child_state, window, cx);
        });
        child.state = Some(child_state);
    }
    let whole_end = window.paint_index();
    let rebuilt: Vec<ViewKey> = splice.iter().map(|child| child.key).collect();
    window.rebase_spliced_paint(key, whole_start..whole_end, &stretches, &rebuilt);
}

#[inline(never)]
fn paint_component(name: &'static str, state: &mut ViewState, window: &mut Window, cx: &mut App) {
    let ViewStateInner::Plain(Some(element)) = &mut state.0 else {
        unreachable!("components are always built");
    };
    window.with_id(ElementId::Name(name.into()), |window| {
        element.paint(window, cx);
    });
}
