//! X11 global shortcut registration through passive key grabs on the root
//! window (PLT-012 R4).

use futures::{FutureExt as _, future::BoxFuture};

use super::{Shortcut, ShortcutError, ShortcutRegistration, modifier_mask};
use crate::capture::x11::X11Error;

/// The lock states in which the key is grabbed besides the base modifiers, so
/// caps lock and num lock do not defeat the shortcut: none, Lock, Mod2,
/// Lock + Mod2.
pub const LOCK_VARIANTS: [u16; 4] = [
    0,
    modifier_mask::LOCK,
    modifier_mask::MOD2,
    modifier_mask::LOCK | modifier_mask::MOD2,
];

/// Parameters of a passive grab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrabParameters {
    /// Whether the grabbing window's events are delivered normally.
    pub owner_events: bool,
    /// Asynchronous pointer and keyboard grab mode.
    pub asynchronous: bool,
}

/// The shortcut grab: owner events off, asynchronous mode.
pub const SHORTCUT_GRAB: GrabParameters = GrabParameters {
    owner_events: false,
    asynchronous: true,
};

/// A key press delivered to the grabbing client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPressEvent {
    pub keycode: u8,
    /// Modifier state including lock modifiers.
    pub state: u16,
}

/// Key grabbing on the root window of an X server.
pub trait X11KeyGrabber: Send + Sync {
    /// The first keycode in the keyboard mapping that contains `keysym`.
    fn keycode_for_keysym(&self, keysym: u32) -> Result<Option<u8>, X11Error>;
    /// Establishes one passive grab on the root window; an error when another
    /// client holds the key.
    fn grab_key(
        &self,
        keycode: u8,
        modifiers: u16,
        parameters: GrabParameters,
    ) -> Result<(), X11Error>;
    fn ungrab_key(&self, keycode: u8, modifiers: u16) -> Result<(), X11Error>;
    /// Discards pending events, including key presses of released grabs.
    fn drain_events(&self) -> Result<(), X11Error>;
    /// The next key press; `None` when the connection ended.
    fn next_key_press(&self) -> BoxFuture<'_, Option<KeyPressEvent>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Grab {
    keycode: u8,
    modifiers: u16,
}

/// [`ShortcutRegistration`] over passive key grabs.
pub struct X11ShortcutRegistration<G> {
    grabber: G,
    grab: Option<Grab>,
    current: Option<Shortcut>,
}

impl<G: X11KeyGrabber> X11ShortcutRegistration<G> {
    pub fn new(grabber: G) -> Self {
        Self {
            grabber,
            grab: None,
            current: None,
        }
    }

    fn release(&mut self) {
        let Some(grab) = self.grab.take() else {
            return;
        };
        for variant in LOCK_VARIANTS {
            if let Err(error) = self
                .grabber
                .ungrab_key(grab.keycode, grab.modifiers | variant)
            {
                log::warn!("releasing the key grab failed: {error}");
            }
        }
    }

    fn apply_sync(&mut self, active: Option<Shortcut>) -> Result<(), ShortcutError> {
        // Rebinding releases the old grab before establishing the new one and
        // drains pending events.
        self.release();
        if let Err(error) = self.grabber.drain_events() {
            log::warn!("draining X events failed: {error}");
        }
        self.current = active.clone();
        let Some(shortcut) = active else {
            return Ok(());
        };

        let keysym = shortcut.x11_keysym();
        let keycode = self
            .grabber
            .keycode_for_keysym(keysym)
            .map_err(|error| ShortcutError::Unavailable(error.to_string()))?
            .ok_or_else(|| {
                ShortcutError::Unavailable(format!(
                    "no key in the keyboard mapping produces keysym {keysym:#x} for {shortcut}"
                ))
            })?;
        let modifiers = shortcut.x11_modifier_mask();
        for variant in LOCK_VARIANTS {
            if let Err(error) = self
                .grabber
                .grab_key(keycode, modifiers | variant, SHORTCUT_GRAB)
            {
                // Another client holds the key: release every grab of it.
                for variant in LOCK_VARIANTS {
                    let _ = self.grabber.ungrab_key(keycode, modifiers | variant);
                }
                return Err(ShortcutError::Unavailable(format!(
                    "{shortcut} is held by another client: {error}"
                )));
            }
        }
        self.grab = Some(Grab { keycode, modifiers });
        Ok(())
    }
}

