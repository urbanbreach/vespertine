//! PLT-012 acceptance test A5: shortcut validation, formatting, preference
//! state, Wayland portal sessions and X11 key grabs.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{
    FutureExt as _, StreamExt as _,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
    future::BoxFuture,
};
use parking_lot::Mutex;

use super::{
    wayland::{
        GlobalShortcutsPortal, PortalShortcut, SHORTCUT_ID, ShortcutSession,
        WaylandShortcutRegistration,
    },
    x11::{
        GrabParameters, KeyPressEvent, LOCK_VARIANTS, SHORTCUT_GRAB, X11KeyGrabber,
        X11ShortcutRegistration,
    },
    *,
};
use crate::capture::{CapabilityState, portal::PortalError, x11::X11Error};

fn shortcut(text: &str) -> Shortcut {
    Shortcut::parse(text).unwrap()
}

type Log = Arc<Mutex<Vec<String>>>;

fn entries(log: &Log) -> Vec<String> {
    log.lock().clone()
}

// ---------------------------------------------------------------- parsing

#[test]
fn spec_invalid_combinations_are_rejected() {
    for text in ["a", "shift-a", "fn-a", "ctrl-mystery", "ctrl-f25"] {
        assert!(Shortcut::validate(text).is_err(), "{text} must be invalid");
    }
    assert_eq!(
        Shortcut::parse("fn-a"),
        Err(ShortcutParseError::FunctionModifier)
    );
    assert_eq!(
        Shortcut::parse("ctrl-fn-a"),
        Err(ShortcutParseError::FunctionModifier)
    );
    assert_eq!(
        Shortcut::parse("shift-a"),
        Err(ShortcutParseError::MissingModifier)
    );
    assert_eq!(Shortcut::parse(""), Err(ShortcutParseError::Empty));
}

#[test]
fn spec_valid_combinations_are_accepted() {
    for text in ["ctrl-alt-space", "ctrl-shift-k", "alt-f12", "ctrl-pageup"] {
        assert!(Shortcut::validate(text).is_ok(), "{text} must be valid");
    }
}

#[test]
fn all_supported_keys_parse_and_others_do_not() {
    for key in [
        "space",
        "tab",
        "enter",
        "backspace",
        "delete",
        "insert",
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "pageup",
        "pagedown",
        "a",
        "z",
        "0",
        "9",
        "f1",
        "f20",
    ] {
        assert!(Shortcut::parse(&format!("ctrl-{key}")).is_ok(), "{key}");
    }
    for key in [
        "f0", "f21", "f01", "esc", "escape", "-", "é", "aa", "ctrl", "pgup", "f1x",
    ] {
        assert!(Shortcut::parse(&format!("ctrl-{key}")).is_err(), "{key}");
    }
    for modifier in ["platform", "super", "cmd", "alt", "control"] {
        assert!(
            Shortcut::parse(&format!("{modifier}-k")).is_ok(),
            "{modifier}"
        );
    }
    assert!(Shortcut::parse("hyper-k").is_err());
}

#[test]
fn format_is_canonical_and_round_trips() {
    for text in [
        "ctrl-alt-shift-platform-f5",
        "alt-k",
        "platform-space",
        "ctrl-shift-5",
    ] {
        assert_eq!(shortcut(text).format(), text);
        assert_eq!(shortcut(&shortcut(text).format()), shortcut(text));
    }
    assert_eq!(shortcut("shift-ctrl-k").format(), "ctrl-shift-k");
    assert_eq!(shortcut("CTRL-Alt-K").format(), "ctrl-alt-k");
}

#[test]
fn unparsable_text_falls_back_to_the_default() {
    let default = shortcut("ctrl-alt-space");
    assert_eq!(Shortcut::parse_or("garbage", default.clone()), default);
    assert_eq!(Shortcut::parse_or("a", default.clone()), default);
    assert_eq!(Shortcut::parse_or("alt-k", default), shortcut("alt-k"));
}

