//! The automation client: backends, the application object, strict locators and gestures.

use super::{
    host::{AutomationHost, call},
    protocol::{AutomationError, Capability, ErrorCode},
    session::{InitInfo, LaunchOptions, Session, SessionOptions},
};
use crate::{Rect, TreeNode};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Instant,
};

/// Default locator wait timeout in milliseconds.
pub const DEFAULT_WAIT_TIMEOUT_MS: f64 = 5000.0;
/// Locator poll interval in milliseconds.
pub const POLL_INTERVAL_MS: f64 = 16.0;
/// Default number of interpolated move steps of a drag.
pub const DEFAULT_DRAG_STEPS: f64 = 8.0;

/// Something an [`Automation`] talks to: validates every request's parameters and result against
/// the method catalog and executes it.
pub trait Backend {
    /// Performs one catalog method.
    fn call(&self, method: &str, params: Value) -> Result<Value, AutomationError>;
    /// What `initialize` reported when the backend connected.
    fn info(&self) -> &InitInfo;
    /// Lets `ms` milliseconds pass while polling: an in-process backend advances the host's
    /// deterministic time, a process backend sleeps.
    fn sleep(&self, ms: f64);
    /// Milliseconds elapsed on the clock [`Backend::sleep`] advances; used for timeouts.
    fn now_ms(&self) -> f64;
    /// Closes the connection; later calls fail with `Closed`.
    fn close(&self);
}

/// A backend that drives an [`AutomationHost`] living in this process (typically a
/// [`TestHost`](crate::TestHost)) through the same validation and dispatch a stdio host uses.
pub struct InProcessBackend {
    host: Rc<RefCell<dyn AutomationHost>>,
    info: InitInfo,
    elapsed_ms: Cell<f64>,
    closed: Cell<bool>,
}

impl InProcessBackend {
    fn connect(host: Rc<RefCell<dyn AutomationHost>>) -> Result<Self, AutomationError> {
        let result = call(
            &mut *host.borrow_mut(),
            "initialize",
            json!({ "protocolVersion": super::protocol::PROTOCOL_VERSION, "client": "in-process" }),
        )?;
        Ok(Self {
            host,
            info: InitInfo::from_json(&result)?,
            elapsed_ms: Cell::new(0.0),
            closed: Cell::new(false),
        })
    }
}

impl Backend for InProcessBackend {
    fn call(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        if self.closed.get() {
            return Err(AutomationError::new(
                ErrorCode::Closed,
                "the session is closed",
            ));
        }
        call(&mut *self.host.borrow_mut(), method, params)
    }

    fn info(&self) -> &InitInfo {
        &self.info
    }

    fn sleep(&self, ms: f64) {
        self.host.borrow_mut().idle(ms);
        self.elapsed_ms.set(self.elapsed_ms.get() + ms);
    }

    fn now_ms(&self) -> f64 {
        self.elapsed_ms.get()
    }

    fn close(&self) {
        self.closed.set(true);
    }
}

/// A backend that talks to a host application over an SSE channel.
pub struct SessionBackend {
    session: Arc<Session>,
    info: InitInfo,
    start: Instant,
}

impl Backend for SessionBackend {
    fn call(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        self.session.call(method, params)
    }

    fn info(&self) -> &InitInfo {
        &self.info
    }

    fn sleep(&self, ms: f64) {
        std::thread::sleep(std::time::Duration::from_secs_f64(ms.max(0.0) / 1000.0));
    }

    fn now_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    fn close(&self) {
        self.session.close();
    }
}

fn not_found(message: String) -> AutomationError {
    AutomationError::new(ErrorCode::NotFound, message)
}

/// A point in window coordinates, or a locator resolved to the center of its painted bounds.
pub enum Target<'a> {
    /// A window point `(x, y)` in logical pixels.
    Point(f64, f64),
    /// The center of the painted bounds of exactly one element.
    Locator(&'a Locator),
}

