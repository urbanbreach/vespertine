//! The preprocessor.

use crate::expr::evaluate;
use crate::lexer::{Raw, RawToken, lex};
use crate::token::{
    Extension, Integer, Location, Pragma, PreprocessorError, Punct, Token, TokenValue, Version,
};
use std::collections::HashMap;

type Item = Result<Token, (PreprocessorError, Location)>;

#[derive(Clone)]
struct Macro {
    params: Option<Vec<String>>,
    body: Vec<RawToken>,
}

/// Preprocesses GLSL text into a stream of tokens.
///
/// Definitions added with [`Preprocessor::add_define`] before the first token is requested act
/// like `#define` lines at the top of the text.
pub struct Preprocessor<'a> {
    input: &'a str,
    predefined: Vec<(String, Vec<RawToken>)>,
    output: Option<std::vec::IntoIter<Item>>,
}

impl<'a> Preprocessor<'a> {
    /// A preprocessor for `input`.
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            predefined: Vec::new(),
            output: None,
        }
    }

    /// Defines the object-like macro `name` as `value`.
    pub fn add_define(&mut self, name: &str, value: &str) -> Result<(), PreprocessorError> {
        let valid =
            name.bytes().enumerate().all(|(i, c)| {
                c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
            }) && !name.is_empty();
        if !valid {
            return Err(PreprocessorError::UnexpectedCharacter);
        }
        let mut body = Vec::new();
        for token in lex(value) {
            match token.kind {
                Raw::Newline => {}
                Raw::Error(error) => return Err(error),
                _ => body.push(token),
            }
        }
        self.predefined.push((name.to_owned(), body));
        Ok(())
    }
}

impl Iterator for Preprocessor<'_> {
    type Item = Item;

    fn next(&mut self) -> Option<Item> {
        if self.output.is_none() {
            let output = Run::new(self.input, std::mem::take(&mut self.predefined)).run();
            self.output = Some(output.into_iter());
        }
        self.output.as_mut().and_then(Iterator::next)
    }
}

struct Cond {
    parent_active: bool,
    taken: bool,
    active: bool,
    saw_else: bool,
}

struct Run<'a> {
    input: &'a str,
    line_starts: Vec<usize>,
    macros: HashMap<String, Macro>,
    conds: Vec<Cond>,
    output: Vec<Item>,
    version: u64,
    line_offset: i64,
}

fn convert(token: &RawToken, location: Location) -> Item {
    let value = match &token.kind {
        Raw::Ident(name) => TokenValue::Ident(name.clone()),
        Raw::Int(integer) => TokenValue::Integer(*integer),
        Raw::Float(float) => TokenValue::Float(*float),
        Raw::Punct(punct) => TokenValue::Punct(*punct),
        Raw::Hash => return Err((PreprocessorError::UnexpectedHash, location)),
        Raw::Error(error) => return Err((error.clone(), location)),
        Raw::Newline => return Err((PreprocessorError::UnexpectedNewLine, location)),
    };
    Ok(Token { value, location })
}

impl<'a> Run<'a> {
    fn new(input: &'a str, predefined: Vec<(String, Vec<RawToken>)>) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(input.match_indices('\n').map(|(i, _)| i + 1));
        let mut macros = HashMap::new();
        for (name, body) in predefined {
            macros.insert(name, Macro { params: None, body });
        }
        Self {
            input,
            line_starts,
            macros,
            conds: Vec::new(),
            output: Vec::new(),
            version: 110,
            line_offset: 0,
        }
    }

    fn line_of(&self, offset: u32) -> u32 {
        let physical = self.line_starts.partition_point(|&s| s <= offset as usize) as i64;
        (physical + self.line_offset).max(0) as u32
    }

    fn active(&self) -> bool {
        self.conds.last().is_none_or(|c| c.active)
    }

    fn error(&mut self, error: PreprocessorError, location: Location) {
        self.output.push(Err((error, location)));
    }

    fn run(mut self) -> Vec<Item> {
        let tokens = lex(self.input);
        let mut lines: Vec<Vec<RawToken>> = vec![Vec::new()];
        for token in tokens {
            if token.kind == Raw::Newline {
                lines.push(Vec::new());
            } else {
                lines.last_mut().expect("at least one line").push(token);
            }
        }
        for line in &lines {
            self.line(line);
        }
        if !self.conds.is_empty() {
            let end = Location::new(self.input.len(), self.input.len());
            self.error(PreprocessorError::UnterminatedConditional, end);
        }
        self.output
    }

