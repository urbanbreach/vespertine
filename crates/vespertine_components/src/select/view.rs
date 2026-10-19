//! The native select: trigger button plus a contained, scrollable menu (CMP-005 R9-R15).
//!
//! The select keeps keyboard focus on its trigger while the menu is open (the highlighted row is
//! exposed as the active descendant) and goes through the popup lifecycle, press tracking, Escape
//! and focus ownership of CMP-002. After a commit or Escape focus is on the trigger.

use super::model::{
    Leading, OptionId, Outcome, SelectKey, SelectOption, SelectState, compact, position_of,
    trigger_name, value_text,
};
use crate::floating::menu::{AnchoredMenu, MENU_PADDING, ROW_GAP};
use crate::floating::{
    BoundsObserver, ContainedAlign, EscapeHandler, EscapeOutcome, FocusTarget, LayerHandle,
    LayerOptions, MenuPart, OutsidePress, Placement, Popup, SurfaceMotion, TriggerBounds,
    classify_key, close_popup, contained_max_size, containment_limits, keys::ListKey, open_layer,
    rows_max_height,
};
use crate::tokens::ComponentTokens;
use std::rc::Rc;
use vespertine::{
    App, Bounds, ClickEvent, Context, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, MotionClock, MouseButton, ParentElement, Pixels, Render, Role,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Toggled, Window, canvas, div,
    hsla, point, prelude::FluentBuilder, px,
};

/// Trigger height.
pub const TRIGGER_HEIGHT: Pixels = px(32.);
/// Minimum menu width.
pub const MENU_MIN_WIDTH: Pixels = px(160.);
/// Row height.
pub const ROW_HEIGHT: Pixels = px(32.);
/// Width of the reserved trailing check slot.
pub const CHECK_SLOT: Pixels = px(18.);
/// Width of the chevron.
pub const CHEVRON: Pixels = px(14.);
/// Gap between the value area and the chevron.
pub const VALUE_GAP: Pixels = px(8.);
/// Height of the heading chrome above the rows.
pub const HEADING_HEIGHT: Pixels = px(24.);
/// Width of the reserved focus edge of the trigger.
pub const FOCUS_EDGE: Pixels = px(2.);

type SelectHandler = Rc<dyn Fn(&OptionId, &mut Window, &mut Context<Select>)>;

struct OpenSelect {
    layer: LayerHandle,
}

/// A single-value select. See the [module documentation](self).
pub struct Select {
    id: ElementId,
    label: SharedString,
    options: Vec<SelectOption>,
    selected: Option<OptionId>,
    placeholder: Option<SharedString>,
    disabled: bool,
    width: Option<Pixels>,
    menu_width: Option<Pixels>,
    heading: Option<SharedString>,
    open_rightward: bool,
    limits: Option<Bounds<Pixels>>,
    limits_provider: Option<Rc<dyn Fn(&Window) -> Bounds<Pixels>>>,
    compact: bool,
    loading: bool,
    font_family: Option<SharedString>,
    on_select: Option<SelectHandler>,
    state: SelectState,
    popup: Popup<OpenSelect>,
    trigger: TriggerBounds,
    exempt: Vec<TriggerBounds>,
    trigger_focus: FocusHandle,
    layer_focus: FocusHandle,
    scroll: ScrollHandle,
    observer: Option<BoundsObserver>,
}

