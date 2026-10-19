//! Unified application automation and strict locators (DEV-007).
//!
//! One validated contract serves in-process tests and live applications. A scenario written
//! against [`Automation`] runs unchanged against an in-process [`TestHost`](crate::TestHost)
//! ([`Automation::connect_in_process`]) and against a launched application speaking the protocol
//! over standard input and output ([`Automation::launch`], [`Automation::connect_session`]), and
//! both produce the same tree, input and clock results.
//!
//! * [`protocol`]: message shapes, the eight error codes, the method catalog (every request's
//!   parameters and every result are validated against it) and protocol version 1.
//! * [`sse`]: Server-Sent-Events framing: one JSON message per `data:` frame; no `id:`/`event:`
//!   fields; a decoder that reassembles frames split at arbitrary bytes and ignores log lines.
//! * [`host`]: [`AutomationHost`], the operations a host application provides, and the shared
//!   dispatcher; [`TestHost`](crate::TestHost) implements it.
//! * [`server`]: the server loop; a host application exposes the contract by serving its
//!   renderer through [`serve`] / [`serve_stdio`].
//! * [`session`]: the client connection: correlation by request id (replies may arrive in any
//!   order), cancellation, closing, launching a subprocess.
//! * [`client`]: [`Automation`], strict [`Locator`]s, [`Mouse`] gestures and the motion
//!   [`ClockControl`].
//!
//! # Scenario
//!
//! ```ignore
//! let host = Rc::new(RefCell::new(TestHost::with_tree(options, tree)?));
//! let app = Automation::connect_in_process(host.clone())?;
//! host.borrow_mut().apply_batch(batch)?;
//! app.get_by_test_id("name").fill("Ada")?;
//! app.get_by_text("Save").click()?;
//! let tree = app.tree()?;
//! app.clock().set(2000.0)?;
//! ```

pub mod client;
pub mod host;
pub mod protocol;
pub mod server;
pub mod session;
pub mod sse;

pub use client::{
    Automation, Backend, ClockControl, DEFAULT_DRAG_STEPS, DEFAULT_WAIT_TIMEOUT_MS, DragOptions,
    InProcessBackend, Locator, Mouse, POLL_INTERVAL_MS, SessionBackend, Target,
};
pub use host::{AutomationHost, dispatch};
pub use protocol::{AutomationError, Capability, ErrorCode, Message, PROTOCOL_VERSION};
pub use server::{serve, serve_stdio};
pub use session::{InitInfo, LaunchOptions, PendingCall, Session, SessionOptions};
