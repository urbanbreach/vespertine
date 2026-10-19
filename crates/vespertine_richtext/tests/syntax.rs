//! TXT-007 acceptance tests: language resolution, token classification, span construction,
//! limits, the bounded cache, theme recoloring and the cache-stress benchmark.

use std::sync::Arc;
use vespertine::{Hsla, TextRun, font, rgb};
use vespertine_richtext::syntax::*;

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

fn highlight_as(language: Language, source: &str) -> HighlightedDocument {
    highlight_language(language, source, Limits::default()).expect("highlight succeeds")
}

/// `(text, kind)` of every span of the document, flattened over all lines.
fn tokens<'a>(source: &'a str, document: &HighlightedDocument) -> Vec<(&'a str, TokenKind)> {
    let contents: Vec<&str> = {
        let mut lines: Vec<&str> = source.split('\n').collect();
        if source.ends_with('\n') {
            lines.pop();
        }
        lines
    };
    let mut out = Vec::new();
    for (index, spans) in document.lines().iter().enumerate() {
        for span in spans {
            out.push((&contents[index][span.range.clone()], span.kind));
        }
    }
    out
}

fn kind_of(source: &str, document: &HighlightedDocument, text: &str) -> Option<TokenKind> {
    tokens(source, document)
        .into_iter()
        .find(|(token, _)| *token == text)
        .map(|(_, kind)| kind)
}

// ---------------------------------------------------------------- A1: language resolution

#[test]
fn every_alias_resolves_to_its_language() {
    let table: &[(&str, Language)] = &[
        ("rust", Language::Rust),
        ("rs", Language::Rust),
        ("javascript", Language::JavaScript),
        ("js", Language::JavaScript),
        ("mjs", Language::JavaScript),
        ("cjs", Language::JavaScript),
        ("jsx", Language::Jsx),
        ("typescript", Language::TypeScript),
        ("ts", Language::TypeScript),
        ("mts", Language::TypeScript),
        ("cts", Language::TypeScript),
        ("tsx", Language::Tsx),
        ("python", Language::Python),
        ("py", Language::Python),
        ("python3", Language::Python),
        ("go", Language::Go),
        ("golang", Language::Go),
        ("json", Language::Json),
        ("jsonc", Language::Jsonc),
        ("bash", Language::Shell),
        ("sh", Language::Shell),
        ("shell", Language::Shell),
        ("zsh", Language::Shell),
        ("console", Language::Shell),
        ("toml", Language::Toml),
        ("markdown", Language::Markdown),
        ("md", Language::Markdown),
        ("html", Language::Html),
        ("htm", Language::Html),
        ("css", Language::Css),
        ("yaml", Language::Yaml),
        ("yml", Language::Yaml),
        ("c", Language::C),
        ("h", Language::C),
    ];
    for (alias, language) in table {
        assert_eq!(resolve_alias(alias), Some(*language), "alias {alias}");
    }
    let resolved: std::collections::HashSet<Language> =
        table.iter().map(|(_, language)| *language).collect();
    assert_eq!(resolved.len(), 16);
    assert_eq!(Language::ALL.len(), 16);
    for language in Language::ALL {
        assert!(resolved.contains(&language));
    }
}

#[test]
fn aliases_use_the_first_token_and_ignore_case() {
    assert_eq!(resolve_alias("  TS foo "), Some(Language::TypeScript));
    assert_eq!(resolve_alias("RUST"), Some(Language::Rust));
    assert_eq!(resolve_alias("Py\tignored words"), Some(Language::Python));
    assert_eq!(resolve_alias("\n  JSON"), Some(Language::Json));
    assert_eq!(resolve_alias("foo ts"), None);
    assert_eq!(resolve_alias(""), None);
    assert_eq!(resolve_alias("   "), None);
    assert_eq!(resolve_alias("rustacean"), None);
    assert_eq!(resolve_alias("c++"), None);
}

#[test]
fn paths_resolve_by_basename_then_extension() {
    for name in [
        "Cargo.toml",
        "cargo.toml",
        "CARGO.TOML",
        "Cargo.lock",
        "cargo.lock",
        "CARGO.LOCK",
        "pyproject.toml",
        "PyProject.TOML",
        "/work/project/Cargo.lock",
        "C:\\work\\Cargo.toml",
    ] {
        assert_eq!(resolve_path(name), Some(Language::Toml), "{name}");
    }
    let by_extension: &[(&str, Language)] = &[
        ("src/main.rs", Language::Rust),
        ("a.js", Language::JavaScript),
        ("a.MJS", Language::JavaScript),
        ("a.cjs", Language::JavaScript),
        ("a.jsx", Language::Jsx),
        ("a.ts", Language::TypeScript),
        ("a.mts", Language::TypeScript),
        ("a.cts", Language::TypeScript),
        ("a.tsx", Language::Tsx),
        ("a.py", Language::Python),
        ("a.go", Language::Go),
        ("a.json", Language::Json),
        ("a.jsonc", Language::Jsonc),
        ("a.sh", Language::Shell),
        ("a.zsh", Language::Shell),
        ("a.bash", Language::Shell),
        ("a.toml", Language::Toml),
        ("README.md", Language::Markdown),
        ("a.markdown", Language::Markdown),
        ("index.html", Language::Html),
        ("index.HTM", Language::Html),
        ("a.css", Language::Css),
        ("a.yaml", Language::Yaml),
        ("a.yml", Language::Yaml),
        ("a.c", Language::C),
        ("a.h", Language::C),
    ];
    for (path, language) in by_extension {
        assert_eq!(resolve_path(path), Some(*language), "{path}");
    }
    assert_eq!(resolve_path("Makefile"), None);
    assert_eq!(resolve_path("notes.txt"), None);
    assert_eq!(resolve_path("dir.rs/file"), None);
    assert_eq!(resolve_path(""), None);
}