impl Select {
    /// A closed select named `label` (the accessible name is `label: current value`).
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        options: Vec<SelectOption>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            options,
            selected: None,
            placeholder: None,
            disabled: false,
            width: None,
            menu_width: None,
            heading: None,
            open_rightward: false,
            limits: None,
            limits_provider: None,
            compact: false,
            loading: false,
            font_family: None,
            on_select: None,
            state: SelectState::new(),
            popup: Popup::new(),
            trigger: TriggerBounds::new(),
            exempt: Vec::new(),
            trigger_focus: cx.focus_handle().tab_stop(true).tab_index(0),
            layer_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            observer: None,
        }
    }

    /// The selected option's id.
    pub fn selected(mut self, selected: Option<OptionId>) -> Self {
        self.selected = selected;
        self
    }

    /// Text shown when no option matches the selection.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// A disabled select is not focusable but keeps its role, label and disabled state.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A fixed trigger width (the menu defaults to it).
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// An explicit menu width; it overrides the trigger width.
    pub fn menu_width(mut self, width: Pixels) -> Self {
        self.menu_width = Some(width);
        self
    }

    /// A small heading above the rows; only the rows scroll.
    pub fn heading(mut self, heading: impl Into<SharedString>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    /// Start-aligns the menu so it opens rightward (the default grows leftward from the
    /// trigger's right edge).
    pub fn open_rightward(mut self, rightward: bool) -> Self {
        self.open_rightward = rightward;
        self
    }

    /// The bounds the menu stays inside (default: the window inset by 8 px).
    pub fn limits(mut self, limits: Bounds<Pixels>) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Bounds that follow something painted, evaluated at every use (a dialog's border box,
    /// CMP-003 R11: see `Dialog::menu_limits_provider`). It overrides [`Select::limits`].
    pub fn limits_provider(mut self, provider: Rc<dyn Fn(&Window) -> Bounds<Pixels>>) -> Self {
        self.limits_provider = Some(provider);
        self
    }

    /// The compact discrete selector preset (R14): 256 px card, up to seven 32 px rows, room for
    /// four skeleton rows while empty or loading.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// The list is loading (shows skeleton rows in the compact preset).
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// The font family of the trigger and the rows.
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    /// Called with the committed option's id after the close began (the select has already
    /// adopted the id).
    pub fn on_select(
        mut self,
        handler: impl Fn(&OptionId, &mut Window, &mut Context<Select>) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Other triggers on the page that keep receiving presses while this menu is open, so that
    /// pressing one switches in a single click.
    pub fn exempt_triggers(mut self, triggers: Vec<TriggerBounds>) -> Self {
        self.exempt = triggers;
        self
    }

    /// Uses a tracker created by the caller for this trigger.
    pub fn trigger_tracker(mut self, tracker: TriggerBounds) -> Self {
        self.trigger = tracker;
        self
    }

    /// Reports the painted bounds of the trigger, the card and every row.
    pub fn observe_bounds(mut self, observer: BoundsObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// The bounds tracker of this trigger.
    pub fn trigger_bounds(&self) -> TriggerBounds {
        self.trigger.clone()
    }

    /// The trigger's focus handle: it keeps focus while the menu is open.
    pub fn trigger_focus(&self) -> &FocusHandle {
        &self.trigger_focus
    }

    /// Replaces the options (identity is the id: the highlight follows it).
    pub fn set_options(&mut self, options: Vec<SelectOption>, cx: &mut Context<Self>) {
        self.options = options;
        self.state
            .reconcile(&self.options, self.selected.as_deref());
        cx.notify();
    }

    /// Sets the selected id (the owner calls it from the selection callback).
    pub fn set_selected(&mut self, selected: Option<OptionId>, cx: &mut Context<Self>) {
        self.selected = selected;
        cx.notify();
    }

    /// Enables or disables the select.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        cx.notify();
    }

    /// The selected id.
    pub fn selected_id(&self) -> Option<&OptionId> {
        self.selected.as_ref()
    }

    /// The options.
    pub fn options(&self) -> &[SelectOption] {
        &self.options
    }

    /// Whether the menu is open (a closing menu reads as closed).
    pub fn is_open(&self) -> bool {
        self.popup.is_open()
    }

    /// Whether the menu is still painted (open or in its exit phase).
    pub fn is_mounted(&self) -> bool {
        self.popup.is_mounted()
    }

    /// The highlighted option id while open.
    pub fn highlighted(&self) -> Option<&OptionId> {
        self.popup.as_open().and_then(|_| self.state.highlighted())
    }

    /// The scroll offset of the rows.
    pub fn scroll_offset(&self) -> vespertine::Point<Pixels> {
        self.scroll.offset()
    }

    /// The accessible name of the trigger: `label: current value`.
    pub fn accessible_name(&self) -> String {
        trigger_name(
            &self.label,
            &self.options,
            self.selected.as_deref(),
            self.placeholder.as_deref(),
        )
    }

    fn row_extent(&self, count: usize) -> Pixels {
        if count == 0 {
            px(0.)
        } else {
            ROW_HEIGHT * count as f32 + ROW_GAP * (count - 1) as f32
        }
    }

    fn effective_limits(&self, window: &Window) -> Bounds<Pixels> {
        if let Some(provider) = &self.limits_provider {
            return provider(window);
        }
        self.limits.unwrap_or_else(|| {
            containment_limits(Bounds::new(Default::default(), window.viewport_size()))
        })
    }

    /// Closes the menu from outside (another menu opening on the same page): Escape stops
    /// reaching it, focus is restored and the exit phase starts.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.popup.as_open() {
            open.layer.close(window, cx);
        }
        if self.popup.is_open() {
            self.state.close();
        }
        close_popup(self, cx, |this| &mut this.popup);
    }

    /// Opens the menu with the highlight on the selected option (without advancing). The
    /// selected row is brought to the vertical center when it lies outside the viewport.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.popup.is_open() {
            return;
        }
        if !self.trigger_focus.is_focused(window) {
            self.trigger_focus.focus(window, cx);
        }
        let view = cx.entity().downgrade();
        let escape: EscapeHandler = Rc::new(move |window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.close(window, cx));
            }
            EscapeOutcome::Dismissed
        });
        let layer = open_layer(
            window,
            cx,
            LayerOptions {
                escape,
                layer_focus: self.layer_focus.clone(),
                trigger: Some(self.trigger_focus.clone()),
                // The trigger keeps keyboard focus while the menu is open.
                initial_focus: FocusTarget::Suppress,
                final_focus: FocusTarget::Default,
                trigger_first: false,
            },
        );
        self.state.open(&self.options, self.selected.as_deref());
        self.center_selected(window);
        let now = MotionClock::now_ms(cx);
        self.popup.open(OpenSelect { layer }, now);
        cx.notify();
    }

    /// Brings the selected row to the vertical center when it lies outside the first viewport.
    fn center_selected(&self, window: &Window) {
        let Some(ix) = self
            .selected
            .as_deref()
            .and_then(|id| position_of(&self.options, id))
        else {
            return;
        };
        let limits = self.effective_limits(window);
        let trigger_height = self
            .trigger
            .get()
            .map_or(TRIGGER_HEIGHT, |bounds| bounds.size.height);
        let chrome = if self.heading.is_some() {
            HEADING_HEIGHT
        } else {
            px(0.)
        };
        let card_max = contained_max_size(limits, trigger_height).height;
        let viewport = f32::from(rows_max_height(card_max, chrome + MENU_PADDING * 2.));
        let content = f32::from(self.row_extent(self.options.len()));
        if content <= viewport {
            return;
        }
        let pitch = f32::from(ROW_HEIGHT + ROW_GAP);
        let center = ix as f32 * pitch + f32::from(ROW_HEIGHT) / 2.;
        let top = (center - viewport / 2.).clamp(0., content - viewport);
        if center > viewport {
            self.scroll.set_offset(point(px(0.), px(-top)));
        }
    }

    fn commit(&mut self, id: &OptionId, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = position_of(&self.options, id).is_some_and(|ix| !self.options[ix].disabled);
        if !enabled {
            return;
        }
        // The close begins first, then the selection callback runs. The select adopts the
        // committed id; an owner that rejects it calls `set_selected` from the callback.
        self.close(window, cx);
        self.selected = Some(id.clone());
        cx.notify();
        if let Some(handler) = self.on_select.clone() {
            handler(id, window, cx);
        }
    }

    fn reveal(&self, id: &str) {
        if let Some(ix) = position_of(&self.options, id) {
            self.scroll.scroll_to_item(ix);
        }
    }

    fn key(&self, event: &KeyDownEvent) -> Option<SelectKey> {
        let keystroke = &event.keystroke;
        if keystroke.key == "space" && !keystroke.modifiers.modified() {
            return Some(SelectKey::Space);
        }
        match classify_key(&keystroke.key, keystroke.modifiers, true) {
            ListKey::Up => Some(SelectKey::Up),
            ListKey::Down => Some(SelectKey::Down),
            ListKey::Home => Some(SelectKey::Home),
            ListKey::End => Some(SelectKey::End),
            ListKey::Enter => Some(SelectKey::Enter),
            _ => None,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let Some(key) = self.key(event) else {
            return;
        };
        let open = self.popup.is_open();
        let outcome = self.state.key_down(
            key,
            event.is_held,
            open,
            &self.options,
            self.selected.as_deref(),
        );
        match outcome {
            Outcome::Ignored => return,
            Outcome::Consumed => {}
            Outcome::Open => {
                // The state already put the highlight on the selection; opening re-resolves it.
                self.open(window, cx);
            }
            Outcome::Highlighted(id) => {
                self.reveal(&id);
                cx.notify();
            }
            Outcome::Commit(id) => self.commit(&id, window, cx),
            Outcome::Close => self.close(window, cx),
        }
        // Consume the key and its default so the base keyboard click of the same press cannot
        // activate the trigger a second time.
        window.prevent_default();
        cx.stop_propagation();
    }

    fn on_key_up(&mut self, event: &KeyUpEvent) {
        match event.keystroke.key.as_str() {
            "enter" => self.state.key_up(SelectKey::Enter),
            "space" => self.state.key_up(SelectKey::Space),
            _ => {}
        }
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

    fn row(
        &self,
        ix: usize,
        option: &SelectOption,
        tokens: &ComponentTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected = self.selected.as_deref() == Some(option.id.as_ref());
        let highlighted = self.state.highlighted() == Some(&option.id);
        let id = option.id.clone();
        let hover_id = option.id.clone();
        let disabled = option.disabled;
        let leading = option.leading.clone();
        div()
            .id(ElementId::NamedInteger("select-row".into(), ix as u64))
            .role(Role::MenuItemRadio)
            .aria_label(option.label.clone())
            .aria_toggled(if selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .aria_disabled(disabled)
            .when(highlighted, |row| row.aria_active_descendant())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .gap(px(8.))
            .w_full()
            .h(ROW_HEIGHT)
            .px(px(8.))
            .rounded(px(4.))
            .text_color(tokens.text)
            .when(highlighted, |row| row.bg(tokens.highlight))
            .when(disabled, |row| row.opacity(tokens.disabled_opacity))
            .when(!disabled, |row| {
                row.on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.state.point_at(&this.options, &hover_id) {
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(
                    move |this, event: &ClickEvent, window, cx| {
                        if !matches!(event, ClickEvent::Keyboard(_)) {
                            this.commit(&id, window, cx);
                        }
                    },
                ))
            })
            .when_some(leading, |row, leading| {
                row.child(leading_element(leading, tokens))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(6.))
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(div().truncate().child(option.label.clone()))
                    .when_some(option.secondary.clone(), |label, secondary| {
                        label.child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(tokens.text_muted)
                                .child(secondary),
                        )
                    }),
            )
            .when_some(option.shortcut.clone(), |row, shortcut| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(tokens.text_muted)
                        .child(shortcut),
                )
            })
            .child(
                // The check slot is reserved on every row so labels align; the check is drawn
                // only on the selected row.
                div()
                    .flex_none()
                    .w(CHECK_SLOT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |slot| slot.child("\u{2713}")),
            )
            .children(self.observe(MenuPart::Row(ix)))
    }

    fn skeleton_rows(&self, tokens: &ComponentTokens) -> Vec<impl IntoElement + use<>> {
        (0..compact::SKELETON_ROWS)
            .map(|ix| {
                div()
                    .id(ElementId::NamedInteger("select-skeleton".into(), ix as u64))
                    .flex_none()
                    .w_full()
                    .h(ROW_HEIGHT)
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .h(px(10.))
                            .w(px(120. - 12. * ix as f32))
                            .rounded(px(3.))
                            .bg(tokens.border),
                    )
            })
            .collect()
    }
}

