//! Syntax coloration of diffs (CMP-015 R14, R15): per side, per file.
//!
//! A diff interleaves two file versions, so each side of each file is highlighted separately:
//! the old side from deleted and context lines, the new side from added and context lines. Only
//! the lines the patch shows are joined (with LF) into one source per side and the per-line
//! spans are mapped back to their real line numbers. Owners may instead supply the full old and
//! new documents, which are used whole when every diff line agrees with them.

use super::model::{DiffFile, DiffLine, LineKind};
use std::collections::HashMap;
use std::sync::Arc;
use vespertine_richtext::syntax::{
    HighlightCache, HighlightRequest, HighlightedDocument, Span, detect,
};

/// A side whose largest line number exceeds this is not highlighted (R14); it bounds the table
/// for hostile hunk numbers.
pub const MAX_HIGHLIGHT_LINE: u32 = 200_000;

/// The full old and new documents of one file that an owner supplies (R15).
#[derive(Clone, Debug, Default)]
pub struct FileDocuments {
    /// Display path of the file these documents belong to.
    pub path: String,
    /// The complete old document.
    pub old: Option<Arc<str>>,
    /// The complete new document.
    pub new: Option<Arc<str>>,
    /// The owner marks the documents stale.
    pub stale: bool,
    /// The owner marks the file binary.
    pub binary: bool,
    /// The owner marks the documents truncated.
    pub truncated: bool,
}

enum Index {
    /// Excerpt coloration: line number to index in the joined source.
    Excerpt(HashMap<u32, usize>),
    /// Whole-document coloration: line number `n` is document line `n - 1`.
    Whole,
}

/// The spans of one side of a file.
pub struct SideSpans {
    document: Option<Arc<HighlightedDocument>>,
    index: Index,
}

impl SideSpans {
    fn none() -> Self {
        Self {
            document: None,
            index: Index::Excerpt(HashMap::new()),
        }
    }

    /// The spans of the line with this number on this side (empty when the patch never shows it
    /// or the side is not highlighted).
    pub fn spans(&self, number: u32) -> &[Span] {
        let Some(document) = &self.document else {
            return &[];
        };
        let index = match &self.index {
            Index::Excerpt(map) => map.get(&number).copied(),
            Index::Whole => (number as usize).checked_sub(1),
        };
        index
            .and_then(|index| document.lines().get(index))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether this side has a highlighted document.
    pub fn is_highlighted(&self) -> bool {
        self.document.is_some()
    }
}

/// The spans of both sides of a file (R14).
pub struct FileHighlights {
    old: SideSpans,
    new: SideSpans,
}

fn highlight_source(
    cache: &HighlightCache,
    path: &str,
    source: &str,
) -> Option<Arc<HighlightedDocument>> {
    let request = HighlightRequest::new(source).path(path);
    cache.highlight(&request)
}

/// The path used to detect a side's language: the old path for the old side of a renamed file.
fn side_path(file: &DiffFile, old: bool) -> &str {
    if old {
        file.old_path.as_deref().unwrap_or(&file.path)
    } else {
        &file.path
    }
}

fn side_lines(file: &DiffFile, old: bool) -> impl Iterator<Item = (u32, &DiffLine)> {
    file.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter_map(move |line| match (line.kind, old) {
            (LineKind::Deleted | LineKind::Context, true) => line.old.map(|n| (n, line)),
            (LineKind::Added | LineKind::Context, false) => line.new.map(|n| (n, line)),
            _ => None,
        })
}

fn excerpt_side(file: &DiffFile, old: bool, cache: &HighlightCache) -> SideSpans {
    let path = side_path(file, old);
    if path.is_empty() || file.binary || file.max_line_of_side(old) > MAX_HIGHLIGHT_LINE {
        return SideSpans::none();
    }
    let mut source = String::new();
    let mut index = HashMap::new();
    for (position, (number, line)) in side_lines(file, old).enumerate() {
        if position > 0 {
            source.push('\n');
        }
        source.push_str(&line.text);
        index.insert(number, position);
    }
    if index.is_empty() {
        return SideSpans::none();
    }
    SideSpans {
        document: highlight_source(cache, path, &source),
        index: Index::Excerpt(index),
    }
}

/// The lines of a document split like the highlighter splits them (LF, trailing CR dropped, no
/// phantom last line).
fn document_lines(document: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = document
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Whether owner-supplied documents are accepted atomically (R15). They are rejected when the
/// owner marks them stale, binary or truncated, when any non-meta diff line's text differs from
/// the corresponding document line (deleted: old document at the old number; added: new document
/// at the new number; context: new document at the new number, else old), or when a required
/// document is absent for a language that is supported.
pub fn documents_accepted(file: &DiffFile, documents: &FileDocuments) -> bool {
    if documents.stale || documents.binary || documents.truncated || file.binary {
        return false;
    }
    let old_lines = documents.old.as_deref().map(document_lines);
    let new_lines = documents.new.as_deref().map(document_lines);
    let supported = |old: bool| detect(None, Some(side_path(file, old)), None).is_some();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            let (document, number, old_side) = match line.kind {
                LineKind::Meta => continue,
                LineKind::Deleted => (&old_lines, line.old, true),
                LineKind::Added => (&new_lines, line.new, false),
                LineKind::Context => {
                    if new_lines.is_some() {
                        (&new_lines, line.new, false)
                    } else {
                        (&old_lines, line.old, true)
                    }
                }
            };
            let Some(lines) = document else {
                if supported(old_side) {
                    return false;
                }
                continue;
            };
            let Some(number) = number else {
                return false;
            };
            let Some(text) = (number as usize)
                .checked_sub(1)
                .and_then(|index| lines.get(index))
            else {
                return false;
            };
            if *text != line.text {
                return false;
            }
        }
    }
    true
}

