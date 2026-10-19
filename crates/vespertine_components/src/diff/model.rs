//! The patch model and its tolerant parser (CMP-015 R1-R5).
//!
//! [`parse_patch`] turns Git unified patch text into [`DiffFile`]s. It never fails: unknown header
//! lines are skipped and a truncated or malformed patch keeps whatever parsed so far.

use std::cell::Cell;

/// How a file changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileStatus {
    /// Content (or mode) changed in place; the default status.
    #[default]
    Modified,
    /// The file was created.
    Added,
    /// The file was removed.
    Deleted,
    /// The file was renamed (possibly with a content change).
    Renamed,
}

/// The kind of one line inside a hunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    /// Present on both sides.
    Context,
    /// Present only on the new side.
    Added,
    /// Present only on the old side.
    Deleted,
    /// A backslash line such as `\ No newline at end of file`; it has no numbers.
    Meta,
}

/// One line of a hunk: its kind, its text (the line minus its first character) and its numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// Classification by the first character.
    pub kind: LineKind,
    /// The text without the leading marker character. For a meta line: the text with leading
    /// backslashes and surrounding whitespace removed.
    pub text: String,
    /// Old-side line number (`None` for added and meta lines).
    pub old: Option<u32>,
    /// New-side line number (`None` for deleted and meta lines).
    pub new: Option<u32>,
}

/// One `@@ ... @@` hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    /// The full header line, kept for display.
    pub header: String,
    /// Old start line from the header.
    pub old_start: u32,
    /// New start line from the header.
    pub new_start: u32,
    /// The hunk's lines in patch order.
    pub lines: Vec<DiffLine>,
}

/// One file of a patch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffFile {
    /// Display path: the new-side path (empty for a patch without a file preamble).
    pub path: String,
    /// The old path, recorded only when it differs from [`DiffFile::path`].
    pub old_path: Option<String>,
    /// Whether the patch only says the contents are binary.
    pub binary: bool,
    /// Added, deleted, renamed or modified.
    pub status: FileStatus,
    /// Parser notices (mode changes) and the truncation notice, in order. The status notices
    /// of R4 come first in [`file_notices`].
    pub notices: Vec<String>,
    /// The hunks.
    pub hunks: Vec<DiffHunk>,
    /// Number of added lines.
    pub additions: usize,
    /// Number of deleted lines.
    pub deletions: usize,
    /// The largest old or new line number seen.
    pub max_line: u32,
}

impl DiffFile {
    /// Number of diff lines (every line of every hunk, meta lines included).
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|hunk| hunk.lines.len()).sum()
    }

    /// The largest line number of one side (`old` or new) shown by the patch, 0 without lines.
    pub fn max_line_of_side(&self, old: bool) -> u32 {
        self.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter_map(|line| if old { line.old } else { line.new })
            .max()
            .unwrap_or(0)
    }
}

thread_local! {
    static PARSES: Cell<u64> = const { Cell::new(0) };
}

/// Number of times [`parse_patch`] ran on this thread. A diagnostic that lets a host (and the
/// tests) verify that a color-only theme change never reparses (R8, R18).
pub fn parse_count() -> u64 {
    PARSES.with(Cell::get)
}

/// Parses a unified Git patch (R1-R3). LF separates lines and a trailing CR is dropped.
pub fn parse_patch(text: &str) -> Vec<DiffFile> {
    PARSES.with(|count| count.set(count.get() + 1));
    let mut parser = Parser::default();
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for line in lines {
        parser.line(line.strip_suffix('\r').unwrap_or(line));
    }
    parser.finish()
}

#[derive(Default)]
struct Parser {
    files: Vec<DiffFile>,
    file: Option<DiffFile>,
    hunk: Option<DiffHunk>,
    old: u32,
    new: u32,
}

impl Parser {
    fn finish(mut self) -> Vec<DiffFile> {
        self.close_file();
        self.files
    }

    fn close_hunk(&mut self) {
        if let (Some(hunk), Some(file)) = (self.hunk.take(), self.file.as_mut()) {
            file.hunks.push(hunk);
        }
    }

    fn close_file(&mut self) {
        self.close_hunk();
        if let Some(file) = self.file.take() {
            self.files.push(file);
        }
    }

