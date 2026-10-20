//! The [`Tree`] view: the virtualized, lazily loaded file tree (CMP-016).
//!
//! The view owns a [`TreeModel`] and renders its visible rows through the virtual list. It
//! contains no file-system access: the owner answers load requests with
//! [`Tree::apply_page`], performs mutations (rename, delete, move) and reports their results.
//! Owner callbacks run deferred (after the current effect cycle), so they may freely update the
//! tree entity.

use super::drag::{
    DRAG_TICK, DragItem, DragSession, DropHit, RAIL_EXCLUSION, ROOT_ROW_HEIGHT, TreeDragPayload,
    destination_paths, resolve_destination,
};
use super::menu::{COPY_PATH, ContextMenu, DELETE, RENAME, copy_path_text, is_mutable, menu_items};
use super::model::{
    Activation, EntryKind, LoadRequest, Outcome, Page, PageError, RowKind, TreeModel, VisibleRow,
};
use super::rename::{RenameCommit, RenameSession, basename_range};
use super::rows::{list_fade_flags, sync_list_rows};
use super::search::{RevealCoordinator, RevealEffect, RevealIntent, RevealProgress, RevealRequest};
use super::style::{
    CHEVRON_SIZE, DISCLOSURE_SLOT, FADE_BAND, GAP, GUIDE_WIDTH, ICON_SIZE, IGNORED_OPACITY,
    NAME_SIZE, RENAME_HEIGHT, RENAME_PADDING, RENAME_RADIUS, ROW_HEIGHT, ROW_PADDING_RIGHT,
    RowVisual, STATUS_SIZE, TreeTheme, TreeWashes, chevron, guide_x, row_padding_left,
};
use crate::floating::{
    EscapeHandler, EscapeOutcome, FocusTarget, LayerHandle, LayerOptions, ListKey, OutsidePress,
    Placement, Popup, PopupMenuItem, SurfaceMotion, anchored_menu_at, classify_key, close_popup,
    menu_item_row, open_layer,
};
use crate::scroll::{ListRail, RailBar, RailOrientation, render_rail, track_hover};
use crate::text::{TextField, TextFieldEvent};
use crate::tokens::ComponentTokens;
use std::rc::Rc;
use std::time::Instant;
use vespertine::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle,
    DragMoveEvent, Entity, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, ListAlignment, ListState, Modifiers, MotionClock, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, Render, Role, ScrollWheelEvent, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window, div,
    edge_faded, list, prelude::FluentBuilder, px,
};

type IdentityFn = Rc<dyn Fn(EntryKind, &str, bool, &mut Window, &mut App) -> AnyElement>;
type DecorationFn = Rc<dyn Fn(&str, bool) -> Option<Hsla>>;
type PathHook = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type LoadHook = Rc<dyn Fn(&LoadRequest, &mut Window, &mut App)>;
type MenuHook = Rc<dyn Fn(&[String], Point<Pixels>, &mut Window, &mut App)>;
type RenameHook = Rc<dyn Fn(&str, &str, &mut Window, &mut App)>;
type PathsHook = Rc<dyn Fn(&[String], &mut Window, &mut App)>;
type MoveHook = Rc<dyn Fn(&[(String, String)], &mut Window, &mut App)>;
type CanMutateFn = Rc<dyn Fn(&str, bool) -> bool>;
type AcceptsDragFn = Rc<dyn Fn(&TreeDragPayload) -> bool>;
type GhostFn = Rc<dyn Fn(&TreeDragPayload, &mut Window, &mut App) -> AnyElement>;
type ResolveFn = Rc<dyn Fn(&str) -> String>;

/// Notifications of the tree that are not owner callbacks.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeEvent {
    /// The selection or the cursor changed through user input.
    SelectionChanged,
    /// A rename commit was rejected with this message (the field stays open).
    RenameRejected(String),
    /// A reveal finished: the path is selected; apply the effect (clear the search, open).
    RevealFinished {
        /// The revealed path.
        path: String,
        /// What the owner does besides selecting and scrolling.
        effect: RevealEffect,
    },
    /// A reveal failed: surface the message as a search error; the tree is unchanged.
    RevealFailed(String),
}

#[derive(Clone, Default)]
struct Hooks {
    identity: Option<IdentityFn>,
    decoration: Option<DecorationFn>,
    on_open: Option<PathHook>,
    load_directory: Option<LoadHook>,
    on_context_menu: Option<MenuHook>,
    on_rename: Option<RenameHook>,
    on_delete: Option<PathsHook>,
    on_move: Option<MoveHook>,
    can_mutate: Option<CanMutateFn>,
    accepts_drag: Option<AcceptsDragFn>,
    drag_ghost: Option<GhostFn>,
    path_resolver: Option<ResolveFn>,
}

struct OwnerMenuItem {
    id: String,
    label: String,
    handler: PathsHook,
}

struct OpenMenu {
    menu: ContextMenu,
    position: Point<Pixels>,
    layer: LayerHandle,
}

struct RenameState {
    session: RenameSession,
    field: Entity<TextField>,
    _subscriptions: Vec<Subscription>,
}

/// The line height of the rename field's text: the field is 23 px tall with a 1 px border.
const RENAME_LINE_HEIGHT: f32 = RENAME_HEIGHT - 2.0;