impl From<(f64, f64)> for Target<'_> {
    fn from((x, y): (f64, f64)) -> Self {
        Target::Point(x, y)
    }
}

impl<'a> From<&'a Locator> for Target<'a> {
    fn from(locator: &'a Locator) -> Self {
        Target::Locator(locator)
    }
}

/// Options of a drag gesture.
#[derive(Clone, Debug)]
pub struct DragOptions {
    /// Number of interpolated move steps; floored and clamped to at least 1. Default 8.
    pub steps: f64,
    /// The button (0 left, 1 middle, 2 right).
    pub button: u8,
    /// Modifiers attached to every event of the gesture.
    pub modifiers: String,
    /// Offset added to the source center to get the start point.
    pub offset: (f64, f64),
}

impl Default for DragOptions {
    fn default() -> Self {
        Self {
            steps: DEFAULT_DRAG_STEPS,
            button: 0,
            modifiers: String::new(),
            offset: (0.0, 0.0),
        }
    }
}

fn frame_name(time_ms: f64) -> String {
    if time_ms.fract() == 0.0 && time_ms.abs() < 1e15 {
        format!("t{}.png", time_ms as i64)
    } else {
        format!("t{time_ms}.png")
    }
}

/// The application-automation object.
#[derive(Clone)]
pub struct Automation {
    backend: Rc<dyn Backend>,
}

