//! The timer thread behind `PlatformDispatcher::dispatch_after`.
//!
//! Linux gives every timer a kernel timer of its own, armed by the thread that asks for it, so an
//! idle process wakes the timer thread only when a timer fires. Other Unixes keep a thread that
//! arms the timers itself, which costs it an extra wakeup per timer.

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
            Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, timerfd_create,
            timerfd_settime,
        },
    };
    use std::{collections::HashMap, io, os::fd::AsRawFd as _};
    use vespertine::profiler;
    use vespertine_util::ResultExt;

    /// Timers longer than this are clamped: a kernel timer cannot hold more, and no process
    /// outlives it.
    const LONGEST_TIMER: Duration = Duration::from_secs(100 * 365 * 24 * 3600);

    /// Registered with the epoll set in place of a timer to end the thread.
    const STOP_KEY: u64 = u64::MAX;

    /// A timer that has been armed and not yet fired.
    struct PendingTimer {
        timer_fd: OwnedFd,
        runnable: RunnableVariant,
    }

    pub(crate) struct Shared {
        epoll: OwnedFd,
        stop: OwnedFd,
        timers: parking_lot::Mutex<HashMap<u64, PendingTimer>>,
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
            Ok(Arc::new(Self {
                epoll,
                stop,
                timers: parking_lot::Mutex::new(HashMap::default()),
            }))
        }

        pub(super) fn arm(
            &self,
            duration: Duration,
            runnable: RunnableVariant,
        ) -> Result<(), RunnableVariant> {
            match Self::arm_timer(duration) {
                Ok(timer_fd) => self.register(timer_fd, runnable),
                Err(_) => Err(runnable),
            }
        }

        fn arm_timer(duration: Duration) -> io::Result<OwnedFd> {
            let timer_fd = timerfd_create(
                TimerfdClockId::Monotonic,
                TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
            )?;
            // A zero value would disarm the timer instead of firing it at once.
            let value = Timespec::try_from(duration.clamp(Duration::from_nanos(1), LONGEST_TIMER))
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            let never = Timespec::try_from(Duration::ZERO)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            timerfd_settime(
                &timer_fd,
                TimerfdTimerFlags::empty(),
                &Itimerspec {
                    it_interval: never,
                    it_value: value,
                },
            )?;
            Ok(timer_fd)
        }

        fn register(
            &self,
            timer_fd: OwnedFd,
            runnable: RunnableVariant,
        ) -> Result<(), RunnableVariant> {
            // Keyed by the descriptor number, which stays unique while the timer is pending.
            let key = timer_fd.as_raw_fd() as u64;
            let mut timers = self.timers.lock();
            let registered = epoll::add(
                &self.epoll,
                &timer_fd,
                epoll::EventData::new_u64(key),
                epoll::EventFlags::IN | epoll::EventFlags::ONESHOT,
            );
            if registered.is_err() {
                return Err(runnable);
            }
            timers.insert(key, PendingTimer { timer_fd, runnable });
            Ok(())
        }

        pub(super) fn stop(&self) {
            rustix::io::write(self.stop.as_fd(), &1u64.to_ne_bytes()).log_err();
        }

        pub(super) fn run(&self) {
            let mut events: Vec<epoll::Event> = Vec::with_capacity(16);
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
                for event in &events {
                    let key = event.data.u64();
                    if key == STOP_KEY {
                        return;
                    }
                    // Dropping the timer closes it, which removes it from the epoll set.
                    let timer = self.timers.lock().remove(&key);
                    if let Some(PendingTimer { timer_fd, runnable }) = timer {
                        drop(timer_fd);
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
