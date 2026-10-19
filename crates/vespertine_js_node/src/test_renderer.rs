//! The offscreen test renderer (DEV-006) exposed to JavaScript.
//!
//! `TestRenderer` implements the renderer interface the framework adapters use, over the
//! production retained tree, scene, layout, hit testing, event dispatch and GPU renderer of an
//! offscreen window ([`vespertine_test_host::TestHost`]). The host lives on the JavaScript
//! thread. Every operation that can produce native events delivers them to the constructor
//! callback, in emission order, after the operation finished and before it returns, exactly as
//! the live renderer's UI thread delivers them (but synchronously, so a test observes their
//! effects when the helper returns).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::{Map, Value, json};
use vespertine_js::HostTreeHost;
use vespertine_test_host::{HostOptions, HostResult, NativeEvent, TestHost, TreeNode};

type Callback = FunctionRef<FnArgs<(Value, Value)>, ()>;

fn fail(error: impl std::fmt::Display) -> Error {
    Error::from_reason(error.to_string())
}

fn node_json(node: Option<TreeNode>) -> Value {
    node.map_or(Value::Null, |node| node.to_json())
}

fn nodes_json(nodes: Vec<TreeNode>) -> Value {
    Value::Array(nodes.into_iter().map(|node| node.to_json()).collect())
}

fn serialize<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(fail)
}

fn wire(event: NativeEvent) -> Value {
    let mut fields = match event.payload {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    fields.insert("type".into(), Value::String(event.event_type));
    fields.insert("id".into(), Value::from(event.element_id));
    Value::Object(fields)
}

fn dimension(options: &Value, name: &str) -> Result<Option<f64>> {
    match options.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| Error::from_reason(format!("{name} must be a number"))),
    }
}

/// Whether the offscreen test host can render on this machine (a GPU or software adapter exists).
#[napi]
pub fn test_host_available() -> bool {
    vespertine_test_host::is_available()
}

/// An offscreen window with the production renderer; see the module documentation.
#[napi]
pub struct TestRenderer {
    host: RefCell<TestHost>,
    generation: Arc<AtomicU64>,
    callback: Option<Callback>,
    selection_notifications: Rc<std::cell::Cell<bool>>,
    pending: Rc<RefCell<VecDeque<NativeEvent>>>,
}

#[napi]
impl TestRenderer {
    /// `callback(error, event)` receives every native event. `options` may carry `width`,
    /// `height` (logical pixels) and `scaleFactor`.
    #[napi(constructor)]
    pub fn new(
        callback: Option<Function<'_, FnArgs<(Value, Value)>, ()>>,
        options: Option<Value>,
    ) -> Result<Self> {
        let options = options.unwrap_or(Value::Null);
        let generation = Arc::new(AtomicU64::new(0));
        let pending = Rc::new(RefCell::new(VecDeque::new()));
        let mut host_options = HostOptions {
            width: dimension(&options, "width")?,
            height: dimension(&options, "height")?,
            ..HostOptions::default()
        };
        if let Some(scale) = dimension(&options, "scaleFactor")? {
            host_options.scale_factor = scale as f32;
        }
        let tree = HostTreeHost::new(generation.clone());
        let selection_notifications = tree.selection_notifications();
        let mut host = TestHost::with_tree(host_options, tree).map_err(fail)?;
        let queue = pending.clone();
        host.set_event_dispatcher(Some(Box::new(move |event| queue.borrow_mut().push_back(event))));
        Ok(Self {
            host: RefCell::new(host),
            generation,
            callback: callback.map(|callback| callback.create_ref()).transpose()?,
            selection_notifications,
            pending,
        })
    }

    /// Runs `operation` on the host, then delivers the native events it produced.
    fn run<T>(&self, env: &Env, operation: impl FnOnce(&mut TestHost) -> HostResult<T>) -> Result<T> {
        let result = {
            let mut host = self.host.borrow_mut();
            let result = operation(&mut host);
            host.dispatch_native_events();
            result
        };
        self.deliver(env)?;
        result.map_err(fail)
    }

    /// Like [`Self::run`] for operations that cannot fail.
    fn run_infallible<T>(&self, env: &Env, operation: impl FnOnce(&mut TestHost) -> T) -> Result<T> {
        self.run(env, |host| Ok(operation(host)))
    }

    /// Hands the queued events to the callback, in order. The host is not borrowed while the
    /// callback runs, so it may call back into the renderer.
    fn deliver(&self, env: &Env) -> Result<()> {
        loop {
            let Some(event) = self.pending.borrow_mut().pop_front() else {
                return Ok(());
            };
            if let Some(callback) = &self.callback {
                callback
                    .borrow_back(env)?
                    .call((Value::Null, wire(event)).into())?;
            }
        }
    }

