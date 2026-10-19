//! The native dialog view (CMP-003 R3-R5, R7, R9-R15).
//!
//! A [`Dialog`] is an entity the owner places anywhere in its tree; while closed it renders
//! nothing. Opening mounts one deferred portal at layer priority 0 that fills the window (its
//! size is read from the viewport at every draw, so a resize reaches the portal in the same
//! frame), is explicitly transparent and centers the popup with flex layout. Priority-1 menus
//! (and priority-2 nested menus) opened from inside the dialog therefore paint above the portal,
//! and the portal above the page.
//!
//! * **Modal** (the default): the portal occludes the page, a scrim backdrop (token role, black
//!   at 50 % alpha) covers all four window edges and closes the dialog on a press unless the
//!   policy forbids it, the popup carries the modal key scope
//!   ([`MODAL_SCOPE_CONTEXT`](vespertine::MODAL_SCOPE_CONTEXT): window-level shortcuts of the
//!   page beneath are not dispatched while focus is inside) and Tab cycles inside the popup
//!   (first to last and back, [`next_tab_stop`](super::model::next_tab_stop) semantics).
//! * **Nonmodal**: the portal has no occluding surface, so the page stays interactive around the
//!   popup, and Tab is not trapped. An outside press is still a dismissal by policy but is never
//!   consumed.
//!
//! The dialog is a dismissal layer of CMP-002: Escape reaches only the top layer, so a menu or
//! select opened from inside the dialog closes before the dialog does. Escape and outside press
//! are cancels (the cancel callback, never the confirm action); with Escape switched off the
//! dialog still consumes the key so it never reaches the page. Focus returns on close to the
//! explicit final focus, then the trigger if mounted, then the focus owner captured at open if
//! still mounted, then the window's default (nothing focused), and only if focus is still inside
//! the dialog or nowhere.

use super::model::{ActionKind, Activation, DialogAction, DialogModel, Dismissal, DismissalPolicy};
use crate::controls::Button;
use crate::floating::{
    EscapeHandler, EscapeOutcome, FocusTarget, LayerHandle, LayerOptions, OutsidePress,
    TriggerBounds, containment_limits, is_top_layer, open_layer, set_layer_bounds,
};
use crate::tokens::ComponentTokens;
use std::rc::Rc;
use vespertine::{
    Anchor, AnyElement, AnyView, App, Bounds, Context, ElementId, Empty, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, anchored, canvas, deferred, div,
    hsla, point, px,
};

/// Layer priority of the portal: below priority-1 menus and priority-2 nested menus.
pub const PORTAL_PRIORITY: usize = 0;
/// Default popup width.
pub const POPUP_WIDTH: Pixels = px(420.);
/// Clearance between the popup and the window edges at the smallest sizes.
pub const POPUP_MARGIN: Pixels = px(16.);
/// Padding of the popup surface.
pub const POPUP_PADDING: Pixels = px(20.);
/// The accessible level of the title heading (R8).
pub const TITLE_LEVEL: usize = 2;

/// A painted part of the dialog, for bounds observers (inspection, test hosts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogPart {
    /// The full-window portal.
    Portal,
    /// The scrim backdrop (modal dialogs only).
    Backdrop,
    /// The popup surface.
    Popup,
    /// The action button wrapper at this index of the action list.
    Action(usize),
}

/// Observer of the painted bounds of the dialog's parts.
pub type DialogObserver = Rc<dyn Fn(DialogPart, Bounds<Pixels>, &mut App)>;

/// Bounds the contained menus of the dialog stay inside.
pub type LimitsProvider = Rc<dyn Fn(&Window) -> Bounds<Pixels>>;

type Callback = Rc<dyn Fn(&mut Window, &mut Context<Dialog>)>;
type OpenChange = Rc<dyn Fn(bool, &mut Window, &mut Context<Dialog>)>;
type ActionHandler = Rc<dyn Fn(&Activation, &mut Window, &mut Context<Dialog>)>;
type KeyHandler = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut Context<Dialog>)>;

struct OpenDialog {
    layer: LayerHandle,
}

