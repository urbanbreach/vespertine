//! System notifications over the freedesktop `org.freedesktop.Notifications`
//! D-Bus service (PLT-011).
//!
//! [`NotificationService`] holds the protocol logic (escaping, transport action
//! keys, bounded tracking of outstanding notifications, activation routing)
//! and talks to the bus through the [`NotificationBus`] trait, so it can be
//! driven by a scripted bus in tests. [`ZbusNotifications`] is the real bus:
//! one lazily created session-bus connection, shared by every notification,
//! with signal subscriptions instead of a thread per notification.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use parking_lot::Mutex;
use vespertine::{
    BackgroundExecutor, ForegroundExecutor, SharedString, SystemNotification,
    SystemNotificationDelivery, SystemNotificationResponse, Task,
};

/// The XDG action key invoked when the user activates the notification body
/// rather than a specific action button.
const DEFAULT_ACTION: &str = "default";

/// Number of outstanding notifications that are tracked; the oldest is
/// evicted when a new one would exceed it.
const MAX_TRACKED_NOTIFICATIONS: usize = 128;

/// Server capability that must be advertised for activation to be supported.
const ACTIONS_CAPABILITY: &str = "actions";

/// Transport key for the action button at `index`. Caller action ids are never
/// sent over the bus, so no caller id can collide with [`DEFAULT_ACTION`] or
/// another transport key.
fn transport_key(index: usize) -> String {
    format!("vespertine-action-{index}")
}

/// Escapes `&`, `<` and `>` so servers that interpret the body as markup
/// (`body-markup` capability) show the text verbatim and externally derived
/// text can never inject markup or links.
fn escape_markup(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BusError {
    /// No session bus, or nothing owns the notification service name.
    Unavailable(String),
    /// The service was reachable but the call failed.
    Failed(String),
}

impl From<BusError> for SystemNotificationDelivery {
    fn from(error: BusError) -> Self {
        match error {
            BusError::Unavailable(_) => SystemNotificationDelivery::Unavailable,
            BusError::Failed(reason) => SystemNotificationDelivery::Failed(reason.into()),
        }
    }
}

/// Arguments of the `Notify` method that the framework chooses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NotifyCall {
    pub(crate) app_name: String,
    pub(crate) replaces_id: u32,
    pub(crate) summary: String,
    pub(crate) body: String,
    /// Alternating key and label, as the `Notify` actions argument.
    pub(crate) actions: Vec<String>,
    /// Value of the `desktop-entry` hint.
    pub(crate) desktop_entry: Option<String>,
}

/// The notification signals the framework reacts to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BusSignal {
    ActionInvoked { id: u32, key: String },
    Closed { id: u32 },
    ActivationToken { id: u32, token: String },
}

/// The session-bus operations the notification logic needs.
pub(crate) trait NotificationBus: Send + Sync + 'static {
    fn capabilities(&self) -> BoxFuture<'static, Result<Vec<String>, BusError>>;
    fn notify(&self, call: NotifyCall) -> BoxFuture<'static, Result<u32, BusError>>;
    fn close(&self, id: u32) -> BoxFuture<'static, Result<(), BusError>>;
    /// Subscribes to the notification signals. The stream ends when the
    /// connection is lost.
    fn subscribe(&self) -> BoxFuture<'static, Result<BoxStream<'static, BusSignal>, BusError>>;
}

/// One outstanding notification.
struct Tracked {
    id: u32,
    tag: SharedString,
    destination: Option<SharedString>,
    /// Transport key to caller action id.
    actions: HashMap<String, SharedString>,
    /// Whether activation is reported for this notification, i.e. the server
    /// advertised `actions` when it was posted.
    activatable: bool,
    activation_token: Option<String>,
}

/// Bounded bookkeeping of outstanding notifications, keyed by the
/// server-assigned id and ordered oldest first.
#[derive(Default)]
struct Tracker {
    entries: HashMap<u32, Tracked>,
    order: VecDeque<u32>,
}

