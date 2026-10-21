//! The native searchable picker and command palette (CMP-006 R9-R18).
//!
//! A [`Picker`] is an entity the owner places anywhere; while closed it renders nothing. Open, it
//! is one dismissal layer at priority [`PRIORITY`] (above ordinary menus; menus it opens pass a
//! higher explicit priority) anchored near the top center of the window, with
//!
//! * the query field ([`TextField`], CMP-007), focused once it is attached to the dispatch tree
//!   (the layer's two-frame opening focus); [`Picker::is_claiming_focus`] says the claim is still
//!   pending so an application does not restore focus elsewhere meanwhile;
//! * optional scope tabs (Tab and Shift-Tab step with wraparound, Ctrl-1..9 select directly);
//! * sections of navigable rows built by the owner's section provider at every render, so the
//!   picker is not coupled to any data source ([`PickerSources`](super::driver::PickerSources)
//!   is the usual driver). Limits, caps, loading, error and empty states come from
//!   [`Results`]; the highlight is kept by row identity ([`PickerModel`]) and revealed with the
//!   nearest-edge rule after a keyboard step;
//! * footer states: `Showing X of Y`, loading more and error with Retry; a paged source is asked
//!   for the next page once when the list is scrolled to its end;
//! * an optional secondary panel and nested choice submenu that replace the list without losing
//!   the query, favorites or selected identity; Escape steps back one depth at a time.
//!
//! Enter activates the highlighted row once per physical press (the [`EnterLatch`]); Ctrl-Enter
//! is the secondary accept. Everything else the picker does not claim reaches the field.

use super::{
    actions::{
        Accelerator1, Accelerator2, Accelerator3, Accelerator4, Accelerator5, Accelerator6,
        Accelerator7, Accelerator8, Accelerator9, BackOut, ConfirmSecondary, MoveDown, MoveUp,
        NextTab, PICKER_CONTEXT, PreviousTab, ToggleFavorite, bind_default_keys,
    },
    highlight::{RowExtent, reveal_offset},
    model::{Depth, EscapeOutcome as DepthOutcome, PickerModel, ReopenPolicy, ScopeTab},
    results::{Footer, ListState, Paging, Results, Row, RowKey, SectionSpec},
};
use crate::floating::{
    EscapeHandler, EscapeOutcome, FocusTarget, LayerHandle, LayerOptions, OutsidePress,
    is_top_layer, open_layer, stepping::StepDirection,
};
use crate::text::{TextField, TextFieldEvent, actions::Submit};
use crate::tokens::ComponentTokens;
use crate::tooltip::text::TextTooltipExt;
use std::rc::Rc;
use vespertine::{
    Anchor, AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, ElementId, Empty,
    Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyUpEvent, MouseButton,
    ParentElement, Pixels, Render, Role, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window, anchored, canvas, deferred, div, point, prelude::FluentBuilder,
    px,
};

/// Layer priority of a palette: above ordinary menus (1); menus it opens use a higher one.
pub const PRIORITY: usize = 2;
/// The explicit layer priority of menus opened from inside a palette (pass it to
/// [`AnchoredMenu::priority`](crate::floating::menu::AnchoredMenu::priority)).
pub const MENU_PRIORITY: usize = PRIORITY + 1;
/// Default card width.
pub const WIDTH: Pixels = px(560.);
/// Card width of the compact preset (the select's compact card).
pub const COMPACT_WIDTH: Pixels = px(256.);
/// Row height.
pub const ROW_HEIGHT: f32 = 32.;
/// Height of a section header.
pub const HEADER_HEIGHT: f32 = 24.;
/// Height of an inline section error with its Retry action.
pub const ERROR_HEIGHT: f32 = 32.;
/// Padding inside the scrolling results area.
pub const LIST_PADDING: f32 = 4.;
/// Height of the footer line.
pub const FOOTER_HEIGHT: f32 = 28.;
/// The results area is at most `clamp(viewport height - 180, 100, 360)` tall.
pub const RESULTS_CHROME: f32 = 180.;
/// Minimum of the results area maximum.
pub const RESULTS_MIN: f32 = 100.;
/// Maximum of the results area maximum.
pub const RESULTS_MAX: f32 = 360.;
/// Rows visible in the compact preset before the list scrolls.
pub const COMPACT_MAX_ROWS: usize = 7;

/// The maximum height of the results area for a window of `viewport_height` (default preset).
pub fn results_max_height(viewport_height: f32) -> f32 {
    (viewport_height - RESULTS_CHROME).clamp(RESULTS_MIN, RESULTS_MAX)
}

/// Whether a section shows a header line (a title or a status).
fn has_header(section: &super::results::Section) -> bool {
    section.title.is_some() || section.status.is_some()
}

