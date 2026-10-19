//! Real Linux backends of the permission status service: the Screenshot
//! portal, the AT-SPI status bus, and the sound server plus the Device portal.

use super::{
    AccessibilityStatusBackend, CaptureAvailability, DesktopCaptureStatusSource, MicrophoneBackend,
    MicrophoneDevices, PermissionError, pulse,
};
use crate::context::zbus_bus;
use futures::{
    StreamExt,
    future::{BoxFuture, FutureExt},
};
use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use zbus::zvariant::{OwnedValue, Value};

fn failed(error: impl std::fmt::Display) -> PermissionError {
    PermissionError::Failed(error.to_string())
}

/// Maps a zbus error to a permission error: an `AccessDenied` or
/// `PermissionDenied` D-Bus error is a denial, anything else a failure.
fn map_zbus_error(error: zbus::Error) -> PermissionError {
    if let zbus::Error::MethodError(name, description, _) = &error {
        let name = name.as_str();
        if name.ends_with("AccessDenied") || name.ends_with("PermissionDenied") {
            return PermissionError::Denied(description.clone().unwrap_or_else(|| name.to_owned()));
        }
    }
    failed(error)
}

/// The Screenshot portal as a capture capability source: unavailable when the
/// portal is absent, older than version 3 or lacks both the active-window and
/// window targets; ready with the active-window target; user selection with
/// only the window target. It sends no capture request and shows nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct PortalCaptureStatusSource;

impl DesktopCaptureStatusSource for PortalCaptureStatusSource {
    fn availability(&self) -> BoxFuture<'_, Result<CaptureAvailability, PermissionError>> {
        async {
            const PORTAL: &str = "org.freedesktop.portal.Desktop";
            const PATH: &str = "/org/freedesktop/portal/desktop";
            const INTERFACE: &str = "org.freedesktop.portal.Screenshot";
            // `AvailableTargets` bits: screen 1, window 2, area 4, active window 8.
            const TARGET_WINDOW: u32 = 2;
            const TARGET_ACTIVE_WINDOW: u32 = 8;

            let Ok(session) = zbus_bus::session_connection().await else {
                return Ok(CaptureAvailability::Unavailable);
            };
            let version = zbus_bus::get_property(&session, PORTAL, PATH, INTERFACE, "version")
                .await
                .ok()
                .and_then(|value| u32::try_from(value).ok());
            if version.is_none_or(|version| version < 3) {
                return Ok(CaptureAvailability::Unavailable);
            }
            let targets =
                zbus_bus::get_property(&session, PORTAL, PATH, INTERFACE, "AvailableTargets")
                    .await
                    .map_err(failed)?;
            let targets = u32::try_from(targets).map_err(failed)?;
            Ok(if targets & TARGET_ACTIVE_WINDOW != 0 {
                CaptureAvailability::Ready
            } else if targets & TARGET_WINDOW != 0 {
                CaptureAvailability::UserSelection
            } else {
                CaptureAvailability::Unavailable
            })
        }
        .boxed()
    }
}

/// The AT-SPI accessibility bus: `org.a11y.Bus` and `org.a11y.Status` on the
/// session bus and the registry root on the accessibility bus.
#[derive(Clone, Copy, Debug, Default)]
pub struct AtspiStatusBackend;

