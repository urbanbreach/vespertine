//! Cubic-bezier and named easing curves (ANM-002 R4).

use std::fmt;

use crate::SharedString;

/// Number of binary subdivisions used to invert the curve's x polynomial.
const BISECTION_STEPS: u32 = 20;

/// Why a motion description, easing curve or transition was rejected (ANM-002 R5).
#[derive(Clone, Debug, PartialEq)]
pub enum MotionError {
    /// A size or border radius is negative, not finite, or not representable as an `f32`.
    InvalidSize(&'static str),
    /// An inset (top, right, bottom or left) is not finite or not representable as an `f32`.
    InvalidInset(&'static str),
    /// The opacity lies outside `0..=1` or is not finite.
    InvalidOpacity,
    /// A duration or delay is negative, not finite, or not a representable duration in seconds.
    InvalidDuration(&'static str),
    /// A Bezier coordinate is not finite, or an x control lies outside `0..=1`.
    InvalidBezier,
    /// The easing name is not one of the named curves.
    UnknownEasing(SharedString),
    /// An `initial` of `true` was supplied; only a style or `false` is valid.
    InvalidInitial,
}

impl fmt::Display for MotionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MotionError::InvalidSize(name) => {
                write!(
                    f,
                    "{name} must be finite, nonnegative and representable as f32"
                )
            }
            MotionError::InvalidInset(name) => {
                write!(f, "{name} must be finite and representable as f32")
            }
            MotionError::InvalidOpacity => write!(f, "opacity must lie within 0 to 1"),
            MotionError::InvalidDuration(name) => {
                write!(
                    f,
                    "{name} must be a finite, nonnegative duration in seconds"
                )
            }
            MotionError::InvalidBezier => write!(
                f,
                "cubic-bezier coordinates must be finite with x controls within 0 to 1"
            ),
            MotionError::UnknownEasing(name) => write!(f, "unknown easing name {name:?}"),
            MotionError::InvalidInitial => write!(f, "initial must be a style or false, not true"),
        }
    }
}

impl std::error::Error for MotionError {}

/// A CSS-style cubic Bezier easing curve through (0,0) and (1,1) with control points
/// `(x1, y1)` and `(x2, y2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    /// X of the first control point.
    pub x1: f32,
    /// X of the second control point.
    pub x2: f32,
    /// Y of the first control point.
    pub y1: f32,
    /// Y of the second control point.
    pub y2: f32,
}

impl CubicBezier {
    /// Creates a curve from its control points. Use [`CubicBezier::validate`] to check them.
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x2, y2, x1, y1 }
    }

    /// Checks that every coordinate is finite and that both x controls lie within 0 to 1.
    pub fn validate(&self) -> Result<(), MotionError> {
        let finite = [self.x1, self.y1, self.x2, self.y2]
            .iter()
            .all(|value| value.is_finite());
        let x_in_range = (0.0..=1.0).contains(&self.x1) && (0.0..=1.0).contains(&self.x2);
        if finite && x_in_range {
            Ok(())
        } else {
            Err(MotionError::InvalidBezier)
        }
    }

    fn component(a: f64, b: f64, s: f64) -> f64 {
        let inverse = 1.0 - s;
        3.0 * inverse * inverse * s * a + 3.0 * inverse * s * s * b + s * s * s
    }

    /// Evaluates the curve's eased value for normalized time `t`.
    ///
    /// The curve's x coordinate is inverted with twenty binary subdivisions of `0..1`, y is
    /// evaluated at the final midpoint and the result is clamped to `0..=1`. The result is exactly
    /// 0 at `t <= 0` and exactly 1 at `t >= 1`.
    pub fn ease(&self, t: f32) -> f32 {
        if t <= 0.0 || t.is_nan() {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let input = f64::from(t);
        let (x1, x2) = (f64::from(self.x1), f64::from(self.x2));
        let (mut low, mut high) = (0.0f64, 1.0f64);
        for _ in 0..BISECTION_STEPS {
            let mid = (low + high) * 0.5;
            if Self::component(x1, x2, mid) < input {
                low = mid;
            } else {
                high = mid;
            }
        }
        let s = (low + high) * 0.5;
        Self::component(f64::from(self.y1), f64::from(self.y2), s).clamp(0.0, 1.0) as f32
    }

    /// Inverts the x polynomial only: returns the curve parameter `s` solving `x(s) = t`.
    pub fn solve_x(&self, t: f32) -> f32 {
        let input = f64::from(t.clamp(0.0, 1.0));
        let (x1, x2) = (f64::from(self.x1), f64::from(self.x2));
        let (mut low, mut high) = (0.0f64, 1.0f64);
        for _ in 0..BISECTION_STEPS {
            let mid = (low + high) * 0.5;
            if Self::component(x1, x2, mid) < input {
                low = mid;
            } else {
                high = mid;
            }
        }
        ((low + high) * 0.5) as f32
    }
}

