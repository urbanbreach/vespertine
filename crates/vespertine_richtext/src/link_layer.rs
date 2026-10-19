//! Link hitboxes, keyboard focus, the actions menu and the destination disclosure of a
//! [`crate::RichText`] (TXT-008 R16-R20). Rules live in [`crate::links`]; this file wires them to
//! elements.

use crate::display_map::OriginalText;
use crate::geometry::RichLayout;
use crate::links::{
    DISCLOSURE_LINE_HEIGHT, DISCLOSURE_PADDING_X, DISCLOSURE_PADDING_Y, DISCLOSURE_TEXT_SIZE,
    DISCLOSURE_WINDOW_MARGIN, DisclosureSize, FileLink, Link, LinkActionKind, LinkMenu, LinkPress,
    MENU_WIDTH, parse_file_destination, pointer_activates,
};
use crate::service::{SELECTION_ALPHA, TextService};
use crate::theme::TextTheme;
use std::cell::RefCell;
use std::rc::Rc;
use vespertine::{
    AnyElement, AnyView, App, AppContext as _, AvailableSpace, ClipboardItem, Context,
    DispatchPhase, ElementId, FocusHandle, GlobalElementId, Hsla, InteractiveElement, IntoElement,
    LinkPolicy, LinkRoutes, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred,
    div, point, prelude::FluentBuilder as _, px, rgb, size,
};

/// What activating a link asks the host to do.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkActivation {
    /// The complete original destination (never the shortened label).
    pub destination: String,
    /// The requested action.
    pub action: LinkActionKind,
    /// The destination parsed as a file with its anchors, when it is one.
    pub file: Option<FileLink>,
}

/// Link behavior of a run.
#[derive(Clone)]
pub struct LinkOptions {
    /// Called for navigation actions (open, open externally) that the policy allows. Without a
    /// callback the destination is handed to the platform's default handler.
    pub on_activate: Option<Rc<dyn Fn(&LinkActivation, &mut Window, &mut App)>>,
    /// Whether navigation is allowed for a destination; copying is always allowed. The default
    /// is the framework's [`LinkPolicy`] (explicit `http`/`https` destinations only, PLT-010 R4).
    pub navigation_allowed: Rc<dyn Fn(&str) -> bool>,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            on_activate: None,
            navigation_allowed: policy_predicate(LinkPolicy::default()),
        }
    }
}

impl LinkOptions {
    /// Sets the activation callback.
    pub fn on_activate(
        mut self,
        callback: impl Fn(&LinkActivation, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Rc::new(callback));
        self
    }

    /// Sets the navigation predicate directly.
    pub fn navigation_allowed(mut self, policy: impl Fn(&str) -> bool + 'static) -> Self {
        self.navigation_allowed = Rc::new(policy);
        self
    }

    /// Allows navigation exactly where `policy` accepts the original destination.
    pub fn policy(mut self, policy: LinkPolicy) -> Self {
        self.navigation_allowed = policy_predicate(policy);
        self
    }

    /// Allows navigation exactly where `routes` accepts the original destination, so mail and
    /// file destinations can be enabled (application policy, TXT-008 notes).
    pub fn routes(mut self, routes: LinkRoutes) -> Self {
        self.navigation_allowed =
            Rc::new(move |destination| routes.classify(destination).0.is_ok());
        self
    }
}

fn policy_predicate(policy: LinkPolicy) -> Rc<dyn Fn(&str) -> bool> {
    Rc::new(move |destination| policy.validate(destination).is_ok())
}

struct OpenMenu {
    link: usize,
    menu: LinkMenu,
    handle: FocusHandle,
}

#[derive(Default)]
struct Runtime {
    handles: Vec<FocusHandle>,
    press: Option<LinkPress>,
    menu: Option<OpenMenu>,
    disclosure_dismissed: bool,
    signature: Vec<[f32; 4]>,
}

/// The per-run link machinery, created at request-layout and painted after the glyphs.
pub struct LinkLayer {
    runtime: Rc<RefCell<Runtime>>,
    overlays: Vec<AnyElement>,
    popups: Vec<AnyElement>,
    card_open: bool,
}

fn activate(
    options: &LinkOptions,
    destination: &str,
    action: LinkActionKind,
    window: &mut Window,
    cx: &mut App,
) {
    match action {
        LinkActionKind::CopyDestination => {
            cx.write_to_clipboard(ClipboardItem::new_string(destination.to_string()));
        }
        LinkActionKind::Open | LinkActionKind::OpenExternally => {
            if !(options.navigation_allowed)(destination) {
                return;
            }
            let activation = LinkActivation {
                destination: destination.to_string(),
                action,
                file: parse_file_destination(destination),
            };
            match &options.on_activate {
                Some(callback) => callback(&activation, window, cx),
                None => cx.open_url(destination),
            }
        }
    }
}