/// The extent of every row of `results` in scrolled-content coordinates, in flat order, for the
/// reveal rule (R13). Rows, headers and inline errors have fixed heights.
pub fn row_extents(results: &Results) -> Vec<RowExtent> {
    let mut extents = Vec::new();
    let mut y = LIST_PADDING;
    for section in &results.sections {
        let header_top = y;
        let header = has_header(section);
        if header {
            y += HEADER_HEIGHT;
        }
        if section.error.is_some() {
            y += ERROR_HEIGHT;
        }
        for (index, _) in section.rows.iter().enumerate() {
            extents.push(RowExtent {
                top: y,
                bottom: y + ROW_HEIGHT,
                header_top: (index == 0 && header).then_some(header_top),
                first_overall: extents.is_empty(),
            });
            y += ROW_HEIGHT;
        }
    }
    extents
}

/// The height of the scrolled content of `results`, padding included.
pub fn content_height(results: &Results) -> f32 {
    match results.state {
        ListState::Loading => 2. * LIST_PADDING + super::results::SKELETON_ROWS as f32 * ROW_HEIGHT,
        ListState::Empty { .. } => 2. * LIST_PADDING + 2. * ROW_HEIGHT,
        ListState::Rows => {
            let mut y = LIST_PADDING;
            for section in &results.sections {
                if has_header(section) {
                    y += HEADER_HEIGHT;
                }
                if section.error.is_some() {
                    y += ERROR_HEIGHT;
                }
                y += section.rows.len() as f32 * ROW_HEIGHT;
            }
            y + LIST_PADDING
        }
    }
}

/// A painted part of the picker, for bounds observers (inspection, test hosts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerPart {
    /// The card.
    Card,
    /// The scrolling results area.
    List,
    /// The row at this flat index.
    Row(usize),
    /// The section header of this section index.
    Header(usize),
    /// The scope tab at this index.
    Tab(usize),
    /// The footer line.
    Footer,
    /// The secondary panel.
    Panel,
}

/// Observer of the painted bounds of the picker's parts.
pub type PickerObserver = Rc<dyn Fn(PickerPart, Bounds<Pixels>, &mut App)>;

/// Builds the sections for a query. Called at every render and event: read the sources' state
/// and rank the local rows here.
pub type SectionProvider = Rc<dyn Fn(&str, &App) -> Vec<SectionSpec>>;

type PagingProvider = Rc<dyn Fn(&App) -> Paging>;
type RowHandler = Rc<dyn Fn(&RowKey, &mut Window, &mut Context<Picker>)>;
type QueryHandler = Rc<dyn Fn(&str, &mut Window, &mut Context<Picker>)>;
type Handler = Rc<dyn Fn(&mut Window, &mut Context<Picker>)>;
type ScopeHandler = Rc<dyn Fn(usize, &ScopeTab, &mut Window, &mut Context<Picker>)>;
type FavoriteHandler = Rc<dyn Fn(&RowKey, bool, &mut Window, &mut Context<Picker>)>;
type PanelBuilder = Rc<dyn Fn(&RowKey, &mut Window, &mut App) -> AnyElement>;

struct OpenPicker {
    layer: LayerHandle,
}

/// A searchable, paged picker or command palette. See the [module documentation](self).
pub struct Picker {
    id: ElementId,
    label: SharedString,
    field: Entity<TextField>,
    model: PickerModel,
    provider: SectionProvider,
    paging: Option<PagingProvider>,
    empty_title: SharedString,
    empty_hint: SharedString,
    reopen: ReopenPolicy,
    compact: bool,
    close_on_activate: bool,
    row_accelerators: bool,
    favorites_enabled: bool,
    font_family: Option<SharedString>,
    tokens: Option<ComponentTokens>,
    on_query: Option<QueryHandler>,
    on_scope: Option<ScopeHandler>,
    on_activate: Option<RowHandler>,
    on_secondary: Option<RowHandler>,
    on_open: Option<Handler>,
    on_close: Option<Handler>,
    on_scrolled_to_end: Option<Handler>,
    on_retry: Option<QueryHandler>,
    on_retry_page: Option<Handler>,
    on_favorite: Option<FavoriteHandler>,
    panel: Option<PanelBuilder>,
    choices: Option<PanelBuilder>,
    observer: Option<PickerObserver>,
    open: Option<OpenPicker>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    results: Results,
    at_end: bool,
    claiming_focus: bool,
    _subscriptions: Vec<Subscription>,
}