    fn line(&mut self, line: &[RawToken]) {
        let Some(first) = line.first() else { return };
        if first.kind == Raw::Hash {
            self.directive(&line[1..], first.loc);
        } else if self.active() {
            let mut out = Vec::new();
            self.expand(line, None, &mut Vec::new(), &mut out);
            for item in out {
                self.output.push(item);
            }
        }
    }

    fn directive_tokens(&mut self, tokens: &[RawToken]) -> Vec<Token> {
        let mut out = Vec::new();
        for token in tokens {
            match convert(token, token.loc) {
                Ok(token) => out.push(token),
                Err((error, location)) => self.error(error, location),
            }
        }
        out
    }

    fn directive(&mut self, tokens: &[RawToken], hash: Location) {
        let Some(name_token) = tokens.first() else {
            return; // the null directive
        };
        let Raw::Ident(name) = &name_token.kind else {
            if self.active() {
                self.error(PreprocessorError::UnexpectedHash, hash);
            }
            return;
        };
        let rest = &tokens[1..];
        let loc = name_token.loc;
        match name.as_str() {
            "if" => {
                let parent_active = self.active();
                let value = if parent_active {
                    self.condition(rest, loc)
                } else {
                    false
                };
                self.conds.push(Cond {
                    parent_active,
                    taken: value,
                    active: parent_active && value,
                    saw_else: false,
                });
            }
            "ifdef" | "ifndef" => {
                let parent_active = self.active();
                let defined = match rest.first().map(|t| &t.kind) {
                    Some(Raw::Ident(macro_name)) => self.is_defined(macro_name),
                    _ => {
                        if parent_active {
                            self.error(PreprocessorError::UnexpectedNewLine, loc);
                        }
                        false
                    }
                };
                let value = if name == "ifdef" { defined } else { !defined };
                self.conds.push(Cond {
                    parent_active,
                    taken: value,
                    active: parent_active && value,
                    saw_else: false,
                });
            }
            "elif" => {
                let Some(cond) = self.conds.last() else {
                    return self.error(PreprocessorError::UnmatchedConditional, loc);
                };
                if cond.saw_else {
                    return self.error(PreprocessorError::ConditionalAfterElse, loc);
                }
                let evaluate_now = cond.parent_active && !cond.taken;
                let value = if evaluate_now {
                    self.condition(rest, loc)
                } else {
                    false
                };
                let cond = self.conds.last_mut().expect("checked above");
                cond.active = evaluate_now && value;
                cond.taken |= value;
            }
            "else" => {
                let Some(cond) = self.conds.last_mut() else {
                    return self.error(PreprocessorError::UnmatchedConditional, loc);
                };
                if cond.saw_else {
                    return self.error(PreprocessorError::ConditionalAfterElse, loc);
                }
                cond.saw_else = true;
                cond.active = cond.parent_active && !cond.taken;
                cond.taken = true;
            }
            "endif" => {
                if self.conds.pop().is_none() {
                    self.error(PreprocessorError::UnmatchedConditional, loc);
                }
            }
            _ if !self.active() => {}
            "define" => self.define(rest, loc),
            "undef" => match rest.first().map(|t| &t.kind) {
                Some(Raw::Ident(macro_name)) => {
                    self.macros.remove(macro_name);
                }
                _ => self.error(PreprocessorError::UnexpectedNewLine, loc),
            },
            "version" => {
                let is_first = self.output.is_empty();
                let tokens = self.directive_tokens(rest);
                if let Some(Token {
                    value: TokenValue::Integer(integer),
                    ..
                }) = tokens.first()
                {
                    self.version = integer.value;
                }
                self.output.push(Ok(Token {
                    value: TokenValue::Version(Version {
                        tokens,
                        is_first_directive: is_first,
                        has_comments_before: false,
                    }),
                    location: loc,
                }));
            }
            "extension" => {
                let tokens = self.directive_tokens(rest);
                self.output.push(Ok(Token {
                    value: TokenValue::Extension(Extension {
                        tokens,
                        has_comments_before: false,
                    }),
                    location: loc,
                }));
            }
            "pragma" => {
                let tokens = self.directive_tokens(rest);
                self.output.push(Ok(Token {
                    value: TokenValue::Pragma(Pragma { tokens }),
                    location: loc,
                }));
            }
            "line" => {
                // `#line N` makes the next line number N.
                if let Some(Raw::Int(integer)) = rest.first().map(|t| &t.kind) {
                    let physical = self
                        .line_starts
                        .partition_point(|&s| s <= loc.start as usize)
                        as i64;
                    self.line_offset = integer.value as i64 - physical - 1;
                }
            }
            "error" => {
                let text = rest
                    .first()
                    .zip(rest.last())
                    .map(|(a, b)| self.input[a.loc.start as usize..b.loc.end as usize].to_owned())
                    .unwrap_or_default();
                self.error(PreprocessorError::ErrorDirective(text), loc);
            }
            other => self.error(PreprocessorError::UnknownDirective(other.to_owned()), loc),
        }
    }

