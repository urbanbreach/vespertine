//! Screenshot portal boundary (PLT-012 R2).
//!
//! [`ScreenshotPortal`] is the injectable boundary; [`ZbusScreenshotPortal`]
//! is the session-bus implementation. A request can only name a window
//! target ([`WindowTarget`]); the full-screen target of the portal is not
//! representable, so a full-screen or default-target request can never be
//! issued by construction.

use futures::future::BoxFuture;
use thiserror::Error;

/// Minimum Screenshot portal interface version for any window target.
pub const MIN_SCREENSHOT_VERSION: u32 = 3;

/// Bitmask of screenshot targets advertised by the portal's
/// `AvailableTargets` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PortalTargets(pub u32);

impl PortalTargets {
    pub const SCREEN: u32 = 1;
    pub const WINDOW: u32 = 2;
    pub const AREA: u32 = 4;
    pub const ACTIVE_WINDOW: u32 = 8;

    pub fn contains(self, target: WindowTarget) -> bool {
        self.0 & target.bit() != 0
    }
}

/// A window-only screenshot target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowTarget {
    /// A window selected by the user in the portal's own picker (interactive
    /// request).
    Window,
    /// The currently active window (non-interactive request).
    ActiveWindow,
}

impl WindowTarget {
    /// The portal's `target` option value (a single value, not a mask).
    pub fn bit(self) -> u32 {
        match self {
            WindowTarget::Window => PortalTargets::WINDOW,
            WindowTarget::ActiveWindow => PortalTargets::ACTIVE_WINDOW,
        }
    }

    /// Whether the portal must show its own UI for this target.
    pub fn interactive(self) -> bool {
        matches!(self, WindowTarget::Window)
    }
}

/// Result of probing the portal: interface version and advertised targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalProbe {
    pub version: u32,
    pub targets: PortalTargets,
}

impl PortalProbe {
    /// The window target this portal can serve, preferring the active window.
    /// `None` when the version is below 3 or no window target is advertised.
    pub fn window_target(&self) -> Option<WindowTarget> {
        if self.version < MIN_SCREENSHOT_VERSION {
            None
        } else if self.targets.contains(WindowTarget::ActiveWindow) {
            Some(WindowTarget::ActiveWindow)
        } else if self.targets.contains(WindowTarget::Window) {
            Some(WindowTarget::Window)
        } else {
            None
        }
    }

    /// Whether `target` is served: version at least 3 and target advertised.
    pub fn supports(&self, target: WindowTarget) -> bool {
        self.version >= MIN_SCREENSHOT_VERSION && self.targets.contains(target)
    }
}

/// A window-only Screenshot request. Always sent non-modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenshotRequest {
    pub target: WindowTarget,
    pub interactive: bool,
}

impl ScreenshotRequest {
    pub fn new(target: WindowTarget) -> Self {
        Self {
            target,
            interactive: target.interactive(),
        }
    }
}

/// Portal failure, preserving the cancelled/denied distinction.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortalError {
    /// The user dismissed the consent dialog or picker.
    #[error("the portal request was cancelled")]
    Cancelled,
    /// The portal or permission store refused the request.
    #[error("the portal refused the request: {0}")]
    Denied(String),
    /// The portal could not be reached or does not implement the interface.
    #[error("the desktop portal is unreachable: {0}")]
    Unreachable(String),
    /// Anything else.
    #[error("{0}")]
    Failed(String),
}

/// Maps a portal Request `Response` code: 0 success, 1 cancelled, 2 and
/// everything else other.
pub fn map_response_code(code: u32) -> Result<(), PortalError> {
    match code {
        0 => Ok(()),
        1 => Err(PortalError::Cancelled),
        2 => Err(PortalError::Failed(
            "the portal request ended with an error (response code 2)".into(),
        )),
        other => Err(PortalError::Failed(format!(
            "the portal request ended with unknown response code {other}"
        ))),
    }
}

