//! List prefixes of single lines (R11) and the display length of a prefix (R7).

use super::text::{is_thematic_break, is_ws, quote_prefix};
use std::ops::Range;

/// A task box (`[ ]`, `[x]` or `[X]`) of a list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskBox {
    /// Byte range of the three box characters within the line.
    pub range: Range<usize>,
    /// Whether the box is checked.
    pub checked: bool,
}

/// The list prefix of one line: optional quote prefix, indentation, bullet or ordered marker,
/// separator and optional task box. All offsets are byte offsets within the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListPrefix {
    /// Where the indentation starts, which is the end of the quote prefix (0 without quotes).
    pub indent_start: usize,
    /// Offset of the bullet or of the first digit.
    pub marker_offset: usize,
    /// End of the marker (after the bullet, or after the `.`/`)` of an ordered marker).
    pub marker_end: usize,
    /// End of the whole prefix: marker, separator whitespace, task box and the whitespace after
    /// it. It equals the content end for an empty item.
    pub prefix_end: usize,
    /// Prefix Enter inserts for the next item: the quote prefix and indentation, the same bullet
    /// or the ordered number plus one with the same delimiter, the source's separator (one space
    /// when there is none) and `[ ] ` when the item has a task box.
    pub next_prefix: String,
    /// Text that replaces an empty item: the quote prefix and the indentation reduced by up to
    /// two whitespace characters.
    pub outdented_prefix: String,
    /// Offset of the bullet for `-`, `*` and `+` items; `None` for ordered items.
    pub bullet_offset: Option<usize>,
    /// The task box, when the item has one.
    pub task: Option<TaskBox>,
    /// Whether nothing but whitespace (and a task box) follows the marker.
    pub is_empty: bool,
}

/// A marker found by [`scan_item`].
pub(crate) struct ItemScan {
    pub marker_end: usize,
    pub bullet: Option<u8>,
    pub number: Option<u32>,
    pub delimiter: u8,
    pub separator_end: usize,
    pub task: Option<TaskBox>,
}

/// Scans a bullet or ordered marker at `marker`, its separator and its task box in `bytes[..end]`
/// (`end` is the content end of the line). A marker must be followed by a space, a tab or the end;
/// a task box must be followed by whitespace or the end and is only recognized after a separator.
pub(crate) fn scan_item(bytes: &[u8], marker: usize, end: usize) -> Option<ItemScan> {
    let first = *bytes.get(marker).filter(|_| marker < end)?;
    let (marker_end, bullet, number, delimiter) = if matches!(first, b'-' | b'*' | b'+') {
        (marker + 1, Some(first), None, first)
    } else if first.is_ascii_digit() {
        let digits = bytes[marker..end]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 9 {
            return None;
        }
        let delimiter = *bytes
            .get(marker + digits)
            .filter(|_| marker + digits < end)?;
        if delimiter != b'.' && delimiter != b')' {
            return None;
        }
        let number = std::str::from_utf8(&bytes[marker..marker + digits])
            .ok()?
            .parse::<u32>()
            .ok()?;
        (marker + digits + 1, None, Some(number), delimiter)
    } else {
        return None;
    };
    if marker_end < end && !is_ws(bytes[marker_end]) {
        return None;
    }
    let mut separator_end = marker_end;
    while separator_end < end && is_ws(bytes[separator_end]) {
        separator_end += 1;
    }
    let task = if separator_end > marker_end {
        scan_task(bytes, separator_end, end)
    } else {
        None
    };
    Some(ItemScan {
        marker_end,
        bullet,
        number,
        delimiter,
        separator_end,
        task,
    })
}

fn scan_task(bytes: &[u8], pos: usize, end: usize) -> Option<TaskBox> {
    if pos + 3 > end || bytes[pos] != b'[' || bytes[pos + 2] != b']' {
        return None;
    }
    let checked = match bytes[pos + 1] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    if pos + 3 < end && !is_ws(bytes[pos + 3]) {
        return None;
    }
    Some(TaskBox {
        range: pos..pos + 3,
        checked,
    })
}