    // ---- tree --------------------------------------------------------------------------------

    /// Applies a serialized mutation batch; returns the destroyed identities.
    #[napi]
    pub fn apply_batch(&self, batch: String) -> Result<Vec<f64>> {
        let destroyed = self.host.borrow_mut().apply_batch(&batch).map_err(fail)?;
        Ok(destroyed.into_iter().map(|id| id as f64).collect())
    }

    /// Announces the generation of the mounted root; window-level key events carry it.
    #[napi]
    pub fn set_window_generation(&self, generation: f64) {
        self.generation.store(generation as u64, Ordering::SeqCst);
    }

    /// Lays out and paints pending changes and runs due executor work; delivers native events.
    #[napi]
    pub fn flush(&self, env: &Env) -> Result<()> {
        self.run_infallible(env, |host| host.flush())
    }

    /// Takes the queued native events without delivering them.
    #[napi]
    pub fn drain_events(&self) -> Vec<Value> {
        let mut host = self.host.borrow_mut();
        host.dispatch_native_events();
        self.pending.borrow_mut().drain(..).map(wire).collect()
    }

    /// Delivers the queued native events to the callback; returns how many there were.
    #[napi]
    pub fn dispatch_native_events(&self, env: &Env) -> Result<u32> {
        let count = self.host.borrow_mut().dispatch_native_events();
        self.deliver(env)?;
        Ok(count as u32)
    }

    // ---- input -------------------------------------------------------------------------------

    #[napi]
    pub fn simulate_keystrokes(&self, env: &Env, keys: String, element_id: Option<f64>) -> Result<()> {
        self.run(env, |host| host.simulate_keystrokes(&keys, element_id.map(|id| id as u64)))
    }

    #[napi]
    pub fn native_simulate_keystrokes(&self, env: &Env, keys: String, element_id: Option<f64>) -> Result<()> {
        self.simulate_keystrokes(env, keys, element_id)
    }

    #[napi]
    pub fn native_simulate_key_down(&self, env: &Env, key: String, is_held: Option<bool>, element_id: Option<f64>) -> Result<()> {
        self.run(env, |host| host.key_down(&key, is_held.unwrap_or(false), element_id.map(|id| id as u64)))
    }

    #[napi]
    pub fn native_simulate_key_up(&self, env: &Env, key: String, element_id: Option<f64>) -> Result<()> {
        self.run(env, |host| host.key_up(&key, element_id.map(|id| id as u64)))
    }

    #[napi]
    pub fn native_simulate_click(&self, env: &Env, x: f64, y: f64, button: Option<u8>, modifiers: Option<String>) -> Result<()> {
        self.run(env, |host| host.click(x, y, button.unwrap_or(0), modifiers.as_deref().unwrap_or("")))
    }

    #[napi]
    pub fn native_simulate_mouse_down(&self, env: &Env, x: f64, y: f64, button: Option<u8>, modifiers: Option<String>) -> Result<()> {
        self.run(env, |host| host.mouse_down(x, y, button.unwrap_or(0), modifiers.as_deref().unwrap_or("")))
    }

    #[napi]
    pub fn native_simulate_mouse_up(&self, env: &Env, x: f64, y: f64, button: Option<u8>, modifiers: Option<String>) -> Result<()> {
        self.run(env, |host| host.mouse_up(x, y, button.unwrap_or(0), modifiers.as_deref().unwrap_or("")))
    }

    #[napi]
    pub fn native_simulate_mouse_move(&self, env: &Env, x: f64, y: f64, pressed_button: Option<u8>, modifiers: Option<String>) -> Result<()> {
        self.run(env, |host| host.mouse_move(x, y, pressed_button, modifiers.as_deref().unwrap_or("")))
    }

    #[napi]
    pub fn native_simulate_mouse_exit(&self, env: &Env, x: f64, y: f64) -> Result<()> {
        self.run(env, |host| host.mouse_exit(x, y))
    }

    /// Wheel input in pixel deltas at a point.
    #[napi]
    pub fn native_simulate_scroll_wheel(&self, env: &Env, x: f64, y: f64, delta_x: f64, delta_y: f64, modifiers: Option<String>) -> Result<()> {
        self.run(env, |host| host.scroll_wheel(x, y, delta_x, delta_y, modifiers.as_deref().unwrap_or("")))
    }

