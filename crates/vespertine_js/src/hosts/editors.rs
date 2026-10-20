//! The `input` (CMP-007 single-line field) and `textarea` (CMP-008 multiline editor) host types.
//!
//! Each host keeps one editor entity across frames. The entity shares the focus handle of the
//! host's node, so focus references, autofocus, tab order and the focus and blur events work
//! like they do for every other focusable host. The host's style draws the surface around the
//! editor; the editor's own surface and border stay transparent where the style declares a
//! background or border color, and its text color, size and line height follow the style.
//!
//! A `value` property is a controlled value: each change of the property is pushed to the editor
//! once (the echo of a value the editor emitted changes nothing, CMP-007 R5). `defaultValue`
//! seeds an uncontrolled editor when it is created.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Value, json};
use vespertine::{
    AnyElement, App, AppContext, ElementAdapter, ElementFactory, Entity, EventSet, FocusHandle,
    HostId, Hsla, IntoElement, Keystroke, ParentElement, RenderContext, SharedString, Styled,
    Subscription, Window, div,
};
use vespertine_components::text::{
    FieldTheme, SubmitMode, TextArea, TextAreaEvent, TextField, TextFieldEvent, TextMetrics,
};

use super::custom;

/// The standard events every editor host declares besides its own.
const INPUT_EVENTS: &[&str] = &[
    "click",
    "enter",
    "leave",
    "fileDrop",
    "change",
    "submit",
    "cancel",
    "pasteFiles",
    "pasteImages",
    "keyDown",
    "keyUp",
];

const INPUT_PROPERTIES: &[&str] = &[
    "value",
    "defaultValue",
    "placeholder",
    "readOnly",
    "disabled",
    "tabIndex",
];

const TEXTAREA_PROPERTIES: &[&str] = &[
    "value",
    "defaultValue",
    "placeholder",
    "readOnly",
    "disabled",
    "tabIndex",
    "minRows",
    "maxRows",
    "submitMode",
];

/// The channel from an editor entity's subscription to the host's listener. The subscription
/// outlives a render, so the emitter and the subscribed event set are refreshed every frame.
#[derive(Default)]
struct Link {
    emit: RefCell<Option<vespertine::EmitEvent>>,
    events: RefCell<EventSet>,
}

impl Link {
    fn attach(&self, context: &RenderContext) {
        *self.emit.borrow_mut() = Some(context.emit.clone());
        *self.events.borrow_mut() = context.events.clone();
    }

    /// Reports `name` to the host when it subscribed to it. `cancel` always goes: the editor
    /// consumes Escape, and the owner's dismissal layers (the top layer's Escape) learn of it
    /// only through this event.
    fn send(&self, name: &'static str, payload: Value, window: &mut Window, cx: &mut App) {
        if name != "cancel" && !self.events.borrow().contains(name) {
            return;
        }
        let emit = self.emit.borrow().clone();
        if let Some(emit) = emit {
            emit(custom(name, payload), window, cx);
        }
    }
}

fn key_payload(keystroke: &Keystroke, held: bool) -> Value {
    json!({
        "key": keystroke.key,
        "char": keystroke.key_char,
        "held": held,
        "shift": keystroke.modifiers.shift,
        "ctrl": keystroke.modifiers.control,
        "alt": keystroke.modifiers.alt,
        "cmd": keystroke.modifiers.platform,
    })
}

fn paths_payload(paths: &[std::path::PathBuf]) -> Value {
    json!({ "paths": paths.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>() })
}

/// What the host's style says about the editor: theme roles and text metrics.
fn look(context: &RenderContext, window: &Window, cx: &App) -> (FieldTheme, TextMetrics) {
    let style = context.style.to_refinement();
    let mut theme = FieldTheme::resolve(cx);
    let clear = Hsla::transparent_black();
    if style.background.is_some() {
        theme.surface = clear;
        theme.disabled_surface = clear;
    }
    if style.border_color.is_some() {
        theme.border = clear;
    }
    if let Some(color) = style.text.color {
        theme.text = color;
        theme.caret = color;
    }
    let rem = window.rem_size();
    let mut metrics = TextMetrics::default();
    if let Some(size) = style.text.font_size {
        metrics.size = size.to_pixels(rem);
        metrics.line_height = (metrics.size * 1.25).round();
    }
    if let Some(height) = style.text.line_height {
        metrics.line_height = height.to_pixels(metrics.size.into(), rem);
    }
    (theme, metrics)
}