/// The interactive virtual file tree.
pub struct Tree {
    model: TreeModel,
    list: ListState,
    rows: Vec<VisibleRow>,
    rows_revision: u64,
    focus_handle: FocusHandle,
    menu_focus: FocusHandle,
    rail: RailBar,
    theme_override: Option<TreeTheme>,
    frame_theme: TreeTheme,
    washes: TreeWashes,
    label: SharedString,
    hooks: Hooks,
    owner_menu_items: Vec<OwnerMenuItem>,
    menu: Popup<OpenMenu>,
    rename: Option<RenameState>,
    drag: Option<DragSession>,
    drag_task: Option<Task<()>>,
    drag_bounds: Option<Bounds<Pixels>>,
    drag_pointer: Option<Point<Pixels>>,
    last_tick: Option<Instant>,
    reveal: RevealCoordinator,
    active_reveal: Option<RevealRequest>,
    context: u64,
    mutation_in_flight: bool,
    root_drop_row: bool,
    drag_items: Option<(u64, Rc<[DragItem]>)>,
    version: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TreeEvent> for Tree {}

impl Focusable for Tree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Tree {
    /// An empty tree with an unloaded, expanded root. Configure the owner callbacks, then call
    /// [`Self::load`] to start loading.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(ROW_HEIGHT * 4.0));
        list.set_estimated_item_height(Some(px(ROW_HEIGHT)));
        let subscriptions = vec![cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.suspend_interactions(window, cx);
            }
        })];
        Self {
            model: TreeModel::new(),
            list,
            rows: Vec::new(),
            rows_revision: 0,
            focus_handle: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            rail: RailBar::new(),
            theme_override: None,
            frame_theme: TreeTheme::current(cx),
            washes: TreeWashes::default(),
            label: SharedString::new_static("Files"),
            hooks: Hooks::default(),
            owner_menu_items: Vec::new(),
            menu: Popup::new(),
            rename: None,
            drag: None,
            drag_task: None,
            drag_bounds: None,
            drag_pointer: None,
            last_tick: None,
            reveal: RevealCoordinator::new(),
            active_reveal: None,
            context: 0,
            mutation_in_flight: false,
            root_drop_row: false,
            drag_items: None,
            version: 0,
            _subscriptions: subscriptions,
        }
    }

    // ---- configuration ----------------------------------------------------------------------

    /// The entry identity icon: `(kind, name, expanded)` to a 14 px element.
    pub fn identity(
        mut self,
        identity: impl Fn(EntryKind, &str, bool, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.hooks.identity = Some(Rc::new(identity));
        self
    }

    /// The owner decoration colour of an entry `(path, is_dir)`, for example a version-control
    /// status; it wins over the selection-dependent text colour.
    pub fn decoration(mut self, decoration: impl Fn(&str, bool) -> Option<Hsla> + 'static) -> Self {
        self.hooks.decoration = Some(Rc::new(decoration));
        self
    }

    /// Called when a file or symlink is opened (plain click, Enter or Space).
    pub fn on_open(mut self, hook: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.hooks.on_open = Some(Rc::new(hook));
        self
    }

    /// Called when a directory page is needed (expansion, retry, load-more). Answer with
    /// [`Self::apply_page`] or [`Self::fail_load`] carrying the request's generation.
    pub fn load_directory(
        mut self,
        hook: impl Fn(&LoadRequest, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.hooks.load_directory = Some(Rc::new(hook));
        self
    }

    /// Called when the context menu opens, with the action paths and the position.
    pub fn on_context_menu(
        mut self,
        hook: impl Fn(&[String], Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.hooks.on_context_menu = Some(Rc::new(hook));
        self
    }

    /// Called when a rename commit is valid: `(path, new_path)`. Answer with
    /// [`Self::finish_rename`]; relocate the subtree with [`Self::relocate_subtree`] on success.
    pub fn on_rename(mut self, hook: impl Fn(&str, &str, &mut Window, &mut App) + 'static) -> Self {
        self.hooks.on_rename = Some(Rc::new(hook));
        self
    }

    /// Called when deletion of the action paths is requested. The confirmation is the
    /// owner's; [`super::delete_subject`] supplies the subject text.
    pub fn on_delete(mut self, hook: impl Fn(&[String], &mut Window, &mut App) + 'static) -> Self {
        self.hooks.on_delete = Some(Rc::new(hook));
        self
    }

    /// Called with the ordered `(from, to)` moves of a valid drop.
    pub fn on_move(
        mut self,
        hook: impl Fn(&[(String, String)], &mut Window, &mut App) + 'static,
    ) -> Self {
        self.hooks.on_move = Some(Rc::new(hook));
        self
    }

    /// The owner's mutation capability of a path `(path, deleting)`.
    pub fn can_mutate(mut self, can: impl Fn(&str, bool) -> bool + 'static) -> Self {
        self.hooks.can_mutate = Some(Rc::new(can));
        self
    }

    /// The owner's policy for a drag payload (stale revision, busy, rename or delete open).
    pub fn accepts_drag(mut self, accepts: impl Fn(&TreeDragPayload) -> bool + 'static) -> Self {
        self.hooks.accepts_drag = Some(Rc::new(accepts));
        self
    }

    /// The owner-rendered drag ghost.
    pub fn drag_ghost(
        mut self,
        ghost: impl Fn(&TreeDragPayload, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.hooks.drag_ghost = Some(Rc::new(ghost));
        self
    }

    /// Resolves an entry path to the text "Copy path" writes, by the owning host's path rules
    /// (the default writes the tree path).
    pub fn path_resolver(mut self, resolve: impl Fn(&str) -> String + 'static) -> Self {
        self.hooks.path_resolver = Some(Rc::new(resolve));
        self
    }

    /// Adds an owner-defined, non-mutating context menu item (always enabled).
    pub fn context_menu_item(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        handler: impl Fn(&[String], &mut Window, &mut App) + 'static,
    ) -> Self {
        self.owner_menu_items.push(OwnerMenuItem {
            id: id.into(),
            label: label.into(),
            handler: Rc::new(handler),
        });
        self
    }

    /// The accessible label of the tree container.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Shows the dedicated "Move to workspace root" row above the list.
    pub fn with_root_drop_row(mut self, show: bool) -> Self {
        self.root_drop_row = show;
        self
    }

    /// Replaces the theme roles (by default the active theme of the application).
    pub fn set_theme(&mut self, theme: Option<TreeTheme>, cx: &mut Context<Self>) {
        self.theme_override = theme;
        cx.notify();
    }

    /// Replaces the wash alphas.
    pub fn set_washes(&mut self, washes: TreeWashes, cx: &mut Context<Self>) {
        self.washes = washes;
        cx.notify();
    }

    /// Reports whether an owner mutation is in flight (mutations are disabled meanwhile).
    pub fn set_mutation_in_flight(&mut self, in_flight: bool, cx: &mut Context<Self>) {
        self.mutation_in_flight = in_flight;
        cx.notify();
    }

    // ---- model access -----------------------------------------------------------------------

    /// The model.
    pub fn model(&self) -> &TreeModel {
        &self.model
    }

    /// The list state backing the rows (scroll geometry, anchors).
    pub fn list_state(&self) -> &ListState {
        &self.list
    }

    /// The open context menu.
    pub fn context_menu(&self) -> Option<&ContextMenu> {
        self.menu.as_open().map(|open| &open.menu)
    }

    /// The drag in progress inside the tree.
    pub fn drag_session(&self) -> Option<&DragSession> {
        self.drag.as_ref()
    }

    /// Whether an inline rename is open.
    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    /// The path being renamed.
    pub fn renaming_path(&self) -> Option<&str> {
        self.rename.as_ref().map(|rename| rename.session.path())
    }

    /// The rename field, while a rename is open.
    pub fn rename_field(&self) -> Option<&Entity<TextField>> {
        self.rename.as_ref().map(|rename| &rename.field)
    }

    /// The message of the last rejected rename commit.
    pub fn rename_error(&self) -> Option<&str> {
        self.rename
            .as_ref()
            .and_then(|rename| rename.session.error())
    }

    /// Runs `f` on the model and keeps the list attached to the same path (R10).
    pub fn update_model<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut TreeModel) -> R,
    ) -> R {
        let result = f(&mut self.model);
        self.model_changed(cx);
        result
    }

    fn model_changed(&mut self, cx: &mut Context<Self>) {
        self.version += 1;
        if self.model.rows_revision() != self.rows_revision {
            self.rows_revision = self.model.rows_revision();
            let new = self.model.visible_rows();
            sync_list_rows(&self.list, &self.rows, new);
            self.rows = new.to_vec();
        }
        cx.notify();
    }

    fn call(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Window, &mut App) + 'static,
    ) {
        window.defer(cx, f);
    }

    fn dispatch_loads(
        &mut self,
        loads: impl IntoIterator<Item = LoadRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for load in loads {
            if let Some(hook) = self.hooks.load_directory.clone() {
                self.call(window, cx, move |window, cx| hook(&load, window, cx));
            }
        }
    }

    // ---- loading ----------------------------------------------------------------------------

    /// Starts loading the root and every expanded directory that is not loaded.
    pub fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let loads = self.model.request_loads();
        self.model_changed(cx);
        self.dispatch_loads(loads, window, cx);
    }

    /// Applies a page answering a [`LoadRequest`] (R3) and continues an active reveal.
    pub fn apply_page(
        &mut self,
        page: Page,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), PageError> {
        let result = self.model.apply_page(page, generation);
        self.model_changed(cx);
        self.advance_reveal(window, cx);
        result
    }

    /// Records a failed load; the retry row repeats the request with the same cursor.
    pub fn fail_load(
        &mut self,
        path: &str,
        message: impl Into<String>,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let failed = self.model.fail_load(path, message, generation);
        self.model_changed(cx);
        self.advance_reveal(window, cx);
        failed
    }

    /// Clears the tree and reloads the root (increments the generation).
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_rename_silently();
        self.model.reset();
        self.load(window, cx);
    }

    /// Discards in-flight pages, marks every directory stale and reloads the expanded ones.
    pub fn invalidate_loads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.invalidate_loads();
        self.load(window, cx);
    }

    /// Refreshes one directory in the background, keeping its rows while it loads.
    pub fn refresh_directory(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.invalidate_directory(path);
        let load = self
            .model
            .is_expanded(path)
            .then(|| self.model.expand(path).load)
            .flatten();
        self.model_changed(cx);
        self.dispatch_loads(load, window, cx);
    }

    /// Shows or hides ignored entries, reloading the expanded directories.
    pub fn set_include_ignored(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model.set_include_ignored(include) {
            self.load(window, cx);
        }
    }

    /// Re-keys a moved or renamed entry and its subtree (R6).
    pub fn relocate_subtree(&mut self, old: &str, new: &str, cx: &mut Context<Self>) -> bool {
        let moved = self.model.relocate_subtree(old, new);
        if moved {
            if let Some(rename) = &mut self.rename {
                if rename.session.path() == old {
                    rename.session.relocated(new);
                }
            }
        }
        self.model_changed(cx);
        moved
    }

    /// Removes an entry and its subtree.
    pub fn remove(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        let removed = self.model.remove(path);
        self.model_changed(cx);
        removed
    }

    /// Closes the menu, cancels a drag and an open rename: the owner target or the window
    /// changed.
    pub fn suspend_interactions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_context_menu(window, cx);
        if self.drag.is_some() {
            if cx.has_active_drag() {
                cx.stop_active_drag(window);
            }
            self.end_drag_session(cx);
        }
        cx.notify();
    }

    /// Sets the owner's request context (for example the checkout). A change drops an active
    /// reveal and suspends interactions.
    pub fn set_context(&mut self, context: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.context != context {
            self.context = context;
            self.active_reveal = None;
            self.suspend_interactions(window, cx);
        }
    }

    // ---- reveal -----------------------------------------------------------------------------

    /// Reveals `path` in the tree (R22): cancels an earlier reveal, loads each ancestor
    /// top-down, expands them, selects the path and scrolls it into view. What the intent
    /// asks of the owner arrives as [`TreeEvent::RevealFinished`].
    pub fn reveal_path(
        &mut self,
        path: &str,
        intent: RevealIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = self.reveal.begin(path, intent, &self.model, self.context);
        self.active_reveal = Some(request);
        self.advance_reveal(window, cx);
    }

    fn advance_reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.active_reveal.clone() else {
            return;
        };
        let progress = self.reveal.step(&request, &mut self.model, self.context);
        self.model_changed(cx);
        match progress {
            RevealProgress::Dropped => self.active_reveal = None,
            RevealProgress::Load(load) => self.dispatch_loads([load], window, cx),
            RevealProgress::Pending => {}
            RevealProgress::Failed(message) => {
                self.active_reveal = None;
                cx.emit(TreeEvent::RevealFailed(message));
            }
            RevealProgress::Missing => {
                self.active_reveal = None;
                cx.emit(TreeEvent::RevealFailed(format!(
                    "{} was not found",
                    request.path
                )));
            }
            RevealProgress::Done(effect) => {
                self.active_reveal = None;
                self.reveal_cursor();
                if let (Some(path), Some(hook)) =
                    (effect.open_file.clone(), self.hooks.on_open.clone())
                {
                    self.call(window, cx, move |window, cx| hook(&path, window, cx));
                }
                cx.emit(TreeEvent::RevealFinished {
                    path: request.path,
                    effect,
                });
            }
        }
    }

    fn reveal_cursor(&self) {
        if let Some(ix) = self
            .model
            .cursor()
            .and_then(|cursor| self.model.row_index(cursor))
        {
            self.list.scroll_to_reveal_item(ix);
        }
    }

    // ---- selection and activation -----------------------------------------------------------

    fn selection_changed(&mut self, changed: bool, cx: &mut Context<Self>) {
        self.model_changed(cx);
        if changed {
            cx.emit(TreeEvent::SelectionChanged);
        }
    }

    fn activate_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.model.activate(path) {
            Activation::Toggled(Outcome { load, .. }) => self.dispatch_loads(load, window, cx),
            Activation::Open(path) => {
                if let Some(hook) = self.hooks.on_open.clone() {
                    self.call(window, cx, move |window, cx| hook(&path, window, cx));
                }
            }
            Activation::Load(load) => self.dispatch_loads(load, window, cx),
            Activation::None => {}
        }
        self.model_changed(cx);
        self.reveal_cursor();
    }

    fn row_clicked(
        &mut self,
        ix: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if event.is_right_click() || event.is_middle_click() || self.rename.is_some() {
            return;
        }
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        let modifiers = event.modifiers();
        match row.kind {
            RowKind::Entry => {
                if modifiers.shift {
                    let changed = self.model.select_range_to(&row.path, modifiers.secondary());
                    self.selection_changed(changed, cx);
                } else if modifiers.secondary() {
                    let changed = self.model.toggle_selected(&row.path);
                    self.selection_changed(changed, cx);
                } else {
                    let changed = self.model.select(&row.path);
                    self.selection_changed(changed, cx);
                    self.activate_path(&row.path, window, cx);
                }
            }
            RowKind::LoadMore(_) | RowKind::Error(_) => {
                if !modifiers.shift && !modifiers.secondary() {
                    let changed = self.model.focus_row_path(&row.path);
                    self.selection_changed(changed, cx);
                    self.activate_path(&row.path, window, cx);
                }
            }
            RowKind::Loading | RowKind::Empty => {}
        }
    }

    fn empty_clicked(&mut self, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.is_right_click() || event.is_middle_click() {
            return;
        }
        let modifiers = event.modifiers();
        if modifiers.shift || modifiers.secondary() {
            return;
        }
        let changed = self.model.clear_selection();
        self.close_context_menu(window, cx);
        self.selection_changed(changed, cx);
    }

    fn row_secondary_pressed(
        &mut self,
        ix: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rename.is_some() {
            return;
        }
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        if !row.is_entry() {
            return;
        }
        let changed = if self.model.is_selected(&row.path) {
            self.model.set_cursor(&row.path)
        } else {
            self.model.select(&row.path)
        };
        self.selection_changed(changed, cx);
        self.open_context_menu(event.position, window, cx);
    }

    /// Handles a key press as the focused tree does (R11, R13). Returns whether it was handled.
    pub fn handle_key(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.rename.is_some() {
            return false;
        }
        if self.menu.is_open() {
            return self.handle_menu_key(classify_key(key, modifiers, true), window, cx);
        }
        let plain =
            !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.function);
        if !plain {
            return false;
        }
        let shift = modifiers.shift;
        let mut navigated = false;
        match key {
            "down" | "up" => {
                let delta = if key == "down" { 1 } else { -1 };
                let changed = if shift {
                    self.model.extend_selection(delta)
                } else if delta > 0 {
                    self.model.select_next()
                } else {
                    self.model.select_previous()
                };
                self.selection_changed(changed, cx);
                navigated = true;
            }
            "left" if !shift => {
                let outcome = self.model.select_parent();
                self.dispatch_loads(outcome.load, window, cx);
                self.selection_changed(outcome.changed, cx);
                navigated = true;
            }
            "right" if !shift => {
                let outcome = self.model.select_first_child();
                self.dispatch_loads(outcome.load, window, cx);
                self.selection_changed(outcome.changed, cx);
                navigated = true;
            }
            "enter" | "space" if !shift => {
                if let Some(cursor) = self.model.cursor().map(str::to_owned) {
                    self.activate_path(&cursor, window, cx);
                }
            }
            "escape" if !shift => {
                if self.drag.is_some() || cx.has_active_drag() {
                    cx.stop_active_drag(window);
                    self.end_drag_session(cx);
                } else {
                    let changed = self.model.collapse_selection();
                    self.selection_changed(changed, cx);
                }
            }
            "f2" if !shift => {
                self.begin_rename(window, cx);
            }
            "delete" if !shift => {
                self.request_delete(window, cx);
            }
            "menu" => {
                let origin = self.list.viewport_bounds().origin;
                self.open_context_menu(origin, window, cx);
            }
            "f10" if shift => {
                let origin = self.list.viewport_bounds().origin;
                self.open_context_menu(origin, window, cx);
            }
            _ => return false,
        }
        if navigated {
            self.reveal_cursor();
        }
        true
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let handled = self.handle_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers,
            window,
            cx,
        );
        if handled {
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    // ---- actions ----------------------------------------------------------------------------

    fn is_mutable_path(&self, path: &str, deleting: bool) -> bool {
        let Some(node) = self.model.node(path) else {
            return false;
        };
        let capability = self
            .hooks
            .can_mutate
            .as_ref()
            .is_none_or(|can| can(path, deleting));
        is_mutable(&node.entry, capability, self.mutation_in_flight)
    }

    /// Requests deletion of the action paths when every one is mutable.
    pub fn request_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let paths = self.model.action_paths();
        if paths.is_empty() || !paths.iter().all(|path| self.is_mutable_path(path, true)) {
            return false;
        }
        if let Some(hook) = self.hooks.on_delete.clone() {
            self.call(window, cx, move |window, cx| hook(&paths, window, cx));
        }
        true
    }

    // ---- context menu -----------------------------------------------------------------------

    /// Opens the context menu for the action paths at `position`, as a CMP-002 layer: it takes
    /// focus, owns Escape and dismisses on an outside press without clicking through. Does
    /// nothing without action paths or while renaming.
    pub fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let paths = self.model.action_paths();
        if paths.is_empty() || self.rename.is_some() {
            return false;
        }
        let rename_mutable: Vec<bool> = paths
            .iter()
            .map(|path| self.is_mutable_path(path, false))
            .collect();
        let delete_mutable: Vec<bool> = paths
            .iter()
            .map(|path| self.is_mutable_path(path, true))
            .collect();
        let owner: Vec<(String, String)> = self
            .owner_menu_items
            .iter()
            .map(|item| (item.id.clone(), item.label.clone()))
            .collect();
        let items = menu_items(&owner, paths.len(), &rename_mutable, &delete_mutable);
        if let Some(previous) = self.menu.as_open() {
            previous.layer.close(window, cx);
        }
        let view = cx.entity().downgrade();
        let escape: EscapeHandler = Rc::new(move |window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.close_context_menu(window, cx));
            }
            EscapeOutcome::Dismissed
        });
        let layer = open_layer(
            window,
            cx,
            LayerOptions {
                escape,
                layer_focus: self.menu_focus.clone(),
                trigger: Some(self.focus_handle.clone()),
                initial_focus: FocusTarget::Default,
                final_focus: FocusTarget::Default,
                trigger_first: false,
            },
        );
        let now = MotionClock::now_ms(cx);
        self.menu.open(
            OpenMenu {
                menu: ContextMenu::new(paths.clone(), items),
                position,
                layer,
            },
            now,
        );
        if let Some(hook) = self.hooks.on_context_menu.clone() {
            self.call(window, cx, move |window, cx| {
                hook(&paths, position, window, cx)
            });
        }
        cx.notify();
        true
    }

    /// Closes the context menu: its layer ends (Escape stops reaching it, focus returns to the
    /// tree) and the exit phase starts.
    pub fn close_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.menu.as_open() {
            open.layer.close(window, cx);
        }
        close_popup(self, cx, |this| &mut this.menu);
    }

    fn set_menu_active(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(open) = self.menu.as_open_mut() {
            open.menu.set_active(Some(ix));
            cx.notify();
        }
    }

    fn handle_menu_key(
        &mut self,
        key: ListKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.menu.as_open_mut() else {
            return false;
        };
        match key {
            ListKey::Down => open.menu.next(),
            ListKey::Up => open.menu.previous(),
            ListKey::Home => open.menu.first(),
            ListKey::End => open.menu.last(),
            ListKey::Enter | ListKey::SecondaryEnter => {
                if let Some(ix) = open.menu.active() {
                    self.activate_menu_item(ix, window, cx);
                }
                return true;
            }
            ListKey::Escape => {
                self.close_context_menu(window, cx);
                return true;
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Dispatches the enabled item `ix` of the open menu and closes the menu.
    pub fn activate_menu_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu.as_open() else {
            return;
        };
        let paths = open.menu.paths.clone();
        let item = open.menu.items.get(ix).filter(|item| item.enabled).cloned();
        self.close_context_menu(window, cx);
        let Some(item) = item else {
            return;
        };
        match item.id.as_str() {
            COPY_PATH => {
                let resolve = self.hooks.path_resolver.clone();
                let text = copy_path_text(&paths, |path| match &resolve {
                    Some(resolve) => resolve(path),
                    None => path.to_owned(),
                });
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            RENAME => {
                self.begin_rename(window, cx);
            }
            DELETE => {
                self.request_delete(window, cx);
            }
            id => {
                if let Some(owner) = self.owner_menu_items.iter().find(|owner| owner.id == id) {
                    let handler = owner.handler.clone();
                    self.call(window, cx, move |window, cx| handler(&paths, window, cx));
                }
            }
        }
    }

    // ---- rename -----------------------------------------------------------------------------

    /// Begins renaming the single action path (F2, the menu item or an owner call); allowed
    /// only for a mutable entry. Selects and reveals the entry and focuses a field whose
    /// selection covers the basename.
    pub fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.rename.is_some() {
            return false;
        }
        let paths = self.model.action_paths();
        let [path] = paths.as_slice() else {
            return false;
        };
        if !self.is_mutable_path(path, false) {
            return false;
        }
        let Some(entry) = self.model.node(path).map(|node| node.entry.clone()) else {
            return false;
        };
        self.close_context_menu(window, cx);
        let changed = self.model.select(path);
        self.selection_changed(changed, cx);
        self.reveal_cursor();
        let theme = self.current_theme(cx);
        let field_theme = theme.field_theme();
        let range = basename_range(&entry.name, entry.is_dir());
        let field = cx.new(|cx| {
            TextField::new(window, cx)
                .chromeless()
                .theme(field_theme)
                .text_metrics(px(NAME_SIZE), px(RENAME_LINE_HEIGHT))
                .accessible_label("Rename")
                .value(&entry.name)
        });
        field.update(cx, |field, cx| {
            field.set_selected_range(range, false, window, cx);
        });
        let focus = field.focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &field,
                window,
                |this, _, event: &TextFieldEvent, window, cx| match event {
                    TextFieldEvent::Submitted(_) => this.commit_rename(window, cx),
                    TextFieldEvent::Cancelled => this.cancel_rename(window, cx),
                    _ => {}
                },
            ),
            cx.on_blur(&focus, window, |this, window, cx| {
                this.rename_blurred(window, cx)
            }),
        ];
        self.rename = Some(RenameState {
            session: RenameSession::new(entry.path.clone(), entry.revision.clone()),
            field,
            _subscriptions: subscriptions,
        });
        window.focus(&focus, cx);
        cx.notify();
        true
    }

    /// Commits the rename field (Enter).
    pub fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = &mut self.rename else {
            return;
        };
        let name = rename.field.read(cx).value_text().to_owned();
        let path = rename.session.path().to_owned();
        let current_revision = self
            .model
            .node(&path)
            .and_then(|node| node.entry.revision.clone());
        match rename.session.commit(&name, current_revision.as_deref()) {
            RenameCommit::Ignored => {}
            RenameCommit::Unchanged => self.cancel_rename(window, cx),
            RenameCommit::Rejected(message) => {
                cx.emit(TreeEvent::RenameRejected(message));
                cx.notify();
            }
            RenameCommit::Request { from, to } => {
                if let Some(hook) = self.hooks.on_rename.clone() {
                    self.call(window, cx, move |window, cx| hook(&from, &to, window, cx));
                }
                cx.notify();
            }
        }
    }

    /// The owner answered a rename request: on success the rename closes and the tree takes
    /// focus; on failure the message is kept and the commit can be retried.
    pub fn finish_rename(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = &mut self.rename else {
            return;
        };
        let failed = result.clone().err();
        if rename.session.finish(result) {
            self.close_rename(true, window, cx);
        } else {
            if let Some(message) = failed {
                cx.emit(TreeEvent::RenameRejected(message));
            }
            cx.notify();
        }
    }

    /// Cancels the rename (Escape) and returns focus to the tree.
    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_rename(true, window, cx);
    }

    fn rename_blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .rename
            .as_ref()
            .is_some_and(|rename| rename.session.cancels_on_blur())
        {
            self.close_rename(false, window, cx);
        }
    }

    fn close_rename(&mut self, focus_tree: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.take().is_some() {
            if focus_tree {
                window.focus(&self.focus_handle, cx);
            }
            cx.notify();
        }
    }

    fn cancel_rename_silently(&mut self) {
        self.rename = None;
    }

    // ---- drag and drop ----------------------------------------------------------------------

    fn drop_hit(&self, position: Point<Pixels>) -> DropHit {
        let Some(bounds) = self.drag_bounds else {
            return DropHit::Outside;
        };
        if !bounds.contains(&position) {
            return DropHit::Outside;
        }
        let viewport = self.list.viewport_bounds();
        if self.root_drop_row && position.y < viewport.top() {
            return DropHit::RootRow;
        }
        if position.x >= bounds.right() - px(RAIL_EXCLUSION) {
            return DropHit::Rail;
        }
        let first = self.list.logical_scroll_top().item_ix;
        for ix in first..self.rows.len() {
            match self.list.bounds_for_item(ix) {
                Some(item) if item.contains(&position) => return DropHit::Row(ix),
                Some(item) if item.top() > position.y => break,
                Some(_) => {}
                None => break,
            }
        }
        DropHit::Empty
    }

    fn policy_accepts(&self, payload: &TreeDragPayload) -> bool {
        self.hooks
            .accepts_drag
            .as_ref()
            .is_none_or(|accepts| accepts(payload))
    }

    fn accepts_payload(&self, payload: &TreeDragPayload, destination: &str) -> bool {
        destination_paths(&payload.items, destination).is_some() && self.policy_accepts(payload)
    }

    fn refresh_drag_destination(&mut self, cx: &mut Context<Self>) {
        let Some(pointer) = self.drag_pointer else {
            return;
        };
        let hit = self.drop_hit(pointer);
        let destination = resolve_destination(&self.model, hit);
        let Some(session) = self.drag.as_ref() else {
            return;
        };
        let valid = destination
            .as_deref()
            .is_some_and(|destination| self.accepts_payload(&session.payload, destination));
        let expandable = matches!(hit, DropHit::Row(_))
            && destination.as_deref().is_some_and(|destination| {
                !destination.is_empty()
                    && !self.model.is_expanded(destination)
                    && self
                        .model
                        .node(destination)
                        .is_some_and(|node| node.entry.is_dir())
            });
        let viewport = self.list.viewport_bounds();
        let changed =
            {
                let session = self.drag.as_mut().expect("checked above");
                session.set_pointer_y(Some(f32::from(pointer.y)).filter(|y| {
                    *y >= f32::from(viewport.top()) && *y <= f32::from(viewport.bottom())
                }));
                session.set_destination(destination, valid, expandable)
            };
        if changed {
            cx.notify();
        }
    }

    fn drag_moved(
        &mut self,
        event: &DragMoveEvent<TreeDragPayload>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.rename.is_some() {
            // Drop targets are disabled while renaming.
            return;
        }
        self.drag_bounds = Some(event.bounds);
        let position = event.event.position;
        if !event.bounds.contains(&position) {
            // Leaving the tree stops the tick and clears the highlight.
            if self.drag.is_some() {
                self.end_drag_session(cx);
            }
            return;
        }
        if self.menu.is_mounted() {
            self.close_context_menu(window, cx);
        }
        let payload = event.drag(cx).clone();
        if self.drag.is_none() {
            self.drag = Some(DragSession::new(payload));
        }
        self.drag_pointer = Some(position);
        self.refresh_drag_destination(cx);
        self.ensure_drag_task(window, cx);
    }

    fn ensure_drag_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag_task.is_some() {
            return;
        }
        self.last_tick = Some(cx.background_executor().now());
        self.drag_task = Some(
            cx.spawn_in(window, async move |this: WeakEntity<Tree>, cx| {
                loop {
                    cx.background_executor().timer(DRAG_TICK).await;
                    let alive = this
                        .update_in(cx, |this, window, cx| this.drag_tick(window, cx))
                        .unwrap_or(false);
                    if !alive {
                        break;
                    }
                }
            }),
        );
    }

    /// One 16 ms tick of a drag inside the tree (R20): autoscroll, destination re-resolution
    /// and hover-to-expand. Returns whether the drag continues.
    pub fn drag_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.drag.is_none() {
            return false;
        }
        if !cx.has_active_drag() {
            self.end_drag_session(cx);
            return false;
        }
        let compatible = self
            .drag
            .as_ref()
            .is_some_and(|session| self.policy_accepts(&session.payload));
        if !compatible || self.rename.is_some() {
            // The owner's compatibility check changed: stop the tick and clear the highlight.
            self.end_drag_session(cx);
            return false;
        }
        let now = cx.background_executor().now();
        let elapsed = now.saturating_duration_since(self.last_tick.unwrap_or(now));
        self.last_tick = Some(now);
        self.refresh_drag_destination(cx);
        let viewport = self.list.viewport_bounds();
        let Some(session) = self.drag.as_mut() else {
            return false;
        };
        let output = session.tick(
            elapsed,
            f32::from(viewport.top()),
            f32::from(viewport.bottom()),
        );
        if output.scroll != 0.0 {
            self.list.scroll_by(px(output.scroll));
            cx.notify();
        }
        if let Some(directory) = output.expand {
            let outcome = self.model.expand(&directory);
            self.model_changed(cx);
            self.dispatch_loads(outcome.load, window, cx);
        }
        true
    }

    fn end_drag_session(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.drag_task = None;
        self.drag_pointer = None;
        self.last_tick = None;
        cx.notify();
    }

    /// Drops `payload` on the destination under the pointer: the drag state clears first, then
    /// one ordered list of moves goes to the owner. Returns whether moves were issued.
    pub fn drop_payload(
        &mut self,
        payload: &TreeDragPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let destination = self
            .drag
            .as_ref()
            .and_then(|session| session.destination().map(str::to_owned));
        self.end_drag_session(cx);
        if self.rename.is_some() {
            return false;
        }
        let Some(destination) = destination else {
            return false;
        };
        if !self.accepts_payload(payload, &destination) {
            return false;
        }
        let Some(moves) = destination_paths(&payload.items, &destination) else {
            return false;
        };
        if let Some(hook) = self.hooks.on_move.clone() {
            self.call(window, cx, move |window, cx| hook(&moves, window, cx));
        }
        true
    }

    // ---- rendering --------------------------------------------------------------------------

    fn current_theme(&self, cx: &App) -> TreeTheme {
        self.theme_override
            .clone()
            .unwrap_or_else(|| TreeTheme::current(cx))
    }

    fn drag_items_for(&mut self) -> Rc<[DragItem]> {
        if let Some((version, items)) = &self.drag_items {
            if *version == self.version {
                return items.clone();
            }
        }
        let items = TreeDragPayload::selection_items(&self.model);
        self.drag_items = Some((self.version, items.clone()));
        items
    }

    fn guides(&self, levels: usize) -> Vec<AnyElement> {
        (0..levels)
            .map(|level| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(guide_x(level)))
                    .w(px(GUIDE_WIDTH))
                    .bg(self.frame_theme.border)
                    .into_any_element()
            })
            .collect()
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        match &row.kind {
            RowKind::Entry => self.render_entry_row(ix, &row, window, cx),
            _ => self.render_status_row(ix, &row, window, cx),
        }
    }

    fn render_status_row(
        &mut self,
        ix: usize,
        row: &VisibleRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.frame_theme.clone();
        let (label, color, clickable) = match &row.kind {
            RowKind::Loading => ("Loading…".to_owned(), theme.text_muted, false),
            RowKind::Empty => ("Empty folder".to_owned(), theme.text_faint, false),
            RowKind::Error(message) => (format!("{message} — Retry"), theme.danger, true),
            RowKind::LoadMore(_) => ("Load more…".to_owned(), theme.text_muted, true),
            RowKind::Entry => unreachable!("entry rows are rendered separately"),
        };
        let focused = self.focus_handle.contains_focused(window, cx);
        let is_cursor = self.model.cursor() == Some(row.path.as_str());
        let visual = RowVisual {
            focused,
            is_cursor,
            ..RowVisual::default()
        };
        let alpha = self.washes.row_alpha(&visual);
        let mut element = div()
            .id(("tree-status", ix))
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(px(row_padding_left(row.depth + 1)))
            .pr(px(ROW_PADDING_RIGHT))
            .text_size(px(STATUS_SIZE))
            .text_color(color)
            .when_some(alpha, |row, alpha| row.bg(theme.wash(alpha)))
            .children(self.guides(row.depth))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            );
        element = if clickable {
            element
                .cursor(CursorStyle::PointingHand)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.row_clicked(ix, event, window, cx)
                }))
        } else {
            element.on_click(|_, _, cx| cx.stop_propagation())
        };
        element.into_any_element()
    }

    fn render_entry_row(
        &mut self,
        ix: usize,
        row: &VisibleRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.frame_theme.clone();
        let Some(node) = self.model.node(&row.path) else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let entry = node.entry.clone();
        let is_dir = entry.is_dir();
        let expanded = is_dir && self.model.is_expanded(&row.path);
        let selected = self.model.is_selected(&row.path);
        let focused = self.focus_handle.contains_focused(window, cx);
        let is_cursor = self.model.cursor() == Some(row.path.as_str());
        let drop_target = self
            .drag
            .as_ref()
            .filter(|session| session.is_valid())
            .and_then(|session| session.destination())
            .is_some_and(|destination| destination == row.path)
            && is_dir;
        let visual = RowVisual {
            selected,
            focused,
            is_cursor,
            multiple_selected: self.model.selection_len() > 1,
            hovered: false,
            drop_target,
        };
        let base_alpha = self.washes.row_alpha(&visual);
        let hover_alpha = (!selected && !drop_target).then_some(self.washes.hover);
        let decoration = self
            .hooks
            .decoration
            .as_ref()
            .and_then(|decoration| decoration(&row.path, is_dir));
        let ignored = self.model.is_ignored(&row.path);
        let text_color = decoration.unwrap_or(if selected {
            theme.text
        } else {
            theme.text_muted
        });
        let faded = ignored && decoration.is_none();
        let renaming = self.renaming_path() == Some(row.path.as_str());
        let icon = match &self.hooks.identity {
            Some(identity) => identity(entry.kind, &entry.name, expanded, window, cx),
            None => default_identity(entry.kind, expanded, &theme),
        };
        let disclosure = div().w(px(DISCLOSURE_SLOT)).h_full().flex_none();
        let slot = if is_dir {
            disclosure
                .items_center()
                .justify_center()
                .flex()
                .child(chevron(expanded, px(CHEVRON_SIZE), theme.text_muted))
        } else {
            disclosure
        };
        let name: AnyElement = if renaming {
            let field = self.rename.as_ref().expect("renaming").field.clone();
            let border = if self.rename_error().is_some() {
                theme.danger
            } else {
                theme.accent
            };
            div()
                .id("tree-rename")
                .flex_1()
                .min_w_0()
                .h(px(RENAME_HEIGHT))
                .px(px(RENAME_PADDING))
                .flex()
                .items_center()
                .rounded(px(RENAME_RADIUS))
                .border_1()
                .border_color(border)
                .bg(theme.input)
                .text_color(theme.text)
                .text_size(px(NAME_SIZE))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(div().flex_1().min_w_0().child(field))
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(NAME_SIZE))
                .text_color(text_color)
                .child(entry.name.clone())
                .into_any_element()
        };
        let payload = if renaming {
            None
        } else if TreeDragPayload::drags_selection(&self.model, &row.path) {
            let items = self.drag_items_for();
            TreeDragPayload::dragged_item(&self.model, &row.path)
                .map(|dragged| TreeDragPayload { dragged, items })
        } else {
            TreeDragPayload::for_drag(&self.model, &row.path)
        };
        let ghost = self.hooks.drag_ghost.clone();
        let ghost_theme = theme.clone();
        let aria_name: SharedString = entry.name.into();
        let element: Stateful<_> = div()
            .id(("tree-row", ix))
            .role(Role::TreeItem)
            .aria_label(aria_name)
            .aria_selected(selected)
            .when(is_dir, |row| row.aria_expanded(expanded))
            .relative()
            .flex()
            .items_center()
            .gap(px(GAP))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(px(row_padding_left(row.depth)))
            .pr(px(ROW_PADDING_RIGHT))
            .when_some(base_alpha, |row, alpha| row.bg(theme.wash(alpha)))
            .when_some(hover_alpha, |row, alpha| {
                let hover = theme.wash(alpha);
                row.hover(move |style| style.bg(hover))
            })
            .children(self.guides(row.depth))
            .child(slot)
            .child(
                div()
                    .size(px(ICON_SIZE))
                    .flex_none()
                    .when(faded, |icon| icon.opacity(IGNORED_OPACITY))
                    .child(icon),
            )
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .flex_1()
                    .when(faded && !renaming, |name| name.opacity(IGNORED_OPACITY))
                    .child(name),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.row_clicked(ix, event, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.row_secondary_pressed(ix, event, window, cx)
                }),
            );
        match payload {
            Some(payload) => element
                .on_drag(payload, move |payload, _, _, cx| {
                    cx.new(|_| DragGhost {
                        payload: payload.clone(),
                        factory: ghost.clone(),
                        theme: ghost_theme.clone(),
                    })
                })
                .into_any_element(),
            None => element.into_any_element(),
        }
    }

    fn render_root_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.root_drop_row {
            return None;
        }
        let theme = &self.frame_theme;
        let valid = self.drag.as_ref().is_some_and(DragSession::is_valid);
        let active = valid
            && self
                .drag
                .as_ref()
                .is_some_and(|session| session.destination() == Some(""));
        let label = if active {
            "Move to workspace root"
        } else {
            "Workspace root"
        };
        Some(
            div()
                .id("tree-root-drop")
                .flex()
                .items_center()
                .w_full()
                .h(px(ROOT_ROW_HEIGHT))
                .px(px(ROW_PADDING_LEFT_PX))
                .text_size(px(STATUS_SIZE))
                .text_color(if active && valid {
                    theme.text
                } else {
                    theme.text_muted
                })
                .when(active, |row| row.bg(theme.wash(self.washes.root_drop)))
                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(label)
                .into_any_element(),
        )
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.menu.get()?;
        let tokens = ComponentTokens::current(cx);
        let position = open.position;
        let dismiss = {
            let view = cx.entity().downgrade();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                if let Some(view) = view.upgrade() {
                    view.update(cx, |this, cx| this.close_context_menu(window, cx));
                }
            })
        };
        let mut menu = anchored_menu_at("tree-context-menu", position)
            .motion(SurfaceMotion::now(
                &self.menu,
                Placement::At(position).side(),
                cx,
            ))
            .track_focus(&self.menu_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = classify_key(&event.keystroke.key, event.keystroke.modifiers, true);
                if this.handle_menu_key(key, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .outside_press(OutsidePress {
                layer: open.layer.id(),
                on_dismiss: dismiss,
                consume: true,
                exempt: Vec::new(),
            })
            .min_w(px(168.0));
        for (ix, item) in open.menu.items.iter().enumerate() {
            let entry = PopupMenuItem {
                label: SharedString::from(item.label.clone()),
                disabled: !item.enabled,
            };
            let active = open.menu.active() == Some(ix);
            let row = menu_item_row(("tree-menu-item", ix), &entry, active, &tokens).when(
                item.enabled,
                |row| {
                    row.cursor(CursorStyle::PointingHand)
                        .on_mouse_move(cx.listener(move |this, _, _, cx| {
                            this.set_menu_active(ix, cx);
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.activate_menu_item(ix, window, cx);
                        }))
                },
            );
            menu = menu.child(row);
        }
        Some(menu.into_any_element())
    }
}

