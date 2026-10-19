//! Public-surface tests of the Markdown layer (CMP-009 acceptance A3, A5, A6 and R2, R4, R14).

use super::*;

fn face_texts(source: &str) -> Vec<(&str, Face)> {
    faces(source)
        .into_iter()
        .map(|(range, face)| (&source[range], face))
        .collect()
}

/// The display text of `source` with the lines touched by `selection` active.
fn render(source: &str, selection: Range<usize>) -> String {
    let active = active_range(source, selection, None);
    let mut text = String::new();
    let mut at = 0;
    for (range, display) in decorations(source, active) {
        text.push_str(&source[at..range.start]);
        text.push_str(&display);
        at = range.end;
    }
    text.push_str(&source[at..]);
    text
}

#[test]
fn faces_cover_the_full_parser_range() {
    assert_eq!(
        face_texts("**b** _i_ ~~s~~ `c`"),
        [
            ("**b**", Face::Bold),
            ("_i_", Face::Italic),
            ("~~s~~", Face::Strikethrough),
            ("`c`", Face::Code),
        ]
    );
}

#[test]
fn nested_faces_combine() {
    assert_eq!(
        face_texts("***both***"),
        [("***both***", Face::Italic), ("**both**", Face::Bold)]
    );
    let source = "**bold _nested_**";
    assert_eq!(
        face_texts(source),
        [(source, Face::Bold), ("_nested_", Face::Italic)]
    );
}

#[test]
fn headings_and_code_blocks_have_faces() {
    assert_eq!(face_texts("# Title"), [("# Title", Face::Bold)]);
    assert_eq!(face_texts("Title\n===="), [("Title\n====", Face::Bold)]);
    assert_eq!(
        face_texts("```\ncode\n```"),
        [("```\ncode\n```", Face::Code)]
    );
    assert_eq!(face_texts("    code"), [("code", Face::Code)]);
    // Text in code never takes bold or italic faces.
    assert_eq!(
        face_texts("```\n**x** _y_\n```"),
        [("```\n**x** _y_\n```", Face::Code)]
    );
    assert_eq!(face_texts("`**x**`"), [("`**x**`", Face::Code)]);
}

#[test]
fn literal_markers_get_no_face() {
    assert!(faces("\\~\\~literal\\~\\~").is_empty());
    assert!(faces("####### seven").is_empty());
    assert!(faces("~~ not closed").is_empty());
    assert!(faces("`~~x~~`").iter().all(|(_, face)| *face == Face::Code));
    assert!(
        faces("```\n~~x~~\n```")
            .iter()
            .all(|(_, face)| *face == Face::Code)
    );
}

#[test]
fn faces_stay_when_the_line_is_active() {
    let source = "**b**";
    assert_eq!(faces(source), [(0..5, Face::Bold)]);
}

#[test]
fn a3_rendered_texts() {
    assert_eq!(
        render("***both*** and **bold _nested_**\n", 33..33),
        "both and bold nested\n"
    );
    assert_eq!(
        render(
            "- [x] done\n- [ ] pending\n- [x]word\n1. [X] ordered\n",
            50..50
        ),
        "\u{2611} done\n\u{2610} pending\n\u{2022} [x]word\n1. \u{2611} ordered\n"
    );
    assert_eq!(render("- [x] done", 0..0), "\u{2022} [x] done");
    for (source, inactive) in [
        ("# Caf\u{e9}", "Caf\u{e9}"),
        ("  ### Caf\u{e9} ###", "Caf\u{e9}"),
        ("###### Caf\u{e9}\t##", "Caf\u{e9}"),
        ("> ## Caf\u{e9} ##", "> Caf\u{e9}"),
    ] {
        let extended = format!("{source}\n");
        assert_eq!(
            render(&extended, extended.len()..extended.len()),
            format!("{inactive}\n")
        );
        assert_eq!(render(source, 0..0), source);
    }
    for source in ["Caf\u{e9}\n====\n", "Title\n-\n"] {
        assert_eq!(render(source, source.len()..source.len()), source);
    }
    assert_eq!(render("```inline```\nx", 13..13), "inline\nx");
}

#[test]
fn a2_styling_never_changes_the_source() {
    let source = "**bold** and ~~gone~~ and `code`\nnext";
    let hidden = render(source, source.len()..source.len());
    assert_eq!(hidden, "bold and gone and code\nnext");
    // Copying works on source: the decorations are only a description.
    assert_eq!(source, "**bold** and ~~gone~~ and `code`\nnext");
    assert_eq!(render(source, 4..4), source);
}

#[test]
fn a6_sources_over_the_limit_get_no_styling() {
    let mut source = "- **x** `c` # h\n".repeat(8_193);
    source.truncate(DEFAULT_PROCESSING_LIMIT + 1);
    assert_eq!(source.len(), 131_073);
    assert!(faces(&source).is_empty());
    assert!(decorations(&source, 0..0).is_empty());
    assert!(syntax_spans(&format!("```rust\nfn main() {{}}\n```\n{source}")).is_empty());
    // Exactly at the limit the layer still works.
    source.truncate(DEFAULT_PROCESSING_LIMIT);
    assert!(!faces(&source).is_empty());
    assert!(!decorations(&source, 0..0).is_empty());
    // Enter continuation is available regardless of the limit.
    let mut long = "- item\n".repeat(20_000);
    long.push_str("- last");
    assert!(newline_edit(&long, long.len()).is_some());
    assert!(!in_code(&long, long.len()));
}