impl Tracker {
    fn insert(&mut self, tracked: Tracked) {
        // A replacement keeps the server id: it becomes the newest entry.
        self.remove(tracked.id);
        while self.entries.len() >= MAX_TRACKED_NOTIFICATIONS {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.order.push_back(tracked.id);
        self.entries.insert(tracked.id, tracked);
    }

    fn remove(&mut self, id: u32) -> Option<Tracked> {
        let removed = self.entries.remove(&id)?;
        self.order.retain(|&queued| queued != id);
        Some(removed)
    }

    /// The most recently posted notification with this tag.
    fn latest_id_for_tag(&self, tag: &str) -> Option<u32> {
        self.order
            .iter()
            .rev()
            .copied()
            .find(|id| self.entries.get(id).is_some_and(|entry| entry.tag == tag))
    }

    fn take_ids_for_tag(&mut self, tag: &str) -> Vec<u32> {
        let ids: Vec<u32> = self
            .order
            .iter()
            .copied()
            .filter(|id| self.entries.get(id).is_some_and(|entry| entry.tag == tag))
            .collect();
        for id in &ids {
            self.remove(*id);
        }
        ids
    }

    fn set_activation_token(&mut self, id: u32, token: String) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.activation_token = Some(token);
        }
    }

    /// The event for an `ActionInvoked` signal, if the notification is still
    /// tracked, activatable, and the key is one we registered.
    fn activation(&mut self, id: u32, key: &str) -> Option<SystemNotificationResponse> {
        let entry = self.entries.get_mut(&id)?;
        if !entry.activatable {
            return None;
        }
        let action_id = if key == DEFAULT_ACTION {
            None
        } else if let Some(action_id) = entry.actions.get(key) {
            Some(action_id.clone())
        } else {
            log::warn!("system notification returned unknown action {key:?}");
            return None;
        };
        Some(SystemNotificationResponse {
            tag: entry.tag.clone(),
            action_id,
            destination: entry.destination.clone(),
            activation_token: entry.activation_token.take().map(SharedString::from),
        })
    }
}

type Spawner = Arc<dyn Fn(BoxFuture<'static, ()>) + Send + Sync>;

/// Notification protocol logic over a [`NotificationBus`]. Cheap to clone;
/// clones share all state.
#[derive(Clone)]
pub(crate) struct NotificationService {
    bus: Arc<dyn NotificationBus>,
    tracker: Arc<Mutex<Tracker>>,
    events: mpsc::UnboundedSender<SystemNotificationResponse>,
    spawner: Spawner,
    subscription: Arc<futures::lock::Mutex<()>>,
    subscribed: Arc<AtomicBool>,
}

impl NotificationService {
    /// `spawner` runs the signal pump. Activation events are sent to `events`.
    pub(crate) fn new(
        bus: Arc<dyn NotificationBus>,
        spawner: Spawner,
        events: mpsc::UnboundedSender<SystemNotificationResponse>,
    ) -> Self {
        Self {
            bus,
            tracker: Arc::default(),
            events,
            spawner,
            subscription: Arc::default(),
            subscribed: Arc::default(),
        }
    }

    /// Makes sure exactly one signal pump is running for the connection.
    async fn ensure_subscribed(&self) -> Result<(), BusError> {
        let _guard = self.subscription.lock().await;
        if self.subscribed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut signals = self.bus.subscribe().await?;
        self.subscribed.store(true, Ordering::Release);
        let service = self.clone();
        (self.spawner)(
            async move {
                while let Some(signal) = signals.next().await {
                    service.handle_signal(signal);
                }
                // The connection went away; the next post subscribes again.
                service.subscribed.store(false, Ordering::Release);
            }
            .boxed(),
        );
        Ok(())
    }

    fn handle_signal(&self, signal: BusSignal) {
        match signal {
            BusSignal::ActivationToken { id, token } => {
                self.tracker.lock().set_activation_token(id, token);
            }
            BusSignal::ActionInvoked { id, key } => {
                let response = self.tracker.lock().activation(id, &key);
                if let Some(response) = response {
                    self.events.unbounded_send(response).ok();
                }
            }
            BusSignal::Closed { id } => {
                self.tracker.lock().remove(id);
            }
        }
    }

    async fn advertises_actions(&self) -> Result<bool, BusError> {
        let capabilities = self.bus.capabilities().await?;
        Ok(capabilities
            .iter()
            .any(|capability| capability == ACTIONS_CAPABILITY))
    }

    /// Whether the service advertises the `actions` capability. `false` when
    /// the service cannot be reached.
    pub(crate) async fn activation_supported(self) -> bool {
        self.advertises_actions().await.unwrap_or(false)
    }

    pub(crate) async fn post(
        self,
        app_name: Option<String>,
        app_id: Option<String>,
        notification: SystemNotification,
    ) -> SystemNotificationDelivery {
        match self.try_post(app_name, app_id, notification).await {
            Ok(()) => SystemNotificationDelivery::Delivered,
            Err(error) => {
                match &error {
                    BusError::Unavailable(reason) => {
                        log::debug!("system notification service unavailable: {reason}")
                    }
                    BusError::Failed(reason) => {
                        log::warn!("failed to post system notification: {reason}")
                    }
                }
                error.into()
            }
        }
    }

    async fn try_post(
        &self,
        app_name: Option<String>,
        app_id: Option<String>,
        notification: SystemNotification,
    ) -> Result<(), BusError> {
        self.ensure_subscribed().await?;
        let activatable = self.advertises_actions().await?;

        let mut actions = Vec::new();
        let mut action_ids = HashMap::new();
        if activatable {
            actions.push(DEFAULT_ACTION.to_string());
            actions.push(DEFAULT_ACTION.to_string());
            for (index, action) in notification.actions.iter().enumerate() {
                let key = transport_key(index);
                actions.push(key.clone());
                actions.push(action.label.to_string());
                action_ids.insert(key, action.id.clone());
            }
        }

        let replaces_id = self
            .tracker
            .lock()
            .latest_id_for_tag(&notification.tag)
            .unwrap_or(0);
        let id = self
            .bus
            .notify(NotifyCall {
                app_name: app_name.unwrap_or_default(),
                replaces_id,
                summary: escape_markup(&notification.title),
                body: escape_markup(&notification.body),
                actions,
                desktop_entry: app_id,
            })
            .await?;

        self.tracker.lock().insert(Tracked {
            id,
            tag: notification.tag,
            destination: notification.destination,
            actions: action_ids,
            activatable,
            activation_token: None,
        });
        Ok(())
    }

    /// Closes the notifications with this tag. Best effort.
    pub(crate) async fn dismiss(self, tag: SharedString) {
        let ids = self.tracker.lock().take_ids_for_tag(&tag);
        for id in ids {
            if let Err(error) = self.bus.close(id).await {
                log::debug!("failed to close system notification {id}: {error:?}");
            }
        }
    }
}

/// The real session-bus implementation.
pub(crate) struct ZbusNotifications {
    connection: Arc<futures::lock::Mutex<Option<zbus::Connection>>>,
}

const SERVICE_NAME: &str = "org.freedesktop.Notifications";
const SERVICE_PATH: &str = "/org/freedesktop/Notifications";

impl ZbusNotifications {
    pub(crate) fn new() -> Self {
        Self {
            connection: Arc::default(),
        }
    }
}

/// Reply errors that mean there is no notification service, as opposed to a
/// service that refused the call.
fn is_service_missing_error(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.NoServer"
    ) || name.starts_with("org.freedesktop.DBus.Error.Spawn.")
}

