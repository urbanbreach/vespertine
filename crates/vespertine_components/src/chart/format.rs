//! Number formatting of chart values (CMP-024 R6).

/// How a value is rendered: an optional prefix, a unit and a number of decimals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NumberFormat {
    /// Text printed before the number (after the sign), for example `"$"`.
    pub prefix: String,
    /// Text printed after the number, for example `"ms"` or `"%"`.
    pub unit: String,
    /// Fixed decimals (0 to 6, larger values clamp to 6); `None` selects them from the value.
    pub decimals: Option<u8>,
}

/// The largest number of explicit decimals.
pub const MAX_DECIMALS: u8 = 6;

impl NumberFormat {
    /// The automatic format: no prefix, no unit, decimals chosen from the value.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the prefix.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Sets the unit.
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }

    /// Sets explicit decimals, clamped to [`MAX_DECIMALS`].
    pub fn decimals(mut self, decimals: u8) -> Self {
        self.decimals = Some(decimals.min(MAX_DECIMALS));
        self
    }

    /// Decimals used for `value`: the explicit setting (clamped) or the automatic rule: 0 when
    /// the value is 0, has absolute value >= 100 or has no fractional part, 1 when the
    /// absolute value is >= 10, else 2.
    pub fn decimals_for(&self, value: f64) -> usize {
        if let Some(decimals) = self.decimals {
            return usize::from(decimals.min(MAX_DECIMALS));
        }
        let magnitude = value.abs();
        if value == 0.0 || value.fract() == 0.0 || magnitude >= 100.0 {
            return 0;
        }
        if magnitude >= 10.0 { 1 } else { 2 }
    }

    /// Renders `value`: grouped integer part, sign before the prefix, the unit after one space
    /// unless it is empty or starts with `%`. Non-finite values render as an empty string.
    pub fn format(&self, value: f64) -> String {
        if !value.is_finite() {
            return String::new();
        }
        let decimals = self.decimals_for(value);
        let digits = format!("{:.*}", decimals, value.abs());
        let (integer, fraction) = match digits.split_once('.') {
            Some((integer, fraction)) => (integer, Some(fraction)),
            None => (digits.as_str(), None),
        };
        let nonzero = digits.bytes().any(|byte| matches!(byte, b'1'..=b'9'));
        let mut text = String::new();
        if value < 0.0 && nonzero {
            text.push('-');
        }
        text.push_str(&self.prefix);
        let first_group = integer.len() % 3;
        for (index, digit) in integer.chars().enumerate() {
            if index != 0 && index % 3 == first_group {
                text.push(',');
            }
            text.push(digit);
        }
        if let Some(fraction) = fraction {
            text.push('.');
            text.push_str(fraction);
        }
        if !self.unit.is_empty() {
            if !self.unit.starts_with('%') {
                text.push(' ');
            }
            text.push_str(&self.unit);
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a10_automatic_decimals_and_grouping() {
        let format = NumberFormat::new();
        assert_eq!(format.format(3.14158), "3.14");
        assert_eq!(format.format(12.5), "12.5");
        assert_eq!(format.format(120.7), "121");
        assert_eq!(format.format(0.0), "0");
        assert_eq!(format.format(1_234_567.0), "1,234,567");
        assert_eq!(format.format(-1234.6), "-1,235");
        assert_eq!(format.format(100.0), "100");
        assert_eq!(format.format(1000.0), "1,000");
    }

    #[test]
    fn a10_prefix_unit_and_explicit_decimals() {
        assert_eq!(
            NumberFormat::new().prefix("$").unit("ms").format(25.0),
            "$25 ms"
        );
        assert_eq!(NumberFormat::new().unit("%").format(42.0), "42%");
        assert_eq!(NumberFormat::new().unit("%").format(0.5), "0.50%");
        assert_eq!(NumberFormat::new().prefix("$").format(-1234.6), "-$1,235");
        assert_eq!(NumberFormat::new().decimals(6).format(1.5), "1.500000");
        assert_eq!(NumberFormat::new().decimals(9).decimals, Some(6));
        assert_eq!(NumberFormat::new().decimals(1).format(-0.04), "0.0");
        assert_eq!(NumberFormat::new().format(f64::NAN), "");
    }
}
