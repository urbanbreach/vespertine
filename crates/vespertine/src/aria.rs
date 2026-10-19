//! Declarative accessibility translation (A11Y-001).
//!
//! Host layers describe accessibility the way a document would: a DOM-style
//! `role` token and hyphenated `aria-*` properties. This module translates
//! those descriptions into the accessibility builders that elements already
//! have, so the AccessKit tree built each frame by the platform adapter stays
//! the single source of truth.
//!
//! * [`AriaRoleToken::parse`] maps the case-sensitive role tokens to AccessKit
//!   roles (R1-R3).
//! * [`AriaValue`] is the dynamically typed value a host passes for a property;
//!   [`Interactivity::apply_aria_attribute`] validates it (R4) and ignores
//!   invalid values without error.
//! * [`Interactivity::apply_role_token`] applies a role token (R1, R2).
//! * [`DeclarativeAria`] exposes both as builder methods on every stateful
//!   element, and [`AccessibleElement`] offers the existing accessibility
//!   builders to elements, such as virtual lists, that do not use
//!   [`Interactivity`] themselves (R7).

use crate::{
    ClickEvent, ElementId, GlobalElementId, Hitbox, Interactivity, List, Modifiers, MouseButton,
    MouseClickEvent, MouseDownEvent, MouseUpEvent, SharedString, StatefulInteractiveElement,
    Window,
};
use accesskit::Role;

/// The meaning of a recognized ARIA role token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AriaRoleToken {
    /// The token maps to this AccessKit role, which overrides the element's
    /// default role.
    Role(Role),
    /// `none` or `presentation`: the element's own semantics are suppressed, so
    /// no node is exported for it (descendants are unaffected).
    Presentation,
    /// `generic`, `subscript` or `superscript`: recognized, but the bridge never
    /// creates a semantic node for them (descendants are unaffected).
    NoNode,
}

impl AriaRoleToken {
    /// Looks up a role token. Tokens are case-sensitive; unknown strings
    /// (including `"Button"`) return `None`.
    pub fn parse(token: &str) -> Option<Self> {
        let role = match token {
            "none" | "presentation" => return Some(Self::Presentation),
            "generic" | "subscript" | "superscript" => return Some(Self::NoNode),
            "alert" => Role::Alert,
            "alertdialog" => Role::AlertDialog,
            "application" => Role::Application,
            "article" => Role::Article,
            "banner" => Role::Banner,
            "blockquote" => Role::Blockquote,
            "button" => Role::Button,
            "caption" => Role::Caption,
            "cell" => Role::Cell,
            "checkbox" => Role::CheckBox,
            "code" => Role::Code,
            "columnheader" => Role::ColumnHeader,
            "combobox" => Role::ComboBox,
            "complementary" => Role::Complementary,
            "contentinfo" => Role::ContentInfo,
            "definition" => Role::Definition,
            "deletion" => Role::ContentDeletion,
            "dialog" => Role::Dialog,
            "document" => Role::Document,
            "emphasis" => Role::Emphasis,
            "feed" => Role::Feed,
            "figure" => Role::Figure,
            "form" => Role::Form,
            "grid" => Role::Grid,
            "gridcell" => Role::GridCell,
            "group" => Role::Group,
            "heading" => Role::Heading,
            "img" => Role::Image,
            "insertion" => Role::ContentInsertion,
            "link" => Role::Link,
            "list" => Role::List,
            "listbox" => Role::ListBox,
            "listitem" => Role::ListItem,
            "log" => Role::Log,
            "main" => Role::Main,
            "marquee" => Role::Marquee,
            "math" => Role::Math,
            "menu" => Role::Menu,
            "menubar" => Role::MenuBar,
            "menuitem" => Role::MenuItem,
            "menuitemcheckbox" => Role::MenuItemCheckBox,
            "menuitemradio" => Role::MenuItemRadio,
            "meter" => Role::Meter,
            "navigation" => Role::Navigation,
            "note" => Role::Note,
            "option" => Role::ListBoxOption,
            "paragraph" => Role::Paragraph,
            "progressbar" => Role::ProgressIndicator,
            "radio" => Role::RadioButton,
            "radiogroup" => Role::RadioGroup,
            "region" => Role::Region,
            "row" => Role::Row,
            "rowgroup" => Role::RowGroup,
            "rowheader" => Role::RowHeader,
            "scrollbar" => Role::ScrollBar,
            "search" => Role::Search,
            "searchbox" => Role::SearchInput,
            "separator" => Role::Splitter,
            "slider" => Role::Slider,
            "spinbutton" => Role::SpinButton,
            "status" => Role::Status,
            "strong" => Role::Strong,
            "switch" => Role::Switch,
            "tab" => Role::Tab,
            "table" => Role::Table,
            "tablist" => Role::TabList,
            "tabpanel" => Role::TabPanel,
            "term" => Role::Term,
            "textbox" => Role::TextInput,
            "time" => Role::Time,
            "timer" => Role::Timer,
            "toolbar" => Role::Toolbar,
            "tooltip" => Role::Tooltip,
            "tree" => Role::Tree,
            "treegrid" => Role::TreeGrid,
            "treeitem" => Role::TreeItem,
            _ => return None,
        };
        Some(Self::Role(role))
    }
}

/// A dynamically typed property value, as a declarative host receives it.
#[derive(Clone, Debug, PartialEq)]
pub enum AriaValue {
    /// An explicit null; never a valid value for any property.
    Null,
    /// A boolean.
    Bool(bool),
    /// An integral number.
    Int(i128),
    /// A floating point number (possibly fractional, negative, or not finite).
    Number(f64),
    /// A string.
    String(SharedString),
}

impl AriaValue {
    fn as_nonempty_string(&self) -> Option<&SharedString> {
        match self {
            Self::String(value) if !value.is_empty() => Some(value),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// A positive integral level that fits the host's unsigned size type.
    fn as_level(&self) -> Option<usize> {
        match self {
            Self::Int(value) => usize::try_from(*value).ok().filter(|level| *level >= 1),
            Self::Number(value) => {
                // `usize::MAX as f64 + 1.0` is the exclusive power-of-two bound.
                let valid = value.is_finite()
                    && value.fract() == 0.0
                    && *value >= 1.0
                    && *value < usize::MAX as f64 + 1.0;
                valid.then_some(*value as usize)
            }
            _ => None,
        }
    }
}

impl From<bool> for AriaValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<&str> for AriaValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string().into())
    }
}

