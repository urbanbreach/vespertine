//! The command channel to the UI thread (APP-005 R2, R3).
//!
//! Commands are queued and executed on the UI thread in submission order. A command that
//! needs a result waits for it with a bounded timeout instead of hanging.

use std::sync::mpsc;
use std::time::Duration;

use crate::error::HostError;

/// How long a result-returning command waits for the UI thread.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// The sending half of a one-shot reply to a result-returning command.
pub struct Reply<T>(mpsc::SyncSender<T>);

impl<T> Reply<T> {
    /// Answers the command. The caller may already have given up; that is not an error.
    pub fn send(self, value: T) {
        let _ = self.0.send(value);
    }
}

/// The caller's side of the command queue. `C` is the command type the UI thread executes.
pub struct UiLink<C> {
    queue: async_channel::Sender<C>,
    timeout: Duration,
}

impl<C> Clone for UiLink<C> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            timeout: self.timeout,
        }
    }
}

impl<C> UiLink<C> {
    /// Creates a link and the receiver the UI loop drains, with the standard 2 s timeout.
    pub fn channel() -> (Self, async_channel::Receiver<C>) {
        Self::channel_with_timeout(RESPONSE_TIMEOUT)
    }

    /// Like [`UiLink::channel`] with a different response timeout (tests only need it short).
    pub fn channel_with_timeout(timeout: Duration) -> (Self, async_channel::Receiver<C>) {
        let (queue, receiver) = async_channel::unbounded();
        (Self { queue, timeout }, receiver)
    }

    /// Queues a fire-and-forget command. Fails when the UI loop is gone.
    pub fn send(&self, command: C) -> Result<(), HostError> {
        self.queue
            .try_send(command)
            .map_err(|_| HostError::NotRunning)
    }

    /// Queues a command built around a [`Reply`] and waits for the answer: it fails with a
    /// timeout error when none arrives in time and with a stopped error when the UI loop
    /// dropped the command unanswered.
    pub fn request<T>(
        &self,
        operation: &'static str,
        build: impl FnOnce(Reply<T>) -> C,
    ) -> Result<T, HostError> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.send(build(Reply(reply)))?;
        answer
            .recv_timeout(self.timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => HostError::TimedOut {
                    operation,
                    after: self.timeout,
                },
                mpsc::RecvTimeoutError::Disconnected => HostError::Stopped(operation),
            })
    }
}
