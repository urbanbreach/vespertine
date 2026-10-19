//! Time source abstraction so the query deadline can be driven by an injected
//! clock in tests.

use futures::future::{BoxFuture, FutureExt};
use std::time::{Duration, Instant};

/// A monotonic clock plus a timer, injectable so that budgets are testable
/// without sleeping.
pub trait ClockSource: Send + Sync + 'static {
    /// The current monotonic time.
    fn now(&self) -> Instant;

    /// A future that completes once `duration` has elapsed on this clock. The
    /// future owns no locks; dropping it cancels the timer.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}

/// The real wall-clock source backed by the operating system's monotonic
/// clock and an OS timer.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl ClockSource for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    #[allow(clippy::disallowed_methods)]
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        async move {
            smol::Timer::after(duration).await;
        }
        .boxed()
    }
}
