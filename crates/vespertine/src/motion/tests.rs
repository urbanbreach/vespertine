//! Tests for ANM-002 (A1-A7) and the native side of ANM-003.

use std::time::Duration;

use super::*;

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn linear_one_second() -> MotionTransition {
    MotionTransition::new().duration(1.0).easing(Easing::LINEAR)
}

fn width_track(from: Option<f64>, to: f64) -> MotionDescription {
    let mut description =
        MotionDescription::new(MotionProps::new().width(to)).transition(linear_one_second());
    if let Some(from) = from {
        description = description.initial(MotionInitial::Style(MotionProps::new().width(from)));
    }
    description
}

mod easing_tests {
    use super::*;
    use rand::{Rng as _, SeedableRng as _, rngs::SmallRng};

    fn reference(curve: CubicBezier, t: f64) -> f64 {
        let component = |a: f64, b: f64, s: f64| {
            let inverse = 1.0 - s;
            3.0 * inverse * inverse * s * a + 3.0 * inverse * s * s * b + s * s * s
        };
        let (mut low, mut high) = (0.0f64, 1.0f64);
        for _ in 0..80 {
            let mid = (low + high) / 2.0;
            if component(f64::from(curve.x1), f64::from(curve.x2), mid) < t {
                low = mid;
            } else {
                high = mid;
            }
        }
        component(f64::from(curve.y1), f64::from(curve.y2), (low + high) / 2.0).clamp(0.0, 1.0)
    }

    fn random_curve(rng: &mut SmallRng, y_range: std::ops::RangeInclusive<f32>) -> CubicBezier {
        CubicBezier::new(
            rng.random_range(0.0..=1.0),
            rng.random_range(y_range.clone()),
            rng.random_range(0.0..=1.0),
            rng.random_range(y_range),
        )
    }

    #[test]
    fn named_curves_hit_their_endpoints() {
        for named in [
            NamedEasing::Linear,
            NamedEasing::Ease,
            NamedEasing::EaseIn,
            NamedEasing::EaseOut,
            NamedEasing::EaseInOut,
        ] {
            assert_eq!(named.ease(0.0), 0.0, "{named:?}");
            assert_eq!(named.ease(1.0), 1.0, "{named:?}");
        }
        assert_eq!(NamedEasing::Linear.ease(0.37), 0.37);
        assert_eq!(
            NamedEasing::from_name("easeInOut"),
            Some(NamedEasing::EaseInOut)
        );
        assert_eq!(NamedEasing::from_name("bounce"), None);
        assert_eq!(
            NamedEasing::EaseOut.curve(),
            Some(CubicBezier::new(0.0, 0.0, 0.58, 1.0))
        );
    }

    #[test]
    fn random_valid_curves_hit_their_endpoints_and_clamp_y() {
        let mut rng = SmallRng::seed_from_u64(7);
        for _ in 0..500 {
            let curve = random_curve(&mut rng, -3.0..=4.0);
            assert!(curve.validate().is_ok());
            assert_eq!(curve.ease(0.0), 0.0);
            assert_eq!(curve.ease(1.0), 1.0);
            for step in 0..=100 {
                let value = curve.ease(step as f32 / 100.0);
                assert!((0.0..=1.0).contains(&value), "{curve:?} -> {value}");
            }
        }
    }

    #[test]
    fn x_inversion_is_monotonic() {
        let mut rng = SmallRng::seed_from_u64(11);
        for _ in 0..200 {
            let curve = random_curve(&mut rng, 0.0..=1.0);
            let mut previous = 0.0;
            for step in 0..=200 {
                let s = curve.solve_x(step as f32 / 200.0);
                assert!(s + 1e-6 >= previous, "{curve:?} step {step}");
                previous = s;
            }
        }
    }

    #[test]
    fn twenty_step_solver_matches_high_precision_reference() {
        let mut rng = SmallRng::seed_from_u64(3);
        let mut curves = vec![
            CubicBezier::new(0.25, 0.1, 0.25, 1.0),
            CubicBezier::new(0.42, 0.0, 1.0, 1.0),
            CubicBezier::new(0.0, 0.0, 0.58, 1.0),
            CubicBezier::new(0.42, 0.0, 0.58, 1.0),
            CubicBezier::new(0.22, 1.0, 0.36, 1.0),
            CubicBezier::new(0.16, 1.0, 0.3, 1.0),
        ];
        for _ in 0..300 {
            curves.push(random_curve(&mut rng, 0.0..=1.0));
        }
        for curve in curves {
            for step in 1..200 {
                let t = step as f32 / 200.0;
                let difference = (f64::from(curve.ease(t)) - reference(curve, f64::from(t))).abs();
                assert!(difference <= 1e-5, "{curve:?} at {t}: {difference}");
            }
        }
    }

    #[test]
    fn invalid_curves_and_names_are_rejected() {
        assert!(CubicBezier::new(-0.1, 0.0, 0.5, 1.0).validate().is_err());
        assert!(CubicBezier::new(0.1, 0.0, 1.5, 1.0).validate().is_err());
        assert!(
            CubicBezier::new(0.1, f32::NAN, 0.5, 1.0)
                .validate()
                .is_err()
        );
        assert!(
            CubicBezier::new(0.1, 0.0, 0.5, f32::INFINITY)
                .validate()
                .is_err()
        );
        assert!(Easing::named("bounce").validate().is_err());
        assert!(Easing::named("easeOut").validate().is_ok());
    }

    #[test]
    fn animation_supports_delay_zero_duration_and_bezier() {
        use crate::Animation;
        let animation = Animation::new(Duration::from_secs(1))
            .with_delay(ms(200))
            .with_cubic_bezier(0.0, 0.0, 0.58, 1.0);
        assert_eq!(animation.sample(ms(0)).raw_progress, 0.0);
        assert_eq!(animation.sample(ms(200)).raw_progress, 0.0);
        assert!((animation.sample(ms(700)).raw_progress - 0.5).abs() < 1e-6);
        assert_eq!(
            animation.sample(ms(700)).value,
            CubicBezier::new(0.0, 0.0, 0.58, 1.0).ease(0.5)
        );
        assert!(!animation.sample(ms(1199)).finished);
        let end = animation.sample(ms(1300));
        assert!(end.finished);
        assert_eq!(end.raw_progress, 1.0);
        assert_eq!(end.value, 1.0);

        let repeating = Animation::new(Duration::from_secs(1)).repeat();
        assert!((repeating.sample(ms(1250)).raw_progress - 0.25).abs() < 1e-6);
        assert!(!repeating.sample(ms(5000)).finished);

        let zero = Animation::new(Duration::ZERO).with_delay(ms(100));
        assert_eq!(zero.sample(ms(99)).raw_progress, 0.0);
        assert_eq!(zero.sample(ms(100)).raw_progress, 1.0);
        assert_eq!(zero.sample(ms(101)).raw_progress, 1.0);
    }
}

mod track_tests {
    use super::*;

    fn width(track: &mut MotionTrack, at: u64) -> Option<f32> {
        track.sample(ms(at), false).style.width
    }

    #[test]
    fn a1_linear_width_track_and_retarget() {
        let mut track = MotionTrack::new();
        track.update(&width_track(Some(0.0), 100.0), ms(0));
        assert_eq!(width(&mut track, 0), Some(0.0));
        assert_eq!(width(&mut track, 500), Some(50.0));

        // Retarget to 0 at 500 ms: a new track from 50 over one second.
        track.update(&width_track(Some(0.0), 0.0), ms(500));
        assert_eq!(width(&mut track, 500), Some(50.0));
        assert_eq!(width(&mut track, 1000), Some(25.0));
        assert_eq!(width(&mut track, 1500), Some(0.0));
    }

    #[test]
    fn raw_progress_follows_delay_and_zero_duration_rules() {
        let (d, t) = (ms(200), ms(1000));
        assert_eq!(raw_progress(ms(0), d, t), 0.0);
        assert_eq!(raw_progress(ms(200), d, t), 0.0);
        assert!((raw_progress(ms(700), d, t) - 0.5).abs() < 1e-12);
        assert!(raw_progress(ms(2000), d, t) > 1.0);
        assert_eq!(raw_progress(ms(199), d, Duration::ZERO), 0.0);
        assert_eq!(raw_progress(ms(200), d, Duration::ZERO), 1.0);
        assert_eq!(raw_progress(ms(900), d, Duration::ZERO), 1.0);
    }

    #[test]
    fn delay_holds_the_start_value() {
        let mut track = MotionTrack::new();
        let description = width_track(Some(0.0), 100.0).transition(linear_one_second().delay(0.5));
        track.update(&description, ms(0));
        assert_eq!(width(&mut track, 100), Some(0.0));
        assert!(
            track.sample(ms(100), false).active,
            "delay still requests frames"
        );
        assert_eq!(width(&mut track, 500), Some(0.0));
        assert_eq!(width(&mut track, 1000), Some(50.0));
        assert_eq!(width(&mut track, 1500), Some(100.0));
    }

    #[test]
    fn zero_duration_is_zero_before_delay_and_one_after() {
        let mut track = MotionTrack::new();
        let description = width_track(Some(0.0), 80.0)
            .transition(MotionTransition::new().duration(0.0).delay(0.25));
        track.update(&description, ms(0));
        assert_eq!(width(&mut track, 249), Some(0.0));
        assert_eq!(width(&mut track, 250), Some(80.0));
    }

    #[test]
    fn unchanged_description_does_not_restart_or_retarget() {
        let mut track = MotionTrack::new();
        let description = width_track(Some(0.0), 100.0);
        track.update(&description, ms(0));
        assert_eq!(width(&mut track, 400), Some(40.0));
        track.update(&description.clone(), ms(400));
        assert_eq!(width(&mut track, 500), Some(50.0));
        // A different generation is a different description.
        track.update(&description.clone().generation(1), ms(500));
        assert_eq!(width(&mut track, 500), Some(50.0));
        assert_eq!(width(&mut track, 1000), Some(75.0));
    }

    #[test]
    fn missing_properties_keep_starts_and_new_ones_start_at_their_target() {
        let mut track = MotionTrack::new();
        track.update(&width_track(Some(0.0), 100.0), ms(0));
        // Replace with a target that lacks width: no override for width, height starts at target.
        let height_only =
            MotionDescription::new(MotionProps::new().height(20.0)).transition(linear_one_second());
        track.update(&height_only, ms(500));
        let sample = track.sample(ms(500), false);
        assert_eq!(sample.style.width, None);
        assert_eq!(sample.style.height, Some(20.0));
        assert!(!sample.active);
        // Bringing width back starts from its retained value (the last visible one).
        let both = MotionDescription::new(MotionProps::new().width(100.0).height(20.0))
            .transition(linear_one_second());
        track.update(&both, ms(600));
        assert_eq!(width(&mut track, 600), Some(50.0));
        assert_eq!(width(&mut track, 1100), Some(75.0));
    }

    #[test]
    fn initial_false_and_omitted_show_the_target_immediately() {
        for initial in [None, Some(MotionInitial::False)] {
            let mut track = MotionTrack::new();
            let mut description = width_track(None, 100.0);
            description.initial = initial;
            track.update(&description, ms(0));
            let sample = track.sample(ms(0), false);
            assert_eq!(sample.style.width, Some(100.0));
            assert!(!sample.active);
            assert_eq!(sample.completion, None);
        }
    }

    #[test]
    fn initial_true_is_rejected() {
        let mut track = MotionTrack::new();
        let mut description = width_track(None, 100.0);
        description.initial = Some(MotionInitial::True);
        assert_eq!(description.validate(), Err(MotionError::InvalidInitial));
        track.update(&description, ms(0));
        assert!(!track.is_valid());
        let sample = track.sample(ms(0), false);
        assert_eq!(sample.style, MotionStyle::default());
        assert!(!sample.active);
    }

