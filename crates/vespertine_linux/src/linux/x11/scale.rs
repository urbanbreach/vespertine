//! X11 scale-factor selection (PLT-004).
//!
//! Precedence, first match wins:
//! 1. the explicit `VESPERTINE_X11_SCALE_FACTOR` override (a number, or `randr`),
//! 2. the `Xft.dpi` resource divided by 96,
//! 3. the `GDK_SCALE` toolkit environment variable (a positive whole number),
//! 4. the physical-monitor RandR DPI estimate,
//! 5. `1.0`.

/// The parsed explicit override from the upstream environment variable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ScaleOverride {
    /// Use the RandR estimate, and `1.0` when it is unavailable.
    Randr,
    /// Use exactly this scale.
    Scale(f32),
    /// No override was given.
    NotSet,
}

/// Where the selected scale came from, for logging.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ScaleSource {
    Override,
    RandrOverride,
    RandrOverrideFailed,
    XftDpi,
    GdkScale,
    Randr,
    Default,
}

/// Parses `GDK_SCALE`: a trimmed positive unsigned decimal integer that fits in
/// 32 bits. Anything else (empty, zero, negative, fractional, nonnumeric, out
/// of range) is ignored.
pub(crate) fn parse_gdk_scale(value: &str) -> Option<f32> {
    let scale = value.trim().parse::<u32>().ok()?;
    (scale > 0).then_some(scale as f32)
}

/// Selects the scale factor from every source. `randr` is only evaluated when
/// no earlier source applies.
pub(crate) fn select_scale_factor(
    scale_override: ScaleOverride,
    xft_dpi: Option<f32>,
    gdk_scale: Option<&str>,
    randr: impl FnOnce() -> Option<f32>,
) -> (f32, ScaleSource) {
    match scale_override {
        ScaleOverride::Scale(scale) => return (scale, ScaleSource::Override),
        ScaleOverride::Randr => {
            return match randr() {
                Some(scale) => (scale, ScaleSource::RandrOverride),
                None => (1.0, ScaleSource::RandrOverrideFailed),
            };
        }
        ScaleOverride::NotSet => {}
    }

    if let Some(dpi) = xft_dpi {
        return (dpi / 96.0, ScaleSource::XftDpi);
    }

    if let Some(scale) = gdk_scale.and_then(parse_gdk_scale) {
        return (scale, ScaleSource::GdkScale);
    }

    if let Some(scale) = randr() {
        return (scale, ScaleSource::Randr);
    }

    (1.0, ScaleSource::Default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(
        scale_override: ScaleOverride,
        xft_dpi: Option<f32>,
        gdk_scale: Option<&str>,
        randr: Option<f32>,
    ) -> (f32, ScaleSource) {
        select_scale_factor(scale_override, xft_dpi, gdk_scale, || randr)
    }

    #[test]
    fn gdk_scale_accepts_whole_numbers_and_trims_whitespace() {
        assert_eq!(parse_gdk_scale("1"), Some(1.0));
        assert_eq!(parse_gdk_scale("2"), Some(2.0));
        assert_eq!(parse_gdk_scale(" 2 "), Some(2.0));
        assert_eq!(parse_gdk_scale("\t3\n"), Some(3.0));
        assert_eq!(parse_gdk_scale("+2"), Some(2.0));
        assert_eq!(parse_gdk_scale("4294967295"), Some(4294967295u32 as f32));
    }

    #[test]
    fn gdk_scale_rejects_everything_else() {
        for rejected in [
            "",
            "   ",
            "0",
            "1.5",
            "-2",
            "auto",
            "2x",
            "4294967296",
            "99999999999999999999",
        ] {
            assert_eq!(parse_gdk_scale(rejected), None, "{rejected:?}");
        }
    }

    #[test]
    fn gdk_scale_is_used_without_xft_dpi() {
        assert_eq!(
            select(ScaleOverride::NotSet, None, Some("2"), Some(1.4167)),
            (2.0, ScaleSource::GdkScale)
        );
        assert_eq!(
            select(ScaleOverride::NotSet, None, Some(" 2 "), None),
            (2.0, ScaleSource::GdkScale)
        );
    }

    #[test]
    fn xft_dpi_wins_over_gdk_scale() {
        assert_eq!(
            select(ScaleOverride::NotSet, Some(192.0), Some("3"), Some(1.4)),
            (2.0, ScaleSource::XftDpi)
        );
    }

    #[test]
    fn explicit_override_wins_over_everything() {
        assert_eq!(
            select(
                ScaleOverride::Scale(1.25),
                Some(192.0),
                Some("3"),
                Some(1.4)
            ),
            (1.25, ScaleSource::Override)
        );
        assert_eq!(
            select(ScaleOverride::Randr, Some(192.0), Some("3"), Some(1.4)),
            (1.4, ScaleSource::RandrOverride)
        );
        assert_eq!(
            select(ScaleOverride::Randr, Some(192.0), Some("3"), None),
            (1.0, ScaleSource::RandrOverrideFailed)
        );
    }

    #[test]
    fn randr_is_used_without_gdk_scale_or_with_ignored_value() {
        assert_eq!(
            select(ScaleOverride::NotSet, None, None, Some(1.4167)),
            (1.4167, ScaleSource::Randr)
        );
        for ignored in ["", "0", "1.5", "-2", "auto", "4294967296"] {
            assert_eq!(
                select(ScaleOverride::NotSet, None, Some(ignored), Some(1.4167)),
                (1.4167, ScaleSource::Randr),
                "{ignored:?}"
            );
        }
        assert_eq!(
            select(ScaleOverride::NotSet, None, None, None),
            (1.0, ScaleSource::Default)
        );
    }

    #[test]
    fn randr_is_not_queried_when_an_earlier_source_applies() {
        let (scale, _) = select_scale_factor(ScaleOverride::NotSet, None, Some("2"), || {
            panic!("RandR must not be queried")
        });
        assert_eq!(scale, 2.0);
    }
}
