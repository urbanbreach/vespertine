//! The Linux side of the retained application lifecycle (PLT-008 R7).
//!
//! The application owns the well-known session-bus name equal to its application id and
//! implements the freedesktop `org.freedesktop.Application` interface. A desktop entry that
//! declares `DBusActivatable=true` makes launchers call it instead of starting a second
//! process:
//!
//! * `Activate(platform_data)` becomes a reopen notification.
//! * `Open(uris, platform_data)` becomes an incoming-URL notification carrying the URI
//!   strings exactly as given.
//! * `ActivateAction(name, parameter, platform_data)` becomes an activated-action
//!   notification with the action name.
//!
//! Each call is followed by an activation request. Its token is the `activation-token` entry
//! of the platform data, or `desktop-startup-id` when that is absent; windows are raised
//! with it through xdg-activation (Wayland) or `_NET_STARTUP_ID` (X11), and without one the
//! window only requests attention.
//!
//! Where calls come from is behind [`ApplicationTransport`], so tests drive the service
//! without a bus. Session termination (`SIGTERM`, `SIGHUP`) arrives as the same kind of
//! event.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, atomic::AtomicI32},
};

use anyhow::{Context as _, Result, bail};
use vespertine::ActivationRequest;
use zbus::zvariant::OwnedValue;

use crate::linux::{LinuxCommon, PlatformHandlers};

/// The `platform_data` key of an xdg-activation token.
pub(crate) const ACTIVATION_TOKEN_KEY: &str = "activation-token";
/// The `platform_data` key of a startup-notification id, used when there is no token.
pub(crate) const STARTUP_ID_KEY: &str = "desktop-startup-id";

pub(crate) type PlatformData = HashMap<String, OwnedValue>;

/// Something the platform's lifecycle mechanisms asked the application to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LifecycleEvent {
    Activate {
        token: Option<String>,
    },
    Open {
        uris: Vec<String>,
        token: Option<String>,
    },
    ActivateAction {
        action: String,
        token: Option<String>,
    },
    /// The session asked the process to terminate.
    QuitRequested,
}

/// Receives events from a transport, on whatever thread the transport runs on.
pub(crate) type EventSink = Arc<dyn Fn(LifecycleEvent) + Send + Sync>;

/// Where `org.freedesktop.Application` calls come from. Production serves them on the
/// session bus; tests provide a fake that emits calls directly.
pub(crate) trait ApplicationTransport: Send + 'static {
    /// Starts serving calls for `app_id` and returns once serving was requested; the
    /// transport sends every call it receives to `events`.
    fn serve(self: Box<Self>, app_id: &str, events: EventSink) -> Result<()>;
}

/// What the lifecycle events turn into inside the platform.
pub(crate) trait LifecycleHandlers {
    fn reopen(&mut self);
    fn open_urls(&mut self, urls: Vec<String>);
    fn activate_action(&mut self, action: String);
    fn activation_request(&mut self, request: ActivationRequest);
    fn quit_requested(&mut self);
}

/// Delivers an event: the notification it stands for, then the activation request.
pub(crate) fn dispatch(event: LifecycleEvent, handlers: &mut dyn LifecycleHandlers) {
    match event {
        LifecycleEvent::Activate { token } => {
            handlers.reopen();
            handlers.activation_request(ActivationRequest { token });
        }
        LifecycleEvent::Open { uris, token } => {
            handlers.open_urls(uris);
            handlers.activation_request(ActivationRequest { token });
        }
        LifecycleEvent::ActivateAction { action, token } => {
            handlers.activate_action(action);
            handlers.activation_request(ActivationRequest { token });
        }
        LifecycleEvent::QuitRequested => handlers.quit_requested(),
    }
}

/// The activation token in a call's platform data: `activation-token`, else
/// `desktop-startup-id`. Empty values count as absent.
pub(crate) fn activation_token(platform_data: &PlatformData) -> Option<String> {
    [ACTIVATION_TOKEN_KEY, STARTUP_ID_KEY]
        .into_iter()
        .find_map(|key| {
            let value = platform_data.get(key)?;
            let token: &str = value.downcast_ref().ok()?;
            (!token.is_empty()).then(|| token.to_owned())
        })
}

/// The object path the Application interface is served at: the application id with `.`
/// as `/` and `-` as `_`, with a leading `/`.
pub(crate) fn object_path(app_id: &str) -> String {
    let path: String = app_id
        .chars()
        .map(|c| match c {
            '.' => '/',
            '-' => '_',
            c => c,
        })
        .collect();
    format!("/{path}")
}