#[test]
fn shebangs_resolve_python_node_and_shells() {
    assert_eq!(
        resolve_shebang("#!/usr/bin/env python3"),
        Some(Language::Python)
    );
    assert_eq!(resolve_shebang("#!/usr/bin/PYTHON"), Some(Language::Python));
    assert_eq!(
        resolve_shebang("#!/usr/bin/env node"),
        Some(Language::JavaScript)
    );
    assert_eq!(resolve_shebang("#!/bin/bash"), Some(Language::Shell));
    assert_eq!(resolve_shebang("#!/usr/bin/zsh -f"), Some(Language::Shell));
    assert_eq!(resolve_shebang("#!/bin/sh"), Some(Language::Shell));
    assert_eq!(resolve_shebang("#!/usr/bin/env sh"), Some(Language::Shell));
    assert_eq!(resolve_shebang("#!/usr/bin/perl"), None);
    assert_eq!(resolve_shebang("#! /usr/bin/ruby"), None);
    assert_eq!(resolve_shebang("/bin/bash"), None);
    assert_eq!(resolve_shebang(" #!/bin/bash"), None);
    // Only the text before the first newline counts.
    assert_eq!(
        resolve_shebang("#!/bin/bash\nprint(1)"),
        Some(Language::Shell)
    );
    assert_eq!(resolve_shebang("echo hi\n#!/bin/bash"), None);
    // Python wins over node over shell.
    assert_eq!(
        resolve_shebang("#!/bin/bash python node"),
        Some(Language::Python)
    );
    assert_eq!(
        resolve_shebang("#!/bin/bash node"),
        Some(Language::JavaScript)
    );
}

#[test]
fn detection_prefers_alias_over_path_over_shebang() {
    let shebang = Some("#!/usr/bin/env python3");
    assert_eq!(
        detect(Some("rust"), Some("a.go"), shebang),
        Some(Language::Rust)
    );
    assert_eq!(detect(None, Some("a.go"), shebang), Some(Language::Go));
    assert_eq!(detect(None, None, shebang), Some(Language::Python));
    // An unrecognized alias does not stop resolution.
    assert_eq!(
        detect(Some("klingon"), Some("a.go"), shebang),
        Some(Language::Go)
    );
    assert_eq!(
        detect(Some("klingon"), Some("a.txt"), shebang),
        Some(Language::Python)
    );
    assert_eq!(detect(Some("klingon"), Some("a.txt"), Some("hello")), None);
    assert_eq!(detect(None, None, None), None);
}

#[test]
fn every_family_highlights_a_sample_into_spans() {
    let samples: &[(Language, &str)] = &[
        (Language::Rust, "fn main() { let x = 1; }"),
        (Language::JavaScript, "function f(a) { return a + 1; }"),
        (Language::Jsx, "const a = <div className=\"x\">hi</div>;"),
        (Language::TypeScript, "let x: number = 1;"),
        (Language::Tsx, "const a = <div>hi</div>;"),
        (Language::Python, "def f(x):\n    return x + 1\n"),
        (Language::Go, "package main\nfunc main() { x := 1 }\n"),
        (Language::Json, "{\"a\": [1, true, null]}"),
        (Language::Jsonc, "{\"a\": 1} // note"),
        (Language::Shell, "if [ -f x ]; then echo \"$HOME\"; fi\n"),
        (Language::Toml, "[package]\nname = \"x\"\nversion = 1\n"),
        (Language::Markdown, "# Title\n\nsome `code` here\n"),
        (Language::Html, "<div class=\"a\">text</div>"),
        (Language::Css, "a { color: red; margin: 0 }"),
        (Language::Yaml, "key: value\nlist:\n  - 1\n"),
        (Language::C, "int main(void) { return 0; }"),
    ];
    assert_eq!(samples.len(), 16);
    for (language, source) in samples {
        let document = highlight_as(*language, source);
        assert!(
            document.span_count() > 0,
            "{} produced no span",
            language.name()
        );
        assert_eq!(document.language(), *language);
    }
}

#[test]
fn multiline_block_comment_colors_each_line_without_newlines() {
    for (language, source) in [
        (Language::Rust, "/* one\n two\n three */\nfn main() {}\n"),
        (Language::C, "/* one\n two\n three */\nint x;\n"),
        (Language::JavaScript, "/* one\n two\n three */\nlet x;\n"),
    ] {
        let document = highlight_as(language, source);
        let lines: Vec<&str> = source.lines().collect();
        for index in 0..3 {
            let spans = &document.lines()[index];
            let comment: Vec<_> = spans
                .iter()
                .filter(|span| span.kind == TokenKind::Comment)
                .collect();
            assert!(
                !comment.is_empty(),
                "{language:?} line {index} has no comment span"
            );
            for span in &comment {
                assert!(!lines[index][span.range.clone()].contains('\n'));
            }
        }
        // The code after the comment is not a comment: the state ended with the comment.
        assert!(
            document.lines()[3]
                .iter()
                .all(|span| span.kind != TokenKind::Comment),
            "{language:?}"
        );
    }
}

#[test]
fn jsx_and_tsx_tags_are_classified_as_tags() {
    let source = "const a = <div className=\"x\">hi</div>;";
    for language in [Language::Jsx, Language::Tsx] {
        let document = highlight_as(language, source);
        let tags: Vec<&str> = tokens(source, &document)
            .into_iter()
            .filter(|(_, kind)| *kind == TokenKind::Tag)
            .map(|(text, _)| text)
            .collect();
        assert_eq!(tags, vec!["div", "div"], "{language:?}");
    }
}

// ------------------------------------------------------------------ A2: token classification

#[test]
fn json_booleans_are_boolean_and_null_is_constant() {
    let source = "{\"a\": true, \"b\": false, \"c\": null}";
    let document = highlight_as(Language::Json, source);
    assert_eq!(kind_of(source, &document, "true"), Some(TokenKind::Boolean));
    assert_eq!(
        kind_of(source, &document, "false"),
        Some(TokenKind::Boolean)
    );
    assert_eq!(
        kind_of(source, &document, "null"),
        Some(TokenKind::Constant)
    );
    assert_eq!(
        kind_of(source, &document, "\"a\""),
        Some(TokenKind::Property)
    );
}