    #[napi]
    pub fn native_simulate_file_drop(&self, env: &Env, x: f64, y: f64, paths: Vec<String>) -> Result<()> {
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        self.run(env, |host| host.file_drop(x, y, &paths))
    }

    /// Drags from one point to another with the left button and returns the selected text.
    #[napi]
    pub fn drag_select(&self, env: &Env, from_x: f64, from_y: f64, to_x: f64, to_y: f64) -> Result<Option<String>> {
        self.run(env, |host| host.drag_select((from_x, from_y), (to_x, to_y)))
    }

    // ---- focus -------------------------------------------------------------------------------

    #[napi]
    pub fn focus_element(&self, env: &Env, id: f64) -> Result<()> {
        self.run(env, |host| host.focus_element(id as u64))
    }

    /// Removes focus from the focused element (from `id` when it holds the focus).
    #[napi]
    pub fn blur_element(&self, env: &Env, id: Option<f64>) -> Result<()> {
        self.run(env, |host| {
            if id.is_none_or(|id| host.focused_element() == Some(id as u64)) {
                host.blur()
            } else {
                Ok(())
            }
        })
    }

    #[napi]
    pub fn get_focused_element_id(&self) -> Option<f64> {
        self.host.borrow_mut().focused_element().map(|id| id as f64)
    }

    #[napi]
    pub fn get_focused_element(&self) -> Option<f64> {
        self.get_focused_element_id()
    }

    #[napi]
    pub fn focus_next(&self, env: &Env) -> Result<()> {
        self.run(env, |host| host.focus_next())
    }

    #[napi]
    pub fn focus_previous(&self, env: &Env) -> Result<()> {
        self.run(env, |host| host.focus_previous())
    }

    #[napi]
    pub fn focus_next_within(&self, env: &Env, id: f64) -> Result<bool> {
        self.run(env, |host| host.focus_next_within(id as u64))
    }

    #[napi]
    pub fn focus_previous_within(&self, env: &Env, id: f64) -> Result<bool> {
        self.run(env, |host| host.focus_previous_within(id as u64))
    }

    // ---- inspection --------------------------------------------------------------------------

    #[napi]
    pub fn get_root(&self) -> Result<Value> {
        Ok(node_json(self.host.borrow_mut().root_element().map_err(fail)?))
    }

    #[napi]
    pub fn get_element(&self, id: f64) -> Result<Value> {
        Ok(node_json(self.host.borrow_mut().element_by_id(id as u64).map_err(fail)?))
    }

    #[napi]
    pub fn find_by_type(&self, element_type: String) -> Result<Value> {
        Ok(nodes_json(self.host.borrow_mut().elements_by_type(&element_type).map_err(fail)?))
    }

    #[napi]
    pub fn find_by_text(&self, text: String) -> Result<Value> {
        Ok(nodes_json(self.host.borrow_mut().elements_by_text(&text).map_err(fail)?))
    }

    #[napi]
    pub fn find_by_test_id(&self, test_id: String) -> Result<Value> {
        Ok(nodes_json(self.host.borrow_mut().elements_by_test_id(&test_id).map_err(fail)?))
    }

    #[napi]
    pub fn get_all_text(&self) -> Result<Vec<String>> {
        let tree = self.host.borrow_mut().serialize_tree().map_err(fail)?;
        let mut texts = Vec::new();
        if let Some(tree) = tree {
            tree.walk(&mut |node| {
                if let Some(text) = &node.text {
                    texts.push(text.clone());
                }
            });
        }
        Ok(texts)
    }

    /// The strings painted in the last frame, in paint order.
    #[napi]
    pub fn get_painted_text(&self) -> Vec<String> {
        self.host.borrow().painted_text()
    }

    #[napi]
    pub fn get_painted_highlights(&self) -> Result<Value> {
        serialize(&self.host.borrow().painted_washes())
    }

    /// The last painted border box `{ x, y, width, height }` of an element, or null.
    #[napi]
    pub fn get_element_bounds(&self, id: f64) -> Result<Value> {
        match self.host.borrow().element_bounds(id as u64) {
            Some(rect) => serialize(&rect),
            None => Ok(Value::Null),
        }
    }

    /// The identities of the elements painted at a point, topmost first.
    #[napi]
    pub fn get_elements_at(&self, x: f64, y: f64) -> Vec<f64> {
        self.host.borrow().elements_at(x, y).into_iter().map(|id| id as f64).collect()
    }

    #[napi]
    pub fn get_retained_element_count(&self) -> Result<f64> {
        Ok(self.host.borrow_mut().element_count().map_err(fail)? as f64)
    }

