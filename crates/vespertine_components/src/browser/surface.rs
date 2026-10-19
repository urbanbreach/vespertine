//! The browser surface entity (CMP-022): toolbar, validation row, body (page frame or
//! placeholder) and the page-provided menu around one embedded [`WebPage`].

use super::address_input::{AddressEvent, AddressInput};
use super::menu::{MenuCard, MenuCommand, MenuState, command_for_action, menu_card};
use super::model::{allowed_navigation, normalize_address};
use super::shortcuts::{
    Back, BrowserShortcuts, CloseTab, FocusAddress, Forward, KEY_CONTEXT, NewTab, Reload,
    bind_shortcuts,
};
use super::theme::{BrowserMetrics, BrowserTheme};
use super::widgets::{globe, icon_button, tooltip};
use std::rc::Rc;
use vespertine::{
    AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton,
    NavigationDirection, ParentElement, Render, Role, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use vespertine_webview::{
    BrowserContext, MenuRequest, PageState, Presentation, WebPage, WebPageEvent, page_element,
};

/// The placeholder text of the address field.
pub const ADDRESS_PLACEHOLDER: &str = "Website or localhost:3000";

/// Notifications of a [`BrowserSurface`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserEvent {
    /// The page state, or whether a page exists or is starting, changed.
    Changed,
    /// A new surface was requested: a page link asked for a new tab (`Some(address)`), or the new
    /// tab action ran (`None`). The embedder decides what a new surface is.
    NewTab(Option<String>),
    /// The close tab action ran.
    Close,
}

/// What the body of the surface shows (R1, R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// The page frame (a native page exists and its state has no error).
    Page,
    /// The error placeholder.
    Error,
    /// The "starting browser" placeholder.
    Starting,
    /// The "enter an address" placeholder.
    Empty,
}

/// What the retry or enter-address button of a placeholder does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceholderAction {
    /// Reload as in R9.
    Retry,
    /// Focus the address field.
    EnterAddress,
}

/// The texts of the placeholder shown instead of the page (R12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placeholder {
    /// Which state it presents.
    pub kind: BodyKind,
    /// The title line.
    pub title: String,
    /// The description line.
    pub description: String,
    /// The label of the action button, when there is one; it is also its accessibility label
    /// except for the retry button, which is labelled "Retry page".
    pub action: Option<(PlaceholderAction, String)>,
    /// The faint keyboard hint under the enter-address button.
    pub hint: Option<String>,
}

/// Everything whose change counts as a change of the surface for [`BrowserEvent::Changed`].
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    state: PageState,
    startup_pending: bool,
    native_page: bool,
}

/// Opens an address in the platform's default browser (PLT-010 external-destination opening).
pub type ExternalOpener = Rc<dyn Fn(&str, &mut App)>;

/// An embedded browser: chrome around one web page.
pub struct BrowserSurface {
    page: Entity<WebPage>,
    address: Entity<AddressInput>,
    /// The page state as last seen (a mirror of the page's state, kept in sync on every change).
    pub state: PageState,
    validation: Option<String>,
    edited: bool,
    menu: Option<MenuState>,
    menu_scroll: ScrollHandle,
    theme: BrowserTheme,
    metrics: BrowserMetrics,
    shortcuts: BrowserShortcuts,
    empty_title: String,
    empty_description: String,
    opener: ExternalOpener,
    last: Snapshot,
    _subscriptions: Vec<vespertine::Subscription>,
}

impl EventEmitter<BrowserEvent> for BrowserSurface {}

impl Focusable for BrowserSurface {
    /// The surface's one focus target is the page (PLT-016 R8).
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.page.read(cx).focus_handle().clone()
    }
}