impl Automation {
    /// Connects to an in-process host (typically a shared [`TestHost`](crate::TestHost)): requests
    /// are validated and dispatched like a stdio host's, against the same native paths.
    pub fn connect_in_process<H: AutomationHost + 'static>(
        host: Rc<RefCell<H>>,
    ) -> Result<Self, AutomationError> {
        let host: Rc<RefCell<dyn AutomationHost>> = host;
        Ok(Self {
            backend: Rc::new(InProcessBackend::connect(host)?),
        })
    }

    /// Connects to a host over an existing session (a byte channel). The session's incoming
    /// chunks must be fed by another thread while this call initializes the session.
    pub fn connect_session(session: Arc<Session>) -> Result<Self, AutomationError> {
        let info = session.initialize("vespertine-automation")?;
        Ok(Self {
            backend: Rc::new(SessionBackend {
                session,
                info,
                start: Instant::now(),
            }),
        })
    }

    /// Launches a command speaking the protocol on its standard input and output, initializes
    /// the session and returns once it is ready. Closing terminates the process.
    pub fn launch(
        launch: &LaunchOptions,
        options: SessionOptions,
    ) -> Result<Self, AutomationError> {
        let session = Session::launch(launch, options).map_err(|error| {
            AutomationError::new(
                ErrorCode::Closed,
                format!("could not launch {:?}: {error}", launch.command),
            )
        })?;
        Self::connect_session(Arc::new(session))
    }

    /// Connects over a custom backend.
    pub fn with_backend(backend: Rc<dyn Backend>) -> Self {
        Self { backend }
    }

    /// What `initialize` reported.
    pub fn info(&self) -> &InitInfo {
        self.backend.info()
    }

    /// Closes the connection.
    pub fn close(&self) {
        self.backend.close();
    }

    /// Performs a raw catalog request.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        self.backend.call(method, params)
    }

    fn ok(&self, method: &str, params: Value) -> Result<(), AutomationError> {
        self.request(method, params).map(drop)
    }

    /// A locator matching elements with this test identifier (exact equality).
    pub fn get_by_test_id(&self, test_id: &str) -> Locator {
        Locator::root(self.clone()).get_by_test_id(test_id)
    }

    /// A locator matching elements whose own text contains `text` (case-sensitive).
    pub fn get_by_text(&self, text: &str) -> Locator {
        Locator::root(self.clone()).get_by_text(text)
    }

    /// A locator matching elements of exactly this type.
    pub fn get_by_type(&self, element_type: &str) -> Locator {
        Locator::root(self.clone()).get_by_type(element_type)
    }

    /// The mouse gestures.
    pub fn mouse(&self) -> Mouse<'_> {
        Mouse { automation: self }
    }

    /// The motion clock controls.
    pub fn clock(&self) -> ClockControl<'_> {
        ClockControl { automation: self }
    }

    /// The retained tree with painted bounds, freshly fetched.
    pub fn tree(&self) -> Result<Option<TreeNode>, AutomationError> {
        let value = self.request("getTree", json!({}))?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|error| {
            AutomationError::new(ErrorCode::Protocol, format!("malformed tree: {error}"))
        })
    }

    fn strings(&self, method: &str) -> Result<Vec<String>, AutomationError> {
        let value = self.request(method, json!({}))?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect())
    }

    /// The strings painted in the last frame, in paint order.
    pub fn painted_text(&self) -> Result<Vec<String>, AutomationError> {
        self.strings("getPaintedText")
    }

    /// All retained text, in document order.
    pub fn all_text(&self) -> Result<Vec<String>, AutomationError> {
        self.strings("getAllText")
    }

    /// The painted bounds of element `id`; `None` when it has none.
    pub fn bounds_of(&self, id: u64) -> Result<Option<Rect>, AutomationError> {
        let value = self.request("getBounds", json!({ "elementId": id }))?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|error| {
            AutomationError::new(ErrorCode::Protocol, format!("malformed bounds: {error}"))
        })
    }

    /// The scroll offset of a container, `None` for elements that do not scroll.
    pub fn scroll_offset(&self, id: u64) -> Result<Option<(f64, f64)>, AutomationError> {
        let value = self.request("getScrollOffset", json!({ "elementId": id }))?;
        Ok(value
            .as_array()
            .and_then(|pair| Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))))
    }

    /// Scrolls a container to an absolute offset.
    pub fn scroll_to(&self, id: u64, x: f64, y: f64) -> Result<(), AutomationError> {
        self.ok("scrollTo", json!({ "elementId": id, "x": x, "y": y }))
    }

    /// Types whole keystroke sequences (space-separated tokens), optionally at an element.
    pub fn keystrokes(&self, keys: &str, element: Option<u64>) -> Result<(), AutomationError> {
        self.ok("keystrokes", with_element(json!({ "keys": keys }), element))
    }

    /// A single key press.
    pub fn key_down(
        &self,
        key: &str,
        held: bool,
        element: Option<u64>,
    ) -> Result<(), AutomationError> {
        self.ok(
            "keyDown",
            with_element(json!({ "key": key, "isHeld": held }), element),
        )
    }

    /// A single key release.
    pub fn key_up(&self, key: &str, element: Option<u64>) -> Result<(), AutomationError> {
        self.ok("keyUp", with_element(json!({ "key": key }), element))
    }

    /// Focuses an element by identity.
    pub fn focus(&self, id: u64) -> Result<(), AutomationError> {
        self.ok("focus", json!({ "elementId": id }))
    }

    /// Removes focus.
    pub fn blur(&self) -> Result<(), AutomationError> {
        self.ok("blur", json!({}))
    }

    /// The selected text.
    pub fn selected_text(&self) -> Result<Option<String>, AutomationError> {
        Ok(self
            .request("getSelectedText", json!({}))?
            .as_str()
            .map(str::to_owned))
    }

    /// Clears the selection.
    pub fn clear_selection(&self) -> Result<(), AutomationError> {
        self.ok("clearSelection", json!({}))
    }

    /// Saves a PNG of the painted frame to `path` and returns it.
    pub fn screenshot(&self, path: impl AsRef<Path>) -> Result<PathBuf, AutomationError> {
        let path = path.as_ref();
        let result = self.request("screenshot", json!({ "path": path.to_string_lossy() }))?;
        Ok(PathBuf::from(result["path"].as_str().unwrap_or_default()))
    }

    /// Captures a frame sequence: creates `directory` if needed, pauses the motion clock and,
    /// for each requested millisecond time, sets the clock to that absolute time and captures a
    /// PNG named `t<time>.png`. Returns the paths in order.
    pub fn capture_frames(
        &self,
        directory: impl AsRef<Path>,
        times_ms: &[f64],
    ) -> Result<Vec<PathBuf>, AutomationError> {
        let directory = directory.as_ref();
        std::fs::create_dir_all(directory).map_err(|error| {
            AutomationError::new(
                ErrorCode::Protocol,
                format!("could not create {}: {error}", directory.display()),
            )
        })?;
        self.clock().pause()?;
        let mut paths = Vec::with_capacity(times_ms.len());
        for &time in times_ms {
            self.clock().set(time)?;
            paths.push(self.screenshot(directory.join(frame_name(time)))?);
        }
        Ok(paths)
    }

    fn point(&self, target: Target<'_>) -> Result<(f64, f64), AutomationError> {
        match target {
            Target::Point(x, y) => Ok((x, y)),
            Target::Locator(locator) => locator.center(),
        }
    }

    fn mouse_event(
        &self,
        method: &str,
        (x, y): (f64, f64),
        button_key: &str,
        button: Option<u8>,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let mut params = json!({ "x": x, "y": y });
        if let Some(button) = button {
            params[button_key] = json!(button);
        }
        if !modifiers.is_empty() {
            params["modifiers"] = json!(modifiers);
        }
        self.ok(method, params)
    }
}

