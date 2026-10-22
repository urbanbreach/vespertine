//! A complete popup menu view built on the floating foundation (CMP-002).
//!
//! [`PopupMenu`] is a trigger button plus an [`AnchoredMenu`] list: keyboard stepping with
//! wraparound over enabled rows, hover highlight by pointer motion only, Escape through the
//! window's dismissal stack, outside-press dismissal that does not click through, trigger press
//! tracking (a press on the trigger of an open menu leaves it closed; pressing another trigger
//! switches in one click), opening focus after attach, closing focus restoration, and the
//! 140 ms entrance / 100 ms exit with an occluding exit overlay.

use super::{
    DismissHandler, EscapeHandler, EscapeOutcome, FocusTarget, LayerHandle, LayerOptions,
    OutsidePress, Placement, Popup, SurfaceMotion, TriggerBounds, close_popup,
    keys::{ListKey, classify_key},
    menu::{AnchoredMenu, ROW_GAP},
    open_layer,
    stepping::{Edge, StepDirection, edge_available, step_available},
};
use crate::tokens::ComponentTokens;
use std::rc::Rc;
use vespertine::{
    App, Bounds, Context, Div, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MotionClock, MouseButton, ParentElement, Pixels, Render, Role, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, canvas, div,
    prelude::FluentBuilder, px,
};

/// One row of a popup menu.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupMenuItem {
    /// Row text.
    pub label: SharedString,
    /// Disabled rows are skipped by stepping, cannot be activated and paint at the disabled
    /// opacity.
    pub disabled: bool,
}

impl PopupMenuItem {
    /// An enabled row.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            disabled: false,
        }
    }

    /// Marks the row disabled.
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
}

/// A painted region a bounds observer is told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPart {
    /// The trigger button.
    Trigger,
    /// The menu card (while mounted).
    Surface,
    /// Row `n` (while mounted).
    Row(usize),
}

/// Observer of painted bounds (test hosts, automation).
pub type BoundsObserver = Rc<dyn Fn(MenuPart, Bounds<Pixels>, &mut App)>;

struct OpenMenu {
    layer: LayerHandle,
    active: Option<usize>,
}

/// Trigger plus popup list. See the [module documentation](self).
pub struct PopupMenu {
    id: ElementId,
    label: SharedString,
    items: Vec<PopupMenuItem>,
    placement: Placement,
    width: Pixels,
    height: Pixels,
    menu_width: Option<Pixels>,
    row_gap: Pixels,
    selected: Option<usize>,
    trigger: TriggerBounds,
    exempt: Vec<TriggerBounds>,
    trigger_focus: FocusHandle,
    layer_focus: FocusHandle,
    scroll: ScrollHandle,
    popup: Popup<OpenMenu>,
    on_select: Option<Rc<dyn Fn(usize, &mut Window, &mut Context<PopupMenu>)>>,
    observer: Option<BoundsObserver>,
}

