//! Global shortcut service (PLT-012 R4).
//!
//! * [`Shortcut`] is the validated value with its parse/format helpers, the
//!   XDG portal trigger string and the X11 keysym/modifier mapping.
//! * [`ShortcutService`] holds the preference state (combination, enabled,
//!   recording), pushes every change of the active shortcut to subscribers
//!   (a new subscriber immediately receives the current value), publishes
//!   activations and the shortcut capability state.
//! * A [`ShortcutRegistration`] ([`wayland`] GlobalShortcuts portal sessions,
//!   [`x11`] passive key grabs) is driven by a [`ShortcutDriver`], which
//!   applies each change of the active shortcut and forwards activations.
//!
//! All external boundaries (portal, X server) are injectable traits.

pub mod wayland;
pub mod x11;

#[cfg(test)]
mod tests;

use std::{fmt, sync::Arc};

use futures::{
    FutureExt as _, StreamExt as _,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
    future::BoxFuture,
    select_biased,
};
use parking_lot::Mutex;
use thiserror::Error;

use crate::capture::CapabilityState;

/// Why a combination was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShortcutParseError {
    #[error("the shortcut is empty")]
    Empty,
    #[error("the function modifier is not supported")]
    FunctionModifier,
    #[error("unknown modifier `{0}`")]
    UnknownModifier(String),
    #[error("unsupported key `{0}`")]
    UnsupportedKey(String),
    #[error("a shortcut must include control, alt or platform")]
    MissingModifier,
}

/// Failure to establish a registration.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShortcutError {
    /// Another client holds the key, or the key does not exist.
    #[error("shortcut unavailable: {0}")]
    Unavailable(String),
    /// The portal is missing, errored or refused the bind; the user must
    /// complete setup.
    #[error("shortcut setup required: {0}")]
    SetupRequired(String),
}

const NAMED_KEYS: [&str; 14] = [
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
];

/// X11 modifier masks.
pub mod modifier_mask {
    pub const SHIFT: u16 = 1 << 0;
    pub const LOCK: u16 = 1 << 1;
    pub const CONTROL: u16 = 1 << 2;
    pub const MOD1: u16 = 1 << 3;
    pub const MOD2: u16 = 1 << 4;
    pub const MOD4: u16 = 1 << 6;
}

/// A global shortcut: a key plus modifiers. Always valid: at least one of
/// control, alt or platform, and a supported key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Shortcut {
    control: bool,
    alt: bool,
    shift: bool,
    platform: bool,
    key: String,
}

impl Shortcut {
    /// Parses a combination written as lowercase modifier names and a key
    /// joined by hyphens, for example `ctrl-alt-space`. Modifier names:
    /// `ctrl`/`control`, `alt`, `shift`, `platform`/`super`/`logo`/`cmd`/`win`.
    /// Input is compared case-insensitively. `fn` is the unsupported function
    /// modifier.
    pub fn parse(text: &str) -> Result<Self, ShortcutParseError> {
        let text = text.trim().to_ascii_lowercase();
        if text.is_empty() {
            return Err(ShortcutParseError::Empty);
        }
        let mut parts: Vec<&str> = text.split('-').collect();
        let key = parts.pop().unwrap_or_default();
        let mut shortcut = Shortcut {
            control: false,
            alt: false,
            shift: false,
            platform: false,
            key: String::new(),
        };
        let mut function = false;
        for modifier in parts {
            match modifier {
                "ctrl" | "control" => shortcut.control = true,
                "alt" => shortcut.alt = true,
                "shift" => shortcut.shift = true,
                "platform" | "super" | "logo" | "cmd" | "win" => shortcut.platform = true,
                "fn" | "function" => function = true,
                other => return Err(ShortcutParseError::UnknownModifier(other.to_owned())),
            }
        }
        if function || key == "fn" {
            return Err(ShortcutParseError::FunctionModifier);
        }
        if !is_supported_key(key) {
            return Err(ShortcutParseError::UnsupportedKey(key.to_owned()));
        }
        if !(shortcut.control || shortcut.alt || shortcut.platform) {
            return Err(ShortcutParseError::MissingModifier);
        }
        shortcut.key = key.to_owned();
        Ok(shortcut)
    }

    /// Whether `text` is a valid combination.
    pub fn validate(text: &str) -> Result<(), ShortcutParseError> {
        Self::parse(text).map(|_| ())
    }

