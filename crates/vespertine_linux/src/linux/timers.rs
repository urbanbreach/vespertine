//! The timer thread behind `PlatformDispatcher::dispatch_after`.
//!
//! Linux keeps pending timers in one deadline queue behind one kernel timer, which the thread
//! asking for a timer arms itself when its deadline comes first, so an idle process wakes the
//! timer thread only when a timer fires. Other Unixes keep a thread that arms the timers
//! itself, which costs it an extra wakeup per timer.

use std::{sync::Arc, thread, time::Duration};
use vespertine::RunnableVariant;

#[cfg(target_os = "linux")]
use self::kernel_timers::Shared;
#[cfg(not(target_os = "linux"))]
use self::timer_loop::Shared;

/// Runs runnables after a delay on a thread of its own. Dropping it stops the thread and drops
/// the timers still pending.
pub(crate) struct Timers {
    shared: Arc<Shared>,
}

impl Timers {
    /// Creates the timers and the thread that runs them.
    pub(crate) fn start() -> (Self, thread::JoinHandle<()>) {
        let shared = Shared::new().expect("Failed to initialize the timer thread!");
        let handle = thread::Builder::new()
            .name("Timer".to_owned())
            .spawn({
                let shared = shared.clone();
                move || shared.run()
            })
            .unwrap();
        (Self { shared }, handle)
    }

    /// Runs `runnable` on the timer thread after `duration`, or hands it back when it cannot be
    /// scheduled any more.
    pub(crate) fn arm(
        &self,
        duration: Duration,
        runnable: RunnableVariant,
    ) -> Result<(), RunnableVariant> {
        self.shared.arm(duration, runnable)
    }

    /// A handle that stays upgradable for as long as the timer thread runs.
    #[cfg(test)]
    pub(crate) fn watch(&self) -> std::sync::Weak<Shared> {
        Arc::downgrade(&self.shared)
    }
}

impl Drop for Timers {
    fn drop(&mut self) {
        self.shared.stop();
    }
}

#[cfg(target_os = "linux")]
mod kernel_timers {
    use super::*;
    use rustix::{
        buffer::spare_capacity,
        event::{EventfdFlags, Timespec, epoll, eventfd},
        fd::{AsFd, OwnedFd},
        io::Errno,
        time::{
            ClockId, Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, clock_gettime,
            timerfd_create, timerfd_settime,
        },
    };
    use std::{cmp::Reverse, collections::BinaryHeap, io};
    use vespertine::profiler;
    use vespertine_util::ResultExt;

    /// Timers longer than this are clamped: a kernel timer cannot hold more, and no process
    /// outlives it.
    const LONGEST_TIMER: Duration = Duration::from_secs(100 * 365 * 24 * 3600);

    /// The epoll key of the kernel timer.
    const TIMER_KEY: u64 = 0;
    /// The epoll key of the event that ends the thread.
    const STOP_KEY: u64 = 1;

    /// A timer that has been armed and not yet fired, ordered by deadline and then by arming
    /// order.
    struct PendingTimer {
        /// On the monotonic clock.
        deadline: Duration,
        sequence: u64,
        runnable: RunnableVariant,
    }

    impl PendingTimer {
        fn key(&self) -> (Duration, u64) {
            (self.deadline, self.sequence)
        }
    }

    impl PartialEq for PendingTimer {
        fn eq(&self, other: &Self) -> bool {
            self.key() == other.key()
        }
    }

    impl Eq for PendingTimer {}

    impl PartialOrd for PendingTimer {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for PendingTimer {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.key().cmp(&other.key())
        }
    }

    #[derive(Default)]
    struct Queue {
        /// Earliest deadline first.
        pending: BinaryHeap<Reverse<PendingTimer>>,
        next_sequence: u64,
    }

    pub(crate) struct Shared {
        epoll: OwnedFd,
        stop: OwnedFd,
        /// Armed for the earliest pending deadline, disarmed when nothing is pending.
        timer_fd: OwnedFd,
        /// Held while the kernel timer is changed, so it always matches the earliest deadline.
        queue: parking_lot::Mutex<Queue>,
    }

    fn monotonic_now() -> Duration {
        // The monotonic clock never reads negative, which is the only conversion failure.
        Duration::try_from(clock_gettime(ClockId::Monotonic)).unwrap_or_default()
    }

    impl Shared {
        pub(super) fn new() -> io::Result<Arc<Self>> {
            let epoll = epoll::create(epoll::CreateFlags::CLOEXEC)?;
            let stop = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
            epoll::add(
                &epoll,
                &stop,
                epoll::EventData::new_u64(STOP_KEY),
                epoll::EventFlags::IN,
            )?;
            let timer_fd = timerfd_create(
                TimerfdClockId::Monotonic,
                TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
            )?;
            epoll::add(
                &epoll,
                &timer_fd,
                epoll::EventData::new_u64(TIMER_KEY),
                epoll::EventFlags::IN,
            )?;
            Ok(Arc::new(Self {
                epoll,
                stop,
                timer_fd,
                queue: parking_lot::Mutex::new(Queue::default()),
            }))
        }

        pub(super) fn arm(
            &self,
            duration: Duration,
            runnable: RunnableVariant,
        ) -> Result<(), RunnableVariant> {
            // A zero deadline would disarm the timer instead of firing it at once.
            let deadline = monotonic_now() + duration.clamp(Duration::from_nanos(1), LONGEST_TIMER);
            let mut queue = self.queue.lock();
            let first = queue
                .pending
                .peek()
                .is_none_or(|Reverse(earliest)| deadline < earliest.deadline);
            // Only a new earliest deadline moves the kernel timer, and this thread moves it, so
            // the timer thread wakes once per firing and never to rearm.
            if first && self.set_deadline(Some(deadline)).is_err() {
                return Err(runnable);
            }
            let sequence = queue.next_sequence;
            queue.next_sequence += 1;
            queue.pending.push(Reverse(PendingTimer {
                deadline,
                sequence,
                runnable,
            }));
            Ok(())
        }