    #[test]
    fn invalid_values_contribute_no_style_and_run_no_time() {
        let bad = [
            MotionProps::new().width(-1.0),
            MotionProps::new().height(f64::NAN),
            MotionProps::new().border_radius(f64::INFINITY),
            MotionProps::new().width(1e300),
            MotionProps::new().top(f64::NAN),
            MotionProps::new().left(1e300),
            MotionProps::new().opacity(1.5),
            MotionProps::new().opacity(-0.1),
        ];
        for props in bad {
            assert!(props.validate().is_err(), "{props:?}");
            let mut track = MotionTrack::new();
            track.update(&MotionDescription::new(props), ms(0));
            let sample = track.sample(ms(0), false);
            assert_eq!(sample.style, MotionStyle::default());
            assert!(!sample.active);
        }
        // Negative insets are valid.
        assert!(MotionProps::new().top(-5.0).left(-1e6).validate().is_ok());
        // Bad transitions.
        for transition in [
            MotionTransition::new().duration(-1.0),
            MotionTransition::new().delay(f64::NAN),
            MotionTransition::new().duration(f64::INFINITY),
            MotionTransition::new().easing(Easing::named("bounce")),
            MotionTransition::new().easing(Easing::cubic_bezier(2.0, 0.0, 0.5, 1.0)),
        ] {
            let description =
                MotionDescription::new(MotionProps::new().width(10.0)).transition(transition);
            assert!(description.validate().is_err());
        }
    }

    #[test]
    fn invalid_tracks_still_honor_completion_duties() {
        // An invalid exit track completes once even though it never runs.
        let mut track = MotionTrack::new();
        let invalid_exit = MotionDescription::new(MotionProps::new().width(10.0))
            .exit(MotionProps::new().width(-5.0))
            .present(false)
            .generation(4);
        track.update(&invalid_exit, ms(0));
        let first = track.sample(ms(0), false);
        assert_eq!(first.style, MotionStyle::default());
        assert_eq!(
            first.completion,
            Some(MotionCompletion {
                generation: 4,
                exit: true
            })
        );
        assert_eq!(track.sample(ms(10), false).completion, None);

        // An invalid description with a changed generation also completes.
        let mut track = MotionTrack::new();
        track.update(&width_track(Some(0.0), 10.0).generation(1), ms(0));
        track.sample(ms(2000), false);
        track.update(
            &MotionDescription::new(MotionProps::new().width(-1.0)).generation(2),
            ms(2000),
        );
        assert_eq!(
            track.sample(ms(2000), false).completion,
            Some(MotionCompletion {
                generation: 2,
                exit: false
            })
        );
    }

    #[test]
    fn generation_change_completes_once_and_noop_exit_completes_exactly_once() {
        let mut track = MotionTrack::new();
        let rest = MotionDescription::new(MotionProps::new().width(10.0));
        track.update(&rest, ms(0));
        assert_eq!(track.sample(ms(0), false).completion, None);

        // Changing only the generation owes a completion although nothing animates.
        track.update(&rest.clone().generation(9), ms(10));
        let sample = track.sample(ms(10), false);
        assert_eq!(
            sample.completion,
            Some(MotionCompletion {
                generation: 9,
                exit: false
            })
        );
        assert_eq!(track.sample(ms(20), false).completion, None);

        // A no-op exit completes exactly once, even though its start equals its target.
        let exit = rest.clone().generation(10).present(false);
        track.update(&exit, ms(30));
        let reports: Vec<_> = (30..300)
            .step_by(30)
            .filter_map(|at| track.sample(ms(at), false).completion)
            .collect();
        assert_eq!(
            reports,
            vec![MotionCompletion {
                generation: 10,
                exit: true
            }]
        );
        // Re-supplying the same exit description does not owe it again.
        track.update(&exit.clone(), ms(400));
        assert_eq!(track.sample(ms(400), false).completion, None);
    }

    #[test]
    fn running_track_never_completes_early_and_reports_once() {
        let mut track = MotionTrack::new();
        track.update(&width_track(Some(0.0), 100.0).generation(3), ms(0));
        for at in (0..1000).step_by(50) {
            let sample = track.sample(ms(at), false);
            assert!(sample.active, "at {at}");
            assert_eq!(sample.completion, None, "completed early at {at}");
        }
        let done = track.sample(ms(1000), false);
        assert!(!done.active);
        assert_eq!(
            done.completion,
            Some(MotionCompletion {
                generation: 3,
                exit: false
            })
        );
        assert_eq!(track.sample(ms(1100), false).completion, None);
        assert_eq!(track.sample(ms(5000), false).completion, None);
    }

    #[test]
    fn interrupted_track_still_owes_its_completion() {
        let mut track = MotionTrack::new();
        track.update(&width_track(Some(0.0), 100.0), ms(0));
        assert!(track.sample(ms(300), false).active);
        // Retarget at once (zero duration) to another value: nothing animates any more, but the
        // interrupted track's completion duty remains.
        let immediate = width_track(Some(0.0), 60.0)
            .transition(MotionTransition::new().duration(0.0).easing(Easing::LINEAR));
        track.update(&immediate, ms(300));
        let sample = track.sample(ms(300), false);
        assert!(!sample.active);
        assert!(sample.completion.is_some());
    }

    #[test]
    fn replacing_an_invalid_description_starts_from_the_new_initial() {
        let mut track = MotionTrack::new();
        track.update(
            &MotionDescription::new(MotionProps::new().width(-1.0)),
            ms(0),
        );
        track.update(&width_track(Some(10.0), 20.0), ms(100));
        assert_eq!(width(&mut track, 100), Some(10.0));
        assert_eq!(width(&mut track, 600), Some(15.0));

        let mut track = MotionTrack::new();
        track.update(
            &MotionDescription::new(MotionProps::new().width(-1.0)),
            ms(0),
        );
        track.update(&width_track(None, 20.0), ms(100));
        assert_eq!(width(&mut track, 100), Some(20.0));
    }

    #[test]
    fn reduced_motion_settles_at_the_target_and_still_completes() {
        let mut track = MotionTrack::new();
        track.update(&width_track(Some(0.0), 100.0).generation(1), ms(0));
        let sample = track.sample(ms(100), true);
        assert_eq!(sample.style.width, Some(100.0));
        assert!(!sample.active);
        assert_eq!(sample.completion.map(|c| c.generation), Some(1));
    }

    #[test]
    fn all_properties_interpolate_and_apply_to_style() {
        let target = MotionProps::new()
            .width(100.0)
            .height(50.0)
            .top(10.0)
            .right(20.0)
            .bottom(30.0)
            .left(-40.0)
            .border_radius(8.0)
            .opacity(0.5);
        let initial = MotionProps::new()
            .width(0.0)
            .height(0.0)
            .top(0.0)
            .right(0.0)
            .bottom(0.0)
            .left(0.0)
            .border_radius(0.0)
            .opacity(1.0);
        let description = MotionDescription::new(target)
            .initial(MotionInitial::Style(initial))
            .transition(linear_one_second());
        let mut track = MotionTrack::new();
        track.update(&description, ms(0));
        let style = track.sample(ms(500), false).style;
        assert_eq!(style.width, Some(50.0));
        assert_eq!(style.height, Some(25.0));
        assert_eq!(style.top, Some(5.0));
        assert_eq!(style.right, Some(10.0));
        assert_eq!(style.bottom, Some(15.0));
        assert_eq!(style.left, Some(-20.0));
        assert_eq!(style.border_radius, Some(4.0));
        assert_eq!(style.opacity, Some(0.75));

        let mut refinement = crate::StyleRefinement::default();
        refinement.opacity = Some(0.5);
        style.apply(&mut refinement);
        assert_eq!(refinement.opacity, Some(0.375));
        assert_eq!(
            refinement.size.width,
            Some(crate::Length::from(crate::px(50.0)))
        );
    }

    #[test]
    fn exit_replaces_animate_while_not_present() {
        let description = MotionDescription::new(MotionProps::new().opacity(1.0))
            .exit(MotionProps::new().opacity(0.0));
        assert_eq!(description.target().opacity, Some(1.0));
        let exiting = description.present(false);
        assert!(exiting.is_exit());
        assert_eq!(exiting.target().opacity, Some(0.0));
        // Without an exit target the exit is a no-op on animate.
        let no_exit = MotionDescription::new(MotionProps::new().opacity(1.0)).present(false);
        assert_eq!(no_exit.target().opacity, Some(1.0));
    }
}

mod glide_tests {
    use super::*;

    const D420: Duration = Duration::from_millis(420);

    fn closed_form(x: f64, v: f64, target: f64, duration: f64, dt: f64) -> (f64, f64) {
        let w = 12.0 / duration;
        let d = x - target;
        let c = v + w * d;
        let decay = (-w * dt).exp();
        (target + (d + c * dt) * decay, (v - w * c * dt) * decay)
    }

    #[test]
    fn step_matches_the_closed_form() {
        for (x, v, target, dt) in [
            (10.0, 3.0, 0.0, 0.1),
            (-250.0, 900.0, 40.0, 0.016),
            (0.0, 0.0, 2000.0, 0.2),
            (5.0, -50.0, 5.0, 0.05),
        ] {
            let stepped = Glide {
                value: x as f32,
                velocity: v as f32,
            }
            .step(target as f32, D420, dt as f32);
            let (expected_x, expected_v) = closed_form(x, v, target, 0.42, dt);
            assert!(
                (f64::from(stepped.value) - expected_x).abs() < 1e-3,
                "{x} {v}"
            );
            assert!(
                (f64::from(stepped.velocity) - expected_v).abs() < 1e-2,
                "{x} {v}"
            );
        }
        assert_eq!(Glide::angular_rate(D420), 12.0 / 0.42);
    }

    #[test]
    fn step_is_frame_rate_independent() {
        let start = Glide {
            value: 100.0,
            velocity: -40.0,
        };
        let once = start.step(0.0, D420, 0.12);
        let mut many = start;
        for _ in 0..12 {
            many = many.step(0.0, D420, 0.01);
        }
        assert!((once.value - many.value).abs() < 1e-2);
        assert!((once.velocity - many.velocity).abs() < 1e-1);
    }

    #[test]
    fn retargeting_preserves_position_and_velocity() {
        let mut glide = Glide::at(0.0);
        glide.advance(500.0, D420, 0.1);
        let before = glide;
        assert!(before.value > 0.0 && before.velocity > 0.0);
        // Retargeting at an arbitrary instant changes nothing about the state itself...
        let unchanged = before.step(-300.0, D420, 0.0);
        assert_eq!(unchanged, before);
        // ...and the next small step is continuous in both position and velocity.
        let next = before.step(-300.0, D420, 1e-6);
        assert!((next.value - before.value).abs() < 0.01, "position jumped");
        assert!(
            (next.velocity - before.velocity).abs() < 5.0,
            "velocity jumped"
        );
    }

    #[test]
    fn reversal_mid_flight_does_not_oscillate() {
        let mut glide = Glide::at(0.0);
        for _ in 0..6 {
            glide.advance(1000.0, D420, 1.0 / 60.0);
        }
        let reversal_value = glide.value;
        assert!(glide.velocity > 0.0);
        let mut peak = reversal_value;
        let mut previous = glide.value;
        let mut turned = false;
        for _ in 0..240 {
            if !glide.advance(0.0, D420, 1.0 / 60.0) {
                break;
            }
            assert!(glide.value >= 0.0, "crossed the target: {}", glide.value);
            if glide.value > previous && !turned {
                peak = glide.value;
            } else {
                turned = true;
                assert!(glide.value <= previous + 1e-4, "oscillation");
            }
            previous = glide.value;
        }
        assert!(peak >= reversal_value, "velocity carried the glide forward");
        assert_eq!(glide, Glide::at(0.0), "snapped exactly to the new target");
    }

    #[test]
    fn terminates_below_thresholds_with_an_exact_snap() {
        let mut glide = Glide::at(10.0);
        let mut frames = 0;
        while glide.advance(0.0, D420, 1.0 / 60.0) {
            frames += 1;
            assert!(frames < 600, "never settled");
        }
        assert_eq!(glide.value, 0.0);
        assert_eq!(glide.velocity, 0.0);
        assert!(!glide.is_active(0.0));
        // No further frames once settled.
        assert!(!glide.advance(0.0, D420, 1.0 / 60.0));
        assert!(
            Glide {
                value: 0.001,
                velocity: 0.0
            }
            .is_active(0.0)
        );
        assert!(
            !Glide {
                value: 0.0004,
                velocity: 0.004
            }
            .is_active(0.0)
        );
        assert!(
            Glide {
                value: 0.0,
                velocity: 0.006
            }
            .is_active(0.0)
        );
    }