    /// Parses `text`, falling back to `default` when it is unparsable or
    /// unsupported.
    pub fn parse_or(text: &str, default: Shortcut) -> Shortcut {
        Self::parse(text).unwrap_or(default)
    }

    pub fn control(&self) -> bool {
        self.control
    }

    pub fn alt(&self) -> bool {
        self.alt
    }

    pub fn shift(&self) -> bool {
        self.shift
    }

    pub fn platform(&self) -> bool {
        self.platform
    }

    /// The lowercase key name (`k`, `5`, `space`, `pageup`, `f12`, ...).
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The canonical text form: modifiers in the order ctrl, alt, shift,
    /// platform, then the key, joined by hyphens.
    pub fn format(&self) -> String {
        let mut parts = Vec::new();
        if self.control {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.platform {
            parts.push("platform");
        }
        parts.push(&self.key);
        parts.join("-")
    }

    /// The trigger in XDG shortcut syntax: modifiers `CTRL`, `ALT`, `SHIFT`,
    /// `LOGO` in that order joined by `+`, then the key name (`enter` ->
    /// `Return`, `backspace` -> `BackSpace`, `pageup` -> `Page_Up`, `pagedown`
    /// -> `Page_Down`, `space` -> `space`, a single character unchanged, any
    /// other name with its first letter capitalized).
    pub fn portal_trigger(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.control {
            parts.push("CTRL".into());
        }
        if self.alt {
            parts.push("ALT".into());
        }
        if self.shift {
            parts.push("SHIFT".into());
        }
        if self.platform {
            parts.push("LOGO".into());
        }
        parts.push(match self.key.as_str() {
            "enter" => "Return".to_owned(),
            "backspace" => "BackSpace".to_owned(),
            "pageup" => "Page_Up".to_owned(),
            "pagedown" => "Page_Down".to_owned(),
            "space" => "space".to_owned(),
            key if key.chars().count() == 1 => key.to_owned(),
            key => {
                let mut chars = key.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            }
        });
        parts.join("+")
    }

    /// The X11 keysym of the key.
    pub fn x11_keysym(&self) -> u32 {
        match self.key.as_str() {
            "up" => 0xff52,
            "down" => 0xff54,
            "left" => 0xff51,
            "right" => 0xff53,
            "space" => 0x20,
            "pageup" => 0xff55,
            "pagedown" => 0xff56,
            "home" => 0xff50,
            "end" => 0xff57,
            "delete" => 0xffff,
            "insert" => 0xff63,
            "backspace" => 0xff08,
            "enter" => 0xff0d,
            "tab" => 0xff09,
            key => {
                if let Some(number) = function_key_number(key) {
                    0xffbe + number - 1
                } else {
                    key.chars().next().map_or(0, u32::from)
                }
            }
        }
    }

    /// The X11 modifier mask without lock modifiers: control, Mod1 for alt,
    /// shift, Mod4 for platform.
    pub fn x11_modifier_mask(&self) -> u16 {
        let mut mask = 0;
        if self.control {
            mask |= modifier_mask::CONTROL;
        }
        if self.alt {
            mask |= modifier_mask::MOD1;
        }
        if self.shift {
            mask |= modifier_mask::SHIFT;
        }
        if self.platform {
            mask |= modifier_mask::MOD4;
        }
        mask
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

fn function_key_number(key: &str) -> Option<u32> {
    let digits = key.strip_prefix('f')?;
    if digits.is_empty()
        || digits.len() > 2
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits
        .parse()
        .ok()
        .filter(|number| (1..=20).contains(number))
}

fn is_supported_key(key: &str) -> bool {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => c.is_ascii_lowercase() || c.is_ascii_digit(),
        (Some(_), Some(_)) => NAMED_KEYS.contains(&key) || function_key_number(key).is_some(),
        _ => false,
    }
}

/// The preference state: combination, enabled flag and recording flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutPreferences {
    pub combination: Shortcut,
    pub enabled: bool,
    pub recording: bool,
}

impl ShortcutPreferences {
    /// The combination while enabled and not recording.
    pub fn active(&self) -> Option<Shortcut> {
        (self.enabled && !self.recording).then(|| self.combination.clone())
    }
}

/// A shortcut activation event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutActivation {
    pub shortcut: Shortcut,
}

