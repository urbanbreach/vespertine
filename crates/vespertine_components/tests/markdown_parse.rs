//! CMP-011 parser: block tree, formulas, bare URLs, incremental equivalence, display repair.

use vespertine_components::markdown_doc::{
    Align, Block, BlockKind, IncrementalParser, PENDING_LINK, Run, parse, source_line,
};

fn runs_text(runs: &[Run]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

fn first_paragraph(source: &str) -> Vec<Run> {
    match parse(source).remove(0).kind {
        BlockKind::Paragraph(runs) => runs,
        other => panic!("not a paragraph: {other:?}"),
    }
}

#[test]
fn blocks_carry_byte_ranges_and_map_to_source_lines() {
    let source = "# Title\n\nfirst\nsecond\n\n- a\n- b\n\n```rust\nlet x;\n```\n";
    let blocks = parse(source);
    assert_eq!(blocks.len(), 4);
    let lines: Vec<usize> = blocks
        .iter()
        .map(|block| source_line(source, block.range.start))
        .collect();
    assert_eq!(lines, [1, 3, 6, 9]);
    match &blocks[3].kind {
        BlockKind::Code { info, fenced, code } => {
            assert_eq!(info, "rust");
            assert!(*fenced);
            assert_eq!(code, "let x;");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn soft_breaks_are_spaces_and_hard_breaks_newlines() {
    let runs = first_paragraph("a\nb  \nc\\\nd");
    assert_eq!(runs_text(&runs), "a b\nc\nd");
}

#[test]
fn inline_styles_compose() {
    let runs = first_paragraph("**bold *both* x** ~~gone~~ `code` [link](u)");
    let both = runs.iter().find(|r| r.text == "both").unwrap();
    assert!(both.style.bold && both.style.italic);
    assert!(runs.iter().any(|r| r.text == "gone" && r.style.strike));
    assert!(runs.iter().any(|r| r.text == "code" && r.style.code));
    assert!(
        runs.iter()
            .any(|r| r.text == "link" && r.link.as_deref() == Some("u"))
    );
}

#[test]
fn nested_lists_keep_start_numbers_and_task_markers() {
    let source = "3. one\n4. two\n   - [x] done\n   - [ ] todo\n";
    let blocks = parse(source);
    let BlockKind::List {
        ordered,
        start,
        items,
    } = &blocks[0].kind
    else {
        panic!()
    };
    assert!(*ordered);
    assert_eq!(*start, 3);
    assert_eq!(items.len(), 2);
    let BlockKind::List {
        ordered,
        items: inner,
        ..
    } = &items[1].blocks[1].kind
    else {
        panic!("{:?}", items[1].blocks)
    };
    assert!(!*ordered);
    let marker = |item: usize| match &inner[item].blocks[0].kind {
        BlockKind::Paragraph(runs) => runs[0].clone(),
        other => panic!("{other:?}"),
    };
    let done = marker(0);
    let task = done.task.clone().unwrap();
    assert!(task.checked);
    assert_eq!(&source[task.range], "[x]");
    assert_eq!(done.text, "[x] ");
    let todo = marker(1).task.unwrap();
    assert!(!todo.checked);
    assert_eq!(&source[todo.range], "[ ]");
}

#[test]
fn loose_task_items_keep_their_markers() {
    let source = "- [x] done\n\n- [ ] todo\n";
    let blocks = parse(source);
    let BlockKind::List { items, .. } = &blocks[0].kind else {
        panic!("{blocks:?}")
    };
    let first_run = |item: usize| match &items[item].blocks[0].kind {
        BlockKind::Paragraph(runs) => runs[0].clone(),
        other => panic!("{other:?}"),
    };
    let done = first_run(0);
    assert_eq!(done.text, "[x] ");
    let task = done.task.unwrap();
    assert!(task.checked);
    assert_eq!(&source[task.range], "[x]");
    let todo = first_run(1);
    assert_eq!(todo.text, "[ ] ");
    let task = todo.task.unwrap();
    assert!(!task.checked);
    assert_eq!(&source[task.range], "[ ]");
}

#[test]
fn tables_keep_alignment_and_missing_cells() {
    let source = "| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 |\n";
    let BlockKind::Table {
        aligns,
        header,
        rows,
    } = &parse(source)[0].kind
    else {
        panic!()
    };
    assert_eq!(aligns, &[Align::Left, Align::Center, Align::Right]);
    assert_eq!(header.len(), 3);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].len() <= 3);
}

#[test]
fn html_is_literal_text() {
    let blocks = parse("<div>hello</div>\n\nuse <b>x</b> here\n");
    let BlockKind::Paragraph(runs) = &blocks[0].kind else {
        panic!("{:?}", blocks[0])
    };
    assert_eq!(runs_text(runs), "<div>hello</div>");
    let BlockKind::Paragraph(runs) = &blocks[1].kind else {
        panic!()
    };
    assert_eq!(runs_text(runs), "use <b>x</b> here");
}

#[test]
fn nesting_deeper_than_32_collapses_to_text() {
    let source = format!("{}deep\n", "> ".repeat(40));
    let mut depth = 0;
    let mut current = parse(&source)[0].clone();
    while let BlockKind::Quote(children) = &current.kind {
        depth += 1;
        current = children[0].clone();
    }
    let block = &current;
    assert!(depth <= 33, "depth {depth}");
    let BlockKind::Paragraph(runs) = &block.kind else {
        panic!("{block:?}")
    };
    assert!(runs_text(runs).contains("deep"));
}

#[test]
fn bare_urls_become_links_outside_links_and_code() {
    let runs =
        first_paragraph("see https://a.com/x. and `https://code.io` and [t](https://l.io) end");
    let links: Vec<_> = runs.iter().filter_map(|r| r.link.clone()).collect();
    assert_eq!(links, ["https://a.com/x", "https://l.io"]);
    assert!(runs.iter().any(|r| r.style.code && r.link.is_none()));
    // Idempotent: parsing the text of the result again gives the same links.
    let again = first_paragraph(&runs_text(&runs));
    let links_again: Vec<_> = again.iter().filter_map(|r| r.link.clone()).collect();
    assert_eq!(links_again.len(), 3.min(links_again.len()));
}

#[test]
fn formula_delimiters_decide_math_and_prose() {
    let formulas = |source: &str| -> Vec<(String, bool)> {
        parse(source)
            .iter()
            .flat_map(|block| match &block.kind {
                BlockKind::Paragraph(runs) => runs
                    .iter()
                    .filter_map(|r| r.formula.clone())
                    .map(|f| (f.tex, f.display))
                    .collect::<Vec<_>>(),
                BlockKind::DisplayMath(f) => vec![(f.tex.clone(), f.display)],
                _ => vec![],
            })
            .collect()
    };
    assert!(formulas("It costs $5 and $10").is_empty());
    assert_eq!(
        formulas("so $x^*$ and $y^*$ hold"),
        vec![("x^*".to_owned(), false), ("y^*".to_owned(), false)]
    );
    assert_eq!(formulas(r"\(x\) here"), vec![("x".to_owned(), false)]);
    assert!(formulas(r"array \[i\]").is_empty());
    assert!(formulas(r"f\(x\)").is_empty());
    // The canonical formula run text is the dollar form.
    let runs = first_paragraph(r"a \(x\) b");
    assert!(runs.iter().any(|r| r.text == "$x$" && r.formula.is_some()));
}

#[test]
fn display_formulas_split_paragraphs_and_share_the_range() {
    let source = "before $$x+y$$ after\n";
    let blocks = parse(source);
    assert_eq!(blocks.len(), 3, "{blocks:?}");
    assert!(matches!(blocks[1].kind, BlockKind::DisplayMath(_)));
    assert!(blocks.iter().all(|b| b.range == blocks[0].range));
    // Alone in its paragraph: a block of its own.
    let blocks = parse("$$\nx\n$$\n");
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].kind, BlockKind::DisplayMath(_)));
    // Nested in emphasis: inline.
    let runs = first_paragraph("**a $$x$$ b**");
    assert!(
        runs.iter()
            .any(|r| r.formula.as_ref().is_some_and(|f| !f.display))
    );
}