    #[test]
    fn a_420ms_glide_across_2000_pixels_settles_within_a_fraction_of_a_pixel() {
        let settled = Glide::at(0.0).step(2000.0, D420, 0.42);
        assert!((2000.0 - settled.value).abs() < 0.5, "{}", settled.value);
        // The glide is monotonic (never overshoots) from rest.
        let mut glide = Glide::at(0.0);
        let mut previous = 0.0;
        for _ in 0..40 {
            glide.advance(2000.0, D420, 0.01);
            assert!(glide.value >= previous && glide.value <= 2000.0);
            previous = glide.value;
        }
    }

    #[test]
    fn zero_duration_snaps_and_reduced_motion_zeroes_both_glides() {
        let mut glide = Glide {
            value: 5.0,
            velocity: 9.0,
        };
        assert!(!glide.advance(1.0, Duration::ZERO, 0.016));
        assert_eq!(glide, Glide::at(1.0));

        let mut travel = TravelGlide::default();
        travel.displace([-120.0, 40.0], true);
        travel.displace([30.0, 0.0], false);
        assert_eq!(travel.displacement(), [-90.0, 40.0]);
        let active = travel.advance(D420, ms(200), 0.0, true);
        assert!(!active);
        assert_eq!(travel.displacement(), [0.0, 0.0]);
    }

    #[test]
    fn reflow_never_touches_the_route_glide() {
        let mut travel = TravelGlide::default();
        travel.displace([-200.0, 0.0], true);
        travel.advance(D420, ms(200), 0.1, false);
        let route = travel.route;
        travel.displace([50.0, 0.0], false);
        assert_eq!(
            travel.route, route,
            "reflow must not restart or retarget route"
        );
        travel.advance(D420, ms(200), 0.05, false);
        let mut route_only = TravelGlide {
            route,
            ..TravelGlide::default()
        };
        route_only.advance(D420, ms(200), 0.05, false);
        assert_eq!(travel.route, route_only.route);
    }
}

mod preset_tests {
    use super::*;

    #[test]
    fn presets_carry_the_spec_parameters() {
        assert_eq!(presets::ENTRANCE.millis(), 500);
        assert_eq!(
            presets::ENTRANCE.easing,
            Easing::cubic_bezier(0.16, 1.0, 0.3, 1.0)
        );
        assert_eq!(presets::ENTRANCE_RISE_PX, 4.0);
        assert_eq!(presets::QUICK_FADE.millis(), 150);
        assert_eq!(presets::MENU_ENTRY.millis(), 140);
        assert_eq!(presets::MENU_EXIT.millis(), 100);
        assert_eq!(presets::DIALOG_ENTRY.millis(), 180);
        assert_eq!(presets::SIZE_TRANSITION.millis(), 200);
        assert_eq!(presets::DOCKING_GLIDE.millis(), 420);
        assert_eq!(presets::RETURN_GLIDE.millis(), 470);
        assert_eq!(presets::BACKGROUND_CROSSFADE.millis(), 180);
        assert_eq!(presets::LABEL_ROLL.millis(), 550);
        assert_eq!(presets::LABEL_TRAVEL_EM, 0.35);
        assert_eq!(presets::LABEL_REST_SCALE, 0.6);
        assert_eq!(presets::LABEL_TILT_DEGREES, 2.0);
        assert_eq!(presets::LABEL_BLUR_EM, 0.1);
        assert_eq!(presets::LABEL_STAGGER, 0.3);
        let defaults = MotionTransition::default();
        assert_eq!((defaults.duration, defaults.delay), (0.3, 0.0));
        assert_eq!(defaults.easing, Easing::EASE_OUT);
    }

    #[test]
    fn timeline_scale_multiplies_preset_durations_and_clamps() {
        // The scale is process-global: restore it before asserting anything else.
        set_timeline_scale(2.0);
        let doubled = presets::QUICK_FADE.duration();
        set_timeline_scale(1000.0);
        let clamped_high = timeline_scale();
        set_timeline_scale(0.0);
        let clamped_low = timeline_scale();
        set_timeline_scale(f32::NAN);
        let ignored = timeline_scale();
        set_timeline_scale(1.0);
        assert_eq!(doubled, ms(300));
        assert_eq!(clamped_high, 100.0);
        assert_eq!(clamped_low, 0.01);
        assert_eq!(ignored, 0.01);
        assert_eq!(timeline_scale(), 1.0);
        assert_eq!(presets::QUICK_FADE.duration(), ms(150));
    }
}

mod presence_tests {
    use super::*;

    type Manager = PresenceManager<&'static str, u32>;