fn classify_zbus_error(error: &zbus::Error) -> BusError {
    match error {
        zbus::Error::MethodError(name, description, _)
            if is_service_missing_error(name.as_str()) =>
        {
            BusError::Unavailable(format!(
                "{name}: {}",
                description.as_deref().unwrap_or("no description")
            ))
        }
        other => BusError::Failed(other.to_string()),
    }
}

/// A proxy for the notification service on a connection created on first
/// use. A failed connection attempt is not cached, so a session bus that
/// appears later is picked up by the next call.
async fn service_proxy(
    connection: &futures::lock::Mutex<Option<zbus::Connection>>,
) -> Result<zbus::Proxy<'static>, BusError> {
    let connection = {
        let mut guard = connection.lock().await;
        match guard.as_ref() {
            Some(connection) => connection.clone(),
            None => {
                let connected = zbus::Connection::session()
                    .await
                    .map_err(|error| BusError::Unavailable(format!("no session bus: {error}")))?;
                *guard = Some(connected.clone());
                connected
            }
        }
    };
    zbus::Proxy::new(&connection, SERVICE_NAME, SERVICE_PATH, SERVICE_NAME)
        .await
        .map_err(|error| classify_zbus_error(&error))
}

/// Forgets the cached connection after an error that says it is broken.
async fn reset_if_disconnected(
    connection: &futures::lock::Mutex<Option<zbus::Connection>>,
    error: &zbus::Error,
) {
    if matches!(error, zbus::Error::InputOutput(_)) {
        *connection.lock().await = None;
    }
}