#[test]
fn rust_builtin_types_are_builtin_and_named_types_are_types() {
    let source = "let a: u32 = 1;\nlet b: bool = true;\nstruct String;\ntype Alias = u8;\n";
    let document = highlight_as(Language::Rust, source);
    assert_eq!(
        kind_of(source, &document, "u32"),
        Some(TokenKind::TypeBuiltin)
    );
    assert_eq!(
        kind_of(source, &document, "bool"),
        Some(TokenKind::TypeBuiltin)
    );
    assert_eq!(kind_of(source, &document, "String"), Some(TokenKind::Type));
    assert_eq!(kind_of(source, &document, "Alias"), Some(TokenKind::Type));
    assert_eq!(kind_of(source, &document, "let"), Some(TokenKind::Keyword));
    assert_eq!(
        kind_of(source, &document, "struct"),
        Some(TokenKind::Keyword)
    );
}

#[test]
fn c_builtin_types_are_types() {
    let source = "int main(void) { long n = 0; return 0; }";
    let document = highlight_as(Language::C, source);
    assert_eq!(kind_of(source, &document, "int"), Some(TokenKind::Type));
    assert_eq!(kind_of(source, &document, "void"), Some(TokenKind::Type));
    assert_eq!(
        kind_of(source, &document, "return"),
        Some(TokenKind::Keyword)
    );
}

#[test]
fn typescript_string_after_a_colon_is_a_builtin_type() {
    let source = "let x: string = \"a\";\nfunction f(a: number): void {}\n";
    let document = highlight_as(Language::TypeScript, source);
    assert_eq!(
        kind_of(source, &document, "string"),
        Some(TokenKind::TypeBuiltin)
    );
    assert_eq!(
        kind_of(source, &document, "number"),
        Some(TokenKind::TypeBuiltin)
    );
    assert_eq!(kind_of(source, &document, "\"a\""), Some(TokenKind::String));
}

#[test]
fn markdown_fenced_code_highlights_the_fenced_language() {
    let source = "# Title\n\n```rust\nlet x: u32 = 1;\n```\n";
    let document = highlight_as(Language::Markdown, source);
    assert_eq!(kind_of(source, &document, "let"), Some(TokenKind::Keyword));
    assert_eq!(kind_of(source, &document, "1"), Some(TokenKind::Number));
    assert_eq!(kind_of(source, &document, "Title"), Some(TokenKind::Label));
}

// ------------------------------------------------------------------ A3: span construction

fn single(
    kind: TokenKind,
) -> impl Fn(std::ops::Range<usize>) -> (std::ops::Range<usize>, TokenKind) {
    move |range| (range, kind)
}

#[test]
fn crlf_sources_produce_spans_that_exclude_the_carriage_return() {
    let source = "// a\r\nlet x = 1;\r\n";
    let document = highlight_as(Language::Rust, source);
    assert_eq!(document.line_count(), 2);
    let first = &document.lines()[0];
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, TokenKind::Comment);
    assert_eq!(first[0].range, 0..4);
    for spans in document.lines() {
        for span in spans {
            assert!(span.range.end <= 12);
        }
    }
    let last = document.lines()[1].last().expect("spans on line two");
    assert_eq!(last.range.end, "let x = 1;".len());

    // Absolute spans covering the terminators are cut at the line content.
    let document = HighlightedDocument::from_absolute_spans(
        Language::Rust,
        "ab\r\ncd\r\n",
        [(0..8, TokenKind::String)],
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        document.lines()[0],
        vec![Span {
            range: 0..2,
            kind: TokenKind::String
        }]
    );
    assert_eq!(
        document.lines()[1],
        vec![Span {
            range: 0..2,
            kind: TokenKind::String
        }]
    );
}

#[test]
fn empty_source_has_exactly_one_empty_line() {
    let document = highlight_as(Language::Rust, "");
    assert_eq!(document.line_count(), 1);
    assert!(document.lines()[0].is_empty());
    let document =
        HighlightedDocument::from_absolute_spans(Language::Json, "", [], Limits::default())
            .unwrap();
    assert_eq!(document.line_count(), 1);
}

#[test]
fn trailing_newline_creates_no_phantom_last_line() {
    assert_eq!(highlight_as(Language::Rust, "let a = 1;\n").line_count(), 1);
    assert_eq!(highlight_as(Language::Rust, "let a = 1;").line_count(), 1);
    assert_eq!(
        highlight_as(Language::Rust, "let a = 1;\n\n").line_count(),
        2
    );
    assert_eq!(highlight_as(Language::Rust, "\n").line_count(), 1);
    assert_eq!(highlight_as(Language::Rust, "a\nb\nc\n").line_count(), 3);
    assert_eq!(highlight_as(Language::Rust, "a\nb\nc").line_count(), 3);
}

#[test]
fn invalid_absolute_spans_reject_the_whole_result() {
    let source = "héllo\nworld";
    let build = |spans: Vec<(std::ops::Range<usize>, TokenKind)>| {
        HighlightedDocument::from_absolute_spans(Language::Rust, source, spans, Limits::default())
    };
    // Past the end.
    assert_eq!(
        build(vec![
            (0..1, TokenKind::Keyword),
            (3..99, TokenKind::Keyword)
        ]),
        Err(HighlightError::InvalidRange)
    );
    // Reversed.
    #[allow(clippy::reversed_empty_ranges)]
    let reversed = build(vec![(5..2, TokenKind::Keyword)]);
    assert_eq!(reversed, Err(HighlightError::InvalidRange));
    // Start past the end even though empty.
    assert_eq!(
        build(vec![(100..100, TokenKind::Keyword)]),
        Err(HighlightError::InvalidRange)
    );
    // Inside the two-byte "é" (bytes 1..3).
    assert_eq!(
        build(vec![(0..2, TokenKind::Keyword)]),
        Err(HighlightError::InvalidUtf8Boundary)
    );
    assert_eq!(
        build(vec![(2..4, TokenKind::Keyword)]),
        Err(HighlightError::InvalidUtf8Boundary)
    );
    // Valid ones are accepted; empty spans are dropped.
    let document = build(vec![(0..3, TokenKind::Keyword), (4..4, TokenKind::String)]).unwrap();
    assert_eq!(document.span_count(), 1);
}