/// The host surface: the host's style and standard events around the editor, which is centered
/// vertically in any height the style gives the host.
fn host_surface(mut context: RenderContext, editor: AnyElement) -> AnyElement {
    // The editor tracks the node's focus handle itself.
    context.focus_handle = None;
    let mut surface = context.surface();
    surface.extend([div()
        .flex()
        .flex_col()
        .size_full()
        .justify_center()
        .child(editor)
        .into_any_element()]);
    surface.into_any_element()
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn rows(value: &Value) -> Option<usize> {
    value
        .as_f64()
        .filter(|rows| rows.is_finite() && *rows >= 1.0)
        .map(|rows| rows as usize)
}

/// A property that is a boolean flag: anything else resets it.
fn flag(value: &Value) -> bool {
    value.as_bool().unwrap_or(false)
}

fn label(context: &RenderContext) -> Option<SharedString> {
    context
        .properties
        .get("aria-label")
        .and_then(Value::as_str)
        .map(|label| SharedString::from(label.to_owned()))
}

// ---- input (CMP-007) ----------------------------------------------------------------------------

pub(super) struct InputFactory;

impl ElementFactory for InputFactory {
    fn type_name(&self) -> &'static str {
        "input"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(InputAdapter::default())
    }
}

#[derive(Default)]
struct InputAdapter {
    field: Option<Entity<TextField>>,
    subscription: Option<Subscription>,
    link: Rc<Link>,
    /// The controlled value waiting to be pushed to the field.
    pending: Option<String>,
    initial: Option<String>,
    placeholder: SharedString,
    read_only: bool,
    disabled: bool,
    tab_stop: Option<bool>,
    applied_label: Option<SharedString>,
    applied_look: Option<(FieldTheme, TextMetrics)>,
}