fn parse_signal(name: &'static str, message: &zbus::Message) -> Option<BusSignal> {
    let body = message.body();
    match name {
        "ActionInvoked" => {
            let (id, key) = body.deserialize::<(u32, String)>().ok()?;
            Some(BusSignal::ActionInvoked { id, key })
        }
        "NotificationClosed" => {
            let (id, _reason) = body.deserialize::<(u32, u32)>().ok()?;
            Some(BusSignal::Closed { id })
        }
        "ActivationToken" => {
            let (id, token) = body.deserialize::<(u32, String)>().ok()?;
            Some(BusSignal::ActivationToken { id, token })
        }
        _ => None,
    }
}

impl NotificationBus for ZbusNotifications {
    fn capabilities(&self) -> BoxFuture<'static, Result<Vec<String>, BusError>> {
        let connection = self.connection.clone();
        async move {
            let proxy = service_proxy(&connection).await?;
            match proxy
                .call::<_, _, Vec<String>>("GetCapabilities", &())
                .await
            {
                Ok(capabilities) => Ok(capabilities),
                Err(error) => {
                    reset_if_disconnected(&connection, &error).await;
                    Err(classify_zbus_error(&error))
                }
            }
        }
        .boxed()
    }

    fn notify(&self, call: NotifyCall) -> BoxFuture<'static, Result<u32, BusError>> {
        let connection = self.connection.clone();
        async move {
            let proxy = service_proxy(&connection).await?;
            let mut hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
            if let Some(desktop_entry) = &call.desktop_entry {
                hints.insert("desktop-entry", desktop_entry.as_str().into());
            }
            let result = proxy
                .call::<_, _, u32>(
                    "Notify",
                    &(
                        call.app_name.as_str(),
                        call.replaces_id,
                        "",
                        call.summary.as_str(),
                        call.body.as_str(),
                        call.actions.as_slice(),
                        hints,
                        -1i32,
                    ),
                )
                .await;
            match result {
                Ok(id) => Ok(id),
                Err(error) => {
                    reset_if_disconnected(&connection, &error).await;
                    Err(classify_zbus_error(&error))
                }
            }
        }
        .boxed()
    }

    fn close(&self, id: u32) -> BoxFuture<'static, Result<(), BusError>> {
        let connection = self.connection.clone();
        async move {
            let proxy = service_proxy(&connection).await?;
            match proxy.call::<_, _, ()>("CloseNotification", &(id,)).await {
                Ok(()) => Ok(()),
                Err(error) => {
                    reset_if_disconnected(&connection, &error).await;
                    Err(classify_zbus_error(&error))
                }
            }
        }
        .boxed()
    }

    fn subscribe(&self) -> BoxFuture<'static, Result<BoxStream<'static, BusSignal>, BusError>> {
        let connection = self.connection.clone();
        async move {
            let proxy = service_proxy(&connection).await?;
            let mut streams = Vec::new();
            for name in ["ActionInvoked", "NotificationClosed", "ActivationToken"] {
                let stream = proxy
                    .receive_signal(name)
                    .await
                    .map_err(|error| classify_zbus_error(&error))?;
                streams.push(
                    stream
                        .filter_map(move |message| {
                            futures::future::ready(parse_signal(name, &message))
                        })
                        .boxed(),
                );
            }
            Ok(futures::stream::select_all(streams).boxed())
        }
        .boxed()
    }
}

type ResponseCallback = Rc<RefCell<Option<Box<dyn FnMut(SystemNotificationResponse)>>>>;

/// Per-platform notification state: the service plus the main-thread delivery
/// of activation events.
pub(crate) struct SystemNotificationState {
    service: NotificationService,
    app_id: Option<String>,
    response_receiver: Option<mpsc::UnboundedReceiver<SystemNotificationResponse>>,
    callback: ResponseCallback,
    _response_task: Option<Task<()>>,
}

impl SystemNotificationState {
    pub(crate) fn new() -> Self {
        let (events, response_receiver) = mpsc::unbounded();
        let spawner: Spawner = Arc::new(|future| smol::spawn(future).detach());
        Self {
            service: NotificationService::new(Arc::new(ZbusNotifications::new()), spawner, events),
            app_id: None,
            response_receiver: Some(response_receiver),
            callback: Rc::new(RefCell::new(None)),
            _response_task: None,
        }
    }

    /// The application id carried in the `desktop-entry` hint.
    pub(crate) fn set_app_id(&mut self, app_id: &str) {
        self.app_id = Some(app_id.to_string());
    }

    /// Posts off the calling thread; the bus connection and the call happen on
    /// the background executor.
    pub(crate) fn post(
        &self,
        executor: &BackgroundExecutor,
        app_name: Option<&str>,
        notification: SystemNotification,
    ) -> Task<SystemNotificationDelivery> {
        executor.spawn(self.service.clone().post(
            app_name.map(str::to_string),
            self.app_id.clone(),
            notification,
        ))
    }

