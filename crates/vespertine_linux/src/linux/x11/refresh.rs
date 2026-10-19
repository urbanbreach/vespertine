//! Demand-gated display refresh policy for X11 windows (ANM-001).
//!
//! A visible window runs a periodic refresh timer only while it has demand.
//! Once a frame completes without remaining demand the window pauses, which
//! disarms the timer; the next invalidation arms it again and the first frame
//! is delivered on the next event-loop turn. The policy is kept free of the X
//! connection and the event loop so it can be tested on its own.

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    time::Duration,
};

/// What the owner of a [`RefreshTimer`] must do to its event-loop source.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TimerAction<T> {
    /// Nothing to do.
    None,
    /// Start a periodic source with this refresh period (its first delivery is
    /// immediate) and report the registration with [`RefreshTimer::started`]. A
    /// start that fails is not reported, so the next arming retries it.
    Start(Duration),
    /// Remove this registration from the event loop.
    Stop(T),
}

/// The refresh timer of one window whose display refresh period is known.
/// `T` is the event-loop registration of a running source.
pub(crate) struct RefreshTimer<T> {
    refresh_rate: Duration,
    visible: bool,
    running: Option<T>,
}

impl<T> RefreshTimer<T> {
    /// A timer for a hidden window: nothing runs until it is shown.
    pub(crate) fn hidden(refresh_rate: Duration) -> Self {
        Self {
            refresh_rate,
            visible: false,
            running: None,
        }
    }

    /// The window was shown or hidden. Showing a window needs a frame, so it
    /// starts the timer; hiding it stops the timer.
    pub(crate) fn set_visible(&mut self, visible: bool) -> TimerAction<T> {
        self.visible = visible;
        if visible {
            self.start_action()
        } else {
            self.stop_action()
        }
    }

    /// Demand arose (invalidation, input, resize, expose, next-frame callback).
    /// A running timer already delivers on every refresh period, and a hidden
    /// window is not presented, so only a visible parked timer starts.
    pub(crate) fn arm(&mut self) -> TimerAction<T> {
        if self.visible {
            self.start_action()
        } else {
            TimerAction::None
        }
    }

    /// The window finished a frame without demand: disarm the timer.
    pub(crate) fn pause(&mut self) -> TimerAction<T> {
        self.stop_action()
    }

    /// Records the registration of a started source.
    pub(crate) fn started(&mut self, registration: T) {
        self.running = Some(registration);
    }