impl From<String> for AriaValue {
    fn from(value: String) -> Self {
        Self::String(value.into())
    }
}

impl From<SharedString> for AriaValue {
    fn from(value: SharedString) -> Self {
        Self::String(value)
    }
}

impl From<f64> for AriaValue {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

macro_rules! aria_value_from_int {
    ($($ty:ty),*) => {$(
        impl From<$ty> for AriaValue {
            fn from(value: $ty) -> Self {
                Self::Int(value as i128)
            }
        }
    )*};
}
aria_value_from_int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// What happened to a property passed to [`Interactivity::apply_aria_attribute`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AriaOutcome {
    /// The value was valid and the property is now set.
    Applied,
    /// The property is supported but the value was invalid, so nothing was set.
    InvalidValue,
    /// The property name is not one of the supported properties.
    UnknownProperty,
}

impl Interactivity {
    /// Applies a DOM-style `role` token. A recognized token replaces the role;
    /// an unknown token (including one with the wrong case) leaves the element
    /// on its default role; `none` and `presentation` suppress the element's
    /// own node, and `generic`, `subscript` and `superscript` never create one
    /// (descendants are unaffected). Returns whether the token was recognized.
    pub fn apply_role_token(&mut self, token: &str) -> bool {
        match AriaRoleToken::parse(token) {
            Some(AriaRoleToken::Role(role)) => {
                self.override_role = Some(role);
                self.aria.suppress_node = false;
            }
            Some(AriaRoleToken::Presentation | AriaRoleToken::NoNode) => {
                self.aria.suppress_node = true
            }
            None => return false,
        }
        true
    }

    /// Applies a hyphenated `aria-*` property. Supported properties are
    /// `aria-id`, `aria-label`, `aria-description` and `aria-valuetext`
    /// (nonempty strings), `aria-expanded` and `aria-selected` (booleans) and
    /// `aria-level` (positive integer). Invalid values are ignored.
    ///
    /// Passing `aria-label` always marks the label attribute as present, even
    /// when its value is rejected, which still blocks the `alt` fallback.
    pub fn apply_aria_attribute(&mut self, name: &str, value: &AriaValue) -> AriaOutcome {
        let aria = &mut self.aria;
        let applied = match name {
            "aria-id" => value
                .as_nonempty_string()
                .map(|v| aria.author_id = Some(v.clone()))
                .is_some(),
            "aria-label" => {
                aria.label_attribute_present = true;
                value
                    .as_nonempty_string()
                    .map(|v| aria.label = Some(v.clone()))
                    .is_some()
            }
            "aria-description" => value
                .as_nonempty_string()
                .map(|v| aria.description = Some(v.clone()))
                .is_some(),
            "aria-valuetext" => value
                .as_nonempty_string()
                .map(|v| aria.value = Some(v.clone()))
                .is_some(),
            "aria-expanded" => value.as_bool().map(|v| aria.expanded = Some(v)).is_some(),
            "aria-selected" => value.as_bool().map(|v| aria.selected = Some(v)).is_some(),
            "aria-level" => value.as_level().map(|v| aria.level = Some(v)).is_some(),
            _ => return AriaOutcome::UnknownProperty,
        };
        if applied {
            AriaOutcome::Applied
        } else {
            AriaOutcome::InvalidValue
        }
    }

    /// Removes a property previously set through [`Self::apply_aria_attribute`]
    /// (the attribute is no longer present). Returns false for unknown names.
    pub fn remove_aria_attribute(&mut self, name: &str) -> bool {
        let aria = &mut self.aria;
        match name {
            "aria-id" => aria.author_id = None,
            "aria-label" => {
                aria.label = None;
                aria.label_attribute_present = false;
            }
            "aria-description" => aria.description = None,
            "aria-valuetext" => aria.value = None,
            "aria-expanded" => aria.expanded = None,
            "aria-selected" => aria.selected = None,
            "aria-level" => aria.level = None,
            _ => return false,
        }
        true
    }

    /// The role exported for this element's own node, or `None` when it exports
    /// none. `GenericContainer` never forces a node, so assigning it behaves
    /// like assigning no role.
    pub(crate) fn a11y_role(&self) -> Option<Role> {
        if self.aria.suppress_node {
            return None;
        }
        self.override_role
            .filter(|role| *role != Role::GenericContainer)
    }

    /// Writes the image alternative text as the label when no label was given:
    /// the `alt` is used only if `aria-label` is absent (not even present with an
    /// invalid value) and `alt` is nonempty.
    pub(crate) fn write_alt_fallback(&self, node: &mut accesskit::Node) {
        let aria = &self.aria;
        if aria.label.is_none() && !aria.label_attribute_present {
            if let Some(alt) = aria.alt.as_ref().filter(|alt| !alt.is_empty()) {
                node.set_label(alt.to_string());
            }
        }
    }
}

/// An element that carries accessibility metadata: every stateful element and
/// the virtual [`List`].
pub trait AriaTarget: Sized {
    /// The element's accessibility state.
    fn aria_interactivity(&mut self) -> &mut Interactivity;
}

impl<T: StatefulInteractiveElement> AriaTarget for T {
    fn aria_interactivity(&mut self) -> &mut Interactivity {
        self.interactivity()
    }
}

impl AriaTarget for List {
    fn aria_interactivity(&mut self) -> &mut Interactivity {
        &mut self.a11y
    }
}

/// Declarative accessibility builders for every [`AriaTarget`].
pub trait DeclarativeAria: AriaTarget {
    /// Sets the DOM-style role token; see [`Interactivity::apply_role_token`].
    fn dom_role(mut self, token: &str) -> Self {
        self.aria_interactivity().apply_role_token(token);
        self
    }

    /// Sets a hyphenated `aria-*` property; invalid values are ignored. See
    /// [`Interactivity::apply_aria_attribute`].
    fn aria_attribute(mut self, name: &str, value: impl Into<AriaValue>) -> Self {
        self.aria_interactivity()
            .apply_aria_attribute(name, &value.into());
        self
    }
}

impl<T: AriaTarget> DeclarativeAria for T {}

