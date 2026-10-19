//! The syntect-backed highlighter (TXT-007 R4-R8, R12, R18).
//!
//! Grammars are TextMate/Sublime definitions executed by syntect with its native Oniguruma
//! regex engine. The whole document is parsed line by line, terminators included, so the
//! grammar state (block comments, strings, heredocs, embedded languages) is carried across
//! newlines. The scope stack of every token region is mapped to a neutral [`TokenKind`].

use super::document::{HighlightError, HighlightedDocument, Limits};
use super::kind::{TokenKind, kind_for_scope, scope_matches_prefix};
use super::language::{Language, detect};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// A highlight request (TXT-007 public API): the source, the optional path and fence label that
/// drive language detection, and the limits.
#[derive(Clone, Copy, Debug)]
pub struct HighlightRequest<'a> {
    pub source: &'a str,
    pub path: Option<&'a str>,
    pub fence_label: Option<&'a str>,
    pub limits: Limits,
}

impl<'a> HighlightRequest<'a> {
    /// A request with no path, no fence label and the default limits.
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            path: None,
            fence_label: None,
            limits: Limits::default(),
        }
    }

    /// Sets the path used for language detection.
    pub fn path(mut self, path: &'a str) -> Self {
        self.path = Some(path);
        self
    }

    /// Sets the fence label (language alias) used for language detection.
    pub fn fence_label(mut self, label: &'a str) -> Self {
        self.fence_label = Some(label);
        self
    }

    /// Sets the limits.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The language this request resolves to (TXT-007 R1), if any.
    pub fn language(&self) -> Option<Language> {
        detect(self.fence_label, self.path, Some(first_line(self.source)))
    }
}

/// The text of `source` before its first newline (the whole source when it has none).
pub fn first_line(source: &str) -> &str {
    source.split('\n').next().unwrap_or(source)
}

/// Resolves the language of the request and highlights it; an unresolved language is
/// [`HighlightError::UnknownLanguage`].
pub fn highlight(request: &HighlightRequest<'_>) -> Result<HighlightedDocument, HighlightError> {
    let language = request.language().ok_or(HighlightError::UnknownLanguage)?;
    highlight_language(language, request.source, request.limits)
}

/// Highlights `source` as `language` (TXT-007 R4-R10, R12).
pub fn highlight_language(
    language: Language,
    source: &str,
    limits: Limits,
) -> Result<HighlightedDocument, HighlightError> {
    SCOPE_INFO.with_borrow_mut(|memo| highlight_with_memo(language, source, limits, memo))
}

thread_local! {
    /// What the mapping needs to know about every scope seen on this thread; a scope's name never
    /// changes, so the memo outlives single documents.
    static SCOPE_INFO: RefCell<HashMap<Scope, ScopeInfo>> = RefCell::new(HashMap::new());
}

fn highlight_with_memo(
    language: Language,
    source: &str,
    limits: Limits,
    memo: &mut HashMap<Scope, ScopeInfo>,
) -> Result<HighlightedDocument, HighlightError> {
    if source.len() > limits.max_source_bytes {
        return Err(HighlightError::SourceTooLarge);
    }
    let syntax = grammar(language).ok_or(HighlightError::UnknownLanguage)?;
    let syntaxes = syntax_set();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut spans: Vec<(Range<usize>, TokenKind)> = Vec::new();
    let mut scratch = String::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let text: &str = if line.ends_with('\n') {
            line
        } else {
            // The grammars are newline-terminated: give the last line the terminator it lacks.
            scratch.clear();
            scratch.push_str(line);
            scratch.push('\n');
            &scratch
        };
        let ops = state
            .parse_line(text, syntaxes)
            .map_err(|error| HighlightError::Parser(error.to_string()))?;
        let mut position = 0;
        for (op_position, op) in &ops {
            emit_region(
                language,
                &stack,
                memo,
                text,
                position..*op_position,
                line.len(),
                offset,
                &mut spans,
                limits,
            )?;
            position = *op_position;
            stack
                .apply(op)
                .map_err(|error| HighlightError::Parser(error.to_string()))?;
        }
        emit_region(
            language,
            &stack,
            memo,
            text,
            position..text.len(),
            line.len(),
            offset,
            &mut spans,
            limits,
        )?;
        offset += line.len();
    }
    HighlightedDocument::from_absolute_spans(language, source, spans, limits)
}