impl PopupMenu {
    /// A closed menu with a 180 x 36 trigger, placed below it.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        items: Vec<PopupMenuItem>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            items,
            placement: Placement::Below {
                align: super::ContainedAlign::Start,
                gap: super::TRIGGER_CLEARANCE,
            },
            width: px(180.),
            height: px(36.),
            menu_width: None,
            row_gap: ROW_GAP,
            selected: None,
            trigger: TriggerBounds::new(),
            exempt: Vec::new(),
            // The trigger is a keyboard tab stop; Enter or Down on it opens the menu.
            trigger_focus: cx.focus_handle().tab_stop(true),
            layer_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            popup: Popup::new(),
            on_select: None,
            observer: None,
        }
    }

    /// Where the menu goes relative to the trigger.
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Trigger size.
    pub fn size(mut self, width: Pixels, height: Pixels) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Card width (default: the trigger width).
    pub fn menu_width(mut self, width: Pixels) -> Self {
        self.menu_width = Some(width);
        self
    }

    /// Gap between rows (default 2 px).
    pub fn row_gap(mut self, gap: Pixels) -> Self {
        self.row_gap = gap;
        self
    }

    /// The row highlighted (and scrolled into view) when the menu opens.
    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// Called with the activated row index; the menu closes first.
    pub fn on_select(
        mut self,
        handler: impl Fn(usize, &mut Window, &mut Context<PopupMenu>) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Other triggers that keep receiving presses while this menu is open, so that pressing
    /// one switches menus in a single click. This menu's own trigger is always exempt.
    pub fn exempt_triggers(mut self, triggers: Vec<TriggerBounds>) -> Self {
        self.exempt = triggers;
        self
    }

    /// Reports the painted bounds of the trigger, the card and every row.
    pub fn observe_bounds(mut self, observer: BoundsObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Uses a tracker created by the caller for this menu's trigger (so other menus can list it
    /// in [`PopupMenu::exempt_triggers`] before this menu exists).
    pub fn trigger_tracker(mut self, tracker: TriggerBounds) -> Self {
        self.trigger = tracker;
        self
    }

    /// The bounds tracker of this menu's trigger.
    pub fn trigger_bounds(&self) -> TriggerBounds {
        self.trigger.clone()
    }

    /// The trigger's focus handle.
    pub fn trigger_focus(&self) -> &FocusHandle {
        &self.trigger_focus
    }

    /// The lifecycle value (open, closing, unmounted).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// The focus handle of the menu card (the layer's container).
    pub fn layer_focus(&self) -> &FocusHandle {
        &self.layer_focus
    }

    /// The scroll offset of the inner scroll region of a contained menu.
    pub fn scroll_offset(&self) -> vespertine::Point<Pixels> {
        self.scroll.offset()
    }

    /// The highlighted row while open.
    pub fn active(&self) -> Option<usize> {
        self.popup.as_open().and_then(|open| open.active)
    }

    /// Whether the card is still painted (open or in its exit phase).
    pub fn is_mounted(&self) -> bool {
        self.popup.is_mounted()
    }

    fn available(&self) -> Vec<bool> {
        self.items.iter().map(|item| !item.disabled).collect()
    }

    /// Opens the menu (no-op when it is open).
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_open() {
            return;
        }
        let view = cx.entity().downgrade();
        let dismiss: DismissHandler = Rc::new(move |window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.close(window, cx));
            }
        });
        let escape: EscapeHandler = {
            let dismiss = dismiss.clone();
            Rc::new(move |window, cx| {
                dismiss(window, cx);
                EscapeOutcome::Dismissed
            })
        };
        let layer = open_layer(
            window,
            cx,
            LayerOptions {
                escape,
                dismiss,
                layer_focus: self.layer_focus.clone(),
                trigger: Some(self.trigger_focus.clone()),
                initial_focus: FocusTarget::Default,
                final_focus: FocusTarget::Default,
                trigger_first: false,
            },
        );
        let active = self
            .selected
            .filter(|ix| self.items.get(*ix).is_some_and(|item| !item.disabled));
        if let Some(ix) = active {
            self.scroll.scroll_to_item(ix);
        }
        let now = MotionClock::now_ms(cx);
        self.popup.open(OpenMenu { layer, active }, now);
        cx.notify();
    }

    /// Closes the menu: Escape stops reaching it, focus is restored and the exit phase starts.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.popup.as_open() {
            open.layer.close(window, cx);
        }
        close_popup(self, cx, |this| &mut this.popup);
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.as_open().is_none() || self.items.get(ix).is_none_or(|item| item.disabled) {
            return;
        }
        self.close(window, cx);
        if let Some(handler) = self.on_select.clone() {
            handler(ix, window, cx);
        }
    }

    fn set_active(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.items.get(ix).is_none_or(|item| item.disabled) {
            return;
        }
        if let Some(open) = self.popup.as_open_mut()
            && open.active != Some(ix)
        {
            open.active = Some(ix);
            cx.notify();
        }
    }

    fn on_menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let available = self.available();
        // A closing menu reads as closed: its key handler falls through.
        let Some(open) = self.popup.as_open_mut() else {
            return;
        };
        let key = classify_key(&event.keystroke.key, event.keystroke.modifiers, true);
        let next = match key {
            ListKey::Down => {
                step_available(available.len(), open.active, StepDirection::Forward, |i| {
                    available[i]
                })
            }
            ListKey::Up => {
                step_available(available.len(), open.active, StepDirection::Backward, |i| {
                    available[i]
                })
            }
            ListKey::Home => edge_available(available.len(), Edge::First, |i| available[i]),
            ListKey::End => edge_available(available.len(), Edge::Last, |i| available[i]),
            ListKey::Enter | ListKey::SecondaryEnter => {
                if let Some(ix) = open.active {
                    cx.stop_propagation();
                    self.select(ix, window, cx);
                }
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        open.active = next;
        if let Some(ix) = next {
            self.scroll.scroll_to_item(ix);
        }
        cx.notify();
    }

    fn observe(&self, part: MenuPart) -> Option<impl IntoElement + use<>> {
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
}

/// The row of a popup menu: its role, geometry, active highlight and disabled look, with the
/// label as its content. Owners of menus that are not a [`PopupMenu`] (a context menu opened at
/// the pointer) build their rows with it and attach their own listeners.
pub fn menu_item_row(
    id: impl Into<ElementId>,
    item: &PopupMenuItem,
    active: bool,
    tokens: &ComponentTokens,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::MenuItem)
        .aria_label(item.label.clone())
        .relative()
        .flex()
        .items_center()
        .flex_none()
        .w_full()
        .h(px(32.))
        .px_2()
        .rounded(px(4.))
        .text_color(tokens.text)
        .when(active, |row| row.bg(tokens.highlight))
        .when(item.disabled, |row| row.opacity(tokens.disabled_opacity))
        .child(item.label.clone())
}