impl Picker {
    /// A closed picker named `label` (the accessible name of the card and of the field) whose
    /// sections come from `provider`.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        provider: impl Fn(&str, &App) -> Vec<SectionSpec> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        bind_default_keys(cx);
        let label = label.into();
        let field = cx.new(|cx| {
            TextField::new(window, cx)
                .placeholder("Search\u{2026}")
                .accessible_label(label.clone())
        });
        let field_focus = field.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&field, window, |this, field, event, window, cx| {
                this.on_field_event(field, event, window, cx)
            }),
            // Losing the field's focus resets the key state (R14).
            cx.on_blur(&field_focus, window, |this, _, cx| {
                this.model.latch().reset();
                cx.notify();
            }),
            cx.on_focus(&field_focus, window, |this, _, cx| {
                this.claiming_focus = false;
                cx.notify();
            }),
        ];
        Self {
            id: id.into(),
            label,
            field,
            model: PickerModel::default(),
            provider: Rc::new(provider),
            paging: None,
            empty_title: "No results".into(),
            empty_hint: "Try a different search".into(),
            reopen: ReopenPolicy::default(),
            compact: false,
            close_on_activate: true,
            row_accelerators: false,
            favorites_enabled: false,
            font_family: None,
            tokens: None,
            on_query: None,
            on_scope: None,
            on_activate: None,
            on_secondary: None,
            on_open: None,
            on_close: None,
            on_scrolled_to_end: None,
            on_retry: None,
            on_retry_page: None,
            on_favorite: None,
            panel: None,
            choices: None,
            observer: None,
            open: None,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            results: Results::build(
                &[],
                &super::results::View::Summary,
                &Paging::default(),
                "",
                "",
            ),
            at_end: false,
            claiming_focus: false,
            _subscriptions: subscriptions,
        }
    }

    /// Scope tabs (R12). Without them the picker shows the summary view only.
    pub fn tabs(mut self, tabs: Vec<ScopeTab>) -> Self {
        self.model = PickerModel::new(tabs);
        self
    }

    /// The paging state of the list (loading more, page error), read at every render.
    pub fn paging(mut self, paging: impl Fn(&App) -> Paging + 'static) -> Self {
        self.paging = Some(Rc::new(paging));
        self
    }

    /// Sets the placeholder of the query field.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        self.field
            .update(cx, |field, cx| field.set_placeholder(placeholder, cx));
    }

    /// The compact preset: the select's compact card width.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// Whether activating a row closes the picker (default true).
    pub fn close_on_activate(mut self, close: bool) -> Self {
        self.close_on_activate = close;
        self
    }

    /// Enables the nth-row accelerators: Ctrl plus a digit 1 to 9 activates the nth visible row
    /// and the rows advertise their hints. They take the place of the tab accelerators.
    pub fn row_accelerators(mut self, enabled: bool) -> Self {
        self.row_accelerators = enabled;
        self
    }

    /// Enables the favorites toggle (Ctrl-Shift-F on the highlighted row).
    pub fn favorites(mut self, enabled: bool) -> Self {
        self.favorites_enabled = enabled;
        self
    }

    /// The font family of the picker's text.
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    /// Overrides the token roles (default: [`ComponentTokens::current`]).
    pub fn tokens(mut self, tokens: ComponentTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Called with the new query text after every user edit: begin the requests of the sources.
    pub fn on_query(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_query = Some(Rc::new(handler));
        self
    }

    /// Called when the active scope tab changed (Tab, Shift-Tab, a click or an accelerator).
    pub fn on_scope(
        mut self,
        handler: impl Fn(usize, &ScopeTab, &mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_scope = Some(Rc::new(handler));
        self
    }

    /// Called with the key of the row to activate (Enter, a click, a numbered accelerator).
    pub fn on_activate(
        mut self,
        handler: impl Fn(&RowKey, &mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_activate = Some(Rc::new(handler));
        self
    }

    /// Called for the secondary accept (Ctrl-Enter); without it the chord does nothing.
    pub fn on_secondary(
        mut self,
        handler: impl Fn(&RowKey, &mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_secondary = Some(Rc::new(handler));
        self
    }

    /// Called after the picker opened (the query is whatever the reopen policy kept): start the
    /// sources' default requests.
    pub fn on_open(
        mut self,
        handler: impl Fn(&mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }

    /// Called after the picker closed, whatever the reason: clear the sources.
    pub fn on_close(
        mut self,
        handler: impl Fn(&mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }

    /// Called when the list is scrolled to its end (R15): request the next page.
    pub fn on_scrolled_to_end(
        mut self,
        handler: impl Fn(&mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_scrolled_to_end = Some(Rc::new(handler));
        self
    }

    /// Called with a section key when its inline Retry was pressed.
    pub fn on_retry(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_retry = Some(Rc::new(handler));
        self
    }

    /// Called when the footer's Retry of a failed page request was pressed.
    pub fn on_retry_page(
        mut self,
        handler: impl Fn(&mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_retry_page = Some(Rc::new(handler));
        self
    }

    /// Called when a row's favorite state was toggled.
    pub fn on_favorite(
        mut self,
        handler: impl Fn(&RowKey, bool, &mut Window, &mut Context<Picker>) + 'static,
    ) -> Self {
        self.on_favorite = Some(Rc::new(handler));
        self
    }

    /// The per-selection settings panel of a row (R16); open it with [`Picker::open_panel`].
    pub fn panel(
        mut self,
        build: impl Fn(&RowKey, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.panel = Some(Rc::new(build));
        self
    }

    /// The nested choice submenu opened from the panel ([`Picker::open_choices`]).
    pub fn choices(
        mut self,
        build: impl Fn(&RowKey, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.choices = Some(Rc::new(build));
        self
    }

    /// Reports the painted bounds of the picker's parts.
    pub fn observe_bounds(mut self, observer: PickerObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// The query field.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// The query text.
    pub fn query(&self) -> &str {
        self.model.query()
    }

    /// Whether the picker is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether the opening focus claim is still pending: the field is not yet attached to the
    /// dispatch tree and focused. An application must not restore focus elsewhere meanwhile.
    pub fn is_claiming_focus(&self) -> bool {
        self.claiming_focus
    }

    /// The highlighted row key (the first row until another is chosen).
    pub fn highlighted(&self) -> Option<&RowKey> {
        self.model.highlight().key()
    }

    /// Whether keyboard mode (focus indicators) is on.
    pub fn keyboard_mode(&self) -> bool {
        self.model.keyboard_mode()
    }

    /// The current depth.
    pub fn depth(&self) -> Depth {
        self.model.depth()
    }

    /// The favorites.
    pub fn favorite_keys(&self) -> Vec<RowKey> {
        self.model.favorites().iter().cloned().collect()
    }

    /// The selected identity.
    pub fn selected(&self) -> Option<&RowKey> {
        self.model.selected()
    }

    /// Sets the selected identity (kept across panel switches and reopening).
    pub fn set_selected(&mut self, selected: Option<RowKey>, cx: &mut Context<Self>) {
        self.model.set_selected(selected);
        cx.notify();
    }

    /// The active scope tab.
    pub fn active_tab(&self) -> usize {
        self.model.active_tab()
    }

    /// The list's scroll offset (distance from the content top).
    pub fn scroll_offset(&self) -> f32 {
        -f32::from(self.scroll.offset().y)
    }

    /// Opens the picker: applies the reopen policy, captures the focus owner, joins the
    /// dismissal stack at priority 2 and focuses the query field once it is attached (R17).
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_some() {
            return;
        }
        let clear_query = self.reopen.clear_query;
        self.model.reopen(self.reopen);
        if clear_query {
            self.field
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.at_end = false;
        let view = cx.entity().downgrade();
        let escape: EscapeHandler = Rc::new(move |window, cx| match view.upgrade() {
            Some(view) => view.update(cx, |this, cx| this.escape(window, cx)),
            None => EscapeOutcome::Dismissed,
        });
        let field_focus = self.field.read(cx).focus_handle(cx);
        let layer = open_layer(
            window,
            cx,
            LayerOptions {
                escape,
                layer_focus: self.focus.clone(),
                trigger: None,
                initial_focus: FocusTarget::Handle(field_focus),
                final_focus: FocusTarget::Default,
                trigger_first: false,
            },
        );
        self.open = Some(OpenPicker { layer });
        self.claiming_focus = true;
        cx.notify();
        if let Some(handler) = self.on_open.clone() {
            handler(window, cx);
        }
    }

    /// Closes the picker: Escape stops reaching it, focus returns to the captured owner if it is
    /// still mounted, otherwise to the window default (R17), and the close callback runs.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else {
            return;
        };
        open.layer.close(window, cx);
        self.claiming_focus = false;
        self.model.latch().reset();
        cx.notify();
        if let Some(handler) = self.on_close.clone() {
            handler(window, cx);
        }
    }

    /// Opens the settings panel of `key` in place of the list (R16). The query, favorites and
    /// selected identity stay; the list's scroll offset and highlight are restored on return.
    pub fn open_panel(&mut self, key: RowKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel.is_none() || self.open.is_none() {
            return;
        }
        let offset = self.scroll_offset();
        self.model.open_panel(key, offset);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Opens the nested choice submenu on top of the open panel.
    pub fn open_choices(&mut self, cx: &mut Context<Self>) {
        if self.choices.is_some() {
            self.model.open_choices();
            cx.notify();
        }
    }

    fn build_results(&self, cx: &App) -> Results {
        let specs = (self.provider)(self.model.query(), cx);
        let paging = self
            .paging
            .as_ref()
            .map(|paging| paging(cx))
            .unwrap_or_default();
        Results::build(
            &specs,
            &self.model.view(),
            &paging,
            &self.empty_title,
            &self.empty_hint,
        )
    }

    fn viewport_height(&self, window: &Window, results: &Results) -> f32 {
        let max = if self.compact {
            COMPACT_MAX_ROWS as f32 * ROW_HEIGHT + 2. * LIST_PADDING
        } else {
            results_max_height(f32::from(window.viewport_size().height))
        };
        content_height(results).min(max)
    }

    fn on_field_event(
        &mut self,
        _field: &Entity<TextField>,
        event: &TextFieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let TextFieldEvent::Changed(text) = event {
            self.model.set_query(text);
            self.model.take_scroll_to_top();
            self.scroll.set_offset(point(px(0.), px(0.)));
            if let Some(handler) = self.on_query.clone() {
                handler(text, window, cx);
            }
            cx.notify();
        }
    }

    /// Escape (R16, CMP-002 R7, R8): step back one depth at a time, then close.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> EscapeOutcome {
        match self.model.escape() {
            DepthOutcome::ClosedChoices => {
                cx.notify();
                EscapeOutcome::SteppedBack
            }
            DepthOutcome::ClosedPanel(restore) => {
                self.scroll
                    .set_offset(point(px(0.), px(-restore.scroll_offset)));
                let focus = self.field.read(cx).focus_handle(cx);
                focus.focus(window, cx);
                cx.notify();
                EscapeOutcome::SteppedBack
            }
            DepthOutcome::Close => {
                self.close(window, cx);
                EscapeOutcome::Dismissed
            }
        }
    }

    fn activate_row(&mut self, key: &RowKey, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handler) = self.on_activate.clone() {
            handler(key, window, cx);
        }
        if self.close_on_activate {
            self.close(window, cx);
        }
    }

    fn highlighted_row(&mut self, cx: &App) -> Option<(Results, RowKey)> {
        let results = self.build_results(cx);
        self.model.reconcile(&results)?;
        let key = self.model.highlight().key().cloned()?;
        Some((results, key))
    }

    fn step(&mut self, direction: StepDirection, window: &Window, cx: &mut Context<Self>) {
        let results = self.build_results(cx);
        if let Some(index) = self.model.step(&results, direction) {
            self.reveal(&results, index, window);
        }
        cx.notify();
    }

    fn reveal(&self, results: &Results, index: usize, window: &Window) {
        let extents = row_extents(results);
        let Some(extent) = extents.get(index).copied() else {
            return;
        };
        let viewport = self.viewport_height(window, results);
        let offset = self.scroll_offset();
        let target = reveal_offset(offset, viewport, extent);
        if (target - offset).abs() > f32::EPSILON {
            self.scroll
                .set_offset(point(self.scroll.offset().x, px(-target)));
        }
    }

    fn after_scope_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.take_scroll_to_top();
        self.scroll.set_offset(point(px(0.), px(0.)));
        let index = self.model.active_tab();
        if let (Some(handler), Some(tab)) =
            (self.on_scope.clone(), self.model.tabs().get(index).cloned())
        {
            handler(index, &tab, window, cx);
        }
        cx.notify();
    }

    fn step_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.model.tabs().len() < 2 {
            return false;
        }
        if self.model.step_tab(forward) {
            self.after_scope_change(window, cx);
        }
        true
    }

    fn accelerator(&mut self, number: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.depth() != Depth::List {
            return;
        }
        if self.row_accelerators {
            let results = self.build_results(cx);
            if let Some(key) = self.model.nth_visible(&results, number) {
                self.activate_row(&key, window, cx);
            }
        } else if self.model.select_tab_number(number) {
            self.after_scope_change(window, cx);
        }
    }

    fn secondary(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(handler) = self.on_secondary.clone() else {
            return false;
        };
        if let Some((_, key)) = self.highlighted_row(cx) {
            handler(&key, window, cx);
            if self.close_on_activate {
                self.close(window, cx);
            }
        }
        true
    }

    fn toggle_favorite(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.favorites_enabled {
            return false;
        }
        if let Some((_, key)) = self.highlighted_row(cx) {
            let now = self.model.toggle_favorite(&key);
            if let Some(handler) = self.on_favorite.clone() {
                handler(&key, now, window, cx);
            }
            cx.notify();
        }
        true
    }

    fn observe(&self, part: PickerPart) -> Option<impl IntoElement + use<>> {
        let observer = self.observer.clone()?;
        Some(
            canvas(
                move |bounds, _, cx| observer(part, bounds, cx),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    fn tabs_row(&self, tokens: &ComponentTokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.model.tabs().is_empty() {
            return None;
        }
        let active = self.model.active_tab();
        let mut row = div()
            .id((self.id.clone(), "tabs"))
            .role(Role::TabList)
            .flex()
            .flex_row()
            .gap(px(4.))
            .px(px(8.))
            .pb(px(4.));
        for (index, tab) in self.model.tabs().iter().enumerate() {
            let selected = index == active;
            row = row.child(
                div()
                    .id((
                        self.id.clone(),
                        SharedString::from(format!("tab-{}", tab.key)),
                    ))
                    .relative()
                    .role(Role::Tab)
                    .aria_label(tab.label.clone())
                    .aria_selected(selected)
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if selected {
                        tokens.text
                    } else {
                        tokens.text_muted
                    })
                    .when(selected, |tab| tab.bg(tokens.highlight))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if this.model.select_tab_number(index + 1) {
                            this.after_scope_change(window, cx);
                        }
                    }))
                    .child(tab.label.clone())
                    .children(self.observe(PickerPart::Tab(index))),
            );
        }
        Some(row.into_any_element())
    }

    fn header(
        &self,
        index: usize,
        section: &super::results::Section,
        tokens: &ComponentTokens,
    ) -> AnyElement {
        div()
            .id((
                self.id.clone(),
                SharedString::from(format!("header-{index}")),
            ))
            .relative()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(HEADER_HEIGHT))
            .px(px(8.))
            .text_size(px(11.))
            .text_color(tokens.text_muted)
            .child(section.title.clone().unwrap_or_default())
            .when_some(section.status.clone(), |header, status| {
                // The loading status is announced politely.
                header.child(
                    div()
                        .id((
                            self.id.clone(),
                            SharedString::from(format!("status-{index}")),
                        ))
                        .role(Role::Status)
                        .aria_label(status.clone())
                        .child(status),
                )
            })
            .children(self.observe(PickerPart::Header(index)))
            .into_any_element()
    }

    fn row(
        &self,
        flat: usize,
        row: &Row,
        highlighted: bool,
        tokens: &ComponentTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.model.selected() == Some(&row.key);
        let favorite = self.model.favorites().contains(&row.key);
        let keyboard = self.model.keyboard_mode();
        let key = row.key.clone();
        let hover_key = row.key.clone();
        let enabled = row.enabled;
        let hint = row.hint.clone();
        let accelerator = (self.row_accelerators && flat < 9).then(|| format!("Ctrl+{}", flat + 1));
        div()
            .id((
                self.id.clone(),
                SharedString::from(format!("row-{}", row.key)),
            ))
            .relative()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .h(px(ROW_HEIGHT))
            .px(px(8.))
            .rounded(px(4.))
            .role(Role::ListBoxOption)
            .aria_label(row.label.clone())
            .aria_selected(selected)
            .aria_disabled(!enabled)
            .when(highlighted && keyboard, |row| row.aria_active_descendant())
            .text_color(tokens.text)
            .when(highlighted, |row| row.bg(tokens.highlight))
            .when(!enabled, |row| row.opacity(tokens.disabled_opacity))
            .when(enabled, |row| {
                row.cursor_pointer()
                    // Real motion only (CMP-002 R24): hover enter after a scroll sends none.
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        let before = (
                            this.model.highlight().key().cloned(),
                            this.model.keyboard_mode(),
                        );
                        let results = this.build_results(cx);
                        this.model.pointer_over(&results, &hover_key);
                        let after = (
                            this.model.highlight().key().cloned(),
                            this.model.keyboard_mode(),
                        );
                        if before != after {
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if !matches!(event, ClickEvent::Keyboard(_)) {
                            this.activate_row(&key, window, cx);
                        }
                    }))
            })
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .child(
                div()
                    .items_baseline()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(div().truncate().child(row.label.clone()))
                    .when_some(row.secondary.clone(), |label, secondary| {
                        label.child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(tokens.text_muted)
                                .child(secondary),
                        )
                    }),
            )
            .when(favorite, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_color(tokens.accent)
                        .child("\u{2605}"),
                )
            })
            .when_some(accelerator, |row, text| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(tokens.text_muted)
                        .child(text),
                )
            })
            .when(selected, |row| {
                row.child(div().flex_none().child("\u{2713}"))
            })
            .children(self.observe(PickerPart::Row(flat)))
            .map(|row| match hint {
                Some(hint) => row.text_tooltip(hint),
                None => row,
            })
            .into_any_element()
    }

    fn skeleton_rows(&self, tokens: &ComponentTokens) -> Vec<AnyElement> {
        (0..super::results::SKELETON_ROWS)
            .map(|ix| {
                div()
                    .id((
                        self.id.clone(),
                        SharedString::from(format!("skeleton-{ix}")),
                    ))
                    .flex_none()
                    .h(px(ROW_HEIGHT))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .h(px(10.))
                            .w(px(200. - 24. * ix as f32))
                            .rounded(px(3.))
                            .bg(tokens.border),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn error_line(
        &self,
        section_key: SharedString,
        message: SharedString,
        tokens: &ComponentTokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id((
                self.id.clone(),
                SharedString::from(format!("error-{section_key}")),
            ))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(ERROR_HEIGHT))
            .px(px(8.))
            .text_size(px(12.))
            .text_color(tokens.danger)
            .child(div().truncate().child(message))
            .child(self.retry_button(
                (
                    self.id.clone(),
                    SharedString::from(format!("retry-{section_key}")),
                ),
                tokens,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if let Some(handler) = this.on_retry.clone() {
                        handler(&section_key, window, cx);
                    }
                }),
            ))
            .into_any_element()
    }

    fn retry_button(
        &self,
        id: impl Into<ElementId>,
        tokens: &ComponentTokens,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id.into())
            .role(Role::Button)
            .aria_label("Retry")
            .px(px(8.))
            .py(px(2.))
            .rounded(px(4.))
            .bg(tokens.control_surface)
            .text_color(tokens.text)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(on_click)
            .child("Retry")
    }

    fn list(
        &mut self,
        results: &Results,
        tokens: &ComponentTokens,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let max = if self.compact {
            COMPACT_MAX_ROWS as f32 * ROW_HEIGHT + 2. * LIST_PADDING
        } else {
            results_max_height(f32::from(window.viewport_size().height))
        };
        let highlighted = self.model.highlight().key().cloned();
        let mut list = div()
            .id((self.id.clone(), "list"))
            .relative()
            .role(Role::List)
            .aria_label(self.label.clone())
            .flex()
            .flex_col()
            .p(px(LIST_PADDING))
            .max_h(px(max))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        match &results.state {
            ListState::Loading => {
                for row in self.skeleton_rows(tokens) {
                    list = list.child(row);
                }
            }
            ListState::Empty { title, hint } => {
                list = list.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .h(px(2. * ROW_HEIGHT))
                        .child(div().text_color(tokens.text).child(title.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(tokens.text_muted)
                                .child(hint.clone()),
                        ),
                );
            }
            ListState::Rows => {
                let mut flat = 0;
                for (section_ix, section) in results.sections.iter().enumerate() {
                    if has_header(section) {
                        list = list.child(self.header(section_ix, section, tokens));
                    }
                    if let Some(message) = section.error.clone() {
                        list =
                            list.child(self.error_line(section.key.clone(), message, tokens, cx));
                    }
                    for row in &section.rows {
                        let is_highlighted = highlighted.as_ref() == Some(&row.key);
                        list = list.child(self.row(flat, row, is_highlighted, tokens, cx));
                        flat += 1;
                    }
                }
            }
        }
        list.children(self.observe(PickerPart::List))
            .into_any_element()
    }

    fn footer(
        &self,
        results: &Results,
        tokens: &ComponentTokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let text = results.footer.text()?;
        let is_error = matches!(results.footer, Footer::MoreError(_));
        let mut line = div()
            .id((self.id.clone(), "footer"))
            .relative()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(FOOTER_HEIGHT))
            .px(px(12.))
            .text_size(px(11.))
            .text_color(if is_error {
                tokens.danger
            } else {
                tokens.text_muted
            })
            .child(
                div()
                    .id((self.id.clone(), "footer-text"))
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .child(text),
            );
        if is_error {
            line = line.child(self.retry_button(
                (self.id.clone(), "retry-page"),
                tokens,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    if let Some(handler) = this.on_retry_page.clone() {
                        handler(window, cx);
                    }
                }),
            ));
        }
        Some(
            line.children(self.observe(PickerPart::Footer))
                .into_any_element(),
        )
    }

    fn panel_view(
        &self,
        key: &RowKey,
        choices: bool,
        tokens: &ComponentTokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let build = if choices {
            self.choices.clone()
        } else {
            self.panel.clone()
        };
        div()
            .id((self.id.clone(), "panel"))
            .relative()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(8.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(tokens.text_muted)
                    .child("Esc \u{2039} back"),
            )
            .children(build.map(|build| build(key, window, cx)))
            .children(self.observe(PickerPart::Panel))
            .into_any_element()
    }
}