/// Starts serving `org.freedesktop.Application` for `app_id` through `transport`.
///
/// Fails when `app_id` is not a valid well-known bus name (at least two dot-separated
/// elements of ASCII letters, digits, `_` and `-`, none starting with a digit).
pub(crate) fn start(
    transport: Box<dyn ApplicationTransport>,
    app_id: &str,
    events: EventSink,
) -> Result<()> {
    zbus::names::WellKnownName::try_from(app_id)
        .with_context(|| format!("{app_id:?} is not a valid D-Bus application id"))?;
    transport.serve(app_id, events)
}

/// The `org.freedesktop.Application` interface object.
pub(crate) struct ApplicationInterface {
    events: EventSink,
}

impl ApplicationInterface {
    pub(crate) fn new(events: EventSink) -> Self {
        Self { events }
    }
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl ApplicationInterface {
    async fn activate(&self, platform_data: PlatformData) {
        (self.events)(LifecycleEvent::Activate {
            token: activation_token(&platform_data),
        });
    }

    async fn open(&self, uris: Vec<String>, platform_data: PlatformData) {
        (self.events)(LifecycleEvent::Open {
            uris,
            token: activation_token(&platform_data),
        });
    }

    async fn activate_action(
        &self,
        action_name: String,
        parameter: Vec<OwnedValue>,
        platform_data: PlatformData,
    ) {
        if !parameter.is_empty() {
            log::warn!("parameters of activated action {action_name:?} are not forwarded");
        }
        (self.events)(LifecycleEvent::ActivateAction {
            action: action_name,
            token: activation_token(&platform_data),
        });
    }
}

/// Serves the interface on the session bus under the well-known name `app_id`.
pub(crate) struct SessionBus;

impl ApplicationTransport for SessionBus {
    fn serve(self: Box<Self>, app_id: &str, events: EventSink) -> Result<()> {
        let name = app_id.to_owned();
        let path = object_path(app_id);
        smol::spawn(async move {
            let connection = zbus::connection::Builder::session()?
                .name(name.as_str())?
                .serve_at(path.as_str(), ApplicationInterface::new(events))?
                .build()
                .await;
            match connection {
                // Keeps the name and the interface for the life of the process.
                Ok(_connection) => std::future::pending::<()>().await,
                Err(error) => log::warn!(
                    "could not own the session bus name {name:?} (another instance may own it): \
                     {error}"
                ),
            }
            anyhow::Ok(())
        })
        .detach();
        Ok(())
    }
}

/// Write end of the pipe the termination signal handler uses; `-1` until installed.
static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn termination_signal_handler(_signal: libc::c_int) {
    use std::sync::atomic::Ordering;
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = 1u8;
        // SAFETY: `write` is async-signal-safe and `byte` outlives the call.
        unsafe { libc::write(fd, (&raw const byte).cast(), 1) };
    }
}