impl InputAdapter {
    fn create_field(
        &mut self,
        focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<TextField> {
        let initial = self.pending.clone().or_else(|| self.initial.clone());
        let field = cx.new(|cx| {
            let field = match focus {
                Some(handle) => TextField::with_focus_handle(handle, window, cx),
                None => TextField::new(window, cx),
            };
            match &initial {
                Some(value) => field.value(value),
                None => field,
            }
        });
        let link = self.link.clone();
        self.subscription =
            Some(
                window.subscribe(&field, cx, move |_, event: &TextFieldEvent, window, cx| {
                    let sent = match event {
                        TextFieldEvent::Changed(value) => ("change", json!({ "value": value })),
                        TextFieldEvent::Submitted(value) => ("submit", json!({ "value": value })),
                        TextFieldEvent::Cancelled => ("cancel", json!({})),
                        TextFieldEvent::PastedPaths(paths) => ("pasteFiles", paths_payload(paths)),
                        TextFieldEvent::PastedImages(images) => {
                            ("pasteImages", json!({ "count": images.len() }))
                        }
                        TextFieldEvent::KeyDown(key) => {
                            ("keyDown", key_payload(&key.keystroke, key.is_held))
                        }
                        TextFieldEvent::KeyUp(key) => ("keyUp", key_payload(&key.keystroke, false)),
                        // The node's own listeners report focus and blur.
                        TextFieldEvent::Focused | TextFieldEvent::Blurred => return,
                    };
                    link.send(sent.0, sent.1, window, cx);
                }),
            );
        field
    }
}

impl ElementAdapter for InputAdapter {
    fn render(&mut self, context: RenderContext, window: &mut Window, cx: &mut App) -> AnyElement {
        self.link.attach(&context);
        let field = match self.field.clone() {
            Some(field) => field,
            None => {
                let field = self.create_field(context.focus_handle.clone(), window, cx);
                self.pending = None;
                self.field = Some(field.clone());
                field
            }
        };
        let look = look(&context, window, cx);
        let new_label = label(&context);
        field.update(cx, |field, cx| {
            if let Some(value) = self.pending.take() {
                field.acknowledge_or_apply(&value, window, cx);
            }
            if field.placeholder_text().as_ref() != self.placeholder.as_ref() {
                field.set_placeholder(self.placeholder.clone(), cx);
            }
            if field.is_read_only() != self.read_only {
                field.set_read_only(self.read_only, cx);
            }
            if field.is_disabled() != self.disabled {
                field.set_disabled(self.disabled, cx);
            }
            if self.applied_label != new_label {
                field.set_accessible_label(new_label.clone(), cx);
                self.applied_label = new_label.clone();
            }
            if self.applied_look.as_ref() != Some(&look) {
                field.set_theme(Some(look.0), cx);
                field.set_text_metrics(look.1.size, look.1.line_height, cx);
                self.applied_look = Some(look);
            }
            if let Some(tab_stop) = self.tab_stop.take() {
                field.set_tab_stop(tab_stop, cx);
            }
        });
        // The field's own node carries the label; the surface around it stays unlabelled.
        let mut context = context;
        let mut properties = (*context.properties).clone();
        properties.remove("aria-label");
        context.properties = Arc::new(properties);
        host_surface(context, field.into_any_element())
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        match name {
            "value" => self.pending = text(value),
            "defaultValue" => self.initial = text(value),
            "placeholder" => {
                self.placeholder = SharedString::from(text(value).unwrap_or_default());
            }
            "readOnly" => self.read_only = flag(value),
            "disabled" => self.disabled = flag(value),
            "tabIndex" => self.tab_stop = Some(value.as_i64().is_none_or(|index| index >= 0)),
            _ => {}
        }
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        INPUT_PROPERTIES
    }

    fn supported_events(&self) -> &'static [&'static str] {
        INPUT_EVENTS
    }

    fn destroy(&mut self, _cx: &mut App) {
        self.subscription = None;
        self.field = None;
    }
}

// ---- textarea (CMP-008) -------------------------------------------------------------------------

pub(super) struct TextareaFactory;

impl ElementFactory for TextareaFactory {
    fn type_name(&self) -> &'static str {
        "textarea"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(TextareaAdapter::default())
    }
}

struct TextareaAdapter {
    area: Option<Entity<TextArea>>,
    subscription: Option<Subscription>,
    link: Rc<Link>,
    pending: Option<String>,
    initial: Option<String>,
    placeholder: SharedString,
    read_only: bool,
    disabled: bool,
    tab_stop: Option<bool>,
    min_rows: usize,
    max_rows: usize,
    submit_mode: SubmitMode,
    applied_look: Option<(FieldTheme, TextMetrics)>,
    applied_rows: Option<(usize, usize)>,
    applied_submit: Option<SubmitMode>,
}

impl Default for TextareaAdapter {
    fn default() -> Self {
        Self {
            area: None,
            subscription: None,
            link: Rc::default(),
            pending: None,
            initial: None,
            placeholder: SharedString::default(),
            read_only: false,
            disabled: false,
            tab_stop: None,
            min_rows: 1,
            max_rows: 10,
            submit_mode: SubmitMode::default(),
            applied_look: None,
            applied_rows: None,
            applied_submit: None,
        }
    }
}

