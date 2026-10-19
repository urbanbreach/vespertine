//! Source scans of the Markdown parser: formula delimiters (CMP-011 R19), bare URLs (R2) and
//! link destinations with spaces (R3). Every scan is linear in the scanned text.

use std::ops::Range;

/// A decided formula in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaSpan {
    /// The whole span including delimiters.
    pub range: Range<usize>,
    /// The formula source (inside the delimiters).
    pub tex: Range<usize>,
    /// Display or inline.
    pub display: bool,
}

/// Sorted, non-overlapping byte ranges that the scans must not touch (code, HTML, URLs).
#[derive(Clone, Debug, Default)]
pub struct Protected {
    ranges: Vec<Range<usize>>,
}

impl Protected {
    /// Builds the set from arbitrary ranges.
    pub fn new(mut ranges: Vec<Range<usize>>) -> Self {
        ranges.retain(|range| range.start < range.end);
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        Self { ranges: merged }
    }

    /// Whether byte `index` is protected.
    pub fn contains(&self, index: usize) -> bool {
        let at = self.ranges.partition_point(|range| range.end <= index);
        self.ranges
            .get(at)
            .is_some_and(|range| range.start <= index)
    }

    /// Whether any byte of `range` is protected.
    pub fn intersects(&self, range: &Range<usize>) -> bool {
        let at = self.ranges.partition_point(|r| r.end <= range.start);
        self.ranges.get(at).is_some_and(|r| r.start < range.end)
    }

    /// The protected ranges.
    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }
}

fn is_space(c: char) -> bool {
    c.is_whitespace()
}

fn char_before(source: &str, index: usize) -> Option<char> {
    source[..index].chars().next_back()
}

fn char_at(source: &str, index: usize) -> Option<char> {
    source[index..].chars().next()
}

fn run_of(source: &str, index: usize, byte: u8) -> usize {
    source.as_bytes()[index..]
        .iter()
        .take_while(|b| **b == byte)
        .count()
}

/// Whether the backslash at `index` is itself escaped (an odd number of backslashes precede it
/// directly makes it a literal backslash pair member).
fn escaped_at(source: &str, index: usize) -> bool {
    let bytes = source.as_bytes();
    let mut count = 0;
    let mut i = index;
    while i > 0 && bytes[i - 1] == b'\\' {
        count += 1;
        i -= 1;
    }
    count % 2 == 1
}