impl FileHighlights {
    /// Highlights a file: from the owner's whole documents when they are accepted (R15), else
    /// from the excerpts the patch shows (R14).
    pub fn build(
        file: &DiffFile,
        cache: &HighlightCache,
        documents: Option<&FileDocuments>,
    ) -> Self {
        if file.binary || file.path.is_empty() {
            return Self {
                old: SideSpans::none(),
                new: SideSpans::none(),
            };
        }
        if let Some(documents) = documents.filter(|d| documents_accepted(file, d)) {
            let whole = |source: &Option<Arc<str>>, old: bool| match source {
                Some(source) => SideSpans {
                    document: highlight_source(cache, side_path(file, old), source),
                    index: Index::Whole,
                },
                None => SideSpans::none(),
            };
            return Self {
                old: whole(&documents.old, true),
                new: whole(&documents.new, false),
            };
        }
        Self {
            old: excerpt_side(file, true, cache),
            new: excerpt_side(file, false, cache),
        }
    }

    /// The spans of a line (R14): a deleted line uses the old spans, an added line the new
    /// spans, a context line prefers the new side and falls back to the old; meta lines have
    /// none.
    pub fn line_spans(&self, line: &DiffLine) -> &[Span] {
        match line.kind {
            LineKind::Deleted => line.old.map_or(&[], |n| self.old.spans(n)),
            LineKind::Added => line.new.map_or(&[], |n| self.new.spans(n)),
            LineKind::Context => {
                let new = line.new.map_or(&[][..], |n| self.new.spans(n));
                if !new.is_empty() || self.new.is_highlighted() {
                    new
                } else {
                    line.old.map_or(&[], |n| self.old.spans(n))
                }
            }
            LineKind::Meta => &[],
        }
    }

    /// The old-side table.
    pub fn old_side(&self) -> &SideSpans {
        &self.old
    }