impl AccessibilityStatusBackend for AtspiStatusBackend {
    fn bus_reachable(&self) -> BoxFuture<'_, Result<(), PermissionError>> {
        async {
            let session = zbus_bus::session_connection().await.map_err(failed)?;
            zbus_bus::accessibility_bus_address(&session)
                .await
                .map(|_| ())
                .map_err(failed)
        }
        .boxed()
    }

    fn registry_answers(&self) -> BoxFuture<'_, bool> {
        async {
            let Ok(connection) = zbus_bus::connect_accessibility_bus().await else {
                return false;
            };
            zbus_bus::get_property(
                &connection,
                "org.a11y.atspi.Registry",
                "/org/a11y/atspi/accessible/root",
                "org.a11y.atspi.Accessible",
                "ChildCount",
            )
            .await
            .is_ok()
        }
        .boxed()
    }

    fn is_enabled(&self) -> BoxFuture<'_, Result<bool, PermissionError>> {
        async {
            let session = zbus_bus::session_connection().await.map_err(failed)?;
            let value = zbus_bus::get_property(
                &session,
                "org.a11y.Bus",
                "/org/a11y/bus",
                "org.a11y.Status",
                "IsEnabled",
            )
            .await
            .map_err(failed)?;
            bool::try_from(value).map_err(failed)
        }
        .boxed()
    }

    fn set_enabled(&self, enabled: bool) -> BoxFuture<'_, Result<(), PermissionError>> {
        async move {
            let session = zbus_bus::session_connection().await.map_err(failed)?;
            session
                .call_method(
                    Some("org.a11y.Bus"),
                    "/org/a11y/bus",
                    Some("org.freedesktop.DBus.Properties"),
                    "Set",
                    &("org.a11y.Status", "IsEnabled", Value::from(enabled)),
                )
                .await
                .map(|_| ())
                .map_err(map_zbus_error)
        }
        .boxed()
    }
}

/// Whether the process runs in a Flatpak (`/.flatpak-info`) or Snap
/// (`SNAP` environment variable) sandbox, where the Device portal mediates
/// device access.
pub fn detect_sandbox() -> bool {
    Path::new("/.flatpak-info").exists() || std::env::var_os("SNAP").is_some()
}

/// Microphone status from the sound server (see [`pulse`]) and the Device
/// portal request hook (issued only when sandboxed).
#[derive(Clone, Copy, Debug)]
pub struct SoundServerMicrophone {
    sandboxed: bool,
}

impl SoundServerMicrophone {
    /// A backend with an explicit sandbox setting.
    pub fn new(sandboxed: bool) -> Self {
        Self { sandboxed }
    }
}

impl Default for SoundServerMicrophone {
    fn default() -> Self {
        Self::new(detect_sandbox())
    }
}

impl MicrophoneBackend for SoundServerMicrophone {
    fn sandboxed(&self) -> bool {
        self.sandboxed
    }

    fn devices(&self) -> BoxFuture<'_, Result<MicrophoneDevices, PermissionError>> {
        pulse::probe_microphone_devices().boxed()
    }

    fn request_device_access(&self) -> BoxFuture<'_, Result<(), PermissionError>> {
        access_microphone_through_portal().boxed()
    }
}

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Calls `org.freedesktop.portal.Device.AccessDevice` for `microphone` and
/// waits for the portal's `Response`: 0 granted, 1 the user cancelled, 2 the
/// request ended otherwise, which for this portal is a refusal by the user or
/// the permission store.
async fn access_microphone_through_portal() -> Result<(), PermissionError> {
    const DESKTOP: &str = "org.freedesktop.portal.Desktop";
    let connection = zbus_bus::session_connection().await.map_err(failed)?;
    let token = format!(
        "vespertine_microphone_{}_{}",
        std::process::id(),
        REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let sender = connection
        .unique_name()
        .ok_or_else(|| failed("session bus connection has no unique name"))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let request = zbus::Proxy::new(
        &connection,
        DESKTOP,
        request_path,
        "org.freedesktop.portal.Request",
    )
    .await
    .map_err(failed)?;
    // Subscribe before the call so the response cannot be missed.
    let mut responses = request.receive_signal("Response").await.map_err(failed)?;
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    connection
        .call_method(
            Some(DESKTOP),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Device"),
            "AccessDevice",
            &(std::process::id(), vec!["microphone"], options),
        )
        .await
        .map_err(map_zbus_error)?;
    let message = responses
        .next()
        .await
        .ok_or_else(|| failed("portal closed the request without a response"))?;
    let (code, _results) = message
        .body()
        .deserialize::<(u32, HashMap<String, OwnedValue>)>()
        .map_err(failed)?;
    match code {
        0 => Ok(()),
        1 => Err(PermissionError::Cancelled),
        _ => Err(PermissionError::Denied(
            "the Device portal refused microphone access".to_owned(),
        )),
    }
}