    fn line(&mut self, line: &str) {
        if self.hunk.is_some() && self.hunk_line(line) {
            return;
        }
        self.header_line(line);
    }

    /// Reads a line inside a hunk; `false` means the line ends the hunk and is a header line.
    fn hunk_line(&mut self, line: &str) -> bool {
        let (Some(hunk), Some(file)) = (self.hunk.as_mut(), self.file.as_mut()) else {
            return false;
        };
        let first = line.chars().next();
        let rest = first.map_or("", |c| &line[c.len_utf8()..]);
        let entry = match first {
            Some('+') => {
                let number = self.new;
                self.new = self.new.saturating_add(1);
                file.additions += 1;
                file.max_line = file.max_line.max(number);
                DiffLine {
                    kind: LineKind::Added,
                    text: rest.to_owned(),
                    old: None,
                    new: Some(number),
                }
            }
            Some('-') => {
                let number = self.old;
                self.old = self.old.saturating_add(1);
                file.deletions += 1;
                file.max_line = file.max_line.max(number);
                DiffLine {
                    kind: LineKind::Deleted,
                    text: rest.to_owned(),
                    old: Some(number),
                    new: None,
                }
            }
            Some(' ') | None => {
                let (old, new) = (self.old, self.new);
                self.old = self.old.saturating_add(1);
                self.new = self.new.saturating_add(1);
                file.max_line = file.max_line.max(old).max(new);
                DiffLine {
                    kind: LineKind::Context,
                    text: rest.to_owned(),
                    old: Some(old),
                    new: Some(new),
                }
            }
            Some('\\') => DiffLine {
                kind: LineKind::Meta,
                text: line.trim_start_matches('\\').trim().to_owned(),
                old: None,
                new: None,
            },
            Some(_) => {
                self.close_hunk();
                return false;
            }
        };
        hunk.lines.push(entry);
        true
    }

    fn header_line(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.close_file();
            let (path, old_path) = split_git_paths(rest);
            self.file = Some(DiffFile {
                path,
                old_path,
                ..DiffFile::default()
            });
            return;
        }
        if self.file.is_none() {
            if line.starts_with("@@") || line.starts_with("--- ") {
                self.file = Some(DiffFile::default());
            } else {
                return;
            }
        }
        if line.starts_with("@@") {
            self.open_hunk(line);
            return;
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if line.starts_with("new file mode") {
            file.status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(from.trim().to_owned());
        } else if let Some(to) = line.strip_prefix("rename to ") {
            file.status = FileStatus::Renamed;
            file.path = to.trim().to_owned();
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            file.binary = true;
        } else if let Some(mode) = line.strip_prefix("new mode ") {
            file.notices
                .push(format!("Mode changed to {}", mode.trim()));
        } else if let Some(target) = line.strip_prefix("+++ ") {
            let target = target.trim();
            if target == "/dev/null" {
                file.status = FileStatus::Deleted;
            } else if file.old_path.is_none() {
                file.path = strip_side_prefix(target).to_owned();
            }
        } else if line.trim_end() == "--- /dev/null" {
            file.status = FileStatus::Added;
        }
        // index, similarity, old mode and every unknown header line are ignored.
    }

    fn open_hunk(&mut self, line: &str) {
        let Some((old_start, new_start)) = parse_hunk_header(line) else {
            return;
        };
        self.close_hunk();
        self.old = old_start;
        self.new = new_start;
        self.hunk = Some(DiffHunk {
            header: line.to_owned(),
            old_start,
            new_start,
            lines: Vec::new(),
        });
    }
}

/// The first decimal number after the first `-` and after the first `+` of a hunk header, each
/// ended by a comma or whitespace; `None` when either does not parse as an unsigned 32-bit
/// integer (R3).
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    fn number_after(text: &str, marker: char) -> Option<u32> {
        let rest = &text[text.find(marker)? + 1..];
        let end = rest
            .find(|c: char| c == ',' || c.is_whitespace())
            .unwrap_or(rest.len());
        let digits = &rest[..end];
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    }
    let body = &line[2..];
    Some((number_after(body, '-')?, number_after(body, '+')?))
}