    fn child(
        key: &'static str,
        signature: u64,
        has_exit: bool,
        payload: u32,
    ) -> PresenceChild<&'static str, u32> {
        PresenceChild {
            key,
            has_exit,
            signature,
            payload,
        }
    }

    fn keys(items: &[PresenceItem<&'static str, u32>]) -> Vec<(&'static str, bool)> {
        items.iter().map(|item| (item.key, item.present)).collect()
    }

    fn generation(manager: &Manager, key: &str) -> u64 {
        manager
            .items()
            .into_iter()
            .find(|item| item.key == key)
            .unwrap()
            .generation
    }

    fn manager_with_abc() -> Manager {
        let mut manager = Manager::new(true, PresenceFlavor::REACT);
        manager.reconcile(vec![
            child("a", 1, true, 0),
            child("b", 1, true, 0),
            child("c", 1, true, 0),
        ]);
        manager
    }

    #[test]
    fn removed_child_is_retained_in_place_until_native_completion() {
        let mut manager = manager_with_abc();
        let items = manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        assert_eq!(keys(&items), [("a", true), ("b", false), ("c", true)]);
        assert!(manager.has_exiting());
        // Effects alone do not release a child that has an exit animation.
        assert_eq!(manager.after_effects(), PresenceOutcome::default());

        let exit_generation = generation(&manager, "b");
        let outcome = manager.motion_complete(&"b", exit_generation);
        assert_eq!(outcome.removed, ["b"]);
        assert_eq!(outcome.motion_callbacks, [("b", exit_generation)]);
        assert!(outcome.exit_complete);
        assert_eq!(keys(&manager.items()), [("a", true), ("c", true)]);
    }

    #[test]
    fn siblings_keep_their_positions_while_an_exit_runs() {
        let mut manager = manager_with_abc();
        manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        // Unrelated children changing props never move the exiting one.
        let items = manager.reconcile(vec![child("a", 1, true, 5), child("c", 2, true, 6)]);
        assert_eq!(keys(&items), [("a", true), ("b", false), ("c", true)]);
        // A new child appended after the exiting one leaves it in place.
        let items = manager.reconcile(vec![
            child("a", 1, true, 5),
            child("c", 2, true, 6),
            child("d", 1, true, 0),
        ]);
        assert_eq!(
            keys(&items),
            [("a", true), ("b", false), ("c", true), ("d", true)]
        );
        // Removing the first child keeps both exits at their old indices.
        let items = manager.reconcile(vec![child("c", 2, true, 6), child("d", 1, true, 0)]);
        assert_eq!(
            keys(&items),
            [("a", false), ("b", false), ("c", true), ("d", true)]
        );
    }

    #[test]
    fn current_children_use_their_latest_props() {
        let mut manager = manager_with_abc();
        let items = manager.reconcile(vec![child("a", 1, true, 10), child("b", 1, true, 20)]);
        let payloads: Vec<_> = items.iter().map(|item| (item.key, item.payload)).collect();
        assert_eq!(payloads, [("a", 10), ("b", 20), ("c", 0)]);
        let items = manager.reconcile(vec![
            child("a", 1, true, 11),
            child("b", 1, true, 21),
            child("c", 1, true, 31),
        ]);
        let payloads: Vec<_> = items.iter().map(|item| item.payload).collect();
        assert_eq!(payloads, [11, 21, 31]);
    }

    #[test]
    fn initial_false_suppresses_enter_only_for_first_render_children() {
        let mut manager = Manager::new(false, PresenceFlavor::REACT);
        let items = manager.reconcile(vec![child("a", 1, true, 0), child("b", 1, true, 0)]);
        assert!(items.iter().all(|item| item.suppress_enter));
        let items = manager.reconcile(vec![
            child("a", 1, true, 0),
            child("b", 1, true, 0),
            child("c", 1, true, 0),
        ]);
        let flags: Vec<_> = items
            .iter()
            .map(|item| (item.key, item.suppress_enter))
            .collect();
        assert_eq!(flags, [("a", true), ("b", true), ("c", false)]);

        let mut manager = Manager::new(true, PresenceFlavor::REACT);
        let items = manager.reconcile(vec![child("a", 1, true, 0)]);
        assert!(!items[0].suppress_enter);
    }

    #[test]
    fn stale_generations_neither_call_user_code_nor_release_presence() {
        let mut manager = manager_with_abc();
        let enter_generation = generation(&manager, "b");
        manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        let exit_generation = generation(&manager, "b");
        assert_ne!(enter_generation, exit_generation);

        let stale = manager.motion_complete(&"b", enter_generation);
        assert_eq!(stale, PresenceOutcome::default());
        assert!(manager.has_exiting());
        assert_eq!(
            manager.motion_complete(&"zzz", 1),
            PresenceOutcome::default()
        );

        let current = manager.motion_complete(&"b", exit_generation);
        assert_eq!(current.removed, ["b"]);
        // Completing again (the entry is gone) is rejected.
        assert_eq!(
            manager.motion_complete(&"b", exit_generation),
            PresenceOutcome::default()
        );
    }

    #[test]
    fn a_changed_target_gets_a_new_generation_so_old_completions_are_stale() {
        let mut manager = manager_with_abc();
        let before = generation(&manager, "a");
        manager.reconcile(vec![
            child("a", 1, true, 9),
            child("b", 1, true, 0),
            child("c", 1, true, 0),
        ]);
        assert_eq!(
            generation(&manager, "a"),
            before,
            "props alone keep the generation"
        );
        manager.reconcile(vec![
            child("a", 2, true, 9),
            child("b", 1, true, 0),
            child("c", 1, true, 0),
        ]);
        let after = generation(&manager, "a");
        assert_ne!(before, after);
        // A current completion of an entering child calls user code but removes nothing.
        let outcome = manager.motion_complete(&"a", after);
        assert_eq!(outcome.motion_callbacks, [("a", after)]);
        assert!(outcome.removed.is_empty());
        assert_eq!(
            manager.motion_complete(&"a", before),
            PresenceOutcome::default()
        );
    }

    #[test]
    fn reentering_child_cancels_its_exit() {
        let mut manager = manager_with_abc();
        manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        let exit_generation = generation(&manager, "b");
        let items = manager.reconcile(vec![
            child("a", 1, true, 0),
            child("b", 1, true, 1),
            child("c", 1, true, 0),
        ]);
        assert_eq!(keys(&items), [("a", true), ("b", true), ("c", true)]);
        assert!(!manager.has_exiting());
        assert_ne!(generation(&manager, "b"), exit_generation);
        // The cancelled exit's completion can no longer remove the child.
        assert_eq!(
            manager.motion_complete(&"b", exit_generation),
            PresenceOutcome::default()
        );
        assert_eq!(keys(&manager.items()).len(), 3);
    }

    #[test]
    fn exit_without_a_target_is_removable_without_native_completion() {
        let mut manager = Manager::new(true, PresenceFlavor::REACT);
        manager.reconcile(vec![child("a", 1, false, 0), child("b", 1, false, 0)]);
        manager.reconcile(vec![child("a", 1, false, 0)]);
        let outcome = manager.after_effects();
        assert_eq!(outcome.removed, ["b"]);
        assert!(outcome.exit_complete);
        assert_eq!(keys(&manager.items()), [("a", true)]);
    }

    #[test]
    fn exits_are_aggregated_before_the_root_callback_and_removal() {
        let mut manager = manager_with_abc();
        manager.reconcile(vec![child("a", 1, true, 0)]);
        let (gb, gc) = (generation(&manager, "b"), generation(&manager, "c"));
        let first = manager.motion_complete(&"b", gb);
        assert_eq!(first.motion_callbacks, [("b", gb)]);
        assert!(first.removed.is_empty() && !first.exit_complete);
        assert_eq!(
            keys(&manager.items()),
            [("a", true), ("b", false), ("c", false)]
        );
        let second = manager.motion_complete(&"c", gc);
        assert_eq!(second.motion_callbacks, [("c", gc)]);
        assert_eq!(second.removed, ["b", "c"]);
        assert!(second.exit_complete);
        assert_eq!(keys(&manager.items()), [("a", true)]);
    }

    #[test]
    fn individual_mode_releases_each_exit_as_it_completes() {
        let mut manager = Manager::new(true, PresenceFlavor::SOLID);
        manager.reconcile(vec![
            child("a", 1, true, 0),
            child("b", 1, true, 0),
            child("c", 1, true, 0),
        ]);
        manager.reconcile(vec![child("a", 1, true, 0)]);
        let gb = generation(&manager, "b");
        let first = manager.motion_complete(&"b", gb);
        assert_eq!(first.removed, ["b"]);
        assert!(!first.exit_complete);
        assert_eq!(keys(&manager.items()), [("a", true), ("c", false)]);
    }

    fn solid_abcd() -> Manager {
        let mut manager = Manager::new(true, PresenceFlavor::SOLID);
        manager.reconcile(vec![
            child("a", 1, true, 0),
            child("b", 1, true, 0),
            child("c", 1, true, 0),
            child("d", 1, true, 0),
        ]);
        manager
    }

    #[test]
    fn solid_flavor_orders_current_children_before_retained_exits() {
        let mut manager = solid_abcd();
        // Retained exit nodes follow the current children (React would keep them in place).
        let items = manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        assert_eq!(
            keys(&items),
            [("a", true), ("c", true), ("b", false), ("d", false)]
        );

        // Reordering and adding current children never lets an exit move in front of them.
        let items = manager.reconcile(vec![
            child("c", 1, true, 0),
            child("a", 1, true, 0),
            child("e", 1, true, 0),
        ]);
        assert_eq!(
            keys(&items),
            [
                ("c", true),
                ("a", true),
                ("e", true),
                ("b", false),
                ("d", false)
            ]
        );

        // A further removal joins the exits, in previous relative order.
        let items = manager.reconcile(vec![child("c", 1, true, 0), child("a", 1, true, 0)]);
        assert_eq!(
            keys(&items),
            [
                ("c", true),
                ("a", true),
                ("e", false),
                ("b", false),
                ("d", false)
            ]
        );

        // Reentry moves the child back among the current ones, ahead of the remaining exits.
        let items = manager.reconcile(vec![
            child("c", 1, true, 0),
            child("d", 1, true, 0),
            child("a", 1, true, 0),
        ]);
        assert_eq!(
            keys(&items),
            [
                ("c", true),
                ("d", true),
                ("a", true),
                ("e", false),
                ("b", false)
            ]
        );
    }

    #[test]
    fn solid_exits_reject_stale_completions_and_reentry_cancels_removal() {
        let mut manager = solid_abcd();
        manager.reconcile(vec![child("a", 1, true, 0)]);
        let (gb, gc) = (generation(&manager, "b"), generation(&manager, "c"));
        assert_eq!(
            keys(&manager.items()),
            [("a", true), ("b", false), ("c", false), ("d", false)]
        );

        // "b" returns: its exit is cancelled and the old exit generation no longer counts.
        manager.reconcile(vec![child("a", 1, true, 0), child("b", 1, true, 1)]);
        assert_ne!(generation(&manager, "b"), gb);
        assert_eq!(
            manager.motion_complete(&"b", gb),
            PresenceOutcome::default()
        );
        assert_eq!(
            keys(&manager.items()),
            [("a", true), ("b", true), ("c", false), ("d", false)]
        );
        // A stale generation for an exit that is still running is ignored too.
        assert_eq!(
            manager.motion_complete(&"c", gc + 1000),
            PresenceOutcome::default()
        );
        assert!(manager.has_exiting());

        // Current completions release each exit individually; the root callback fires last.
        let released = manager.motion_complete(&"c", gc);
        assert_eq!(released.motion_callbacks, [("c", gc)]);
        assert_eq!(released.removed, ["c"]);
        assert!(!released.exit_complete);
        let gd = generation(&manager, "d");
        let last = manager.motion_complete(&"d", gd);
        assert_eq!(last.removed, ["d"]);
        assert!(last.exit_complete);
        assert_eq!(keys(&manager.items()), [("a", true), ("b", true)]);
    }

    #[test]
    fn solid_children_without_an_exit_target_are_removed_immediately() {
        let mut manager = Manager::new(true, PresenceFlavor::SOLID);
        manager.reconcile(vec![child("plain", 1, false, 0), child("host", 1, true, 0)]);
        let items = manager.reconcile(vec![]);
        assert_eq!(keys(&items), [("plain", false), ("host", false)]);
        // The exit-less child goes at once; the host with an exit target waits for completion.
        let outcome = manager.after_effects();
        assert_eq!(outcome.removed, ["plain"]);
        assert!(!outcome.exit_complete);
        assert_eq!(keys(&manager.items()), [("host", false)]);
        let g = generation(&manager, "host");
        assert_eq!(manager.motion_complete(&"host", g).removed, ["host"]);
    }

    #[test]
    fn flavors_are_constructible_and_documented_combinations() {
        assert_eq!(PresenceFlavor::REACT.order, PresenceOrder::Positional);
        assert_eq!(PresenceFlavor::REACT.removal, RemovalMode::Aggregate);
        assert_eq!(
            PresenceFlavor::SOLID.order,
            PresenceOrder::ExitsAfterCurrent
        );
        assert_eq!(PresenceFlavor::SOLID.removal, RemovalMode::Individual);
    }

    #[test]
    fn exit_waits_for_every_registered_participant_of_the_same_cycle() {
        let mut manager = manager_with_abc();
        let p1 = manager.register_participant(&"b").unwrap();
        let p2 = manager.register_participant(&"b").unwrap();
        manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        let g = generation(&manager, "b");

        // Native completion alone is not enough while participants are pending.
        assert!(manager.motion_complete(&"b", g).removed.is_empty());
        assert!(manager.participant_complete(&"b", p1, g).removed.is_empty());
        // Completed participants stay completed across unrelated exit renders.
        manager.reconcile(vec![child("a", 1, true, 1), child("c", 1, true, 2)]);
        // Stale generation and unregistered participants are ignored.
        let stale = manager.participant_complete(&"b", p2, g + 100);
        assert_eq!(stale, PresenceOutcome::default());
        let stranger = manager.participant_complete(&"b", ParticipantId(9999), g);
        assert_eq!(stranger, PresenceOutcome::default());
        assert!(manager.has_exiting());

        let outcome = manager.participant_complete(&"b", p2, g);
        assert_eq!(outcome.removed, ["b"]);
    }

    #[test]
    fn unregistering_the_final_participant_queues_a_completion_check() {
        let mut manager = manager_with_abc();
        let participant = manager.register_participant(&"b").unwrap();
        manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        let g = generation(&manager, "b");
        manager.motion_complete(&"b", g);
        assert!(manager.has_exiting(), "the participant has not finished");
        assert!(manager.unregister_participant(&"b", participant));
        // Nothing happens until the queued (microtask) check runs.
        assert!(manager.has_exiting());
        let outcome = manager.run_queued_check();
        assert_eq!(outcome.removed, ["b"]);
        assert_eq!(manager.run_queued_check(), PresenceOutcome::default());
        // Unregistering an unknown participant queues nothing.
        assert!(!manager.unregister_participant(&"a", ParticipantId(1)));
    }

    #[test]
    fn exit_committed_before_the_first_native_frame_still_completes() {
        // The exiting child's native track never became active (its start equals its target and
        // it never saw a frame), yet it reports completion on its first sample.
        let mut manager = manager_with_abc();
        manager.reconcile(vec![child("a", 1, true, 0), child("c", 1, true, 0)]);
        let g = generation(&manager, "b");

        let mut track = MotionTrack::new();
        let exit = MotionDescription::new(MotionProps::new().opacity(1.0))
            .exit(MotionProps::new().opacity(1.0))
            .present(false)
            .generation(g);
        track.update(&exit, ms(0));
        let completion = track
            .sample(ms(0), false)
            .completion
            .expect("completion owed");
        assert_eq!(
            completion,
            MotionCompletion {
                generation: g,
                exit: true
            }
        );
        let outcome = manager.motion_complete(&"b", completion.generation);
        assert_eq!(outcome.removed, ["b"]);
    }

    #[test]
    fn outside_a_presence_context_children_are_present_with_no_callback() {
        assert!(use_is_present(None));
        let (present, remove) = use_presence(None);
        assert!(present);
        assert!(remove.is_none());
        let default = PresenceContext::default();
        assert!(default.is_present && default.safe_to_remove.is_none());

        let called = std::rc::Rc::new(std::cell::Cell::new(0));
        let context = PresenceContext {
            is_present: false,
            safe_to_remove: Some({
                let called = called.clone();
                std::rc::Rc::new(move || called.set(called.get() + 1))
            }),
        };
        assert!(!use_is_present(Some(&context)));
        let (present, remove) = use_presence(Some(&context));
        assert!(!present);
        (remove.unwrap())();
        assert_eq!(called.get(), 1);
    }

    #[test]
    fn lifecycle_states_completion_fallback_and_reopen() {
        let mut lifecycle = PresenceLifecycle::new(ms(140));
        assert_eq!(lifecycle.state(), PresenceState::Open);
        assert!(lifecycle.is_interactive() && lifecycle.is_painted());

        let generation = lifecycle.close(ms(1000));
        assert_eq!(lifecycle.state(), PresenceState::Closing);
        assert!(lifecycle.is_painted() && !lifecycle.is_interactive());
        // The fallback never fires early.
        assert!(!lifecycle.poll_fallback(ms(1139)));
        assert_eq!(lifecycle.state(), PresenceState::Closing);
        // A stale generation does not unmount.
        assert!(!lifecycle.complete(generation + 1));
        assert_eq!(lifecycle.state(), PresenceState::Closing);
        // Reopening while closing cancels the exit and rejects its completion.
        lifecycle.open();
        assert_eq!(lifecycle.state(), PresenceState::Open);
        assert!(!lifecycle.complete(generation));

        let generation = lifecycle.close(ms(2000));
        assert!(lifecycle.complete(generation));
        assert_eq!(lifecycle.state(), PresenceState::Unmounted);
        assert!(!lifecycle.is_painted());
        assert!(!lifecycle.poll_fallback(ms(9999)));

        let mut lifecycle = PresenceLifecycle::new(ms(140));
        lifecycle.close(ms(0));
        assert!(lifecycle.poll_fallback(ms(140)));
        assert_eq!(lifecycle.state(), PresenceState::Unmounted);
    }
}

mod label_logic_tests {
    use super::label::*;
    use super::*;
    use crate::{FontId, GlyphId, SharedString};

    #[test]
    fn shared_runs_follow_the_specification_examples() {
        assert_eq!(
            shared_runs("feature/a", "fix/a"),
            SharedRuns {
                leading: 1,
                trailing: 2
            }
        );
        assert_eq!(
            shared_runs("aa", "aaa"),
            SharedRuns {
                leading: 2,
                trailing: 0
            }
        );
        assert_eq!(
            shared_runs("ab", "b"),
            SharedRuns {
                leading: 0,
                trailing: 1
            }
        );
        assert_eq!(
            shared_runs("abc", "abc"),
            SharedRuns {
                leading: 3,
                trailing: 0
            }
        );
        assert_eq!(
            shared_runs("", "xyz"),
            SharedRuns {
                leading: 0,
                trailing: 0
            }
        );
        assert_eq!(
            shared_runs("x", "y"),
            SharedRuns {
                leading: 0,
                trailing: 0
            }
        );
    }