impl BrowserSurface {
    /// A surface without a page; the first navigation starts the helper. It is hidden until the
    /// embedder calls [`BrowserSurface::set_presentation`] (R16).
    pub fn new(context: BrowserContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = BrowserTheme::default();
        let page = cx.new(|cx| WebPage::new(context, window, cx));
        let address = cx.new({
            let theme = theme.clone();
            |cx| AddressInput::new(ADDRESS_PLACEHOLDER, theme, window, cx)
        });
        let address_focus = address.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&page, window, Self::page_event),
            cx.subscribe_in(&address, window, Self::address_event),
            cx.on_focus(&address_focus, window, |_, _, cx| cx.notify()),
            cx.on_blur(&address_focus, window, |_, _, cx| cx.notify()),
        ];
        let state = page.read(cx).state().clone();
        let last = Snapshot {
            state: state.clone(),
            startup_pending: false,
            native_page: false,
        };
        Self {
            page,
            address,
            state,
            validation: None,
            edited: false,
            menu: None,
            menu_scroll: ScrollHandle::new(),
            theme,
            metrics: BrowserMetrics::default(),
            shortcuts: BrowserShortcuts::default(),
            empty_title: "Enter an address".to_owned(),
            empty_description: "Open a website or a local web app beside your content.".to_owned(),
            opener: Rc::new(|url, cx| cx.open_url(url)),
            last,
            _subscriptions: subscriptions,
        }
    }

    // ---- accessors --------------------------------------------------------------------------

    /// The page entity (for embedders that need its focus handle, state or events).
    pub fn page(&self) -> &Entity<WebPage> {
        &self.page
    }

    /// The address field entity.
    pub fn address(&self) -> &Entity<AddressInput> {
        &self.address
    }

    /// The label for tab titles (R6): the page title, else the host, else "Browser".
    pub fn title(&self) -> String {
        self.state.label()
    }

    /// The validation message shown under the toolbar, if any.
    pub fn validation(&self) -> Option<&str> {
        self.validation.as_deref()
    }

    /// Whether the address text differs from the page's address after an edit (R7).
    pub fn is_edited(&self) -> bool {
        self.edited
    }

    /// The text of the address field.
    pub fn address_text<'a>(&self, cx: &'a App) -> &'a str {
        self.address.read(cx).text()
    }

    /// The open page menu, if any.
    pub fn menu(&self) -> Option<&MenuState> {
        self.menu.as_ref()
    }

    /// The current presentation.
    pub fn presentation(&self, cx: &App) -> Presentation {
        self.page.read(cx).presentation()
    }

    /// What the body shows right now (R1, R12).
    pub fn body_kind(&self, cx: &App) -> BodyKind {
        let page = self.page.read(cx);
        if self.state.error.is_some() {
            BodyKind::Error
        } else if page.has_native_page() {
            BodyKind::Page
        } else if page.startup_pending() {
            BodyKind::Starting
        } else {
            BodyKind::Empty
        }
    }

    /// The placeholder the body shows, or `None` while it shows the page (R12).
    pub fn placeholder(&self, cx: &App) -> Option<Placeholder> {
        match self.body_kind(cx) {
            BodyKind::Page => None,
            kind @ BodyKind::Error => Some(Placeholder {
                kind,
                title: "Couldn’t load this page".to_owned(),
                description: self.state.error.clone().unwrap_or_default(),
                action: Some((PlaceholderAction::Retry, "Try again".to_owned())),
                hint: None,
            }),
            kind @ BodyKind::Starting => Some(Placeholder {
                kind,
                title: "Starting browser…".to_owned(),
                description: "You can keep working while your page opens.".to_owned(),
                action: None,
                hint: None,
            }),
            kind @ BodyKind::Empty => Some(Placeholder {
                kind,
                title: self.empty_title.clone(),
                description: self.empty_description.clone(),
                action: Some((
                    PlaceholderAction::EnterAddress,
                    "Enter an address".to_owned(),
                )),
                hint: self.shortcuts.focus_address_hint(),
            }),
        }
    }

    /// Whether the back button is enabled.
    pub fn can_go_back(&self) -> bool {
        self.state.can_back
    }

    /// Whether the forward button is enabled.
    pub fn can_go_forward(&self) -> bool {
        self.state.can_forward
    }

    /// Whether reload is enabled: a page address exists (R9).
    pub fn can_reload(&self) -> bool {
        self.state.url.is_some()
    }

    /// Whether the open-in-default-browser button is enabled: the address satisfies allowed
    /// navigation (R9).
    pub fn can_open_externally(&self) -> bool {
        self.state.url.as_deref().is_some_and(allowed_navigation)
    }

    // ---- configuration ----------------------------------------------------------------------

    /// Replaces the theme roles (R2).
    pub fn set_theme(&mut self, theme: BrowserTheme, cx: &mut Context<Self>) {
        self.address
            .update(cx, |address, cx| address.set_theme(theme.clone(), cx));
        self.theme = theme;
        cx.notify();
    }

    /// Replaces the metrics of the chrome (R2, R12).
    pub fn set_metrics(&mut self, metrics: BrowserMetrics, cx: &mut Context<Self>) {
        self.metrics = metrics;
        cx.notify();
    }

    /// Replaces the placeholder text of the address field.
    pub fn set_address_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        self.address
            .update(cx, |address, cx| address.set_placeholder(placeholder, cx));
    }

    /// Replaces the title and description of the "no address yet" placeholder (R12 (c)).
    pub fn set_empty_state(
        &mut self,
        title: impl Into<String>,
        description: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.empty_title = title.into();
        self.empty_description = description.into();
        cx.notify();
    }

    /// Applies the embedder's chords (R11): they replace the defaults in the hint and are
    /// registered with the keymap unless the keymap already assigns them to another command.
    pub fn set_shortcuts(&mut self, shortcuts: BrowserShortcuts, cx: &mut Context<Self>) {
        bind_shortcuts(cx, &shortcuts);
        self.shortcuts = shortcuts;
        cx.notify();
    }

    /// Replaces how addresses are opened in the default browser (the default is the platform's
    /// external-destination opener).
    pub fn set_external_opener(&mut self, opener: impl Fn(&str, &mut App) + 'static) {
        self.opener = Rc::new(opener);
    }

    // ---- navigation -------------------------------------------------------------------------

    /// Navigates to typed text (R7). Returns whether it was accepted.
    ///
    /// A rejected input stores its message as the validation text and changes nothing else. An
    /// accepted one clears the validation, replaces the field text with the normalized address,
    /// tells the page to load it (starting the helper first if needed) and moves focus to the
    /// page.
    pub fn navigate(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match normalize_address(input) {
            Err(message) => {
                self.validation = Some(message);
                cx.notify();
                false
            }
            Ok(url) => {
                self.validation = None;
                self.close_menu(cx);
                self.address
                    .update(cx, |address, cx| address.set_text(&url, window, cx));
                self.edited = false;
                self.page
                    .update(cx, |page, cx| page.navigate_to(url, window, cx));
                self.sync(window, cx);
                true
            }
        }
    }

    /// Focuses the address field and selects its text (R10).
    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(cx);
        let handle = self.address.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.address
            .update(cx, |address, cx| address.select_all(window, cx));
        cx.notify();
    }

    /// Reload (R9): an error re-loads the page address, otherwise the engine reloads; without a
    /// native page startup restarts.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_reload() {
            return;
        }
        self.close_menu(cx);
        self.page.update(cx, |page, cx| page.reload(window, cx));
    }

    /// History back; only while the state allows it.
    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.state.can_back {
            self.page.update(cx, |page, _| page.history(false));
        }
    }

    /// History forward; only while the state allows it.
    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if self.state.can_forward {
            self.page.update(cx, |page, _| page.history(true));
        }
    }

    /// Opens the page address in the default browser, only when it satisfies allowed navigation
    /// (R5, R9).
    pub fn open_externally(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.state.url.clone()
            && allowed_navigation(&url)
        {
            let opener = self.opener.clone();
            opener(&url, cx);
        }
    }

    /// Applies a presentation (R16). A pending page menu is dismissed whenever the surface stops
    /// being live.
    pub fn set_presentation(
        &mut self,
        presentation: Presentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if presentation != Presentation::Live {
            self.close_menu(cx);
        }
        self.page.update(cx, |page, cx| {
            page.set_presentation(presentation, window, cx)
        });
    }

    /// Closes the surface (R17): hidden, startup cancelled, page and frame dropped. A later
    /// [`BrowserSurface::navigate`] starts again.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.page.update(cx, |page, cx| page.close(window, cx));
        self.sync(window, cx);
    }

    // ---- page and address events ------------------------------------------------------------

    fn page_event(
        &mut self,
        _page: &Entity<WebPage>,
        event: &WebPageEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            WebPageEvent::Changed => self.sync(window, cx),
            WebPageEvent::Menu(request) => self.open_menu(request.clone(), cx),
            WebPageEvent::NewTab(url) => cx.emit(BrowserEvent::NewTab(Some(url.clone()))),
            WebPageEvent::EvalResult(_) => {}
        }
    }

    /// Reads the page state (R8): mirrors it, refreshes the unfocused address field and emits a
    /// changed event only when something actually changed.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (state, startup_pending, native_page) = {
            let page = self.page.read(cx);
            (
                page.state().clone(),
                page.startup_pending(),
                page.has_native_page(),
            )
        };
        self.state = state;
        let (focused, text) = {
            let address = self.address.read(cx);
            (address.is_focused(window), address.text().to_owned())
        };
        if !focused
            && let Some(url) = self.state.url.clone()
            && url != text
        {
            self.address
                .update(cx, |address, cx| address.set_text(&url, window, cx));
            self.edited = false;
        } else {
            let url = self.state.url.as_deref().unwrap_or("");
            self.edited = text != url;
        }
        let snapshot = Snapshot {
            state: self.state.clone(),
            startup_pending,
            native_page,
        };
        if snapshot != self.last {
            self.last = snapshot;
            cx.emit(BrowserEvent::Changed);
        }
        cx.notify();
    }

    fn address_event(
        &mut self,
        _address: &Entity<AddressInput>,
        event: &AddressEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AddressEvent::Edited => {
                self.validation = None;
                let text = self.address.read(cx).text();
                self.edited = text != self.state.url.as_deref().unwrap_or("");
                cx.notify();
            }
            AddressEvent::Submit => {
                let text = self.address.read(cx).text().to_owned();
                self.navigate(&text, window, cx);
            }
            AddressEvent::Escape => {
                let url = self.state.url.clone().unwrap_or_default();
                self.address
                    .update(cx, |address, cx| address.set_text(&url, window, cx));
                self.edited = false;
                self.validation = None;
                let page_focus = self.page.read(cx).focus_handle().clone();
                window.focus(&page_focus, cx);
                cx.notify();
            }
        }
    }

    // ---- page menu (R14) --------------------------------------------------------------------

    fn open_menu(&mut self, request: MenuRequest, cx: &mut Context<Self>) {
        if request.items.is_empty() {
            self.page.update(cx, |page, _| page.dismiss_menu());
            return;
        }
        self.menu = Some(MenuState::new(request));
        self.menu_scroll = ScrollHandle::new();
        if let Some(menu) = &self.menu {
            self.menu_scroll.scroll_to_item(menu.active());
        }
        cx.notify();
    }

    /// Closes the open menu, if any, and tells the page ("dismiss-menu").
    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.page.update(cx, |page, _| page.dismiss_menu());
            cx.notify();
        }
    }

    /// Activates row `index` when it is enabled: sends its action, closes the menu and sends
    /// "dismiss-menu" (R14). The action "text" first reads the clipboard's plain text.
    fn activate_menu_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self
            .menu
            .as_ref()
            .and_then(|menu| menu.enabled_item(index))
            .cloned()
        else {
            return;
        };
        self.menu = None;
        match command_for_action(&item.action) {
            MenuCommand::Send(command) => {
                self.page.update(cx, |page, _| {
                    page.send_command(command);
                });
            }
            MenuCommand::Paste => self.page.update(cx, |page, cx| page.paste(cx)),
            MenuCommand::None => {}
        }
        self.page.update(cx, |page, _| page.dismiss_menu());
        cx.notify();
    }

    /// Hovering a row makes it the active one.
    fn hover_menu_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu
            && menu.active() != index
            && menu.enabled_item(index).is_some()
        {
            menu.set_active(index);
            cx.notify();
        }
    }

    /// Keyboard of an open menu: Up and Down move, Enter activates, Escape dismisses; everything
    /// else is swallowed by the caller.
    fn menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        match event.keystroke.key.as_str() {
            "down" => menu.step(true),
            "up" => menu.step(false),
            "enter" => {
                let active = menu.active();
                self.activate_menu_row(active, cx);
                return;
            }
            "escape" => {
                self.close_menu(cx);
                return;
            }
            _ => return,
        }
        self.menu_scroll.scroll_to_item(menu.active());
        cx.notify();
    }

    // ---- rendering --------------------------------------------------------------------------

    fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let metrics = &self.metrics;
        let focused = self.address.read(cx).is_focused(window);
        let show_go = focused || self.edited;
        let border = if self.validation.is_some() {
            theme.danger
        } else if focused {
            theme.strong_border
        } else {
            theme.border
        };
        let hover = theme.hover;
        let field = div()
            .id("browser-address-field")
            .debug_selector(|| "browser-address-field".to_owned())
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w_0()
            .h(metrics.address_height)
            .ml(metrics.address_margin_left)
            .mr(metrics.address_margin_right)
            .px(px(8.))
            .gap(px(6.))
            .rounded(metrics.address_radius)
            .border_1()
            .border_color(border)
            .bg(theme.page_background)
            .text_size(metrics.address_text_size)
            .line_height(metrics.address_line_height)
            .text_color(theme.text)
            .child(globe(metrics.globe_size, theme.muted))
            .child(self.address.clone())
            .when(show_go, |field| {
                field.child(
                    div()
                        .id("browser-go")
                        .debug_selector(|| "browser-go".to_owned())
                        .role(Role::Button)
                        .aria_label("Go to address")
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .size(metrics.go_button_size)
                        .rounded(px(4.))
                        .text_size(px(12.))
                        .text_color(theme.muted)
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .tooltip(tooltip("Go to address", theme))
                        // The go button never takes focus from the field (R9).
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            let text = this.address.read(cx).text().to_owned();
                            this.navigate(&text, window, cx);
                        }))
                        .child("→"),
                )
            });
        div()
            .id("browser-toolbar")
            .debug_selector(|| "browser-toolbar".to_owned())
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .h(metrics.toolbar_height)
            .px(metrics.toolbar_padding)
            .gap(metrics.toolbar_gap)
            .bg(theme.raised)
            .child(icon_button(
                "browser-back",
                "Back",
                "←",
                self.can_go_back(),
                theme,
                metrics,
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_back(cx)),
            ))
            .child(icon_button(
                "browser-forward",
                "Forward",
                "→",
                self.can_go_forward(),
                theme,
                metrics,
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_forward(cx)),
            ))
            .child(icon_button(
                "browser-reload",
                "Reload page",
                "↻",
                self.can_reload(),
                theme,
                metrics,
                cx.listener(|this, _: &ClickEvent, window, cx| this.reload(window, cx)),
            ))
            .child(field)
            .child(icon_button(
                "browser-external",
                "Open in default browser",
                "↗",
                self.can_open_externally(),
                theme,
                metrics,
                cx.listener(|this, _: &ClickEvent, _, cx| this.open_externally(cx)),
            ))
            .into_any_element()
    }

    fn render_placeholder(&self, placeholder: Placeholder, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let hover_background = theme.raised.blend(theme.hover);
        let action = placeholder.action.map(|(action, label)| {
            let accessibility_label = match action {
                PlaceholderAction::Retry => "Retry page".to_owned(),
                PlaceholderAction::EnterAddress => label.clone(),
            };
            let button: Stateful<Div> = div()
                .id("browser-placeholder-action")
                .debug_selector(|| "browser-placeholder-action".to_owned())
                .role(Role::Button)
                .aria_label(accessibility_label)
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.))
                .px(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.raised)
                .text_size(px(12.))
                .text_color(theme.text)
                .cursor_pointer()
                .hover(move |style| style.bg(hover_background))
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, window, cx| match action {
                        PlaceholderAction::Retry => this.reload(window, cx),
                        PlaceholderAction::EnterAddress => this.focus_address(window, cx),
                    }),
                )
                .child(label);
            button
        });
        div()
            .id("browser-placeholder")
            .debug_selector(|| "browser-placeholder".to_owned())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .max_w(px(300.))
                    .gap(px(12.))
                    .p(px(24.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(44.))
                            .rounded(px(12.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.raised.opacity(0.5))
                            .child(globe(px(22.), theme.muted)),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(placeholder.title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .text_center()
                            .text_color(theme.muted)
                            .child(placeholder.description),
                    )
                    .children(action)
                    .children(
                        placeholder.hint.map(|hint| {
                            div().text_size(px(10.)).text_color(theme.faint).child(hint)
                        }),
                    ),
            )
            .into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let has_address = self.state.url.is_some();
        let hidden = self.presentation(cx) == Presentation::Hidden;
        let content = match self.placeholder(cx) {
            Some(placeholder) => Some(self.render_placeholder(placeholder, cx)),
            // A hidden surface paints no page (PLT-015 R20); the backing stays.
            None if hidden => None,
            None => Some(page_element(&self.page, cx).into_any_element()),
        };
        let menu = self.menu.as_ref().map(|menu| {
            let this = cx.entity().downgrade();
            let on_activate: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new({
                let this = this.clone();
                move |index, _, cx| {
                    this.update(cx, |this, cx| this.activate_menu_row(index, cx))
                        .ok();
                }
            });
            let on_hover: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new({
                let this = this.clone();
                move |index, _, cx| {
                    this.update(cx, |this, cx| this.hover_menu_row(index, cx))
                        .ok();
                }
            });
            let on_dismiss: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |_, cx| {
                this.update(cx, |this, cx| this.close_menu(cx)).ok();
            });
            menu_card(MenuCard {
                state: menu,
                theme,
                scroll: &self.menu_scroll,
                on_activate,
                on_hover,
                on_dismiss,
            })
        });
        div()
            .id("browser-body")
            .debug_selector(|| "browser-body".to_owned())
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            // The opaque backing keeps a loading page from showing the window through it (R1, R13).
            .when(has_address, |body| body.bg(theme.page_background))
            .children(content)
            .children(menu)
            .into_any_element()
    }
}

