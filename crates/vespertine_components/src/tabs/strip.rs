//! The [`TabStrip`] view (CMP-017 R1-R12, R15).

use super::chip::{TabSpec, tab_chip, tab_strip_region, tooltip};
use super::model::{
    CloseDecision, ClosePolicy, SlideSet, TAB_GAP, TAB_HEIGHT, TAB_SLOT, TAB_WIDTH, TabId,
    VisitHistory, active_after_close, cycle_target, drop_index, edge_hidden, reorder,
    reveal_offset,
};
use crate::nav_theme::NavTheme;
use std::{collections::HashMap, rc::Rc};
use vespertine::{
    App, AppContext, Bounds, ClickEvent, Context, DragMoveEvent, ElementId, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, MotionClock, ParentElement, Pixels, Point, Render,
    Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
    actions, div, point, prelude::FluentBuilder, px,
};

actions!(
    tabs,
    [
        /// Selects the next tab, wrapping around.
        NextTab,
        /// Selects the previous tab, wrapping around.
        PreviousTab,
    ]
);

/// The default Linux bindings of [`NextTab`] and [`PreviousTab`]: Ctrl+Tab and Ctrl+Shift+Tab.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
    ]);
}

/// Routes [`NextTab`] and [`PreviousTab`] reaching `element` to the strip. Owners wrap their
/// content region with this so cycling works while focus is inside a tab's content; the strip
/// element itself already handles them.
pub fn cycle_on<E: InteractiveElement>(element: E, strip: &Entity<TabStrip>) -> E {
    let next = strip.downgrade();
    let previous = strip.downgrade();
    element
        .on_action(move |_: &NextTab, window, cx| {
            next.update(cx, |strip, cx| strip.cycle(true, window, cx))
                .ok();
        })
        .on_action(move |_: &PreviousTab, window, cx| {
            previous
                .update(cx, |strip, cx| strip.cycle(false, window, cx))
                .ok();
        })
}

/// The payload of a tab drag: the strip key, the source index and the ghost title.
#[derive(Clone, Debug, PartialEq)]
pub struct TabDrag {
    /// The key of the strip the drag began in.
    pub strip: SharedString,
    /// The source index at the start of the drag.
    pub source: usize,
    /// The dragged tab.
    pub id: TabId,
    /// The title the ghost shows.
    pub title: SharedString,
}

struct TabGhost {
    title: SharedString,
    theme: NavTheme,
}

impl Render for TabGhost {
    fn render(
        &mut self,
        _window: &mut vespertine::Window,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .w(px(TAB_WIDTH))
            .h(px(TAB_HEIGHT))
            .px(px(8.))
            .rounded(px(6.))
            .bg(self.theme.popup_surface)
            .border_1()
            .border_color(self.theme.popup_border)
            .text_size(px(11.5))
            .text_color(self.theme.text)
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.title.clone())
    }
}

#[derive(Clone, Debug)]
struct DragState {
    source: usize,
    over: usize,
    slides: SlideSet,
}

enum Notice {
    Select(TabId),
    Close(TabId),
    Reorder(usize, usize),
    ContextMenu(TabId, Point<Pixels>),
    Add,
}

type TabCallback = Rc<dyn Fn(&TabId, &mut Window, &mut App)>;
type PrepareClose = Rc<dyn Fn(&TabId, &mut Window, &mut App) -> CloseDecision>;
type ReorderCallback = Rc<dyn Fn(usize, usize, &mut Window, &mut App)>;
type ContextCallback = Rc<dyn Fn(&TabId, Point<Pixels>, &mut Window, &mut App)>;
type AddCallback = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Default, Clone)]
struct Callbacks {
    select: Option<TabCallback>,
    close: Option<TabCallback>,
    prepare_close: Option<PrepareClose>,
    reorder: Option<ReorderCallback>,
    context_menu: Option<ContextCallback>,
    add: Option<AddCallback>,
}

/// A horizontally scrolling strip of closable, reorderable tabs with stable identities.
///
/// The strip owns the display order, the selection and the visit history; the owner owns the
/// content of every tab by identity and is told about changes through callbacks. Callbacks
/// other than `prepare_close` run right after the update that produced them (never while the
/// strip is borrowed), so they may call back into the strip. `prepare_close` runs inline and
/// must not update the strip.
pub struct TabStrip {
    key: SharedString,
    tabs: Vec<TabSpec>,
    selected: Option<TabId>,
    policy: ClosePolicy,
    history: VisitHistory,
    scroll: ScrollHandle,
    focus: HashMap<TabId, FocusHandle>,
    content_focus: HashMap<TabId, FocusHandle>,
    primary_focus: Option<FocusHandle>,
    pending_close: Option<TabId>,
    pending_reveal: Option<TabId>,
    overlay_active: bool,
    theme: Option<NavTheme>,
    callbacks: Callbacks,
    drag: Option<DragState>,
    outbox: Vec<Notice>,
    flush_scheduled: bool,
}