/// Classifies the region `range` of `text` (clipped to the real line length) and appends its
/// absolute span, failing as soon as the raw span limit is crossed.
#[allow(clippy::too_many_arguments)]
fn emit_region(
    language: Language,
    stack: &ScopeStack,
    memo: &mut HashMap<Scope, ScopeInfo>,
    text: &str,
    range: Range<usize>,
    line_len: usize,
    line_offset: usize,
    spans: &mut Vec<(Range<usize>, TokenKind)>,
    limits: Limits,
) -> Result<(), HighlightError> {
    let end = range.end.min(line_len);
    if range.start >= end {
        return Ok(());
    }
    if let Some(kind) = classify_region(language, stack.as_slice(), memo, &text[range.start..end]) {
        if spans.len() >= limits.max_raw_spans {
            return Err(HighlightError::TooManySpans);
        }
        spans.push((line_offset + range.start..line_offset + end, kind));
    }
    Ok(())
}

/// What the mapping needs to know about one scope; computed once per distinct scope.
#[derive(Clone, Copy)]
struct ScopeInfo {
    kind: Option<TokenKind>,
    constant_language: bool,
    storage_type: bool,
}

impl ScopeInfo {
    fn of(scope: Scope) -> Self {
        let name = scope.build_string();
        Self {
            kind: kind_for_scope(&name),
            constant_language: scope_matches_prefix(&name, "constant.language"),
            storage_type: scope_matches_prefix(&name, "storage.type"),
        }
    }
}

/// Kind of a token region (TXT-007 R8): the highest-precedence mapped scope (deeper wins ties),
/// then the `constant.language` and `storage.type` refinements using the trimmed region text.
fn classify_region(
    language: Language,
    stack: &[Scope],
    memo: &mut HashMap<Scope, ScopeInfo>,
    region: &str,
) -> Option<TokenKind> {
    let mut best: Option<TokenKind> = None;
    let mut constant_language = false;
    let mut storage_type = false;
    for scope in stack {
        let info = *memo.entry(*scope).or_insert_with(|| ScopeInfo::of(*scope));
        constant_language |= info.constant_language;
        storage_type |= info.storage_type;
        if let Some(kind) = info.kind {
            if best.is_none_or(|b| kind.precedence() >= b.precedence()) {
                best = Some(kind);
            }
        }
    }
    let best = best?;
    let text = region.trim();
    if constant_language {
        return Some(if is_boolean_word(text) {
            TokenKind::Boolean
        } else {
            TokenKind::Constant
        });
    }
    if storage_type {
        return Some(if is_builtin_type(language, text) {
            if language == Language::C {
                TokenKind::Type
            } else {
                TokenKind::TypeBuiltin
            }
        } else {
            TokenKind::Keyword
        });
    }
    Some(best)
}

fn is_boolean_word(text: &str) -> bool {
    matches!(
        text,
        "true" | "false" | "True" | "False" | "TRUE" | "FALSE" | "yes" | "no" | "on" | "off"
    )
}

/// Built-in type names per language (TXT-007 R8).
fn is_builtin_type(language: Language, text: &str) -> bool {
    match language {
        Language::C => matches!(
            text,
            "int"
                | "void"
                | "char"
                | "unsigned"
                | "signed"
                | "long"
                | "short"
                | "float"
                | "double"
                | "_Bool"
                | "bool"
                | "size_t"
                | "ssize_t"
                | "int8_t"
                | "int16_t"
                | "int32_t"
                | "int64_t"
                | "uint8_t"
                | "uint16_t"
                | "uint32_t"
                | "uint64_t"
                | "uintptr_t"
                | "intptr_t"
                | "ptrdiff_t"
        ),
        Language::Rust => matches!(
            text,
            "i8" | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "f32"
                | "f64"
                | "bool"
                | "char"
                | "str"
                | "Self"
        ),
        Language::JavaScript | Language::Jsx | Language::TypeScript | Language::Tsx => matches!(
            text,
            "string"
                | "number"
                | "boolean"
                | "bigint"
                | "symbol"
                | "object"
                | "undefined"
                | "never"
                | "unknown"
                | "any"
                | "void"
        ),
        _ => false,
    }
}

/// The shared grammar set (loaded once, about 3 ms).
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// The grammar of `language` (TXT-007 R5): primary name, listed fallback names, then lookup by
/// file extension; `None` when no grammar is found.
fn grammar(language: Language) -> Option<&'static SyntaxReference> {
    static GRAMMARS: OnceLock<Vec<Option<&'static SyntaxReference>>> = OnceLock::new();
    let grammars = GRAMMARS.get_or_init(|| {
        let set = syntax_set();
        Language::ALL
            .iter()
            .map(|language| {
                language
                    .grammar_names()
                    .iter()
                    .find_map(|name| set.find_syntax_by_name(name))
                    .or_else(|| {
                        language
                            .grammar_extensions()
                            .iter()
                            .find_map(|extension| set.find_syntax_by_extension(extension))
                    })
            })
            .collect()
    });
    let index = Language::ALL.iter().position(|l| *l == language)?;
    grammars[index]
}

/// Warms the grammar set so the first highlight does not pay for loading it.
pub fn preload_grammars() {
    let _ = grammar(Language::Rust);
}