impl Render for PopupMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = ComponentTokens::current(cx);
        let view = cx.entity().downgrade();

        let menu = self.popup.get().map(|open| {
            let motion = SurfaceMotion::now(&self.popup, self.placement.side(), cx);
            let dismiss = {
                let view = view.clone();
                Rc::new(move |window: &mut Window, cx: &mut App| {
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |this, cx| this.close(window, cx));
                    }
                })
            };
            let mut exempt = self.exempt.clone();
            exempt.push(self.trigger.clone());
            let card_width = self.menu_width.unwrap_or(self.width);
            let mut menu =
                AnchoredMenu::new((self.id.clone(), "menu"), &self.trigger, self.placement)
                    .row_gap(self.row_gap)
                    .motion(motion)
                    .scroll_handle(&self.scroll)
                    .track_focus(&self.layer_focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.on_menu_key(event, window, cx)
                    }))
                    .outside_press(OutsidePress {
                        layer: open.layer.id(),
                        on_dismiss: dismiss,
                        consume: !matches!(self.placement, Placement::Beside { .. }),
                        exempt,
                    })
                    .w(card_width);
            if let Some(observer) = self.observer.clone() {
                menu = menu.observe_bounds(Rc::new(move |bounds, cx| {
                    observer(MenuPart::Surface, bounds, cx)
                }));
            }
            for (ix, item) in self.items.iter().enumerate() {
                let active = open.active == Some(ix);
                let row = menu_item_row(
                    ElementId::NamedInteger("popup-menu-row".into(), ix as u64),
                    item,
                    active,
                    &tokens,
                )
                .when(!item.disabled, |row| {
                    row.on_mouse_move(cx.listener(move |this, _, _, cx| this.set_active(ix, cx)))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.select(ix, window, cx)),
                        )
                })
                .children(self.observe(MenuPart::Row(ix)));
                menu = menu.child(row);
            }
            menu
        });

        div()
            .id(self.id.clone())
            .role(Role::Button)
            .relative()
            .flex()
            .items_center()
            .w(self.width)
            .h(self.height)
            .px_3()
            .rounded(tokens.button_radius)
            .bg(tokens.control_surface)
            .text_color(tokens.text)
            .track_focus(&self.trigger_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.popup.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                // The press found this menu mounted: the click leaves it closed.
                if this.popup.take_press_was_open() {
                    return;
                }
                this.open(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.popup.is_mounted() {
                    return;
                }
                if matches!(
                    classify_key(&event.keystroke.key, event.keystroke.modifiers, false),
                    ListKey::Enter | ListKey::Down
                ) {
                    cx.stop_propagation();
                    this.open(window, cx);
                }
            }))
            .child(self.label.clone())
            .child(self.trigger.probe())
            .children(self.observe(MenuPart::Trigger))
            .children(menu)
    }
}