    #[test]
    fn shared_runs_split_on_whole_characters_and_never_overlap() {
        // 'é' (2 bytes) and 'è' share their first UTF-8 byte but are different characters.
        assert_eq!(
            shared_runs("é", "è"),
            SharedRuns {
                leading: 0,
                trailing: 0
            }
        );
        assert_eq!(
            shared_runs("aéz", "aèz"),
            SharedRuns {
                leading: 1,
                trailing: 1
            }
        );
        // 'あ' is three bytes: the whole character counts.
        assert_eq!(
            shared_runs("あい", "あう"),
            SharedRuns {
                leading: 3,
                trailing: 0
            }
        );
        for (old, new) in [
            ("abab", "ab"),
            ("ab", "abab"),
            ("aaaa", "aa"),
            ("xaax", "xax"),
        ] {
            let runs = shared_runs(old, new);
            assert!(
                runs.leading + runs.trailing <= old.len().min(new.len()),
                "{old} {new}"
            );
            assert!(old.is_char_boundary(runs.leading) && new.is_char_boundary(runs.leading));
            assert!(old.is_char_boundary(old.len() - runs.trailing));
            assert!(new.is_char_boundary(new.len() - runs.trailing));
        }
    }

    fn glyphs(text: &str, advance: f32, emoji_at: Option<usize>) -> Vec<LabelGlyph> {
        text.char_indices()
            .enumerate()
            .map(|(i, (index, ch))| LabelGlyph {
                font_id: FontId(0),
                glyph_id: GlyphId(ch as u32),
                x: i as f32 * advance,
                advance,
                index,
                is_emoji: emoji_at == Some(i),
            })
            .collect()
    }

    fn plan(old: &str, new: &str, emoji_at: Option<usize>) -> RollPlan {
        let (og, ng) = (glyphs(old, 10.0, emoji_at), glyphs(new, 10.0, emoji_at));
        RollPlan::new(
            (old, &og, old.chars().count() as f32 * 10.0 + 0.4),
            (new, &ng, new.chars().count() as f32 * 10.0 + 0.4),
            presets::LABEL_ROLL.duration(),
            presets::SIZE_TRANSITION.duration(),
        )
    }

    fn roles(frame: &RollFrame, role: GlyphRole) -> Vec<&FrameGlyph> {
        frame.glyphs.iter().filter(|g| g.role == role).collect()
    }

    #[test]
    fn roll_lasts_1_3_times_the_duration() {
        let plan = plan("ab", "xy", None);
        assert_eq!(plan.total_duration(), ms(715));
        assert!(plan.frame(ms(714)).is_some());
        assert!(plan.frame(ms(715)).is_none());
    }

    #[test]
    fn shared_runs_stay_put_and_only_the_middle_rolls() {
        let plan = plan("feature/a", "fix/a", None);
        let frame = plan.frame(ms(100)).unwrap();
        let leading = roles(&frame, GlyphRole::Leading);
        assert_eq!(leading.len(), 1);
        assert_eq!(leading[0].x, 0.0);
        assert!(leading[0].is_plain());
        // "ix" is new (2 glyphs), "eature" old (6 glyphs), "/a" trails.
        assert_eq!(roles(&frame, GlyphRole::New).len(), 2);
        assert_eq!(roles(&frame, GlyphRole::Old).len(), 6);
        assert_eq!(roles(&frame, GlyphRole::Trailing).len(), 2);
    }

    #[test]
    fn trailing_run_slides_with_the_size_curve_and_following_content_settles_early() {
        // "xa" (trailing "a" at x = 10) to "xyza" (trailing "a" at x = 30).
        let plan = plan("xya", "xyza", None);
        let at = |t| {
            let frame = plan.frame(ms(t)).unwrap();
            (frame.width, roles(&frame, GlyphRole::Trailing)[0].x)
        };
        // Old width 30.4 -> 31, new 40.4 -> 41.
        let (width0, x0) = at(0);
        assert_eq!(width0, 31.0);
        assert!((x0 - 20.0).abs() < 1e-4, "starts at the old x: {x0}");
        let size_midpoint = CubicBezier::new(0.0, 0.0, 0.58, 1.0).ease(0.5);
        let (width_mid, x_mid) = at(100);
        assert!((width_mid - (31.0 + 10.0 * size_midpoint)).abs() < 1e-3);
        assert!((x_mid - (30.0 - 10.0 * (1.0 - size_midpoint))).abs() < 1e-3);
        // At 200 ms the width and the trailing run are final while glyphs are still rolling.
        let (width_end, x_end) = at(200);
        assert_eq!(width_end, 41.0);
        assert!((x_end - 30.0).abs() < 1e-4);
        let (width_late, x_late) = at(600);
        assert_eq!((width_late, x_late), (41.0, 30.0));
    }

    #[test]
    fn old_and_new_glyph_motion_follows_the_spec_formulas() {
        let plan = plan("a", "b", None);
        let curve = presets::LABEL_GLYPH_CURVE;
        let duration = 0.55f32;
        let frame = plan.frame(ms(0)).unwrap();
        let old = roles(&frame, GlyphRole::Old)[0];
        assert_eq!(
            (
                old.opacity,
                old.scale,
                old.dy_em,
                old.tilt_degrees,
                old.blur_em
            ),
            (1.0, 1.0, 0.0, 0.0, 0.0)
        );
        // The incoming glyph is invisible, 0.35 em below, at the resting scale 0.6.
        let new = roles(&frame, GlyphRole::New)[0];
        assert_eq!(new.opacity, 0.0);
        assert!((new.dy_em - 0.35).abs() < 1e-6);
        assert!((new.scale - 0.6).abs() < 1e-6);
        assert!((new.tilt_degrees - 2.0).abs() < 1e-6);
        assert!((new.blur_em - 0.1).abs() < 1e-6);

        let frame = plan.frame(ms(100)).unwrap();
        let p = curve.ease(0.1 / duration);
        let old = roles(&frame, GlyphRole::Old)[0];
        assert!((old.dy_em + 0.35 * p).abs() < 1e-4);
        assert!((old.scale - (1.0 - 0.4 * p)).abs() < 1e-4);
        assert!((old.opacity - (1.0 - p)).abs() < 1e-4);
        assert!((old.tilt_degrees - 2.0 * p).abs() < 1e-4);
        assert!((old.blur_em - 0.1 * p).abs() < 1e-4);
        let new = roles(&frame, GlyphRole::New)[0];
        assert!((new.dy_em - 0.35 * (1.0 - p)).abs() < 1e-4);
        assert!((new.scale - (0.6 + 0.4 * p)).abs() < 1e-4);
        assert!((new.opacity - p).abs() < 1e-4);
        assert!((new.tilt_degrees - 2.0 * (1.0 - p)).abs() < 1e-4);
        assert!((new.blur_em - 0.1 * (1.0 - p)).abs() < 1e-4);

        // Late in the roll the old glyph is gone and the new one is at rest (plain).
        let frame = plan.frame(ms(700)).unwrap();
        assert!(roles(&frame, GlyphRole::Old)[0].opacity < 1e-3);
        let new = roles(&frame, GlyphRole::New)[0];
        assert!((new.opacity - 1.0).abs() < 1e-3);
        assert!(new.blur_em < 1e-3 && (new.scale - 1.0).abs() < 1e-3);
    }

    #[test]
    fn changed_glyphs_start_staggered_left_to_right() {
        // Four changed glyphs per side: spread = 0.55 * 0.3 * 3 / 4 = 0.12375 s.
        let plan = plan("abcd", "wxyz", None);
        let progress_at = |t: u64| -> Vec<f32> {
            plan.frame(ms(t))
                .unwrap()
                .glyphs
                .iter()
                .filter(|g| g.role == GlyphRole::New)
                .map(|g| g.opacity)
                .collect()
        };
        // At 100 ms the first three glyphs have started (their delays are 0, 41 and 83 ms) and
        // progress decreases left to right; the last glyph (delay 124 ms) has not started.
        let started = progress_at(100);
        assert!(
            started[0] > started[1] && started[1] > started[2] && started[2] > 0.0,
            "{started:?}"
        );
        assert_eq!(started[3], 0.0);
        assert!(progress_at(130)[3] > 0.0);
        // A single changed glyph per side has no stagger at all.
        let single = super::label_logic_tests::plan("a", "b", None);
        assert!(
            single
                .frame(ms(10))
                .unwrap()
                .glyphs
                .iter()
                .any(|g| g.role == GlyphRole::New && g.opacity > 0.0)
        );
    }

    #[test]
    fn emoji_swap_at_the_midpoint_and_move_by_translation_only() {
        let plan = plan("😀", "😃", Some(0));
        let curve = presets::LABEL_GLYPH_CURVE;
        // Find the time at which glyph progress crosses 1/2.
        let crossing = (0..715u64)
            .find(|t| curve.ease((*t as f32 / 1000.0) / 0.55) >= 0.5)
            .unwrap();
        let before = plan.frame(ms(crossing - 5)).unwrap();
        let after = plan.frame(ms(crossing + 5)).unwrap();
        let shown =
            |frame: &RollFrame, role| frame.glyphs.iter().filter(|g| g.role == role).count();
        assert_eq!(
            (
                shown(&before, GlyphRole::Old),
                shown(&before, GlyphRole::New)
            ),
            (1, 0)
        );
        assert_eq!(
            (shown(&after, GlyphRole::Old), shown(&after, GlyphRole::New)),
            (0, 1)
        );
        for glyph in before.glyphs.iter().chain(&after.glyphs) {
            assert_eq!(
                (
                    glyph.scale,
                    glyph.tilt_degrees,
                    glyph.blur_em,
                    glyph.opacity
                ),
                (1.0, 0.0, 0.0, 1.0)
            );
        }
        // They do translate.
        let moving = plan.frame(ms(crossing - 5)).unwrap().glyphs[0];
        assert!(moving.dy_em < 0.0);
    }

    #[test]
    fn transformation_scales_and_tilts_about_the_glyph_center() {
        let pivot = crate::point(crate::px(100.0), crate::px(50.0));
        let transform = glyph_transformation(pivot, crate::px(-7.0), 0.6, 2.0, 2.0);
        // The pivot maps to itself shifted by the rise (all in device pixels).
        let moved = transform.apply(crate::point(crate::px(200.0), crate::px(100.0)));
        assert!((f32::from(moved.x) - 200.0).abs() < 1e-3);
        assert!((f32::from(moved.y) - (100.0 - 14.0)).abs() < 1e-3);
        let identity = glyph_transformation(pivot, crate::px(0.0), 1.0, 0.0, 2.0);
        let point = identity.apply(crate::point(crate::px(33.0), crate::px(44.0)));
        assert!(
            (f32::from(point.x) - 33.0).abs() < 1e-3 && (f32::from(point.y) - 44.0).abs() < 1e-3
        );
    }

    #[test]
    fn mask_expands_by_twice_the_travel_and_the_blur_halo() {
        let bounds = crate::Bounds::new(
            crate::point(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(80.0), crate::px(20.0)),
        );
        let mask = roll_mask_bounds(bounds, crate::px(20.0));
        // Travel is 0.35 * 20 = 7 px, twice that vertically; halo is ceil(3 * 0.1 * 20) = 6 px.
        assert_eq!(mask.origin.x, crate::px(4.0));
        assert_eq!(mask.origin.y, crate::px(20.0 - 14.0));
        assert_eq!(mask.size.width, crate::px(92.0));
        assert_eq!(mask.size.height, crate::px(20.0 + 28.0));
    }

    fn text(value: &str) -> SharedString {
        SharedString::from(value.to_owned())
    }

    const TOTAL: Duration = Duration::from_millis(715);

    #[test]
    fn first_sight_of_a_key_does_not_animate() {
        let mut memory = LabelRollMemory::<u32>::default();
        assert_eq!(
            memory.observe(&1, &text("alpha"), ms(0), false, TOTAL),
            Observation::Plain
        );
        assert_eq!(
            memory.observe(&1, &text("alpha"), ms(50), false, TOTAL),
            Observation::Plain
        );
        // A change rolls from the previous text, started now.
        assert_eq!(
            memory.observe(&1, &text("beta"), ms(100), false, TOTAL),
            Observation::Roll {
                old: text("alpha"),
                started: ms(100)
            }
        );
        // The roll continues for the same text and ends after 1.3 x the duration.
        assert!(matches!(
            memory.observe(&1, &text("beta"), ms(500), false, TOTAL),
            Observation::Roll { .. }
        ));
        assert_eq!(
            memory.observe(&1, &text("beta"), ms(815), false, TOTAL),
            Observation::Plain
        );
        // Another key's first sight is plain even with different text.
        assert_eq!(
            memory.observe(&2, &text("beta"), ms(900), false, TOTAL),
            Observation::Plain
        );
    }

