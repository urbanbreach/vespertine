use crate::{BackgroundExecutor, Task};
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
    task,
    time::Duration,
};

/// A helper trait for building complex objects with imperative conditionals in a fluent style.
pub trait FluentBuilder {
    /// Imperatively modify self with the given closure.
    fn map<U>(self, f: impl FnOnce(Self) -> U) -> U
    where
        Self: Sized,
    {
        f(self)
    }

    /// Conditionally modify self with the given closure.
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if condition { then(this) } else { this })
    }

    /// Conditionally modify self with the given closure.
    fn when_else(
        self,
        condition: bool,
        then: impl FnOnce(Self) -> Self,
        else_fn: impl FnOnce(Self) -> Self,
    ) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if condition { then(this) } else { else_fn(this) })
    }

    /// Conditionally unwrap and modify self with the given closure, if the given option is Some.
    fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| {
            if let Some(value) = option {
                then(this, value)
            } else {
                this
            }
        })
    }
    /// Conditionally unwrap and modify self with the given closure, if the given option is None.
    fn when_none<T>(self, option: &Option<T>, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if option.is_some() { this } else { then(this) })
    }
}

/// Extensions for Future types that provide additional combinators and utilities.
pub trait FutureExt {
    /// Requires a Future to complete before the specified duration has elapsed.
    /// Similar to tokio::timeout.
    fn with_timeout(self, timeout: Duration, executor: &BackgroundExecutor) -> WithTimeout<Self>
    where
        Self: Sized;
}

impl<T: Future> FutureExt for T {
    fn with_timeout(self, timeout: Duration, executor: &BackgroundExecutor) -> WithTimeout<Self>
    where
        Self: Sized,
    {
        WithTimeout {
            future: self,
            timer: executor.timer(timeout),
        }
    }
}

#[pin_project::pin_project]
pub struct WithTimeout<T> {
    #[pin]
    future: T,
    #[pin]
    timer: Task<()>,
}

#[derive(Debug, thiserror::Error)]
#[error("Timed out before future resolved")]
/// Error returned by with_timeout when the timeout duration elapsed before the future resolved
pub struct Timeout;

impl<T: Future> Future for WithTimeout<T> {
    type Output = Result<T::Output, Timeout>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context) -> task::Poll<Self::Output> {
        let this = self.project();

        if let task::Poll::Ready(output) = this.future.poll(cx) {
            task::Poll::Ready(Ok(output))
        } else if this.timer.poll(cx).is_ready() {
            task::Poll::Ready(Err(Timeout))
        } else {
            task::Poll::Pending
        }
    }
}

/// Increment the given atomic counter if it is not zero.
/// Return the new value of the counter.
pub(crate) fn atomic_incr_if_not_zero(counter: &AtomicUsize) -> usize {
    let mut loaded = counter.load(SeqCst);
    loop {
        if loaded == 0 {
            return 0;
        }
        match counter.compare_exchange_weak(loaded, loaded + 1, SeqCst, SeqCst) {
            Ok(x) => return x + 1,
            Err(actual) => loaded = actual,
        }
    }
}

/// `f32::floor`, computed through a hardware truncation for magnitudes below 2^23 (all layout
/// and glyph coordinates). The baseline x86-64 target has no rounding instruction, so
/// `f32::floor` is a library call there. Identical to `f32::floor`, including the sign of zero.
#[inline]
pub(crate) fn floor_fast(value: f32) -> f32 {
    if value.abs() < 8_388_608.0 {
        let truncated = value as i32 as f32;
        let floor = if truncated > value {
            truncated - 1.0
        } else {
            truncated
        };
        if floor == 0.0 {
            0.0f32.copysign(value)
        } else {
            floor
        }
    } else {
        value.floor()
    }
}

/// `f32::round` (ties away from zero) computed like [`floor_fast`].
#[inline]
pub(crate) fn round_fast(value: f32) -> f32 {
    if value.abs() < 8_388_608.0 {
        let truncated = value as i32 as f32;
        let fraction = value - truncated;
        let round = if fraction >= 0.5 {
            truncated + 1.0
        } else if fraction <= -0.5 {
            truncated - 1.0
        } else {
            truncated
        };
        if round == 0.0 {
            0.0f32.copysign(value)
        } else {
            round
        }
    } else {
        value.round()
    }
}

/// `f32::ceil` computed like [`floor_fast`].
#[inline]
pub(crate) fn ceil_fast(value: f32) -> f32 {
    -floor_fast(-value)
}

/// Rounds to the nearest integer with 0.5 ties toward zero.
#[inline]
pub(crate) fn round_half_toward_zero(value: f32) -> f32 {
    ceil_fast(value.abs() - 0.5).copysign(value)
}

#[inline]
pub(crate) fn round_half_toward_zero_f64(value: f64) -> f64 {
    (value.abs() - 0.5).ceil().copysign(value)
}

#[inline]
pub(crate) fn round_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    round_half_toward_zero(logical * scale_factor)
}

#[inline]
pub(crate) fn round_stroke_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    if logical == 0.0 {
        0.0
    } else {
        round_to_device_pixel(logical.max(0.0), scale_factor).max(1.0)
    }
}

#[inline]
pub(crate) fn floor_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    floor_fast(logical * scale_factor)
}

#[inline]
pub(crate) fn ceil_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    ceil_fast(logical * scale_factor)
}

#[cfg(test)]
mod tests {
    use crate::TestAppContext;

    use super::*;

