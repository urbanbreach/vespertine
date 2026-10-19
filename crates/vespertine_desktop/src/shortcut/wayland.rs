//! Wayland global shortcut registration through the GlobalShortcuts portal
//! (PLT-012 R4).

use futures::{FutureExt as _, future::BoxFuture};

use super::{Shortcut, ShortcutError, ShortcutRegistration};
use crate::capture::portal::PortalError;

/// Fixed id of the one shortcut bound through the portal.
pub const SHORTCUT_ID: &str = "capture-window";
/// Human description shown by the portal.
pub const SHORTCUT_DESCRIPTION: &str = "Capture the active window";
/// Interface version from which the configure request exists.
pub const CONFIGURE_MIN_VERSION: u32 = 2;

/// The one shortcut bound in a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalShortcut {
    pub id: String,
    pub description: String,
    /// The preferred trigger in XDG shortcut syntax.
    pub preferred_trigger: String,
}

/// The GlobalShortcuts portal.
pub trait GlobalShortcutsPortal: Send + Sync {
    /// The interface version; an error when the portal is missing.
    fn version(&self) -> BoxFuture<'_, Result<u32, PortalError>>;
    /// Creates a session.
    fn create_session(&self) -> BoxFuture<'_, Result<Box<dyn ShortcutSession>, PortalError>>;
}

/// One GlobalShortcuts portal session.
pub trait ShortcutSession: Send {
    /// Binds the shortcut. An error means the portal rejected the bind.
    fn bind(&mut self, shortcut: &PortalShortcut) -> BoxFuture<'_, Result<(), PortalError>>;
    /// Issues the configure-shortcuts request (interface version 2+).
    fn configure(&mut self) -> BoxFuture<'_, Result<(), PortalError>>;
    /// The id of the next `Activated` signal; `None` when the signal stream
    /// ended.
    fn next_activated(&mut self) -> BoxFuture<'_, Option<String>>;
    /// Closes the session explicitly. Dropping the proxy does not close it.
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), PortalError>>;
}

/// [`ShortcutRegistration`] over a GlobalShortcuts portal.
pub struct WaylandShortcutRegistration {
    portal: Box<dyn GlobalShortcutsPortal>,
    session: Option<Box<dyn ShortcutSession>>,
    version: u32,
    /// The combination of the most recent successful bind.
    last_bound: Option<Shortcut>,
    current: Option<Shortcut>,
}

impl WaylandShortcutRegistration {
    pub fn new(portal: Box<dyn GlobalShortcutsPortal>) -> Self {
        Self {
            portal,
            session: None,
            version: 0,
            last_bound: None,
            current: None,
        }
    }

    async fn close_session(&mut self) {
        if let Some(session) = self.session.take()
            && let Err(error) = session.close().await
        {
            log::warn!("closing the global shortcuts session failed: {error}");
        }
    }

    async fn establish(&mut self, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        let setup_required = |error: PortalError| ShortcutError::SetupRequired(error.to_string());
        self.version = self.portal.version().await.map_err(setup_required)?;
        let mut session = self.portal.create_session().await.map_err(setup_required)?;
        let bind = PortalShortcut {
            id: SHORTCUT_ID.to_owned(),
            description: SHORTCUT_DESCRIPTION.to_owned(),
            preferred_trigger: shortcut.portal_trigger(),
        };
        if let Err(error) = session.bind(&bind).await {
            // A session is always closed, also when the bind was refused.
            if let Err(close_error) = session.close().await {
                log::warn!("closing the global shortcuts session failed: {close_error}");
            }
            return Err(setup_required(error));
        }
        // The portal remembers earlier bindings: let the user confirm the new
        // trigger in the system UI when a different combination was bound.
        let rebinding = self
            .last_bound
            .as_ref()
            .is_some_and(|previous| previous != shortcut);
        if rebinding && self.version >= CONFIGURE_MIN_VERSION {
            if let Err(error) = session.configure().await {
                log::warn!("the configure-shortcuts request failed: {error}");
            }
        }
        self.last_bound = Some(shortcut.clone());
        self.session = Some(session);
        Ok(())
    }

    async fn apply_inner(&mut self, active: Option<Shortcut>) -> Result<(), ShortcutError> {
        // The old session is closed explicitly before a new one is created.
        self.close_session().await;
        self.current = active.clone();
        match active {
            Some(shortcut) => self.establish(&shortcut).await,
            None => Ok(()),
        }
    }

    async fn setup_inner(&mut self) -> Result<(), ShortcutError> {
        if self.version >= CONFIGURE_MIN_VERSION
            && let Some(session) = self.session.as_mut()
        {
            return session
                .configure()
                .await
                .map_err(|error| ShortcutError::SetupRequired(error.to_string()));
        }
        let current = self.current.clone();
        self.apply_inner(current).await
    }
}