struct State {
    preferences: ShortcutPreferences,
    capability: CapabilityState,
    error: Option<ShortcutError>,
    active_subscribers: Vec<UnboundedSender<Option<Shortcut>>>,
    activation_subscribers: Vec<UnboundedSender<ShortcutActivation>>,
    setup_requests: Vec<UnboundedSender<()>>,
}

impl State {
    fn notify_if_changed(&mut self, before: Option<Shortcut>) {
        let after = self.preferences.active();
        if before != after {
            self.active_subscribers
                .retain(|subscriber| subscriber.unbounded_send(after.clone()).is_ok());
        }
    }
}

/// The shortcut preference state and status hub. Cloning shares the state.
#[derive(Clone)]
pub struct ShortcutService {
    state: Arc<Mutex<State>>,
}

impl ShortcutService {
    pub fn new(combination: Shortcut, enabled: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                preferences: ShortcutPreferences {
                    combination,
                    enabled,
                    recording: false,
                },
                capability: CapabilityState::Checking,
                error: None,
                active_subscribers: Vec::new(),
                activation_subscribers: Vec::new(),
                setup_requests: Vec::new(),
            })),
        }
    }

    /// Creates a service from a stored combination text; unparsable or
    /// unsupported text falls back to `default`.
    pub fn from_text(text: &str, default: Shortcut, enabled: bool) -> Self {
        Self::new(Shortcut::parse_or(text, default), enabled)
    }

    pub fn preferences(&self) -> ShortcutPreferences {
        self.state.lock().preferences.clone()
    }

    /// The active shortcut: the combination only while enabled and not
    /// recording.
    pub fn active_shortcut(&self) -> Option<Shortcut> {
        self.state.lock().preferences.active()
    }

    /// Sets the combination. Setting a value equal to the current one does
    /// nothing.
    pub fn set_combination(&self, combination: Shortcut) {
        let mut state = self.state.lock();
        if state.preferences.combination == combination {
            return;
        }
        let before = state.preferences.active();
        state.preferences.combination = combination;
        state.notify_if_changed(before);
    }

    /// Sets the combination from text; unparsable or unsupported text falls
    /// back to `default`. Returns the combination in effect.
    pub fn set_combination_text(&self, text: &str, default: Shortcut) -> Shortcut {
        let combination = Shortcut::parse_or(text, default);
        self.set_combination(combination.clone());
        combination
    }

    pub fn set_enabled(&self, enabled: bool) {
        let mut state = self.state.lock();
        if state.preferences.enabled == enabled {
            return;
        }
        let before = state.preferences.active();
        state.preferences.enabled = enabled;
        state.notify_if_changed(before);
    }

    /// Sets the recording flag. While recording, the active shortcut is
    /// `None`, so registrations are released and the keystrokes reach the
    /// recorder; they are restored when recording ends.
    pub fn set_recording(&self, recording: bool) {
        let mut state = self.state.lock();
        if state.preferences.recording == recording {
            return;
        }
        let before = state.preferences.active();
        state.preferences.recording = recording;
        state.notify_if_changed(before);
    }

    /// Subscribes to the active shortcut. The current value is delivered
    /// immediately, followed by every change.
    pub fn subscribe(&self) -> UnboundedReceiver<Option<Shortcut>> {
        let (sender, receiver) = unbounded();
        let mut state = self.state.lock();
        let _ = sender.unbounded_send(state.preferences.active());
        state.active_subscribers.push(sender);
        receiver
    }

    /// The activation event stream.
    pub fn activations(&self) -> UnboundedReceiver<ShortcutActivation> {
        let (sender, receiver) = unbounded();
        self.state.lock().activation_subscribers.push(sender);
        receiver
    }

    /// Publishes an activation of the active shortcut. Ignored when no
    /// shortcut is active.
    pub fn emit_activation(&self) {
        let mut state = self.state.lock();
        let Some(shortcut) = state.preferences.active() else {
            return;
        };
        state.activation_subscribers.retain(|subscriber| {
            subscriber
                .unbounded_send(ShortcutActivation {
                    shortcut: shortcut.clone(),
                })
                .is_ok()
        });
    }

    /// The shortcut capability state.
    pub fn capability_state(&self) -> CapabilityState {
        self.state.lock().capability
    }

    /// The error of the last failed registration, if any.
    pub fn registration_error(&self) -> Option<ShortcutError> {
        self.state.lock().error.clone()
    }

    /// Why capture requests are currently refused, if they are: the feature is
    /// switched off or a recording is in progress.
    pub fn refusal_reason(&self) -> Option<&'static str> {
        let state = self.state.lock();
        if !state.preferences.enabled {
            Some("the capture shortcut is switched off")
        } else if state.preferences.recording {
            Some("a shortcut recording is in progress")
        } else {
            None
        }
    }

    /// User-driven setup action: opens the portal's configuration (Wayland)
    /// or retries the grab (X11). No consent is granted unattended.
    pub fn request_setup(&self) {
        self.state
            .lock()
            .setup_requests
            .retain(|subscriber| subscriber.unbounded_send(()).is_ok());
    }

    /// The stream of setup requests, consumed by the driver.
    pub fn setup_requests(&self) -> UnboundedReceiver<()> {
        let (sender, receiver) = unbounded();
        self.state.lock().setup_requests.push(sender);
        receiver
    }

    /// Records the result of applying the active shortcut to a registration.
    pub fn report_registration(
        &self,
        active: Option<&Shortcut>,
        result: Result<(), ShortcutError>,
    ) {
        let mut state = self.state.lock();
        match result {
            Ok(()) => {
                state.capability = if active.is_some() {
                    CapabilityState::Ready
                } else {
                    CapabilityState::Unavailable
                };
                state.error = None;
            }
            Err(error) => {
                log::warn!("global shortcut registration failed: {error}");
                state.capability = CapabilityState::SetupRequired;
                state.error = Some(error);
            }
        }
    }

    /// Ends all subscriptions; a running [`ShortcutDriver`] releases its
    /// registration and returns.
    pub fn shutdown(&self) {
        let mut state = self.state.lock();
        state.active_subscribers.clear();
        state.activation_subscribers.clear();
        state.setup_requests.clear();
    }
}