/// The CSS-style named easing curves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedEasing {
    /// The identity curve.
    Linear,
    /// `cubic-bezier(0.25, 0.1, 0.25, 1)`.
    Ease,
    /// `cubic-bezier(0.42, 0, 1, 1)`.
    EaseIn,
    /// `cubic-bezier(0, 0, 0.58, 1)`.
    EaseOut,
    /// `cubic-bezier(0.42, 0, 0.58, 1)`.
    EaseInOut,
}

impl NamedEasing {
    /// Parses `linear`, `ease`, `easeIn`, `easeOut` or `easeInOut`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "linear" => Self::Linear,
            "ease" => Self::Ease,
            "easeIn" => Self::EaseIn,
            "easeOut" => Self::EaseOut,
            "easeInOut" => Self::EaseInOut,
            _ => return None,
        })
    }

    /// The Bezier control points of this curve, or `None` for [`NamedEasing::Linear`].
    pub const fn curve(self) -> Option<CubicBezier> {
        match self {
            Self::Linear => None,
            Self::Ease => Some(CubicBezier::new(0.25, 0.1, 0.25, 1.0)),
            Self::EaseIn => Some(CubicBezier::new(0.42, 0.0, 1.0, 1.0)),
            Self::EaseOut => Some(CubicBezier::new(0.0, 0.0, 0.58, 1.0)),
            Self::EaseInOut => Some(CubicBezier::new(0.42, 0.0, 0.58, 1.0)),
        }
    }

    /// Applies the curve to normalized time.
    pub fn ease(self, t: f32) -> f32 {
        match self.curve() {
            None => t,
            Some(curve) => curve.ease(t),
        }
    }
}

/// An easing function description: a named curve, a cubic Bezier, or an unrecognized name
/// carried from a declarative description (which makes that description invalid, ANM-002 R5).
#[derive(Clone, Debug, PartialEq)]
pub enum Easing {
    /// One of the named curves.
    Named(NamedEasing),
    /// A custom cubic Bezier curve.
    CubicBezier(CubicBezier),
    /// An easing name that is not one of the named curves.
    Unknown(SharedString),
}

impl Easing {
    /// The identity curve.
    pub const LINEAR: Easing = Easing::Named(NamedEasing::Linear);
    /// The `ease` named curve.
    pub const EASE: Easing = Easing::Named(NamedEasing::Ease);
    /// The `easeIn` named curve.
    pub const EASE_IN: Easing = Easing::Named(NamedEasing::EaseIn);
    /// The `easeOut` named curve (the default for declarative tracks).
    pub const EASE_OUT: Easing = Easing::Named(NamedEasing::EaseOut);
    /// The `easeInOut` named curve.
    pub const EASE_IN_OUT: Easing = Easing::Named(NamedEasing::EaseInOut);

    /// Creates a cubic-bezier easing.
    pub const fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Easing::CubicBezier(CubicBezier::new(x1, y1, x2, y2))
    }

    /// Parses a named easing; an unknown name yields [`Easing::Unknown`], which fails validation.
    pub fn named(name: &str) -> Self {
        match NamedEasing::from_name(name) {
            Some(named) => Easing::Named(named),
            None => Easing::Unknown(SharedString::from(name.to_owned())),
        }
    }

    /// Validates the curve.
    pub fn validate(&self) -> Result<(), MotionError> {
        match self {
            Easing::Named(_) => Ok(()),
            Easing::CubicBezier(curve) => curve.validate(),
            Easing::Unknown(name) => Err(MotionError::UnknownEasing(name.clone())),
        }
    }

    /// Applies the easing to normalized time. An [`Easing::Unknown`] easing is the identity.
    pub fn apply(&self, t: f32) -> f32 {
        match self {
            Easing::Named(named) => named.ease(t),
            Easing::CubicBezier(curve) => curve.ease(t),
            Easing::Unknown(_) => t,
        }
    }
}

impl Default for Easing {
    fn default() -> Self {
        Easing::EASE_OUT
    }
}

impl From<NamedEasing> for Easing {
    fn from(named: NamedEasing) -> Self {
        Easing::Named(named)
    }
}

impl From<CubicBezier> for Easing {
    fn from(curve: CubicBezier) -> Self {
        Easing::CubicBezier(curve)
    }
}