/// Removes a leading `a/` or `b/` (R1).
fn strip_side_prefix(path: &str) -> &str {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
}

/// Trims, unquotes (resolving `\"` and `\\`) and strips the side prefix of one path (R1).
fn clean_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        strip_side_prefix(&out).to_owned()
    } else {
        strip_side_prefix(trimmed).to_owned()
    }
}

/// Splits the remainder of a `diff --git` line into the display path and the old path (recorded
/// only when it differs). The separator is the LAST ` b/` (or ` "b/` when quoted).
fn split_git_paths(rest: &str) -> (String, Option<String>) {
    let plain = rest.rfind(" b/");
    let quoted = rest.rfind(" \"b/");
    let separator = match (plain, quoted) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    match separator {
        Some(index) => {
            let old = clean_path(&rest[..index]);
            let new = clean_path(&rest[index + 1..]);
            let old_path = (old != new).then_some(old);
            (new, old_path)
        }
        None => (clean_path(rest), None),
    }
}

/// The notices of a file, in order (R4): `New file`, `Deleted file`, `Renamed from <old or ?>`,
/// `Binary file — contents not shown`, then the parser notices (mode changes, truncation).
pub fn file_notices(file: &DiffFile) -> Vec<String> {
    let mut notices = Vec::new();
    match file.status {
        FileStatus::Added => notices.push("New file".to_owned()),
        FileStatus::Deleted => notices.push("Deleted file".to_owned()),
        FileStatus::Renamed => notices.push(format!(
            "Renamed from {}",
            file.old_path.as_deref().unwrap_or("?")
        )),
        FileStatus::Modified => {}
    }
    if file.binary {
        notices.push("Binary file \u{2014} contents not shown".to_owned());
    }
    notices.extend(file.notices.iter().cloned());
    notices
}

