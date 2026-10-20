//! The terminal view: tabbed retained emulators with input coalescing, resize debouncing,
//! selection gestures, wheel scrolling and focus routing (CMP-021 R11-R14, R16-R22), and the
//! closable, reorderable tab strip drawn above the grid (R20, CMP-017).

use std::rc::Rc;
use std::time::Duration;

use vespertine::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Styled,
    Task, TouchPhase, Window, div,
};

use super::element::TerminalElement;
use super::emulator::{Emulator, GridPoint, GridSize, SelectionType, Side};
use super::geometry::{CellHit, DRAG_THRESHOLD, WheelAccumulator, cell_at, edge_scroll_lines};
use super::input::{
    ClipboardShortcut, InputCoalescer, clipboard_shortcut, keystroke_to_bytes, paste_bytes,
};
use super::theme::TerminalTheme;
use crate::tabs::{ClosePolicy, TabId as StripTabId, TabSpec, TabStrip, cycle_on};

/// Delay before a changed grid size is reported (R5).
pub const RESIZE_DEBOUNCE: Duration = Duration::from_millis(80);
/// Window in which input bytes are coalesced into one write (R14).
pub const INPUT_COALESCE: Duration = Duration::from_millis(12);
/// Period of the selection edge auto-scroll (R18).
pub const AUTOSCROLL_TICK: Duration = Duration::from_millis(24);

/// The size of a tab's grid before the first measurement.
const INITIAL_GRID: GridSize = GridSize { cols: 80, rows: 24 };

/// The identity of a terminal tab, stable across moves and closes of other tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

/// The measured placement of the grid, reported by the element every frame (R5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridGeometry {
    /// The element bounds (window coordinates, logical pixels).
    pub bounds: Bounds<Pixels>,
    /// The grid origin: the bounds inset by the padding.
    pub origin: Point<Pixels>,
    /// Cell width in logical pixels.
    pub cell_width: f32,
    /// Line height in logical pixels.
    pub line_height: f32,
    /// Columns.
    pub cols: usize,
    /// Rows.
    pub rows: usize,
}

/// One terminal tab: its own emulator, wheel remainder, input buffer and timers (R20).
struct Tab {
    id: TabId,
    emulator: Emulator,
    wheel: WheelAccumulator,
    input: InputCoalescer,
    flush_task: Option<Task<()>>,
    resize_task: Option<Task<()>>,
    reported_size: Option<GridSize>,
    ready: bool,
    exited: Option<i32>,
    title: Option<String>,
}

/// An active selection gesture (R16).
struct Gesture {
    press: Point<Pixels>,
    press_point: GridPoint,
    press_side: Side,
    /// The selection exists (the drag threshold was crossed, or the gesture selects on press).
    started: bool,
    /// The latest pointer sample, used by the edge auto-scroll while the pointer is still.
    latest: Point<Pixels>,
}

type InputCallback = Rc<dyn Fn(TabId, &[u8], &mut App)>;
type ResizeCallback = Rc<dyn Fn(TabId, usize, usize, &mut App)>;
type TitleCallback = Rc<dyn Fn(TabId, Option<&str>, &mut App)>;
type BellCallback = Rc<dyn Fn(TabId, &mut App)>;
type SelectCallback = Rc<dyn Fn(TabId, &str, &mut App)>;
type NewTabCallback = Rc<dyn Fn(&mut Window, &mut App)>;
type TabClosedCallback = Rc<dyn Fn(TabId, usize, &mut Window, &mut App)>;

/// The tabbed terminal view (R20-R22).
///
/// Callbacks run inside an update of the panel and must not update it, except
/// [`on_new_tab`](Self::on_new_tab) and [`on_tab_closed`](Self::on_tab_closed), which run after
/// the strip's update has finished and may call back into the panel. The panel owns the tab
/// model and draws it as a CMP-017 tab strip above the grid: selecting, closing, reordering and
/// adding tabs on the strip act on the model, and every change of the model (by the host or the
/// strip) is mirrored into the strip.
pub struct TerminalPanel {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    focus_handle: FocusHandle,
    pending_focus: bool,
    theme: Option<TerminalTheme>,
    geometry: Option<GridGeometry>,
    gesture: Option<Gesture>,
    autoscroll: Option<Task<()>>,
    on_input: Option<InputCallback>,
    on_resize: Option<ResizeCallback>,
    on_title: Option<TitleCallback>,
    on_bell: Option<BellCallback>,
    on_select: Option<SelectCallback>,
    on_new_tab: Option<NewTabCallback>,
    on_tab_closed: Option<TabClosedCallback>,
    strip: Entity<TabStrip>,
}