#[test]
fn portal_triggers_use_xdg_syntax() {
    assert_eq!(
        shortcut("ctrl-alt-space").portal_trigger(),
        "CTRL+ALT+space"
    );
    assert_eq!(
        shortcut("ctrl-shift-pageup").portal_trigger(),
        "CTRL+SHIFT+Page_Up"
    );
    let cases = [
        ("ctrl-enter", "CTRL+Return"),
        ("ctrl-backspace", "CTRL+BackSpace"),
        ("ctrl-pagedown", "CTRL+Page_Down"),
        ("ctrl-tab", "CTRL+Tab"),
        ("ctrl-delete", "CTRL+Delete"),
        ("ctrl-insert", "CTRL+Insert"),
        ("ctrl-up", "CTRL+Up"),
        ("ctrl-home", "CTRL+Home"),
        ("ctrl-end", "CTRL+End"),
        ("alt-f5", "ALT+F5"),
        ("ctrl-k", "CTRL+k"),
        ("ctrl-7", "CTRL+7"),
        ("ctrl-alt-shift-platform-left", "CTRL+ALT+SHIFT+LOGO+Left"),
        ("platform-space", "LOGO+space"),
    ];
    for (text, expected) in cases {
        assert_eq!(shortcut(text).portal_trigger(), expected, "{text}");
    }
}

#[test]
fn x11_keysyms_and_modifier_masks_follow_the_spec() {
    let keysyms = [
        ("space", 0x20),
        ("tab", 0xff09),
        ("enter", 0xff0d),
        ("backspace", 0xff08),
        ("delete", 0xffff),
        ("insert", 0xff63),
        ("up", 0xff52),
        ("down", 0xff54),
        ("left", 0xff51),
        ("right", 0xff53),
        ("home", 0xff50),
        ("end", 0xff57),
        ("pageup", 0xff55),
        ("pagedown", 0xff56),
        ("k", u32::from(b'k')),
        ("5", u32::from(b'5')),
        ("f1", 0xffbe),
        ("f12", 0xffc9),
        ("f20", 0xffd1),
    ];
    for (key, keysym) in keysyms {
        assert_eq!(
            shortcut(&format!("ctrl-{key}")).x11_keysym(),
            keysym,
            "{key}"
        );
    }
    assert_eq!(shortcut("ctrl-k").x11_modifier_mask(), 0x04);
    assert_eq!(shortcut("alt-k").x11_modifier_mask(), 0x08);
    assert_eq!(shortcut("ctrl-shift-k").x11_modifier_mask(), 0x05);
    assert_eq!(shortcut("platform-k").x11_modifier_mask(), 0x40);
    assert_eq!(
        shortcut("ctrl-alt-shift-platform-k").x11_modifier_mask(),
        0x4d
    );
}

// ------------------------------------------------------- preference state

fn drain<T>(receiver: &mut UnboundedReceiver<T>) -> Vec<T> {
    let mut items = Vec::new();
    while let Ok(item) = receiver.try_recv() {
        items.push(item);
    }
    items
}

#[test]
fn active_shortcut_requires_enabled_and_not_recording() {
    let service = ShortcutService::new(shortcut("ctrl-alt-space"), true);
    assert_eq!(service.active_shortcut(), Some(shortcut("ctrl-alt-space")));
    service.set_recording(true);
    assert_eq!(service.active_shortcut(), None);
    service.set_recording(false);
    service.set_enabled(false);
    assert_eq!(service.active_shortcut(), None);
    assert!(service.refusal_reason().is_some());
    service.set_enabled(true);
    assert_eq!(service.refusal_reason(), None);
    service.set_recording(true);
    assert!(service.refusal_reason().is_some());
}

#[test]
fn new_subscribers_receive_the_current_value_and_changes_are_pushed() {
    let service = ShortcutService::new(shortcut("ctrl-alt-space"), true);
    let mut subscriber = service.subscribe();
    assert_eq!(
        drain(&mut subscriber),
        vec![Some(shortcut("ctrl-alt-space"))]
    );

    service.set_combination(shortcut("alt-f12"));
    service.set_recording(true);
    service.set_recording(false);
    service.set_enabled(false);
    service.set_enabled(true);
    assert_eq!(
        drain(&mut subscriber),
        vec![
            Some(shortcut("alt-f12")),
            None,
            Some(shortcut("alt-f12")),
            None,
            Some(shortcut("alt-f12")),
        ]
    );

    // A late subscriber sees only the current value.
    let mut late = service.subscribe();
    assert_eq!(drain(&mut late), vec![Some(shortcut("alt-f12"))]);
}

#[test]
fn setting_the_current_value_does_nothing() {
    let service = ShortcutService::new(shortcut("ctrl-alt-space"), true);
    let mut subscriber = service.subscribe();
    drain(&mut subscriber);
    service.set_combination(shortcut("ctrl-alt-space"));
    service.set_enabled(true);
    service.set_recording(false);
    assert!(drain(&mut subscriber).is_empty());
}