const ROW_PADDING_LEFT_PX: f32 = super::style::ROW_PADDING_LEFT;

/// The default entry identity icon: a folder or file glyph in neutral colours.
pub fn default_identity(kind: EntryKind, expanded: bool, theme: &TreeTheme) -> AnyElement {
    let base = div()
        .size(px(ICON_SIZE))
        .flex()
        .items_center()
        .justify_center();
    match kind {
        EntryKind::Directory => base
            .child(
                div()
                    .w(px(12.0))
                    .h(px(if expanded { 9.0 } else { 10.0 }))
                    .rounded(px(2.0))
                    .bg(theme.accent.opacity(if expanded { 0.55 } else { 0.8 })),
            )
            .into_any_element(),
        EntryKind::File => base
            .child(
                div()
                    .w(px(9.0))
                    .h(px(12.0))
                    .rounded(px(1.5))
                    .border_1()
                    .border_color(theme.text_muted),
            )
            .into_any_element(),
        EntryKind::Symlink => base
            .child(
                div()
                    .w(px(9.0))
                    .h(px(12.0))
                    .rounded(px(1.5))
                    .border_1()
                    .border_color(theme.accent),
            )
            .into_any_element(),
    }
}

struct DragGhost {
    payload: TreeDragPayload,
    factory: Option<GhostFn>,
    theme: TreeTheme,
}