impl TabStrip {
    /// An empty strip. `key` scopes drag payloads and the visit history: strips with different
    /// keys never accept each other's tabs.
    pub fn new(key: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            tabs: Vec::new(),
            selected: None,
            policy: ClosePolicy::default(),
            history: VisitHistory::new(),
            scroll: ScrollHandle::new(),
            focus: HashMap::new(),
            content_focus: HashMap::new(),
            primary_focus: None,
            pending_close: None,
            pending_reveal: None,
            overlay_active: false,
            theme: None,
            callbacks: Callbacks::default(),
            drag: None,
            outbox: Vec::new(),
            flush_scheduled: false,
        }
    }

    /// The tabs, in display order.
    pub fn tabs(mut self, tabs: Vec<TabSpec>) -> Self {
        self.tabs = tabs;
        self
    }

    /// The selected tab (builder form of [`Self::select`], without a visit being revealed).
    pub fn selected(mut self, id: impl Into<Option<TabId>>) -> Self {
        self.selected = id.into();
        if let Some(id) = &self.selected {
            self.history.visit(&self.key, id);
        }
        self
    }

    /// Called with the id when the user (or cycling, or a close survivor) changes the selection.
    pub fn on_select(mut self, callback: impl Fn(&TabId, &mut Window, &mut App) + 'static) -> Self {
        self.callbacks.select = Some(Rc::new(callback));
        self
    }

    /// Called with the id after a tab was removed (close button, middle click, owner action).
    /// The owner drops the tab's content here.
    pub fn on_close(mut self, callback: impl Fn(&TabId, &mut Window, &mut App) + 'static) -> Self {
        self.callbacks.close = Some(Rc::new(callback));
        self
    }

    /// The owner's veto hook: asked before a tab is removed.
    pub fn prepare_close(
        mut self,
        callback: impl Fn(&TabId, &mut Window, &mut App) -> CloseDecision + 'static,
    ) -> Self {
        self.callbacks.prepare_close = Some(Rc::new(callback));
        self
    }

    /// Called with `(from, to)` after a drag moved a tab (never for a no-op).
    pub fn on_reorder(
        mut self,
        callback: impl Fn(usize, usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.callbacks.reorder = Some(Rc::new(callback));
        self
    }

    /// Called with the tab and the pointer position on a right click (or touch long press).
    pub fn on_context_menu(
        mut self,
        callback: impl Fn(&TabId, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.callbacks.context_menu = Some(Rc::new(callback));
        self
    }

    /// Shows the trailing add-tab button; `callback` runs when it is activated.
    pub fn on_add(mut self, callback: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.callbacks.add = Some(Rc::new(callback));
        self
    }

    /// The policy that chooses the surviving selection after the selected tab closes.
    pub fn close_policy(mut self, policy: ClosePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Uses `scroll` as the strip's scroll handle, so the owner can observe or drive it.
    pub fn scroll_handle(mut self, scroll: ScrollHandle) -> Self {
        self.scroll = scroll;
        self
    }

    /// Fixes the theme roles; by default they follow the active theme.
    pub fn theme(mut self, theme: NavTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// The strip's scroll handle.
    pub fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// The strip key.
    pub fn key(&self) -> &SharedString {
        &self.key
    }

    /// The tabs in display order.
    pub fn specs(&self) -> &[TabSpec] {
        &self.tabs
    }

    /// The selected tab, if any.
    pub fn selected_id(&self) -> Option<&TabId> {
        self.selected.as_ref()
    }

    /// The tab whose close the owner has not yet resolved (after a Pending or Blocked veto).
    pub fn pending_close(&self) -> Option<&TabId> {
        self.pending_close.as_ref()
    }

    /// The tab whose chip has keyboard focus, if any.
    pub fn focused_tab(&self, window: &Window) -> Option<&TabId> {
        self.tabs.iter().map(|tab| &tab.id).find(|id| {
            self.focus
                .get(*id)
                .is_some_and(|focus| focus.is_focused(window))
        })
    }

    /// The visit history store.
    pub fn visit_history(&self) -> &VisitHistory {
        &self.history
    }

    /// Whether a drag started in this strip is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The drop index of the drag in progress.
    pub fn drag_over(&self) -> Option<usize> {
        self.drag.as_ref().map(|drag| drag.over)
    }

    /// The restart counter of the drag's slide animation.
    pub fn drag_epoch(&self) -> Option<u64> {
        self.drag.as_ref().map(|drag| drag.slides.epoch())
    }

    /// The displacement, in pixels, of the tab at `ix` at `now_ms` during a drag.
    pub fn slide_px(&self, ix: usize, now_ms: f64) -> f32 {
        self.drag
            .as_ref()
            .map_or(0.0, |drag| drag.slides.value(ix, now_ms) * TAB_SLOT)
    }

    /// Registers the focus handle of the content shown for tab `id`. Focus inside it is "inside
    /// the strip's content region": closing the tab or cycling moves focus into the new
    /// selection's content.
    pub fn set_content_focus(&mut self, id: TabId, focus: FocusHandle) {
        self.content_focus.insert(id, focus);
    }

    /// The owner's primary region, focused when the last tab closes while focus was inside the
    /// closed tab's content.
    pub fn set_primary_focus(&mut self, focus: Option<FocusHandle>) {
        self.primary_focus = focus;
    }

    /// While an overlay (menu, dialog, rename field) owns the keyboard, cycling is suppressed.
    pub fn set_overlay_active(&mut self, active: bool) {
        self.overlay_active = active;
    }

    /// Replaces the tabs. A selection that is no longer listed is replaced by the policy's
    /// survivor as if the tab had been closed; closed tabs are forgotten.
    pub fn set_tabs(&mut self, tabs: Vec<TabSpec>, cx: &mut Context<Self>) {
        let old = std::mem::replace(&mut self.tabs, tabs);
        let live: Vec<TabId> = self.tabs.iter().map(|tab| tab.id.clone()).collect();
        for gone in old.iter().filter(|tab| !live.contains(&tab.id)) {
            self.history.forget(&self.key, &gone.id);
            self.content_focus.remove(&gone.id);
            if self.pending_close.as_ref() == Some(&gone.id) {
                self.pending_close = None;
            }
        }
        self.focus.retain(|id, _| live.contains(id));
        if let Some(selected) = self.selected.clone()
            && !live.contains(&selected)
        {
            let closed_at = old.iter().position(|tab| tab.id == selected).unwrap_or(0);
            self.selected = self.survivor(closed_at);
            if let Some(id) = &self.selected {
                self.history.visit(&self.key, id);
            }
        }
        if self.drag.is_some() {
            self.drag = None;
        }
        cx.notify();
    }

    /// Activates tab `id` programmatically: selects it, records the visit and reveals it. The
    /// owner already knows, so `on_select` is not called.
    pub fn select(&mut self, id: &TabId, cx: &mut Context<Self>) {
        if self.index_of(id).is_none() {
            return;
        }
        self.selected = Some(id.clone());
        self.history.visit(&self.key, id);
        self.reveal(id);
        cx.notify();
    }

    /// Shows the empty state: nothing is selected. This is not a visit.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        cx.notify();
    }

    /// Scrolls horizontally by the minimum that brings tab `id` fully into view.
    pub fn scroll_to_tab(&mut self, id: &TabId, cx: &mut Context<Self>) {
        self.reveal(id);
        cx.notify();
    }

    /// Whether the selected tab is not fully visible, judged from the previous frame.
    pub fn selected_hidden(&self) -> bool {
        let Some(ix) = self.selected.as_ref().and_then(|id| self.index_of(id)) else {
            return false;
        };
        let viewport = self.scroll.bounds().size.width.as_f32();
        if viewport <= 0.0 {
            return false;
        }
        let start = -self.scroll.offset().x.as_f32();
        let left = ix as f32 * TAB_SLOT;
        left < start - 0.5 || left + TAB_WIDTH > start + viewport + 0.5
    }

    /// `(hidden_start, hidden_end)` of the scroll, sampled from the previous frame with a 1 px
    /// tolerance.
    pub fn edge_hidden(&self) -> (bool, bool) {
        edge_hidden(
            -self.scroll.offset().x.as_f32(),
            self.scroll.max_offset().x.as_f32(),
        )
    }

    /// Selects the next (or previous) tab with wrap-around, reveals it and moves focus into its
    /// content when focus was inside the content region. Suppressed while an overlay owns the
    /// keyboard. Returns whether the selection changed.
    pub fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.overlay_active {
            return false;
        }
        let order: Vec<TabId> = self.tabs.iter().map(|tab| tab.id.clone()).collect();
        let Some(target) = cycle_target(&order, self.selected.as_ref(), forward) else {
            return false;
        };
        let focus_in_content = self
            .content_focus
            .values()
            .any(|focus| focus.contains_focused(window, cx));
        let id = order[target].clone();
        self.change_selection(&id);
        self.reveal(&id);
        if focus_in_content && let Some(focus) = self.content_focus.get(&id) {
            window.focus(focus, cx);
        }
        self.flush_soon(window, cx);
        cx.notify();
        true
    }

    /// Closes tab `id` through the owner's veto (an owner action). Returns whether the tab was
    /// removed.
    pub fn close_tab(&mut self, id: &TabId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.close_internal(id, true, window, cx)
    }

    /// The owner resolves a pending close: `allow` removes the tab, otherwise the pending
    /// record is dropped and the tab stays.
    pub fn resolve_close(
        &mut self,
        id: &TabId,
        allow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_close.as_ref() == Some(id) {
            self.pending_close = None;
        }
        if allow {
            self.close_internal(id, false, window, cx);
        }
        cx.notify();
    }

    fn index_of(&self, id: &TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| &tab.id == id)
    }

    /// Selects `id` for a user-driven reason: visit, and a notice when it changed.
    fn change_selection(&mut self, id: &TabId) {
        let changed = self.selected.as_ref() != Some(id);
        self.selected = Some(id.clone());
        self.history.visit(&self.key, id);
        if changed {
            self.outbox.push(Notice::Select(id.clone()));
        }
    }

    /// The surviving selection after the tab that was at `closed_at` left `self.tabs`.
    fn survivor(&self, closed_at: usize) -> Option<TabId> {
        match self.policy {
            ClosePolicy::NearestIndex => active_after_close(closed_at, closed_at, self.tabs.len())
                .and_then(|ix| self.tabs.get(ix))
                .map(|tab| tab.id.clone()),
            ClosePolicy::MostRecentlyVisited => {
                let live: Vec<TabId> = self.tabs.iter().map(|tab| tab.id.clone()).collect();
                self.history.fallback(&self.key, &live)
            }
        }
    }

    fn close_internal(
        &mut self,
        id: &TabId,
        ask: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        if ask
            && let Some(prepare) = self.callbacks.prepare_close.clone()
            && prepare(id, window, cx) != CloseDecision::Allow
        {
            // Pending or Blocked: the tab stays, becomes selected and is recorded as a pending
            // close until the owner resolves it.
            self.pending_close = Some(id.clone());
            self.change_selection(id);
            self.reveal(id);
            self.flush_soon(window, cx);
            cx.notify();
            return false;
        }
        let was_selected = self.selected.as_ref() == Some(id);
        let focus_in_closed = self
            .content_focus
            .get(id)
            .is_some_and(|focus| focus.contains_focused(window, cx));
        self.tabs.remove(index);
        self.history.forget(&self.key, id);
        self.focus.remove(id);
        self.content_focus.remove(id);
        if self.pending_close.as_ref() == Some(id) {
            self.pending_close = None;
        }
        self.drag = None;
        self.outbox.push(Notice::Close(id.clone()));
        if was_selected {
            self.selected = self.survivor(index);
            if let Some(survivor) = self.selected.clone() {
                self.outbox.push(Notice::Select(survivor));
            }
            if focus_in_closed {
                let target = self
                    .selected
                    .as_ref()
                    .and_then(|survivor| self.content_focus.get(survivor))
                    .or(self.primary_focus.as_ref());
                if let Some(focus) = target {
                    window.focus(focus, cx);
                }
            }
        }
        self.flush_soon(window, cx);
        cx.notify();
        true
    }

    /// Scrolls so tab `id` is fully visible with the minimal movement. Before the first layout
    /// the viewport is unknown and the reveal waits for the next render.
    fn reveal(&mut self, id: &TabId) {
        let Some(ix) = self.index_of(id) else {
            return;
        };
        let viewport = self.scroll.bounds().size.width.as_f32();
        if viewport <= 0.0 {
            self.pending_reveal = Some(id.clone());
            return;
        }
        self.pending_reveal = None;
        let left = ix as f32 * TAB_SLOT;
        let offset = self.scroll.offset();
        let target = reveal_offset(offset.x.as_f32(), left, left + TAB_WIDTH, viewport);
        if target != offset.x.as_f32() {
            self.scroll.set_offset(point(px(target), offset.y));
        }
    }

    /// Runs the callbacks of queued notices at the end of the current effect cycle.
    fn flush_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.outbox.is_empty() || self.flush_scheduled {
            return;
        }
        self.flush_scheduled = true;
        let strip: WeakEntity<Self> = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            let Ok((notices, callbacks)) = strip.update(cx, |strip, _| {
                strip.flush_scheduled = false;
                (std::mem::take(&mut strip.outbox), strip.callbacks.clone())
            }) else {
                return;
            };
            for notice in notices {
                match notice {
                    Notice::Select(id) => {
                        if let Some(callback) = &callbacks.select {
                            callback(&id, window, cx);
                        }
                    }
                    Notice::Close(id) => {
                        if let Some(callback) = &callbacks.close {
                            callback(&id, window, cx);
                        }
                    }
                    Notice::Reorder(from, to) => {
                        if let Some(callback) = &callbacks.reorder {
                            callback(from, to, window, cx);
                        }
                    }
                    Notice::ContextMenu(id, position) => {
                        if let Some(callback) = &callbacks.context_menu {
                            callback(&id, position, window, cx);
                        }
                    }
                    Notice::Add => {
                        if let Some(callback) = &callbacks.add {
                            callback(window, cx);
                        }
                    }
                }
            }
        });
    }

    fn begin_drag(&mut self, source: usize, cx: &mut Context<Self>) {
        self.drag = Some(DragState {
            source,
            over: source,
            slides: SlideSet::new(self.tabs.len()),
        });
        cx.notify();
    }

    /// The drop index of a pointer at window x `pointer_x` (content coordinates: the strip's
    /// left edge and the scroll offset are folded in).
    fn over_at(&self, pointer_x: f32) -> usize {
        let rel =
            pointer_x - self.scroll.bounds().origin.x.as_f32() - self.scroll.offset().x.as_f32();
        drop_index(rel, TAB_SLOT, self.tabs.len())
    }

    fn drag_moved(
        &mut self,
        payload: &TabDrag,
        pointer: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if payload.strip != self.key {
            return;
        }
        let Some(source) = self.drag.as_ref().map(|drag| drag.source) else {
            return;
        };
        let over = if bounds.contains(&pointer) {
            self.over_at(pointer.x.as_f32())
        } else {
            source
        };
        let now = MotionClock::now_ms(cx);
        let reduced = cx.reduce_motion();
        if let Some(drag) = &mut self.drag
            && drag.over != over
        {
            drag.over = over;
            drag.slides
                .retarget_for_drag(drag.source, over, now, reduced);
            cx.notify();
        }
    }

    /// A drop of `payload` at `pointer`: moves the tab when both indices are valid and differ;
    /// a payload of another strip is rejected.
    fn dropped(
        &mut self,
        payload: &TabDrag,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if payload.strip != self.key {
            self.drag = None;
            cx.notify();
            return;
        }
        self.drag = None;
        let over = self.over_at(pointer.x.as_f32());
        let from = match self.tabs.get(payload.source) {
            Some(tab) if tab.id == payload.id => Some(payload.source),
            _ => self.index_of(&payload.id),
        };
        if let Some(from) = from
            && reorder(&mut self.tabs, from, over)
        {
            self.outbox.push(Notice::Reorder(from, over));
            self.flush_soon(window, cx);
        }
        cx.notify();
    }

    fn theme_for(&self, cx: &App) -> NavTheme {
        self.theme.clone().unwrap_or_else(|| NavTheme::of(cx))
    }
}