    /// The retained tree from the root with painted bounds.
    #[napi(js_name = "toJSON")]
    pub fn to_json(&self) -> Result<Value> {
        Ok(node_json(self.host.borrow_mut().serialize_tree().map_err(fail)?))
    }

    #[napi]
    pub fn get_automation_tree(&self) -> Result<Value> {
        self.to_json()
    }

    /// The scroll offset `{ x, y }` of a scrolling element, or null.
    #[napi]
    pub fn get_scroll_offset(&self, id: f64) -> Result<Value> {
        Ok(self
            .host
            .borrow_mut()
            .scroll_offset(id as u64)
            .map_err(fail)?
            .map_or(Value::Null, |(x, y)| json!({ "x": x, "y": y })))
    }

    #[napi]
    pub fn get_list_scroll_top(&self, id: f64) -> Result<Value> {
        match self.host.borrow_mut().list_anchor(id as u64).map_err(fail)? {
            Some(anchor) => serialize(&anchor),
            None => Ok(Value::Null),
        }
    }

    #[napi]
    pub fn get_selected_text(&self) -> Result<Option<String>> {
        self.host.borrow_mut().selected_text().map_err(fail)
    }

    /// Enables or disables `windowSelectionChange` events.
    #[napi]
    pub fn set_selection_notifications(&self, enabled: bool) {
        self.selection_notifications.set(enabled);
    }

    #[napi]
    pub fn clear_selection(&self, env: &Env) -> Result<()> {
        self.run(env, |host| host.clear_selection())
    }

    #[napi]
    pub fn get_syntax_cache_stats(&self) -> Result<Value> {
        serialize(&self.host.borrow_mut().syntax_cache_counters().map_err(fail)?)
    }

    #[napi]
    pub fn get_accessibility_tree(&self) -> Result<Value> {
        Ok(self.host.borrow_mut().accessibility_tree().map_err(fail)?.unwrap_or(Value::Null))
    }

    /// The viewport `{ width, height }` in logical pixels.
    #[napi]
    pub fn get_window_size(&self) -> Result<Value> {
        serialize(&self.host.borrow_mut().viewport())
    }

    // ---- scrolling and content ---------------------------------------------------------------

    #[napi]
    pub fn scroll_to(&self, env: &Env, id: f64, x: f64, y: f64) -> Result<()> {
        self.run(env, |host| host.scroll_to(id as u64, x, y))
    }

    #[napi]
    pub fn scroll_to_item(&self, env: &Env, id: f64, index: f64, offset: Option<f64>) -> Result<()> {
        self.run(env, |host| host.scroll_to_item(id as u64, index as usize, offset.unwrap_or(0.0)))
    }

    #[napi]
    pub fn scroll_into_view(&self, env: &Env, id: f64, _options: Option<Value>) -> Result<()> {
        self.run(env, |host| host.scroll_into_view(id as u64))
    }

    #[napi]
    pub fn set_image(&self, env: &Env, id: f64, bytes: Uint8Array) -> Result<()> {
        self.run(env, |host| host.set_image_bytes(id as u64, &bytes))
    }

    #[napi]
    pub fn register_font(&self, env: &Env, bytes: Uint8Array) -> Result<()> {
        self.run(env, |host| host.register_font(bytes.to_vec()))
    }

    // ---- time and capture --------------------------------------------------------------------

    #[napi]
    pub fn clock_pause(&self, env: &Env) -> Result<f64> {
        self.run_infallible(env, |host| host.motion_pause())
    }

    #[napi]
    pub fn clock_set(&self, env: &Env, ms: f64) -> Result<f64> {
        self.run(env, |host| host.motion_set(ms))
    }

    #[napi]
    pub fn clock_fast_forward(&self, env: &Env, delta_ms: f64) -> Result<f64> {
        self.run(env, |host| host.motion_fast_forward(delta_ms))
    }

    #[napi]
    pub fn clock_resume(&self, env: &Env) -> Result<f64> {
        self.run_infallible(env, |host| host.motion_resume())
    }

    /// Advances the deterministic executor time (timers) by `ms` milliseconds.
    #[napi]
    pub fn advance_time(&self, env: &Env, ms: f64) -> Result<()> {
        self.run(env, |host| host.advance_executor_time(ms))
    }

    /// Saves a PNG of the GPU-rendered frame and returns the path.
    #[napi]
    pub fn capture_screenshot(&self, env: &Env, path: String) -> Result<String> {
        self.run(env, |host| host.screenshot(&path))?;
        Ok(path)
    }
}