/// Reports `SIGTERM` and `SIGHUP` to `events` as [`LifecycleEvent::QuitRequested`] instead of
/// letting them end the process. Installed once per process; later calls do nothing.
pub(crate) fn install_termination_handlers(events: EventSink) -> Result<()> {
    use std::sync::atomic::Ordering;

    if SIGNAL_PIPE.load(Ordering::Relaxed) >= 0 {
        return Ok(());
    }
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` has room for the two descriptors `pipe2` writes.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        bail!(
            "could not create the signal pipe: {}",
            std::io::Error::last_os_error()
        );
    }
    let [read_fd, write_fd] = fds;
    SIGNAL_PIPE.store(write_fd, Ordering::Relaxed);

    for signal in [libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: the handler only calls the async-signal-safe `write`; the zeroed
        // `sigaction` is a valid starting value that is fully initialized below.
        let installed = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = termination_signal_handler as extern "C" fn(libc::c_int) as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut())
        };
        if installed != 0 {
            bail!(
                "could not install the handler for signal {signal}: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    std::thread::Builder::new()
        .name("vespertine-termination-signals".into())
        .spawn(move || {
            let mut byte = 0u8;
            loop {
                // SAFETY: `byte` is valid for one byte of writes.
                let read = unsafe { libc::read(read_fd, (&raw mut byte).cast(), 1) };
                if read == 1 {
                    events(LifecycleEvent::QuitRequested);
                } else if read < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                } else {
                    break;
                }
            }
        })
        .context("could not start the signal listener thread")?;
    Ok(())
}

/// Registers the source on the event loop that delivers lifecycle events to the platform's
/// callbacks, and gives `common` the sender transports use.
pub(crate) fn install(
    common: &Rc<RefCell<LinuxCommon>>,
    loop_handle: &calloop::LoopHandle<'static, ()>,
) -> Result<()> {
    let (sender, receiver) = calloop::channel::channel();
    loop_handle
        .insert_source(receiver, {
            let common = common.clone();
            move |event, _, _| {
                if let calloop::channel::Event::Msg(event) = event {
                    dispatch(event, &mut CommonHandlers(&common));
                }
            }
        })
        .map_err(|error| anyhow::anyhow!("failed to register the lifecycle source: {error}"))?;
    common.borrow_mut().lifecycle_events = Some(sender);
    Ok(())
}

impl LinuxCommon {
    fn lifecycle_sink(&self) -> Option<EventSink> {
        let sender = self.lifecycle_events.clone()?;
        Some(Arc::new(move |event| {
            sender.send(event).ok();
        }))
    }

    /// Starts serving `org.freedesktop.Application` under the application id, once it is
    /// known. Without a session bus, or when another process owns the name, the failure is
    /// logged and the application runs without the service.
    pub(crate) fn start_application_service(&mut self) {
        if self.application_service_started {
            return;
        }
        let (Some(app_id), Some(events)) = (self.app_id.clone(), self.lifecycle_sink()) else {
            return;
        };
        self.application_service_started = true;
        if let Err(error) = start(Box::new(SessionBus), &app_id, events) {
            log::warn!("not serving org.freedesktop.Application: {error:#}");
        }
    }

    /// Reports `SIGTERM` and `SIGHUP` as quit requests, once.
    pub(crate) fn install_termination_handlers(&mut self) {
        if self.termination_handlers_installed {
            return;
        }
        let Some(events) = self.lifecycle_sink() else {
            return;
        };
        self.termination_handlers_installed = true;
        if let Err(error) = install_termination_handlers(events) {
            log::warn!("termination signals will not be reported as quit requests: {error:#}");
        }
    }
}

/// Runs the platform callbacks for lifecycle events. Each callback runs with the platform's
/// common state unborrowed, since it calls back into the application and the platform.
struct CommonHandlers<'a>(&'a RefCell<LinuxCommon>);

impl CommonHandlers<'_> {
    fn call<F: ?Sized>(
        &self,
        slot: fn(&mut PlatformHandlers) -> &mut Option<Box<F>>,
        call: impl FnOnce(&mut Box<F>),
    ) {
        let callback = slot(&mut self.0.borrow_mut().callbacks).take();
        if let Some(mut callback) = callback {
            call(&mut callback);
            let mut common = self.0.borrow_mut();
            // The callback may have replaced itself.
            slot(&mut common.callbacks).get_or_insert(callback);
        }
    }
}

impl LifecycleHandlers for CommonHandlers<'_> {
    fn reopen(&mut self) {
        self.call(|callbacks| &mut callbacks.reopen, |callback| callback());
    }

    fn open_urls(&mut self, urls: Vec<String>) {
        self.call(
            |callbacks| &mut callbacks.open_urls,
            |callback| callback(urls),
        );
    }

    fn activate_action(&mut self, action: String) {
        self.call(
            |callbacks| &mut callbacks.activate_action,
            |callback| callback(action),
        );
    }

    fn activation_request(&mut self, request: ActivationRequest) {
        self.call(
            |callbacks| &mut callbacks.activation_request,
            |callback| callback(request),
        );
    }

    fn quit_requested(&mut self) {
        self.call(
            |callbacks| &mut callbacks.quit_requested,
            |callback| callback(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
        urls: Vec<Vec<String>>,
        requests: Vec<ActivationRequest>,
    }

    impl LifecycleHandlers for Recorder {
        fn reopen(&mut self) {
            self.calls.push("reopen".into());
        }
        fn open_urls(&mut self, urls: Vec<String>) {
            self.calls.push("open_urls".into());
            self.urls.push(urls);
        }
        fn activate_action(&mut self, action: String) {
            self.calls.push(format!("action:{action}"));
        }
        fn activation_request(&mut self, request: ActivationRequest) {
            self.calls.push("activation".into());
            self.requests.push(request);
        }
        fn quit_requested(&mut self) {
            self.calls.push("quit".into());
        }
    }

    fn data(entries: &[(&str, &str)]) -> PlatformData {
        entries
            .iter()
            .map(|(key, value)| {
                (
                    (*key).to_owned(),
                    OwnedValue::try_from(zbus::zvariant::Value::from(*value)).unwrap(),
                )
            })
            .collect()
    }

    /// A transport that records what it was asked to serve and lets the test emit calls.
    #[derive(Clone, Default)]
    struct FakeTransport {
        served: Arc<Mutex<Option<(String, EventSink)>>>,
    }

    impl ApplicationTransport for FakeTransport {
        fn serve(self: Box<Self>, app_id: &str, events: EventSink) -> Result<()> {
            *self.served.lock().unwrap() = Some((app_id.to_owned(), events));
            Ok(())
        }
    }

    fn collecting_sink() -> (EventSink, Arc<Mutex<Vec<LifecycleEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink: EventSink = {
            let events = events.clone();
            Arc::new(move |event| events.lock().unwrap().push(event))
        };
        (sink, events)
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        smol::block_on(future)
    }

    #[test]
    fn activate_is_a_reopen_followed_by_an_activation_request_with_the_token() {
        let mut handlers = Recorder::default();
        dispatch(
            LifecycleEvent::Activate {
                token: Some("tok".into()),
            },
            &mut handlers,
        );
        assert_eq!(handlers.calls, ["reopen", "activation"]);
        assert_eq!(
            handlers.requests,
            [ActivationRequest {
                token: Some("tok".into())
            }]
        );
    }

    #[test]
    fn open_delivers_the_uris_exactly_as_given_in_order() {
        let uris = vec![
            "HTTPS://Example.COM/a b?x=%7e".to_owned(),
            "myapp:open?id=1".to_owned(),
        ];
        let mut handlers = Recorder::default();
        dispatch(
            LifecycleEvent::Open {
                uris: uris.clone(),
                token: None,
            },
            &mut handlers,
        );
        assert_eq!(handlers.calls, ["open_urls", "activation"]);
        assert_eq!(handlers.urls, [uris]);
        assert_eq!(handlers.requests, [ActivationRequest { token: None }]);
    }

    #[test]
    fn activate_action_reports_the_action_name_then_activates() {
        let mut handlers = Recorder::default();
        dispatch(
            LifecycleEvent::ActivateAction {
                action: "new-window".into(),
                token: None,
            },
            &mut handlers,
        );
        assert_eq!(handlers.calls, ["action:new-window", "activation"]);
    }

    #[test]
    fn termination_is_a_quit_request_without_activation() {
        let mut handlers = Recorder::default();
        dispatch(LifecycleEvent::QuitRequested, &mut handlers);
        assert_eq!(handlers.calls, ["quit"]);
    }

    #[test]
    fn token_comes_from_activation_token_then_desktop_startup_id() {
        assert_eq!(
            activation_token(&data(&[
                (STARTUP_ID_KEY, "startup"),
                (ACTIVATION_TOKEN_KEY, "token")
            ])),
            Some("token".to_owned())
        );
        assert_eq!(
            activation_token(&data(&[(STARTUP_ID_KEY, "startup_TIME123")])),
            Some("startup_TIME123".to_owned())
        );
        assert_eq!(activation_token(&data(&[(ACTIVATION_TOKEN_KEY, "")])), None);
        assert_eq!(activation_token(&data(&[])), None);
    }

    #[test]
    fn object_path_follows_the_application_id() {
        assert_eq!(object_path("org.example.My-App"), "/org/example/My_App");
    }

    #[test]
    fn invalid_application_ids_are_rejected_before_serving() {
        let transport = FakeTransport::default();
        let served = transport.served.clone();
        let (sink, _) = collecting_sink();
        assert!(start(Box::new(transport.clone()), "nodots", sink.clone()).is_err());
        assert!(start(Box::new(transport.clone()), "org.1bad.App", sink).is_err());
        assert!(served.lock().unwrap().is_none());
    }

    #[test]
    fn interface_calls_reach_the_handlers_through_an_injected_transport() {
        let transport = FakeTransport::default();
        let served = transport.served.clone();
        let (sink, events) = collecting_sink();
        start(Box::new(transport), "org.example.App", sink).unwrap();
        let (id, emit) = served.lock().unwrap().take().unwrap();
        assert_eq!(id, "org.example.App");

        // Calls arrive as the bus would deliver them: through the interface object.
        let interface = ApplicationInterface::new(emit);
        block_on(interface.activate(data(&[(ACTIVATION_TOKEN_KEY, "t1")])));
        block_on(interface.open(
            vec!["one://a".into(), "two://b".into()],
            data(&[(STARTUP_ID_KEY, "t2")]),
        ));
        block_on(interface.activate_action("quit".into(), Vec::new(), data(&[])));

        let mut handlers = Recorder::default();
        for event in events.lock().unwrap().drain(..) {
            dispatch(event, &mut handlers);
        }
        assert_eq!(
            handlers.calls,
            [
                "reopen",
                "activation",
                "open_urls",
                "activation",
                "action:quit",
                "activation"
            ]
        );
        assert_eq!(
            handlers.urls,
            [vec!["one://a".to_owned(), "two://b".to_owned()]]
        );
        assert_eq!(
            handlers.requests,
            [
                ActivationRequest {
                    token: Some("t1".into())
                },
                ActivationRequest {
                    token: Some("t2".into())
                },
                ActivationRequest { token: None }
            ]
        );
    }
}