/// The new active index after closing tab `closed` while `active` was active (R20): `active - 1`
/// if `closed < active`, else `active`, clamped to the last tab; 0 when no tabs remain.
pub fn active_after_close(active: usize, closed: usize, remaining: usize) -> usize {
    if remaining == 0 {
        return 0;
    }
    let next = if closed < active { active - 1 } else { active };
    next.min(remaining - 1)
}

/// The new active index after moving a tab from `from` to `to` (remove at `from`, insert at
/// `to`) (R20): `to` when the moved tab was active; `active - 1` when `from < active` and
/// `to >= active`; `active + 1` when `from > active` and `to <= active`; otherwise unchanged.
pub fn active_after_move(active: usize, from: usize, to: usize) -> usize {
    if from == active {
        return to;
    }
    if from < active && to >= active {
        return active - 1;
    }
    if from > active && to <= active {
        return active + 1;
    }
    active
}

impl TerminalPanel {
    /// An empty panel; open tabs with [`open_tab`](Self::open_tab).
    pub fn new(cx: &mut Context<Self>) -> Self {
        let strip = Self::build_strip(cx);
        Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            focus_handle: cx.focus_handle(),
            pending_focus: false,
            theme: None,
            geometry: None,
            gesture: None,
            autoscroll: None,
            on_input: None,
            on_resize: None,
            on_title: None,
            on_bell: None,
            on_select: None,
            on_new_tab: None,
            on_tab_closed: None,
            strip,
        }
    }

    /// The tab strip: its callbacks translate user gestures into updates of the panel's model.
    /// Select, close and reorder notices arrive after the strip has already applied them to its
    /// own list, so the panel's reaction (which mirrors the model back) is idempotent.
    fn build_strip(cx: &mut Context<Self>) -> Entity<TabStrip> {
        let panel = cx.weak_entity();
        let key = format!("terminal-tabs:{}", cx.entity_id().as_u64());
        let on_select = panel.clone();
        let on_close = panel.clone();
        let on_reorder = panel.clone();
        let on_add = panel;
        cx.new(|_| {
            TabStrip::new(key)
                .close_policy(ClosePolicy::NearestIndex)
                .on_select(move |id, window, cx| {
                    let Some(tab) = parse_strip_id(id) else {
                        return;
                    };
                    on_select
                        .update(cx, |panel, cx| {
                            panel.select_tab_id(tab, cx);
                            window.focus(&panel.focus_handle, cx);
                        })
                        .ok();
                })
                .on_close(move |id, window, cx| {
                    let Some(tab) = parse_strip_id(id) else {
                        return;
                    };
                    let closed = on_close.update(cx, |panel, cx| {
                        let remaining = panel.close_tab(tab, cx);
                        window.focus(&panel.focus_handle, cx);
                        (remaining, panel.on_tab_closed.clone())
                    });
                    if let Ok((remaining, Some(callback))) = closed {
                        callback(tab, remaining, window, cx);
                    }
                })
                .on_reorder(move |from, to, _window, cx| {
                    on_reorder
                        .update(cx, |panel, cx| panel.move_tab(from, to, cx))
                        .ok();
                })
                .on_add(move |window, cx| {
                    let Ok(callback) = on_add.update(cx, |panel, _| panel.on_new_tab.clone())
                    else {
                        return;
                    };
                    match callback {
                        Some(callback) => callback(window, cx),
                        None => {
                            on_add
                                .update(cx, |panel, cx| {
                                    panel.open_tab(cx);
                                    window.focus(&panel.focus_handle, cx);
                                })
                                .ok();
                        }
                    }
                })
        })
    }

    /// Mirrors the tab model (order, titles, exit state, active tab) into the strip.
    fn sync_strip(&self, cx: &mut Context<Self>) {
        let specs: Vec<TabSpec> = self.tabs.iter().map(strip_spec).collect();
        let active = self.active_tab().map(strip_id);
        let focus = self.focus_handle.clone();
        self.strip.update(cx, |strip, cx| {
            strip.set_tabs(specs, cx);
            for tab in &self.tabs {
                strip.set_content_focus(strip_id(tab.id), focus.clone());
            }
            strip.set_primary_focus(Some(focus));
            match active {
                Some(id) => strip.select(&id, cx),
                None => strip.clear_selection(cx),
            }
        });
    }

    // --- Callbacks ---------------------------------------------------------------------

    /// Receives the coalesced bytes destined for a tab's session (keystrokes, pastes and the
    /// emulator's query responses), in order.
    pub fn on_input(&mut self, callback: impl Fn(TabId, &[u8], &mut App) + 'static) {
        self.on_input = Some(Rc::new(callback));
    }

    /// Receives a tab's grid size `(cols, rows)` after it changed, debounced by 80 ms.
    pub fn on_resize(&mut self, callback: impl Fn(TabId, usize, usize, &mut App) + 'static) {
        self.on_resize = Some(Rc::new(callback));
    }

    /// Receives a tab's window title when the program sets it (`None`: reset).
    pub fn on_title(&mut self, callback: impl Fn(TabId, Option<&str>, &mut App) + 'static) {
        self.on_title = Some(Rc::new(callback));
    }

    /// Receives a bell of a tab.
    pub fn on_bell(&mut self, callback: impl Fn(TabId, &mut App) + 'static) {
        self.on_bell = Some(Rc::new(callback));
    }

    /// Receives the selected text when a selection gesture finishes with a non-empty
    /// selection (a click without a drag selects nothing and never fires this).
    pub fn on_select(&mut self, callback: impl Fn(TabId, &str, &mut App) + 'static) {
        self.on_select = Some(Rc::new(callback));
    }

    /// Receives the strip's new-tab button. The host starts a session and calls
    /// [`open_tab`](Self::open_tab) (or [`open_tab_pending`](Self::open_tab_pending)); the
    /// callback runs outside the panel's update. Without one the button appends a ready tab.
    pub fn on_new_tab(&mut self, callback: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_new_tab = Some(Rc::new(callback));
    }

    /// Receives a tab the user closed on the strip (close button or middle click) with the
    /// number of tabs left, after the panel dropped its state; the host ends the session and
    /// decides whether an empty dock closes. Runs outside the panel's update. Closing through
    /// [`close_tab`](Self::close_tab) does not call it: the host asked for that itself.
    pub fn on_tab_closed(
        &mut self,
        callback: impl Fn(TabId, usize, &mut Window, &mut App) + 'static,
    ) {
        self.on_tab_closed = Some(Rc::new(callback));
    }

    // --- Theme -------------------------------------------------------------------------

    /// Overrides the colors and font; `None` follows the active theme (THM-003).
    pub fn set_theme(&mut self, theme: Option<TerminalTheme>, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// The theme in effect.
    pub fn theme(&self, cx: &App) -> TerminalTheme {
        self.theme
            .clone()
            .unwrap_or_else(|| TerminalTheme::current(cx))
    }

    // --- Tabs --------------------------------------------------------------------------

    fn new_tab(&mut self, ready: bool) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        let size = self.geometry.map_or(INITIAL_GRID, |geometry| {
            GridSize::new(geometry.cols, geometry.rows)
        });
        self.tabs.push(Tab {
            id,
            emulator: Emulator::new(size.cols, size.rows),
            wheel: WheelAccumulator::default(),
            input: InputCoalescer::default(),
            flush_task: None,
            resize_task: None,
            reported_size: None,
            ready,
            exited: None,
            title: None,
        });
        self.active = self.tabs.len() - 1;
        self.end_gesture();
        id
    }

    /// Appends a tab whose session is ready and makes it active.
    pub fn open_tab(&mut self, cx: &mut Context<Self>) -> TabId {
        let id = self.new_tab(true);
        self.sync_strip(cx);
        cx.notify();
        id
    }

    /// Appends a tab whose session has no identity yet and makes it active. Input stays buffered
    /// (the flush keeps rescheduling) until [`set_session_ready`](Self::set_session_ready).
    pub fn open_tab_pending(&mut self, cx: &mut Context<Self>) -> TabId {
        let id = self.new_tab(false);
        self.sync_strip(cx);
        cx.notify();
        id
    }

    /// Marks whether a tab's session can take input.
    pub fn set_session_ready(&mut self, tab: TabId, ready: bool) {
        if let Some(tab) = self.tab_mut(tab) {
            tab.ready = ready;
        }
    }

    /// The tab strip entity drawn above the grid: hosts may restyle it (`theme`) or read its
    /// state; the panel keeps its tabs and selection in sync with the model.
    pub fn tab_strip(&self) -> &Entity<TabStrip> {
        &self.strip
    }

    /// The number of tabs.
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// The tab identities in strip order.
    pub fn tab_ids(&self) -> Vec<TabId> {
        self.tabs.iter().map(|tab| tab.id).collect()
    }

    /// The index of the active tab (0 when there are none).
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The active tab, if any.
    pub fn active_tab(&self) -> Option<TabId> {
        self.tabs.get(self.active).map(|tab| tab.id)
    }

    /// The program-set title of a tab.
    pub fn tab_title(&self, tab: TabId) -> Option<String> {
        self.tab(tab).and_then(|tab| tab.emulator.title())
    }

    /// The exit status of a tab marked exited.
    pub fn exit_status(&self, tab: TabId) -> Option<i32> {
        self.tab(tab).and_then(|tab| tab.exited)
    }

    /// The emulator of a tab (read-only snapshots).
    pub fn emulator(&self, tab: TabId) -> Option<&Emulator> {
        self.tab(tab).map(|tab| &tab.emulator)
    }

    /// The emulator of the active tab.
    pub fn active_emulator(&self) -> Option<&Emulator> {
        self.tabs.get(self.active).map(|tab| &tab.emulator)
    }

    /// The bytes waiting in a tab's input buffer.
    pub fn pending_input(&self, tab: TabId) -> bool {
        self.tab(tab).is_some_and(|tab| !tab.input.is_empty())
    }

    pub(super) fn has_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    /// The measured geometry of the last painted frame.
    pub fn geometry(&self) -> Option<GridGeometry> {
        self.geometry
    }

    fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    /// Selects the tab at `index`; out-of-range indexes do nothing. Only changes which
    /// emulator is painted.
    pub fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() && index != self.active {
            self.active = index;
            self.end_gesture();
            self.sync_strip(cx);
            cx.notify();
        }
    }

    /// Selects a tab by identity.
    pub fn select_tab_id(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|candidate| candidate.id == tab) {
            self.select_tab(index, cx);
        }
    }

    /// Closes a tab, dropping its state and cancelling its timers; returns the number of tabs
    /// left (the host decides whether an empty dock closes).
    pub fn close_tab(&mut self, tab: TabId, cx: &mut Context<Self>) -> usize {
        let Some(index) = self.tabs.iter().position(|candidate| candidate.id == tab) else {
            return self.tabs.len();
        };
        self.tabs.remove(index);
        self.active = active_after_close(self.active, index, self.tabs.len());
        self.end_gesture();
        self.sync_strip(cx);
        cx.notify();
        self.tabs.len()
    }

    /// Moves the tab at `from` to `to`; equal or out-of-range indexes do nothing.
    pub fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.tabs.len();
        if from == to || from >= len || to >= len {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = active_after_move(self.active, from, to);
        self.sync_strip(cx);
        cx.notify();
    }

    /// Marks a tab exited with `status`: input is discarded and the surface keeps showing its
    /// last grid. The host may append a status line through [`feed`](Self::feed) (see
    /// [`exit_status_line`]).
    pub fn mark_exited(&mut self, tab: TabId, status: i32, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab_mut(tab) {
            tab.exited = Some(status);
            tab.input.clear();
            tab.flush_task = None;
        }
        self.sync_strip(cx);
        cx.notify();
    }

    // --- Output ------------------------------------------------------------------------

    /// Folds output bytes into a tab (active or not). Query responses of the emulator go to the
    /// session through the tab's input buffer; title and bell changes are reported.
    pub fn feed(&mut self, tab: TabId, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(entry) = self.tab_mut(tab) else {
            return;
        };
        let responses = entry.emulator.feed(bytes);
        let title = entry.emulator.title();
        let title_changed = title != entry.title;
        entry.title = title.clone();
        let bell = entry.emulator.take_bell();
        let is_active = self.active_tab() == Some(tab);
        if !responses.is_empty() {
            self.send_input(tab, &responses, cx);
        }
        if title_changed {
            self.sync_strip(cx);
            if let Some(callback) = self.on_title.clone() {
                callback(tab, title.as_deref(), cx);
            }
        }
        if bell && let Some(callback) = self.on_bell.clone() {
            callback(tab, cx);
        }
        if is_active {
            cx.notify();
        }
    }

    // --- Input -------------------------------------------------------------------------

    /// Queues bytes for a tab's session through its coalescing buffer (R14): the first push
    /// into an empty buffer schedules one flush after 12 ms. Input of an exited tab is
    /// discarded.
    pub fn send_input(&mut self, tab: TabId, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(entry) = self.tab_mut(tab) else {
            return;
        };
        if entry.exited.is_some() {
            return;
        }
        if entry.input.push(bytes) {
            self.schedule_flush(tab, cx);
        }
    }

    fn schedule_flush(&mut self, id: TabId, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(INPUT_COALESCE).await;
                let Ok(done) = this.update(cx, |this, cx| this.try_flush(id, cx)) else {
                    return;
                };
                if done {
                    return;
                }
            }
        });
        if let Some(tab) = self.tab_mut(id) {
            tab.flush_task = Some(task);
        }
    }

    /// Runs when a flush timer fires; `true` when the flush is finished (flushed, discarded or
    /// the tab is gone), `false` to reschedule (the session is not ready yet).
    fn try_flush(&mut self, id: TabId, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tab_mut(id) else {
            return true;
        };
        if tab.exited.is_some() {
            tab.input.clear();
            tab.flush_task = None;
            return true;
        }
        if !tab.ready {
            return false;
        }
        let bytes = tab.input.take();
        tab.flush_task = None;
        if !bytes.is_empty()
            && let Some(callback) = self.on_input.clone()
        {
            callback(id, &bytes, cx);
        }
        true
    }

    /// Pastes `text` into the active tab (R13): bracket markers are sanitized and, in
    /// bracketed-paste mode, wrapped.
    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let bytes = paste_bytes(text, tab.emulator.bracketed_paste_mode());
        let id = tab.id;
        self.snap_to_bottom(cx);
        self.send_input(id, &bytes, cx);
    }

    /// Copies the active selection to the clipboard; `false` (and the clipboard untouched) when
    /// nothing is selected.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.emulator.selection_text())
            .filter(|text| !text.is_empty())
        else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn snap_to_bottom(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.emulator.display_offset() > 0
        {
            tab.emulator.scroll_to_bottom();
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match clipboard_shortcut(&keystroke.key, keystroke.modifiers) {
            Some(ClipboardShortcut::Copy) => {
                if self.copy_selection(cx) {
                    cx.stop_propagation();
                    return;
                }
            }
            Some(ClipboardShortcut::Paste) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.paste(&text, cx);
                }
                cx.stop_propagation();
                return;
            }
            None => {}
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let id = tab.id;
        let Some(bytes) = keystroke_to_bytes(
            &keystroke.key,
            keystroke.key_char.as_deref(),
            keystroke.modifiers,
            tab.emulator.app_cursor_mode(),
        ) else {
            return;
        };
        self.snap_to_bottom(cx);
        self.send_input(id, &bytes, cx);
        cx.stop_propagation();
    }

    // --- Focus -------------------------------------------------------------------------

    /// Requests keyboard focus; applied at the next render once the view is mounted.
    pub fn request_focus(&mut self, cx: &mut Context<Self>) {
        self.pending_focus = true;
        cx.notify();
    }

    /// Clears a pending focus request (the host closed the view).
    pub fn cancel_focus_request(&mut self) {
        self.pending_focus = false;
    }

    /// Whether a focus request is waiting for the next render.
    pub fn has_pending_focus(&self) -> bool {
        self.pending_focus
    }

    // --- Measurement -------------------------------------------------------------------

    /// Reports the geometry measured during prepaint. The active emulator is resized
    /// immediately when the grid changed; the size event is debounced (R5).
    pub(super) fn measured(&mut self, geometry: GridGeometry, cx: &mut Context<Self>) {
        self.geometry = Some(geometry);
        let size = GridSize::new(geometry.cols, geometry.rows);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.emulator.cols() != size.cols || tab.emulator.rows() != size.rows {
            tab.emulator.resize(size.cols, size.rows);
        }
        if tab.reported_size != Some(size) && tab.resize_task.is_none() {
            let id = tab.id;
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RESIZE_DEBOUNCE).await;
                this.update(cx, |this, cx| this.report_resize(id, cx)).ok();
            });
            tab.resize_task = Some(task);
        }
    }

    fn report_resize(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_mut(id) else {
            return;
        };
        tab.resize_task = None;
        // The current size is re-read: intermediate sizes are never reported.
        let size = GridSize::new(tab.emulator.cols(), tab.emulator.rows());
        if tab.reported_size == Some(size) {
            return;
        }
        tab.reported_size = Some(size);
        if let Some(callback) = self.on_resize.clone() {
            callback(id, size.cols, size.rows, cx);
        }
    }

    // --- Pointer -----------------------------------------------------------------------

    fn hit(&self, position: Point<Pixels>) -> Option<CellHit> {
        let geometry = self.geometry?;
        Some(cell_at(
            f32::from(position.x - geometry.origin.x),
            f32::from(position.y - geometry.origin.y),
            geometry.cell_width,
            geometry.line_height,
            geometry.cols,
            geometry.rows,
        ))
    }

    fn end_gesture(&mut self) {
        self.gesture = None;
        self.autoscroll = None;
    }

    /// A press inside the surface (R16): focuses, then arms the gesture by click count.
    pub fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        window.focus(&self.focus_handle, cx);
        self.pending_focus = false;
        let Some(hit) = self.hit(event.position) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let point = tab.emulator.grid_point(hit.row, hit.col);
        // One click arms a character selection that starts after the drag threshold (a shift
        // click extends an existing selection at once); two clicks select a word and three or
        // more a line, both complete on press.
        let started = match event.click_count {
            0 | 1 => {
                if event.modifiers.shift && tab.emulator.has_selection() {
                    tab.emulator.update_selection(point, hit.side);
                    true
                } else {
                    tab.emulator.clear_selection();
                    false
                }
            }
            2 => {
                tab.emulator
                    .start_selection(SelectionType::Semantic, point, hit.side);
                true
            }
            _ => {
                tab.emulator
                    .start_selection(SelectionType::Lines, point, hit.side);
                true
            }
        };
        self.gesture = Some(Gesture {
            press: event.position,
            press_point: point,
            press_side: hit.side,
            started,
            latest: event.position,
        });
        cx.notify();
    }

    /// Pointer movement (R16, R18): extends the selection of a live gesture.
    pub fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.gesture.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            // The release happened where no listener saw it.
            self.end_gesture();
            return;
        }
        let Some(hit) = self.hit(event.position) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(gesture) = self.gesture.as_mut() else {
            return;
        };
        gesture.latest = event.position;
        if !gesture.started {
            let dx = f32::from(event.position.x - gesture.press.x);
            let dy = f32::from(event.position.y - gesture.press.y);
            if dx.hypot(dy) < DRAG_THRESHOLD {
                return;
            }
            // Anchored at the press cell and side, not where the threshold tripped.
            tab.emulator.start_selection(
                SelectionType::Simple,
                gesture.press_point,
                gesture.press_side,
            );
            gesture.started = true;
        }
        let point = tab.emulator.grid_point(hit.row, hit.col);
        tab.emulator.update_selection(point, hit.side);
        self.ensure_autoscroll(cx);
        cx.notify();
    }

    /// The release of the left button, inside or outside the surface: ends the gesture and
    /// cancels edge scrolling.
    pub fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        self.autoscroll = None;
        if !gesture.started {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if let Some(text) = tab
            .emulator
            .selection_text()
            .filter(|text| !text.is_empty())
            && let Some(callback) = self.on_select.clone()
        {
            let id = tab.id;
            callback(id, &text, cx);
        }
        cx.notify();
    }

    fn ensure_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.autoscroll.is_some() || self.edge_lines() == 0 {
            return;
        }
        self.autoscroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSCROLL_TICK).await;
                let Ok(again) = this.update(cx, |this, cx| {
                    let again = this.autoscroll_tick(cx);
                    if !again {
                        this.autoscroll = None;
                    }
                    again
                }) else {
                    return;
                };
                if !again {
                    return;
                }
            }
        }));
    }

    fn edge_lines(&self) -> i32 {
        let (Some(geometry), Some(gesture)) = (self.geometry, self.gesture.as_ref()) else {
            return 0;
        };
        if !gesture.started {
            return 0;
        }
        edge_scroll_lines(
            f32::from(gesture.latest.y),
            f32::from(geometry.origin.y),
            geometry.line_height,
            geometry.rows,
        )
    }

    /// One auto-scroll tick: scrolls by the edge speed and extends the selection to the latest
    /// pointer sample even when the pointer is stationary. `false` when the pointer left the
    /// edge band (the timer stops).
    fn autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let lines = self.edge_lines();
        if lines == 0 {
            return false;
        }
        let Some(latest) = self.gesture.as_ref().map(|gesture| gesture.latest) else {
            return false;
        };
        let Some(hit) = self.hit(latest) else {
            return false;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return false;
        };
        tab.emulator.scroll(lines);
        let point = tab.emulator.grid_point(hit.row, hit.col);
        tab.emulator.update_selection(point, hit.side);
        cx.notify();
        true
    }

    /// A wheel event over the surface (R19): whole lines scroll, the fraction accumulates in
    /// the tab's remainder.
    pub fn scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if event.touch_phase == TouchPhase::Started {
            tab.wheel.reset();
        }
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / geometry.line_height,
        };
        let whole = tab.wheel.push(lines);
        if whole != 0 {
            tab.emulator.scroll(whole);
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.pending_focus) {
            window.focus(&self.focus_handle, cx);
        }
        let root = div()
            .id("terminal-panel")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_col();
        let grid = TerminalElement::new(cx.entity());
        let tab_row = div().flex_none().w_full().child(self.strip.clone());
        let grid_area = div().w_full().min_h_0().flex_1().child(grid);
        // Tab cycling keys reach the strip while focus is in the grid.
        cycle_on(root, &self.strip).child(tab_row).child(grid_area)
    }
}