impl Render for TabStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A drag that ended without a drop on a strip (released outside, escaped) is cleared once
        // no drag is active any more.
        if self.drag.is_some() && !cx.has_active_drag() {
            self.drag = None;
        }
        if let Some(id) = self.pending_reveal.clone() {
            self.reveal(&id);
        }
        let theme = self.theme_for(cx);
        let now = MotionClock::now_ms(cx);
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.slides.is_animating(now))
        {
            window.request_animation_frame();
        }
        let strip = cx.entity();
        let key = self.key.clone();

        let mut chips = Vec::with_capacity(self.tabs.len());
        for (ix, spec) in self.tabs.clone().into_iter().enumerate() {
            let focus = self
                .focus
                .entry(spec.id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let selected = self.selected.as_ref() == Some(&spec.id);
            let id = spec.id.clone();
            let dragged = self.drag.as_ref().is_some_and(|drag| drag.source == ix);
            let offset = self.slide_px(ix, now);
            let payload = TabDrag {
                strip: key.clone(),
                source: ix,
                id: id.clone(),
                title: spec.title.clone(),
            };
            let ghost_theme = theme.clone();
            let close_id = id.clone();
            let chip = tab_chip(
                &spec,
                &theme,
                cx.listener(move |this: &mut Self, _: &ClickEvent, window, cx| {
                    this.close_internal(&close_id, true, window, cx);
                }),
            )
            .selected(selected)
            .track_focus(&focus)
            .into_element()
            .on_click(cx.listener({
                let id = id.clone();
                move |this: &mut Self, _: &ClickEvent, window, cx| {
                    this.change_selection(&id);
                    this.flush_soon(window, cx);
                    cx.notify();
                }
            }))
            .on_aux_click(cx.listener({
                let id = id.clone();
                move |this: &mut Self, event: &ClickEvent, window, cx| {
                    if event.is_middle_click() {
                        this.close_internal(&id, true, window, cx);
                    } else if event.is_secondary()
                        && let Some(position) = event.mouse_position().or(Some(event.position()))
                    {
                        this.outbox.push(Notice::ContextMenu(id.clone(), position));
                        this.flush_soon(window, cx);
                    }
                }
            }))
            .on_drag(payload, {
                let strip = strip.clone();
                move |payload: &TabDrag, _offset, _window, cx| {
                    let source = payload.source;
                    strip.update(cx, |strip, cx| strip.begin_drag(source, cx));
                    cx.new(|_| TabGhost {
                        title: payload.title.clone(),
                        theme: ghost_theme.clone(),
                    })
                }
            })
            .on_drop(
                cx.listener(move |this: &mut Self, payload: &TabDrag, window, cx| {
                    this.dropped(payload, window.mouse_position(), window, cx);
                }),
            )
            .when(dragged, |chip| chip.opacity(0.0))
            .when(!dragged && offset != 0.0, |chip| {
                chip.relative().left(px(offset))
            });
            chips.push(chip);
        }

        let row = div()
            .id(ElementId::Name(format!("tab-row:{key}").into()))
            .flex()
            .flex_row()
            .flex_shrink(1.)
            .min_w_0()
            .gap(px(TAB_GAP))
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .children(chips);

        let add = (self.callbacks.add.is_some() && !self.tabs.is_empty()).then(|| {
            let wash = theme.wash(super::chip::HOVER_WASH);
            div()
                .id("tab-add")
                .debug_selector(|| "tab-add".to_owned())
                .role(Role::Button)
                .aria_label("New tab")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(6.))
                .text_size(px(14.))
                .text_color(theme.muted)
                .cursor_pointer()
                .hover(move |style| style.bg(wash))
                .tooltip(tooltip("New tab".into(), &theme))
                .on_click(cx.listener(|this: &mut Self, _: &ClickEvent, window, cx| {
                    this.outbox.push(Notice::Add);
                    this.flush_soon(window, cx);
                }))
                .child("+")
        });

        div()
            .id(ElementId::Name(format!("tab-strip:{key}").into()))
            .debug_selector(move || format!("tab-strip:{key}"))
            .role(Role::TabList)
            .key_context("TabStrip")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(TAB_GAP))
            .w_full()
            .h(px(TAB_HEIGHT))
            .on_action(cx.listener(|this: &mut Self, _: &NextTab, window, cx| {
                this.cycle(true, window, cx);
            }))
            .on_action(cx.listener(|this: &mut Self, _: &PreviousTab, window, cx| {
                this.cycle(false, window, cx);
            }))
            .on_drag_move(cx.listener(
                |this: &mut Self, event: &DragMoveEvent<TabDrag>, _window, cx| {
                    let payload = event.drag(cx).clone();
                    this.drag_moved(&payload, event.event.position, event.bounds, cx);
                },
            ))
            .on_drop(
                cx.listener(move |this: &mut Self, payload: &TabDrag, window, cx| {
                    this.dropped(payload, window.mouse_position(), window, cx);
                }),
            )
            .child(tab_strip_region(row, &self.scroll, &theme))
            .children(add)
    }
}