/// The containment limits of a dialog whose painted popup bounds `popup` tracks: the border box
/// inset by 8 px, or before the first paint the window inset by 8 px (CMP-003 R11). Lets an
/// owner build a select that lives inside the dialog before the dialog exists
/// ([`Dialog::popup_tracker`]).
pub fn limits_following(popup: &TriggerBounds) -> LimitsProvider {
    let popup = popup.clone();
    Rc::new(move |window: &Window| {
        containment_limits(
            popup
                .get()
                .unwrap_or_else(|| Bounds::new(Default::default(), window.viewport_size())),
        )
    })
}

/// A modal or nonmodal dialog with a title, body text, form children and actions. See the
/// [module documentation](self).
pub struct Dialog {
    id: ElementId,
    title: SharedString,
    body: Option<SharedString>,
    form: Vec<AnyView>,
    model: DialogModel,
    modal: bool,
    width: Pixels,
    initial_focus: FocusTarget,
    final_focus: FocusTarget,
    trigger: Option<FocusHandle>,
    menu_bounds: Option<LimitsProvider>,
    font_family: Option<SharedString>,
    tokens: Option<ComponentTokens>,
    on_confirm: Option<Callback>,
    on_cancel: Option<Callback>,
    on_open_change: Option<OpenChange>,
    on_action: Option<ActionHandler>,
    on_key_down: Option<KeyHandler>,
    observer: Option<DialogObserver>,
    open: Option<OpenDialog>,
    popup_focus: FocusHandle,
    popup_bounds: TriggerBounds,
}