/// Maps a D-Bus error name (and message) to a portal error: Cancelled errors
/// map to cancelled, NotAllowed/AccessDenied-style errors to denied,
/// everything else to failed.
pub fn map_dbus_error(name: &str, message: &str) -> PortalError {
    let last = name.rsplit('.').next().unwrap_or(name);
    match last {
        "Cancelled" => PortalError::Cancelled,
        "NotAllowed" | "AccessDenied" | "PermissionDenied" => {
            PortalError::Denied(if message.is_empty() {
                name.to_owned()
            } else {
                message.to_owned()
            })
        }
        _ => PortalError::Failed(if message.is_empty() {
            name.to_owned()
        } else {
            format!("{name}: {message}")
        }),
    }
}

/// The Screenshot portal.
pub trait ScreenshotPortal: Send + Sync {
    /// Reads the interface version and, for version 3 or newer, the
    /// available-targets set. An error means the portal cannot be reached.
    fn probe(&self) -> BoxFuture<'_, Result<PortalProbe, PortalError>>;

    /// Sends one non-modal Screenshot request and resolves with the response
    /// URI string.
    fn screenshot(&self, request: ScreenshotRequest) -> BoxFuture<'_, Result<String, PortalError>>;
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use zbus_portal::ZbusScreenshotPortal;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod zbus_portal {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicU64, Ordering},
    };

    use futures::{FutureExt as _, StreamExt as _};
    use zbus::{
        Connection,
        zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
    };

    use super::*;

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
    const SCREENSHOT_INTERFACE: &str = "org.freedesktop.portal.Screenshot";
    const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";

    static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Screenshot portal over the session bus.
    pub struct ZbusScreenshotPortal {
        connection: Connection,
    }

    impl ZbusScreenshotPortal {
        pub async fn connect() -> Result<Self, PortalError> {
            let connection = Connection::session()
                .await
                .map_err(|error| PortalError::Unreachable(error.to_string()))?;
            Ok(Self { connection })
        }

        pub fn with_connection(connection: Connection) -> Self {
            Self { connection }
        }

        async fn screenshot_proxy(&self) -> Result<zbus::Proxy<'static>, PortalError> {
            zbus::Proxy::new(
                &self.connection,
                DESTINATION,
                OBJECT_PATH,
                SCREENSHOT_INTERFACE,
            )
            .await
            .map_err(|error| PortalError::Unreachable(error.to_string()))
        }

        async fn do_probe(&self) -> Result<PortalProbe, PortalError> {
            let proxy = self.screenshot_proxy().await?;
            let version: u32 = proxy
                .get_property("version")
                .await
                .map_err(|error| PortalError::Unreachable(error.to_string()))?;
            let targets = if version >= MIN_SCREENSHOT_VERSION {
                let mask: u32 = proxy
                    .get_property("AvailableTargets")
                    .await
                    .map_err(|error| PortalError::Unreachable(error.to_string()))?;
                PortalTargets(mask)
            } else {
                PortalTargets(0)
            };
            Ok(PortalProbe { version, targets })
        }

        async fn do_screenshot(&self, request: ScreenshotRequest) -> Result<String, PortalError> {
            let proxy = self.screenshot_proxy().await?;
            let sender = self
                .connection
                .unique_name()
                .ok_or_else(|| PortalError::Failed("the bus connection has no unique name".into()))?
                .as_str()
                .trim_start_matches(':')
                .replace('.', "_");
            let token = format!(
                "vespertine_capture_{}_{}",
                std::process::id(),
                REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
            let request_proxy = zbus::Proxy::new(
                &self.connection,
                DESTINATION,
                ObjectPath::try_from(request_path.as_str())
                    .map_err(|error| PortalError::Failed(error.to_string()))?,
                REQUEST_INTERFACE,
            )
            .await
            .map_err(|error| PortalError::Failed(error.to_string()))?;
            // Subscribe before calling so the response cannot be missed.
            let mut responses = request_proxy
                .receive_signal("Response")
                .await
                .map_err(|error| PortalError::Failed(error.to_string()))?;

            let mut options: HashMap<&str, Value<'_>> = HashMap::new();
            options.insert("handle_token", Value::from(token.as_str()));
            options.insert("modal", Value::from(false));
            options.insert("interactive", Value::from(request.interactive));
            options.insert("target", Value::from(request.target.bit()));

            let handle: OwnedObjectPath = proxy
                .call("Screenshot", &("", options))
                .await
                .map_err(map_zbus_error)?;
            debug_assert_eq!(handle.as_str(), request_path);

            let message = responses
                .next()
                .await
                .ok_or_else(|| PortalError::Failed("the portal request ended silently".into()))?;
            let (code, results): (u32, HashMap<String, OwnedValue>) = message
                .body()
                .deserialize()
                .map_err(|error| PortalError::Failed(error.to_string()))?;
            map_response_code(code)?;
            let uri = results
                .get("uri")
                .and_then(|value| <&str>::try_from(&**value).ok())
                .ok_or_else(|| PortalError::Failed("the portal response has no uri".into()))?;
            Ok(uri.to_owned())
        }
    }

    fn map_zbus_error(error: zbus::Error) -> PortalError {
        match &error {
            zbus::Error::MethodError(name, message, _) => {
                map_dbus_error(name.as_str(), message.as_deref().unwrap_or_default())
            }
            other => PortalError::Failed(other.to_string()),
        }
    }

    impl ScreenshotPortal for ZbusScreenshotPortal {
        fn probe(&self) -> BoxFuture<'_, Result<PortalProbe, PortalError>> {
            self.do_probe().boxed()
        }

        fn screenshot(
            &self,
            request: ScreenshotRequest,
        ) -> BoxFuture<'_, Result<String, PortalError>> {
            self.do_screenshot(request).boxed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_codes_map_to_distinct_outcomes() {
        assert_eq!(map_response_code(0), Ok(()));
        assert_eq!(map_response_code(1), Err(PortalError::Cancelled));
        assert!(matches!(map_response_code(2), Err(PortalError::Failed(_))));
        assert!(matches!(map_response_code(7), Err(PortalError::Failed(_))));
    }

    #[test]
    fn dbus_errors_keep_the_cancelled_denied_distinction() {
        assert_eq!(
            map_dbus_error("org.freedesktop.DBus.Error.Cancelled", ""),
            PortalError::Cancelled
        );
        assert_eq!(
            map_dbus_error("org.freedesktop.portal.Error.Cancelled", "x"),
            PortalError::Cancelled
        );
        assert!(matches!(
            map_dbus_error("org.freedesktop.portal.Error.NotAllowed", "no"),
            PortalError::Denied(_)
        ));
        assert!(matches!(
            map_dbus_error("org.freedesktop.DBus.Error.AccessDenied", ""),
            PortalError::Denied(_)
        ));
        assert!(matches!(
            map_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "m"),
            PortalError::Failed(_)
        ));
    }

    #[test]
    fn probe_prefers_active_window_and_requires_version_three() {
        let both = PortalProbe {
            version: 3,
            targets: PortalTargets(PortalTargets::WINDOW | PortalTargets::ACTIVE_WINDOW),
        };
        assert_eq!(both.window_target(), Some(WindowTarget::ActiveWindow));
        let window_only = PortalProbe {
            version: 3,
            targets: PortalTargets(PortalTargets::SCREEN | PortalTargets::WINDOW),
        };
        assert_eq!(window_only.window_target(), Some(WindowTarget::Window));
        let screen_only = PortalProbe {
            version: 3,
            targets: PortalTargets(PortalTargets::SCREEN | PortalTargets::AREA),
        };
        assert_eq!(screen_only.window_target(), None);
        let old = PortalProbe {
            version: 2,
            targets: PortalTargets(PortalTargets::WINDOW | PortalTargets::ACTIVE_WINDOW),
        };
        assert_eq!(old.window_target(), None);
        assert!(!old.supports(WindowTarget::Window));
    }

    #[test]
    fn interactive_flag_follows_the_target() {
        assert!(ScreenshotRequest::new(WindowTarget::Window).interactive);
        assert!(!ScreenshotRequest::new(WindowTarget::ActiveWindow).interactive);
    }
}
