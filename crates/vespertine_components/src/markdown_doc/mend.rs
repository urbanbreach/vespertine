//! Display-only repair of the hanging inline syntax of a streaming block (CMP-011 R17).
//!
//! [`mend`] takes the source of the last top-level block and returns the text the display tree is
//! parsed from, or `None` when nothing hangs. The stored source and the canonical tree never see
//! the repaired text.

use super::scan::{Protected, find_urls, scan_formulas};
use super::tree::PENDING_LINK;
use std::ops::Range;

/// Zero-width space appended after a final line that could read as a setext underline.
pub const ZERO_WIDTH_SPACE: char = '\u{200b}';

#[derive(Clone, Debug)]
enum Hang {
    Emphasis { ch: u8, count: usize },
    Bracket,
}

/// Repairs hanging inline syntax at the end of `tail`; `None` when nothing needs repair.
pub fn mend(tail: &str) -> Option<String> {
    let bytes = tail.as_bytes();
    let mut protected: Vec<Range<usize>> = Vec::new();
    let mut hanging_code: Option<usize> = None;

    // Code spans: matched backtick runs are opaque; an unmatched last run hangs and swallows the
    // rest of the block.
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'`' => {
                let n = run(bytes, i, b'`');
                match find_run(bytes, i + n, b'`', n) {
                    Some(close) => {
                        protected.push(i..close + n);
                        i = close + n;
                    }
                    None => {
                        hanging_code = Some(n);
                        protected.push(i..bytes.len());
                        i = bytes.len();
                    }
                }
            }
            _ => i += 1,
        }
    }
    for url in find_urls(tail) {
        protected.push(url);
    }
    let protected = Protected::new(protected);

    // Formulas: complete ones are opaque to emphasis; a hanging display opener may need its
    // closer completed; an unclosed formula stays literal (and shields what follows).
    let formulas = scan_formulas(tail, 0..tail.len(), &protected);
    let mut opaque: Vec<Range<usize>> = protected.ranges().to_vec();
    opaque.extend(formulas.iter().map(|f| f.range.clone()));
    let opaque = Protected::new(opaque);

    let mut stack: Vec<Hang> = Vec::new();
    let mut insert_before: Vec<(usize, String)> = Vec::new();
    let mut cut: Option<usize> = None;
    let mut display_completion: Option<&'static str> = None;
    let mut shielded_from: Option<usize> = None;

    let mut i = 0;
    let formula_starts: Vec<(usize, bool)> = formulas
        .iter()
        .map(|f| (f.range.start, f.display))
        .collect();
    while i < bytes.len() {
        if opaque.contains(i) {
            // A complete display formula ends any emphasis or link opened before it.
            if let Some((_, true)) = formula_starts.iter().find(|(start, _)| *start == i)
                && !stack.is_empty()
            {
                insert_before.push((i, closers(&mut stack)));
            }
            i += 1;
            continue;
        }
        match bytes[i] {
            b'\\' => {
                // `\[` hanging display opener.
                if bytes.get(i + 1) == Some(&b'[') && shielded_from.is_none() {
                    if let Some(end) = tail[i + 2..].find("\\]") {
                        i += 2 + end + 2;
                        continue;
                    }
                    if tail.ends_with('\\') && tail.len() > i + 2 {
                        if !stack.is_empty() {
                            insert_before.push((i, closers(&mut stack)));
                        }
                        display_completion = Some("]");
                    } else if !stack.is_empty() && tail[i + 2..].trim().len() > 0 {
                        // A display opener that has not closed yet still ends emphasis before it.
                        insert_before.push((i, closers(&mut stack)));
                    }
                    shielded_from = Some(i);
                    i = bytes.len();
                    continue;
                }
                i += 2;
            }
            b'$' => {
                let n = run(bytes, i, b'$');
                if n == 2 {
                    // An unclosed display opener: complete a half-streamed closer, shield the
                    // rest from emphasis.
                    if !stack.is_empty() {
                        insert_before.push((i, closers(&mut stack)));
                    }
                    let body = &tail[i + 2..];
                    if body.trim_end().ends_with('$')
                        && body.trim_end().len() > 1
                        && !body.trim_end().ends_with("$$")
                        && body.trim().len() > 1
                    {
                        display_completion = Some("$");
                    }
                    shielded_from = Some(i);
                    i = bytes.len();
                    continue;
                }
                // A single dollar followed by a non-space with no closer: literal, and what
                // follows is formula text that emphasis must not see.
                if n == 1
                    && tail[i + 1..]
                        .chars()
                        .next()
                        .is_some_and(|c| !c.is_whitespace())
                {
                    shielded_from = Some(i);
                    i = bytes.len();
                    continue;
                }
                i += n;
            }
            b'*' | b'_' | b'~' => {
                let ch = bytes[i];
                let n = run(bytes, i, ch);
                let before = tail[..i].chars().next_back();
                let after = tail[i + n..].chars().next();
                let left = after.is_some_and(|c| !c.is_whitespace());
                let right = before.is_some_and(|c| !c.is_whitespace());
                let punct = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
                let (can_open, can_close) = if ch == b'_' {
                    (
                        left && (!right || punct(before)),
                        right && (!left || punct(after)),
                    )
                } else {
                    (left, right)
                };
                // Markers glued to bullets or thematic runs never match anything.
                if ch == b'~' && n > 2 {
                    i += n;
                    continue;
                }
                let mut remaining = n;
                if can_close {
                    while remaining > 0 {
                        let Some(position) = stack.iter().rposition(
                            |hang| matches!(hang, Hang::Emphasis { ch: c, .. } if *c == ch),
                        ) else {
                            break;
                        };
                        // Everything opened after the matched opener is literal now.
                        stack.truncate(position + 1);
                        let Some(Hang::Emphasis { count, .. }) = stack.last_mut() else {
                            break;
                        };
                        let used = if *count >= 2 && remaining >= 2 { 2 } else { 1 }
                            .min(remaining)
                            .min(*count);
                        *count -= used;
                        remaining -= used;
                        if *count == 0 {
                            stack.pop();
                        }
                    }
                }
                if remaining > 0 && can_open {
                    stack.push(Hang::Emphasis {
                        ch,
                        count: remaining,
                    });
                }
                i += n;
            }
            b'!' if bytes.get(i + 1) == Some(&b'[') => {
                stack.push(Hang::Bracket);
                i += 2;
            }
            b'[' => {
                stack.push(Hang::Bracket);
                i += 1;
            }
            b']' => {
                // Emphasis opened inside the link text cannot cross its end.
                if let Some(position) = stack.iter().rposition(|hang| matches!(hang, Hang::Bracket))
                {
                    stack.truncate(position);
                    if bytes.get(i + 1) == Some(&b'(') {
                        match find_destination_end(tail, i + 2) {
                            Some(end) => {
                                i = end + 1;
                                continue;
                            }
                            None => {
                                cut = Some(i + 2);
                                i = bytes.len();
                                continue;
                            }
                        }
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }

    let mut hanging_links = 0usize;
    for hang in &stack {
        if matches!(hang, Hang::Bracket) {
            hanging_links += 1;
        }
    }

    // Assemble.
    let mut out = String::with_capacity(tail.len() + 8);
    let body_end = cut.unwrap_or(tail.len());
    insert_before.sort_by_key(|(at, _)| *at);
    let mut last = 0;
    for (at, text) in &insert_before {
        if *at > body_end {
            continue;
        }
        out.push_str(&tail[last..*at]);
        out.push_str(text);
        last = *at;
    }
    out.push_str(&tail[last..body_end]);
    let mut changed = !insert_before.is_empty();
    if cut.is_some() {
        out.push_str(PENDING_LINK);
        out.push(')');
        changed = true;
    }
    if let Some(n) = hanging_code {
        out.extend(std::iter::repeat_n('`', n));
        changed = true;
    }
    if let Some(closer) = display_completion {
        out.push_str(closer);
        changed = true;
    }
    // Closers of what is still open, innermost first. Brackets hang as `](pending)`.
    let mut closing = String::new();
    for hang in stack.iter().rev() {
        match hang {
            Hang::Emphasis { ch, count } => {
                closing.extend(std::iter::repeat_n(*ch as char, *count));
            }
            Hang::Bracket => {
                closing.push_str("](");
                closing.push_str(PENDING_LINK);
                closing.push(')');
            }
        }
    }
    if hanging_code.is_some() {
        // Emphasis inside a hanging code span is code text: nothing else to close.
        closing.clear();
    }
    if !closing.is_empty() {
        out.push_str(&closing);
        changed = true;
    }
    let _ = hanging_links;

    // A last line of only "-", "--", "=" or "==" must not read as a setext underline.
    let last_line = out.rsplit('\n').next().unwrap_or("");
    if matches!(last_line.trim_end(), "-" | "--" | "=" | "==") && last_line == last_line.trim_end()
    {
        out.push(ZERO_WIDTH_SPACE);
        changed = true;
    }
    changed.then_some(out)
}

/// Closers of every open emphasis (innermost first); empties the stack.
fn closers(stack: &mut Vec<Hang>) -> String {
    let mut out = String::new();
    for hang in stack.iter().rev() {
        match hang {
            Hang::Emphasis { ch, count } => out.extend(std::iter::repeat_n(*ch as char, *count)),
            Hang::Bracket => {
                out.push_str("](");
                out.push_str(PENDING_LINK);
                out.push(')');
            }
        }
    }
    stack.clear();
    out
}

fn run(bytes: &[u8], at: usize, byte: u8) -> usize {
    bytes[at..].iter().take_while(|b| **b == byte).count()
}

/// The start of the next run of exactly `n` of `byte` at or after `from`.
fn find_run(bytes: &[u8], from: usize, byte: u8, n: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == byte {
            let len = run(bytes, i, byte);
            if len == n {
                return Some(i);
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}

/// The index of the `)` closing a link destination that starts at `from`, or `None`.
fn find_destination_end(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            b'\n' if bytes.get(i + 1) == Some(&b'\n') => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(text: &str) -> String {
        mend(text).unwrap_or_else(|| text.to_owned())
    }

    #[test]
    fn nothing_hanging_costs_nothing() {
        assert_eq!(mend("plain text"), None);
        assert_eq!(mend("**bold** and *it* and `code` and [a](b)"), None);
        assert_eq!(mend("snake_case_name and 2 * 3 = 6"), None);
    }

    #[test]
    fn unmatched_emphasis_is_closed_innermost_first() {
        assert_eq!(m("**bold"), "**bold**");
        assert_eq!(m("a **bold *and italic"), "a **bold *and italic***");
        assert_eq!(m("~~gone"), "~~gone~~");
        assert_eq!(m("*one **two"), "*one **two***");
    }

    #[test]
    fn a_lone_trailing_asterisk_completes_a_pair() {
        assert_eq!(m("**bold*"), "**bold**");
        assert_eq!(m("**bold**"), "**bold**");
    }

    #[test]
    fn code_backtick_parity() {
        assert_eq!(m("use `foo"), "use `foo`");
        assert_eq!(m("use ``foo ` bar"), "use ``foo ` bar``");
        assert_eq!(mend("use `foo` bar"), None);
    }

    #[test]
    fn unfinished_links_and_images_get_a_pending_destination() {
        assert_eq!(m("[text](http://exa"), "[text](pending-link:)");
        assert_eq!(m("see [text](http://exa"), "see [text](pending-link:)");
        assert_eq!(m("[tex"), "[tex](pending-link:)");
        assert_eq!(m("![alt](ima"), "![alt](pending-link:)");
        assert_eq!(m("**[a](htt"), "**[a](pending-link:)**");
        assert_eq!(mend("[a](http://x.y)"), None);
    }

    #[test]
    fn formulas_shield_their_text_and_stay_unclosed() {
        // The star inside an unclosed formula never makes emphasis.
        assert_eq!(mend("so $x^*"), None);
        assert_eq!(mend("$x^* and $y^*$"), None);
        assert_eq!(m("$$ x + y $"), "$$ x + y $$");
        assert_eq!(mend("$$ x + y"), None);
        // A display formula ends emphasis opened before it.
        assert_eq!(m("**bold $$x$$ more"), "**bold **$$x$$ more");
    }

    #[test]
    fn a_final_setext_lookalike_gets_a_zero_width_space() {
        assert_eq!(m("item\n-"), format!("item\n-{ZERO_WIDTH_SPACE}"));
        assert_eq!(m("item\n=="), format!("item\n=={ZERO_WIDTH_SPACE}"));
        assert_eq!(mend("item\n---"), None);
    }

    #[test]
    fn escaped_markers_never_hang() {
        assert_eq!(mend(r"a \* b"), None);
        assert_eq!(mend(r"a \[ b"), None);
    }
}