#[test]
fn spans_crossing_newlines_split_per_line() {
    let source = "ab\ncd\nef";
    let document = HighlightedDocument::from_absolute_spans(
        Language::Rust,
        source,
        [(1..7, TokenKind::Comment)],
        Limits::default(),
    )
    .unwrap();
    assert_eq!(document.line_count(), 3);
    assert_eq!(
        document.lines()[0],
        vec![Span {
            range: 1..2,
            kind: TokenKind::Comment
        }]
    );
    assert_eq!(
        document.lines()[1],
        vec![Span {
            range: 0..2,
            kind: TokenKind::Comment
        }]
    );
    assert_eq!(
        document.lines()[2],
        vec![Span {
            range: 0..1,
            kind: TokenKind::Comment
        }]
    );
}

#[test]
fn normalization_resolves_overlaps_by_precedence_then_order_and_merges_touching_kinds() {
    let source = "0123456789";
    let build = |spans: Vec<(std::ops::Range<usize>, TokenKind)>| {
        HighlightedDocument::from_absolute_spans(Language::Rust, source, spans, Limits::default())
            .unwrap()
            .lines()[0]
            .iter()
            .map(|span| (span.range.clone(), span.kind))
            .collect::<Vec<_>>()
    };
    let keyword = single(TokenKind::Keyword);
    let string = single(TokenKind::String);
    let property = single(TokenKind::Property);
    let punctuation = single(TokenKind::Punctuation);
    // Equal precedence: the later span wins on the overlap.
    assert_eq!(
        build(vec![keyword(0..6), string(4..10)]),
        vec![(0..4, TokenKind::Keyword), (4..10, TokenKind::String)]
    );
    assert_eq!(
        build(vec![string(4..10), keyword(0..6)]),
        vec![(0..6, TokenKind::Keyword), (6..10, TokenKind::String)]
    );
    // Higher precedence wins regardless of order, and the covered span resumes after it.
    assert_eq!(
        build(vec![property(3..5), keyword(0..10)]),
        vec![
            (0..3, TokenKind::Keyword),
            (3..5, TokenKind::Property),
            (5..10, TokenKind::Keyword)
        ]
    );
    assert_eq!(
        build(vec![keyword(0..10), punctuation(3..5)]),
        vec![(0..10, TokenKind::Keyword)]
    );
    // Touching spans of equal kind merge; a gap keeps them apart; unsorted input is sorted.
    assert_eq!(
        build(vec![keyword(0..3), keyword(3..5)]),
        vec![(0..5, TokenKind::Keyword)]
    );
    assert_eq!(
        build(vec![keyword(6..8), keyword(0..3)]),
        vec![(0..3, TokenKind::Keyword), (6..8, TokenKind::Keyword)]
    );
}

// ------------------------------------------------------------------ A4: limits and failure

#[test]
fn source_limit_is_exact() {
    let line = format!("//{}\n", "a".repeat(61));
    assert_eq!(line.len(), 64);
    let exact = line.repeat(8192);
    assert_eq!(exact.len(), MAX_SOURCE_BYTES);
    let document = highlight_language(Language::Rust, &exact, Limits::default())
        .expect("exactly 524,288 bytes is allowed");
    assert_eq!(document.line_count(), 8192);
    assert!(document.lines().iter().all(|spans| spans.len() == 1));

    let mut over = exact;
    over.push('a');
    assert_eq!(
        highlight_language(Language::Rust, &over, Limits::default()),
        Err(HighlightError::SourceTooLarge)
    );
    let cache = HighlightCache::new();
    assert!(
        cache
            .highlight_as(Language::Rust, &over, Limits::default())
            .is_none()
    );
}

#[test]
fn span_limit_aborts_as_soon_as_it_is_crossed() {
    let exactly = (0..MAX_RAW_SPANS).map(|i| (i..i + 1, TokenKind::Number));
    let source = "x".repeat(MAX_RAW_SPANS + 1);
    assert!(
        HighlightedDocument::from_absolute_spans(
            Language::Rust,
            &source,
            exactly,
            Limits::default()
        )
        .is_ok()
    );
    let mut produced = 0usize;
    let over = (0..MAX_RAW_SPANS + 10).map(|i| {
        produced += 1;
        (i..i + 1, TokenKind::Number)
    });
    assert_eq!(
        HighlightedDocument::from_absolute_spans(Language::Rust, &source, over, Limits::default()),
        Err(HighlightError::TooManySpans)
    );
    assert_eq!(
        produced,
        MAX_RAW_SPANS + 1,
        "aborts right after crossing the limit"
    );

    // A real source that tokenizes into more than 200,000 regions.
    let dense = format!("[{}]", "1,".repeat(150_000));
    assert!(dense.len() <= MAX_SOURCE_BYTES);
    assert_eq!(
        highlight_language(Language::Json, &dense, Limits::default()),
        Err(HighlightError::TooManySpans)
    );
}