macro_rules! list_aria_builders {
    ($($(#[$meta:meta])* $name:ident => $($field:ident).+: $ty:ty;)*) => {$(
        $(#[$meta])*
        pub fn $name(mut self, value: $ty) -> Self {
            self.a11y.$($field).+ = Some(value);
            self
        }
    )*};
}

macro_rules! list_aria_string_builders {
    ($($(#[$meta:meta])* $name:ident => $($field:ident).+;)*) => {$(
        $(#[$meta])*
        pub fn $name(mut self, value: impl Into<SharedString>) -> Self {
            self.a11y.$($field).+ = Some(value.into());
            self
        }
    )*};
}

/// The existing accessibility builders, applied directly to a virtual list
/// (R7). They mirror the builders of [`StatefulInteractiveElement`].
impl List {
    /// Gives the list a stable element identity, which an accessibility node
    /// requires.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.a11y.element_id = Some(id.into());
        self
    }

    /// Sets the accessible role. `Role::GenericContainer` behaves like no role.
    pub fn role(mut self, role: Role) -> Self {
        self.a11y.override_role = Some(role);
        self.a11y.aria.suppress_node = false;
        self
    }

    list_aria_string_builders! {
        /// Sets the author-provided identifier exposed to accessibility clients.
        accessibility_id => aria.author_id;
        /// Sets the accessible label.
        aria_label => aria.label;
        /// Sets the accessible description.
        aria_description => aria.description;
        /// Sets the accessible keyboard shortcut text.
        aria_keyshortcuts => aria.keyshortcuts;
        /// Sets the textual value.
        aria_value => aria.value;
        /// Sets the placeholder.
        aria_placeholder => aria.placeholder;
    }

    list_aria_builders! {
        /// Sets the selected state.
        aria_selected => aria.selected: bool;
        /// Sets the expanded state.
        aria_expanded => aria.expanded: bool;
        /// Sets the toggled state.
        aria_toggled => aria.toggled: accesskit::Toggled;
        /// Sets the numeric value.
        aria_numeric_value => aria.numeric_value: f64;
        /// Sets the numeric value step.
        aria_numeric_value_step => aria.numeric_value_step: f64;
        /// Sets the minimum numeric value.
        aria_min_numeric_value => aria.min_numeric_value: f64;
        /// Sets the maximum numeric value.
        aria_max_numeric_value => aria.max_numeric_value: f64;
        /// Sets the orientation.
        aria_orientation => aria.orientation: accesskit::Orientation;
        /// Sets the hierarchical level.
        aria_level => aria.level: usize;
        /// Sets the position in the containing set.
        aria_position_in_set => aria.position_in_set: usize;
        /// Sets the size of the containing set.
        aria_size_of_set => aria.size_of_set: usize;
        /// Sets the row index.
        aria_row_index => aria.row_index: usize;
        /// Sets the column index.
        aria_column_index => aria.column_index: usize;
        /// Sets the row count.
        aria_row_count => aria.row_count: usize;
        /// Sets the column count.
        aria_column_count => aria.column_count: usize;
    }
}

impl Interactivity {
    /// Exposes the accessible click action for a node subscribed to click.
    ///
    /// Invoking it delivers the ordinary click payload straight to the click
    /// listeners: a left-button press and release with click count 1 at the
    /// hitbox center and no modifiers (so `is_right_click()` is false). Nodes
    /// without a click subscription get no handler, and an explicit
    /// `on_a11y_action(Click, ..)` takes precedence.
    pub(crate) fn register_a11y_click(
        &self,
        global_id: Option<&GlobalElementId>,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        if !window.a11y.is_active()
            || self.click_listeners.is_empty()
            || self
                .a11y_action_listeners
                .iter()
                .any(|(action, _)| *action == accesskit::Action::Click)
        {
            return;
        }
        let Some(global_id) = global_id else {
            return;
        };
        let listeners = self.click_listeners.clone();
        let position = hitbox.bounds.center();
        window.on_a11y_action(
            global_id.accesskit_node_id(),
            accesskit::Action::Click,
            move |_, window, cx| {
                let event = ClickEvent::Mouse(MouseClickEvent {
                    down: MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    },
                    up: MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    },
                });
                for listener in &listeners {
                    listener(&event, window, cx);
                }
            },
        );
    }
}

impl From<&serde_json::Value> for AriaValue {
    fn from(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Bool(value) => Self::Bool(*value),
            serde_json::Value::Number(number) => {
                if let Some(int) = number.as_i64() {
                    Self::Int(int as i128)
                } else if let Some(int) = number.as_u64() {
                    Self::Int(int as i128)
                } else {
                    // Always representable as f64 without `arbitrary_precision`.
                    Self::Number(number.as_f64().unwrap_or(f64::NAN))
                }
            }
            serde_json::Value::String(value) => Self::String(value.clone().into()),
            // Arrays and objects are wrong types for every property.
            serde_json::Value::Null
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => Self::Null,
        }
    }
}