/// Scans `source[container]` for formulas. `protected` masks code, raw HTML, autolinks, link
/// destinations and bare URLs. Formulas of one container never overlap and are returned in
/// order.
pub fn scan_formulas(
    source: &str,
    container: Range<usize>,
    protected: &Protected,
) -> Vec<FormulaSpan> {
    let bytes = source.as_bytes();
    let end = container.end.min(source.len());
    let mut out = Vec::new();
    let mut i = container.start;
    while i < end {
        if protected.contains(i) {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'\\' => {
                if escaped_at(source, i) {
                    i += 1;
                    continue;
                }
                match bytes.get(i + 1) {
                    Some(b'(') | Some(b'[') => {
                        let display = bytes[i + 1] == b'[';
                        if let Some(span) = scan_backslash(source, i, end, display, protected) {
                            i = span.range.end;
                            out.push(span);
                            continue;
                        }
                        i += 2;
                    }
                    Some(_) => i += 2,
                    None => i += 1,
                }
            }
            b'$' => {
                let n = run_of(source, i, b'$');
                match n {
                    1 => {
                        if let Some(span) = scan_single_dollar(source, i, end, protected) {
                            i = span.range.end;
                            out.push(span);
                        } else {
                            i += 1;
                        }
                    }
                    2 => {
                        if let Some(span) = scan_display_dollar(source, i, end, protected) {
                            i = span.range.end;
                            out.push(span);
                        } else {
                            i += 2;
                        }
                    }
                    _ => i += n,
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn scan_single_dollar(
    source: &str,
    open: usize,
    end: usize,
    protected: &Protected,
) -> Option<FormulaSpan> {
    let after = char_at(source, open + 1)?;
    if is_space(after) {
        return None;
    }
    let bytes = source.as_bytes();
    let mut j = open + 1;
    while j < end {
        if protected.contains(j) {
            return None;
        }
        match bytes[j] {
            b'\\' => {
                j += 2;
                continue;
            }
            b'$' => {
                let n = run_of(source, j, b'$');
                if n == 1 {
                    let before = char_before(source, j)?;
                    if !is_space(before) {
                        return accept_dollar(source, open, j);
                    }
                    // A closer must follow a non-space: this `$` can only open the next pair, so
                    // the span before it reads as prose (a price).
                    return None;
                }
                j += n;
                continue;
            }
            b'\n' if bytes.get(j + 1) == Some(&b'\n') => return None,
            _ => j += 1,
        }
    }
    None
}

fn accept_dollar(source: &str, open: usize, close: usize) -> Option<FormulaSpan> {
    let body = &source[open + 1..close];
    let before = char_before(source, open);
    let after = char_at(source, close + 1);
    // A closer glued to a digit: a price ("$5 and 10$5").
    if after.is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    // An identifier glued to both sides.
    if before.is_some_and(char::is_alphanumeric) && after.is_some_and(char::is_alphanumeric) {
        return None;
    }
    // A quoted interpolation.
    if let (Some(b), Some(a)) = (before, after)
        && matches!(b, '"' | '\'' | '`')
        && a == b
    {
        return None;
    }
    // Shell: command or parameter substitution, or an environment variable.
    if body.starts_with(['(', '{']) {
        return None;
    }
    if !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && body.chars().next().is_some_and(|c| !c.is_ascii_digit())
        && body.len() > 1
    {
        return None;
    }
    Some(FormulaSpan {
        range: open..close + 1,
        tex: open + 1..close,
        display: false,
    })
}

fn scan_display_dollar(
    source: &str,
    open: usize,
    end: usize,
    protected: &Protected,
) -> Option<FormulaSpan> {
    let bytes = source.as_bytes();
    let mut j = open + 2;
    while j < end {
        if protected.contains(j) {
            return None;
        }
        match bytes[j] {
            b'\\' => j += 2,
            b'$' => {
                let n = run_of(source, j, b'$');
                if n == 2 {
                    if source[open + 2..j].trim().is_empty() {
                        return None;
                    }
                    return Some(FormulaSpan {
                        range: open..j + 2,
                        tex: open + 2..j,
                        display: true,
                    });
                }
                j += n;
            }
            _ => j += 1,
        }
    }
    None
}

fn scan_backslash(
    source: &str,
    open: usize,
    end: usize,
    display: bool,
    protected: &Protected,
) -> Option<FormulaSpan> {
    let bytes = source.as_bytes();
    let closer = if display { b']' } else { b')' };
    let before = char_before(source, open);
    // Not glued to a word, URL or path.
    if before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '~'))
    {
        return None;
    }
    let mut j = open + 2;
    while j < end {
        if protected.contains(j) {
            return None;
        }
        match bytes[j] {
            b'\\' => {
                if bytes.get(j + 1) == Some(&closer) {
                    return accept_backslash(source, open, j, display, before);
                }
                j += 2;
            }
            b'$' | b'`' | b'%' => return None,
            b'\n' => return None,
            _ => j += 1,
        }
    }
    None
}

fn has_tex_syntax(body: &str) -> bool {
    let mut previous_backslash = false;
    for c in body.chars() {
        if previous_backslash && c.is_ascii_alphabetic() {
            return true;
        }
        previous_backslash = c == '\\';
        if matches!(
            c,
            '^' | '_' | '{' | '}' | '=' | '+' | '<' | '>' | '*' | '|' | '/'
        ) {
            return true;
        }
    }
    false
}

fn is_short_variable(body: &str) -> bool {
    let body = body.trim();
    !body.is_empty()
        && body.len() <= 8
        && body.split(',').all(|part| {
            let part = part.trim();
            let mut chars = part.chars();
            chars.next().is_some_and(|c| c.is_alphabetic())
                && chars.all(|c| c.is_ascii_digit() || c == '\'')
        })
}

fn accept_backslash(
    source: &str,
    open: usize,
    close: usize,
    display: bool,
    before: Option<char>,
) -> Option<FormulaSpan> {
    let body = &source[open + 2..close];
    let trimmed = body.trim();
    if trimmed.is_empty() || body.ends_with('\\') {
        return None;
    }
    // Surrounding whitespace stays on one line.
    if body.starts_with('\n') || body.ends_with('\n') {
        return None;
    }
    let after = char_at(source, close + 2);
    if after.is_some_and(|c| c.is_alphanumeric()) {
        return None;
    }
    if trimmed.contains("http://") || trimmed.contains("https://") || trimmed.contains("www.") {
        return None;
    }
    if trimmed.contains(['"', '\'', '#']) {
        return None;
    }
    let tex = has_tex_syntax(trimmed);
    // Display opened mid-line is prose unless it has TeX syntax.
    let mid_line = before.is_some_and(|c| c != '\n');
    if display && mid_line && !tex {
        return None;
    }
    if !tex && !is_short_variable(trimmed) {
        // A bare citation or number, or words.
        let numeric = trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | ' ' | '-' | '.' | ';'));
        if numeric
            || trimmed.split_whitespace().count() > 1
            || trimmed.chars().all(char::is_alphabetic)
        {
            return None;
        }
    }
    Some(FormulaSpan {
        range: open..close + 2,
        tex: open + 2..close,
        display,
    })
}

/// The bare `http://` / `https://` URL ranges of `text` (R2).
pub fn find_urls(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut urls = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let scheme = if bytes[i..].starts_with(b"https://") {
            8
        } else if bytes[i..].starts_with(b"http://") {
            7
        } else {
            i += 1;
            continue;
        };
        if i > 0 && char_before(text, i).is_some_and(|c| c.is_alphanumeric()) {
            i += scheme;
            continue;
        }
        let mut end = i + scheme;
        while end < bytes.len() {
            let c = char_at(text, end).unwrap();
            if c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`') {
                break;
            }
            end += c.len_utf8();
        }
        end = trim_url_end(text, i, end);
        if end > i + scheme {
            urls.push(i..end);
            i = end;
        } else {
            i += scheme;
        }
    }
    urls
}

fn trim_url_end(text: &str, start: usize, mut end: usize) -> usize {
    loop {
        let url = &text[start..end];
        let Some(last) = url.chars().next_back() else {
            return end;
        };
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_' | '~' => true,
            ')' => url.matches(')').count() > url.matches('(').count(),
            _ => false,
        };
        if !trim {
            return end;
        }
        end -= last.len_utf8();
    }
}

/// Positions (byte offsets) at which `<` and `>` must be inserted to wrap link destinations that
/// are absolute paths containing literal spaces (R3): `(open_paren + 1, close_paren)` pairs.
///
/// `text` is a paragraph, heading, cell or item source; `protected` masks code, formulas, raw
/// HTML and autolinks. Only once the closing parenthesis exists; the destination ends at the
/// first unescaped closing parenthesis at paren depth zero; a control character leaves the link
/// alone.
pub fn wrap_destinations(
    source: &str,
    container: Range<usize>,
    protected: &Protected,
) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let end = container.end.min(source.len());
    let mut out = Vec::new();
    let mut i = container.start;
    while i + 3 < end {
        if protected.contains(i) {
            i += 1;
            continue;
        }
        if bytes[i] == b']' && bytes[i + 1] == b'(' && bytes[i + 2] == b'/' {
            let start = i + 2;
            let mut depth = 0usize;
            let mut j = start;
            let mut spaces = false;
            let mut control = false;
            let mut close = None;
            while j < end {
                if protected.contains(j) {
                    break;
                }
                match bytes[j] {
                    b'\\' => {
                        j += 2;
                        continue;
                    }
                    b'(' => depth += 1,
                    b')' => {
                        if depth == 0 {
                            close = Some(j);
                            break;
                        }
                        depth -= 1;
                    }
                    b' ' => spaces = true,
                    b'<' | b'>' => control = true,
                    c if c < 0x20 || c == 0x7f => control = true,
                    _ => {}
                }
                j += 1;
            }
            if let Some(close) = close
                && spaces
                && !control
            {
                // The destination may carry a title after the path: only wrap when no quote.
                let dest = &source[start..close];
                if !dest.contains(['"', '\'']) {
                    out.push((start, close));
                    i = close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formulas(text: &str) -> Vec<(String, bool)> {
        scan_formulas(text, 0..text.len(), &Protected::default())
            .into_iter()
            .map(|span| (text[span.tex].to_owned(), span.display))
            .collect()
    }

    #[test]
    fn prices_and_shell_variables_stay_literal() {
        assert!(formulas("It costs $5 and $10 today").is_empty());
        assert!(formulas("echo $HOME and $PATH").is_empty());
        assert!(formulas("kill $$").is_empty());
        assert!(
            formulas("a$x$b").is_empty(),
            "identifier glued to both sides"
        );
        assert!(formulas("x = \"$a$\"").is_empty(), "quoted interpolation");
        assert!(formulas("$a$5").is_empty(), "closer glued to a digit");
    }

    #[test]
    fn dollar_pairs_with_star_and_underscore_are_math() {
        assert_eq!(
            formulas("$x^*$ and $y^*$"),
            vec![("x^*".into(), false), ("y^*".into(), false)]
        );
        assert_eq!(formulas("so $a_b$ holds"), vec![("a_b".into(), false)]);
        assert_eq!(formulas("$$x+y$$"), vec![("x+y".into(), true)]);
        assert!(formulas("$$$ x $$$").is_empty(), "longer runs are literal");
    }

    #[test]
    fn a_closer_may_open_the_next_pair_after_a_rejected_one() {
        // "$5" opens but the next dollar is preceded by a space; the later pair is math.
        assert_eq!(formulas("$5 and $x$"), vec![("x".into(), false)]);
    }

    #[test]
    fn backslash_delimiters_follow_the_prose_rules() {
        assert_eq!(formulas(r"\(x\)"), vec![("x".into(), false)]);
        assert_eq!(formulas(r"so \(x, y\) ok"), vec![("x, y".into(), false)]);
        assert!(
            formulas(r"array \[i\]").is_empty(),
            "display mid-line is prose"
        );
        assert!(formulas(r"f\(x\)").is_empty(), "glued stays text");
        assert!(
            formulas(r"see \(12\) and").is_empty(),
            "a bare number is a citation"
        );
        assert!(
            formulas(r"say \(hello world\)").is_empty(),
            "words are prose"
        );
        assert_eq!(
            formulas(r"\[\frac{a}{b}\]"),
            vec![(r"\frac{a}{b}".into(), true)]
        );
        assert!(formulas(r"\(a $ b\)").is_empty());
    }

    #[test]
    fn escaped_dollars_are_literal() {
        assert!(formulas(r"\$a\$").is_empty());
        assert_eq!(formulas(r"\\$a$"), vec![("a".into(), false)]);
    }

    #[test]
    fn protected_bytes_are_never_scanned() {
        let text = "`$a$` and $b$";
        let protected = Protected::new(vec![0..5]);
        let found = scan_formulas(text, 0..text.len(), &protected);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].tex.clone()], "b");
    }

    #[test]
    fn bare_urls_follow_the_trimming_rules() {
        let urls = |text: &str| -> Vec<String> {
            find_urls(text)
                .into_iter()
                .map(|r| text[r].to_owned())
                .collect()
        };
        assert_eq!(urls("see https://a.com/x."), ["https://a.com/x"]);
        assert_eq!(urls("(https://a.com/x)"), ["https://a.com/x"]);
        assert_eq!(urls("https://a.com/(x)"), ["https://a.com/(x)"]);
        assert_eq!(urls("https://a.com/x),"), ["https://a.com/x"]);
        assert_eq!(urls("a http://b.c/d?e=1; f"), ["http://b.c/d?e=1"]);
        assert!(urls("foohttps://x.y").is_empty(), "glued scheme");
        assert!(urls("https://").is_empty(), "scheme only");
        assert_eq!(urls("<https://a.b/c>"), ["https://a.b/c"]);
        assert_eq!(urls("https://a.b/c_*~"), ["https://a.b/c"]);
    }

    #[test]
    fn destinations_with_spaces_are_wrapped() {
        let text = "[doc](/home/me/My Docs/a.md) and [b](/x)";
        let wrapped = wrap_destinations(text, 0..text.len(), &Protected::default());
        assert_eq!(wrapped.len(), 1);
        assert_eq!(&text[wrapped[0].0..wrapped[0].1], "/home/me/My Docs/a.md");
        // Nested parentheses balance; the closing parenthesis must exist.
        let text = "[doc](/a (b) c.md)";
        assert_eq!(
            wrap_destinations(text, 0..text.len(), &Protected::default()).len(),
            1
        );
        let text = "[doc](/a b";
        assert!(wrap_destinations(text, 0..text.len(), &Protected::default()).is_empty());
    }
}