    #[test]
    fn floor_ceil_and_round_fast_match_the_library_bit_for_bit() {
        let mut values = vec![
            0.0f32,
            -0.0,
            0.25,
            -0.25,
            0.5,
            -0.5,
            1.0,
            -1.0,
            0.999_999_9,
            -0.999_999_9,
            8_388_607.5,
            -8_388_607.5,
            8_388_608.0,
            -8_388_608.0,
            1.0e10,
            -1.0e10,
            f32::MAX,
            f32::MIN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MIN_POSITIVE,
            -f32::MIN_POSITIVE,
        ];
        let mut state = 0x9E37_79B9u32;
        for _ in 0..20_000 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            values.push((state as i32 as f32) / 4096.0);
            values.push((state >> 16) as f32 / 64.0 - 512.0);
        }
        for value in values {
            assert_eq!(
                floor_fast(value).to_bits(),
                value.floor().to_bits(),
                "floor {value}"
            );
            assert_eq!(
                ceil_fast(value).to_bits(),
                value.ceil().to_bits(),
                "ceil {value}"
            );
            assert_eq!(
                round_fast(value).to_bits(),
                value.round().to_bits(),
                "round {value}"
            );
        }
        assert!(
            floor_fast(f32::NAN).is_nan()
                && ceil_fast(f32::NAN).is_nan()
                && round_fast(f32::NAN).is_nan()
        );
    }

    #[test]
    fn test_round_half_toward_zero() {
        // Midpoint ties go toward zero
        assert_eq!(round_half_toward_zero(0.5), 0.0);
        assert_eq!(round_half_toward_zero(1.5), 1.0);
        assert_eq!(round_half_toward_zero(2.5), 2.0);
        assert_eq!(round_half_toward_zero(-0.5), 0.0);
        assert_eq!(round_half_toward_zero(-1.5), -1.0);
        assert_eq!(round_half_toward_zero(-2.5), -2.0);

        // Non-midpoint values round to nearest
        assert_eq!(round_half_toward_zero(1.5001), 2.0);
        assert_eq!(round_half_toward_zero(1.4999), 1.0);
        assert_eq!(round_half_toward_zero(-1.5001), -2.0);
        assert_eq!(round_half_toward_zero(-1.4999), -1.0);

        // Integers are unchanged
        assert_eq!(round_half_toward_zero(0.0), 0.0);
        assert_eq!(round_half_toward_zero(3.0), 3.0);
        assert_eq!(round_half_toward_zero(-3.0), -3.0);
    }

    #[test]
    fn test_device_pixel_helpers() {
        // Snap uses half-toward-zero: 1.0 * 1.5 = 1.5 ties toward 1.0.
        assert_eq!(round_to_device_pixel(1.0, 1.5), 1.0);
        // Below the tie rounds down, above rounds up.
        assert_eq!(round_to_device_pixel(0.3, 2.0), 1.0);
        assert_eq!(round_to_device_pixel(1.4, 1.0), 1.0);
        assert_eq!(round_to_device_pixel(1.6, 1.0), 2.0);

        // Stroke uses snap, but clamps non-zero input up to at least 1dp.
        assert_eq!(round_stroke_to_device_pixel(0.0, 1.0), 0.0);
        assert_eq!(round_stroke_to_device_pixel(0.4, 1.0), 1.0);
        assert_eq!(round_stroke_to_device_pixel(0.5, 1.0), 1.0);
        assert_eq!(round_stroke_to_device_pixel(1.0, 1.5), 1.0);
        assert_eq!(round_stroke_to_device_pixel(1.6, 1.0), 2.0);

        // Cover's near edge floors, far edge ceils. Together they form a strict superset.
        assert_eq!(floor_to_device_pixel(0.3, 2.0), 0.0);
        assert_eq!(ceil_to_device_pixel(0.3, 2.0), 1.0);
        assert_eq!(floor_to_device_pixel(2.1, 1.0), 2.0);
        assert_eq!(ceil_to_device_pixel(2.1, 1.0), 3.0);

        // Integer device-pixel inputs are stable under all three.
        assert_eq!(round_to_device_pixel(2.0, 2.0), 4.0);
        assert_eq!(floor_to_device_pixel(2.0, 2.0), 4.0);
        assert_eq!(ceil_to_device_pixel(2.0, 2.0), 4.0);
    }

    #[test]
    fn test_round_half_toward_zero_f64() {
        assert_eq!(round_half_toward_zero_f64(0.5), 0.0);
        assert_eq!(round_half_toward_zero_f64(-0.5), 0.0);
        assert_eq!(round_half_toward_zero_f64(1.5), 1.0);
        assert_eq!(round_half_toward_zero_f64(-1.5), -1.0);
        assert_eq!(round_half_toward_zero_f64(2.5001), 3.0);
    }

    #[vespertine::test]
    async fn test_with_timeout(cx: &mut TestAppContext) {
        Task::ready(())
            .with_timeout(Duration::from_secs(1), &cx.executor())
            .await
            .expect("Timeout should be noop");

        let long_duration = Duration::from_secs(6000);
        let short_duration = Duration::from_secs(1);
        cx.executor()
            .timer(long_duration)
            .with_timeout(short_duration, &cx.executor())
            .await
            .expect_err("timeout should have triggered");

        let fut = cx
            .executor()
            .timer(long_duration)
            .with_timeout(short_duration, &cx.executor());
        cx.executor().advance_clock(short_duration * 2);
        futures::FutureExt::now_or_never(fut)
            .unwrap_or_else(|| panic!("timeout should have triggered"))
            .expect_err("timeout");
    }
}