/// Caps a file's hunks at `max_lines` total diff lines (R5). Hunks are truncated in order and
/// later hunks dropped; the notice `Diff truncated — showing first N of T lines` is appended
/// (T = the total before the cap) and the maximum line number is recomputed from what remains so
/// gutters fit what renders. A file within the cap is left unchanged.
pub fn truncate_file_lines(file: &mut DiffFile, max_lines: usize) {
    let total = file.line_count();
    if total <= max_lines {
        return;
    }
    let mut remaining = max_lines;
    let mut kept = Vec::new();
    for mut hunk in std::mem::take(&mut file.hunks) {
        if remaining == 0 {
            break;
        }
        hunk.lines.truncate(remaining);
        remaining -= hunk.lines.len();
        kept.push(hunk);
    }
    file.hunks = kept;
    file.max_line = file
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .flat_map(|line| [line.old, line.new])
        .flatten()
        .max()
        .unwrap_or(0);
    file.notices.push(format!(
        "Diff truncated \u{2014} showing first {max_lines} of {total} lines"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(patch: &str) -> DiffFile {
        let mut files = parse_patch(patch);
        assert_eq!(files.len(), 1, "{files:?}");
        files.remove(0)
    }

    #[test]
    fn modified_file_with_one_hunk() {
        let file = one(
            "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@ fn x\n ctx\n-old\n+new\n\\ No newline at end of file\n",
        );
        assert_eq!(file.path, "src/a.rs");
        assert_eq!(file.old_path, None);
        assert_eq!(file.status, FileStatus::Modified);
        assert_eq!((file.additions, file.deletions, file.max_line), (1, 1, 2));
        let hunk = &file.hunks[0];
        assert_eq!(hunk.header, "@@ -1,2 +1,2 @@ fn x");
        let kinds: Vec<_> = hunk.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Context,
                LineKind::Deleted,
                LineKind::Added,
                LineKind::Meta
            ]
        );
        assert_eq!(hunk.lines[0].old, Some(1));
        assert_eq!(hunk.lines[0].new, Some(1));
        assert_eq!(hunk.lines[1].old, Some(2));
        assert_eq!(hunk.lines[1].new, None);
        assert_eq!(hunk.lines[2].new, Some(2));
        assert_eq!(hunk.lines[3].text, "No newline at end of file");
        assert_eq!(hunk.lines[3].old, None);
    }

    #[test]
    fn added_and_deleted_files() {
        let added = one(
            "diff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\n",
        );
        assert_eq!(added.status, FileStatus::Added);
        assert_eq!(file_notices(&added), ["New file"]);
        assert_eq!(added.hunks[0].old_start, 0);
        let deleted = one(
            "diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n",
        );
        assert_eq!(deleted.status, FileStatus::Deleted);
        assert_eq!(deleted.path, "gone.txt");
        assert_eq!(file_notices(&deleted), ["Deleted file"]);
    }

    #[test]
    fn renamed_with_and_without_content_change() {
        let pure = one(
            "diff --git a/old name.txt b/new name.txt\nsimilarity index 100%\nrename from old name.txt\nrename to new name.txt\n",
        );
        assert_eq!(pure.status, FileStatus::Renamed);
        assert_eq!(pure.path, "new name.txt");
        assert_eq!(pure.old_path.as_deref(), Some("old name.txt"));
        assert!(pure.hunks.is_empty());
        assert_eq!(file_notices(&pure), ["Renamed from old name.txt"]);
        let changed = one(
            "diff --git a/a.rs b/b.rs\nsimilarity index 80%\nrename from a.rs\nrename to b.rs\n--- a/a.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-x\n+y\n",
        );
        assert_eq!(changed.path, "b.rs");
        assert_eq!(changed.old_path.as_deref(), Some("a.rs"));
        assert_eq!(changed.hunks.len(), 1);
        let unknown = DiffFile {
            status: FileStatus::Renamed,
            ..DiffFile::default()
        };
        assert_eq!(file_notices(&unknown), ["Renamed from ?"]);
    }

    #[test]
    fn binary_and_mode_change() {
        let binary = one("diff --git a/i.png b/i.png\nBinary files a/i.png and b/i.png differ\n");
        assert!(binary.binary);
        assert_eq!(
            file_notices(&binary),
            ["Binary file \u{2014} contents not shown"]
        );
        let git_binary = one("diff --git a/i.bin b/i.bin\nGIT binary patch\nliteral 3\n");
        assert!(git_binary.binary);
        let mode = one("diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n");
        assert_eq!(mode.notices, ["Mode changed to 100755"]);
        assert_eq!(file_notices(&mode), ["Mode changed to 100755"]);
    }

    #[test]
    fn notice_order_is_status_binary_then_parser_notices() {
        let file = DiffFile {
            status: FileStatus::Added,
            binary: true,
            notices: vec!["Mode changed to 100755".into()],
            ..DiffFile::default()
        };
        assert_eq!(
            file_notices(&file),
            [
                "New file",
                "Binary file \u{2014} contents not shown",
                "Mode changed to 100755"
            ]
        );
    }

    #[test]
    fn quoted_paths_with_spaces_and_escapes() {
        let file =
            one("diff --git \"a/we ird \\\"q\\\" \\\\x.txt\" \"b/we ird \\\"q\\\" \\\\x.txt\"\n");
        assert_eq!(file.path, "we ird \"q\" \\x.txt");
        assert_eq!(file.old_path, None);
        let spaced = one("diff --git a/dir with space/f g.txt b/dir with space/f g.txt\n");
        assert_eq!(spaced.path, "dir with space/f g.txt");
        assert_eq!(spaced.old_path, None);
        let renamed = one("diff --git \"a/old one.txt\" \"b/new one.txt\"\n");
        assert_eq!(renamed.path, "new one.txt");
        assert_eq!(renamed.old_path.as_deref(), Some("old one.txt"));
    }

    #[test]
    fn without_separator_one_path_is_used_for_both() {
        let file = one("diff --git foo.txt\n");
        assert_eq!(file.path, "foo.txt");
        assert_eq!(file.old_path, None);
    }

    #[test]
    fn plain_unified_output_and_bare_hunks_yield_one_unnamed_file() {
        let plain = one("--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(plain.path, "x.txt");
        assert_eq!(plain.hunks.len(), 1);
        let bare = one("@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(bare.path, "");
        assert_eq!(bare.old_path, None);
        assert_eq!(bare.additions, 1);
        assert!(parse_patch("nothing useful here\n").is_empty());
        assert!(parse_patch("").is_empty());
    }

    #[test]
    fn truncated_and_malformed_patches_keep_what_parsed() {
        let files = parse_patch(
            "diff --git a/a b/a\n@@ -1 +1 @@\n-x\n+y\ngarbage\ndiff --git a/b b/b\nindex\n@@ -1,3 +1",
        );
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].hunks[0].lines.len(), 2);
        assert_eq!(
            files[1].hunks.len(),
            1,
            "a truncated header still opens an empty hunk"
        );
        assert!(files[1].hunks[0].lines.is_empty());
        // CR is dropped, unknown headers skipped, and a stray '+' line outside a hunk ignored.
        let files = parse_patch(
            "diff --git a/a b/a\r\nweird: header\r\n+stray\r\n@@ -1 +1 @@\r\n-x\r\n+y\r\n",
        );
        assert_eq!(files[0].hunks[0].lines[0].text, "x");
        assert_eq!(files[0].hunks[0].lines.len(), 2);
    }

    #[test]
    fn hunk_numbers_must_fit_32_bits_or_the_header_is_ignored() {
        let file = one("diff --git a/a b/a\n@@ -99999999999 +1 @@\n-x\n@@ -1 +1 @@\n-y\n");
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.hunks[0].lines[0].text, "y");
        let huge = one("diff --git a/a b/a\n@@ -4000000000,2 +4000000000,2 @@\n-x\n+y\n");
        assert_eq!(huge.hunks[0].lines[0].old, Some(4_000_000_000));
        assert_eq!(huge.max_line, 4_000_000_000);
        let edge = one("diff --git a/a b/a\n@@ -4294967295 +4294967295 @@\n x\n x\n");
        assert_eq!(edge.hunks[0].lines[1].new, Some(u32::MAX), "saturates");
    }

    #[test]
    fn hunk_lines_classify_by_first_character_and_other_characters_end_the_hunk() {
        let file = one(
            "diff --git a/a b/a\n@@ -1,3 +1,3 @@\n a\n\n--- not a header\n+++ also a line\n\\\\  spaced  \nindex 1..2\n",
        );
        let lines = &file.hunks[0].lines;
        assert_eq!(lines.len(), 5, "{lines:?}");
        assert_eq!(lines[1].kind, LineKind::Context, "empty line is context");
        assert_eq!(lines[2].kind, LineKind::Deleted);
        assert_eq!(lines[2].text, "-- not a header");
        assert_eq!(lines[3].kind, LineKind::Added);
        assert_eq!(lines[4].kind, LineKind::Meta);
        assert_eq!(lines[4].text, "spaced");
    }

    #[test]
    fn two_files_and_hunk_after_a_non_hunk_line() {
        let files = parse_patch(
            "diff --git a/a b/a\n@@ -1 +1 @@\n-x\nindex ignored\n@@ -9 +9 @@\n+z\ndiff --git a/b b/b\n@@ -1 +1 @@\n y\n",
        );
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[1].lines[0].new, Some(9));
        assert_eq!(files[1].path, "b");
    }

    #[test]
    fn truncation_caps_hunks_notice_and_max_line() {
        let mut file =
            one("diff --git a/a b/a\n@@ -1,3 +1,3 @@\n a\n-b\n+c\n@@ -100,2 +100,2 @@\n x\n-y\n");
        assert_eq!(file.max_line, 101);
        truncate_file_lines(&mut file, 4);
        assert_eq!(file.hunks.len(), 2);
        assert_eq!(file.hunks[1].lines.len(), 1);
        assert_eq!(file.max_line, 100);
        assert_eq!(
            file.notices,
            ["Diff truncated \u{2014} showing first 4 of 5 lines"]
        );
        let mut early = one("diff --git a/a b/a\n@@ -1,2 +1,2 @@\n a\n-b\n@@ -9 +9 @@\n+z\n");
        truncate_file_lines(&mut early, 2);
        assert_eq!(early.hunks.len(), 1, "later hunks are dropped");
        let mut small = one("diff --git a/a b/a\n@@ -1 +1 @@\n a\n");
        truncate_file_lines(&mut small, 10);
        assert!(small.notices.is_empty());
    }
}