    /// Takes the running registration, if any, e.g. when the window is closed.
    pub(crate) fn take_running(&mut self) -> Option<T> {
        self.running.take()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    fn start_action(&self) -> TimerAction<T> {
        if self.running.is_some() {
            TimerAction::None
        } else {
            TimerAction::Start(self.refresh_rate)
        }
    }

    fn stop_action(&mut self) -> TimerAction<T> {
        match self.running.take() {
            Some(registration) => TimerAction::Stop(registration),
            None => TimerAction::None,
        }
    }
}

/// Defers a closure to a later event-loop turn.
pub(crate) type Defer = Rc<dyn Fn(Box<dyn FnOnce()>)>;

/// Applies `arm` to the state behind `target`. If the state is currently
/// borrowed (arming can happen anywhere in an application callback) the
/// restart is deferred to a later event-loop turn instead of failing or
/// blocking, and retried until the state can be borrowed. If the state is
/// gone, because the window or client was destroyed before the deferred
/// restart ran, the request is ignored.
pub(crate) fn arm_or_defer<S: 'static>(
    target: &Weak<RefCell<S>>,
    defer: &Defer,
    arm: &Rc<dyn Fn(&mut S)>,
) {
    let Some(state) = target.upgrade() else {
        return;
    };
    match state.try_borrow_mut() {
        Ok(mut state) => arm(&mut state),
        Err(_) => {
            let target = target.clone();
            let retry_defer = defer.clone();
            let arm = arm.clone();
            defer(Box::new(move || arm_or_defer(&target, &retry_defer, &arm)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: Duration = Duration::from_micros(16_667);

    /// ANM-001 A6 (X11): a visible window with demand runs the periodic timer,
    /// the timer is disarmed when parked, and demand rearms it.
    #[test]
    fn timer_is_disarmed_when_parked_and_rearmed_on_demand() {
        let mut timer = RefreshTimer::<u32>::hidden(PERIOD);
        assert_eq!(timer.arm(), TimerAction::None, "hidden windows stay off");

        assert_eq!(timer.set_visible(true), TimerAction::Start(PERIOD));
        timer.started(1);
        assert!(timer.is_running());

        // While demand remains the timer keeps running; arming is a no-op.
        assert_eq!(timer.arm(), TimerAction::None);
        assert_eq!(timer.arm(), TimerAction::None);

        assert_eq!(timer.pause(), TimerAction::Stop(1));
        assert!(!timer.is_running());
        assert_eq!(timer.pause(), TimerAction::None, "parked stays parked");

        assert_eq!(timer.arm(), TimerAction::Start(PERIOD));
        timer.started(2);
        assert_eq!(timer.arm(), TimerAction::None, "one source per wake");
        assert_eq!(timer.pause(), TimerAction::Stop(2));
    }

    #[test]
    fn hiding_stops_the_timer_and_showing_restarts_it() {
        let mut timer = RefreshTimer::<u32>::hidden(PERIOD);
        assert_eq!(timer.set_visible(true), TimerAction::Start(PERIOD));
        timer.started(7);
        assert_eq!(timer.set_visible(false), TimerAction::Stop(7));
        assert_eq!(timer.arm(), TimerAction::None);
        assert_eq!(timer.set_visible(true), TimerAction::Start(PERIOD));
    }

    /// ANM-001 R7: a source that failed to start is retried by the next
    /// arming.
    #[test]
    fn failed_start_is_retried_on_the_next_arming() {
        let mut timer = RefreshTimer::<u32>::hidden(PERIOD);
        assert_eq!(timer.set_visible(true), TimerAction::Start(PERIOD));
        // The start failed: `started` is never called.
        assert!(!timer.is_running());
        assert_eq!(timer.arm(), TimerAction::Start(PERIOD));
        timer.started(3);
        assert_eq!(timer.take_running(), Some(3));
    }

    struct Deferred(Rc<RefCell<Vec<Box<dyn FnOnce()>>>>);

    impl Deferred {
        fn new() -> Self {
            Self(Rc::default())
        }

        fn defer(&self) -> Defer {
            let queue = self.0.clone();
            Rc::new(move |job| queue.borrow_mut().push(job))
        }

        /// Runs the jobs queued so far, returning how many ran.
        fn run(&self) -> usize {
            let jobs = std::mem::take(&mut *self.0.borrow_mut());
            let count = jobs.len();
            for job in jobs {
                job();
            }
            count
        }
    }

    fn counting_arm() -> Rc<dyn Fn(&mut u32)> {
        Rc::new(|armed| *armed += 1)
    }

    #[test]
    fn arming_a_free_state_applies_immediately() {
        let state = Rc::new(RefCell::new(0u32));
        let deferred = Deferred::new();
        arm_or_defer(&Rc::downgrade(&state), &deferred.defer(), &counting_arm());
        assert_eq!(*state.borrow(), 1);
        assert_eq!(deferred.run(), 0);
    }

    /// ANM-001 A4 / R2: arming while the platform state is borrowed defers the
    /// restart and retries until it can run.
    #[test]
    fn arming_a_borrowed_state_defers_the_restart() {
        let state = Rc::new(RefCell::new(0u32));
        let deferred = Deferred::new();

        let guard = state.borrow_mut();
        arm_or_defer(&Rc::downgrade(&state), &deferred.defer(), &counting_arm());
        assert_eq!(deferred.run(), 1, "the restart was queued");
        assert_eq!(deferred.run(), 1, "still borrowed: deferred again");
        drop(guard);

        assert_eq!(*state.borrow(), 0);
        assert_eq!(deferred.run(), 1);
        assert_eq!(*state.borrow(), 1, "applied once the state was free");
        assert_eq!(deferred.run(), 0);
    }

    /// ANM-001 A4 / R9: a restart deferred before the window was destroyed is
    /// ignored when it finally runs.
    #[test]
    fn deferred_restart_after_destruction_is_ignored() {
        let state = Rc::new(RefCell::new(0u32));
        let deferred = Deferred::new();
        let target = Rc::downgrade(&state);

        let guard = state.borrow_mut();
        arm_or_defer(&target, &deferred.defer(), &counting_arm());
        drop(guard);
        drop(state);

        assert_eq!(deferred.run(), 1);
        assert!(target.upgrade().is_none());
        assert_eq!(deferred.run(), 0, "nothing was re-queued");
    }
}