fn with_element(mut params: Value, element: Option<u64>) -> Value {
    if let Some(id) = element {
        params["elementId"] = json!(id);
    }
    params
}

/// Pointer gestures accepting window points or locators.
pub struct Mouse<'a> {
    automation: &'a Automation,
}

impl Mouse<'_> {
    /// Moves the pointer.
    pub fn move_to<'t>(&self, target: impl Into<Target<'t>>) -> Result<(), AutomationError> {
        let point = self.automation.point(target.into())?;
        self.automation
            .mouse_event("mouseMove", point, "pressedButton", None, "")
    }

    /// Presses a button.
    pub fn down<'t>(
        &self,
        target: impl Into<Target<'t>>,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let point = self.automation.point(target.into())?;
        self.automation
            .mouse_event("mouseDown", point, "button", Some(button), modifiers)
    }

    /// Releases a button.
    pub fn up<'t>(
        &self,
        target: impl Into<Target<'t>>,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let point = self.automation.point(target.into())?;
        self.automation
            .mouse_event("mouseUp", point, "button", Some(button), modifiers)
    }

    /// Clicks.
    pub fn click<'t>(
        &self,
        target: impl Into<Target<'t>>,
        button: u8,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let point = self.automation.point(target.into())?;
        self.automation
            .mouse_event("click", point, "button", Some(button), modifiers)
    }

    /// Wheel input in pixel deltas (scrolling content down is a negative `delta_y`).
    pub fn wheel<'t>(
        &self,
        target: impl Into<Target<'t>>,
        delta_x: f64,
        delta_y: f64,
        modifiers: &str,
    ) -> Result<(), AutomationError> {
        let (x, y) = self.automation.point(target.into())?;
        let mut params = json!({ "x": x, "y": y, "deltaX": delta_x, "deltaY": delta_y });
        if !modifiers.is_empty() {
            params["modifiers"] = json!(modifiers);
        }
        self.automation.ok("scrollWheel", params)
    }

    /// Drags from `source` to `destination`. Both points are resolved before anything is
    /// pressed (a locator destination can move under a live preview; the caller means where it
    /// is now). The pointer moves to the start, presses, issues `steps` linearly interpolated
    /// held-button moves (for step k the pointer is at `start + (destination - start) * k /
    /// steps`) and releases at the exact destination. The gesture's modifiers are attached to
    /// every event.
    pub fn drag<'s, 'd>(
        &self,
        source: impl Into<Target<'s>>,
        destination: impl Into<Target<'d>>,
        options: &DragOptions,
    ) -> Result<(), AutomationError> {
        let (sx, sy) = self.automation.point(source.into())?;
        let start = (sx + options.offset.0, sy + options.offset.1);
        let destination = self.automation.point(destination.into())?;
        self.drag_points(start, destination, options)
    }

    /// Drags from the source center plus the offset by `(dx, dy)`.
    pub fn drag_by<'s>(
        &self,
        source: impl Into<Target<'s>>,
        dx: f64,
        dy: f64,
        options: &DragOptions,
    ) -> Result<(), AutomationError> {
        let (sx, sy) = self.automation.point(source.into())?;
        let start = (sx + options.offset.0, sy + options.offset.1);
        self.drag_points(start, (start.0 + dx, start.1 + dy), options)
    }

    fn drag_points(
        &self,
        start: (f64, f64),
        destination: (f64, f64),
        options: &DragOptions,
    ) -> Result<(), AutomationError> {
        let automation = self.automation;
        let modifiers = options.modifiers.as_str();
        automation.mouse_event("mouseMove", start, "pressedButton", None, modifiers)?;
        automation.mouse_event(
            "mouseDown",
            start,
            "button",
            Some(options.button),
            modifiers,
        )?;
        let steps = options.steps.floor().max(1.0);
        let count = steps as u64;
        for step in 1..=count {
            let fraction = step as f64 / steps;
            let point = (
                start.0 + (destination.0 - start.0) * fraction,
                start.1 + (destination.1 - start.1) * fraction,
            );
            automation.mouse_event(
                "mouseMove",
                point,
                "pressedButton",
                Some(options.button),
                modifiers,
            )?;
        }
        automation.mouse_event(
            "mouseUp",
            destination,
            "button",
            Some(options.button),
            modifiers,
        )
    }
}