    pub(crate) fn activation_supported(&self, executor: &BackgroundExecutor) -> Task<bool> {
        executor.spawn(self.service.clone().activation_supported())
    }

    pub(crate) fn dismiss(&self, executor: &BackgroundExecutor, tag: &str) {
        executor
            .spawn(
                self.service
                    .clone()
                    .dismiss(SharedString::from(tag.to_string())),
            )
            .detach();
    }

    pub(crate) fn on_response(
        &mut self,
        executor: &ForegroundExecutor,
        callback: Box<dyn FnMut(SystemNotificationResponse)>,
    ) {
        *self.callback.borrow_mut() = Some(callback);

        // Activation events are produced on the signal pump; this task hands
        // them to the registered callback on the main thread.
        if let Some(mut receiver) = self.response_receiver.take() {
            let callback = self.callback.clone();
            self._response_task = Some(executor.spawn(async move {
                while let Some(response) = receiver.next().await {
                    // Take the callback out for the call: it may re-enter the
                    // platform or replace itself.
                    let taken = callback.borrow_mut().take();
                    if let Some(mut taken) = taken {
                        taken(response);
                        callback.borrow_mut().get_or_insert(taken);
                    }
                }
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use vespertine::SystemNotificationAction;

    use super::*;

    /// A bus whose replies are scripted and which records every call.
    struct ScriptedBus {
        capabilities: Mutex<Result<Vec<String>, BusError>>,
        notify_results: Mutex<VecDeque<Result<u32, BusError>>>,
        next_id: AtomicUsize,
        notified: Mutex<Vec<NotifyCall>>,
        closed: Mutex<Vec<u32>>,
        subscriptions: AtomicUsize,
        signals: Mutex<Option<mpsc::UnboundedReceiver<BusSignal>>>,
    }

    impl ScriptedBus {
        fn new(capabilities: &[&str]) -> (Arc<Self>, mpsc::UnboundedSender<BusSignal>) {
            let (sender, receiver) = mpsc::unbounded();
            let bus = Arc::new(Self {
                capabilities: Mutex::new(Ok(capabilities
                    .iter()
                    .map(|capability| capability.to_string())
                    .collect())),
                notify_results: Mutex::default(),
                next_id: AtomicUsize::new(1),
                notified: Mutex::default(),
                closed: Mutex::default(),
                subscriptions: AtomicUsize::new(0),
                signals: Mutex::new(Some(receiver)),
            });
            (bus, sender)
        }
    }

    impl NotificationBus for ScriptedBus {
        fn capabilities(&self) -> BoxFuture<'static, Result<Vec<String>, BusError>> {
            futures::future::ready(self.capabilities.lock().clone()).boxed()
        }

        fn notify(&self, call: NotifyCall) -> BoxFuture<'static, Result<u32, BusError>> {
            self.notified.lock().push(call);
            let scripted = self.notify_results.lock().pop_front();
            let result =
                scripted.unwrap_or_else(|| Ok(self.next_id.fetch_add(1, Ordering::SeqCst) as u32));
            futures::future::ready(result).boxed()
        }

        fn close(&self, id: u32) -> BoxFuture<'static, Result<(), BusError>> {
            self.closed.lock().push(id);
            futures::future::ready(Ok(())).boxed()
        }

        fn subscribe(&self) -> BoxFuture<'static, Result<BoxStream<'static, BusSignal>, BusError>> {
            self.subscriptions.fetch_add(1, Ordering::SeqCst);
            let receiver = self.signals.lock().take();
            futures::future::ready(match receiver {
                Some(receiver) => Ok(receiver.boxed()),
                None => Ok(futures::stream::pending().boxed()),
            })
            .boxed()
        }
    }

    struct Harness {
        bus: Arc<ScriptedBus>,
        signals: mpsc::UnboundedSender<BusSignal>,
        service: NotificationService,
        events: mpsc::UnboundedReceiver<SystemNotificationResponse>,
    }

    fn harness(capabilities: &[&str]) -> Harness {
        let (bus, signals) = ScriptedBus::new(capabilities);
        let (event_sender, events) = mpsc::unbounded();
        let spawner: Spawner = Arc::new(|future| smol::spawn(future).detach());
        let service = NotificationService::new(bus.clone(), spawner, event_sender);
        Harness {
            bus,
            signals,
            service,
            events,
        }
    }

    impl Harness {
        fn post(&self, notification: SystemNotification) -> SystemNotificationDelivery {
            smol::block_on(self.service.clone().post(
                Some("Test App".to_string()),
                Some("com.example.test".to_string()),
                notification,
            ))
        }

        fn next_event(&mut self) -> Option<SystemNotificationResponse> {
            self.events.try_recv().ok()
        }
    }

    fn notification(tag: &str, actions: &[(&str, &str)]) -> SystemNotification {
        SystemNotification {
            tag: tag.to_string().into(),
            title: format!("title {tag}").into(),
            body: format!("body {tag}").into(),
            actions: actions
                .iter()
                .map(|(id, label)| SystemNotificationAction {
                    id: id.to_string().into(),
                    label: label.to_string().into(),
                })
                .collect(),
            destination: Some(format!("route/{tag}").into()),
        }
    }

    const ACTIONS_AND_BODY: &[&str] = &["actions", "body", "body-markup"];

    #[test]
    fn posting_reports_delivered_and_describes_the_notification_to_the_service() {
        let harness = harness(ACTIONS_AND_BODY);
        // The service never learns whether the window is focused: suppression
        // is the caller's policy, so a post always reaches the bus.
        assert_eq!(
            harness.post(notification("a", &[("open", "Open")])),
            SystemNotificationDelivery::Delivered
        );

        let notified = harness.bus.notified.lock().clone();
        assert_eq!(notified.len(), 1);
        let call = &notified[0];
        assert_eq!(call.app_name, "Test App");
        assert_eq!(call.summary, "title a");
        assert_eq!(call.body, "body a");
        assert_eq!(call.replaces_id, 0);
        assert_eq!(call.desktop_entry.as_deref(), Some("com.example.test"));
        // Body on the default action, the button on a transport key.
        assert_eq!(
            call.actions,
            ["default", "default", "vespertine-action-0", "Open"]
        );
    }

    #[test]
    fn missing_service_is_unavailable_and_service_errors_are_failures() {
        let harness = harness(ACTIONS_AND_BODY);
        *harness.bus.capabilities.lock() = Err(BusError::Unavailable("no bus".into()));
        assert_eq!(
            harness.post(notification("a", &[])),
            SystemNotificationDelivery::Unavailable
        );
        assert!(harness.bus.notified.lock().is_empty());
        assert!(!smol::block_on(
            harness.service.clone().activation_supported()
        ));

        *harness.bus.capabilities.lock() = Ok(vec!["actions".to_string()]);
        harness
            .bus
            .notify_results
            .lock()
            .push_back(Err(BusError::Failed("server exploded".into())));
        assert_eq!(
            harness.post(notification("a", &[])),
            SystemNotificationDelivery::Failed("server exploded".into())
        );

        harness
            .bus
            .notify_results
            .lock()
            .push_back(Err(BusError::Unavailable("gone".into())));
        assert_eq!(
            harness.post(notification("a", &[])),
            SystemNotificationDelivery::Unavailable
        );
    }

    #[test]
    fn same_action_id_on_two_outstanding_notifications_routes_to_each_own_notification() {
        let mut harness = harness(ACTIONS_AND_BODY);
        harness.post(notification("one", &[("open", "Open")]));
        harness.post(notification("two", &[("open", "Open")]));

        // Ids 1 and 2 were assigned in order; activate the second first.
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 2,
            key: "vespertine-action-0".into(),
        });
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "vespertine-action-0".into(),
        });

        let second = harness.next_event().unwrap();
        let first = harness.next_event().unwrap();
        assert_eq!(
            (second.tag.as_ref(), second.action_id.as_deref()),
            ("two", Some("open"))
        );
        assert_eq!(second.destination.as_deref(), Some("route/two"));
        assert_eq!(
            (first.tag.as_ref(), first.action_id.as_deref()),
            ("one", Some("open"))
        );
        assert_eq!(first.destination.as_deref(), Some("route/one"));
        assert!(harness.next_event().is_none());
    }

    #[test]
    fn caller_action_id_default_is_not_confused_with_the_body() {
        let mut harness = harness(ACTIONS_AND_BODY);
        harness.post(notification(
            "a",
            &[("default", "Default?"), ("vespertine-action-0", "Marker")],
        ));

        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "vespertine-action-0".into(),
        });
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "vespertine-action-1".into(),
        });
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "default".into(),
        });
        // A key that was never registered is ignored.
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "open".into(),
        });

        let ids: Vec<Option<String>> = std::iter::from_fn(|| harness.next_event())
            .map(|event| event.action_id.map(|id| id.to_string()))
            .collect();
        assert_eq!(
            ids,
            [
                Some("default".to_string()),
                Some("vespertine-action-0".to_string()),
                None,
            ]
        );
    }

    #[test]
    fn closed_without_activation_emits_nothing_and_releases_the_entry() {
        let mut harness = harness(ACTIONS_AND_BODY);
        harness.post(notification("a", &[("open", "Open")]));
        assert_eq!(harness.service.tracker.lock().entries.len(), 1);

        harness.service.handle_signal(BusSignal::Closed { id: 1 });
        assert!(harness.next_event().is_none());
        assert!(harness.service.tracker.lock().entries.is_empty());
        assert!(harness.service.tracker.lock().order.is_empty());

        // A click that arrives after the close is ignored.
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "default".into(),
        });
        assert!(harness.next_event().is_none());
    }

    #[test]
    fn service_without_actions_reports_false_and_never_invokes_the_handler() {
        let mut harness = harness(&["body", "body-markup"]);
        assert!(!smol::block_on(
            harness.service.clone().activation_supported()
        ));
        assert_eq!(
            harness.post(notification("a", &[("open", "Open")])),
            SystemNotificationDelivery::Delivered
        );

        // No action buttons are registered with a server that cannot show them.
        assert!(harness.bus.notified.lock()[0].actions.is_empty());

        harness.service.handle_signal(BusSignal::ActivationToken {
            id: 1,
            token: "token".into(),
        });
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "default".into(),
        });
        assert!(harness.next_event().is_none());

        *harness.bus.capabilities.lock() = Ok(vec!["actions".to_string()]);
        assert!(smol::block_on(
            harness.service.clone().activation_supported()
        ));
    }

    #[test]
    fn the_129th_outstanding_notification_evicts_the_first() {
        let mut harness = harness(ACTIONS_AND_BODY);
        for index in 0..=MAX_TRACKED_NOTIFICATIONS {
            harness.post(notification(&format!("n{index}"), &[]));
        }
        assert_eq!(
            harness.service.tracker.lock().entries.len(),
            MAX_TRACKED_NOTIFICATIONS
        );

        // Server ids are 1..=129; id 1 was the first and is evicted.
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "default".into(),
        });
        assert!(harness.next_event().is_none());

        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 2,
            key: "default".into(),
        });
        assert_eq!(harness.next_event().unwrap().tag, "n1");
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 129,
            key: "default".into(),
        });
        assert_eq!(harness.next_event().unwrap().tag, "n128");
    }

    #[test]
    fn activation_token_accompanies_only_its_own_event() {
        let mut harness = harness(ACTIONS_AND_BODY);
        harness.post(notification("a", &[]));
        harness.post(notification("b", &[]));

        harness.service.handle_signal(BusSignal::ActivationToken {
            id: 1,
            token: "xdg-token-a".into(),
        });
        // A token for a notification we do not track is dropped.
        harness.service.handle_signal(BusSignal::ActivationToken {
            id: 99,
            token: "stray".into(),
        });
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 2,
            key: "default".into(),
        });
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "default".into(),
        });

        let b = harness.next_event().unwrap();
        let a = harness.next_event().unwrap();
        assert_eq!((b.tag.as_ref(), b.activation_token), ("b", None));
        assert_eq!(a.tag, "a");
        assert_eq!(a.activation_token.as_deref(), Some("xdg-token-a"));
    }

    #[test]
    fn markup_characters_reach_the_service_escaped_and_option_like_titles_stay_text() {
        let harness = harness(ACTIONS_AND_BODY);
        harness.post(SystemNotification {
            title: "Tom & <Jerry>".into(),
            body: "<a href=\"x\">&amp; & ></a>".into(),
            ..notification("a", &[])
        });
        harness.post(SystemNotification {
            title: "--urgency=critical -h".into(),
            body: "-b".into(),
            ..notification("b", &[])
        });

        let notified = harness.bus.notified.lock().clone();
        assert_eq!(notified[0].summary, "Tom &amp; &lt;Jerry&gt;");
        assert_eq!(
            notified[0].body,
            "&lt;a href=\"x\"&gt;&amp;amp; &amp; &gt;&lt;/a&gt;"
        );
        assert_eq!(notified[1].summary, "--urgency=critical -h");
        assert_eq!(notified[1].body, "-b");
    }

    #[test]
    fn escaping_never_double_escapes_across_replacement_and_is_unconditional() {
        // Even when the service does not advertise body-markup.
        let harness = harness(&["actions"]);
        harness.post(SystemNotification {
            title: "a<b".into(),
            body: "c&d".into(),
            ..notification("a", &[])
        });
        let call = harness.bus.notified.lock()[0].clone();
        assert_eq!(
            (call.summary.as_str(), call.body.as_str()),
            ("a&lt;b", "c&amp;d")
        );
    }

    #[test]
    fn reposting_a_tag_replaces_the_earlier_notification() {
        let mut harness = harness(ACTIONS_AND_BODY);
        harness.post(notification("same", &[("open", "Open")]));
        // The server replaces and hands back the same id.
        harness.bus.notify_results.lock().push_back(Ok(1));
        harness.post(SystemNotification {
            destination: Some("route/new".into()),
            ..notification("same", &[("open", "Open")])
        });

        let notified = harness.bus.notified.lock().clone();
        assert_eq!((notified[0].replaces_id, notified[1].replaces_id), (0, 1));
        assert_eq!(harness.service.tracker.lock().entries.len(), 1);

        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "vespertine-action-0".into(),
        });
        let event = harness.next_event().unwrap();
        assert_eq!(event.destination.as_deref(), Some("route/new"));
    }

    #[test]
    fn dismiss_closes_every_notification_with_the_tag() {
        let mut harness = harness(ACTIONS_AND_BODY);
        harness.post(notification("a", &[]));
        harness.post(notification("b", &[]));
        // The service does not replace: a second notification for tag a.
        harness.post(notification("a", &[]));

        smol::block_on(harness.service.clone().dismiss("a".into()));
        assert_eq!(*harness.bus.closed.lock(), [1, 3]);

        // Dismissed notifications no longer produce events; others still do.
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 1,
            key: "default".into(),
        });
        assert!(harness.next_event().is_none());
        harness.service.handle_signal(BusSignal::ActionInvoked {
            id: 2,
            key: "default".into(),
        });
        assert_eq!(harness.next_event().unwrap().tag, "b");

        // Dismissing an unknown tag makes no bus call.
        smol::block_on(harness.service.clone().dismiss("unknown".into()));
        assert_eq!(harness.bus.closed.lock().len(), 2);
    }

    #[test]
    fn one_subscription_serves_all_notifications_and_signals_flow_through_it() {
        let mut harness = harness(ACTIONS_AND_BODY);
        harness.post(notification("a", &[("open", "Open")]));
        harness.post(notification("b", &[("open", "Open")]));
        harness.post(notification("c", &[("open", "Open")]));
        assert_eq!(harness.bus.subscriptions.load(Ordering::SeqCst), 1);

        harness
            .signals
            .unbounded_send(BusSignal::ActivationToken {
                id: 2,
                token: "tok".into(),
            })
            .unwrap();
        harness
            .signals
            .unbounded_send(BusSignal::ActionInvoked {
                id: 2,
                key: "vespertine-action-0".into(),
            })
            .unwrap();
        let event = smol::block_on(harness.events.next()).unwrap();
        assert_eq!(event.tag, "b");
        assert_eq!(event.action_id.as_deref(), Some("open"));
        assert_eq!(event.activation_token.as_deref(), Some("tok"));
    }

    #[test]
    fn a_lost_connection_resubscribes_on_the_next_post() {
        let harness = harness(ACTIONS_AND_BODY);
        harness.post(notification("a", &[]));
        assert_eq!(harness.bus.subscriptions.load(Ordering::SeqCst), 1);

        // Ending the signal stream stands for losing the connection.
        harness.signals.close_channel();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.service.subscribed.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline, "pump did not stop");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        harness.post(notification("b", &[]));
        assert_eq!(harness.bus.subscriptions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn service_missing_errors_are_unavailable_and_others_failed() {
        for name in [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.freedesktop.DBus.Error.Spawn.ServiceNotFound",
        ] {
            assert!(is_service_missing_error(name), "{name}");
        }
        for name in [
            "org.freedesktop.DBus.Error.NoReply",
            "org.freedesktop.DBus.Error.InvalidArgs",
            "org.freedesktop.DBus.Error.AccessDenied",
        ] {
            assert!(!is_service_missing_error(name), "{name}");
        }
    }

    #[test]
    fn escape_markup_escapes_ampersand_first() {
        assert_eq!(escape_markup("&lt;"), "&amp;lt;");
        assert_eq!(escape_markup("plain -text"), "plain -text");
    }
}