    fn is_defined(&self, name: &str) -> bool {
        self.macros.contains_key(name) || matches!(name, "__LINE__" | "__FILE__" | "__VERSION__")
    }

    fn condition(&mut self, tokens: &[RawToken], loc: Location) -> bool {
        if tokens.is_empty() {
            self.error(PreprocessorError::UnexpectedNewLine, loc);
            return false;
        }
        // Resolve `defined` before macro expansion.
        let mut resolved = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            if matches!(&token.kind, Raw::Ident(n) if n == "defined") {
                let (name, consumed) =
                    match (tokens.get(i + 1), tokens.get(i + 2), tokens.get(i + 3)) {
                        (
                            Some(RawToken {
                                kind: Raw::Ident(n),
                                ..
                            }),
                            _,
                            _,
                        ) => (n.clone(), 2),
                        (
                            Some(RawToken {
                                kind: Raw::Punct(Punct::LeftParen),
                                ..
                            }),
                            Some(RawToken {
                                kind: Raw::Ident(n),
                                ..
                            }),
                            Some(RawToken {
                                kind: Raw::Punct(Punct::RightParen),
                                ..
                            }),
                        ) => (n.clone(), 4),
                        _ => {
                            self.error(PreprocessorError::InvalidExpression, token.loc);
                            return false;
                        }
                    };
                resolved.push(RawToken {
                    kind: Raw::Int(Integer {
                        value: self.is_defined(&name) as u64,
                        signed: true,
                        width: 32,
                    }),
                    loc: token.loc,
                });
                i += consumed;
            } else {
                resolved.push(token.clone());
                i += 1;
            }
        }
        let mut expanded = Vec::new();
        self.expand(&resolved, None, &mut Vec::new(), &mut expanded);
        let mut raw = Vec::new();
        for item in expanded {
            match item {
                Ok(token) => raw.push(RawToken {
                    kind: match token.value {
                        TokenValue::Ident(name) => Raw::Ident(name),
                        TokenValue::Integer(integer) => Raw::Int(integer),
                        TokenValue::Float(float) => Raw::Float(float),
                        TokenValue::Punct(punct) => Raw::Punct(punct),
                        _ => Raw::Ident(String::new()),
                    },
                    loc: token.location,
                }),
                Err((error, location)) => {
                    self.error(error, location);
                    return false;
                }
            }
        }
        match evaluate(&raw) {
            Ok(value) => value != 0,
            Err(error) => {
                self.error(error, loc);
                false
            }
        }
    }

    fn define(&mut self, rest: &[RawToken], loc: Location) {
        let Some(RawToken {
            kind: Raw::Ident(name),
            loc: name_loc,
        }) = rest.first()
        else {
            return self.error(PreprocessorError::UnexpectedNewLine, loc);
        };
        if name.starts_with("__") {
            return self.error(PreprocessorError::ReservedMacro(name.clone()), *name_loc);
        }
        let mut body_start = 1;
        let mut params = None;
        if let Some(RawToken {
            kind: Raw::Punct(Punct::LeftParen),
            loc: paren,
        }) = rest.get(1)
            && paren.start == name_loc.end
        {
            let mut names: Vec<String> = Vec::new();
            let mut i = 2;
            loop {
                match rest.get(i).map(|t| (&t.kind, t.loc)) {
                    Some((Raw::Punct(Punct::RightParen), _)) if names.is_empty() => {
                        i += 1;
                        break;
                    }
                    Some((Raw::Ident(p), p_loc)) => {
                        if names.contains(p) {
                            return self
                                .error(PreprocessorError::DuplicateParameter(p.clone()), p_loc);
                        }
                        names.push(p.clone());
                        i += 1;
                        match rest.get(i).map(|t| &t.kind) {
                            Some(Raw::Punct(Punct::Comma)) => i += 1,
                            Some(Raw::Punct(Punct::RightParen)) => {
                                i += 1;
                                break;
                            }
                            _ => return self.error(PreprocessorError::InvalidExpression, p_loc),
                        }
                    }
                    Some((_, other)) => {
                        return self.error(PreprocessorError::InvalidExpression, other);
                    }
                    None => return self.error(PreprocessorError::UnexpectedNewLine, *name_loc),
                }
            }
            params = Some(names);
            body_start = i;
        }
        let body = rest[body_start..].to_vec();
        self.macros.insert(name.clone(), Macro { params, body });
    }

    /// Expands macros in `tokens`, appending the result to `out`. `call` is the span of the
    /// outermost macro invocation: every token produced by an expansion carries it.
    fn expand(
        &mut self,
        tokens: &[RawToken],
        call: Option<Location>,
        hide: &mut Vec<String>,
        out: &mut Vec<Item>,
    ) {
        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];
            i += 1;
            let location = call.unwrap_or(token.loc);
            let Raw::Ident(name) = &token.kind else {
                out.push(convert(token, location));
                continue;
            };
            let builtin = |value: u64| {
                Ok(Token {
                    value: TokenValue::Integer(Integer {
                        value,
                        signed: true,
                        width: 32,
                    }),
                    location,
                })
            };
            match name.as_str() {
                "__LINE__" => {
                    out.push(builtin(self.line_of(token.loc.start) as u64));
                    continue;
                }
                "__FILE__" => {
                    out.push(builtin(0));
                    continue;
                }
                "__VERSION__" => {
                    out.push(builtin(self.version));
                    continue;
                }
                _ => {}
            }
            let Some(definition) = self.macros.get(name).cloned() else {
                out.push(convert(token, location));
                continue;
            };
            if hide.contains(name) {
                out.push(convert(token, location));
                continue;
            }
            match &definition.params {
                None => {
                    hide.push(name.clone());
                    self.expand(&definition.body, Some(location), hide, out);
                    hide.pop();
                }
                Some(params) => {
                    // A function-like macro without an argument list is an ordinary identifier.
                    if !matches!(
                        tokens.get(i),
                        Some(RawToken {
                            kind: Raw::Punct(Punct::LeftParen),
                            ..
                        })
                    ) {
                        out.push(convert(token, location));
                        continue;
                    }
                    let mut depth = 0usize;
                    let mut args: Vec<Vec<RawToken>> = vec![Vec::new()];
                    let mut j = i + 1;
                    let mut closed = None;
                    while j < tokens.len() {
                        match &tokens[j].kind {
                            Raw::Punct(Punct::LeftParen) => {
                                depth += 1;
                                args.last_mut()
                                    .expect("one argument")
                                    .push(tokens[j].clone());
                            }
                            Raw::Punct(Punct::RightParen) if depth == 0 => {
                                closed = Some(tokens[j].loc);
                                break;
                            }
                            Raw::Punct(Punct::RightParen) => {
                                depth -= 1;
                                args.last_mut()
                                    .expect("one argument")
                                    .push(tokens[j].clone());
                            }
                            Raw::Punct(Punct::Comma) if depth == 0 => args.push(Vec::new()),
                            _ => args
                                .last_mut()
                                .expect("one argument")
                                .push(tokens[j].clone()),
                        }
                        j += 1;
                    }
                    let Some(close) = closed else {
                        out.push(Err((PreprocessorError::UnexpectedEndOfInput, token.loc)));
                        return;
                    };
                    if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                        args.clear();
                    }
                    if args.len() != params.len() {
                        out.push(Err((
                            PreprocessorError::WrongNumberOfArguments {
                                name: name.clone(),
                                expected: params.len(),
                                found: args.len(),
                            },
                            token.loc,
                        )));
                        i = j + 1;
                        continue;
                    }
                    let invocation = call.unwrap_or(token.loc.join(close));
                    // Expand the arguments, then substitute them into the body.
                    let mut expanded_args: Vec<Vec<RawToken>> = Vec::new();
                    for arg in &args {
                        let mut items = Vec::new();
                        self.expand(arg, Some(invocation), hide, &mut items);
                        let mut raw = Vec::new();
                        for item in items {
                            match item {
                                Ok(t) => raw.push(RawToken {
                                    kind: match t.value {
                                        TokenValue::Ident(n) => Raw::Ident(n),
                                        TokenValue::Integer(v) => Raw::Int(v),
                                        TokenValue::Float(v) => Raw::Float(v),
                                        TokenValue::Punct(v) => Raw::Punct(v),
                                        _ => continue,
                                    },
                                    loc: t.location,
                                }),
                                Err(e) => out.push(Err(e)),
                            }
                        }
                        expanded_args.push(raw);
                    }
                    let mut body = Vec::new();
                    for body_token in &definition.body {
                        match &body_token.kind {
                            Raw::Ident(n) if params.contains(n) => {
                                let index = params.iter().position(|p| p == n).expect("contains");
                                body.extend(expanded_args[index].iter().cloned());
                            }
                            _ => body.push(body_token.clone()),
                        }
                    }
                    hide.push(name.clone());
                    self.expand(&body, Some(invocation), hide, out);
                    hide.pop();
                    i = j + 1;
                }
            }
        }
    }
}