    #[test]
    fn reduced_motion_settles_immediately_including_mid_roll() {
        let mut memory = LabelRollMemory::<u32>::default();
        memory.observe(&1, &text("one"), ms(0), false, TOTAL);
        assert!(matches!(
            memory.observe(&1, &text("two"), ms(10), false, TOTAL),
            Observation::Roll { .. }
        ));
        // Reduced motion switched on mid-roll: no roll is shown, and none resumes.
        assert_eq!(
            memory.observe(&1, &text("two"), ms(20), true, TOTAL),
            Observation::Plain
        );
        assert_eq!(
            memory.observe(&1, &text("two"), ms(30), false, TOTAL),
            Observation::Plain
        );
        // A change under reduced motion never starts a roll.
        assert_eq!(
            memory.observe(&1, &text("three"), ms(40), true, TOTAL),
            Observation::Plain
        );
        assert_eq!(
            memory.observe(&1, &text("three"), ms(50), false, TOTAL),
            Observation::Plain
        );
    }

    #[test]
    fn at_most_256_keys_are_remembered_and_settled_keys_are_forgotten() {
        let mut memory = LabelRollMemory::<u32>::default();
        for key in 0..1000u32 {
            memory.observe(&key, &text("x"), ms(0), false, TOTAL);
            assert!(memory.len() <= MAX_REMEMBERED_KEYS);
        }
        assert!(memory.len() <= MAX_REMEMBERED_KEYS);
        // Once the limit was reached settled keys were forgotten; a forgotten key shows plainly
        // on next sight, whatever its text.
        assert_eq!(
            memory.observe(&0, &text("changed"), ms(0), false, TOTAL),
            Observation::Plain
        );

        // With every remembered key rolling, a new key is not remembered (the cap holds).
        let mut memory = LabelRollMemory::<u32>::default();
        for key in 0..MAX_REMEMBERED_KEYS as u32 {
            memory.observe(&key, &text("a"), ms(0), false, TOTAL);
            memory.observe(&key, &text("b"), ms(1), false, TOTAL);
        }
        assert_eq!(
            memory.observe(&9999, &text("new"), ms(2), false, TOTAL),
            Observation::Plain
        );
        assert_eq!(memory.len(), MAX_REMEMBERED_KEYS);
        assert!(!memory.contains(&9999));
        // Once the rolls have finished the settled keys are forgotten to make room.
        assert_eq!(
            memory.observe(&9999, &text("new"), ms(5000), false, TOTAL),
            Observation::Plain
        );
        assert!(memory.contains(&9999));
        assert!(memory.len() <= MAX_REMEMBERED_KEYS);
    }
}