/// The motion clock.
pub struct ClockControl<'a> {
    automation: &'a Automation,
}

impl ClockControl<'_> {
    fn now(&self, method: &str, params: Value) -> Result<f64, AutomationError> {
        Ok(self.automation.request(method, params)?["nowMs"]
            .as_f64()
            .unwrap_or_default())
    }

    /// Freezes the clock at the current instant and returns it in milliseconds.
    pub fn pause(&self) -> Result<f64, AutomationError> {
        self.now("clockPause", json!({}))
    }

    /// Freezes the clock at an absolute elapsed time.
    pub fn set(&self, now_ms: f64) -> Result<f64, AutomationError> {
        self.now("clockSet", json!({ "nowMs": now_ms }))
    }

    /// Adds `delta_ms` to the current instant and freezes there.
    pub fn fast_forward(&self, delta_ms: f64) -> Result<f64, AutomationError> {
        self.now("clockFastForward", json!({ "deltaMs": delta_ms }))
    }

    /// Resumes from the frozen value.
    pub fn resume(&self) -> Result<f64, AutomationError> {
        self.now("clockResume", json!({}))
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Selector {
    TestId(String),
    Type(String),
    Text(String),
}

impl Selector {
    fn matches(&self, node: &TreeNode) -> bool {
        match self {
            Selector::TestId(id) => node.test_id.as_deref() == Some(id),
            Selector::Type(element_type) => &node.element_type == element_type,
            Selector::Text(text) => node.text.as_deref().unwrap_or("").contains(text.as_str()),
        }
    }

    fn describe(&self) -> String {
        match self {
            Selector::TestId(id) => format!("testId={id:?}"),
            Selector::Type(element_type) => format!("type={element_type:?}"),
            Selector::Text(text) => format!("text~{text:?}"),
        }
    }
}

/// A strict locator. Locators cache nothing: every operation evaluates its selectors against a
/// freshly fetched retained tree.
#[derive(Clone)]
pub struct Locator {
    automation: Automation,
    selectors: Vec<Selector>,
}

impl Locator {
    fn root(automation: Automation) -> Self {
        Self {
            automation,
            selectors: Vec::new(),
        }
    }

    fn then(&self, selector: Selector) -> Locator {
        let mut selectors = self.selectors.clone();
        selectors.push(selector);
        Locator {
            automation: self.automation.clone(),
            selectors,
        }
    }

    /// A nested locator: matches test identifier `test_id` among the descendants of every node
    /// this locator matches (never the matched node itself).
    pub fn get_by_test_id(&self, test_id: &str) -> Locator {
        self.then(Selector::TestId(test_id.to_owned()))
    }

    /// A nested locator by own-text substring.
    pub fn get_by_text(&self, text: &str) -> Locator {
        self.then(Selector::Text(text.to_owned()))
    }

    /// A nested locator by exact element type.
    pub fn get_by_type(&self, element_type: &str) -> Locator {
        self.then(Selector::Type(element_type.to_owned()))
    }

    fn describe(&self) -> String {
        self.selectors
            .iter()
            .map(Selector::describe)
            .collect::<Vec<_>>()
            .join(" >> ")
    }

    /// Every matching node in document order, without repeats. Never fails on zero or many
    /// matches.
    pub fn all(&self) -> Result<Vec<TreeNode>, AutomationError> {
        let Some(root) = self.automation.tree()? else {
            return Ok(Vec::new());
        };
        // Pre-order flattening; `end` is the exclusive end of the node's subtree.
        let mut flat: Vec<(&TreeNode, usize)> = Vec::new();
        fn flatten<'a>(node: &'a TreeNode, flat: &mut Vec<(&'a TreeNode, usize)>) {
            let index = flat.len();
            flat.push((node, index + 1));
            for child in &node.children {
                flatten(child, flat);
            }
            flat[index].1 = flat.len();
        }
        flatten(&root, &mut flat);

        let mut selectors = self.selectors.iter();
        let mut matched: BTreeSet<usize> = match selectors.next() {
            // No selector: the whole tree including the root.
            None => (0..flat.len()).collect(),
            Some(first) => (0..flat.len())
                .filter(|&ix| first.matches(flat[ix].0))
                .collect(),
        };
        for selector in selectors {
            let mut next = BTreeSet::new();
            for &ix in &matched {
                for candidate in ix + 1..flat[ix].1 {
                    if selector.matches(flat[candidate].0) {
                        next.insert(candidate);
                    }
                }
            }
            matched = next;
        }
        Ok(matched.into_iter().map(|ix| flat[ix].0.clone()).collect())
    }

    /// The number of matches.
    pub fn count(&self) -> Result<usize, AutomationError> {
        Ok(self.all()?.len())
    }

    /// The single matching node. Fails with `NotFound` for zero matches and `Ambiguous`
    /// (reporting the count) for several; it never picks the first.
    pub fn element(&self) -> Result<TreeNode, AutomationError> {
        let mut nodes = self.all()?;
        match nodes.len() {
            1 => Ok(nodes.remove(0)),
            0 => Err(not_found(format!("no element matches {}", self.describe()))),
            count => Err(AutomationError::with_data(
                ErrorCode::Ambiguous,
                format!(
                    "{count} elements match {}; a strict locator needs exactly one",
                    self.describe()
                ),
                json!({ "count": count }),
            )),
        }
    }

    /// The painted bounds of the single matching element: the bounds attached to its tree node
    /// or, failing that, a bounds query by id. Fails with `NotFound` when the element has no
    /// painted bounds (not painted, hidden or speculative).
    pub fn bounds(&self) -> Result<Rect, AutomationError> {
        let node = self.element()?;
        if let Some(bounds) = node.bounds {
            return Ok(bounds);
        }
        self.automation.bounds_of(node.id)?.ok_or_else(|| {
            not_found(format!(
                "element {} ({}) has no painted bounds",
                node.id,
                self.describe()
            ))
        })
    }

    /// The center of the painted bounds: `(x + width / 2, y + height / 2)`.
    pub fn center(&self) -> Result<(f64, f64), AutomationError> {
        Ok(self.bounds()?.center())
    }

    /// Clicks the center of the painted bounds.
    pub fn click(&self) -> Result<(), AutomationError> {
        self.automation.mouse().click(self, 0, "")
    }

    /// Moves the pointer to the center of the painted bounds.
    pub fn hover(&self) -> Result<(), AutomationError> {
        self.automation.mouse().move_to(self)
    }

    /// Wheel input at the center of the painted bounds.
    pub fn wheel(&self, delta_x: f64, delta_y: f64) -> Result<(), AutomationError> {
        self.automation.mouse().wheel(self, delta_x, delta_y, "")
    }

    /// Drags this element's center (plus the options' offset) to a target.
    pub fn drag_to<'t>(
        &self,
        destination: impl Into<Target<'t>>,
        options: &DragOptions,
    ) -> Result<(), AutomationError> {
        self.automation.mouse().drag(self, destination, options)
    }

    /// Drags from this element's center (plus the options' offset) by `(dx, dy)`.
    pub fn drag_by(&self, dx: f64, dy: f64, options: &DragOptions) -> Result<(), AutomationError> {
        self.automation.mouse().drag_by(self, dx, dy, options)
    }

    /// Replaces the content of a single element: focuses it by sending keystrokes targeted at
    /// it, selects all (`ctrl-a`; `cmd-a` on macOS), then types `text`. An empty replacement
    /// sends backspace after select-all. The space character is the token `space`, a newline
    /// `enter`, a tab `tab`; any other character is itself.
    pub fn fill(&self, text: &str) -> Result<(), AutomationError> {
        let id = self.element()?.id;
        let select_all = if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        };
        self.automation.keystrokes(select_all, Some(id))?;
        if text.is_empty() {
            return self.automation.keystrokes("backspace", Some(id));
        }
        let tokens: Vec<String> = text
            .chars()
            .map(|character| match character {
                ' ' => "space".to_owned(),
                '\n' => "enter".to_owned(),
                '\t' => "tab".to_owned(),
                other => other.to_string(),
            })
            .collect();
        self.automation.keystrokes(&tokens.join(" "), Some(id))
    }

    /// Sends keystrokes to the single matching element through the native keyboard path.
    pub fn press(&self, keys: &str) -> Result<(), AutomationError> {
        let id = self.element()?.id;
        self.automation.keystrokes(keys, Some(id))
    }

    /// The text content: the node's own text followed by the text content of each descendant in
    /// document order, with no separators.
    pub fn text_content(&self) -> Result<String, AutomationError> {
        Ok(self.element()?.text_content())
    }

    /// Polls until exactly one node matches and returns it. On expiry fails with `Timeout` if
    /// there were no matches, `Ambiguous` if there were several at the last poll. `timeout_ms`
    /// defaults to [`DEFAULT_WAIT_TIMEOUT_MS`]; the poll interval is [`POLL_INTERVAL_MS`].
    pub fn wait_for(&self, timeout_ms: Option<f64>) -> Result<TreeNode, AutomationError> {
        let timeout = timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
        let backend = &self.automation.backend;
        let start = backend.now_ms();
        loop {
            let mut nodes = self.all()?;
            if nodes.len() == 1 {
                return Ok(nodes.remove(0));
            }
            if backend.now_ms() - start >= timeout {
                return Err(if nodes.is_empty() {
                    AutomationError::new(
                        ErrorCode::Timeout,
                        format!(
                            "timed out after {timeout} ms waiting for {}",
                            self.describe()
                        ),
                    )
                } else {
                    AutomationError::with_data(
                        ErrorCode::Ambiguous,
                        format!(
                            "{} elements match {} after {timeout} ms",
                            nodes.len(),
                            self.describe()
                        ),
                        json!({ "count": nodes.len() }),
                    )
                });
            }
            backend.sleep(POLL_INTERVAL_MS);
        }
    }
}

impl Capability {
    /// Whether `info` lists this capability.
    pub fn is_offered(self, info: &InitInfo) -> bool {
        info.capabilities.contains(&self)
    }
}