#[test]
fn failures_fall_back_to_plain_and_are_never_cached() {
    let cache = HighlightCache::new();
    let mut expected_misses = 0;

    // Unknown language.
    let request = HighlightRequest::new("hello").fence_label("klingon");
    assert!(cache.highlight(&request).is_none());
    expected_misses += 1;
    assert!(cache.highlight(&request).is_none());
    expected_misses += 1;

    // Over the source limit.
    let big = "a".repeat(MAX_SOURCE_BYTES + 1);
    for _ in 0..2 {
        assert!(
            cache
                .highlight_as(Language::Rust, &big, Limits::default())
                .is_none()
        );
        expected_misses += 1;
    }

    // Over the span limit.
    let dense = format!("[{}]", "1,".repeat(150_000));
    for _ in 0..2 {
        assert!(
            cache
                .highlight_as(Language::Json, &dense, Limits::default())
                .is_none()
        );
        expected_misses += 1;
    }

    // A tokenizer error.
    for _ in 0..2 {
        let out = cache.get_or_build(Language::Rust, "abc", Limits::default(), || {
            Err(HighlightError::Parser("boom".into()))
        });
        assert!(out.is_none());
        expected_misses += 1;
    }

    let stats = cache.stats();
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, expected_misses);
    assert_eq!(stats.documents, 0);
    assert_eq!(stats.retained_bytes, 0);
}

// ------------------------------------------------------------------ A5: cache

fn rust_source(n: usize) -> String {
    format!("fn f{n}() -> u32 {{ {n} }}\n")
}

#[test]
fn second_lookup_returns_the_same_shared_document() {
    let cache = HighlightCache::new();
    let source = rust_source(1);
    let first = cache
        .highlight_as(Language::Rust, &source, Limits::default())
        .unwrap();
    assert_eq!(cache.stats().hits, 0);
    assert_eq!(cache.stats().misses, 1);
    let second = cache
        .highlight_as(Language::Rust, &source, Limits::default())
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let stats = cache.stats();
    assert_eq!(
        (stats.hits, stats.misses, stats.parses, stats.documents),
        (1, 1, 1, 1)
    );
    assert_eq!(stats.retained_bytes, first.weight());
}

#[test]
fn different_sources_languages_and_generations_do_not_share_entries() {
    let cache = HighlightCache::new();
    let a = cache
        .highlight_as(Language::Rust, "let a = 1;", Limits::default())
        .unwrap();
    let b = cache
        .highlight_as(Language::Rust, "let a = 2;", Limits::default())
        .unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
    assert_eq!(cache.stats().misses, 2);

    // Same text, other language: a miss with its own entry.
    let c = cache
        .highlight_as(Language::C, "let a = 1;", Limits::default())
        .unwrap();
    assert!(!Arc::ptr_eq(&a, &c));
    assert_eq!(cache.stats().misses, 3);
    assert_eq!(cache.stats().documents, 3);

    // A generation bump can never be served from older entries.
    assert_eq!(cache.generation(), GRAMMAR_GENERATION);
    assert_eq!(GRAMMAR_GENERATION, 2);
    cache.set_generation(GRAMMAR_GENERATION + 1);
    let d = cache
        .highlight_as(Language::Rust, "let a = 1;", Limits::default())
        .unwrap();
    assert!(!Arc::ptr_eq(&a, &d));
    assert_eq!(cache.stats().misses, 4);
    assert_eq!(cache.stats().hits, 0);
}

#[test]
fn more_than_ninety_six_documents_leave_at_most_ninety_six_resident() {
    let cache = HighlightCache::new();
    for n in 0..116 {
        cache
            .highlight_as(Language::Rust, &rust_source(n), Limits::default())
            .unwrap();
    }
    let stats = cache.stats();
    assert_eq!(stats.documents, 96);
    assert_eq!(stats.misses, 116);
    // The oldest were evicted, the newest are resident.
    cache
        .highlight_as(Language::Rust, &rust_source(115), Limits::default())
        .unwrap();
    assert_eq!(cache.stats().hits, 1);
    cache
        .highlight_as(Language::Rust, &rust_source(0), Limits::default())
        .unwrap();
    assert_eq!(cache.stats().hits, 1);
}

#[test]
fn a_hit_marks_the_entry_most_recently_used() {
    let cache = HighlightCache::with_limits(CacheLimits {
        max_documents: 3,
        max_bytes: CACHE_MAX_BYTES,
    });
    for n in 0..3 {
        cache
            .highlight_as(Language::Rust, &rust_source(n), Limits::default())
            .unwrap();
    }
    // Touch 0, then insert 3: 1 (the least recently used) is evicted, 0 stays.
    cache
        .highlight_as(Language::Rust, &rust_source(0), Limits::default())
        .unwrap();
    cache
        .highlight_as(Language::Rust, &rust_source(3), Limits::default())
        .unwrap();
    let hits = cache.stats().hits;
    cache
        .highlight_as(Language::Rust, &rust_source(0), Limits::default())
        .unwrap();
    assert_eq!(cache.stats().hits, hits + 1);
    cache
        .highlight_as(Language::Rust, &rust_source(1), Limits::default())
        .unwrap();
    assert_eq!(cache.stats().hits, hits + 1);
}

/// A synthetic source of `bytes` bytes (distinct per `seed`) with one number span per 2 bytes.
fn dense_document(
    seed: usize,
    bytes: usize,
) -> (
    String,
    impl FnOnce() -> Result<HighlightedDocument, HighlightError>,
) {
    let mut source = format!("{seed:08}\n");
    while source.len() < bytes {
        source.push_str("1,\n");
    }
    let text = source.clone();
    (source, move || {
        let spans = (0..text.len())
            .filter(|i| text.as_bytes()[*i] == b'1')
            .map(|i| (i..i + 1, TokenKind::Number));
        HighlightedDocument::from_absolute_spans(Language::Json, &text, spans, Limits::default())
    })
}

#[test]
fn dense_documents_keep_retained_bytes_within_the_byte_bound() {
    let cache = HighlightCache::new();
    let mut total_weight = 0usize;
    for seed in 0..60 {
        let (source, build) = dense_document(seed, 400_000);
        let document = cache
            .get_or_build(Language::Json, &source, Limits::default(), build)
            .unwrap();
        total_weight += document.weight();
        assert!(cache.stats().retained_bytes <= CACHE_MAX_BYTES);
        assert!(cache.stats().documents <= CACHE_MAX_DOCUMENTS);
    }
    assert!(
        total_weight > CACHE_MAX_BYTES,
        "the stream must exceed the byte bound"
    );
    let stats = cache.stats();
    assert!(stats.retained_bytes <= CACHE_MAX_BYTES);
    assert!(
        stats.documents < 60,
        "byte bound evicted documents: {}",
        stats.documents
    );
}