mod element_tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;
    use crate::{
        AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement,
        ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestAppContext,
        VisualTestContext, Window, div, point, px, size,
    };

    /// A view whose content is built by a closure each render.
    struct TestView {
        build: Box<dyn Fn(&mut Window, &mut Context<Self>) -> AnyElement>,
    }

    impl Render for TestView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            (self.build)(window, cx)
        }
    }

    fn open(
        cx: &mut TestAppContext,
        build: impl Fn(&mut Window, &mut Context<TestView>) -> AnyElement + 'static,
    ) -> crate::WindowHandle<TestView> {
        let window = cx.open_window(size(px(600.0), px(400.0)), move |_, _| TestView {
            build: Box::new(build),
        });
        cx.run_until_parked();
        window
    }

    fn redraw(window: &crate::WindowHandle<TestView>, cx: &mut TestAppContext) {
        cx.update_window(crate::AnyWindowHandle::from(*window), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    fn bounds_of(
        window: &crate::WindowHandle<TestView>,
        cx: &mut TestAppContext,
        selector: &'static str,
    ) -> Option<crate::Bounds<crate::Pixels>> {
        window
            .update(cx, |_, window, _| {
                window.rendered_frame.debug_bounds.get(selector).copied()
            })
            .unwrap()
    }

    fn click(window: &crate::WindowHandle<TestView>, cx: &mut TestAppContext, x: f32, y: f32) {
        let mut visual = VisualTestContext::from_window((*window).into(), cx);
        visual.simulate_click(point(px(x), px(y)), crate::Modifiers::default());
    }

    #[vespertine::test]
    fn paint_opacity_multiplies_into_each_primitive_without_relayout(cx: &mut TestAppContext) {
        let window = open(cx, |_, _| {
            div()
                .size_full()
                .child(
                    div()
                        .w(px(100.0))
                        .h(px(40.0))
                        .bg(crate::rgb(0x405470))
                        .child(div().size(px(20.0)).bg(crate::rgb(0xff0000)))
                        .with_paint_opacity(0.5),
                )
                .child(
                    div()
                        .debug_selector(|| "sibling".into())
                        .w(px(30.0))
                        .h(px(10.0))
                        .bg(crate::rgb(0x00ff00)),
                )
                .into_any_element()
        });
        let quads = window
            .update(cx, |_, window, _| window.rendered_frame.scene.quads.clone())
            .unwrap();
        let alphas: Vec<f32> = quads
            .iter()
            .filter_map(|q| q.background.as_solid().map(|c| c.a))
            .collect();
        // Parent fill and the overlapping child fill are each at 0.5 (per primitive, not as a
        // group); the sibling outside the wrapper stays opaque.
        assert_eq!(
            alphas.iter().filter(|a| (**a - 0.5).abs() < 1e-6).count(),
            2,
            "{alphas:?}"
        );
        assert_eq!(
            alphas.iter().filter(|a| (**a - 1.0).abs() < 1e-6).count(),
            1,
            "{alphas:?}"
        );
        // The sibling's position is the same as without the wrapper (no relayout).
        let sibling = bounds_of(&window, cx, "sibling").unwrap();
        assert_eq!(sibling.origin.y, px(40.0));
    }

    #[vespertine::test]
    fn displacement_moves_paint_and_hitboxes_but_not_siblings(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new(0));
        let offset = Rc::new(Cell::new(point(px(0.0), px(0.0))));
        let window = open(cx, {
            let clicks = clicks.clone();
            let offset = offset.clone();
            move |_, _| {
                let clicks = clicks.clone();
                div()
                    .size_full()
                    .child(
                        div()
                            .id("target")
                            .debug_selector(|| "target".into())
                            .w(px(100.0))
                            .h(px(40.0))
                            .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                            .with_displacement(offset.get()),
                    )
                    .child(
                        div()
                            .debug_selector(|| "sibling".into())
                            .w(px(100.0))
                            .h(px(40.0)),
                    )
                    .into_any_element()
            }
        });
        let sibling_before = bounds_of(&window, cx, "sibling").unwrap();
        assert_eq!(bounds_of(&window, cx, "target").unwrap().origin.x, px(0.0));

        offset.set(point(px(150.0), px(20.0)));
        redraw(&window, cx);
        let target = bounds_of(&window, cx, "target").unwrap();
        assert_eq!(target.origin, point(px(150.0), px(20.0)));
        assert_eq!(bounds_of(&window, cx, "sibling").unwrap(), sibling_before);

        // The hitbox moved with the paint: the old place is dead, the displayed place is live.
        click(&window, cx, 10.0, 10.0);
        assert_eq!(clicks.get(), 0);
        click(&window, cx, 160.0, 30.0);
        assert_eq!(clicks.get(), 1);
    }

    #[vespertine::test]
    fn a_motion_track_samples_the_application_motion_clock_once_it_is_operated(
        cx: &mut TestAppContext,
    ) {
        let width = Rc::new(Cell::new(100.0));
        let window = open(cx, {
            let width = width.clone();
            move |_, _| {
                div()
                    .size_full()
                    .child(
                        div()
                            .debug_selector(|| "pane".into())
                            .h(px(10.0))
                            .with_motion(
                                "pane",
                                MotionDescription::new(MotionProps::new().width(width.get()))
                                    .initial(MotionInitial::False)
                                    .transition(linear_one_second()),
                            ),
                    )
                    .into_any_element()
            }
        });
        cx.update(|cx| {
            crate::MotionClock::pause(cx);
            crate::MotionClock::set(cx, 0.0);
        });
        redraw(&window, cx);
        assert_eq!(
            bounds_of(&window, cx, "pane").unwrap().size.width,
            px(100.0)
        );
        width.set(0.0);
        redraw(&window, cx);
        for (at, expected) in [(0.0, 100.0), (250.0, 75.0), (500.0, 50.0), (1000.0, 0.0)] {
            cx.update(|cx| crate::MotionClock::set(cx, at));
            redraw(&window, cx);
            assert_eq!(
                bounds_of(&window, cx, "pane").unwrap().size.width,
                px(expected),
                "at {at} ms"
            );
        }
    }

    #[vespertine::test]
    fn closing_component_stays_painted_but_inert_until_completion(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new(0));
        let lifecycle = Rc::new(std::cell::RefCell::new(PresenceLifecycle::new(ms(140))));
        let window = open(cx, {
            let clicks = clicks.clone();
            let lifecycle = lifecycle.clone();
            move |_, _| {
                let clicks = clicks.clone();
                let child = div()
                    .id("closable")
                    .debug_selector(|| "closable".into())
                    .w(px(100.0))
                    .h(px(40.0))
                    .bg(crate::rgb(0x336699))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1));
                div()
                    .size_full()
                    .child(lifecycle.borrow().render(child))
                    .child(
                        div()
                            .debug_selector(|| "sibling".into())
                            .w(px(100.0))
                            .h(px(40.0)),
                    )
                    .into_any_element()
            }
        });
        click(&window, cx, 10.0, 10.0);
        assert_eq!(clicks.get(), 1, "open components receive pointer events");
        let sibling = bounds_of(&window, cx, "sibling").unwrap();

        let generation = lifecycle.borrow_mut().close(ms(0));
        redraw(&window, cx);
        // Painted: the fill is still in the scene and the element still has bounds.
        assert!(bounds_of(&window, cx, "closable").is_some());
        let painted = window
            .update(cx, |_, window, _| {
                window
                    .rendered_frame
                    .scene
                    .quads
                    .iter()
                    .any(|q| q.background.as_solid().is_some())
            })
            .unwrap();
        assert!(painted);
        // Inert: no pointer events, siblings stable.
        click(&window, cx, 10.0, 10.0);
        assert_eq!(
            clicks.get(),
            1,
            "closing components receive no pointer events"
        );
        assert_eq!(bounds_of(&window, cx, "sibling").unwrap(), sibling);

        // A stale completion keeps it painted; the current completion unmounts it.
        assert!(!lifecycle.borrow_mut().complete(generation + 7));
        redraw(&window, cx);
        assert!(bounds_of(&window, cx, "closable").is_some());
        assert!(lifecycle.borrow_mut().complete(generation));
        redraw(&window, cx);
        assert!(bounds_of(&window, cx, "closable").is_none());
    }

    /// A route-glide scene: a pane position and a route key; the travelling input lives in an
    /// absolutely positioned container at `left`.
    struct TravelScene {
        left: Rc<Cell<f32>>,
        route: Rc<Cell<u32>>,
    }

    fn travel_window(
        cx: &mut TestAppContext,
        scene: &TravelScene,
    ) -> crate::WindowHandle<TestView> {
        let (left, route) = (scene.left.clone(), scene.route.clone());
        open(cx, move |_, _| {
            div()
                .relative()
                .size_full()
                .child(
                    div().absolute().top(px(0.0)).left(px(left.get())).child(
                        div()
                            .id("input")
                            .debug_selector(|| "input".into())
                            .w(px(100.0))
                            .h(px(40.0))
                            .with_travel("travel", route.get()),
                    ),
                )
                .into_any_element()
        })
    }

    fn input_x(window: &crate::WindowHandle<TestView>, cx: &mut TestAppContext) -> f32 {
        f32::from(bounds_of(window, cx, "input").unwrap().origin.x)
    }

    #[vespertine::test]
    fn route_change_glides_continuously_and_reversal_keeps_velocity(cx: &mut TestAppContext) {
        let scene = TravelScene {
            left: Rc::new(Cell::new(0.0)),
            route: Rc::new(Cell::new(0)),
        };
        let window = travel_window(cx, &scene);
        assert_eq!(input_x(&window, cx), 0.0);

        // The route changes: the natural position jumps to 300 but the displayed one stays put.
        scene.left.set(300.0);
        scene.route.set(1);
        redraw(&window, cx);
        assert!((input_x(&window, cx) - 0.0).abs() <= 1.0);

        let mut previous = 0.0;
        for _ in 0..6 {
            cx.executor().advance_clock(ms(16));
            redraw(&window, cx);
            let x = input_x(&window, cx);
            assert!(x >= previous - 1.0, "moves toward the destination");
            previous = x;
        }
        assert!(
            previous > 20.0 && previous < 300.0,
            "mid-flight at {previous}"
        );

        // Reverse mid-flight: the displayed position is continuous at the reversal...
        scene.left.set(0.0);
        scene.route.set(2);
        redraw(&window, cx);
        let at_reversal = input_x(&window, cx);
        assert!(
            (at_reversal - previous).abs() <= 1.0,
            "{at_reversal} vs {previous}"
        );
        // ...and velocity carries it a little further before it returns to the origin.
        let mut peak = at_reversal;
        for _ in 0..80 {
            cx.executor().advance_clock(ms(16));
            redraw(&window, cx);
            peak = peak.max(input_x(&window, cx));
        }
        assert!(peak >= at_reversal);
        assert!(
            input_x(&window, cx).abs() <= 1.0,
            "settled on the new route"
        );
        // Settled: no frames are requested any more.
        let requested = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        let _ = requested;
        redraw(&window, cx);
        let requested = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert_eq!(requested, 0);
    }

    fn reflow_experiment(reflow: bool, cx: &mut TestAppContext) -> f32 {
        let scene = TravelScene {
            left: Rc::new(Cell::new(0.0)),
            route: Rc::new(Cell::new(0)),
        };
        let window = travel_window(cx, &scene);
        scene.left.set(200.0);
        scene.route.set(1);
        redraw(&window, cx);
        cx.executor().advance_clock(ms(100));
        redraw(&window, cx);
        if reflow {
            // An unrelated pane resized: the same route is now laid out 50 px further right.
            scene.left.set(250.0);
            redraw(&window, cx);
        }
        cx.executor().advance_clock(ms(100));
        redraw(&window, cx);
        input_x(&window, cx)
    }

    #[vespertine::test]
    fn reflow_does_not_restart_the_route_glide(cx: &mut TestAppContext) {
        let without = reflow_experiment(false, cx);
        let with = reflow_experiment(true, cx);
        // The reflow shifted the natural position by 50 px; its own glide absorbed all but
        // (1 + 60 * 0.1) * exp(-6) * 50 = 0.87 px of it after 100 ms. A restarted route glide
        // would differ by tens of pixels.
        assert!(
            (with - without - 50.0).abs() < 2.0,
            "with {with} without {without}"
        );
    }

    #[vespertine::test]
    fn travel_keeps_hitboxes_on_the_displayed_surface(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new(0));
        let left = Rc::new(Cell::new(0.0f32));
        let route = Rc::new(Cell::new(0u32));
        let window = open(cx, {
            let (clicks, left, route) = (clicks.clone(), left.clone(), route.clone());
            move |_, _| {
                let clicks = clicks.clone();
                div()
                    .relative()
                    .size_full()
                    .child(
                        div().absolute().top(px(0.0)).left(px(left.get())).child(
                            div()
                                .id("input")
                                .debug_selector(|| "input".into())
                                .w(px(100.0))
                                .h(px(40.0))
                                .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                                .with_travel("travel", route.get()),
                        ),
                    )
                    .into_any_element()
            }
        });
        left.set(400.0);
        route.set(1);
        redraw(&window, cx);
        // Displayed at the old place although the layout position is 400.
        click(&window, cx, 10.0, 10.0);
        assert_eq!(clicks.get(), 1);
        click(&window, cx, 450.0, 10.0);
        assert_eq!(clicks.get(), 1);
        // After settling the surface is at its natural position.
        cx.executor().advance_clock(ms(1000));
        redraw(&window, cx);
        redraw(&window, cx);
        click(&window, cx, 450.0, 10.0);
        assert_eq!(clicks.get(), 2);
    }

    #[vespertine::test]
    fn frozen_motion_track_renders_the_fidelity_rectangle(cx: &mut TestAppContext) {
        // FID-ANM-002-1: width 0 -> 100 linear over one second, frozen at 500 ms.
        let window = open(cx, |_, _| {
            let description = MotionDescription::new(MotionProps::new().width(100.0))
                .initial(MotionInitial::Style(MotionProps::new().width(0.0)))
                .transition(linear_one_second());
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px(20.0))
                        .top(px(40.0))
                        .h(px(40.0))
                        .rounded(px(4.0))
                        .bg(crate::rgb(0x3366cc))
                        .debug_selector(|| "rect".into())
                        .with_motion("rect-motion", description)
                        .frozen_at(ms(500)),
                )
                .into_any_element()
        });
        let bounds = bounds_of(&window, cx, "rect").unwrap();
        assert_eq!(
            bounds,
            crate::Bounds::new(point(px(20.0), px(40.0)), size(px(50.0), px(40.0)))
        );
        // Frozen tracks request no frames.
        let requested = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert_eq!(requested, 0);
    }

    #[vespertine::test]
    fn live_motion_track_requests_frames_while_active_and_reports_completion_once(
        cx: &mut TestAppContext,
    ) {
        let completions = Rc::new(std::cell::RefCell::new(Vec::new()));
        let target = Rc::new(Cell::new(0.0f64));
        let generation = Rc::new(Cell::new(1u64));
        let window = open(cx, {
            let (completions, target, generation) =
                (completions.clone(), target.clone(), generation.clone());
            move |_, _| {
                let completions = completions.clone();
                let description = MotionDescription::new(MotionProps::new().width(target.get()))
                    .initial(MotionInitial::Style(MotionProps::new().width(0.0)))
                    .transition(MotionTransition::new().duration(0.2).easing(Easing::LINEAR))
                    .generation(generation.get());
                div()
                    .size_full()
                    .child(
                        div()
                            .h(px(10.0))
                            .debug_selector(|| "bar".into())
                            .with_motion("bar-motion", description)
                            .on_motion_complete(move |completion, _, _| {
                                completions.borrow_mut().push(completion)
                            }),
                    )
                    .into_any_element()
            }
        });
        // A no-op initial description (start == target, same generation) owes nothing.
        cx.run_until_parked();
        assert!(completions.borrow().is_empty());

        target.set(100.0);
        generation.set(2);
        redraw(&window, cx);
        cx.run_until_parked();
        let frames = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert!(frames > 0, "an active track requests frames");
        assert!(completions.borrow().is_empty(), "no early completion");

        cx.executor().advance_clock(ms(100));
        redraw(&window, cx);
        cx.run_until_parked();
        let width = f32::from(bounds_of(&window, cx, "bar").unwrap().size.width);
        assert!((width - 50.0).abs() < 1.5, "{width}");
        assert!(completions.borrow().is_empty());

        cx.executor().advance_clock(ms(150));
        redraw(&window, cx);
        cx.run_until_parked();
        assert_eq!(
            *completions.borrow(),
            [MotionCompletion {
                generation: 2,
                exit: false
            }]
        );
        // Settled: more frames deliver nothing and request nothing.
        redraw(&window, cx);
        cx.run_until_parked();
        assert_eq!(completions.borrow().len(), 1);
        let _ = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        redraw(&window, cx);
        let frames = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert_eq!(frames, 0);
    }

    #[vespertine::test]
    fn exit_track_committed_before_its_first_frame_reports_completion(cx: &mut TestAppContext) {
        let completions = Rc::new(std::cell::RefCell::new(Vec::new()));
        let _window = open(cx, {
            let completions = completions.clone();
            move |_, _| {
                let completions = completions.clone();
                let description = MotionDescription::new(MotionProps::new().opacity(1.0))
                    .present(false)
                    .generation(5);
                div()
                    .size_full()
                    .child(
                        div()
                            .with_motion("exiting", description)
                            .on_motion_complete(move |completion, _, _| {
                                completions.borrow_mut().push(completion)
                            }),
                    )
                    .into_any_element()
            }
        });
        cx.run_until_parked();
        assert_eq!(
            *completions.borrow(),
            [MotionCompletion {
                generation: 5,
                exit: true
            }]
        );
    }

    #[vespertine::test]
    fn rolling_label_shows_plain_text_on_first_sight_and_rolls_on_change(cx: &mut TestAppContext) {
        let text = Rc::new(std::cell::RefCell::new("alpha".to_owned()));
        let window = open(cx, {
            let text = text.clone();
            move |_, _| {
                div()
                    .size_full()
                    .flex()
                    .child(
                        div()
                            .debug_selector(|| "label".into())
                            .child(RollingLabel::new("status", text.borrow().clone())),
                    )
                    .into_any_element()
            }
        });
        let plain_width = f32::from(bounds_of(&window, cx, "label").unwrap().size.width);
        assert!(plain_width > 0.0);
        let frames = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert_eq!(frames, 0, "first sight is plain: nothing animates");
        let leased = cx.update(|cx| cx.animation_lease_count());
        assert_eq!(leased, 0);

        *text.borrow_mut() = "alphabet soup".to_owned();
        redraw(&window, cx);
        // While rolling the element renews a lease on the shared bounded clock.
        let leased = cx.update(|cx| cx.animation_lease_count());
        assert_eq!(leased, 1);
        let rolling_width = f32::from(bounds_of(&window, cx, "label").unwrap().size.width);
        assert!(
            (rolling_width - plain_width.ceil()).abs() <= 1.0,
            "starts at the old width"
        );

        cx.executor().advance_clock(ms(800));
        redraw(&window, cx);
        let final_width = f32::from(bounds_of(&window, cx, "label").unwrap().size.width);
        assert!(final_width > plain_width, "settled at the new, wider text");
    }

    #[vespertine::test]
    fn rolling_label_settles_immediately_under_reduced_motion(cx: &mut TestAppContext) {
        let text = Rc::new(std::cell::RefCell::new("one".to_owned()));
        let window = open(cx, {
            let text = text.clone();
            move |_, _| {
                div()
                    .size_full()
                    .flex()
                    .child(
                        div()
                            .debug_selector(|| "label".into())
                            .child(RollingLabel::new("reduced", text.borrow().clone())),
                    )
                    .into_any_element()
            }
        });
        let initial = bounds_of(&window, cx, "label").unwrap().size.width;
        cx.update(|cx| cx.set_reduce_motion(true));
        *text.borrow_mut() = "one two three".to_owned();
        redraw(&window, cx);
        assert_eq!(cx.update(|cx| cx.animation_lease_count()), 0);
        let settled = bounds_of(&window, cx, "label").unwrap().size.width;
        assert!(
            settled > initial,
            "the new, wider text shows at once: {initial:?} {settled:?}"
        );
        cx.executor().advance_clock(ms(800));
        redraw(&window, cx);
        assert_eq!(bounds_of(&window, cx, "label").unwrap().size.width, settled);
    }

    /// An input handler whose input-method bounds are the bounds of the element that painted it.
    struct BoundsHandler {
        bounds: crate::Bounds<crate::Pixels>,
    }

    impl crate::InputHandler for BoundsHandler {
        fn bounds_for_range(
            &mut self,
            _range: std::ops::Range<usize>,
            _window: &mut Window,
            _cx: &mut crate::App,
        ) -> Option<crate::Bounds<crate::Pixels>> {
            Some(self.bounds)
        }

        fn selected_text_range(
            &mut self,
            _ignore_disabled_input: bool,
            _window: &mut Window,
            _cx: &mut crate::App,
        ) -> Option<crate::UTF16Selection> {
            None
        }

        fn marked_text_range(
            &mut self,
            _window: &mut Window,
            _cx: &mut crate::App,
        ) -> Option<std::ops::Range<usize>> {
            None
        }

        fn text_for_range(
            &mut self,
            _range: std::ops::Range<usize>,
            _adjusted_range: &mut Option<std::ops::Range<usize>>,
            _window: &mut Window,
            _cx: &mut crate::App,
        ) -> Option<String> {
            None
        }

        fn unmark_text(&mut self, _window: &mut Window, _cx: &mut crate::App) {}

        fn replace_text_in_range(
            &mut self,
            _range: Option<std::ops::Range<usize>>,
            _text: &str,
            _window: &mut Window,
            _cx: &mut crate::App,
        ) {
        }

        fn replace_and_mark_text_in_range(
            &mut self,
            _range: Option<std::ops::Range<usize>>,
            _new_text: &str,
            _new_selected_range: Option<std::ops::Range<usize>>,
            _window: &mut Window,
            _cx: &mut crate::App,
        ) {
        }

        fn character_index_for_point(
            &mut self,
            _point: crate::Point<crate::Pixels>,
            _window: &mut Window,
            _cx: &mut crate::App,
        ) -> Option<usize> {
            None
        }
    }

    #[vespertine::test]
    fn caret_and_input_method_bounds_follow_the_displaced_surface(cx: &mut TestAppContext) {
        let offset = Rc::new(Cell::new(point(px(0.0), px(0.0))));
        let focus = Rc::new(std::cell::RefCell::new(None::<crate::FocusHandle>));
        let window = open(cx, {
            let offset = offset.clone();
            let focus = focus.clone();
            move |_, cx| {
                let handle = focus
                    .borrow_mut()
                    .get_or_insert_with(|| cx.focus_handle())
                    .clone();
                div()
                    .size_full()
                    .child(
                        div()
                            .track_focus(&handle)
                            .child(
                                crate::canvas(
                                    |bounds, _, _| bounds,
                                    move |bounds, _, window, cx| {
                                        window.handle_input(&handle, BoundsHandler { bounds }, cx);
                                        // The caret is a 2 px bar at the left edge of the input.
                                        let caret = crate::Bounds::new(
                                            bounds.origin,
                                            size(px(2.0), bounds.size.height),
                                        );
                                        window.paint_quad(crate::fill(caret, crate::rgb(0xffffff)));
                                    },
                                )
                                .w(px(100.0))
                                .h(px(40.0)),
                            )
                            .with_displacement(offset.get()),
                    )
                    .into_any_element()
            }
        });
        let handle = focus.borrow().clone().unwrap();
        window
            .update(cx, |_, window, cx| window.focus(&handle, cx))
            .unwrap();

        let probe = |window: &crate::WindowHandle<TestView>, cx: &mut TestAppContext| {
            redraw(window, cx);
            // The platform input handler reaches back into the window, so it can only be queried
            // while the window is not being updated.
            let (caret, handler) = window
                .update(cx, |_, window, _| {
                    let caret = window
                        .rendered_frame
                        .scene
                        .quads
                        .last()
                        .map(|quad| quad.bounds.origin.x.0 / window.scale_factor());
                    (caret, window.platform_window.take_input_handler())
                })
                .unwrap();
            let ime = handler
                .and_then(|mut handler| handler.bounds_for_range(0..0))
                .map(|bounds| f32::from(bounds.origin.x));
            (caret, ime)
        };
        let (caret, ime) = probe(&window, cx);
        assert_eq!((caret, ime), (Some(0.0), Some(0.0)));
        offset.set(point(px(120.0), px(0.0)));
        let (caret, ime) = probe(&window, cx);
        assert_eq!((caret, ime), (Some(120.0), Some(120.0)));
    }

    fn animated_view(
        cx: &mut TestAppContext,
        animation: crate::Animation,
    ) -> (
        Rc<std::cell::RefCell<Vec<f32>>>,
        crate::WindowHandle<TestView>,
    ) {
        use crate::AnimationExt as _;
        let deltas = Rc::new(std::cell::RefCell::new(Vec::new()));
        let window = open(cx, {
            let deltas = deltas.clone();
            move |_, _| {
                let deltas = deltas.clone();
                div()
                    .size_full()
                    .child(div().with_animation(
                        "animated",
                        animation.clone(),
                        move |this, delta| {
                            deltas.borrow_mut().push(delta);
                            this
                        },
                    ))
                    .into_any_element()
            }
        });
        (deltas, window)
    }

    #[vespertine::test]
    fn animation_element_holds_its_start_during_the_delay_then_runs(cx: &mut TestAppContext) {
        let (deltas, window) = animated_view(
            cx,
            crate::Animation::new(Duration::from_secs(1)).with_delay(ms(500)),
        );
        let last = |deltas: &Rc<std::cell::RefCell<Vec<f32>>>| *deltas.borrow().last().unwrap();
        assert_eq!(last(&deltas), 0.0);
        // During the delay frames keep being requested and the start state is held.
        cx.executor().advance_clock(ms(300));
        redraw(&window, cx);
        assert_eq!(last(&deltas), 0.0);
        assert!(
            window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap()
                > 0
        );
        // 1000 ms after the start: (1000 - 500) / 1000.
        cx.executor().advance_clock(ms(700));
        redraw(&window, cx);
        assert!((last(&deltas) - 0.5).abs() < 1e-4, "{}", last(&deltas));
        // Finished: the end state, and no further frames.
        cx.executor().advance_clock(ms(600));
        redraw(&window, cx);
        assert_eq!(last(&deltas), 1.0);
        let _ = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        redraw(&window, cx);
        assert_eq!(
            window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap(),
            0
        );
    }

    #[vespertine::test]
    fn animation_element_renders_end_or_start_state_under_reduced_motion(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (one_shot, window) = animated_view(
            cx,
            crate::Animation::new(Duration::from_secs(1)).with_delay(ms(500)),
        );
        assert_eq!(*one_shot.borrow().last().unwrap(), 1.0);
        assert_eq!(
            window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap(),
            0
        );
        let (repeating, window) =
            animated_view(cx, crate::Animation::new(Duration::from_secs(1)).repeat());
        assert_eq!(*repeating.borrow().last().unwrap(), 0.0);
        assert_eq!(
            window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap(),
            0
        );
    }
}