impl ShortcutRegistration for WaylandShortcutRegistration {
    fn apply(&mut self, active: Option<Shortcut>) -> BoxFuture<'_, Result<(), ShortcutError>> {
        self.apply_inner(active).boxed()
    }

    fn setup(&mut self) -> BoxFuture<'_, Result<(), ShortcutError>> {
        self.setup_inner().boxed()
    }

    fn next_activation(&mut self) -> BoxFuture<'_, ()> {
        async move {
            let Some(session) = self.session.as_mut() else {
                return std::future::pending().await;
            };
            loop {
                match session.next_activated().await {
                    // Only the application's own shortcut id activates.
                    Some(id) if id == SHORTCUT_ID => return,
                    Some(_) => continue,
                    None => return std::future::pending().await,
                }
            }
        }
        .boxed()
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use portal::AshpdGlobalShortcuts;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod portal {
    use std::sync::Arc;

    use ashpd::{
        Error,
        desktop::{
            CreateSessionOptions, ResponseError, Session,
            global_shortcuts::{
                Activated, BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts,
                NewShortcut,
            },
        },
    };
    use futures::StreamExt as _;
    use zbus::proxy::SignalStream;

    use super::*;

    fn map_error(error: Error) -> PortalError {
        match error {
            Error::Response(ResponseError::Cancelled) => PortalError::Cancelled,
            Error::Portal(ashpd::PortalError::Cancelled(_)) => PortalError::Cancelled,
            Error::Portal(ashpd::PortalError::NotAllowed(reason)) => PortalError::Denied(reason),
            Error::PortalNotFound(interface) => {
                PortalError::Unreachable(format!("the portal does not implement {interface}"))
            }
            other => PortalError::Failed(other.to_string()),
        }
    }

    /// GlobalShortcuts portal over the session bus (via `ashpd`).
    pub struct AshpdGlobalShortcuts {
        portal: Arc<GlobalShortcuts>,
    }

    impl AshpdGlobalShortcuts {
        pub async fn connect() -> Result<Self, PortalError> {
            let portal = GlobalShortcuts::new().await.map_err(map_error)?;
            Ok(Self {
                portal: Arc::new(portal),
            })
        }
    }

    impl GlobalShortcutsPortal for AshpdGlobalShortcuts {
        fn version(&self) -> BoxFuture<'_, Result<u32, PortalError>> {
            async move { Ok(self.portal.version()) }.boxed()
        }

        fn create_session(&self) -> BoxFuture<'_, Result<Box<dyn ShortcutSession>, PortalError>> {
            async move {
                // Subscribe before the bind so no activation can be missed.
                let activated = self
                    .portal
                    .receive_signal("Activated")
                    .await
                    .map_err(|error| PortalError::Failed(error.to_string()))?;
                let session = self
                    .portal
                    .create_session(CreateSessionOptions::default())
                    .await
                    .map_err(map_error)?;
                Ok(Box::new(AshpdSession {
                    portal: self.portal.clone(),
                    session,
                    activated,
                }) as Box<dyn ShortcutSession>)
            }
            .boxed()
        }
    }

    struct AshpdSession {
        portal: Arc<GlobalShortcuts>,
        session: Session<GlobalShortcuts>,
        activated: SignalStream<'static>,
    }

    impl ShortcutSession for AshpdSession {
        fn bind(&mut self, shortcut: &PortalShortcut) -> BoxFuture<'_, Result<(), PortalError>> {
            let new = NewShortcut::new(shortcut.id.clone(), shortcut.description.clone())
                .preferred_trigger(shortcut.preferred_trigger.as_str());
            let id = shortcut.id.clone();
            async move {
                let request = self
                    .portal
                    .bind_shortcuts(&self.session, &[new], None, BindShortcutsOptions::default())
                    .await
                    .map_err(map_error)?;
                let bound = request.response().map_err(map_error)?;
                if bound.shortcuts().iter().any(|bound| bound.id() == id) {
                    Ok(())
                } else {
                    Err(PortalError::Denied(
                        "the portal did not bind the shortcut".into(),
                    ))
                }
            }
            .boxed()
        }

        fn configure(&mut self) -> BoxFuture<'_, Result<(), PortalError>> {
            async move {
                self.portal
                    .configure_shortcuts(&self.session, None, ConfigureShortcutsOptions::default())
                    .await
                    .map_err(map_error)
            }
            .boxed()
        }

        fn next_activated(&mut self) -> BoxFuture<'_, Option<String>> {
            async move {
                loop {
                    let message = self.activated.next().await?;
                    if let Ok(activated) = message.body().deserialize::<Activated>() {
                        return Some(activated.shortcut_id().to_owned());
                    }
                }
            }
            .boxed()
        }

        fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), PortalError>> {
            async move { self.session.close().await.map_err(map_error) }.boxed()
        }
    }
}
