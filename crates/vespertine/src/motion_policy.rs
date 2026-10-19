//! The motion policy: how the system's reduced-motion preference, a user override and a
//! pause-in-background setting combine into the framework's reduced-motion flag
//! ([`App::reduce_motion`]).
//!
//! The resolved flag is
//! `(preference == FollowSystem ? system_reduced : preference == On) ||
//! (pause_in_background && !window_active)`
//! and is written to the framework flag whenever any input changes.

use crate::{App, Global, WindowId, animation_clock::quiet_global_mut};
use vespertine_collections::FxHashSet;

/// The user's reduced-motion preference.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum MotionPreference {
    /// Reduce motion exactly when the system reports reduced motion.
    #[default]
    FollowSystem,
    /// Always reduce motion (forced reduction).
    On,
    /// Never reduce motion because of the preference (normal motion).
    Off,
}

/// The inputs of the motion policy and their resolution.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MotionPolicy {
    /// The user's reduced-motion preference.
    pub preference: MotionPreference,
    /// Whether animations pause (reduce) while the application's window is not active.
    pub pause_in_background: bool,
    /// Whether the system reports reduced motion.
    pub system_reduced: bool,
    /// Whether one of the application's windows is active.
    pub window_active: bool,
}

impl Default for MotionPolicy {
    fn default() -> Self {
        Self {
            preference: MotionPreference::FollowSystem,
            pause_in_background: false,
            system_reduced: false,
            window_active: true,
        }
    }
}

impl MotionPolicy {
    /// The resolved reduced-motion flag.
    pub fn resolve(&self) -> bool {
        let preferred = match self.preference {
            MotionPreference::FollowSystem => self.system_reduced,
            MotionPreference::On => true,
            MotionPreference::Off => false,
        };
        preferred || self.paused_in_background()
    }

    /// Whether the background pause is in effect.
    pub fn paused_in_background(&self) -> bool {
        self.pause_in_background && !self.window_active
    }

    /// Whether motion is reduced *only* because the system asks for it: the preference follows
    /// the system, the system reports reduced motion and the window is not paused by the
    /// background setting. Activity indicators keep a gentle uniform brightness cue in this state.
    pub fn system_only_reduction(&self) -> bool {
        self.preference == MotionPreference::FollowSystem
            && self.system_reduced
            && !self.paused_in_background()
    }
}

/// Installed motion-policy state, an application global. Its absence means the motion policy has
/// not been installed (for example in tests) and the reduced-motion flag is left to the caller.
#[derive(Default)]
struct MotionState {
    policy: MotionPolicy,
    active_windows: FxHashSet<WindowId>,
}

impl Global for MotionState {}

impl App {
    /// Installs the motion policy: the resolved flag becomes owned by the policy, and the
    /// system's reduced-motion value is read from the platform (off the foreground) and applied
    /// when it returns. [`Application::run`](crate::Application::run) calls this at launch.
    /// Installing twice does nothing.
    pub fn install_motion_policy(&mut self) {
        if self.has_global::<MotionState>() {
            return;
        }
        self.ensure_motion_state();
        self.refresh_system_reduced_motion();
    }

    fn ensure_motion_state(&mut self) -> &mut MotionState {
        quiet_global_mut::<MotionState>(self)
    }

    /// The current motion policy inputs. Before the policy is installed this is the default
    /// policy (follow the system, no background pause, system value false, window active).
    pub fn motion_policy(&self) -> MotionPolicy {
        self.try_global::<MotionState>()
            .map(|state| state.policy)
            .unwrap_or_default()
    }

    /// Applies a change to the policy inputs and writes the resolved flag to the framework's
    /// reduced-motion flag. Installs the policy state if it is missing.
    fn update_motion_policy(&mut self, update: impl FnOnce(&mut MotionState)) {
        let state = self.ensure_motion_state();
        update(state);
        let resolved = state.policy.resolve();
        self.set_reduce_motion(resolved);
    }

    /// Sets the reduced-motion preference (follow the system, forced on, or off).
    pub fn set_motion_preference(&mut self, preference: MotionPreference) {
        self.update_motion_policy(|state| state.policy.preference = preference);
    }