    /// The new-side table.
    pub fn new_side(&self) -> &SideSpans {
        &self.new
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::model::parse_patch;
    use vespertine_richtext::syntax::TokenKind;

    fn file(patch: &str) -> DiffFile {
        parse_patch(patch).remove(0)
    }

    fn kinds(spans: &[Span]) -> Vec<(String, TokenKind)> {
        spans
            .iter()
            .map(|span| (format!("{:?}", span.range), span.kind))
            .collect()
    }

    #[test]
    fn each_side_is_highlighted_from_its_own_lines() {
        let file = file(
            "diff --git a/a.rs b/a.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    let x = 1;\n+    let y = \"s\";\n }\n",
        );
        let cache = HighlightCache::new();
        let highlights = FileHighlights::build(&file, &cache, None);
        let lines = &file.hunks[0].lines;
        let deleted = highlights.line_spans(&lines[1]);
        let added = highlights.line_spans(&lines[2]);
        assert!(!deleted.is_empty(), "old side highlighted");
        assert!(
            added.iter().any(|span| span.kind == TokenKind::String),
            "new side sees its own string: {:?}",
            kinds(added)
        );
        assert!(
            !deleted.iter().any(|span| span.kind == TokenKind::String),
            "the old side never sees new-side text"
        );
        let context = highlights.line_spans(&lines[0]);
        assert!(!context.is_empty());
    }

    #[test]
    fn spans_are_stored_at_real_line_numbers_without_padding_gaps() {
        let file = file(
            "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n /* open\n@@ -50 +50 @@\n close */ let a = 1;\n",
        );
        let cache = HighlightCache::new();
        let highlights = FileHighlights::build(&file, &cache, None);
        let new = highlights.new_side();
        assert!(new.spans(1).iter().any(|s| s.kind == TokenKind::Comment));
        assert!(
            new.spans(2).is_empty(),
            "lines the patch never shows stay empty"
        );
        // The joined excerpt continues the grammar state across the gap: line 50 is still inside
        // the comment that opened on line 1.
        assert!(new.spans(50).iter().any(|s| s.kind == TokenKind::Comment));
    }

    #[test]
    fn huge_line_numbers_skip_highlighting_of_that_side_only() {
        let file = file(
            "diff --git a/a.rs b/a.rs\n@@ -3000000000,2 +1,2 @@\n-let a = 1;\n-let b = 2;\n+let a = 3;\n+let b = 4;\n",
        );
        let cache = HighlightCache::new();
        let highlights = FileHighlights::build(&file, &cache, None);
        assert!(!highlights.old_side().is_highlighted());
        assert!(highlights.new_side().is_highlighted());
    }

    #[test]
    fn languages_come_from_each_sides_own_path_and_binary_or_unnamed_files_are_plain() {
        let renamed = file(
            "diff --git a/old.py b/new.rs\nrename from old.py\nrename to new.rs\n@@ -1 +1 @@\n-def f(): pass\n+fn f() {}\n",
        );
        let cache = HighlightCache::new();
        let highlights = FileHighlights::build(&renamed, &cache, None);
        let lines = &renamed.hunks[0].lines;
        assert!(
            highlights
                .line_spans(&lines[0])
                .iter()
                .any(|s| s.kind == TokenKind::Keyword),
            "python def"
        );
        assert!(
            highlights
                .line_spans(&lines[1])
                .iter()
                .any(|s| s.kind == TokenKind::Keyword)
        );
        let bare = file("@@ -1 +1 @@\n-let a = 1;\n+let a = 2;\n");
        assert!(
            !FileHighlights::build(&bare, &cache, None)
                .new_side()
                .is_highlighted()
        );
        let binary = file("diff --git a/x.rs b/x.rs\nBinary files a/x.rs and b/x.rs differ\n");
        assert!(
            !FileHighlights::build(&binary, &cache, None)
                .new_side()
                .is_highlighted()
        );
    }

    #[test]
    fn context_prefers_the_new_side_and_falls_back_to_old() {
        let deleted_only =
            file("diff --git a/a.rs b/a.rs\n@@ -1,2 +1 @@\n-let a = 1;\n let b = 2;\n");
        let cache = HighlightCache::new();
        let highlights = FileHighlights::build(&deleted_only, &cache, None);
        let ctx = &deleted_only.hunks[0].lines[1];
        assert!(!highlights.line_spans(ctx).is_empty());
    }

    #[test]
    fn whole_documents_are_accepted_only_when_every_line_agrees() {
        let file = file(
            "diff --git a/a.rs b/a.rs\n@@ -2,2 +2,2 @@\n-let a = 1;\n+let a = 2;\n let b = 3;\n",
        );
        let old: Arc<str> = Arc::from("fn x() {\nlet a = 1;\nlet b = 3;\n}\n");
        let new: Arc<str> = Arc::from("fn x() {\nlet a = 2;\nlet b = 3;\n}\n");
        let good = FileDocuments {
            path: "a.rs".into(),
            old: Some(old),
            new: Some(new),
            ..FileDocuments::default()
        };
        assert!(documents_accepted(&file, &good));
        let cache = HighlightCache::new();
        let highlights = FileHighlights::build(&file, &cache, Some(&good));
        let lines = &file.hunks[0].lines;
        assert!(!highlights.line_spans(&lines[1]).is_empty());
        for flag in 0..3 {
            let mut bad = good.clone();
            match flag {
                0 => bad.stale = true,
                1 => bad.binary = true,
                _ => bad.truncated = true,
            }
            assert!(!documents_accepted(&file, &bad));
        }
        let mismatch = FileDocuments {
            new: Some(Arc::from("fn x() {\nlet a = 9;\nlet b = 3;\n}\n")),
            ..good.clone()
        };
        assert!(!documents_accepted(&file, &mismatch));
        let missing_old = FileDocuments {
            old: None,
            ..good.clone()
        };
        assert!(
            !documents_accepted(&file, &missing_old),
            "deleted line needs the old document"
        );
        let short = FileDocuments {
            new: Some(Arc::from("x\n")),
            ..good
        };
        assert!(!documents_accepted(&file, &short));
    }

    #[test]
    fn a_missing_document_of_an_unsupported_language_does_not_reject() {
        let file = file("diff --git a/a.xyz b/a.xyz\n@@ -1 +1 @@\n-one\n+two\n");
        let docs = FileDocuments {
            new: Some(Arc::from("two\n")),
            ..FileDocuments::default()
        };
        assert!(documents_accepted(&file, &docs));
    }
}