/// Applies the accessibility part of a host's property map to `element`: the
/// `role` token (R1-R2), every supported `aria-*` property with validation
/// (R4) and the image `alt` text (R5, which only takes effect on image hosts).
/// Keys that are not accessibility properties are skipped; invalid values are
/// ignored without error.
pub fn apply_aria_properties<E: AriaTarget>(
    mut element: E,
    props: &serde_json::Map<String, serde_json::Value>,
) -> E {
    let interactivity = element.aria_interactivity();
    // A non-string role is not a role token: the default role stays.
    if let Some(serde_json::Value::String(token)) = props.get("role") {
        interactivity.apply_role_token(token);
    }
    if let Some(serde_json::Value::String(alt)) = props.get("alt") {
        interactivity.aria.alt = Some(alt.clone().into());
    }
    for (name, value) in props {
        if name.starts_with("aria-") {
            interactivity.apply_aria_attribute(name, &AriaValue::from(value));
        }
    }
    element
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnyElement, App, AppContext as _, ClickEvent, Context, FocusHandle,
        InteractiveElement as _, IntoElement, ListAlignment, ListState, MouseButton,
        ParentElement as _, Render, Styled as _, TestAppContext, WindowHandle, div, img, list, px,
        svg,
    };
    use accesskit::{Action, NodeId};
    use serde_json::{Value, json};
    use std::{cell::RefCell, rc::Rc};

    type Build = Rc<dyn Fn() -> AnyElement>;

    struct Host {
        build: Rc<RefCell<Build>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let build = self.build.borrow().clone();
            build()
        }
    }

    struct Harness {
        window: WindowHandle<Host>,
        build: Rc<RefCell<Build>>,
    }

    impl Harness {
        fn new(cx: &mut TestAppContext, build: impl Fn() -> AnyElement + 'static) -> Self {
            let build: Rc<RefCell<Build>> = Rc::new(RefCell::new(Rc::new(build)));
            let window = cx.add_window({
                let build = build.clone();
                move |_, _| Host { build }
            });
            Self { window, build }
        }

        fn set_build(&self, build: impl Fn() -> AnyElement + 'static) {
            *self.build.borrow_mut() = Rc::new(build);
        }

        fn redraw(&self, cx: &mut TestAppContext) {
            self.window.update(cx, |_, _, cx| cx.notify()).unwrap();
            cx.update_window(self.window.into(), |_, window, cx| {
                window.draw(cx).clear(cx)
            })
            .unwrap();
        }

        fn dump(&self, cx: &mut TestAppContext) -> Option<Value> {
            cx.update_window(self.window.into(), |_, window, _| {
                window.debug_a11y_tree_json()
            })
            .unwrap()
            .map(|json| serde_json::from_str(&json).unwrap())
        }

        /// Simulates activation, draws one frame and returns the debug tree.
        fn tree(&self, cx: &mut TestAppContext) -> Value {
            cx.update_window(self.window.into(), |_, window, _| {
                window.set_a11y_active_for_tests(true)
            })
            .unwrap();
            self.redraw(cx);
            self.dump(cx).expect("an accessibility tree was built")
        }
    }

    fn root(children: Vec<AnyElement>) -> AnyElement {
        div()
            .id("root")
            .role(Role::Group)
            .size(px(400.))
            .children(children)
            .into_any_element()
    }

    fn by_author<'a>(tree: &'a Value, author: &str) -> Option<&'a Value> {
        tree["nodes"]
            .as_object()?
            .values()
            .find(|node| node["aria"]["author_id"] == author)
    }

    fn role_of(node: &Value) -> &str {
        node["aria"]["role"].as_str().unwrap()
    }

    fn node_count(tree: &Value) -> u64 {
        tree["frame"]["node_count"].as_u64().unwrap()
    }

    #[test]
    fn every_role_token_maps_to_its_role() {
        let table = [
            ("alert", Role::Alert),
            ("alertdialog", Role::AlertDialog),
            ("application", Role::Application),
            ("article", Role::Article),
            ("banner", Role::Banner),
            ("blockquote", Role::Blockquote),
            ("button", Role::Button),
            ("caption", Role::Caption),
            ("cell", Role::Cell),
            ("checkbox", Role::CheckBox),
            ("code", Role::Code),
            ("columnheader", Role::ColumnHeader),
            ("combobox", Role::ComboBox),
            ("complementary", Role::Complementary),
            ("contentinfo", Role::ContentInfo),
            ("definition", Role::Definition),
            ("deletion", Role::ContentDeletion),
            ("dialog", Role::Dialog),
            ("document", Role::Document),
            ("emphasis", Role::Emphasis),
            ("feed", Role::Feed),
            ("figure", Role::Figure),
            ("form", Role::Form),
            ("grid", Role::Grid),
            ("gridcell", Role::GridCell),
            ("group", Role::Group),
            ("heading", Role::Heading),
            ("img", Role::Image),
            ("insertion", Role::ContentInsertion),
            ("link", Role::Link),
            ("list", Role::List),
            ("listbox", Role::ListBox),
            ("listitem", Role::ListItem),
            ("log", Role::Log),
            ("main", Role::Main),
            ("marquee", Role::Marquee),
            ("math", Role::Math),
            ("menu", Role::Menu),
            ("menubar", Role::MenuBar),
            ("menuitem", Role::MenuItem),
            ("menuitemcheckbox", Role::MenuItemCheckBox),
            ("menuitemradio", Role::MenuItemRadio),
            ("meter", Role::Meter),
            ("navigation", Role::Navigation),
            ("note", Role::Note),
            ("option", Role::ListBoxOption),
            ("paragraph", Role::Paragraph),
            ("progressbar", Role::ProgressIndicator),
            ("radio", Role::RadioButton),
            ("radiogroup", Role::RadioGroup),
            ("region", Role::Region),
            ("row", Role::Row),
            ("rowgroup", Role::RowGroup),
            ("rowheader", Role::RowHeader),
            ("scrollbar", Role::ScrollBar),
            ("search", Role::Search),
            ("searchbox", Role::SearchInput),
            ("separator", Role::Splitter),
            ("slider", Role::Slider),
            ("spinbutton", Role::SpinButton),
            ("status", Role::Status),
            ("strong", Role::Strong),
            ("switch", Role::Switch),
            ("tab", Role::Tab),
            ("table", Role::Table),
            ("tablist", Role::TabList),
            ("tabpanel", Role::TabPanel),
            ("term", Role::Term),
            ("textbox", Role::TextInput),
            ("time", Role::Time),
            ("timer", Role::Timer),
            ("toolbar", Role::Toolbar),
            ("tooltip", Role::Tooltip),
            ("tree", Role::Tree),
            ("treegrid", Role::TreeGrid),
            ("treeitem", Role::TreeItem),
        ];
        for (token, role) in table {
            assert_eq!(
                AriaRoleToken::parse(token),
                Some(AriaRoleToken::Role(role)),
                "{token}"
            );
            // Role names are case-sensitive.
            let upper = token.to_uppercase();
            assert_eq!(AriaRoleToken::parse(&upper), None, "{upper}");
        }
        // 76 role tokens plus none, presentation, generic, subscript, superscript.
        assert_eq!(table.len(), 76);
        assert_eq!(
            AriaRoleToken::parse("none"),
            Some(AriaRoleToken::Presentation)
        );
        assert_eq!(
            AriaRoleToken::parse("presentation"),
            Some(AriaRoleToken::Presentation)
        );
        for token in ["generic", "subscript", "superscript"] {
            assert_eq!(AriaRoleToken::parse(token), Some(AriaRoleToken::NoNode));
        }
        assert_eq!(AriaRoleToken::parse("Button"), None);
        assert_eq!(AriaRoleToken::parse(""), None);
        assert_eq!(AriaRoleToken::parse("bogus"), None);
    }

    #[test]
    fn property_values_are_validated() {
        let mut interactivity = Interactivity::default();
        let mut apply =
            |name: &str, value: AriaValue| interactivity.apply_aria_attribute(name, &value);

        // Strings must be nonempty strings.
        for name in [
            "aria-id",
            "aria-label",
            "aria-description",
            "aria-valuetext",
        ] {
            for invalid in [
                AriaValue::from(""),
                AriaValue::Bool(true),
                AriaValue::Int(4),
                AriaValue::Number(1.5),
                AriaValue::Null,
            ] {
                assert_eq!(apply(name, invalid), AriaOutcome::InvalidValue, "{name}");
            }
        }
        // Booleans accept booleans only.
        for name in ["aria-expanded", "aria-selected"] {
            for invalid in [
                AriaValue::from("true"),
                AriaValue::Int(1),
                AriaValue::Number(1.0),
                AriaValue::Null,
            ] {
                assert_eq!(apply(name, invalid), AriaOutcome::InvalidValue, "{name}");
            }
        }
        // Levels are positive integers representable as usize.
        let overflow = usize::MAX as i128 + 1;
        for invalid in [
            AriaValue::Int(0),
            AriaValue::Int(-1),
            AriaValue::Int(overflow),
            AriaValue::Number(0.0),
            AriaValue::Number(-1.0),
            AriaValue::Number(1.5),
            AriaValue::Number(1e30),
            AriaValue::Number(usize::MAX as f64 + 1.0),
            AriaValue::Number(f64::NAN),
            AriaValue::Number(f64::INFINITY),
            AriaValue::from("2"),
            AriaValue::Bool(true),
            AriaValue::Null,
        ] {
            assert_eq!(
                apply("aria-level", invalid.clone()),
                AriaOutcome::InvalidValue,
                "{invalid:?}"
            );
        }
        assert_eq!(
            apply("aria-unknown", AriaValue::Bool(true)),
            AriaOutcome::UnknownProperty
        );
        assert_eq!(
            apply("ARIA-LABEL", AriaValue::from("x")),
            AriaOutcome::UnknownProperty
        );

        // Nothing was set by any invalid value.
        let aria = &interactivity.aria;
        assert!(aria.author_id.is_none() && aria.label.is_none() && aria.description.is_none());
        assert!(aria.value.is_none() && aria.expanded.is_none() && aria.selected.is_none());
        assert!(aria.level.is_none());

        let mut apply =
            |name: &str, value: AriaValue| interactivity.apply_aria_attribute(name, &value);
        assert_eq!(apply("aria-level", AriaValue::Int(3)), AriaOutcome::Applied);
        assert_eq!(
            apply("aria-expanded", AriaValue::Bool(false)),
            AriaOutcome::Applied
        );
        assert_eq!(
            apply("aria-selected", AriaValue::Bool(true)),
            AriaOutcome::Applied
        );
        assert_eq!(
            apply("aria-id", AriaValue::from("id")),
            AriaOutcome::Applied
        );
        assert_eq!(
            apply("aria-label", AriaValue::from("label")),
            AriaOutcome::Applied
        );
        assert_eq!(
            apply("aria-description", AriaValue::from("desc")),
            AriaOutcome::Applied
        );
        assert_eq!(
            apply("aria-valuetext", AriaValue::from("text")),
            AriaOutcome::Applied
        );
        assert_eq!(
            apply("aria-level", AriaValue::Number(2.0)),
            AriaOutcome::Applied
        );
        assert_eq!(
            apply("aria-level", AriaValue::Int(usize::MAX as i128)),
            AriaOutcome::Applied
        );
        let aria = &interactivity.aria;
        assert_eq!(aria.level, Some(usize::MAX));
        assert_eq!(aria.expanded, Some(false));
        assert_eq!(aria.selected, Some(true));
        assert_eq!(aria.author_id.as_deref(), Some("id"));
        assert_eq!(aria.label.as_deref(), Some("label"));
        assert_eq!(aria.description.as_deref(), Some("desc"));
        assert_eq!(aria.value.as_deref(), Some("text"));
    }

    #[test]
    fn json_properties_are_translated_and_validated() {
        let props = json!({
            "role": "heading",
            "aria-level": 3,
            "aria-label": "Title",
            "aria-expanded": "yes",
            "aria-selected": null,
            "aria-bogus": 1,
            "onclick": "ignored",
        });
        let element = apply_aria_properties(div().id("h"), props.as_object().unwrap());
        let interactivity = &element.element_interactivity_for_tests();
        assert_eq!(interactivity.a11y_role(), Some(Role::Heading));
        assert_eq!(interactivity.aria.level, Some(3));
        assert_eq!(interactivity.aria.label.as_deref(), Some("Title"));
        assert_eq!(interactivity.aria.expanded, None);
        assert_eq!(interactivity.aria.selected, None);

        // A non-string role keeps the default role; an overflowing u64 is a valid level.
        let props = json!({"role": 7, "aria-level": u64::MAX});
        let element =
            apply_aria_properties(div().id("h").role(Role::Group), props.as_object().unwrap());
        let interactivity = element.element_interactivity_for_tests();
        assert_eq!(interactivity.a11y_role(), Some(Role::Group));
        assert_eq!(interactivity.aria.level, Some(u64::MAX as usize));
    }

    trait InteractivityForTests {
        fn element_interactivity_for_tests(self) -> Interactivity;
    }

    impl<E: AriaTarget> InteractivityForTests for E {
        fn element_interactivity_for_tests(mut self) -> Interactivity {
            std::mem::take(self.aria_interactivity())
        }
    }

    #[vespertine::test]
    fn roles_suppression_and_property_validation_in_the_tree(cx: &mut TestAppContext) {
        let labeled_child = |name: &'static str| {
            div()
                .id(name)
                .role(Role::Label)
                .accessibility_id(name)
                .size(px(10.))
        };
        let host = Harness::new(cx, move || {
            root(vec![
                // Unknown and case-mismatched roles fall back to the default role.
                div()
                    .id("unknown")
                    .role(Role::Group)
                    .dom_role("bogus")
                    .aria_attribute("aria-id", "unknown")
                    .into_any_element(),
                div()
                    .id("case")
                    .role(Role::Group)
                    .dom_role("Button")
                    .aria_attribute("aria-id", "case")
                    .into_any_element(),
                // A recognized role overrides the default role.
                div()
                    .id("button")
                    .role(Role::Group)
                    .dom_role("button")
                    .aria_attribute("aria-id", "button")
                    .into_any_element(),
                // Suppressing and no-node roles export no node but keep descendants.
                div()
                    .id("none")
                    .role(Role::Group)
                    .dom_role("none")
                    .aria_attribute("aria-id", "none")
                    .child(labeled_child("none-child"))
                    .into_any_element(),
                div()
                    .id("presentation")
                    .role(Role::Group)
                    .dom_role("presentation")
                    .aria_attribute("aria-id", "presentation")
                    .child(labeled_child("presentation-child"))
                    .into_any_element(),
                div()
                    .id("generic")
                    .dom_role("generic")
                    .aria_attribute("aria-id", "generic")
                    .child(labeled_child("generic-child"))
                    .into_any_element(),
                div()
                    .id("subscript")
                    .role(Role::Group)
                    .dom_role("subscript")
                    .aria_attribute("aria-id", "subscript")
                    .child(labeled_child("subscript-child"))
                    .into_any_element(),
                div()
                    .id("superscript")
                    .role(Role::Group)
                    .dom_role("superscript")
                    .aria_attribute("aria-id", "superscript")
                    .child(labeled_child("superscript-child"))
                    .into_any_element(),
                // Levels.
                div()
                    .id("level-valid")
                    .dom_role("heading")
                    .aria_attribute("aria-id", "level-valid")
                    .aria_attribute("aria-level", 3)
                    .into_any_element(),
                div()
                    .id("level-zero")
                    .dom_role("heading")
                    .aria_attribute("aria-id", "level-zero")
                    .aria_attribute("aria-level", 0)
                    .into_any_element(),
                div()
                    .id("level-negative")
                    .dom_role("heading")
                    .aria_attribute("aria-id", "level-negative")
                    .aria_attribute("aria-level", -1)
                    .into_any_element(),
                div()
                    .id("level-fraction")
                    .dom_role("heading")
                    .aria_attribute("aria-id", "level-fraction")
                    .aria_attribute("aria-level", 1.5)
                    .into_any_element(),
                div()
                    .id("level-string")
                    .dom_role("heading")
                    .aria_attribute("aria-id", "level-string")
                    .aria_attribute("aria-level", "2")
                    .into_any_element(),
                div()
                    .id("level-overflow")
                    .dom_role("heading")
                    .aria_attribute("aria-id", "level-overflow")
                    .aria_attribute("aria-level", usize::MAX as f64 * 4.0)
                    .into_any_element(),
                // Booleans.
                div()
                    .id("expanded-bad")
                    .dom_role("button")
                    .aria_attribute("aria-id", "expanded-bad")
                    .aria_attribute("aria-expanded", "true")
                    .aria_attribute("aria-selected", 1)
                    .into_any_element(),
                div()
                    .id("expanded-good")
                    .dom_role("button")
                    .aria_attribute("aria-id", "expanded-good")
                    .aria_attribute("aria-expanded", true)
                    .aria_attribute("aria-selected", false)
                    .aria_attribute("aria-valuetext", "half")
                    .aria_attribute("aria-description", "d")
                    .into_any_element(),
            ])
        });
        let tree = host.tree(cx);

        for name in ["unknown", "case"] {
            assert_eq!(role_of(by_author(&tree, name).unwrap()), "Group", "{name}");
        }
        assert_eq!(role_of(by_author(&tree, "button").unwrap()), "Button");
        for name in [
            "none",
            "presentation",
            "generic",
            "subscript",
            "superscript",
        ] {
            assert!(by_author(&tree, name).is_none(), "{name} exports a node");
        }
        for name in [
            "none-child",
            "presentation-child",
            "generic-child",
            "subscript-child",
            "superscript-child",
        ] {
            assert_eq!(role_of(by_author(&tree, name).unwrap()), "Label", "{name}");
        }

        let level = |name: &str| by_author(&tree, name).unwrap()["aria"]["level"].clone();
        assert_eq!(level("level-valid"), json!(3));
        for name in [
            "level-zero",
            "level-negative",
            "level-fraction",
            "level-string",
            "level-overflow",
        ] {
            assert_eq!(role_of(by_author(&tree, name).unwrap()), "Heading");
            assert_eq!(level(name), Value::Null, "{name}");
        }

        let bad = by_author(&tree, "expanded-bad").unwrap();
        assert!(bad["aria"].get("expanded").is_none() && bad["aria"].get("selected").is_none());
        let good = by_author(&tree, "expanded-good").unwrap();
        assert_eq!(good["aria"]["expanded"], json!(true));
        assert_eq!(good["aria"]["selected"], json!(false));
        assert_eq!(good["aria"]["value"], json!("half"));
        assert_eq!(good["aria"]["description"], json!("d"));
    }

    #[vespertine::test]
    fn alt_is_a_label_fallback_only_without_an_aria_label(cx: &mut TestAppContext) {
        let host = Harness::new(cx, || {
            let image = |name: &'static str| {
                img(|_: &mut Window, _: &mut App| None)
                    .id(name)
                    .dom_role("img")
                    .aria_attribute("aria-id", name)
                    .size(px(10.))
            };
            root(vec![
                image("alt-only").alt("A cat").into_any_element(),
                image("alt-and-label")
                    .alt("A cat")
                    .aria_attribute("aria-label", "Pet")
                    .into_any_element(),
                image("alt-and-empty-label")
                    .alt("A cat")
                    .aria_attribute("aria-label", "")
                    .into_any_element(),
                image("alt-and-invalid-label")
                    .alt("A cat")
                    .aria_attribute("aria-label", 5)
                    .into_any_element(),
                image("empty-alt").alt("").into_any_element(),
                image("no-alt").into_any_element(),
                // The same rule applies to declarative property maps.
                apply_aria_properties(
                    image("map-alt"),
                    json!({"alt": "From map"}).as_object().unwrap(),
                )
                .into_any_element(),
                apply_aria_properties(
                    image("map-empty-label"),
                    json!({"alt": "From map", "aria-label": ""})
                        .as_object()
                        .unwrap(),
                )
                .into_any_element(),
            ])
        });
        let tree = host.tree(cx);
        let label = |name: &str| {
            by_author(&tree, name).unwrap()["aria"]
                .get("label")
                .cloned()
        };
        assert_eq!(label("alt-only"), Some(json!("A cat")));
        assert_eq!(label("alt-and-label"), Some(json!("Pet")));
        assert_eq!(label("alt-and-empty-label"), None);
        assert_eq!(label("alt-and-invalid-label"), None);
        assert_eq!(label("empty-alt"), None);
        assert_eq!(label("no-alt"), None);
        assert_eq!(label("map-alt"), Some(json!("From map")));
        assert_eq!(label("map-empty-label"), None);
        assert_eq!(role_of(by_author(&tree, "alt-only").unwrap()), "Image");
    }

    #[vespertine::test]
    fn properties_update_on_the_next_frame(cx: &mut TestAppContext) {
        let state = Rc::new(RefCell::new((String::from("First"), 1_i64, false)));
        let host = Harness::new(cx, {
            let state = state.clone();
            move || {
                let (label, level, expanded) = state.borrow().clone();
                root(vec![
                    div()
                        .id("heading")
                        .dom_role("heading")
                        .aria_attribute("aria-id", "heading")
                        .aria_attribute("aria-level", level)
                        .aria_attribute("aria-label", label.as_str())
                        .into_any_element(),
                    div()
                        .id("toggle")
                        .dom_role("button")
                        .aria_attribute("aria-id", "toggle")
                        .aria_attribute("aria-expanded", expanded)
                        .aria_attribute("aria-selected", !expanded)
                        .into_any_element(),
                ])
            }
        });
        let tree = host.tree(cx);
        let heading = by_author(&tree, "heading").unwrap();
        assert_eq!(heading["aria"]["label"], json!("First"));
        assert_eq!(heading["aria"]["level"], json!(1));
        assert_eq!(
            by_author(&tree, "toggle").unwrap()["aria"]["expanded"],
            json!(false)
        );

        *state.borrow_mut() = ("Second".into(), 2, true);
        host.redraw(cx);
        let tree = host.dump(cx).unwrap();
        let heading = by_author(&tree, "heading").unwrap();
        assert_eq!(heading["aria"]["label"], json!("Second"));
        assert_eq!(heading["aria"]["level"], json!(2));
        let toggle = by_author(&tree, "toggle").unwrap();
        assert_eq!(toggle["aria"]["expanded"], json!(true));
        assert_eq!(toggle["aria"]["selected"], json!(false));
    }

    #[vespertine::test]
    fn accessible_click_delivers_the_ordinary_click_payload(cx: &mut TestAppContext) {
        let clicks: Rc<RefCell<Vec<ClickEvent>>> = Rc::default();
        let host = Harness::new(cx, {
            let clicks = clicks.clone();
            move || {
                root(vec![
                    div()
                        .id("clickable")
                        .dom_role("button")
                        .aria_attribute("aria-id", "clickable")
                        .size(px(40.))
                        .on_click({
                            let clicks = clicks.clone();
                            move |event, _, _| clicks.borrow_mut().push(event.clone())
                        })
                        .into_any_element(),
                    div()
                        .id("plain")
                        .dom_role("button")
                        .aria_attribute("aria-id", "plain")
                        .size(px(40.))
                        .into_any_element(),
                ])
            }
        });
        let tree = host.tree(cx);
        let actions = |name: &str| by_author(&tree, name).unwrap()["aria"]["on_action"].clone();
        assert_eq!(actions("clickable"), json!(["Click"]));
        assert_eq!(
            actions("plain"),
            Value::Null,
            "no click action without a subscription"
        );

        let node_id = |name: &str| {
            let id = by_author(&tree, name).unwrap()["accesskit_id"]
                .as_str()
                .unwrap();
            NodeId(id.parse().unwrap())
        };
        let invoke = |cx: &mut TestAppContext, target_node: NodeId| {
            cx.update_window(host.window.into(), |_, window, cx| {
                window.handle_a11y_action(
                    accesskit::ActionRequest {
                        action: Action::Click,
                        target_tree: accesskit::TreeId::ROOT,
                        target_node,
                        data: None,
                    },
                    cx,
                )
            })
            .unwrap();
        };

        // A node without a subscription gets no synthesized handler.
        invoke(cx, node_id("plain"));
        assert!(clicks.borrow().is_empty());

        invoke(cx, node_id("clickable"));
        let clicks = clicks.borrow();
        assert_eq!(clicks.len(), 1);
        let ClickEvent::Mouse(mouse) = &clicks[0] else {
            panic!(
                "expected the ordinary mouse click payload, got {:?}",
                clicks[0]
            );
        };
        assert_eq!(mouse.down.button, MouseButton::Left);
        assert_eq!(mouse.up.button, MouseButton::Left);
        assert_eq!(clicks[0].click_count(), 1);
        assert!(!clicks[0].is_right_click());
        assert!(clicks[0].standard_click());
    }

    #[vespertine::test]
    fn image_vector_image_and_list_forward_accessibility_metadata(cx: &mut TestAppContext) {
        let host = Harness::new(cx, || {
            root(vec![
                img(|_: &mut Window, _: &mut App| None)
                    .id("image")
                    .role(Role::Image)
                    .aria_label("Image label")
                    .aria_description("Image description")
                    .aria_value("Image value")
                    .accessibility_id("image-id")
                    .size(px(10.))
                    .into_any_element(),
                svg()
                    .id("vector")
                    .role(Role::GraphicsObject)
                    .aria_label("Vector label")
                    .accessibility_id("vector-id")
                    .aria_expanded(true)
                    .size(px(10.))
                    .into_any_element(),
                list(ListState::new(0, ListAlignment::Top, px(10.)), |_, _, _| {
                    div().into_any_element()
                })
                .id("list")
                .role(Role::List)
                .aria_label("List label")
                .aria_level(4)
                .aria_size_of_set(9)
                .accessibility_id("list-id")
                .h(px(50.))
                .into_any_element(),
                // An empty author identifier is present; a missing one is omitted.
                img(|_: &mut Window, _: &mut App| None)
                    .id("empty-author")
                    .role(Role::Image)
                    .aria_label("Empty author")
                    .accessibility_id("")
                    .size(px(10.))
                    .into_any_element(),
                img(|_: &mut Window, _: &mut App| None)
                    .id("no-author")
                    .role(Role::Image)
                    .aria_label("No author")
                    .size(px(10.))
                    .into_any_element(),
            ])
        });
        let tree = host.tree(cx);

        let image = by_author(&tree, "image-id").unwrap();
        assert_eq!(role_of(image), "Image");
        assert_eq!(image["aria"]["label"], json!("Image label"));
        assert_eq!(image["aria"]["description"], json!("Image description"));
        assert_eq!(image["aria"]["value"], json!("Image value"));

        let vector = by_author(&tree, "vector-id").unwrap();
        assert_eq!(role_of(vector), "GraphicsObject");
        assert_eq!(vector["aria"]["label"], json!("Vector label"));
        assert_eq!(vector["aria"]["expanded"], json!(true));

        let list = by_author(&tree, "list-id").unwrap();
        assert_eq!(role_of(list), "List");
        assert_eq!(list["aria"]["label"], json!("List label"));
        assert_eq!(list["aria"]["level"], json!(4));
        assert_eq!(list["aria"]["size_of_set"], json!(9));

        let empty = by_author(&tree, "").expect("the empty identifier is present");
        assert_eq!(empty["aria"]["label"], json!("Empty author"));
        assert_eq!(empty["aria"]["author_id"], json!(""));

        let missing = tree["nodes"]
            .as_object()
            .unwrap()
            .values()
            .find(|node| node["aria"]["label"] == "No author")
            .unwrap();
        assert!(missing["aria"].get("author_id").is_none());
    }

    #[vespertine::test]
    fn generic_container_role_forces_no_node(cx: &mut TestAppContext) {
        let host = Harness::new(cx, || root(vec![]));
        let baseline = node_count(&host.tree(cx));

        host.set_build(|| {
            root(vec![
                img(|_: &mut Window, _: &mut App| None)
                    .id("image")
                    .role(Role::GenericContainer)
                    .aria_label("Image")
                    .size(px(10.))
                    .into_any_element(),
                list(ListState::new(0, ListAlignment::Top, px(10.)), |_, _, _| {
                    div().into_any_element()
                })
                .id("list")
                .role(Role::GenericContainer)
                .aria_label("List")
                .h(px(50.))
                .into_any_element(),
            ])
        });
        host.redraw(cx);
        let tree = host.dump(cx).unwrap();
        assert_eq!(node_count(&tree), baseline);
        assert!(
            tree["nodes"]
                .as_object()
                .unwrap()
                .values()
                .all(|node| role_of(node) != "GenericContainer" && node["aria"]["label"] != "Image")
        );
    }

    #[vespertine::test]
    fn simulated_accessibility_builds_a_tree_without_an_assistive_client(cx: &mut TestAppContext) {
        let host = Harness::new(cx, || root(vec![]));
        let is_active = |cx: &mut TestAppContext| {
            cx.update_window(host.window.into(), |_, window, _| window.is_a11y_active())
                .unwrap()
        };
        host.redraw(cx);
        assert!(host.dump(cx).is_none(), "no tree before activation");
        assert!(!is_active(cx));

        let tree = host.tree(cx);
        assert!(is_active(cx));
        let frame_number = tree["frame"]["frame_number"].clone();

        cx.update_window(host.window.into(), |_, window, _| {
            window.set_a11y_active_for_tests(false)
        })
        .unwrap();
        host.redraw(cx);
        assert!(!is_active(cx), "inactive on the following frame");
        assert_eq!(
            host.dump(cx).unwrap()["frame"]["frame_number"],
            frame_number,
            "no tree is built while inactive"
        );
    }

    #[vespertine::test]
    fn force_disabled_accessibility_ignores_simulated_activation(cx: &mut TestAppContext) {
        cx.disable_accessibility();
        let host = Harness::new(cx, || root(vec![]));
        cx.update_window(host.window.into(), |_, window, _| {
            window.set_a11y_active_for_tests(true)
        })
        .unwrap();
        host.redraw(cx);
        cx.update_window(host.window.into(), |_, window, _| {
            assert!(!window.is_a11y_enabled());
            assert!(!window.is_a11y_active());
        })
        .unwrap();
        assert!(host.dump(cx).is_none());
    }

    struct ActivityView {
        focus: FocusHandle,
    }

    impl Render for ActivityView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().id("activity").track_focus(&self.focus).size(px(10.))
        }
    }

    #[vespertine::test]
    fn simulated_activity_drives_observers_without_native_activation(cx: &mut TestAppContext) {
        let events: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let window = cx.add_window({
            let events = events.clone();
            move |window, cx| {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                cx.on_focus(&focus, window, {
                    let events = events.clone();
                    move |_, _, _| events.borrow_mut().push("focus")
                })
                .detach();
                cx.on_blur(&focus, window, {
                    let events = events.clone();
                    move |_, _, _| events.borrow_mut().push("blur")
                })
                .detach();
                cx.observe_window_activation(window, {
                    let events = events.clone();
                    move |_, window, _| {
                        events.borrow_mut().push(if window.is_window_active() {
                            "activated"
                        } else {
                            "deactivated"
                        })
                    }
                })
                .detach();
                ActivityView { focus }
            }
        });
        let any = window.into();
        let draw = |cx: &mut TestAppContext| {
            cx.update_window(any, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        };
        let set_active = |cx: &mut TestAppContext, active: bool| {
            cx.update_window(any, |_, window, cx| window.set_active_for_tests(active, cx))
                .unwrap();
            draw(cx);
        };
        let state = |cx: &mut TestAppContext| {
            let focused = window.read_with(cx, |view, _| view.focus.clone()).unwrap();
            cx.update_window(any, |_, window, cx| {
                (
                    window.is_window_active(),
                    window.focused(cx) == Some(focused),
                )
            })
            .unwrap()
        };

        draw(cx);
        assert_eq!(state(cx), (false, true));
        assert!(events.borrow().is_empty());

        set_active(cx, true);
        assert_eq!(state(cx), (true, true));
        assert_eq!(*events.borrow(), ["activated", "focus"]);

        set_active(cx, false);
        assert_eq!(state(cx), (false, true));
        assert_eq!(
            *events.borrow(),
            ["activated", "focus", "deactivated", "blur"]
        );

        // No request reached the window system, and the keyboard focus was never moved.
        assert_eq!(cx.update(|cx| cx.active_window()), None);
    }
}