#[test]
fn a6_big_documents() {
    let source = "- **\u{e9}**\n".repeat(10_000);
    assert!(source.len() < DEFAULT_PROCESSING_LIMIT);
    let replacements = decorations(&source, source.len()..source.len());
    assert_eq!(replacements.len(), 30_000);
    assert!(
        replacements
            .windows(2)
            .all(|pair| pair[0].0.end <= pair[1].0.start)
    );
    assert!(replacements.iter().all(|(range, _)| {
        source.is_char_boundary(range.start) && source.is_char_boundary(range.end)
    }));
    let over = "- **\u{e9}**\n".repeat(20_000);
    assert!(over.len() > DEFAULT_PROCESSING_LIMIT);
    assert!(decorations(&over, over.len()..over.len()).is_empty());
    assert!(faces(&over).is_empty());
}

#[test]
fn a_configured_limit_applies() {
    let options = MarkdownOptions {
        processing_limit: 4,
        list_indent: false,
    };
    assert!(options.faces("**ab**").is_empty());
    assert!(options.decorations("**ab**", 0..0).is_empty());
    assert_eq!(options.faces("*a*").len(), 1);
    assert_eq!(MarkdownOptions::default().processing_limit, 131_072);
    assert!(MarkdownOptions::default().list_indent);
    assert_eq!(options.indent_edit("- a", 0..0, false, false), None);
    assert!(
        MarkdownOptions::default()
            .indent_edit("- a", 0..0, false, false)
            .is_some()
    );
}

#[test]
fn active_range_is_the_union_of_touched_lines() {
    let source = "aaa\nbbb\nccc\nddd";
    assert_eq!(active_range(source, 5..5, None), 4..7);
    assert_eq!(active_range(source, 1..9, None), 0..11);
    assert_eq!(active_range(source, 0..4, None), 0..3);
    assert_eq!(active_range(source, 5..5, Some(13..14)), 4..15);
    assert_eq!(active_range(source, 5..5, Some(0..1)), 0..7);
    assert_eq!(active_range(source, 15..15, None), 12..15);
    assert_eq!(active_range("a\r\nb", 0..0, None), 0..1);
    assert_eq!(active_range("", 0..0, None), 0..0);
    assert_eq!(active_range("ab", 99..99, None), 0..2);
}

#[test]
fn syntax_spans_through_the_public_function() {
    let source = "# Heading\n\n```rust\nfn main() {}\n```\n\n`fn` prose\n";
    let spans = syntax_spans(source);
    assert!(!spans.is_empty());
    let fn_at = source.find("fn main").expect("fn");
    assert!(spans.contains(&HighlightSpan {
        range: fn_at..fn_at + 2,
        kind: TokenKind::Keyword
    }));
    assert!(spans.iter().all(|span| span.range.start >= fn_at
        && span.range.end < source.find("```\n\n`fn`").expect("end")));
}

#[test]
fn arbitrary_input_never_panics_and_keeps_invariants() {
    const ALPHABET: &[&str] = &[
        "-", "*", "+", "1", ".", ")", ">", "`", "~", "#", "_", " ", "\t", "\r\n", "\n", "[", "]",
        "x", "X", "a", "\\", "\u{e9}", "9", "=", "```", "**",
    ];
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as usize
    };
    for _ in 0..400 {
        let mut source = String::new();
        for _ in 0..next(40) {
            source.push_str(ALPHABET[next(ALPHABET.len())]);
        }
        let boundary = |source: &str, mut at: usize| {
            at = at.min(source.len());
            while !source.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        for (range, _) in faces(&source) {
            assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
        }
        let caret = boundary(&source, next(source.len() + 1));
        let active = active_range(&source, caret..caret, None);
        let replacements = decorations(&source, active);
        let mut at = 0;
        for (range, _) in &replacements {
            assert!(range.start >= at && range.start < range.end, "{source:?}");
            assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
            at = range.end;
        }
        for span in syntax_spans(&source) {
            assert!(source.is_char_boundary(span.range.start));
            assert!(source.is_char_boundary(span.range.end));
        }
        for offset in 0..=source.len() {
            let code = in_code(&source, offset);
            if let Some((range, text)) = newline_edit(&source, offset) {
                assert!(!code && source.is_char_boundary(range.start));
                assert!(range.end <= source.len() && text.len() < 64 + source.len());
            }
        }
        let start = boundary(&source, next(source.len() + 1));
        let end = boundary(&source, start + next(source.len() + 1));
        for outdent in [false, true] {
            if let Some(edit) = indent_edit(&source, start..end, false, outdent) {
                let text = edit.apply(&source);
                assert!(edit.selection.end <= text.len());
                assert!(text.is_char_boundary(edit.selection.start));
                assert!(text.is_char_boundary(edit.selection.end));
            }
        }
        for line in list_lines(&source) {
            assert!(source.is_char_boundary(line.line_start));
        }
    }
}
