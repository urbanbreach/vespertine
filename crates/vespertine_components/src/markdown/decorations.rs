//! Display decorations (R4): replacements that hide delimiters and show glyphs.

use super::list::scan_item;
use super::parse::{Heading, InlineKind, Parsed};
use super::text::{content_end, intersects, is_ws, line_end, line_start, lines_covering};
use std::ops::Range;

/// Bullet shown for `-`, `*` and `+` list markers.
const BULLET: &str = "\u{2022}";
/// Glyph shown for a checked task box.
const CHECKED: &str = "\u{2611}";
/// Glyph shown for an unchecked task box.
const UNCHECKED: &str = "\u{2610}";

/// The replacements (source range, display text) of `source` for the active range `active`
/// (R4), sorted by start and non-overlapping. Source text is never changed.
pub(crate) fn compute(source: &str, active: Range<usize>) -> Vec<(Range<usize>, String)> {
    let active = lines_covering(source, active);
    let parsed = Parsed::parse(source);
    let mut out: Vec<(Range<usize>, String)> = Vec::new();
    for element in &parsed.inline {
        if !intersects(&element.range, &active) {
            hide_delimiters(source, element.range.clone(), element.kind, &mut out);
        }
    }
    for heading in &parsed.headings {
        if !intersects(&heading.range, &active) {
            hide_heading_markers(source, heading, &mut out);
        }
    }
    for item in &parsed.items {
        decorate_item(source, item.start, &active, &mut out);
    }
    out.sort_by_key(|(range, _)| (range.start, range.end));
    let mut sorted = Vec::with_capacity(out.len());
    let mut last_end = 0;
    for (range, text) in out {
        if range.start >= last_end {
            last_end = range.end;
            sorted.push((range, text));
        }
    }
    sorted
}

/// Hides the delimiter pair of an inline element: strong 2 characters, emphasis 1, strikethrough
/// and inline code the leading run length `n`; only when the element is longer than `2n`.
fn hide_delimiters(
    source: &str,
    range: Range<usize>,
    kind: InlineKind,
    out: &mut Vec<(Range<usize>, String)>,
) {
    let bytes = source.as_bytes();
    let run = |mark: u8| {
        bytes[range.start..range.end]
            .iter()
            .take_while(|&&byte| byte == mark)
            .count()
    };
    let (n, marks): (usize, &[u8]) = match kind {
        InlineKind::Strong => (2, b"*_"),
        InlineKind::Emphasis => (1, b"*_"),
        InlineKind::Strikethrough => (run(b'~'), b"~"),
        InlineKind::Code => (run(b'`'), b"`"),
    };
    if n == 0 || range.len() <= 2 * n {
        return;
    }
    let leading = range.start..range.start + n;
    let trailing = range.end - n..range.end;
    let is_delimiter =
        |part: &Range<usize>| bytes[part.clone()].iter().all(|byte| marks.contains(byte));
    if is_delimiter(&leading) && is_delimiter(&trailing) {
        out.push((leading, String::new()));
        out.push((trailing, String::new()));
    }
}

/// Hides the opening hashes (with the indentation before them at the top level and the
/// whitespace after them) and a closing hash sequence of an ATX heading; setext headings are
/// left alone.
fn hide_heading_markers(source: &str, heading: &Heading, out: &mut Vec<(Range<usize>, String)>) {
    let bytes = source.as_bytes();
    let start = heading.range.start;
    let hashes = bytes[start..]
        .iter()
        .take_while(|&&byte| byte == b'#')
        .count();
    let after = start + hashes;
    let atx = (1..=6).contains(&hashes)
        && bytes
            .get(after)
            .is_none_or(|&byte| is_ws(byte) || byte == b'\r' || byte == b'\n');
    if !atx {
        return;
    }
    let line_begin = line_start(source, start);
    let line_finish = content_end(source, line_begin, line_end(source, start));
    let mut lead_start = start;
    if heading.container_depth == 0 && bytes[line_begin..start].iter().all(|&byte| is_ws(byte)) {
        lead_start = line_begin;
    }
    let mut content = after;
    while content < line_finish && is_ws(bytes[content]) {
        content += 1;
    }
    let mut text_end = line_finish;
    while text_end > content && is_ws(bytes[text_end - 1]) {
        text_end -= 1;
    }
    let mut run_start = text_end;
    while run_start > content && bytes[run_start - 1] == b'#' {
        run_start -= 1;
    }
    let closing = run_start < text_end && (run_start == content || is_ws(bytes[run_start - 1]));
    let mut leading = lead_start..content;
    if closing {
        let mut close_start = run_start;
        while close_start > content && is_ws(bytes[close_start - 1]) {
            close_start -= 1;
        }
        if close_start == content {
            leading.end = text_end;
            out.push((leading, String::new()));
        } else {
            out.push((leading, String::new()));
            out.push((close_start..text_end, String::new()));
        }
    } else {
        out.push((leading, String::new()));
    }
}

