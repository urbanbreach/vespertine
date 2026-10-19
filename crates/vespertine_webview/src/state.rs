//! Page state, input state and events shared between the helper reader threads and the UI.

use serde_json::{Map, Value};

/// Error text of a page whose helper stopped (PLT-014 R13).
pub const ERROR_HELPER_STOPPED: &str =
    "The browser helper stopped. Check that WebKitGTK 4.1 is installed, then reopen the tab.";
/// Error text of a page whose helper stopped answering pings (PLT-014 R14).
pub const ERROR_HELPER_HUNG: &str = "The browser helper is not responding. Reload to restart it.";
/// Prefix of the error text of a page whose startup failed (PLT-014 R15).
pub const ERROR_OPEN_PREFIX: &str = "Could not open this page: ";

/// How a surface takes part in the window (PLT-015 R20).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub enum Presentation {
    /// Not painted, the helper is told "visible 0", no frames flow, no input.
    #[default]
    Hidden,
    /// Painted, frames flow, input is forwarded.
    Live,
    /// Painted and frames flow, but no input reaches the page.
    Passthrough,
}

impl Presentation {
    /// Maps the host's state to a presentation: hidden if the host is inactive, passthrough if
    /// active and a framework drag is in progress, live otherwise.
    pub fn for_host(active: bool, dragging: bool) -> Self {
        match (active, dragging) {
            (false, _) => Presentation::Hidden,
            (true, true) => Presentation::Passthrough,
            (true, false) => Presentation::Live,
        }
    }
}

/// The state of a page as last reported by the helper (CMP-022 R6).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PageState {
    pub url: Option<String>,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub error: Option<String>,
}

impl PageState {
    /// The label used for tab titles: the trimmed title when not empty, otherwise the host of the
    /// url, otherwise "Browser".
    pub fn label(&self) -> String {
        let title = self.title.trim();
        if !title.is_empty() {
            return title.to_owned();
        }
        if let Some(url) = &self.url
            && let Ok(parsed) = url::Url::parse(url)
            && let Some(host) = parsed.host_str()
            && !host.is_empty()
        {
            return host.to_owned();
        }
        "Browser".to_owned()
    }

    /// Parses a complete state snapshot (`S` message). A snapshot with an error is treated as not
    /// loading (PLT-015 R5).
    pub fn from_snapshot(object: &Map<String, Value>) -> Self {
        let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
        let flag = |key: &str| object.get(key).and_then(Value::as_bool).unwrap_or(false);
        let error = text("error");
        Self {
            url: text("url"),
            title: text("title").unwrap_or_default(),
            loading: flag("loading") && error.is_none(),
            can_back: flag("can_back"),
            can_forward: flag("can_forward"),
            error,
        }
    }
}

/// The input-method state reported by the helper (`I` messages), merged key by key
/// (PLT-015 R5, PLT-016 R13).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputState {
    pub focused: bool,
    /// Caret rectangle x, y, width, height in device pixels of the page.
    pub caret: Option<[i32; 4]>,
    /// Surrounding text and byte offsets of the cursor and the selection anchor.
    pub text: Option<String>,
    pub cursor: Option<usize>,
    pub selection: Option<usize>,
}

impl InputState {
    /// Merges the members present in `object`.
    pub fn merge(&mut self, object: &Map<String, Value>) {
        if let Some(focused) = object.get("focused").and_then(Value::as_bool) {
            self.focused = focused;
        }
        if let Some(Value::Array(values)) = object.get("caret")
            && values.len() == 4
        {
            let mut caret = [0i32; 4];
            let mut valid = true;
            for (slot, value) in caret.iter_mut().zip(values) {
                match value.as_f64() {
                    Some(number) => *slot = number as i32,
                    None => valid = false,
                }
            }
            if valid {
                self.caret = Some(caret);
            }
        }
        if let Some(text) = object.get("text").and_then(Value::as_str) {
            self.text = Some(text.to_owned());
        }
        if let Some(cursor) = object.get("cursor").and_then(Value::as_u64) {
            self.cursor = Some(cursor as usize);
        }
        if let Some(selection) = object.get("selection").and_then(Value::as_u64) {
            self.selection = Some(selection as usize);
        }
    }
}

/// One entry of a page-provided menu (`M` message).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub action: String,
    pub enabled: bool,
    pub selected: bool,
}