fn leading_element(leading: Leading, tokens: &ComponentTokens) -> impl IntoElement + use<> {
    let slot = div()
        .flex_none()
        .w(px(16.))
        .flex()
        .items_center()
        .justify_center();
    match leading {
        Leading::Glyph(glyph) => slot.text_color(tokens.text_muted).child(glyph),
        Leading::Swatch(color) => slot.child(
            div()
                .size(px(12.))
                .rounded(px(3.))
                .border_1()
                .border_color(tokens.border)
                .bg(color),
        ),
    }
}

impl Render for Select {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = ComponentTokens::current(cx);
        let open = self.popup.is_open();
        let hovered_fill = tokens.control_surface_hover;
        let rest_fill = if open {
            hovered_fill
        } else {
            tokens.control_surface
        };
        let selected_option = self
            .selected
            .as_deref()
            .and_then(|id| position_of(&self.options, id))
            .map(|ix| self.options[ix].clone());
        let value = value_text(
            &self.options,
            self.selected.as_deref(),
            self.placeholder.as_deref(),
        );
        let showing_placeholder = selected_option.is_none();
        let name = self.accessible_name();
        let disabled = self.disabled;

        let menu = self.popup.get().map(|open_select| {
            let motion = SurfaceMotion::now(&self.popup, crate::floating::Side::Bottom, cx);
            let view = cx.entity().downgrade();
            let dismiss = Rc::new(move |window: &mut Window, cx: &mut App| {
                if let Some(view) = view.upgrade() {
                    view.update(cx, |this, cx| this.close(window, cx));
                }
            });
            let mut exempt = self.exempt.clone();
            exempt.push(self.trigger.clone());
            let placement = Placement::Contained {
                limits: self.effective_limits(window),
                align: if self.open_rightward {
                    ContainedAlign::Start
                } else {
                    ContainedAlign::End
                },
            };
            let trigger_width = self.trigger.get().map(|bounds| bounds.size.width);
            let width = if self.compact {
                px(compact::WIDTH)
            } else {
                self.menu_width.unwrap_or_else(|| {
                    self.width
                        .or(trigger_width)
                        .map_or(MENU_MIN_WIDTH, |w| w.max(MENU_MIN_WIDTH))
                })
            };
            let mut menu = AnchoredMenu::new((self.id.clone(), "menu"), &self.trigger, placement)
                .row_gap(ROW_GAP)
                .motion(motion)
                .scroll_handle(&self.scroll)
                .outside_press(OutsidePress {
                    layer: open_select.layer.id(),
                    on_dismiss: dismiss,
                    consume: true,
                    exempt,
                })
                .w(width);
            if !self.compact || self.menu_width.is_some() {
                menu = menu.min_w(MENU_MIN_WIDTH);
            }
            if self.compact {
                menu = menu.h(px(compact::list_height(self.options.len())));
            }
            if let Some(family) = self.font_family.clone() {
                menu = menu.font_family(family);
            }
            if let Some(observer) = self.observer.clone() {
                menu = menu.observe_bounds(Rc::new(move |bounds, cx| {
                    observer(MenuPart::Surface, bounds, cx)
                }));
            }
            if let Some(heading) = self.heading.clone() {
                menu = menu.heading(
                    div()
                        .h(HEADING_HEIGHT)
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .text_color(tokens.text_muted)
                        .child(heading),
                    HEADING_HEIGHT,
                );
            }
            if self.compact && (self.options.is_empty() || self.loading) {
                for row in self.skeleton_rows(&tokens) {
                    menu = menu.child(row);
                }
            } else {
                for (ix, option) in self.options.iter().enumerate() {
                    menu = menu.child(self.row(ix, option, &tokens, cx));
                }
            }
            menu
        });