/// Shows a bullet as "\u{2022}" on every line and a task box as a glyph on inactive lines.
fn decorate_item(
    source: &str,
    marker: usize,
    active: &Range<usize>,
    out: &mut Vec<(Range<usize>, String)>,
) {
    let line_begin = line_start(source, marker);
    let line_finish = content_end(source, line_begin, line_end(source, marker));
    let Some(scan) = scan_item(source.as_bytes(), marker, line_finish) else {
        return;
    };
    let inactive_task = scan
        .task
        .filter(|_| !intersects(&(line_begin..line_finish), active));
    let glyph = |checked: bool| if checked { CHECKED } else { UNCHECKED };
    match (scan.bullet.is_some(), inactive_task) {
        (true, Some(task)) => out.push((marker..task.range.end, glyph(task.checked).to_string())),
        (true, None) => out.push((marker..marker + 1, BULLET.to_string())),
        (false, Some(task)) => out.push((task.range, glyph(task.checked).to_string())),
        (false, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The display text of `source` with `active` as the active range.
    fn render(source: &str, active: Range<usize>) -> String {
        let mut text = String::new();
        let mut at = 0;
        let replacements = compute(source, active);
        for (range, display) in &replacements {
            text.push_str(&source[at..range.start]);
            text.push_str(display);
            at = range.end;
        }
        text.push_str(&source[at..]);
        text
    }

    /// Display text when nothing in `source` is active (an extra final line is active).
    fn inactive(source: &str) -> String {
        let extended = format!("{source}\n");
        let rendered = render(&extended, extended.len()..extended.len());
        rendered
            .strip_suffix('\n')
            .expect("the extra line survives")
            .to_string()
    }

    #[test]
    fn nested_emphasis_and_strong_are_hidden() {
        assert_eq!(
            inactive("***both*** and **bold _nested_**"),
            "both and bold nested"
        );
    }

    #[test]
    fn delimiters_reappear_when_the_line_is_active() {
        let source = "**bold** and ~~gone~~ and `code`";
        assert_eq!(inactive(source), "bold and gone and code");
        assert_eq!(render(source, 3..3), source);
        assert_eq!(render(source, 0..source.len()), source);
        assert_eq!(render(source, source.len()..source.len()), source);
    }

    #[test]
    fn only_the_active_line_shows_its_delimiters() {
        let source = "**a**\n**b**";
        assert_eq!(render(source, 0..0), "**a**\nb");
        assert_eq!(render(source, 6..6), "a\n**b**");
        assert_eq!(render(source, 0..source.len()), source);
    }

    #[test]
    fn a_multi_line_element_is_active_as_a_whole() {
        let source = "**a\nb**\nc";
        assert_eq!(render(source, 4..4), source);
        assert_eq!(render(source, source.len()..source.len()), "a\nb\nc");
    }

    #[test]
    fn task_and_bullet_glyphs() {
        let source = "- [x] done\n- [ ] pending\n- [x]word\n1. [X] ordered";
        assert_eq!(
            inactive(source),
            "\u{2611} done\n\u{2610} pending\n\u{2022} [x]word\n1. \u{2611} ordered"
        );
        assert_eq!(render("- [x] done", 0..0), "\u{2022} [x] done");
        assert_eq!(render("- [x] done", 10..10), "\u{2022} [x] done");
    }

    #[test]
    fn bullets_of_every_kind_show_on_all_lines() {
        assert_eq!(
            render("- a\n* b\n+ c", 0..0),
            "\u{2022} a\n\u{2022} b\n\u{2022} c"
        );
        assert_eq!(inactive("1. a\n2) b"), "1. a\n2) b");
        assert_eq!(inactive("- [x]\n- [ ]"), "\u{2611}\n\u{2610}");
    }

    #[test]
    fn task_in_a_quote_keeps_the_quote_prefix() {
        assert_eq!(inactive("> - [x] done"), "> \u{2611} done");
        assert_eq!(inactive("> 1. [ ] todo"), "> 1. \u{2610} todo");
    }

    #[test]
    fn atx_headings_lose_their_markers_when_inactive() {
        assert_eq!(inactive("# Caf\u{e9}"), "Caf\u{e9}");
        assert_eq!(inactive("  ### Caf\u{e9} ###"), "Caf\u{e9}");
        assert_eq!(inactive("###### Caf\u{e9}\t##"), "Caf\u{e9}");
        assert_eq!(inactive("> ## Caf\u{e9} ##"), "> Caf\u{e9}");
        assert_eq!(inactive("- ## Caf\u{e9} ##"), "\u{2022} Caf\u{e9}");
        for source in ["# Caf\u{e9}", "  ### Caf\u{e9} ###", "###### Caf\u{e9}\t##"] {
            assert_eq!(render(source, 0..0), source);
        }
    }

    #[test]
    fn heading_edge_cases() {
        assert_eq!(inactive("# foo \\#"), "foo \\#");
        assert_eq!(inactive("# foo#"), "foo#");
        assert_eq!(inactive("# foo ##  "), "foo  ");
        assert_eq!(inactive("#"), "");
        assert_eq!(inactive("###"), "");
        assert_eq!(inactive("# #"), "");
        assert_eq!(inactive("## ##"), "");
        assert_eq!(inactive("####### seven"), "####### seven");
        assert_eq!(inactive("#hashtag"), "#hashtag");
        assert_eq!(inactive("# a\r\n# b"), "a\r\nb");
    }

    #[test]
    fn setext_headings_and_underlines_are_untouched() {
        assert_eq!(inactive("Caf\u{e9}\n===="), "Caf\u{e9}\n====");
        assert_eq!(inactive("Title\n-"), "Title\n-");
        assert_eq!(inactive("Title\n---"), "Title\n---");
    }

    #[test]
    fn nothing_inside_code_or_breaks_is_decorated() {
        assert_eq!(inactive("`**x**`"), "**x**");
        assert_eq!(
            inactive("```\n- a\n**b**\n# c\n```"),
            "```\n- a\n**b**\n# c\n```"
        );
        assert_eq!(inactive("    - a\n    **b**"), "    - a\n    **b**");
        assert_eq!(inactive("---\n***\n___\n- - -"), "---\n***\n___\n- - -");
        assert_eq!(
            inactive("a\n\n> text\n\n    > literal"),
            "a\n\n> text\n\n    > literal"
        );
        assert_eq!(inactive("\\~\\~literal\\~\\~"), "\\~\\~literal\\~\\~");
        assert_eq!(inactive("- `- a`"), "\u{2022} - a");
    }

    #[test]
    fn a_literal_quote_marker_is_untouched() {
        assert_eq!(inactive("a\n\n    > - [x] a"), "a\n\n    > - [x] a");
        assert_eq!(inactive("\\> - [x] a"), "\\> - [x] a");
        assert_eq!(
            inactive("> - [x] a\n> - [ ] b"),
            "> \u{2611} a\n> \u{2610} b"
        );
    }

    #[test]
    fn code_spans_and_fences_hide_their_backtick_runs() {
        assert_eq!(inactive("```inline```"), "inline");
        assert_eq!(inactive("`a` and ``b ` c``"), "a and b ` c");
        assert_eq!(inactive("``"), "``");
        assert_eq!(inactive("` `"), " ");
    }

    #[test]
    fn strikethrough_runs_hide_n_tildes() {
        assert_eq!(inactive("~~gone~~ ~one~"), "gone one");
        assert_eq!(inactive("~~~"), "~~~");
    }

    #[test]
    fn replacements_are_sorted_non_overlapping_and_on_char_boundaries() {
        let source = "# **Caf\u{e9}** `x` ~~y~~ _z_ #\n- [x] **\u{e9}**\n1. [ ] a";
        let replacements = compute(source, source.len()..source.len());
        let mut at = 0;
        for (range, _) in &replacements {
            assert!(range.start >= at, "{replacements:?}");
            assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
            at = range.end;
        }
        assert!(!replacements.is_empty());
    }

    #[test]
    fn r15_three_replacements_per_bold_item() {
        let source = "- **\u{e9}**\n".repeat(10_000);
        let replacements = compute(&source, source.len()..source.len());
        assert_eq!(replacements.len(), 30_000);
        assert!(
            replacements
                .windows(2)
                .all(|pair| pair[0].0.end <= pair[1].0.start)
        );
        assert!(
            replacements
                .iter()
                .all(|(range, _)| source.is_char_boundary(range.start)
                    && source.is_char_boundary(range.end))
        );
    }

    #[test]
    fn r15_one_replacement_per_task_line() {
        let source = "- [x] t\n".repeat(10_000);
        let replacements = compute(&source, source.len()..source.len());
        assert_eq!(replacements.len(), 10_000);
    }
}