#[test]
fn oversize_document_is_not_inserted_and_removes_the_previous_entry() {
    let cache = HighlightCache::new();
    let source = "\n".repeat(1_048_576);
    let huge_limits = Limits {
        max_source_bytes: usize::MAX,
        max_raw_spans: MAX_RAW_SPANS,
    };
    let huge = Arc::new(
        HighlightedDocument::from_absolute_spans(Language::Json, &source, [], huge_limits).unwrap(),
    );
    assert_eq!(huge.line_count(), 1_048_576);
    assert!(huge.weight() > CACHE_MAX_BYTES);

    // A small document under the same key is removed by the oversize replacement.
    let small = Arc::new(HighlightedDocument::unstyled(Language::Json, "x"));
    cache.insert(Language::Json, &source, small.clone());
    assert_eq!(cache.stats().documents, 1);
    assert_eq!(cache.stats().retained_bytes, small.weight());
    cache.insert(Language::Json, &source, huge);
    let stats = cache.stats();
    assert_eq!((stats.documents, stats.retained_bytes), (0, 0));

    // One line fewer than the first oversize weight (32 + 24 x 1,048,575) is still accepted.
    let at_bound_source = "\n".repeat(1_048_574);
    let at_bound = Arc::new(
        HighlightedDocument::from_absolute_spans(Language::Json, &at_bound_source, [], huge_limits)
            .unwrap(),
    );
    assert_eq!(at_bound.weight(), 32 + 24 * 1_048_574);
    assert!(at_bound.weight() <= CACHE_MAX_BYTES);
    cache.insert(Language::Json, &at_bound_source, at_bound.clone());
    assert_eq!(cache.stats().documents, 1);
    assert_eq!(cache.stats().retained_bytes, at_bound.weight());

    let over_source = "\n".repeat(1_048_575);
    let over = Arc::new(
        HighlightedDocument::from_absolute_spans(Language::Json, &over_source, [], huge_limits)
            .unwrap(),
    );
    assert!(over.weight() > CACHE_MAX_BYTES);
    cache.insert(Language::Json, &over_source, over);
    assert_eq!(
        cache.stats().documents,
        1,
        "the over-bound document was not inserted"
    );
}

#[test]
fn inserting_an_existing_key_replaces_the_entry_and_adjusts_the_total() {
    let cache = HighlightCache::new();
    let source = "ab\ncd\n";
    let first = Arc::new(HighlightedDocument::unstyled(Language::Rust, source));
    cache.insert(Language::Rust, source, first);
    assert_eq!(cache.stats().retained_bytes, 32 + 24 * 2);
    let second = Arc::new(
        HighlightedDocument::from_absolute_spans(
            Language::Rust,
            source,
            [(0..1, TokenKind::Keyword), (3..5, TokenKind::String)],
            Limits::default(),
        )
        .unwrap(),
    );
    cache.insert(Language::Rust, source, second.clone());
    let stats = cache.stats();
    assert_eq!(stats.documents, 1);
    assert_eq!(stats.retained_bytes, 32 + 24 * 2 + 24 * 2);
    let served = cache
        .highlight_as(Language::Rust, source, Limits::default())
        .unwrap();
    assert!(Arc::ptr_eq(&served, &second));
}

#[test]
fn weight_is_32_plus_24_per_line_plus_24_per_span() {
    let source = "ab\ncd\nef\n";
    let document = HighlightedDocument::from_absolute_spans(
        Language::Rust,
        source,
        [
            (0..1, TokenKind::Keyword),
            (1..2, TokenKind::String),
            (3..5, TokenKind::Number),
        ],
        Limits::default(),
    )
    .unwrap();
    assert_eq!((document.line_count(), document.span_count()), (3, 3));
    assert_eq!(document.weight(), 32 + 24 * 3 + 24 * 3);
    assert_eq!(
        HighlightedDocument::unstyled(Language::Rust, "").weight(),
        32 + 24
    );
    let cache = HighlightCache::new();
    let source = "let a = 1;\nlet b = 2;\n";
    let cached = cache
        .highlight_as(Language::Rust, source, Limits::default())
        .unwrap();
    assert_eq!(
        cache.stats().retained_bytes,
        32 + 24 * cached.line_count() + 24 * cached.span_count()
    );
}

// ------------------------------------------------------------------ A6: theme and widgets

#[test]
fn palette_change_recolors_with_zero_lookups_and_zero_parses() {
    let cache = HighlightCache::new();
    let source: Arc<str> = Arc::from("pub const A: u32 = 1; // note\nfn f() {}\n");
    let mut retained = RetainedHighlight::new();
    let hint = LanguageHint {
        fence_label: Some("rust"),
        path: None,
    };
    let document = retained.resolve(&cache, &source, hint).cloned().unwrap();
    let after_first = cache.stats();
    assert_eq!((after_first.misses, after_first.parses), (1, 1));

    let mut runs_with = |colors: &SyntaxColors| -> Vec<Hsla> {
        let view = CodeView::new(source.clone())
            .language("rust")
            .retained(&mut retained, &cache)
            .colors(*colors);
        view.lines()
            .iter()
            .flat_map(|line| line.runs().iter().map(|run| run.color))
            .collect()
    };
    let dark = SyntaxColors::dark();
    let other = dark
        .with_category("keyword", hex(0xFF6688))
        .with_category("comment", hex(0x99AA88));
    let a = runs_with(&dark);
    let b = runs_with(&other);
    assert_ne!(a, b, "the recolor changed some span colors");
    assert_eq!(a.len(), b.len(), "same runs, only colors differ");
    assert_eq!(cache.stats(), after_first, "no lookup, no parse, no hit");
    assert!(Arc::ptr_eq(retained.document().unwrap(), &document));
}