impl Dialog {
    /// A closed, modal dialog titled `title`. `id` keys the dialog's elements.
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            body: None,
            form: Vec::new(),
            model: DialogModel::default(),
            modal: true,
            width: POPUP_WIDTH,
            initial_focus: FocusTarget::Default,
            final_focus: FocusTarget::Default,
            trigger: None,
            menu_bounds: None,
            font_family: None,
            tokens: None,
            on_confirm: None,
            on_cancel: None,
            on_open_change: None,
            on_action: None,
            on_key_down: None,
            observer: None,
            open: None,
            // Tab index -1 and not a tab stop: the popup is focusable by script and pointer,
            // and the first Tab enters its first focusable descendant (R5, R6).
            popup_focus: cx.focus_handle().tab_index(-1),
            popup_bounds: TriggerBounds::new(),
        }
    }

    /// Body text below the title.
    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Adds a form child. Children are retained views: an editable input inside keeps its
    /// state, cursor and selection across re-renders while the dialog stays open (R13).
    pub fn child(mut self, child: impl Into<AnyView>) -> Self {
        self.form.push(child.into());
        self
    }

    /// The actions and the dismissal policy.
    pub fn model(mut self, model: DialogModel) -> Self {
        self.model = model;
        self
    }

    /// The actions in display order (the policy is kept).
    pub fn actions(mut self, actions: Vec<DialogAction>) -> Self {
        self.model.actions = actions;
        self
    }

    /// The dismissal policy: outside press and Escape (R10).
    pub fn policy(mut self, policy: DismissalPolicy) -> Self {
        self.model.policy = policy;
        self
    }

    /// Modal (the default) or nonmodal (R3, R7, R14).
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// Popup width (default 420 px; it never exceeds the window minus 16 px margins).
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// The owning trigger: the first closing-focus fallback (R15) and part of the dialog's
    /// focus contract (R6).
    pub fn trigger(mut self, trigger: FocusHandle) -> Self {
        self.trigger = Some(trigger);
        self
    }

    /// Initial focus (R6): [`FocusTarget::Default`] focuses the popup itself.
    pub fn initial_focus(mut self, target: FocusTarget) -> Self {
        self.initial_focus = target;
        self
    }

    /// Final focus (R6, R15): [`FocusTarget::Default`] follows the trigger, prior owner,
    /// window-default order.
    pub fn final_focus(mut self, target: FocusTarget) -> Self {
        self.final_focus = target;
        self
    }

    /// Owner-supplied bounds for contained menus (R11); without it the dialog's own border
    /// box is the limit (see [`Dialog::menu_limits_provider`]).
    pub fn menu_bounds(mut self, provider: impl Fn(&Window) -> Bounds<Pixels> + 'static) -> Self {
        self.menu_bounds = Some(Rc::new(provider));
        self
    }

    /// Uses a popup-bounds tracker created by the caller (see [`limits_following`]).
    pub fn popup_tracker(mut self, tracker: TriggerBounds) -> Self {
        self.popup_bounds = tracker;
        self
    }

    /// The font family of the dialog's text.
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    /// Overrides the token roles (default: [`ComponentTokens::current`]).
    pub fn tokens(mut self, tokens: ComponentTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Called when a primary action is activated (before the action callback and before the
    /// dialog closes). Dismissal by Escape or outside press never calls it.
    pub fn on_confirm(
        mut self,
        handler: impl Fn(&mut Window, &mut Context<Dialog>) + 'static,
    ) -> Self {
        self.on_confirm = Some(Rc::new(handler));
        self
    }

    /// Called when the dialog is cancelled by Escape or an outside press (R10).
    pub fn on_cancel(
        mut self,
        handler: impl Fn(&mut Window, &mut Context<Dialog>) + 'static,
    ) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    /// Called with `true` when the dialog opens and `false` when it closes, whatever the
    /// reason.
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut Context<Dialog>) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }

    /// Called for every activated action (R12), primary or not, after the confirm callback.
    pub fn on_action(
        mut self,
        handler: impl Fn(&Activation, &mut Window, &mut Context<Dialog>) + 'static,
    ) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    /// A key handler on the popup that runs before the dialog's own Tab handling and can veto
    /// it by preventing the event's default (R7).
    pub fn on_key_down(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut Context<Dialog>) + 'static,
    ) -> Self {
        self.on_key_down = Some(Rc::new(handler));
        self
    }

    /// Reports the painted bounds of the dialog's parts.
    pub fn observe_bounds(mut self, observer: DialogObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Whether the dialog is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether the dialog is modal.
    pub fn is_modal(&self) -> bool {
        self.modal
    }

    /// The popup's focus handle (initial focus target by default).
    pub fn popup_focus(&self) -> &FocusHandle {
        &self.popup_focus
    }

    /// The painted bounds of the popup, once painted.
    pub fn popup_bounds(&self) -> TriggerBounds {
        self.popup_bounds.clone()
    }

    /// The focus owner captured when the dialog opened (the focus-return handle), while open.
    pub fn focus_return(&self) -> Option<&FocusHandle> {
        self.open.as_ref().and_then(|open| open.layer.prior_focus())
    }

    /// The current actions and policy.
    pub fn current_model(&self) -> &DialogModel {
        &self.model
    }

    /// The bounds contained menus opened from the dialog stay inside (R11): the owner-supplied
    /// bounds, else the popup's border box inset by 8 px, else (before the first paint) the
    /// window inset by 8 px. Pass the result to
    /// [`Select::limits_provider`](crate::select::Select::limits_provider) so the menu follows
    /// the dialog.
    pub fn menu_limits_provider(&self) -> LimitsProvider {
        let popup = self.popup_bounds.clone();
        let owner = self.menu_bounds.clone();
        Rc::new(move |window: &Window| match &owner {
            Some(owner) => containment_limits(owner(window)),
            None => limits_following(&popup)(window),
        })
    }

    /// Replaces the actions (an owner marking an action busy or disabled); the policy is kept.
    pub fn set_actions(&mut self, actions: Vec<DialogAction>, cx: &mut Context<Self>) {
        self.model.actions = actions;
        cx.notify();
    }

    /// Sets the busy flag of one action.
    pub fn set_action_busy(&mut self, id: &str, busy: bool, cx: &mut Context<Self>) {
        if let Some(action) = self.model.actions.iter_mut().find(|a| a.id.as_ref() == id) {
            action.busy = busy;
            cx.notify();
        }
    }

    /// Sets the disabled flag of one action.
    pub fn set_action_disabled(&mut self, id: &str, disabled: bool, cx: &mut Context<Self>) {
        if let Some(action) = self.model.actions.iter_mut().find(|a| a.id.as_ref() == id) {
            action.disabled = disabled;
            cx.notify();
        }
    }

    /// Replaces the dismissal policy.
    pub fn set_policy(&mut self, policy: DismissalPolicy, cx: &mut Context<Self>) {
        self.model.policy = policy;
        cx.notify();
    }

    /// Opens the dialog: captures the focus owner (before the popup mounts), joins the
    /// dismissal stack and focuses the popup (or the initial target) once it is attached.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_some() {
            return;
        }
        let view = cx.entity().downgrade();
        let escape: EscapeHandler = Rc::new(move |window, cx| match view.upgrade() {
            Some(view) => view.update(cx, |this, cx| this.escape(window, cx)),
            None => EscapeOutcome::Dismissed,
        });
        let layer = open_layer(
            window,
            cx,
            LayerOptions {
                escape,
                layer_focus: self.popup_focus.clone(),
                trigger: self.trigger.clone(),
                initial_focus: self.initial_focus.clone(),
                final_focus: self.final_focus.clone(),
                trigger_first: true,
            },
        );
        self.open = Some(OpenDialog { layer });
        cx.notify();
        if let Some(handler) = self.on_open_change.clone() {
            handler(true, window, cx);
        }
    }

    /// Closes the dialog without calling the cancel callback: Escape stops reaching it at once,
    /// the popup unmounts and focus is restored after the current event (R15). The open-change
    /// callback runs with `false`.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else {
            return;
        };
        open.layer.close(window, cx);
        cx.notify();
        if let Some(handler) = self.on_open_change.clone() {
            handler(false, window, cx);
        }
    }

    /// Cancels the dialog: the cancel callback, then close.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_none() {
            return;
        }
        if let Some(handler) = self.on_cancel.clone() {
            handler(window, cx);
        }
        self.close(window, cx);
    }

    /// Activates an action by pointer or keyboard. Returns false (and does nothing) when the
    /// action is unknown, disabled or busy (R12). A primary action runs the confirm callback;
    /// every action runs the action callback; the dialog then closes unless the activated action
    /// is busy by then (a busy primary action keeps the dialog open) or keeps the dialog open.
    pub fn activate(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.open.is_none() {
            return false;
        }
        let Some(activation) = self.model.activate(id) else {
            return false;
        };
        self.run_activation(activation, id, window, cx);
        true
    }

    /// The default-action key (Enter in a single-line field): the primary action when it is
    /// enabled, otherwise nothing happens (R12). Returns whether an action ran.
    pub fn activate_default(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.open.is_none() {
            return false;
        }
        let Some(activation) = self.model.activate_default() else {
            return false;
        };
        let id = activation.id.clone();
        self.run_activation(activation, &id, window, cx);
        true
    }

    fn run_activation(
        &mut self,
        activation: Activation,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if activation.kind == ActionKind::Primary
            && let Some(handler) = self.on_confirm.clone()
        {
            handler(window, cx);
        }
        if let Some(handler) = self.on_action.clone() {
            handler(&activation, window, cx);
        }
        let stays = self
            .model
            .action(id)
            .is_some_and(|action| action.busy || action.keeps_open);
        if !stays {
            self.close(window, cx);
        }
        cx.notify();
    }

    /// Escape (R9, R10): a cancel when allowed; consumed either way.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> EscapeOutcome {
        match self.model.escape() {
            Dismissal::Cancel => {
                self.cancel(window, cx);
                EscapeOutcome::Dismissed
            }
            Dismissal::Ignored { .. } => EscapeOutcome::Consumed,
        }
    }

    /// A press on the backdrop or, for a nonmodal dialog, outside the popup (R4, R10). A nested
    /// layer above the dialog owns presses on its surface and consumes the others first.
    fn outside_press(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.open else {
            return;
        };
        if !is_top_layer(window, cx, open.layer.id()) {
            return;
        }
        if self.model.outside_press() == Dismissal::Cancel {
            self.cancel(window, cx);
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(handler) = self.on_key_down.clone() {
            handler(event, window, cx);
        }
        if !self.modal || window.default_prevented() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if keystroke.key != "tab" || modifiers.control || modifiers.alt || modifiers.platform {
            return;
        }
        // R7: the popup takes the Tab, wrapping inside its own tab stops.
        if modifiers.shift {
            window.focus_prev_within(&self.popup_focus, cx);
        } else {
            window.focus_next_within(&self.popup_focus, cx);
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn action_tokens(tokens: &ComponentTokens, kind: ActionKind) -> ComponentTokens {
        let mut tokens = tokens.clone();
        let (fill, text) = match kind {
            ActionKind::Primary => (tokens.accent, tokens.accent_contrast),
            ActionKind::Destructive => (tokens.danger, tokens.accent_contrast),
            ActionKind::Secondary => return tokens,
        };
        tokens.control_surface = fill;
        tokens.control_surface_hover = fill.blend(tokens.highlight);
        tokens.control_surface_pressed = fill.blend(hsla(0., 0., 0., 0.2));
        tokens.text = text;
        tokens
    }

    fn observe(&self, part: DialogPart) -> Option<impl IntoElement + use<>> {
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

    fn actions_row(&self, tokens: &ComponentTokens, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().flex().flex_row().justify_end().gap(px(8.)).mt(px(4.));
        for (ix, action) in self.model.actions.iter().enumerate() {
            let id = action.id.clone();
            let view = cx.entity().downgrade();
            let button = Button::new((self.id.clone(), action.id.clone()))
                .label(action.label.clone())
                .disabled(action.disabled)
                .busy(action.busy)
                .tokens(Self::action_tokens(tokens, action.kind))
                .on_click(move |_, window, cx| {
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |this, cx| {
                            this.activate(&id, window, cx);
                        });
                    }
                });
            row = row.child(
                div()
                    .relative()
                    .flex_none()
                    .child(button)
                    .children(self.observe(DialogPart::Action(ix))),
            );
        }
        row.into_any_element()
    }
}

impl Render for Dialog {
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

        let mut popup = div()
            .id((self.id.clone(), "popup"))
            .relative()
            .flex()
            .flex_col()
            .gap(px(12.))
            .w(self.width)
            .max_w(viewport.width - POPUP_MARGIN * 2.)
            .max_h(viewport.height - POPUP_MARGIN * 2.)
            .p(POPUP_PADDING)
            .bg(tokens.surface_dialog)
            .border_1()
            .border_color(tokens.border_strong)
            .rounded(tokens.surface_radius)
            .shadow_lg()
            .text_color(tokens.text)
            .text_size(px(13.))
            .role(Role::Dialog)
            .aria_label(self.title.clone())
            .track_focus(&self.popup_focus)
            .key_context(if self.modal {
                // `ModalScope` is `MODAL_SCOPE_CONTEXT`.
                "Dialog ModalScope"
            } else {
                "Dialog"
            })
            // The popup occludes everything behind it: a press inside never reaches the
            // backdrop (R4).
            .occlude()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key_down(event, window, cx)
            }))
            .child(
                div()
                    .id((self.id.clone(), "title"))
                    .role(Role::Heading)
                    .aria_level(TITLE_LEVEL)
                    .aria_label(self.title.clone())
                    .text_size(px(16.))
                    .child(self.title.clone()),
            );
        if let Some(family) = self.font_family.clone() {
            popup = popup.font_family(family);
        }
        if let Some(body) = self.body.clone() {
            popup = popup.child(div().text_color(tokens.text_muted).child(body));
        }
        for child in &self.form {
            popup = popup.child(child.clone());
        }
        if !self.model.actions.is_empty() {
            popup = popup.child(self.actions_row(&tokens, cx));
        }
        popup = popup.child(self.popup_bounds.probe());
        popup = popup.child(
            canvas(
                move |bounds, window, cx| set_layer_bounds(window, cx, layer, bounds),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        popup = popup.children(self.observe(DialogPart::Popup));
        if !self.modal {
            // A nonmodal dialog never consumes the press: the page stays interactive.
            let view = cx.entity().downgrade();
            popup = popup.child(
                OutsidePress {
                    layer,
                    on_dismiss: Rc::new(move |window: &mut Window, cx: &mut App| {
                        if let Some(view) = view.upgrade() {
                            view.update(cx, |this, cx| this.outside_press(window, cx));
                        }
                    }),
                    consume: false,
                    exempt: Vec::new(),
                }
                .probe(),
            );
        }

        let mut portal = div()
            .id((self.id.clone(), "portal"))
            .absolute()
            .top_0()
            .left_0()
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .items_center()
            .justify_center()
            // Explicitly transparent: the portal paints nothing itself.
            .bg(hsla(0., 0., 0., 0.));
        if self.modal {
            portal = portal.occlude().child(
                div()
                    .id((self.id.clone(), "backdrop"))
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .bg(tokens.scrim)
                    .occlude()
                    .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                        this.outside_press(window, cx);
                        cx.stop_propagation();
                    }))
                    .children(self.observe(DialogPart::Backdrop)),
            );
        }
        portal = portal
            .child(popup)
            .children(self.observe(DialogPart::Portal));

        deferred(
            anchored()
                .position(point(px(0.), px(0.)))
                .anchor(Anchor::TopLeft)
                .child(portal),
        )
        .with_priority(PORTAL_PRIORITY)
        .into_any_element()
    }
}