        /// Arms the kernel timer for an absolute monotonic `deadline`, or disarms it.
        fn set_deadline(&self, deadline: Option<Duration>) -> io::Result<()> {
            let invalid = |_| io::Error::from(io::ErrorKind::InvalidInput);
            let never = Timespec::try_from(Duration::ZERO).map_err(invalid)?;
            let value = match deadline {
                Some(deadline) => Timespec::try_from(deadline).map_err(invalid)?,
                None => never,
            };
            timerfd_settime(
                &self.timer_fd,
                TimerfdTimerFlags::ABSTIME,
                &Itimerspec {
                    it_interval: never,
                    it_value: value,
                },
            )?;
            Ok(())
        }

        /// Takes the timers whose deadline passed, earliest first, and arms the kernel timer
        /// for the next one.
        fn take_due(&self) -> Vec<RunnableVariant> {
            let mut queue = self.queue.lock();
            // Clears the expiration; nothing is left to read when a caller moved the deadline.
            let mut expirations = [0u8; 8];
            let _ = rustix::io::read(&self.timer_fd, &mut expirations);
            let now = monotonic_now();
            let mut due = Vec::new();
            while let Some(Reverse(earliest)) = queue.pending.peek()
                && earliest.deadline <= now
            {
                if let Some(Reverse(timer)) = queue.pending.pop() {
                    due.push(timer.runnable);
                }
            }
            let next = queue.pending.peek().map(|Reverse(timer)| timer.deadline);
            if let Err(error) = self.set_deadline(next) {
                log::error!("arming the timer thread's kernel timer failed: {error}");
            }
            due
        }

        pub(super) fn stop(&self) {
            rustix::io::write(self.stop.as_fd(), &1u64.to_ne_bytes()).log_err();
        }

        pub(super) fn run(&self) {
            let mut events: Vec<epoll::Event> = Vec::with_capacity(2);
            loop {
                events.clear();
                match epoll::wait(&self.epoll, spare_capacity(&mut events), None) {
                    Ok(_) => {}
                    Err(Errno::INTR) => continue,
                    Err(error) => {
                        log::error!("the timer thread stopped waiting: {error}");
                        return;
                    }
                }
                if events.iter().any(|event| event.data.u64() == STOP_KEY) {
                    return;
                }
                for runnable in self.take_due() {
                    let location = runnable.metadata().location;
                    let spawned = runnable.metadata().spawned;
                    profiler::update_running_task(spawned, location);
                    runnable.run();
                    profiler::save_task_timing();
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod timer_loop {
    use super::*;
    use calloop::{
        EventLoop, LoopSignal,
        channel::{self, Sender},
        timer::{TimeoutAction, Timer},
    };
    use vespertine::profiler;
    use vespertine_util::ResultExt;

    struct TimerAfter {
        duration: Duration,
        runnable: RunnableVariant,
    }

    pub(crate) struct Shared {
        /// Taken by `stop`: the loop ends when the last sender is gone.
        sender: parking_lot::Mutex<Option<Sender<TimerAfter>>>,
        receiver: parking_lot::Mutex<Option<channel::Channel<TimerAfter>>>,
    }

    impl Shared {
        pub(super) fn new() -> Result<Arc<Self>, std::convert::Infallible> {
            let (sender, receiver) = channel::channel::<TimerAfter>();
            Ok(Arc::new(Self {
                sender: parking_lot::Mutex::new(Some(sender)),
                receiver: parking_lot::Mutex::new(Some(receiver)),
            }))
        }

        pub(super) fn arm(
            &self,
            duration: Duration,
            runnable: RunnableVariant,
        ) -> Result<(), RunnableVariant> {
            let sender = self.sender.lock();
            let Some(sender) = sender.as_ref() else {
                return Err(runnable);
            };
            sender
                .send(TimerAfter { duration, runnable })
                .map_err(|error| error.0.runnable)
        }

        pub(super) fn stop(&self) {
            self.sender.lock().take();
        }

        pub(super) fn run(&self) {
            let Some(channel) = self.receiver.lock().take() else {
                return;
            };
            let mut event_loop: EventLoop<()> =
                EventLoop::try_new().expect("Failed to initialize timer loop!");
            let handle = event_loop.handle();
            let timer_handle = event_loop.handle();
            let signal: LoopSignal = event_loop.get_signal();
            handle
                .insert_source(channel, move |event, _, _| match event {
                    channel::Event::Closed => signal.stop(),
                    channel::Event::Msg(timer) => {
                        let mut runnable = Some(timer.runnable);
                        timer_handle
                            .insert_source(Timer::from_duration(timer.duration), move |_, _, _| {
                                if let Some(runnable) = runnable.take() {
                                    let location = runnable.metadata().location;
                                    let spawned = runnable.metadata().spawned;
                                    profiler::update_running_task(spawned, location);
                                    runnable.run();
                                    profiler::save_task_timing();
                                }
                                TimeoutAction::Drop
                            })
                            .expect("Failed to start timer");
                    }
                })
                .expect("Failed to start timer thread");
            event_loop.run(None, &mut (), |_| {}).log_err();
        }
    }
}