#[test]
fn changes_while_inactive_push_nothing_until_active_again() {
    let service = ShortcutService::new(shortcut("ctrl-alt-space"), true);
    let mut subscriber = service.subscribe();
    drain(&mut subscriber);
    service.set_recording(true);
    assert_eq!(drain(&mut subscriber), vec![None]);
    service.set_combination(shortcut("ctrl-shift-k"));
    assert!(drain(&mut subscriber).is_empty());
    service.set_recording(false);
    assert_eq!(drain(&mut subscriber), vec![Some(shortcut("ctrl-shift-k"))]);
}

#[test]
fn combination_text_falls_back_to_the_default() {
    let default = shortcut("ctrl-alt-space");
    let service = ShortcutService::from_text("nonsense", default.clone(), true);
    assert_eq!(service.preferences().combination, default);
    let applied = service.set_combination_text("shift-a", default.clone());
    assert_eq!(applied, default);
    let applied = service.set_combination_text("alt-f9", default);
    assert_eq!(service.preferences().combination, applied);
    assert_eq!(applied, shortcut("alt-f9"));
}

// -------------------------------------------------------------- Wayland fakes

struct FakePortal {
    log: Log,
    version: Mutex<Result<u32, PortalError>>,
    bind_result: Arc<Mutex<Result<(), PortalError>>>,
    next_session: Mutex<u32>,
    /// Activations delivered to each new session.
    scripted_activations: Arc<Mutex<VecDeque<String>>>,
}

impl FakePortal {
    fn new(log: &Log, version: u32) -> Arc<Self> {
        Arc::new(Self {
            log: log.clone(),
            version: Mutex::new(Ok(version)),
            bind_result: Arc::new(Mutex::new(Ok(()))),
            next_session: Mutex::new(1),
            scripted_activations: Arc::new(Mutex::new(VecDeque::new())),
        })
    }
}

struct SharedPortal(Arc<FakePortal>);

impl GlobalShortcutsPortal for SharedPortal {
    fn version(&self) -> BoxFuture<'_, Result<u32, PortalError>> {
        let version = self.0.version.lock().clone();
        async move { version }.boxed()
    }

    fn create_session(&self) -> BoxFuture<'_, Result<Box<dyn ShortcutSession>, PortalError>> {
        let mut next = self.0.next_session.lock();
        let id = *next;
        *next += 1;
        self.0.log.lock().push(format!("create {id}"));
        let session = FakeSession {
            id,
            log: self.0.log.clone(),
            bind_result: self.0.bind_result.clone(),
            activations: self.0.scripted_activations.clone(),
        };
        async move { Ok(Box::new(session) as Box<dyn ShortcutSession>) }.boxed()
    }
}

struct FakeSession {
    id: u32,
    log: Log,
    bind_result: Arc<Mutex<Result<(), PortalError>>>,
    activations: Arc<Mutex<VecDeque<String>>>,
}

impl ShortcutSession for FakeSession {
    fn bind(&mut self, shortcut: &PortalShortcut) -> BoxFuture<'_, Result<(), PortalError>> {
        self.log.lock().push(format!(
            "bind {} {} {:?} {}",
            self.id, shortcut.preferred_trigger, shortcut.id, shortcut.description
        ));
        let result = self.bind_result.lock().clone();
        async move { result }.boxed()
    }

    fn configure(&mut self) -> BoxFuture<'_, Result<(), PortalError>> {
        self.log.lock().push(format!("configure {}", self.id));
        async { Ok(()) }.boxed()
    }

    fn next_activated(&mut self) -> BoxFuture<'_, Option<String>> {
        let next = self.activations.lock().pop_front();
        async move {
            match next {
                Some(id) => Some(id),
                None => futures::future::pending().await,
            }
        }
        .boxed()
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), PortalError>> {
        self.log.lock().push(format!("close {}", self.id));
        async { Ok(()) }.boxed()
    }
}

fn wayland_registration(portal: &Arc<FakePortal>) -> WaylandShortcutRegistration {
    WaylandShortcutRegistration::new(Box::new(SharedPortal(portal.clone())))
}

// ---------------------------------------------------------------- Wayland