fn strip_id(tab: TabId) -> StripTabId {
    StripTabId::from(tab.0)
}

fn parse_strip_id(id: &StripTabId) -> Option<TabId> {
    id.as_str().parse().ok().map(TabId)
}

/// The strip's description of a tab: the program-set title (or "Terminal N"), with the exit
/// status in its accessible name and tooltip once the session ended.
fn strip_spec(tab: &Tab) -> TabSpec {
    let title = tab
        .title
        .clone()
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| format!("Terminal {}", tab.id.0));
    let spec = TabSpec::new(strip_id(tab.id), title.clone());
    match tab.exited {
        Some(status) => spec
            .accessible_name(format!("{title} (exited with status {status})"))
            .detail(format!("Process exited with status {status}")),
        None => spec,
    }
}

/// The reference exit status line: a blank line, a dimmed `[process exited N]` and a newline;
/// feed it to a tab marked exited.
pub fn exit_status_line(status: i32) -> Vec<u8> {
    format!("\r\n\x1b[2m[process exited {status}]\x1b[22m\r\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_tab_moves_the_active_index() {
        // (active, closed, remaining) -> new active
        assert_eq!(active_after_close(2, 0, 2), 1);
        assert_eq!(active_after_close(1, 2, 2), 1);
        assert_eq!(active_after_close(1, 1, 2), 1);
        assert_eq!(active_after_close(2, 2, 2), 1);
        assert_eq!(active_after_close(0, 0, 0), 0);
        assert_eq!(active_after_close(0, 0, 3), 0);
    }

    #[test]
    fn moving_a_tab_follows_the_active_tab() {
        assert_eq!(active_after_move(1, 1, 3), 3);
        assert_eq!(active_after_move(2, 0, 2), 1);
        assert_eq!(active_after_move(2, 0, 3), 1);
        assert_eq!(active_after_move(1, 3, 1), 2);
        assert_eq!(active_after_move(1, 3, 0), 2);
        assert_eq!(active_after_move(1, 0, 0), 1);
        assert_eq!(active_after_move(3, 0, 1), 3);
        assert_eq!(active_after_move(0, 2, 3), 0);
    }

    #[test]
    fn exit_status_line_is_blank_dim_and_newline_terminated() {
        let mut emulator = Emulator::new(30, 4);
        emulator.feed(b"done\r\n");
        emulator.feed(&exit_status_line(3));
        assert_eq!(emulator.row_text(1), "");
        assert_eq!(emulator.row_text(2), "[process exited 3]");
        assert!(emulator.line(2)[0].dim);
    }
}