impl TextareaAdapter {
    fn create_area(
        &mut self,
        focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<TextArea> {
        let initial = self.pending.clone().or_else(|| self.initial.clone());
        let area = cx.new(|cx| {
            let area = match focus {
                Some(handle) => TextArea::with_focus_handle(handle, window, cx),
                None => TextArea::new(window, cx),
            };
            match &initial {
                Some(value) => area.value(value),
                None => area,
            }
        });
        let link = self.link.clone();
        self.subscription =
            Some(
                window.subscribe(&area, cx, move |_, event: &TextAreaEvent, window, cx| {
                    let sent = match event {
                        TextAreaEvent::Changed(value) => ("change", json!({ "value": value })),
                        TextAreaEvent::Submitted(value) => ("submit", json!({ "value": value })),
                        TextAreaEvent::Cancelled => ("cancel", json!({})),
                        TextAreaEvent::PastedPaths(paths) => ("pasteFiles", paths_payload(paths)),
                        TextAreaEvent::PastedImages(images) => {
                            ("pasteImages", json!({ "count": images.len() }))
                        }
                        TextAreaEvent::KeyDown(key) => {
                            ("keyDown", key_payload(&key.keystroke, key.is_held))
                        }
                        TextAreaEvent::KeyUp(key) => ("keyUp", key_payload(&key.keystroke, false)),
                        _ => return,
                    };
                    link.send(sent.0, sent.1, window, cx);
                }),
            );
        area
    }
}

fn submit_mode(value: &Value) -> SubmitMode {
    match value.as_str() {
        Some("enterSubmits") => SubmitMode::EnterSubmits,
        Some("modifiedEnter") => SubmitMode::ModifiedEnter,
        _ => SubmitMode::Enter,
    }
}

impl ElementAdapter for TextareaAdapter {
    fn render(&mut self, context: RenderContext, window: &mut Window, cx: &mut App) -> AnyElement {
        self.link.attach(&context);
        let area = match self.area.clone() {
            Some(area) => area,
            None => {
                let area = self.create_area(context.focus_handle.clone(), window, cx);
                self.pending = None;
                self.area = Some(area.clone());
                area
            }
        };
        let look = look(&context, window, cx);
        let rows = (self.min_rows, self.max_rows.max(self.min_rows));
        area.update(cx, |area, cx| {
            if let Some(value) = self.pending.take() {
                area.acknowledge_or_apply(&value, window, cx);
            }
            if area.placeholder_text().as_ref() != self.placeholder.as_ref() {
                area.set_placeholder(self.placeholder.clone(), cx);
            }
            if area.is_read_only() != self.read_only {
                area.set_read_only(self.read_only, cx);
            }
            if area.is_disabled() != self.disabled {
                area.set_disabled(self.disabled, cx);
            }
            if self.applied_look.as_ref() != Some(&look) {
                area.set_theme(Some(look.0), cx);
                area.set_text_metrics(look.1.size, look.1.line_height, cx);
                self.applied_look = Some(look);
            }
            if self.applied_rows != Some(rows) {
                area.set_rows(rows.0, rows.1, cx);
                self.applied_rows = Some(rows);
            }
            if self.applied_submit != Some(self.submit_mode) {
                area.set_submit_mode(self.submit_mode, cx);
                self.applied_submit = Some(self.submit_mode);
            }
            if let Some(tab_stop) = self.tab_stop.take() {
                area.set_tab_stop(tab_stop, cx);
            }
        });
        // The multiline editor exposes no label of its own: the host surface carries it.
        host_surface(context, area.into_any_element())
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        match name {
            "value" => self.pending = text(value),
            "defaultValue" => self.initial = text(value),
            "placeholder" => {
                self.placeholder = SharedString::from(text(value).unwrap_or_default());
            }
            "readOnly" => self.read_only = flag(value),
            "disabled" => self.disabled = flag(value),
            "tabIndex" => self.tab_stop = Some(value.as_i64().is_none_or(|index| index >= 0)),
            "minRows" => self.min_rows = rows(value).unwrap_or(1),
            "maxRows" => self.max_rows = rows(value).unwrap_or(10),
            "submitMode" => self.submit_mode = submit_mode(value),
            _ => {}
        }
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        TEXTAREA_PROPERTIES
    }

    fn supported_events(&self) -> &'static [&'static str] {
        INPUT_EVENTS
    }

    fn destroy(&mut self, _cx: &mut App) {
        self.subscription = None;
        self.area = None;
    }
}