#[test]
fn wayland_binds_one_shortcut_with_a_fixed_id_and_the_formatted_trigger() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    let mut registration = wayland_registration(&portal);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();
    let entries = entries(&log);
    assert_eq!(entries[0], "create 1");
    assert_eq!(
        entries[1],
        format!("bind 1 CTRL+ALT+space {SHORTCUT_ID:?} Capture the active window")
    );
    // First binding: no earlier combination, so no configure request.
    assert_eq!(entries.len(), 2, "{entries:?}");
}

#[test]
fn wayland_rebinding_closes_the_old_session_first_and_configures_on_version_two() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    let mut registration = wayland_registration(&portal);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();
    smol::block_on(registration.apply(Some(shortcut("ctrl-shift-pageup")))).unwrap();
    let entries = entries(&log);
    let position = |needle: &str| {
        entries
            .iter()
            .position(|entry| entry.starts_with(needle))
            .unwrap_or_else(|| panic!("{needle} missing in {entries:?}"))
    };
    assert!(position("close 1") < position("create 2"), "{entries:?}");
    assert!(position("create 2") < position("bind 2 CTRL+SHIFT+Page_Up"));
    assert!(position("bind 2") < position("configure 2"));
    assert_eq!(
        entries
            .iter()
            .filter(|e| e.starts_with("configure"))
            .count(),
        1
    );
}

#[test]
fn wayland_version_one_never_requests_configuration() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 1);
    let mut registration = wayland_registration(&portal);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();
    smol::block_on(registration.apply(Some(shortcut("alt-f12")))).unwrap();
    assert!(!entries(&log).iter().any(|e| e.starts_with("configure")));
    assert!(entries(&log).iter().any(|e| e == "close 1"));
}

#[test]
fn wayland_recording_suspends_and_restores_without_reconfiguring() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    let mut registration = wayland_registration(&portal);
    let combination = shortcut("ctrl-alt-space");
    smol::block_on(registration.apply(Some(combination.clone()))).unwrap();
    smol::block_on(registration.apply(None)).unwrap();
    assert_eq!(entries(&log).last().unwrap(), "close 1");
    smol::block_on(registration.apply(Some(combination))).unwrap();
    let entries = entries(&log);
    assert!(entries.contains(&"create 2".to_owned()));
    // The restored combination equals the one bound earlier.
    assert!(
        !entries.iter().any(|e| e.starts_with("configure")),
        "{entries:?}"
    );
}

#[test]
fn wayland_rejected_bind_requires_setup_closes_the_session_and_retries_on_the_next_change() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    *portal.bind_result.lock() = Err(PortalError::Denied("refused".into()));
    let mut registration = wayland_registration(&portal);
    let result = smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space"))));
    assert!(
        matches!(result, Err(ShortcutError::SetupRequired(_))),
        "{result:?}"
    );
    assert_eq!(
        entries(&log).last().unwrap(),
        "close 1",
        "session always closed"
    );

    *portal.bind_result.lock() = Ok(());
    smol::block_on(registration.apply(Some(shortcut("alt-f12")))).unwrap();
    assert!(
        entries(&log)
            .iter()
            .any(|e| e.starts_with("bind 2 ALT+F12"))
    );
}

#[test]
fn wayland_missing_portal_requires_setup() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    *portal.version.lock() = Err(PortalError::Unreachable("no portal".into()));
    let mut registration = wayland_registration(&portal);
    let result = smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space"))));
    assert!(
        matches!(result, Err(ShortcutError::SetupRequired(reason)) if reason.contains("no portal"))
    );
    assert!(entries(&log).is_empty());
}

#[test]
fn wayland_activations_are_filtered_to_the_own_shortcut_id() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    portal.scripted_activations.lock().extend([
        "something-else".to_owned(),
        "another".to_owned(),
        SHORTCUT_ID.to_owned(),
    ]);
    let mut registration = wayland_registration(&portal);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();
    smol::block_on(registration.next_activation());

    // With only foreign ids pending, no activation is produced.
    portal
        .scripted_activations
        .lock()
        .extend(["foreign".to_owned()]);
    let mut future = registration.next_activation();
    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    assert!(future.as_mut().poll(&mut cx).is_pending());
}