/// The list prefix of `line` (R11), or `None` when the line is not a list line.
///
/// `line` is a single logical line; a trailing carriage return (and anything from a line feed
/// on) is ignored. The prefix is purely lexical: whether the line is inside code or a heading
/// is a question for the whole document (see [`newline_edit`](super::newline_edit)). A
/// thematic break (`- - -`, `___`) is not a list line, and a marker followed by non-space text
/// is not a marker.
pub fn list_prefix(line: &str) -> Option<ListPrefix> {
    let line = line.split('\n').next().unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let bytes = line.as_bytes();
    let (indent_start, _) = quote_prefix(line);
    let mut marker_offset = indent_start;
    while marker_offset < bytes.len() && is_ws(bytes[marker_offset]) {
        marker_offset += 1;
    }
    let scan = scan_item(bytes, marker_offset, bytes.len())?;
    if is_thematic_break(&line[indent_start..]) {
        return None;
    }
    let mut prefix_end = scan.separator_end;
    if let Some(task) = &scan.task {
        prefix_end = task.range.end;
        while prefix_end < bytes.len() && is_ws(bytes[prefix_end]) {
            prefix_end += 1;
        }
    }
    let separator = if scan.separator_end > scan.marker_end {
        &line[scan.marker_end..scan.separator_end]
    } else {
        " "
    };
    let mut next_prefix = String::from(&line[..marker_offset]);
    match scan.number {
        Some(number) => {
            next_prefix.push_str(&number.saturating_add(1).to_string());
            next_prefix.push(scan.delimiter as char);
        }
        None => next_prefix.push(scan.delimiter as char),
    }
    next_prefix.push_str(separator);
    if scan.task.is_some() {
        next_prefix.push_str("[ ] ");
    }
    let indentation = &line[indent_start..marker_offset];
    let kept = indentation.len().saturating_sub(2);
    let mut outdented_prefix = String::from(&line[..indent_start]);
    outdented_prefix.push_str(&indentation[indentation.len() - kept..]);
    Some(ListPrefix {
        indent_start,
        marker_offset,
        marker_end: scan.marker_end,
        prefix_end,
        next_prefix,
        outdented_prefix,
        bullet_offset: scan.bullet.map(|_| marker_offset),
        task: scan.task,
        is_empty: prefix_end == bytes.len(),
    })
}