impl Render for BrowserSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.render_toolbar(window, cx);
        let validation = self.validation.clone().map(|message| {
            div()
                .id("browser-validation")
                .debug_selector(|| "browser-validation".to_owned())
                .flex_shrink_0()
                .px(px(12.))
                .py(px(8.))
                .text_size(self.metrics.validation_text_size)
                .text_color(self.theme.danger)
                .child(message)
        });
        let body = self.render_body(cx);
        let back_button = MouseButton::Navigate(NavigationDirection::Back);
        let forward_button = MouseButton::Navigate(NavigationDirection::Forward);
        div()
            .key_context(KEY_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &Reload, window, cx| this.reload(window, cx)))
            .on_action(
                cx.listener(|this, _: &FocusAddress, window, cx| this.focus_address(window, cx)),
            )
            .on_action(cx.listener(|_, _: &NewTab, _, cx| cx.emit(BrowserEvent::NewTab(None))))
            .on_action(cx.listener(|_, _: &CloseTab, _, cx| cx.emit(BrowserEvent::Close)))
            .on_action(cx.listener(|this, _: &Back, _, cx| this.go_back(cx)))
            .on_action(cx.listener(|this, _: &Forward, _, cx| this.go_forward(cx)))
            // An open menu takes every key (R10 of PLT-016, R14).
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.menu.is_some() {
                    this.menu_key(event, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_key_up(cx.listener(|this, _: &KeyUpEvent, _, cx| {
                if this.menu.is_some() {
                    cx.stop_propagation();
                }
            }))
            // Mouse back and forward buttons over the surface do nothing (R11).
            .on_mouse_down(back_button, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(forward_button, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(back_button, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(forward_button, |_, _, cx| cx.stop_propagation())
            .child(toolbar)
            .children(validation)
            .child(body)
    }
}