#[test]
fn retained_handle_re_resolves_only_when_source_hint_or_generation_changes() {
    let cache = HighlightCache::new();
    let mut retained = RetainedHighlight::new();
    let hint = LanguageHint {
        fence_label: Some("rust"),
        path: None,
    };
    let source: Arc<str> = Arc::from("fn a() {}");
    let first = retained.resolve(&cache, &source, hint).cloned().unwrap();
    let baseline = cache.stats();

    // Same Arc, an equal copy: neither touches the cache.
    for _ in 0..5 {
        retained.resolve(&cache, &source, hint);
    }
    let copy: Arc<str> = Arc::from("fn a() {}");
    let again = retained.resolve(&cache, &copy, hint).cloned().unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(cache.stats(), baseline);

    // New text: a new key and a new request.
    let edited: Arc<str> = Arc::from("fn b() {}");
    let second = retained.resolve(&cache, &edited, hint).cloned().unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(cache.stats().parses, baseline.parses + 1);

    // A changed fence label re-resolves the language and the key.
    let ts = LanguageHint {
        fence_label: Some("ts"),
        path: None,
    };
    let third = retained.resolve(&cache, &edited, ts).cloned().unwrap();
    assert_eq!(third.language(), Language::TypeScript);
    assert_eq!(cache.stats().parses, baseline.parses + 2);

    // A generation bump re-resolves.
    cache.set_generation(GRAMMAR_GENERATION + 1);
    retained.resolve(&cache, &edited, ts);
    assert_eq!(cache.stats().parses, baseline.parses + 3);
}

#[test]
fn code_view_and_editable_surface_share_kinds() {
    let cache = Arc::new(HighlightCache::new());
    let text = "pub struct P { x: u32 }\n// done\n";
    let hint = LanguageHint {
        fence_label: Some("rust"),
        path: None,
    };
    // The editable surface asks the adapter for the document of its current revision.
    let adapter: &dyn HighlighterAdapter = &*cache;
    let from_adapter = adapter.document_for(text, hint).unwrap();

    let colors = SyntaxColors::dark();
    let view = CodeView::new(text)
        .language("rust")
        .cache(cache.clone())
        .colors(colors);
    let lines = view.lines();
    let from_view = cache
        .highlight_as(Language::Rust, text, Limits::default())
        .unwrap();
    assert!(
        Arc::ptr_eq(&from_adapter, &from_view),
        "one shared document for both"
    );
    assert_eq!(cache.stats().parses, 1);

    // The view's run colors are exactly the palette colors of the document's kinds.
    for (index, spans) in from_adapter.lines().iter().enumerate() {
        let line = &lines[index];
        let mut offset = 0;
        for run in line.runs() {
            let kind = spans
                .iter()
                .find(|span| span.range.start <= offset && offset < span.range.end)
                .map(|span| span.kind);
            let expected = kind.map(|k| colors.color_for(k)).unwrap_or(colors.plain);
            assert_eq!(run.color, expected, "line {index} offset {offset}");
            offset += run.len;
        }
        assert_eq!(offset, line.text().len());
    }
}

#[test]
fn editing_changes_the_key_and_stale_spans_are_clipped() {
    let cache = HighlightCache::new();
    let old = "let value = 12345;";
    let new = "let v = 1;";
    let old_document = cache
        .highlight_as(Language::Rust, old, Limits::default())
        .unwrap();
    let new_document = cache
        .highlight_as(Language::Rust, new, Limits::default())
        .unwrap();
    assert!(!Arc::ptr_eq(&old_document, &new_document));
    assert_eq!(cache.stats().misses, 2);

    // Applying the previous revision's document to the new text clips instead of overflowing.
    let view = CodeView::new(new)
        .language("rust")
        .document(Some(old_document.clone()));
    for line in view.lines() {
        assert_eq!(
            line.runs().iter().map(|run| run.len).sum::<usize>(),
            line.text().len()
        );
    }
    // A document with fewer lines leaves the extra lines plain.
    let two_lines =
        CodeView::new("let a = 1;\nlet b = 2;\nlet c = 3;").document(Some(old_document));
    let lines = two_lines.lines();
    assert_eq!(lines.len(), 3);
    assert!(
        lines[2]
            .runs()
            .iter()
            .all(|run| run.color == SyntaxColors::dark().plain)
    );
}