/// A page-provided menu (context menu or option menu) to show at a point of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRequest {
    /// Anchor in logical pixels relative to the page's top-left corner.
    pub x: f64,
    pub y: f64,
    pub items: Vec<MenuItem>,
}

impl MenuRequest {
    pub fn from_json(object: &Map<String, Value>) -> Self {
        let number = |key: &str| object.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let items = match object.get("items") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_object)
                .map(|item| MenuItem {
                    label: item
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    action: item
                        .get("action")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    enabled: item
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    selected: item
                        .get("selected")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect(),
            _ => Vec::new(),
        };
        Self {
            x: number("x"),
            y: number("y"),
            items,
        }
    }
}

/// A notification from a page's helper connection.
///
/// `StateChanged`, `FrameReady` and `InputStateChanged` carry no data: the latest values are
/// stored on the page and the notification may be dropped when the channel is full. Menu,
/// clipboard, new-tab and evaluation events carry data that cannot be reconstructed and are
/// always delivered.
#[derive(Clone, Debug, PartialEq)]
pub enum PageEvent {
    StateChanged,
    FrameReady,
    InputStateChanged,
    Menu(MenuRequest),
    Clipboard(String),
    NewTab(String),
    EvalResult(String),
}

impl PageEvent {
    /// Whether the event may be dropped when the event channel is full.
    pub fn is_lossy(&self) -> bool {
        matches!(
            self,
            PageEvent::StateChanged | PageEvent::FrameReady | PageEvent::InputStateChanged
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn presentation_mapping_table() {
        assert_eq!(Presentation::for_host(false, false), Presentation::Hidden);
        assert_eq!(Presentation::for_host(false, true), Presentation::Hidden);
        assert_eq!(
            Presentation::for_host(true, true),
            Presentation::Passthrough
        );
        assert_eq!(Presentation::for_host(true, false), Presentation::Live);
        assert_eq!(Presentation::default(), Presentation::Hidden);
    }

    #[test]
    fn labels_follow_title_host_then_default() {
        assert_eq!(PageState::default().label(), "Browser");
        let mut state = PageState {
            url: Some("http://localhost:3000/path".into()),
            ..PageState::default()
        };
        assert_eq!(state.label(), "localhost");
        state.title = "  Local preview ".into();
        assert_eq!(state.label(), "Local preview");
        state.title = "   ".into();
        assert_eq!(state.label(), "localhost");
        state.url = Some("not a url".into());
        assert_eq!(state.label(), "Browser");
    }

    #[test]
    fn snapshots_with_an_error_are_not_loading() {
        let state = PageState::from_snapshot(&object(
            json!({"url":"http://a/","title":"t","error":"boom","loading":true,"can_back":true}),
        ));
        assert!(!state.loading && state.can_back && !state.can_forward);
        assert_eq!(state.error.as_deref(), Some("boom"));
        let state = PageState::from_snapshot(&object(json!({"url":null,"loading":true})));
        assert!(state.loading && state.url.is_none() && state.error.is_none());
    }

    #[test]
    fn input_state_merges_key_by_key() {
        let mut state = InputState::default();
        state.merge(&object(json!({"focused":true,"caret":[1,2,3,4]})));
        state.merge(&object(json!({"text":"abc","cursor":1,"selection":2})));
        state.merge(&object(json!({"focused":false})));
        assert_eq!(
            state,
            InputState {
                focused: false,
                caret: Some([1, 2, 3, 4]),
                text: Some("abc".into()),
                cursor: Some(1),
                selection: Some(2)
            }
        );
        state.merge(&object(json!({"caret":[1,2]})));
        assert_eq!(state.caret, Some([1, 2, 3, 4]));
    }

    #[test]
    fn menus_parse_with_defaults() {
        let menu = MenuRequest::from_json(&object(json!({
            "x": 10.5, "y": 20.0,
            "items": [{"label":"Copy","action":"copy","enabled":true},{"label":"x"}, 5]
        })));
        assert_eq!((menu.x, menu.y), (10.5, 20.0));
        assert_eq!(menu.items.len(), 2);
        assert!(menu.items[0].enabled && !menu.items[0].selected);
        assert!(!menu.items[1].enabled && menu.items[1].action.is_empty());
    }
}