#[test]
fn wayland_setup_configures_the_open_session_or_retries() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    let mut registration = wayland_registration(&portal);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();
    smol::block_on(registration.setup()).unwrap();
    assert_eq!(entries(&log).last().unwrap(), "configure 1");

    // After a rejected bind there is no session: setup retries the bind.
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    *portal.bind_result.lock() = Err(PortalError::Denied("refused".into()));
    let mut registration = wayland_registration(&portal);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap_err();
    *portal.bind_result.lock() = Ok(());
    smol::block_on(registration.setup()).unwrap();
    assert!(
        entries(&log)
            .iter()
            .any(|e| e.starts_with("bind 2 CTRL+ALT+space"))
    );
}

// ------------------------------------------------------------ driver + service

async fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition not reached in time");
        smol::Timer::at(std::time::Instant::now() + Duration::from_millis(2)).await;
    }
}

#[test]
fn driver_applies_preference_changes_and_publishes_state_and_activations() {
    let log = Log::default();
    let portal = FakePortal::new(&log, 2);
    let service = ShortcutService::new(shortcut("ctrl-alt-space"), true);
    let mut activations = service.activations();
    assert_eq!(service.capability_state(), CapabilityState::Checking);
    let driver = ShortcutDriver::new(service.clone(), wayland_registration(&portal));

    smol::block_on(async {
        let task = smol::spawn(driver.run());
        wait_until(|| service.capability_state() == CapabilityState::Ready).await;

        // Activation of the own id surfaces as an event.
        portal
            .scripted_activations
            .lock()
            .push_back(SHORTCUT_ID.to_owned());
        service.set_combination(shortcut("ctrl-shift-k"));
        wait_until(|| entries(&log).iter().any(|e| e == "configure 2")).await;
        let activation = next_within(&mut activations).await;
        assert_eq!(activation.shortcut, shortcut("ctrl-shift-k"));

        // Recording releases the registration; ending it restores.
        service.set_recording(true);
        wait_until(|| entries(&log).iter().any(|e| e == "close 2")).await;
        wait_until(|| service.capability_state() == CapabilityState::Unavailable).await;
        service.set_recording(false);
        wait_until(|| service.capability_state() == CapabilityState::Ready).await;

        // A rejected bind sets setup-required.
        *portal.bind_result.lock() = Err(PortalError::Denied("no".into()));
        service.set_combination(shortcut("alt-f12"));
        wait_until(|| service.capability_state() == CapabilityState::SetupRequired).await;
        assert!(matches!(
            service.registration_error(),
            Some(ShortcutError::SetupRequired(_))
        ));

        // The user-driven setup action retries after the cause is fixed.
        *portal.bind_result.lock() = Ok(());
        service.request_setup();
        wait_until(|| service.capability_state() == CapabilityState::Ready).await;
        assert_eq!(service.registration_error(), None);

        service.shutdown();
        task.await;
    });
    assert_eq!(
        entries(&log)
            .iter()
            .filter(|e| e.starts_with("close"))
            .count(),
        entries(&log)
            .iter()
            .filter(|e| e.starts_with("create"))
            .count(),
        "every session is closed: {:?}",
        entries(&log)
    );
}

async fn next_within<T>(receiver: &mut UnboundedReceiver<T>) -> T {
    match futures::future::select(
        receiver.next(),
        smol::Timer::at(std::time::Instant::now() + Duration::from_secs(5)),
    )
    .await
    {
        futures::future::Either::Left((item, _)) => item.expect("stream ended"),
        futures::future::Either::Right(_) => panic!("no event within 5 seconds"),
    }
}

// --------------------------------------------------------------------- X11

struct FakeGrabber {
    log: Log,
    keymap: HashMap<u32, u8>,
    grab_calls: Mutex<usize>,
    /// 1-based index of the grab call that fails.
    fail_grab: Mutex<Option<usize>>,
    parameters: Mutex<Vec<GrabParameters>>,
    events: Mutex<UnboundedReceiver<KeyPressEvent>>,
}

fn fake_grabber(log: &Log) -> (Arc<FakeGrabber>, UnboundedSender<KeyPressEvent>) {
    let (sender, receiver) = unbounded();
    let keymap = HashMap::from([
        (0x20, 65u8),
        (u32::from(b'k'), 45),
        (0xffbe + 11, 96),
        (0xff55, 112),
    ]);
    (
        Arc::new(FakeGrabber {
            log: log.clone(),
            keymap,
            grab_calls: Mutex::new(0),
            fail_grab: Mutex::new(None),
            parameters: Mutex::new(Vec::new()),
            events: Mutex::new(receiver),
        }),
        sender,
    )
}