/// An exact text hit inside `link`'s range: the index lookup must report a hit on a glyph; the
/// empty space after the last glyph of a line never activates (R16).
fn exact_hit(layout: &RichLayout, link: &Link, position: Point<Pixels>) -> bool {
    let hit = layout.index_for_position(position);
    hit.is_exact() && link.range.contains(&hit.index())
}

impl LinkLayer {
    /// Attaches the persistent runtime of the element `id`.
    pub fn attach(id: &GlobalElementId, window: &mut Window, _cx: &mut App) -> Self {
        let runtime = window.with_element_state::<Rc<RefCell<Runtime>>, _>(id, |state, _| {
            let runtime = state.unwrap_or_default();
            (runtime.clone(), runtime)
        });
        Self {
            runtime,
            overlays: Vec::new(),
            popups: Vec::new(),
            card_open: false,
        }
    }

    /// Builds one hit region per visual row of every link, the menu and the disclosure card.
    pub fn prepaint(
        &mut self,
        links: &[Link],
        options: &LinkOptions,
        original: OriginalText,
        layout: &RichLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let service = TextService::current();
        let accent = service
            .as_ref()
            .map_or_else(|| TextTheme::current(cx).accent, TextService::accent);
        let selection = service.as_ref().map_or(
            Hsla {
                a: SELECTION_ALPHA,
                ..accent
            },
            |service| service.selection_color(window),
        );
        let runtime = self.runtime.clone();
        let mut signature = Vec::new();
        let window_size = window.viewport_size();

        for (index, link) in links.iter().enumerate() {
            let rects = layout.range_rects(link.range.clone(), px(0.), px(0.));
            let Some(first) = rects.first().copied() else {
                continue;
            };
            let handle = {
                let mut state = runtime.borrow_mut();
                while state.handles.len() <= index {
                    state.handles.push(cx.focus_handle().tab_stop(true));
                }
                state.handles[index].clone()
            };
            signature.push([
                f32::from(first.origin.x),
                f32::from(first.origin.y),
                f32::from(first.size.width),
                f32::from(first.size.height),
            ]);
            let label = SharedString::from(original.copy(link.range.clone()).to_string());
            for (row, rect) in rects.iter().enumerate() {
                let mut overlay = div()
                    .id(ElementId::from(("rich-link", index * 4096 + row)))
                    .w(rect.size.width)
                    .h(rect.size.height)
                    .cursor_pointer()
                    .role(vespertine::Role::Link)
                    .aria_label(label.clone());
                if row == 0 {
                    overlay = overlay
                        .track_focus(&handle)
                        .tab_stop(true)
                        .focus_visible(|style| style.bg(selection).border_1().border_color(accent));
                }
                let overlay = overlay
                    .on_mouse_down(MouseButton::Left, {
                        let runtime = runtime.clone();
                        let layout = layout.clone();
                        let link = link.clone();
                        move |event, _window, _cx| {
                            runtime.borrow_mut().press = Some(LinkPress {
                                position: event.position,
                                button: MouseButton::Left,
                                click_count: event.click_count,
                                over_link: exact_hit(&layout, &link, event.position),
                            });
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let runtime = runtime.clone();
                        let layout = layout.clone();
                        let link = link.clone();
                        let options = options.clone();
                        let service = service.clone();
                        move |event, window, cx| {
                            let press = runtime.borrow_mut().press.take();
                            let Some(press) = press else { return };
                            let selection_has_text = service
                                .as_ref()
                                .is_some_and(TextService::selection_has_text);
                            if pointer_activates(
                                &press,
                                MouseButton::Left,
                                event.position,
                                exact_hit(&layout, &link, event.position),
                                selection_has_text,
                            ) {
                                activate(
                                    &options,
                                    &link.destination,
                                    LinkActionKind::Open,
                                    window,
                                    cx,
                                );
                            }
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let runtime = runtime.clone();
                        let link = link.clone();
                        let options = options.clone();
                        move |event, window, cx| {
                            open_menu(&runtime, index, &link, &options, event.position, window, cx);
                            cx.stop_propagation();
                        }
                    })
                    .on_key_down({
                        let runtime = runtime.clone();
                        let link = link.clone();
                        let options = options.clone();
                        move |event, window, cx| {
                            let keystroke = &event.keystroke;
                            match (keystroke.key.as_str(), keystroke.modifiers.shift) {
                                ("enter", false) | ("space", false) => {
                                    activate(
                                        &options,
                                        &link.destination,
                                        LinkActionKind::Open,
                                        window,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }
                                ("f10", true) => {
                                    open_menu(
                                        &runtime,
                                        index,
                                        &link,
                                        &options,
                                        point(first.left(), first.bottom()),
                                        window,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }
                                ("escape", _) => {
                                    let mut state = runtime.borrow_mut();
                                    state.menu = None;
                                    state.disclosure_dismissed = true;
                                    drop(state);
                                    window.refresh();
                                }
                                _ => {}
                            }
                        }
                    })
                    .tooltip({
                        let destination = link.destination.clone();
                        move |window, cx| disclosure_view(&destination, window_size, window, cx)
                    });
                let mut overlay = overlay.into_any_element();
                overlay.prepaint_as_root(
                    rect.origin,
                    size(
                        AvailableSpace::Definite(rect.size.width),
                        AvailableSpace::Definite(rect.size.height),
                    ),
                    window,
                    cx,
                );
                self.overlays.push(overlay);
            }

            // The focused link shows its complete destination below the first hit rectangle.
            let focused = handle.is_focused(window) && window.last_input_was_keyboard();
            if !focused {
                runtime.borrow_mut().disclosure_dismissed = false;
            } else if !runtime.borrow().disclosure_dismissed {
                let card = disclosure_view(&link.destination, window_size, window, cx);
                let mut popup = deferred(
                    anchored()
                        .position(point(first.left(), first.bottom()))
                        .snap_to_window_with_margin(px(DISCLOSURE_WINDOW_MARGIN))
                        .child(card),
                )
                .with_priority(2)
                .into_any_element();
                popup.prepaint_as_root(
                    point(px(0.), px(0.)),
                    size(
                        AvailableSpace::Definite(window_size.width),
                        AvailableSpace::Definite(window_size.height),
                    ),
                    window,
                    cx,
                );
                self.popups.push(popup);
                self.card_open = true;
            }
        }

        {
            let mut state = runtime.borrow_mut();
            if state.signature != signature {
                // Any change of a link's bounds dismisses the disclosure and the menu.
                if !state.signature.is_empty() {
                    state.menu = None;
                }
                state.signature = signature;
            }
        }

        let menu = runtime
            .borrow()
            .menu
            .as_ref()
            .map(|open| (open.link, open.menu.clone(), open.handle.clone()));
        if let Some((link_index, menu, handle)) = menu
            && let Some(link) = links.get(link_index)
        {
            let mut popup = menu_element(
                &runtime, link, link_index, &menu, &handle, options, accent, selection,
            );
            popup.prepaint_as_root(
                point(px(0.), px(0.)),
                size(
                    AvailableSpace::Definite(window_size.width),
                    AvailableSpace::Definite(window_size.height),
                ),
                window,
                cx,
            );
            self.popups.push(popup);
        }
    }

    /// Paints the hit regions (invisible unless focus-visible) and the popups.
    pub fn paint(&mut self, window: &mut Window, cx: &mut App) {
        for overlay in &mut self.overlays {
            overlay.paint(window, cx);
        }
        let menu_open = self.runtime.borrow().menu.is_some();
        if menu_open {
            // A scroll wheel event outside the menu closes it, in the capture phase.
            let runtime = self.runtime.clone();
            window.on_mouse_event(move |_: &ScrollWheelEvent, phase, window, _cx| {
                if phase == DispatchPhase::Capture {
                    runtime.borrow_mut().menu = None;
                    window.refresh();
                }
            });
        }
        if self.card_open {
            // A press outside dismisses the keyboard-focused disclosure; it traps no input.
            let runtime = self.runtime.clone();
            window.on_mouse_event(move |_: &MouseDownEvent, phase, window, _cx| {
                if phase == DispatchPhase::Capture {
                    runtime.borrow_mut().disclosure_dismissed = true;
                    window.refresh();
                }
            });
        }
        for popup in &mut self.popups {
            popup.paint(window, cx);
        }
    }
}

fn open_menu(
    runtime: &Rc<RefCell<Runtime>>,
    link_index: usize,
    link: &Link,
    options: &LinkOptions,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = cx.focus_handle();
    let allowed = (options.navigation_allowed)(&link.destination);
    runtime.borrow_mut().menu = Some(OpenMenu {
        link: link_index,
        menu: LinkMenu::new(allowed, origin),
        handle: handle.clone(),
    });
    handle.focus(window, cx);
    window.refresh();
}

fn close_menu(
    runtime: &Rc<RefCell<Runtime>>,
    link_index: usize,
    return_focus: bool,
    window: &mut Window,
    cx: &mut App,
) {
    runtime.borrow_mut().menu = None;
    if return_focus {
        let handle = runtime.borrow().handles.get(link_index).cloned();
        if let Some(handle) = handle {
            handle.focus(window, cx);
        }
    }
    window.refresh();
}

const ROW_LABELS: [&str; 3] = ["Open", "Open externally", "Copy link address"];

#[allow(clippy::too_many_arguments)]
fn menu_element(
    runtime: &Rc<RefCell<Runtime>>,
    link: &Link,
    link_index: usize,
    menu: &LinkMenu,
    handle: &FocusHandle,
    options: &LinkOptions,
    accent: Hsla,
    selection: Hsla,
) -> AnyElement {
    let rows = menu.rows().iter().enumerate().map(|(row_ix, row)| {
        let action = row.action;
        let enabled = row.enabled;
        let selected = menu.selected() == row_ix;
        div()
            .id(ElementId::from(("rich-link-menu-row", row_ix)))
            .px(px(10.))
            .h(px(26.))
            .flex()
            .items_center()
            .text_size(px(13.))
            .text_color(if enabled {
                rgb(0xeeeeee)
            } else {
                rgb(0x777777)
            })
            .when(selected, |row| row.bg(selection))
            .child(ROW_LABELS[row_ix])
            .when(enabled, |row| {
                row.cursor_pointer().on_mouse_down(MouseButton::Left, {
                    let runtime = runtime.clone();
                    let options = options.clone();
                    let destination = link.destination.clone();
                    move |_, window, cx| {
                        close_menu(&runtime, link_index, true, window, cx);
                        activate(&options, &destination, action, window, cx);
                        cx.stop_propagation();
                    }
                })
            })
    });
    let content = div()
        .id("rich-link-menu")
        .track_focus(handle)
        .w(px(MENU_WIDTH))
        .py(px(4.))
        .bg(rgb(0x262626))
        .border_1()
        .border_color(accent)
        .rounded(px(6.))
        .occlude()
        .role(vespertine::Role::Menu)
        .on_key_down({
            let runtime = runtime.clone();
            let options = options.clone();
            let destination = link.destination.clone();
            move |event, window, cx| {
                let keystroke = &event.keystroke;
                let shift = keystroke.modifiers.shift;
                let mut handled = true;
                match keystroke.key.as_str() {
                    "tab" if shift => step_menu(&runtime, false),
                    "tab" | "down" => step_menu(&runtime, true),
                    "up" => step_menu(&runtime, false),
                    "escape" => close_menu(&runtime, link_index, true, window, cx),
                    "enter" | "space" => {
                        let action = runtime.borrow().menu.as_ref().map(|open| {
                            (
                                open.menu.selected_action(),
                                open.menu.rows()[open.menu.selected()].enabled,
                            )
                        });
                        if let Some((action, true)) = action {
                            close_menu(&runtime, link_index, true, window, cx);
                            activate(&options, &destination, action, window, cx);
                        }
                    }
                    _ => handled = false,
                }
                if handled {
                    window.refresh();
                    cx.stop_propagation();
                }
            }
        })
        .on_mouse_down_out({
            let runtime = runtime.clone();
            move |_, window, cx| close_menu(&runtime, link_index, false, window, cx)
        })
        .children(rows);
    deferred(
        anchored()
            .position(menu.origin)
            .snap_to_window_with_margin(px(DISCLOSURE_WINDOW_MARGIN))
            .child(content),
    )
    .with_priority(3)
    .into_any_element()
}

fn step_menu(runtime: &Rc<RefCell<Runtime>>, forward: bool) {
    if let Some(open) = runtime.borrow_mut().menu.as_mut() {
        if forward {
            open.menu.next();
        } else {
            open.menu.previous();
        }
    }
}

/// The destination disclosure card: local text only, zero-width spaces between graphemes so even
/// a single long segment wraps (R19). The presentation string never replaces the destination.
struct DisclosureCard {
    text: SharedString,
    size: DisclosureSize,
}

impl Render for DisclosureCard {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("rich-link-disclosure")
            .w(px(self.size.width))
            .max_w(px(self.size.max_width))
            .max_h(px(self.size.max_height))
            .overflow_y_scroll()
            .px(px(DISCLOSURE_PADDING_X))
            .py(px(DISCLOSURE_PADDING_Y))
            .bg(rgb(0x262626))
            .border_1()
            .border_color(rgb(0x555555))
            .rounded(px(4.))
            .text_size(px(DISCLOSURE_TEXT_SIZE))
            .line_height(px(DISCLOSURE_LINE_HEIGHT))
            .text_color(rgb(0xeeeeee))
            .child(self.text.clone())
    }
}

fn disclosure_view(
    destination: &str,
    viewport: vespertine::Size<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let (presented, _) = crate::display_map::with_break_opportunities(destination);
    let natural = window
        .text_system()
        .shape_line(
            SharedString::from(destination.to_string()),
            px(DISCLOSURE_TEXT_SIZE),
            &[vespertine::TextRun {
                len: destination.len(),
                font: window.text_style().font(),
                color: rgb(0xffffff).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
        .width;
    let size = DisclosureSize::compute(viewport, f32::from(natural));
    cx.new(|_| DisclosureCard {
        text: SharedString::from(presented),
        size,
    })
    .into()
}
