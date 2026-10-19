//! Line model of the code block host (CMP-014 R1, R6): normalization, splitting and the analytic
//! gutter width.

use std::ops::Range;
use vespertine_theme::document::DocumentMetrics;

/// Normalizes line endings: every CRLF becomes LF (a lone CR is kept).
pub fn normalize(code: &str) -> String {
    code.replace("\r\n", "\n")
}

/// Byte ranges (without terminators) of the logical lines of already normalized code: split on
/// LF, empty lines and an empty final line preserved. Empty code is one empty line.
pub fn line_ranges(code: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, byte) in code.bytes().enumerate() {
        if byte == b'\n' {
            ranges.push(start..index);
            start = index + 1;
        }
    }
    ranges.push(start..code.len());
    ranges
}

/// The number of decimal digits of `n` (at least 1).
pub fn digits(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

/// Width of the line-number gutter: `max(digits(line count) x digit width + right padding,
/// minimum)` in logical pixels (R6).
pub fn gutter_width(line_count: usize, metrics: &DocumentMetrics) -> f32 {
    (digits(line_count) as f32 * metrics.code_gutter_digit_width
        + metrics.code_gutter_padding_right)
        .max(metrics.code_gutter_min_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_keeps_blank_and_final_empty_lines() {
        let code = normalize("a\r\n\r\nb\r\n");
        assert_eq!(code, "a\n\nb\n");
        let lines: Vec<&str> = line_ranges(&code).into_iter().map(|r| &code[r]).collect();
        assert_eq!(lines, ["a", "", "b", ""]);
    }

    #[test]
    fn empty_code_is_one_empty_row() {
        assert_eq!(line_ranges(""), vec![0..0]);
    }

    #[test]
    fn gutter_width_follows_the_digit_rule() {
        let metrics = DocumentMetrics::default();
        for count in [1, 9, 10, 99] {
            assert_eq!(gutter_width(count, &metrics), 28.);
        }
        assert_eq!(gutter_width(100, &metrics), 33.);
        assert_eq!(gutter_width(1000, &metrics), 40.);
        assert_eq!(gutter_width(10000, &metrics), 47.);
    }
}