impl Render for DragGhost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(factory) = &self.factory {
            return factory(&self.payload, window, cx);
        }
        let label = match self.payload.items.len() {
            1 => super::model::file_name(&self.payload.dragged.path).to_owned(),
            count => format!("{count} items"),
        };
        div()
            .px(px(8.0))
            .h(px(22.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .border_1()
            .border_color(self.theme.border_strong)
            .bg(self.theme.overlay)
            .text_size(px(NAME_SIZE))
            .text_color(self.theme.text)
            .child(label)
            .into_any_element()
    }
}

impl Render for Tree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame_theme = self.current_theme(cx);
        let theme = self.frame_theme.clone();
        let list_state = self.list.clone();
        let fade_list = self.list.clone();
        let dragging = self.drag.as_ref().map(DragSession::is_valid);
        let root_active = self
            .drag
            .as_ref()
            .is_some_and(|session| session.is_valid() && session.destination() == Some(""));
        let root_wash =
            (root_active && !self.root_drop_row).then(|| theme.wash(self.washes.root_drop));
        let rows = list(
            list_state,
            cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
        )
        .size_full();
        let faded = edge_faded(px(FADE_BAND), true, true, rows)
            .fade_overflow_y_with(move |_, _| list_fade_flags(&fade_list));
        // The overlay rail reads its geometry from the list state (R9), never from row counts.
        let rail = render_rail(
            "tree-rail",
            Rc::new(ListRail::new(self.list.clone(), self.rail.clone())),
            RailOrientation::Vertical,
            window,
            cx,
        );
        let list_pane = track_hover(
            div()
                .id("tree-list")
                .relative()
                .flex_1()
                .min_h_0()
                .child(faded)
                .children(rail),
            &self.rail,
        );
        let root_row = self.render_root_row(cx);
        let menu = self.render_menu(cx);
        div()
            .id("file-tree")
            .role(Role::Tree)
            .aria_label(self.label.clone())
            .key_context("FileTree")
            .track_focus(&self.focus_handle)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .text_color(theme.text)
            .when_some(root_wash, |tree, wash| tree.bg(wash))
            .when_some(dragging, |tree, valid| {
                tree.cursor(if valid {
                    CursorStyle::ClosedHand
                } else {
                    CursorStyle::OperationNotAllowed
                })
            })
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::focus_on_press))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::focus_on_press))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::focus_on_press))
            .on_click(cx.listener(Self::empty_clicked))
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, window, cx| {
                this.close_context_menu(window, cx);
            }))
            .on_drag_move(cx.listener(Self::drag_moved))
            .on_drop(cx.listener(|this, payload: &TreeDragPayload, window, cx| {
                this.drop_payload(payload, window, cx);
            }))
            .children(root_row)
            .child(list_pane)
            .children(menu)
    }
}

impl Tree {
    fn focus_on_press(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }
}