/// Byte length, within the display text, of the list prefix of a source line (R7): the marker,
/// separator and task box as the projection shows them.
///
/// `line` is the source line starting at source offset `line_start`; `decorations` are the
/// document's replacements from [`decorations`](super::decorations) (sorted, non-overlapping,
/// absolute source ranges). Each replacement inside the prefix contributes its display text in
/// place of its source bytes. Returns `None` when the line has no list prefix.
pub fn list_prefix_display_len(
    line: &str,
    line_start: usize,
    decorations: &[(Range<usize>, String)],
) -> Option<usize> {
    let prefix = list_prefix(line)?;
    let end = line_start + prefix.prefix_end;
    let first = decorations.partition_point(|(range, _)| range.end <= line_start);
    let mut len = prefix.prefix_end;
    for (range, display) in &decorations[first..] {
        if range.start >= end {
            break;
        }
        let covered = range.start.max(line_start)..range.end.min(end);
        len = len - covered.len() + display.len();
    }
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(line: &str) -> String {
        list_prefix(line).expect("list line").next_prefix
    }

    #[test]
    fn next_prefixes_follow_r11() {
        assert_eq!(next("9. item"), "10. ");
        assert_eq!(next("  - [x] done"), "  - [ ] ");
        assert_eq!(next("-\titem"), "-\t");
        assert_eq!(next("12)\titem"), "13)\t");
        assert_eq!(next("+   item"), "+   ");
        assert_eq!(next("> - item"), "> - ");
        assert_eq!(next(">> 9) item"), ">> 10) ");
        assert_eq!(next(">   - child"), ">   - ");
        assert_eq!(next("> > - [x] done"), "> > - [ ] ");
        assert_eq!(next("-"), "- ");
        assert_eq!(next("1."), "2. ");
        assert_eq!(next("* [ ]"), "* [ ] ");
        assert_eq!(next("999999999. x"), "1000000000. ");
    }

    #[test]
    fn outdented_prefixes_follow_r11() {
        let outdented = |line: &str| list_prefix(line).expect("list line").outdented_prefix;
        assert_eq!(outdented("- "), "");
        assert_eq!(outdented("> - "), "> ");
        assert_eq!(outdented(">     - "), ">   ");
        assert_eq!(outdented("    - "), "  ");
        assert_eq!(outdented(" - "), "");
        assert_eq!(outdented("\t- "), "");
        assert_eq!(outdented("1."), "");
    }

    #[test]
    fn empty_items() {
        for line in ["- ", "- [ ]", "1.", "- [X]\t", "-", "> - ", "- [x]\r"] {
            assert!(list_prefix(line).expect(line).is_empty, "{line:?}");
        }
        for line in ["- a", "- [x]word", "1. [ ] a", "- [x] a"] {
            assert!(!list_prefix(line).expect(line).is_empty, "{line:?}");
        }
    }

    #[test]
    fn offsets() {
        let prefix = list_prefix("> \t1) [X]  text").expect("list line");
        assert_eq!(prefix.indent_start, 2);
        assert_eq!(prefix.marker_offset, 3);
        assert_eq!(prefix.marker_end, 5);
        assert_eq!(prefix.prefix_end, 11);
        assert_eq!(prefix.bullet_offset, None);
        assert_eq!(
            prefix.task,
            Some(TaskBox {
                range: 6..9,
                checked: true
            })
        );
        let bullet = list_prefix("  * x").expect("list line");
        assert_eq!(bullet.indent_start, 0);
        assert_eq!(bullet.marker_offset, 2);
        assert_eq!(bullet.bullet_offset, Some(2));
        assert_eq!(bullet.prefix_end, 4);
    }

    #[test]
    fn not_lists() {
        for line in [
            "text",
            "-text",
            "1.5",
            "10",
            "- - -",
            "***",
            "___",
            "* * *",
            "1234567890. x",
            "+x",
            "",
            "#",
            "[x] a",
            ">",
        ] {
            assert_eq!(list_prefix(line), None, "{line:?}");
        }
    }

    #[test]
    fn task_boxes_need_whitespace_after() {
        assert!(list_prefix("- [x]word").expect("list").task.is_none());
        assert!(list_prefix("- [x]").expect("list").task.is_some());
        assert!(list_prefix("- [y] a").expect("list").task.is_none());
        assert!(list_prefix("-[x] a").is_none());
    }

    #[test]
    fn carriage_return_is_not_part_of_the_line() {
        let prefix = list_prefix("- [ ]\r").expect("list line");
        assert_eq!(prefix.prefix_end, 5);
        assert!(prefix.is_empty);
        assert_eq!(list_prefix("- a\r\nb").expect("list line").prefix_end, 2);
    }

    #[test]
    fn display_length_of_the_prefix_follows_the_replacements() {
        // "- [x] done" with the bullet and box shown as one glyph.
        let decorations = vec![(0..5, "\u{2611}".to_string())];
        assert_eq!(
            list_prefix_display_len("- [x] done", 0, &decorations),
            Some("\u{2611} ".len())
        );
        // Active line: only the bullet is replaced.
        let decorations = vec![(0..1, "\u{2022}".to_string())];
        assert_eq!(
            list_prefix_display_len("- [x] done", 0, &decorations),
            Some("\u{2022} [x] ".len())
        );
        // Offsets are absolute: the line starts at 4 and an earlier replacement is ignored.
        let decorations = vec![(0..2, String::new()), (4..5, "\u{2022}".to_string())];
        assert_eq!(
            list_prefix_display_len("- a", 4, &decorations),
            Some("\u{2022} ".len())
        );
        assert_eq!(list_prefix_display_len("text", 0, &decorations), None);
        // No decorations: the source length.
        assert_eq!(list_prefix_display_len("12) x", 0, &[]), Some(4));
    }
}