        let mut trigger = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(VALUE_GAP)
            .h(TRIGGER_HEIGHT)
            .px(px(10.))
            .rounded(tokens.button_radius)
            // The reserved focus edge: transparent unless focus-visible.
            .border(FOCUS_EDGE)
            .border_color(hsla(0., 0., 0., 0.))
            .bg(rest_fill)
            .text_color(tokens.text)
            .text_size(px(13.))
            .when_some(self.width, |trigger, width| trigger.w(width))
            .when_some(self.font_family.clone(), |trigger, family| {
                trigger.font_family(family)
            })
            .role(Role::Button)
            .aria_label(name)
            .aria_expanded(open)
            .aria_disabled(disabled)
            .child(
                crate::layout::centered_row()
                    .gap(px(6.))
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .when_some(
                        selected_option.and_then(|option| option.leading),
                        |area, leading| area.child(leading_element(leading, &tokens)),
                    )
                    .child(
                        div()
                            .truncate()
                            .when(showing_placeholder, |text| {
                                text.text_color(tokens.text_muted)
                            })
                            .child(value),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(CHEVRON)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(tokens.text_muted)
                    .child(if open { "\u{25B4}" } else { "\u{25BE}" }),
            )
            .child(self.trigger.probe())
            .children(self.observe(MenuPart::Trigger));

        if disabled {
            trigger = trigger.opacity(tokens.disabled_opacity);
        } else {
            trigger = trigger
                .track_focus(&self.trigger_focus)
                .focus_visible(|style| style.border_color(tokens.focus_ring))
                .cursor_pointer()
                .hover(|style| style.bg(hovered_fill))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.popup.note_trigger_press()),
                )
                .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                    // The keyboard path belongs to the key handler.
                    if matches!(event, ClickEvent::Keyboard(_)) {
                        return;
                    }
                    // The press found the menu mounted: the click leaves it closed.
                    if this.popup.take_press_was_open() {
                        return;
                    }
                    this.open(window, cx);
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.on_key_down(event, window, cx)
                }))
                .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| this.on_key_up(event)));
        }
        trigger.children(menu)
    }
}