#[test]
fn link_destinations_with_spaces_are_accepted() {
    let runs = first_paragraph("[doc](/home/me/My Docs/a.md) tail");
    let link = runs.iter().find(|r| r.link.is_some()).expect("a link");
    assert_eq!(link.text, "doc");
    assert_eq!(link.link.as_deref(), Some("/home/me/My Docs/a.md"));
}

const CORPUS: &str = "# Title\n\nA paragraph with **strong *nested* text**, `code`, a [link](https://x.y/z) and $a^2$.\nSecond line.\n\n> quote\n> continues\n\n1. one\n2. two\n   - [ ] task\n   - [x] done\n3. three\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n$$\nx^2\n$$\n\n---\n\nSetext\n======\n\n    indented code\n\nlast para https://example.com/a_b.\n";

#[test]
fn streaming_byte_by_byte_equals_a_full_parse() {
    let mut parser = IncrementalParser::new();
    let mut fed = String::new();
    for ch in CORPUS.chars() {
        let mut buf = [0u8; 4];
        let text = ch.encode_utf8(&mut buf);
        parser.append(text);
        fed.push(ch);
        assert_eq!(
            parser.source(),
            fed,
            "the stored source is exactly the appended text"
        );
        let full = parse(&fed);
        if parser.tree() != full.as_slice() {
            let at = parser
                .tree()
                .iter()
                .zip(full.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            panic!(
                "tree after {} bytes ({:?}) differs at block {at}:\nincremental {:#?}\nfull {:#?}",
                fed.len(),
                &fed[fed.len().saturating_sub(30)..],
                parser.tree().get(at),
                full.get(at)
            );
        }
    }
}

#[test]
fn only_the_last_block_is_reparsed() {
    let mut parser = IncrementalParser::new();
    parser.append("# Title\n\nfirst paragraph\n\nsecond paragraph");
    let blocks_before: Vec<Block> = parser.tree().to_vec();
    assert_eq!(blocks_before.len(), 3);
    parser.append(" grows");
    assert_eq!(parser.stable_prefix_blocks(), 2);
    assert_eq!(parser.tree()[..2], blocks_before[..2]);
    let last_start = parser.tree()[2].range.start;
    assert_eq!(
        parser.last_parse_bytes(),
        parser.source().len() - last_start
    );
    // A new block starts at the end of the previous one.
    parser.append("\n\nthird");
    assert_eq!(
        parser.stable_prefix_blocks(),
        2,
        "the old last block is reparsed"
    );
    parser.append(" more");
    assert_eq!(parser.stable_prefix_blocks(), 3);
}

#[test]
fn display_math_split_siblings_are_reparsed_together() {
    let mut parser = IncrementalParser::new();
    parser.append("intro\n\nbefore $$x$$ af");
    let pieces = parser.tree().len();
    assert_eq!(pieces, 4);
    parser.append("ter");
    assert_eq!(
        parser.stable_prefix_blocks(),
        1,
        "the paragraph and both split pieces"
    );
    assert_eq!(parser.tree().len(), pieces);
}

#[test]
fn reference_definitions_force_full_reparses() {
    let mut parser = IncrementalParser::new();
    parser.append("first [x] link\n\nsecond\n\n");
    assert!(!parser.forces_full_parse());
    parser.append("[x]: https://example.com\n");
    assert!(parser.forces_full_parse());
    assert_eq!(parser.stable_prefix_blocks(), 0);
    assert_eq!(parser.tree(), parse(parser.source()).as_slice());
    let runs = match &parser.tree()[0].kind {
        BlockKind::Paragraph(runs) => runs.clone(),
        other => panic!("{other:?}"),
    };
    assert!(
        runs.iter()
            .any(|r| r.link.as_deref() == Some("https://example.com"))
    );
    parser.append("more\n");
    assert_eq!(parser.last_parse_bytes(), parser.source().len());
}

#[test]
fn mended_display_trees_never_alter_the_canonical_tree_or_source() {
    let mut parser = IncrementalParser::new();
    parser.append("intro\n\nsome **strong and [a link](http://exa");
    assert_eq!(
        parser.source(),
        "intro\n\nsome **strong and [a link](http://exa"
    );
    assert_eq!(parser.tree(), parse(parser.source()).as_slice());
    let display = parser.display_tree().to_vec();
    assert_eq!(display.len(), parser.tree().len());
    let BlockKind::Paragraph(runs) = &display[1].kind else {
        panic!("{:?}", display[1])
    };
    let strong = runs.iter().find(|r| r.style.bold).expect("repaired strong");
    assert!(strong.text.contains("strong and"));
    let link = runs.iter().find(|r| r.link.is_some()).unwrap();
    assert_eq!(link.link.as_deref(), Some(PENDING_LINK));
    // The canonical tree has no pending link and no strong run.
    let BlockKind::Paragraph(canonical) = &parser.tree()[1].kind else {
        panic!()
    };
    assert!(
        canonical
            .iter()
            .all(|r| r.link.as_deref() != Some(PENDING_LINK) && !r.style.bold)
    );
    // Completing the text settles to the full parse.
    parser.append(".com)** done");
    assert_eq!(parser.tree(), parse(parser.source()).as_slice());
    assert_eq!(parser.display_tree().to_vec(), parser.tree().to_vec());
}

#[test]
fn a_streaming_code_fence_is_not_repaired() {
    let mut parser = IncrementalParser::new();
    parser.append("```rust\nlet a = *b");
    let display = parser.display_tree().to_vec();
    assert_eq!(display.as_slice(), parser.tree());
    let BlockKind::Code { code, .. } = &display[0].kind else {
        panic!()
    };
    assert_eq!(code, "let a = *b");
}

#[test]
fn replace_resets_the_stored_source() {
    let mut parser = IncrementalParser::new();
    parser.append("abc\n\ndef");
    parser.replace("# Other\n");
    assert_eq!(parser.source(), "# Other\n");
    assert_eq!(parser.tree().len(), 1);
    parser.reset();
    assert!(parser.tree().is_empty());
}

const CORPORA: &[&str] = &[
    "- item one\n- item two\n  continued\n\n  second para\n- three\n\n1. a\n1. b\n\ntext right after\n",
    "> quote **bold\n> more** text\n>\n> - list in quote\n> - another\n\nafter quote\n",
    "Setext one\n==========\n\nSetext two\n----------\n\n***\n\n* * *\n\n<div>\nraw html\n</div>\n\nend\n",
    "    code one\n    code two\n\n~~~\nfence\n~~~\n\ntext\twith tab\n\n    code three\n",
    "price $5 and $10, then $x^*$ and $$a+b$$ and \\(y\\) plus \\[ \\frac{1}{2} \\] and `$no$` here.\n\nsecond $$\nmulti\nline\n$$ para\n",
    "[ref]: https://ref.example/x\n\nuse [ref] and [other][ref] and ![img](pic.png \"t\")\n\n[doc](/a b/c d.md)\n",
    "| h1 | h2 |\n|----|----|\n| a | **b** |\n| c |\n\nafter table\n| not a row\n",
    "1. one\n   1. nested one\n   2. nested two\n      - deep\n2. two\n\n- [x] done\n- [ ] todo with `code`\n",
    "Hard  \nbreak and\\\nbackslash break and https://example.com/a(b)c) tail.\n\n## Heading with *emph*\n###### six\n####### seven\n",
];

#[test]
fn streaming_more_documents_byte_by_byte_equals_a_full_parse() {
    for corpus in CORPORA {
        let mut parser = IncrementalParser::new();
        let mut fed = String::new();
        for ch in corpus.chars() {
            let mut buf = [0u8; 4];
            parser.append(ch.encode_utf8(&mut buf));
            fed.push(ch);
            let full = parse(&fed);
            if parser.tree() != full.as_slice() {
                let at = parser
                    .tree()
                    .iter()
                    .zip(full.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(parser.tree().len().min(full.len()));
                panic!(
                    "after {:?}: block {at}\nincremental {:?}\nfull {:?}",
                    &fed[fed.len().saturating_sub(24)..],
                    parser.tree().get(at),
                    full.get(at)
                );
            }
        }
    }
}

/// A deterministic pseudo-random source of Markdown-significant characters.
fn soup(seed: u64, len: usize) -> String {
    const ALPHABET: &[&str] = &[
        "*",
        "_",
        "`",
        "$",
        "$$",
        "[",
        "]",
        "(",
        ")",
        "<",
        ">",
        "\\",
        "\n",
        "\n\n",
        " ",
        "  ",
        "#",
        "- ",
        "1. ",
        "> ",
        "|",
        "---",
        "~~",
        "!",
        ":",
        "a",
        "b",
        "word ",
        "http://x.y/z",
        "x",
        "9",
        "\t",
        "    ",
        "```",
        "[x] ",
        "&amp;",
        "é",
        "\\(",
        "\\)",
        "\\[",
        "\\]",
    ];
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = String::new();
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push_str(ALPHABET[(state % ALPHABET.len() as u64) as usize]);
    }
    out
}

#[test]
fn random_markdown_soup_streams_to_the_same_tree_as_a_full_parse_and_never_panics() {
    for seed in 1..=300u64 {
        let corpus = soup(seed, 60);
        let mut parser = IncrementalParser::new();
        let mut fed = String::new();
        for ch in corpus.chars() {
            let mut buf = [0u8; 4];
            parser.append(ch.encode_utf8(&mut buf));
            fed.push(ch);
            let full = parse(&fed);
            if parser.tree() != full.as_slice() {
                let at = parser
                    .tree()
                    .iter()
                    .zip(full.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(parser.tree().len().min(full.len()));
                panic!(
                    "seed {seed}: after {fed:?}: block {at}\nincremental {:?}\nfull {:?}",
                    parser.tree().get(at),
                    full.get(at)
                );
            }
            // The display tree is built for every state too.
            let _ = parser.display_tree().len();
        }
    }
}