#[test]
fn syntax_colors_map_every_kind_to_its_palette_category() {
    let pairs: &[(TokenKind, &str)] = &[
        (TokenKind::Comment, "comment"),
        (TokenKind::Keyword, "keyword"),
        (TokenKind::String, "string"),
        (TokenKind::StringSpecial, "stringSpecial"),
        (TokenKind::Escape, "escape"),
        (TokenKind::Number, "number"),
        (TokenKind::Boolean, "boolean"),
        (TokenKind::Type, "typeName"),
        (TokenKind::TypeBuiltin, "typeBuiltin"),
        (TokenKind::Constructor, "constructor"),
        (TokenKind::Function, "function"),
        (TokenKind::FunctionBuiltin, "functionBuiltin"),
        (TokenKind::Macro, "macroName"),
        (TokenKind::Property, "property"),
        (TokenKind::Constant, "constant"),
        (TokenKind::Variable, "variable"),
        (TokenKind::VariableSpecial, "variableSpecial"),
        (TokenKind::Parameter, "parameter"),
        (TokenKind::Operator, "operator"),
        (TokenKind::Punctuation, "punctuation"),
        (TokenKind::Embedded, "punctuation"),
        (TokenKind::Tag, "tag"),
        (TokenKind::Attribute, "attribute"),
        (TokenKind::Label, "label"),
        (TokenKind::Invalid, "invalid"),
    ];
    assert_eq!(pairs.len(), TokenKind::ALL.len());
    let colors = SyntaxColors::dark();
    for (kind, key) in pairs {
        assert_eq!(kind.category_key(), *key);
        assert_eq!(Some(colors.color_for(*kind)), colors.category(key));
    }
    assert_eq!(CATEGORY_KEYS.len(), 24);
    for key in CATEGORY_KEYS {
        assert!(colors.category(key).is_some(), "{key}");
    }
    assert_eq!(colors.category("nope"), None);

    // Dark defaults of THM-002.
    assert_eq!(colors.comment, hex(0x808080));
    assert_eq!(colors.keyword, hex(0x8F96ED));
    assert_eq!(colors.function, hex(0x8F96ED));
    assert_eq!(colors.string, hex(0x1EB787));
    assert_eq!(colors.type_builtin, hex(0x1EB787));
    assert_eq!(colors.constant, hex(0x1EB787));
    assert_eq!(colors.escape, hex(0xE679B4));
    assert_eq!(colors.tag, hex(0xE679B4));
    assert_eq!(colors.number, hex(0xDBA924));
    assert_eq!(colors.attribute, hex(0xDBA924));
    assert_eq!(colors.variable, hex(0xE5E5E5));
    assert_eq!(colors.punctuation, hex(0xE5E5E5));
    assert_eq!(colors.invalid, hex(0xE9797C));

    // Per-category override changes only that category.
    let changed = colors.with_category("typeName", hex(0x123456));
    assert_eq!(changed.color_for(TokenKind::Type), hex(0x123456));
    assert_eq!(
        changed.color_for(TokenKind::TypeBuiltin),
        colors.type_builtin
    );
    assert!(!SyntaxColors::dark().clone().set_category("nope", hex(0)));
}

// ------------------------------------------------------------------ A7: the visual fixture

fn colored_text(line: &str, runs: &[TextRun]) -> Vec<(String, Hsla)> {
    let mut offset = 0;
    runs.iter()
        .map(|run| {
            let text = line[offset..offset + run.len].to_owned();
            offset += run.len;
            (text, run.color)
        })
        .collect()
}

#[test]
fn json_fixture_line_gets_the_prescribed_colors_with_one_shared_font() {
    let source = r#"{"enabled":true,"count":2}"#;
    assert_eq!(source.len(), 26);
    let orange = hex(0xFFAA44);
    let white = hex(0xEEEEEE);
    let colors = SyntaxColors::dark()
        .with_plain(white)
        .with_category("string", hex(0x44CC88))
        .with_category("property", orange)
        .with_category("boolean", orange)
        .with_category("number", orange)
        .with_category("punctuation", white);
    let cache = Arc::new(HighlightCache::new());
    let view = CodeView::new(source)
        .language("json")
        .cache(cache)
        .colors(colors)
        .font_family("Liberation Mono")
        .font_size(vespertine::px(14.))
        .line_height(vespertine::px(22.));
    let lines = view.lines();
    assert_eq!(lines.len(), 1);
    let runs = lines[0].runs();
    let expected = [
        ("{", white),
        ("\"enabled\"", orange),
        (":", white),
        ("true", orange),
        (",", white),
        ("\"count\"", orange),
        (":", white),
        ("2", orange),
        ("}", white),
    ];
    let actual = colored_text(source, runs);
    assert_eq!(actual.len(), expected.len());
    for ((text, color), (expected_text, expected_color)) in actual.iter().zip(expected) {
        assert_eq!(text, expected_text);
        assert_eq!(*color, expected_color, "color of {text}");
        assert_ne!(*color, hex(0x44CC88));
    }
    let shared = font("Liberation Mono");
    assert!(
        runs.iter().all(|run| run.font == shared),
        "one font for every run"
    );
}

// ------------------------------------------------------------------ A8: cache stress

#[test]
fn cache_stress_benchmark_keeps_both_bounds_and_reports_numbers() {
    let cache = HighlightCache::new();
    let start = std::time::Instant::now();

    // Phase 1: 200 distinct ~2 KB documents.
    for seed in 0..200usize {
        let mut source = String::new();
        let mut line = 0;
        while source.len() < 2_000 {
            source.push_str(&format!(
                "pub const ITEM_{seed}_{line}: u32 = {}; // note {line}\n",
                seed * line
            ));
            line += 1;
        }
        cache
            .highlight_as(Language::Rust, &source, Limits::default())
            .unwrap();
    }
    let phase_one = cache.stats();
    println!(
        "cache-stress phase 1 (200 x ~2 KB): documents={} retained_bytes={} parses={} ({:?})",
        phase_one.documents,
        phase_one.retained_bytes,
        phase_one.parses,
        start.elapsed()
    );
    assert_eq!(
        phase_one.documents, CACHE_MAX_DOCUMENTS,
        "the count limit is reached"
    );
    assert!(phase_one.retained_bytes <= CACHE_MAX_BYTES);

    // Phase 2: ~20 dense documents of ~400 KB each.
    let mut total_weight = 0;
    for seed in 0..20usize {
        let (source, build) = dense_document(seed + 1_000, 400_000);
        let document = cache
            .get_or_build(Language::Json, &source, Limits::default(), build)
            .unwrap();
        total_weight += document.weight();
    }
    let phase_two = cache.stats();
    println!(
        "cache-stress phase 2 (20 x ~400 KB, total weight {total_weight}): documents={} retained_bytes={} parses={}",
        phase_two.documents, phase_two.retained_bytes, phase_two.parses
    );
    assert!(phase_two.documents <= CACHE_MAX_DOCUMENTS);
    assert!(
        phase_two.retained_bytes <= CACHE_MAX_BYTES,
        "the byte limit holds"
    );
    assert!(
        total_weight > CACHE_MAX_BYTES,
        "the byte limit is what bounded phase 2"
    );
    assert!(phase_two.documents < phase_one.documents);
}