struct SharedGrabber(Arc<FakeGrabber>);

impl X11KeyGrabber for SharedGrabber {
    fn keycode_for_keysym(&self, keysym: u32) -> Result<Option<u8>, X11Error> {
        self.0.log.lock().push(format!("keycode {keysym:#x}"));
        Ok(self.0.keymap.get(&keysym).copied())
    }

    fn grab_key(
        &self,
        keycode: u8,
        modifiers: u16,
        parameters: GrabParameters,
    ) -> Result<(), X11Error> {
        let mut calls = self.0.grab_calls.lock();
        *calls += 1;
        self.0.parameters.lock().push(parameters);
        if *self.0.fail_grab.lock() == Some(*calls) {
            self.0
                .log
                .lock()
                .push(format!("grab-failed {keycode} {modifiers:#x}"));
            return Err(X11Error::new("BadAccess"));
        }
        self.0
            .log
            .lock()
            .push(format!("grab {keycode} {modifiers:#x}"));
        Ok(())
    }

    fn ungrab_key(&self, keycode: u8, modifiers: u16) -> Result<(), X11Error> {
        self.0
            .log
            .lock()
            .push(format!("ungrab {keycode} {modifiers:#x}"));
        Ok(())
    }

    fn drain_events(&self) -> Result<(), X11Error> {
        self.0.log.lock().push("drain".into());
        while self.0.events.lock().try_recv().is_ok() {}
        Ok(())
    }

    fn next_key_press(&self) -> BoxFuture<'_, Option<KeyPressEvent>> {
        futures::future::poll_fn(move |cx| self.0.events.lock().poll_next_unpin(cx)).boxed()
    }
}

fn x11_registration(grabber: &Arc<FakeGrabber>) -> X11ShortcutRegistration<SharedGrabber> {
    X11ShortcutRegistration::new(SharedGrabber(grabber.clone()))
}

#[test]
fn x11_makes_four_grabs_in_async_mode_with_owner_events_off() {
    let log = Log::default();
    let (grabber, _events) = fake_grabber(&log);
    let mut registration = x11_registration(&grabber);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();
    let base = 0x04 | 0x08;
    assert_eq!(
        entries(&log)
            .into_iter()
            .filter(|e| e.starts_with("grab "))
            .collect::<Vec<_>>(),
        vec![
            format!("grab 65 {base:#x}"),
            format!("grab 65 {:#x}", base | 0x02),
            format!("grab 65 {:#x}", base | 0x10),
            format!("grab 65 {:#x}", base | 0x02 | 0x10),
        ]
    );
    assert_eq!(*grabber.parameters.lock(), vec![SHORTCUT_GRAB; 4]);
    assert!(!SHORTCUT_GRAB.owner_events && SHORTCUT_GRAB.asynchronous);
    assert_eq!(LOCK_VARIANTS.len(), 4);
}