macro_rules! accelerator_listeners {
    ($card:expr, $cx:expr, $( $action:ident => $number:expr ),* $(,)?) => {{
        let mut card = $card;
        $(
            card = card.on_action($cx.listener(|this: &mut Picker, _: &$action, window, cx| {
                this.accelerator($number, window, cx)
            }));
        )*
        card
    }};
}

impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = &self.open else {
            return Empty.into_any_element();
        };
        let layer = open.layer.id();
        let tokens = self
            .tokens
            .clone()
            .unwrap_or_else(|| ComponentTokens::current(cx));
        let viewport = window.viewport_size();
        let depth = self.model.depth();

        let results = self.build_results(cx);
        self.model.reconcile(&results);
        if depth == Depth::List {
            self.signal_scrolled_to_end(&results, window, cx);
        }
        self.results = results.clone();

        let width = if self.compact { COMPACT_WIDTH } else { WIDTH };
        let view = cx.entity().downgrade();
        let mut card = div()
            .id((self.id.clone(), "card"))
            .relative()
            .flex()
            .flex_col()
            .w(width)
            .max_w(viewport.width - px(16.))
            .py(px(8.))
            .bg(tokens.surface)
            .border_1()
            .border_color(tokens.border_strong)
            .rounded(tokens.surface_radius)
            .shadow_lg()
            .text_color(tokens.text)
            .text_size(px(13.))
            .role(Role::Dialog)
            .aria_label(self.label.clone())
            .key_context(if matches!(depth, Depth::Choices(_)) {
                // Both identifiers: the bindings of the picker and of its choice submenu apply
                // (`PICKER_CONTEXT` and `CHOICES_CONTEXT`).
                "Picker PickerChoices"
            } else {
                PICKER_CONTEXT
            })
            .track_focus(&self.focus)
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            // A press inside never moves focus off the field: children that are not the field
            // prevent the default focus transfer of the tracked card.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if this.model.depth() == Depth::List {
                    this.step(StepDirection::Backward, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if this.model.depth() == Depth::List {
                    this.step(StepDirection::Forward, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                if !this.step_tab(true, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                if !this.step_tab(false, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &ConfirmSecondary, window, cx| {
                if !this.secondary(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFavorite, window, cx| {
                if !this.toggle_favorite(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &BackOut, _, cx| {
                if this.model.left() {
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            // Enter reaches the field as its submit action; the picker claims it first so the
            // press activates once (R14). During a composition Enter belongs to the input method,
            // so the field's own guard handles it.
            .capture_action(cx.listener(|this, _: &Submit, window, cx| {
                if this.model.depth() != Depth::List || this.field.read(cx).marked_range().is_some()
                {
                    return;
                }
                let results = this.build_results(cx);
                if let Some(key) = this.model.enter_down(&results, false) {
                    this.activate_row(&key, window, cx);
                }
                cx.stop_propagation();
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| {
                if event.keystroke.key == "enter" {
                    this.model.latch().key_up();
                }
            }));
        card = accelerator_listeners!(
            card, cx,
            Accelerator1 => 1, Accelerator2 => 2, Accelerator3 => 3,
            Accelerator4 => 4, Accelerator5 => 5, Accelerator6 => 6,
            Accelerator7 => 7, Accelerator8 => 8, Accelerator9 => 9,
        );
        if let Some(family) = self.font_family.clone() {
            card = card.font_family(family);
        }

        match &depth {
            Depth::List => {
                // The text input node itself carries the picker's label (R18).
                card = card.child(
                    div()
                        .id((self.id.clone(), "search"))
                        .px(px(8.))
                        .pb(px(8.))
                        .child(self.field.clone()),
                );
                if let Some(tabs) = self.tabs_row(&tokens, cx) {
                    card = card.child(tabs);
                }
                card = card.child(self.list(&results, &tokens, window, cx));
                if let Some(footer) = self.footer(&results, &tokens, cx) {
                    card = card.child(footer);
                }
            }
            Depth::Panel(key) | Depth::Choices(key) => {
                let choices = matches!(depth, Depth::Choices(_));
                card = card.child(self.panel_view(key, choices, &tokens, window, cx));
            }
        }
        card = card.child(
            OutsidePress {
                layer,
                on_dismiss: Rc::new(move |window: &mut Window, cx: &mut App| {
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |this, cx| {
                            // A menu opened from the picker sits above it and owns its presses.
                            if is_top_layer(window, cx, layer) {
                                this.close(window, cx);
                            }
                        });
                    }
                }),
                consume: true,
                exempt: Vec::new(),
            }
            .probe(),
        );
        card = card.children(self.observe(PickerPart::Card));

        deferred(
            anchored()
                .position(point(viewport.width / 2., viewport.height * 0.12))
                .anchor(Anchor::TopCenter)
                .snap_to_window_with_margin(px(8.))
                .child(card),
        )
        .with_priority(PRIORITY)
        .into_any_element()
    }
}

impl Picker {
    /// Reports that the list is scrolled to its end once per arrival at the end (R15). The
    /// source set additionally allows at most one outstanding page request per generation.
    fn signal_scrolled_to_end(
        &mut self,
        results: &Results,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if results.state != ListState::Rows {
            self.at_end = false;
            return;
        }
        let max = f32::from(self.scroll.max_offset().y);
        let at_end = self.scroll_offset() >= max - 1.0;
        if at_end && !self.at_end {
            self.at_end = true;
            if let Some(handler) = self.on_scrolled_to_end.clone() {
                cx.defer_in(window, move |this, window, cx| {
                    if this.open.is_some() {
                        handler(window, cx);
                    }
                });
            }
        } else if !at_end {
            self.at_end = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::results::{Paging, Row, SectionSpec, View};

    fn results(sections: Vec<SectionSpec>) -> Results {
        Results::build(&sections, &View::Summary, &Paging::default(), "", "")
    }

    #[test]
    fn the_results_area_height_is_clamped() {
        assert_eq!(results_max_height(300.), 120.);
        assert_eq!(results_max_height(200.), 100.);
        assert_eq!(results_max_height(2000.), 360.);
        assert_eq!(results_max_height(f32::from(px(540.))), 360.);
    }

    #[test]
    fn extents_follow_headers_errors_and_the_flat_order() {
        let mut files = SectionSpec::new("files", vec![Row::new("a", "a"), Row::new("b", "b")]);
        files.title = Some("Files".into());
        let commands = SectionSpec::new("cmds", vec![Row::new("c", "c")]);
        let results = results(vec![files, commands]);
        let extents = row_extents(&results);
        assert_eq!(extents.len(), 3);
        // Padding 4, header 24, then 32-px rows.
        assert_eq!(extents[0].top, 28.);
        assert_eq!(extents[0].header_top, Some(4.));
        assert!(extents[0].first_overall);
        assert_eq!(extents[1].top, 60.);
        assert_eq!(extents[1].header_top, None);
        // The second section has no header: its first row has no header_top.
        assert_eq!(extents[2].top, 92.);
        assert_eq!(extents[2].header_top, None);
        assert!(!extents[2].first_overall);
        assert_eq!(content_height(&results), 4. + 24. + 3. * 32. + 4.);
    }

    #[test]
    fn loading_and_empty_lists_reserve_their_room() {
        let mut loading = SectionSpec::new("files", vec![]);
        loading.loading = true;
        assert_eq!(content_height(&results(vec![loading])), 8. + 4. * 32.);
        assert_eq!(content_height(&results(vec![])), 8. + 64.);
    }
}