impl<G: X11KeyGrabber> ShortcutRegistration for X11ShortcutRegistration<G> {
    fn apply(&mut self, active: Option<Shortcut>) -> BoxFuture<'_, Result<(), ShortcutError>> {
        async move { self.apply_sync(active) }.boxed()
    }

    /// X11 setup is a retry of the current registration.
    fn setup(&mut self) -> BoxFuture<'_, Result<(), ShortcutError>> {
        let current = self.current.clone();
        async move { self.apply_sync(current) }.boxed()
    }

    fn next_activation(&mut self) -> BoxFuture<'_, ()> {
        async move {
            let Some(grab) = self.grab.clone() else {
                return std::future::pending().await;
            };
            let relevant = !(modifier_mask::LOCK | modifier_mask::MOD2) & 0xff;
            loop {
                match self.grabber.next_key_press().await {
                    Some(event)
                        if event.keycode == grab.keycode
                            && event.state & relevant == grab.modifiers =>
                    {
                        return;
                    }
                    Some(_) => continue,
                    None => return std::future::pending().await,
                }
            }
        }
        .boxed()
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use real::X11rbKeyGrabber;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod real {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use futures::{
        StreamExt as _,
        channel::mpsc::{UnboundedReceiver, unbounded},
    };
    use parking_lot::Mutex;
    use rustix::event::{PollFd, PollFlags, poll};
    use x11rb::{
        connection::Connection as _,
        protocol::{
            Event,
            xproto::{ConnectionExt as _, GrabMode, ModMask, Window},
        },
        rust_connection::RustConnection,
    };

    use super::*;

    /// How long the event thread sleeps in `poll` before checking for
    /// shutdown. Key presses wake it immediately.
    const POLL_INTERVAL: Duration = Duration::from_millis(10);

    fn error(error: impl std::fmt::Display) -> X11Error {
        X11Error::new(error.to_string())
    }

    /// [`X11KeyGrabber`] over its own X11 connection with a thread that
    /// forwards key presses with minimal latency.
    pub struct X11rbKeyGrabber {
        connection: Arc<RustConnection>,
        root: Window,
        events: Mutex<UnboundedReceiver<KeyPressEvent>>,
        stop: Arc<AtomicBool>,
    }

    impl X11rbKeyGrabber {
        pub fn connect() -> Result<Self, X11Error> {
            let (connection, screen) = RustConnection::connect(None).map_err(error)?;
            let root = connection.setup().roots[screen].root;
            let connection = Arc::new(connection);
            let (sender, receiver) = unbounded();
            let stop = Arc::new(AtomicBool::new(false));
            std::thread::Builder::new()
                .name("x11-shortcut-events".into())
                .spawn({
                    let connection = connection.clone();
                    let stop = stop.clone();
                    move || {
                        while !stop.load(Ordering::Acquire) {
                            match connection.poll_for_event() {
                                Ok(Some(Event::KeyPress(press))) => {
                                    let event = KeyPressEvent {
                                        keycode: press.detail,
                                        state: u16::from(press.state),
                                    };
                                    if sender.unbounded_send(event).is_err() {
                                        break;
                                    }
                                }
                                Ok(Some(_)) => {}
                                Ok(None) => {
                                    let fd = rustix::fd::AsFd::as_fd(connection.stream());
                                    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                                    let timeout = rustix::event::Timespec {
                                        tv_sec: 0,
                                        tv_nsec: POLL_INTERVAL.subsec_nanos().into(),
                                    };
                                    let _ = poll(&mut fds, Some(&timeout));
                                }
                                Err(_) => break,
                            }
                        }
                    }
                })
                .map_err(error)?;
            Ok(Self {
                connection,
                root,
                events: Mutex::new(receiver),
                stop,
            })
        }
    }

    impl Drop for X11rbKeyGrabber {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
        }
    }

    impl X11KeyGrabber for X11rbKeyGrabber {
        fn keycode_for_keysym(&self, keysym: u32) -> Result<Option<u8>, X11Error> {
            let setup = self.connection.setup();
            let first = setup.min_keycode;
            let count = setup.max_keycode - first + 1;
            let mapping = self
                .connection
                .get_keyboard_mapping(first, count)
                .map_err(error)?
                .reply()
                .map_err(error)?;
            let per_keycode = usize::from(mapping.keysyms_per_keycode);
            if per_keycode == 0 {
                return Ok(None);
            }
            Ok(mapping
                .keysyms
                .chunks(per_keycode)
                .position(|keysyms| keysyms.contains(&keysym))
                .map(|index| first + index as u8))
        }

        fn grab_key(
            &self,
            keycode: u8,
            modifiers: u16,
            parameters: GrabParameters,
        ) -> Result<(), X11Error> {
            let mode = if parameters.asynchronous {
                GrabMode::ASYNC
            } else {
                GrabMode::SYNC
            };
            self.connection
                .grab_key(
                    parameters.owner_events,
                    self.root,
                    ModMask::from(modifiers),
                    keycode,
                    mode,
                    mode,
                )
                .map_err(error)?
                .check()
                .map_err(error)
        }

        fn ungrab_key(&self, keycode: u8, modifiers: u16) -> Result<(), X11Error> {
            self.connection
                .ungrab_key(keycode, self.root, ModMask::from(modifiers))
                .map_err(error)?
                .check()
                .map_err(error)
        }

        fn drain_events(&self) -> Result<(), X11Error> {
            // A round trip guarantees earlier events reached the client.
            self.connection
                .get_input_focus()
                .map_err(error)?
                .reply()
                .map_err(error)?;
            let mut events = self.events.lock();
            while events.try_recv().is_ok() {}
            Ok(())
        }

        fn next_key_press(&self) -> BoxFuture<'_, Option<KeyPressEvent>> {
            futures::future::poll_fn(move |cx| self.events.lock().poll_next_unpin(cx)).boxed()
        }
    }
}