/// Mirrors `examples/presence_list.rs` (ANM-003 A2) on the test dispatcher.
mod presence_list_tests {
    use super::*;
    use crate::{
        Context, IntoElement, ParentElement as _, Render, ScrollStrategy, Styled as _,
        TestAppContext, UniformListScrollHandle, Window, div, px, size, uniform_list,
    };

    const ROWS: u32 = 40;
    const FALLBACK_GRACE: Duration = Duration::from_millis(150);

    struct PresenceList {
        manager: PresenceManager<u32>,
        rows: Vec<u32>,
        scroll: UniformListScrollHandle,
        native_completions: Vec<u32>,
        fallback_completions: Vec<u32>,
    }

    impl PresenceList {
        fn new() -> Self {
            let mut this = Self {
                manager: PresenceManager::new(
                    true,
                    PresenceFlavor {
                        order: PresenceOrder::Positional,
                        removal: RemovalMode::Individual,
                    },
                ),
                rows: (0..ROWS).collect(),
                scroll: UniformListScrollHandle::new(),
                native_completions: Vec::new(),
                fallback_completions: Vec::new(),
            };
            this.reconcile();
            this
        }

        fn reconcile(&mut self) {
            self.manager.reconcile(
                self.rows
                    .iter()
                    .map(|&key| PresenceChild {
                        key,
                        has_exit: true,
                        signature: 1,
                        payload: (),
                    })
                    .collect(),
            );
            self.manager.after_effects();
        }

        fn retained(&self) -> Vec<u32> {
            self.manager
                .items()
                .iter()
                .filter(|item| !item.present)
                .map(|item| item.key)
                .collect()
        }

        fn remove(&mut self, keys: &[u32], cx: &mut Context<Self>) {
            self.rows.retain(|key| !keys.contains(key));
            self.reconcile();
            let exiting: Vec<(u32, u64)> = self
                .manager
                .items()
                .into_iter()
                .filter(|item| !item.present && keys.contains(&item.key))
                .map(|item| (item.key, item.generation))
                .collect();
            spawn_fallback_timer(
                presets::MENU_EXIT.duration() + FALLBACK_GRACE,
                cx,
                move |this: &mut Self, cx| {
                    for (key, generation) in exiting {
                        let outcome = this.manager.motion_complete(&key, generation);
                        this.fallback_completions.extend(outcome.removed);
                    }
                    cx.notify();
                },
            );
            cx.notify();
        }
    }

    impl Render for PresenceList {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let count = self.manager.items().len();
            div().size_full().child(
                uniform_list(
                    "rows",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                        let items = this.manager.items();
                        let view = cx.weak_entity();
                        range
                            .map(|ix| {
                                let item = &items[ix];
                                let key = item.key;
                                let description =
                                    MotionDescription::new(MotionProps::new().opacity(1.0))
                                        .exit(MotionProps::new().opacity(0.0))
                                        .present(item.present)
                                        .generation(item.generation)
                                        .transition(presets::MENU_EXIT.transition());
                                let view = view.clone();
                                PaintScope::new(
                                    div()
                                        .h(px(28.0))
                                        .child(format!("Row {key}"))
                                        .with_motion(("row", key as usize), description)
                                        .on_motion_complete(move |completion, _, cx| {
                                            view.update(cx, |this, cx| {
                                                let outcome = this
                                                    .manager
                                                    .motion_complete(&key, completion.generation);
                                                this.native_completions.extend(outcome.removed);
                                                cx.notify();
                                            })
                                            .ok();
                                        }),
                                )
                                .inert(!item.present)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .h_full(),
            )
        }
    }

    #[vespertine::test]
    fn offscreen_exiting_rows_finish_instead_of_being_retained_forever(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(300.0), px(200.0)), |_, _| PresenceList::new());
        cx.run_until_parked();

        // Scroll to the bottom: rows 10..=12 are no longer painted. (The list lays out its first
        // row to measure item height, so the rows under test avoid index 0.)
        window
            .update(cx, |view, _, cx| {
                view.scroll
                    .scroll_to_item(ROWS as usize - 1, ScrollStrategy::Top);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();

        let offset = window
            .read_with(cx, |view, _| view.scroll.0.borrow().base_handle.offset().y)
            .unwrap();
        assert!(
            offset < px(-500.0),
            "the list scrolled away from rows 10..=12: {offset:?}"
        );
        // Three offscreen rows and one visible row start their exit.
        window
            .update(cx, |view, _, cx| view.remove(&[10, 11, 12, ROWS - 1], cx))
            .unwrap();
        cx.run_until_parked();
        let retained = window.read_with(cx, |view, _| view.retained()).unwrap();
        assert_eq!(
            retained,
            [10, 11, 12, ROWS - 1],
            "exiting rows keep their positions"
        );

        // The visible row finishes through its native exit completion...
        cx.executor()
            .advance_clock(presets::MENU_EXIT.duration() + ms(20));
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
        let (retained, native, fallback) = window
            .read_with(cx, |view, _| {
                (
                    view.retained(),
                    view.native_completions.clone(),
                    view.fallback_completions.clone(),
                )
            })
            .unwrap();
        assert_eq!(
            native,
            vec![ROWS - 1],
            "only the painted row completed natively"
        );
        assert!(fallback.is_empty());
        assert_eq!(
            retained,
            [10, 11, 12],
            "unpainted rows never saw a native frame"
        );

        // ...while the unpainted ones finish through the delayed fallback.
        cx.executor().advance_clock(FALLBACK_GRACE);
        cx.run_until_parked();
        let (retained, native, fallback) = window
            .read_with(cx, |view, _| {
                (
                    view.retained(),
                    view.native_completions.clone(),
                    view.fallback_completions.clone(),
                )
            })
            .unwrap();
        assert!(retained.is_empty(), "retained forever: {retained:?}");
        assert_eq!((native, fallback), (vec![ROWS - 1], vec![10, 11, 12]));
    }
}