#[test]
fn x11_failure_of_any_grab_releases_all_of_them() {
    for failing in 1..=4 {
        let log = Log::default();
        let (grabber, _events) = fake_grabber(&log);
        *grabber.fail_grab.lock() = Some(failing);
        let mut registration = x11_registration(&grabber);
        let result = smol::block_on(registration.apply(Some(shortcut("ctrl-shift-k"))));
        assert!(
            matches!(&result, Err(ShortcutError::Unavailable(reason)) if reason.contains("another client")),
            "{result:?}"
        );
        let entries = entries(&log);
        let base = 0x04 | 0x01;
        for variant in [0, 0x02, 0x10, 0x12] {
            assert!(
                entries.contains(&format!("ungrab 45 {:#x}", base | variant)),
                "grab {failing}: variant {variant:#x} not released in {entries:?}"
            );
        }
        // Nothing stays grabbed: no activation is possible.
        let mut future = registration.next_activation();
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
}

#[test]
fn x11_missing_key_fails_registration() {
    let log = Log::default();
    let (grabber, _events) = fake_grabber(&log);
    let mut registration = x11_registration(&grabber);
    let result = smol::block_on(registration.apply(Some(shortcut("ctrl-f5"))));
    assert!(matches!(result, Err(ShortcutError::Unavailable(_))));
    assert!(!entries(&log).iter().any(|e| e.starts_with("grab")));
}

#[test]
fn x11_rebinding_releases_first_and_drains_pending_events() {
    let log = Log::default();
    let (grabber, events) = fake_grabber(&log);
    let mut registration = x11_registration(&grabber);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();
    events
        .unbounded_send(KeyPressEvent {
            keycode: 65,
            state: 0x0c,
        })
        .unwrap();
    log.lock().clear();
    smol::block_on(registration.apply(Some(shortcut("ctrl-shift-k")))).unwrap();
    let entries = entries(&log);
    let last_ungrab = entries
        .iter()
        .rposition(|e| e.starts_with("ungrab"))
        .unwrap();
    let drain = entries.iter().position(|e| e == "drain").unwrap();
    let first_grab = entries.iter().position(|e| e.starts_with("grab")).unwrap();
    assert!(last_ungrab < drain && drain < first_grab, "{entries:?}");
    assert_eq!(
        entries.iter().filter(|e| e.starts_with("ungrab")).count(),
        4
    );
    // The stale event of the old grab was drained.
    assert!(grabber.events.lock().try_recv().is_err());
}

#[test]
fn x11_recording_ungrabs_and_restore_regrabs() {
    let log = Log::default();
    let (grabber, _events) = fake_grabber(&log);
    let mut registration = x11_registration(&grabber);
    let combination = shortcut("ctrl-alt-space");
    smol::block_on(registration.apply(Some(combination.clone()))).unwrap();
    log.lock().clear();
    smol::block_on(registration.apply(None)).unwrap();
    assert_eq!(
        entries(&log)
            .iter()
            .filter(|e| e.starts_with("ungrab"))
            .count(),
        4
    );
    assert_eq!(
        entries(&log)
            .iter()
            .filter(|e| e.starts_with("grab"))
            .count(),
        0
    );
    smol::block_on(registration.apply(Some(combination))).unwrap();
    assert_eq!(
        entries(&log)
            .iter()
            .filter(|e| e.starts_with("grab "))
            .count(),
        4
    );
}

#[test]
fn x11_activation_filters_keycode_and_modifiers_ignoring_locks_and_is_prompt() {
    let log = Log::default();
    let (grabber, events) = fake_grabber(&log);
    let mut registration = x11_registration(&grabber);
    smol::block_on(registration.apply(Some(shortcut("ctrl-alt-space")))).unwrap();

    let base = 0x04u16 | 0x08;
    for event in [
        KeyPressEvent {
            keycode: 66,
            state: base,
        },
        KeyPressEvent {
            keycode: 65,
            state: 0x04,
        },
        KeyPressEvent {
            keycode: 65,
            state: base | 0x01,
        },
    ] {
        events.unbounded_send(event).unwrap();
    }
    // Caps lock and num lock do not defeat the shortcut.
    events
        .unbounded_send(KeyPressEvent {
            keycode: 65,
            state: base | 0x02 | 0x10,
        })
        .unwrap();
    let started = Instant::now();
    smol::block_on(registration.next_activation());
    assert!(
        started.elapsed() < Duration::from_millis(20),
        "{:?}",
        started.elapsed()
    );
    // Exactly the one matching event was consumed as the activation.
    assert!(grabber.events.lock().try_recv().is_err());
}

#[test]
fn x11_driver_reports_setup_required_and_setup_retries() {
    let log = Log::default();
    let (grabber, events) = fake_grabber(&log);
    *grabber.fail_grab.lock() = Some(3);
    let service = ShortcutService::new(shortcut("ctrl-alt-space"), true);
    let mut activations = service.activations();
    let driver = ShortcutDriver::new(service.clone(), x11_registration(&grabber));
    smol::block_on(async {
        let task = smol::spawn(driver.run());
        wait_until(|| service.capability_state() == CapabilityState::SetupRequired).await;
        assert!(matches!(
            service.registration_error(),
            Some(ShortcutError::Unavailable(_))
        ));

        // The user-driven retry succeeds once the key is free.
        *grabber.fail_grab.lock() = None;
        service.request_setup();
        wait_until(|| service.capability_state() == CapabilityState::Ready).await;

        events
            .unbounded_send(KeyPressEvent {
                keycode: 65,
                state: 0x04 | 0x08 | 0x02,
            })
            .unwrap();
        let activation = next_within(&mut activations).await;
        assert_eq!(activation.shortcut, shortcut("ctrl-alt-space"));

        service.set_recording(true);
        wait_until(|| service.capability_state() == CapabilityState::Unavailable).await;
        service.shutdown();
        task.await;
    });
}