/// A live registration of the global shortcut with the window system.
pub trait ShortcutRegistration: Send {
    /// Makes `active` the registered shortcut: releases any previous
    /// registration first, then registers the new one. `None` releases
    /// everything.
    fn apply(&mut self, active: Option<Shortcut>) -> BoxFuture<'_, Result<(), ShortcutError>>;

    /// The user-driven setup action: the portal's configure request when a
    /// session exists (Wayland, version 2+), otherwise a retry of the current
    /// registration.
    fn setup(&mut self) -> BoxFuture<'_, Result<(), ShortcutError>>;

    /// Resolves when the registered shortcut was activated. Pends forever
    /// while nothing is registered.
    fn next_activation(&mut self) -> BoxFuture<'_, ()>;
}

/// Drives a [`ShortcutRegistration`] from a [`ShortcutService`].
pub struct ShortcutDriver<R> {
    service: ShortcutService,
    registration: R,
}

enum DriverEvent {
    Active(Option<Option<Shortcut>>),
    Setup(Option<()>),
    Activated,
}

impl<R: ShortcutRegistration> ShortcutDriver<R> {
    pub fn new(service: ShortcutService, registration: R) -> Self {
        Self {
            service,
            registration,
        }
    }

    /// Applies every change of the active shortcut, serves setup requests and
    /// forwards activations until [`ShortcutService::shutdown`]. The
    /// registration is released on exit.
    pub async fn run(mut self) {
        let mut active = self.service.subscribe();
        let mut setup = self.service.setup_requests();
        loop {
            let event = {
                let next_active = active.next().fuse();
                let next_setup = setup.next().fuse();
                let activated = self.registration.next_activation().fuse();
                futures::pin_mut!(next_active, next_setup, activated);
                select_biased! {
                    value = next_active => DriverEvent::Active(value),
                    value = next_setup => DriverEvent::Setup(value),
                    () = activated => DriverEvent::Activated,
                }
            };
            match event {
                DriverEvent::Active(Some(shortcut)) => {
                    let result = self.registration.apply(shortcut.clone()).await;
                    self.service.report_registration(shortcut.as_ref(), result);
                }
                DriverEvent::Setup(Some(())) => {
                    let result = self.registration.setup().await;
                    let current = self.service.active_shortcut();
                    self.service.report_registration(current.as_ref(), result);
                }
                DriverEvent::Activated => self.service.emit_activation(),
                DriverEvent::Active(None) | DriverEvent::Setup(None) => break,
            }
        }
        if let Err(error) = self.registration.apply(None).await {
            log::warn!("releasing the global shortcut failed: {error}");
        }
    }
}