    /// Sets whether animations pause while the application's window is not active.
    pub fn set_pause_in_background(&mut self, pause: bool) {
        self.update_motion_policy(|state| state.policy.pause_in_background = pause);
    }

    /// Sets the system's reduced-motion value.
    pub fn set_system_reduced_motion(&mut self, reduced: bool) {
        self.update_motion_policy(|state| state.policy.system_reduced = reduced);
    }

    /// Reads the system's reduced-motion value from the platform and applies it on the
    /// foreground when the read returns. On Linux this is a desktop settings portal round trip
    /// performed off the foreground; a missing portal or key reads as no preference.
    pub fn refresh_system_reduced_motion(&mut self) {
        let read = self.platform.system_reduced_motion();
        self.spawn(async move |cx| {
            let reduced = read.await;
            // The application may have been released while the read was in flight.
            if let Some(app) = cx.app.upgrade() {
                app.borrow_mut()
                    .update(|cx| cx.set_system_reduced_motion(reduced));
            }
        })
        .detach();
    }

    /// Records that `window` became active or inactive. The application counts as active while
    /// any window is active. Regaining activation re-reads the system's reduced-motion value.
    /// Does nothing before the motion policy is installed.
    pub fn observe_active_window(&mut self, window: WindowId, active: bool) {
        if !self.has_global::<MotionState>() {
            return;
        }
        let windows_alive = |cx: &App, id: &WindowId| cx.windows.contains_key(*id);
        let mut live = FxHashSet::default();
        for id in self.global::<MotionState>().active_windows.iter() {
            if *id != window && windows_alive(self, id) {
                live.insert(*id);
            }
        }
        if active {
            live.insert(window);
        }
        let window_active = !live.is_empty();
        self.update_motion_policy(|state| {
            state.active_windows = live;
            state.policy.window_active = window_active;
        });
        if active {
            self.refresh_system_reduced_motion();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActivityIndicator, Context, IntoElement, Render, TestAppContext, Window, WindowHandle,
        animation_clock::{
            ACTIVITY_INDICATOR_PERIOD, ANIMATION_CLOCK_INTERVAL, gentle_cue_opacity,
        },
        div, px, size,
    };
    use std::time::Duration;

    const PREFERENCES: [MotionPreference; 3] = [
        MotionPreference::FollowSystem,
        MotionPreference::On,
        MotionPreference::Off,
    ];

    #[test]
    fn resolver_matches_the_formula_for_every_combination() {
        for preference in PREFERENCES {
            for system_reduced in [false, true] {
                for pause_in_background in [false, true] {
                    for window_active in [false, true] {
                        let policy = MotionPolicy {
                            preference,
                            pause_in_background,
                            system_reduced,
                            window_active,
                        };
                        let preferred = match preference {
                            MotionPreference::FollowSystem => system_reduced,
                            MotionPreference::On => true,
                            MotionPreference::Off => false,
                        };
                        let expected = preferred || (pause_in_background && !window_active);
                        assert_eq!(policy.resolve(), expected, "{policy:?}");

                        let system_only = preference == MotionPreference::FollowSystem
                            && system_reduced
                            && !(pause_in_background && !window_active);
                        assert_eq!(policy.system_only_reduction(), system_only, "{policy:?}");
                    }
                }
            }
        }
    }

    struct IndicatorView {
        indicator: ActivityIndicator,
        renders: usize,
    }

    impl Render for IndicatorView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders += 1;
            self.indicator = window.activity_indicator(cx);
            div()
        }
    }

    fn open_indicator(cx: &mut TestAppContext) -> WindowHandle<IndicatorView> {
        let window = cx.open_window(size(px(100.), px(100.)), |_, _| IndicatorView {
            indicator: ActivityIndicator {
                phase: 0.0,
                gentle_cue: false,
                opacity: 1.0,
            },
            renders: 0,
        });
        cx.run_until_parked();
        window
    }

    fn indicator(
        cx: &mut TestAppContext,
        window: &WindowHandle<IndicatorView>,
    ) -> (ActivityIndicator, usize) {
        window
            .read_with(cx, |view, _| (view.indicator, view.renders))
            .unwrap()
    }

    fn tick(cx: &mut TestAppContext) {
        cx.executor().advance_clock(ANIMATION_CLOCK_INTERVAL);
        cx.run_until_parked();
    }

    /// Installs the policy and waits for its initial platform read to be applied.
    fn install(cx: &mut TestAppContext) {
        cx.update(|cx| cx.install_motion_policy());
        cx.run_until_parked();
    }

    fn set_inactive(cx: &mut TestAppContext, window: &WindowHandle<IndicatorView>, active: bool) {
        cx.test_window((*window).into())
            .simulate_active_status_change(active);
        cx.run_until_parked();
    }

    #[vespertine::test]
    fn setters_write_the_resolved_flag_to_the_framework_flag(cx: &mut TestAppContext) {
        let window = open_indicator(cx);
        install(cx);
        cx.update(|cx| {
            assert!(!cx.reduce_motion());
            cx.set_system_reduced_motion(true);
            assert!(cx.reduce_motion());
            cx.set_motion_preference(MotionPreference::Off);
            assert!(!cx.reduce_motion());
            cx.set_motion_preference(MotionPreference::On);
            cx.set_system_reduced_motion(false);
            assert!(cx.reduce_motion());
            cx.set_motion_preference(MotionPreference::FollowSystem);
            assert!(!cx.reduce_motion());
            cx.set_pause_in_background(true);
            assert!(!cx.reduce_motion(), "the window is active at launch");
        });

        set_inactive(cx, &window, false);
        assert!(cx.update(|cx| cx.reduce_motion()));
        set_inactive(cx, &window, true);
        assert!(!cx.update(|cx| cx.reduce_motion()));
        cx.update(|cx| {
            cx.set_pause_in_background(false);
        });
        set_inactive(cx, &window, false);
        assert!(!cx.update(|cx| cx.reduce_motion()));
    }

    #[vespertine::test]
    fn activation_observation_does_nothing_before_the_policy_is_installed(cx: &mut TestAppContext) {
        let window = open_indicator(cx);
        cx.update(|cx| cx.set_reduce_motion(false));
        set_inactive(cx, &window, false);
        assert!(!cx.update(|cx| cx.has_global::<MotionState>()));
        assert!(cx.update(|cx| cx.motion_policy().window_active));
        assert!(!cx.update(|cx| cx.reduce_motion()));
    }

    #[vespertine::test]
    fn regaining_activation_and_install_read_the_system_value_from_the_platform(
        cx: &mut TestAppContext,
    ) {
        let window = open_indicator(cx);
        cx.test_platform().system_reduced_motion.set(true);
        cx.update(|cx| cx.install_motion_policy());
        // The read is applied on the foreground once it returns.
        cx.run_until_parked();
        assert!(cx.update(|cx| cx.motion_policy().system_reduced));
        assert!(cx.update(|cx| cx.reduce_motion()));

        // The platform value changes while the window is in the background; regaining
        // activation picks it up.
        set_inactive(cx, &window, false);
        cx.test_platform().system_reduced_motion.set(false);
        assert!(cx.update(|cx| cx.reduce_motion()));
        set_inactive(cx, &window, true);
        assert!(!cx.update(|cx| cx.motion_policy().system_reduced));
        assert!(!cx.update(|cx| cx.reduce_motion()));
    }

    #[vespertine::test]
    fn indicator_animates_at_normal_motion(cx: &mut TestAppContext) {
        let window = open_indicator(cx);
        install(cx);
        cx.executor().advance_clock(Duration::from_millis(600));
        tick(cx);
        let (indicator, _) = indicator(cx, &window);
        assert!(!indicator.gentle_cue);
        assert_eq!(indicator.opacity, 1.0);
        assert!(indicator.phase > 0.0 && indicator.phase < 1.0);
    }

    #[vespertine::test]
    fn system_only_reduction_keeps_a_uniform_gentle_cue_at_stride_two(cx: &mut TestAppContext) {
        let window = open_indicator(cx);
        install(cx);
        cx.update(|cx| {
            cx.set_system_reduced_motion(true);
        });
        cx.run_until_parked();
        assert!(cx.update(|cx| cx.reduce_motion()));

        let (_, renders0) = indicator(cx, &window);
        let mut seen_min = f32::MAX;
        let mut seen_max = f32::MIN;
        for _ in 0..12 {
            tick(cx);
            let (indicator, _) = indicator(cx, &window);
            assert!(indicator.gentle_cue);
            assert!(indicator.opacity >= 0.6 - 1e-6 && indicator.opacity <= 0.8 + 1e-6);
            assert_eq!(indicator.opacity, gentle_cue_opacity(indicator.phase));
            seen_min = seen_min.min(indicator.opacity);
            seen_max = seen_max.max(indicator.opacity);
        }
        let (_, renders) = indicator(cx, &window);
        assert_eq!(renders - renders0, 6, "stride 2: every second 33 ms tick");
        assert!(seen_max > seen_min, "the cue varies over time");

        // Phase 0 is the dimmest and phase 0.5 the brightest point of the cue, taken over the
        // 2.4 s period from the shared epoch.
        assert!((gentle_cue_opacity(0.0) - 0.6).abs() < 1e-6);
        assert!((gentle_cue_opacity(0.5) - 0.8).abs() < 1e-6);
        assert_eq!(ACTIVITY_INDICATOR_PERIOD, Duration::from_millis(2400));
    }

    #[vespertine::test]
    fn gentle_cue_is_uniform_across_indicators(cx: &mut TestAppContext) {
        let first = open_indicator(cx);
        let second = open_indicator(cx);
        install(cx);
        cx.update(|cx| {
            cx.set_system_reduced_motion(true);
        });
        cx.run_until_parked();
        for _ in 0..4 {
            tick(cx);
        }
        let a = indicator(cx, &first).0;
        let b = indicator(cx, &second).0;
        assert!(a.gentle_cue && b.gentle_cue);
        assert!((a.phase - b.phase).abs() < 1e-3 || (a.phase - b.phase).abs() > 0.999);
        assert!((a.opacity - b.opacity).abs() < 1e-3);
    }

    #[vespertine::test]
    fn forced_reduction_freezes_the_indicator(cx: &mut TestAppContext) {
        let window = open_indicator(cx);
        install(cx);
        cx.update(|cx| {
            cx.set_system_reduced_motion(true);
            cx.set_motion_preference(MotionPreference::On);
        });
        cx.run_until_parked();
        for _ in 0..8 {
            tick(cx);
        }
        let (indicator, _) = indicator(cx, &window);
        assert_eq!(
            indicator,
            ActivityIndicator {
                phase: 0.0,
                gentle_cue: false,
                opacity: 1.0
            }
        );
        assert!(!cx.update(|cx| cx.animation_clock_is_running()));
        assert_eq!(cx.update(|cx| cx.animation_lease_count()), 0);
    }

    #[vespertine::test]
    fn background_pause_freezes_the_indicator(cx: &mut TestAppContext) {
        let window = open_indicator(cx);
        // The platform value is re-read when the window regains activation below.
        cx.test_platform().system_reduced_motion.set(true);
        install(cx);
        cx.update(|cx| cx.set_pause_in_background(true));
        cx.run_until_parked();
        assert!(
            indicator(cx, &window).0.gentle_cue,
            "active window: system-only cue"
        );

        set_inactive(cx, &window, false);
        for _ in 0..12 {
            tick(cx);
        }
        let (frozen, renders) = indicator(cx, &window);
        assert!(!frozen.gentle_cue);
        assert_eq!((frozen.phase, frozen.opacity), (0.0, 1.0));
        assert!(!cx.update(|cx| cx.animation_clock_is_running()));
        tick(cx);
        assert_eq!(indicator(cx, &window).1, renders, "no frames while paused");

        set_inactive(cx, &window, true);
        assert!(indicator(cx, &window).0.gentle_cue);
    }
}
