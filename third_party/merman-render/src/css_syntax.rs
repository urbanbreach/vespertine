//! A CSS Syntax Module Level 3 tokenizer and component-value parser.
//!
//! Written from the W3C specification (<https://www.w3.org/TR/css-syntax-3/>): the tokenizer
//! follows section 4 ("Tokenization"), the parser walks the token stream the way sections 5.4
//! ("Parser Algorithms") describe it: simple blocks and functions nest, a list of rules is made
//! of at-rules and qualified rules, and error recovery skips to the end of the offending rule or
//! declaration.
//!
//! The whole input is tokenized once into a token vector with source offsets. Blocks are matched
//! iteratively, so arbitrarily deep nesting costs no stack; the callers bound recursion
//! themselves. Every token reports its source span so callers can copy unmodified source text
//! (comments included) instead of re-serializing tokens.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

/// A CSS token (CSS Syntax 4, "Token Railroad Diagrams"). Comments are not tokens.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token<'i> {
    Ident(Cow<'i, str>),
    AtKeyword(Cow<'i, str>),
    /// `#name` that is not a valid identifier (type flag "unrestricted").
    Hash(Cow<'i, str>),
    /// `#name` that is a valid identifier (type flag "id").
    IDHash(Cow<'i, str>),
    QuotedString(Cow<'i, str>),
    UnquotedUrl(Cow<'i, str>),
    Delim(char),
    Number {
        has_sign: bool,
        value: f32,
        int_value: Option<i32>,
    },
    Percentage {
        has_sign: bool,
        value: f32,
        int_value: Option<i32>,
    },
    Dimension {
        has_sign: bool,
        value: f32,
        int_value: Option<i32>,
        unit: Cow<'i, str>,
    },
    WhiteSpace(&'i str),
    CDO,
    CDC,
    Colon,
    Semicolon,
    Comma,
    Function(Cow<'i, str>),
    ParenthesisBlock,
    SquareBracketBlock,
    CurlyBracketBlock,
    BadUrl(Cow<'i, str>),
    BadString(Cow<'i, str>),
    CloseParenthesis,
    CloseSquareBracket,
    CloseCurlyBracket,
}

impl Token<'_> {
    fn opens_block(&self) -> bool {
        matches!(
            self,
            Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock
        )
    }

    fn closing_kind(&self) -> Option<BlockKind> {
        match self {
            Token::CloseParenthesis => Some(BlockKind::Parenthesis),
            Token::CloseSquareBracket => Some(BlockKind::SquareBracket),
            Token::CloseCurlyBracket => Some(BlockKind::CurlyBracket),
            _ => None,
        }
    }

    fn opening_kind(&self) -> Option<BlockKind> {
        match self {
            Token::Function(_) | Token::ParenthesisBlock => Some(BlockKind::Parenthesis),
            Token::SquareBracketBlock => Some(BlockKind::SquareBracket),
            Token::CurlyBracketBlock => Some(BlockKind::CurlyBracket),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Parenthesis,
    SquareBracket,
    CurlyBracket,
}

/// Serializes a number token's value the way a CSS serializer writes it: the sign of a
/// signed number is preserved, integers print without a fraction, and a number written with a
/// fraction or exponent keeps a decimal point.
pub(crate) fn number_to_css(value: f32, int_value: Option<i32>, has_sign: bool) -> String {
    let mut out = String::new();
    if has_sign && value.is_sign_positive() {
        out.push('+');
    }
    if let Some(int_value) = int_value {
        out.push_str(&int_value.to_string());
        return out;
    }
    if value == 0.0 && value.is_sign_negative() {
        out.push_str("-0");
    } else {
        out.push_str(&value.to_string());
    }
    if value.fract() == 0.0 && !out.contains('.') && !out.contains('e') {
        out.push_str(".0");
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Tok<'i> {
    token: Token<'i>,
    start: usize,
    end: usize,
}

fn is_newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{000C}')
}

fn is_whitespace(c: char) -> bool {
    is_newline(c) || matches!(c, '\t' | ' ')
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_ident_char(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit() || c == '-'
}

fn is_non_printable(c: char) -> bool {
    matches!(c, '\u{0000}'..='\u{0008}' | '\u{000B}' | '\u{000E}'..='\u{001F}' | '\u{007F}')
}

struct Tokenizer<'i> {
    src: &'i str,
    pos: usize,
}

impl<'i> Tokenizer<'i> {
    /// The code point `n` positions ahead; U+0000 reads as U+FFFD (input preprocessing).
    fn peek(&self, n: usize) -> Option<char> {
        self.src[self.pos..]
            .chars()
            .nth(n)
            .map(|c| if c == '\0' { '\u{FFFD}' } else { c })
    }

    fn bump(&mut self) {
        if let Some(c) = self.src[self.pos..].chars().next() {
            self.pos += c.len_utf8();
        }
    }

    fn starts_with(&self, text: &str) -> bool {
        self.src[self.pos..].starts_with(text)
    }

    fn is_valid_escape(&self, n: usize) -> bool {
        self.peek(n) == Some('\\') && !self.peek(n + 1).is_some_and(is_newline)
    }

    fn would_start_ident(&self, n: usize) -> bool {
        match self.peek(n) {
            Some('-') => match self.peek(n + 1) {
                Some(c) if is_ident_start(c) || c == '-' => true,
                _ => self.is_valid_escape(n + 1),
            },
            Some('\\') => self.is_valid_escape(n),
            Some(c) => is_ident_start(c),
            None => false,
        }
    }

    fn would_start_number(&self, n: usize) -> bool {
        match self.peek(n) {
            Some('+' | '-') => {
                self.peek(n + 1).is_some_and(|c| c.is_ascii_digit())
                    || (self.peek(n + 1) == Some('.')
                        && self.peek(n + 2).is_some_and(|c| c.is_ascii_digit()))
            }
            Some('.') => self.peek(n + 1).is_some_and(|c| c.is_ascii_digit()),
            Some(c) => c.is_ascii_digit(),
            None => false,
        }
    }

    fn skip_comments(&mut self) {
        while self.starts_with("/*") {
            self.pos += 2;
            match self.src[self.pos..].find("*/") {
                Some(offset) => self.pos += offset + 2,
                None => self.pos = self.src.len(),
            }
        }
    }

    /// Consumes an escape; the backslash is already consumed.
    fn consume_escape(&mut self) -> char {
        let Some(c) = self.peek(0) else {
            return '\u{FFFD}';
        };
        if !c.is_ascii_hexdigit() {
            self.bump();
            return c;
        }
        let mut value = 0u32;
        let mut digits = 0;
        while digits < 6 {
            let Some(digit) = self.peek(0).and_then(|c| c.to_digit(16)) else {
                break;
            };
            value = value * 16 + digit;
            digits += 1;
            self.bump();
        }
        if self.peek(0).is_some_and(is_whitespace) {
            let crlf = self.starts_with("\r\n");
            self.bump();
            if crlf {
                self.bump();
            }
        }
        match char::from_u32(value) {
            Some(c) if value != 0 => c,
            _ => '\u{FFFD}',
        }
    }

    fn consume_name(&mut self) -> Cow<'i, str> {
        let start = self.pos;
        let mut owned: Option<String> = None;
        let mut segment = start;
        loop {
            let Some(c) = self.peek(0) else { break };
            if is_ident_char(c) {
                if self.src[self.pos..].starts_with('\0') {
                    let out = owned.get_or_insert_with(String::new);
                    out.push_str(&self.src[segment..self.pos]);
                    out.push('\u{FFFD}');
                    self.bump();
                    segment = self.pos;
                } else {
                    self.bump();
                }
            } else if self.is_valid_escape(0) {
                let out = owned.get_or_insert_with(String::new);
                out.push_str(&self.src[segment..self.pos]);
                self.bump();
                out.push(self.consume_escape());
                segment = self.pos;
            } else {
                break;
            }
        }
        match owned {
            Some(mut out) => {
                out.push_str(&self.src[segment..self.pos]);
                Cow::Owned(out)
            }
            None => Cow::Borrowed(&self.src[start..self.pos]),
        }
    }

    fn consume_string(&mut self, quote: char) -> Token<'i> {
        self.bump();
        let mut out = String::new();
        loop {
            match self.peek(0) {
                None => break,
                Some(c) if c == quote => {
                    self.bump();
                    break;
                }
                Some(c) if is_newline(c) => return Token::BadString(Cow::Owned(out)),
                Some('\\') => {
                    self.bump();
                    match self.peek(0) {
                        None => {}
                        Some(c) if is_newline(c) => {
                            let crlf = self.starts_with("\r\n");
                            self.bump();
                            if crlf {
                                self.bump();
                            }
                        }
                        Some(_) => out.push(self.consume_escape()),
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
        Token::QuotedString(Cow::Owned(out))
    }

    /// Consumes a number; returns its value, whether it is an integer, and whether it has a sign.
    fn consume_number(&mut self) -> (f64, bool, bool) {
        let start = self.pos;
        let has_sign = matches!(self.peek(0), Some('+' | '-'));
        if has_sign {
            self.bump();
        }
        let mut is_integer = true;
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            is_integer = false;
            self.bump();
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        if matches!(self.peek(0), Some('e' | 'E')) {
            let signed = matches!(self.peek(1), Some('+' | '-'));
            let digit_at = if signed { 2 } else { 1 };
            if self.peek(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                is_integer = false;
                self.bump();
                if signed {
                    self.bump();
                }
                while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                    self.bump();
                }
            }
        }
        let value = self.src[start..self.pos].parse::<f64>().unwrap_or(0.0);
        (value, is_integer, has_sign)
    }

    fn consume_numeric(&mut self) -> Token<'i> {
        let (value, is_integer, has_sign) = self.consume_number();
        let int_value = is_integer.then(|| {
            if value >= f64::from(i32::MAX) {
                i32::MAX
            } else if value <= f64::from(i32::MIN) {
                i32::MIN
            } else {
                value as i32
            }
        });
        let value = value as f32;
        if self.would_start_ident(0) {
            let unit = self.consume_name();
            Token::Dimension {
                has_sign,
                value,
                int_value,
                unit,
            }
        } else if self.peek(0) == Some('%') {
            self.bump();
            Token::Percentage {
                has_sign,
                value,
                int_value,
            }
        } else {
            Token::Number {
                has_sign,
                value,
                int_value,
            }
        }
    }

    fn consume_bad_url_remnants(&mut self) {
        loop {
            match self.peek(0) {
                None => return,
                Some(')') => {
                    self.bump();
                    return;
                }
                _ if self.is_valid_escape(0) => {
                    self.bump();
                    self.consume_escape();
                }
                Some(_) => self.bump(),
            }
        }
    }

    /// Consumes the rest of an unquoted `url(` token; `url(` is already consumed.
    fn consume_url(&mut self) -> Token<'i> {
        while self.peek(0).is_some_and(is_whitespace) {
            self.bump();
        }
        let mut out = String::new();
        loop {
            match self.peek(0) {
                None => return Token::UnquotedUrl(Cow::Owned(out)),
                Some(')') => {
                    self.bump();
                    return Token::UnquotedUrl(Cow::Owned(out));
                }
                Some(c) if is_whitespace(c) => {
                    while self.peek(0).is_some_and(is_whitespace) {
                        self.bump();
                    }
                    match self.peek(0) {
                        None => return Token::UnquotedUrl(Cow::Owned(out)),
                        Some(')') => {
                            self.bump();
                            return Token::UnquotedUrl(Cow::Owned(out));
                        }
                        Some(_) => {
                            self.consume_bad_url_remnants();
                            return Token::BadUrl(Cow::Owned(out));
                        }
                    }
                }
                Some('"' | '\'' | '(') => {
                    self.consume_bad_url_remnants();
                    return Token::BadUrl(Cow::Owned(out));
                }
                Some(c) if is_non_printable(c) => {
                    self.consume_bad_url_remnants();
                    return Token::BadUrl(Cow::Owned(out));
                }
                Some('\\') => {
                    if self.is_valid_escape(0) {
                        self.bump();
                        out.push(self.consume_escape());
                    } else {
                        self.consume_bad_url_remnants();
                        return Token::BadUrl(Cow::Owned(out));
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.bump();
                }
            }
        }
    }

    fn consume_ident_like(&mut self) -> Token<'i> {
        let name = self.consume_name();
        if self.peek(0) != Some('(') {
            return Token::Ident(name);
        }
        self.bump();
        if !name.eq_ignore_ascii_case("url") {
            return Token::Function(name);
        }
        // `url(` followed by optional whitespace and a quote is a function token; anything
        // else starts an unquoted url.
        let mut ahead = 0;
        while self.peek(ahead).is_some_and(is_whitespace) {
            ahead += 1;
        }
        if matches!(self.peek(ahead), Some('"' | '\'')) {
            Token::Function(name)
        } else {
            self.consume_url()
        }
    }

    fn consume_token(&mut self) -> Token<'i> {
        let Some(c) = self.peek(0) else {
            unreachable!("consume_token is only called before the end of input");
        };
        match c {
            c if is_whitespace(c) => {
                let start = self.pos;
                while self.peek(0).is_some_and(is_whitespace) {
                    self.bump();
                }
                Token::WhiteSpace(&self.src[start..self.pos])
            }
            '"' | '\'' => self.consume_string(c),
            '#' => {
                self.bump();
                if self.peek(0).is_some_and(is_ident_char) || self.is_valid_escape(0) {
                    let is_id = self.would_start_ident(0);
                    let name = self.consume_name();
                    if is_id {
                        Token::IDHash(name)
                    } else {
                        Token::Hash(name)
                    }
                } else {
                    Token::Delim('#')
                }
            }
            '(' => {
                self.bump();
                Token::ParenthesisBlock
            }
            ')' => {
                self.bump();
                Token::CloseParenthesis
            }
            '[' => {
                self.bump();
                Token::SquareBracketBlock
            }
            ']' => {
                self.bump();
                Token::CloseSquareBracket
            }
            '{' => {
                self.bump();
                Token::CurlyBracketBlock
            }
            '}' => {
                self.bump();
                Token::CloseCurlyBracket
            }
            ',' => {
                self.bump();
                Token::Comma
            }
            ':' => {
                self.bump();
                Token::Colon
            }
            ';' => {
                self.bump();
                Token::Semicolon
            }
            '+' | '.' => {
                if self.would_start_number(0) {
                    self.consume_numeric()
                } else {
                    self.bump();
                    Token::Delim(c)
                }
            }
            '-' => {
                if self.would_start_number(0) {
                    self.consume_numeric()
                } else if self.starts_with("-->") {
                    self.pos += 3;
                    Token::CDC
                } else if self.would_start_ident(0) {
                    self.consume_ident_like()
                } else {
                    self.bump();
                    Token::Delim('-')
                }
            }
            '<' => {
                if self.starts_with("<!--") {
                    self.pos += 4;
                    Token::CDO
                } else {
                    self.bump();
                    Token::Delim('<')
                }
            }
            '@' => {
                self.bump();
                if self.would_start_ident(0) {
                    Token::AtKeyword(self.consume_name())
                } else {
                    Token::Delim('@')
                }
            }
            '\\' => {
                if self.is_valid_escape(0) {
                    self.consume_ident_like()
                } else {
                    self.bump();
                    Token::Delim('\\')
                }
            }
            c if c.is_ascii_digit() => self.consume_numeric(),
            c if is_ident_start(c) => self.consume_ident_like(),
            c => {
                self.bump();
                Token::Delim(c)
            }
        }
    }
}

fn tokenize(src: &str) -> Vec<Tok<'_>> {
    let mut tokenizer = Tokenizer { src, pos: 0 };
    let mut tokens = Vec::new();
    loop {
        tokenizer.skip_comments();
        if tokenizer.pos >= src.len() {
            return tokens;
        }
        let start = tokenizer.pos;
        let token = tokenizer.consume_token();
        tokens.push(Tok {
            token,
            start,
            end: tokenizer.pos,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// A 1-based line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceLocation {
    pub(crate) line: u32,
    pub(crate) column: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BasicParseErrorKind<'i> {
    UnexpectedToken(Token<'i>),
    EndOfInput,
    QualifiedRuleInvalid,
}

impl fmt::Display for BasicParseErrorKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedToken(token) => write!(f, "unexpected token: {token:?}"),
            Self::EndOfInput => f.write_str("unexpected end of input"),
            Self::QualifiedRuleInvalid => f.write_str("invalid qualified rule"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BasicParseError<'i> {
    pub(crate) kind: BasicParseErrorKind<'i>,
    pub(crate) location: SourceLocation,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParseErrorKind<'i, E> {
    Basic(BasicParseErrorKind<'i>),
    Custom(E),
}

impl<E: fmt::Display> fmt::Display for ParseErrorKind<'_, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Basic(kind) => kind.fmt(f),
            Self::Custom(error) => error.fmt(f),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParseError<'i, E> {
    pub(crate) kind: ParseErrorKind<'i, E>,
    pub(crate) location: SourceLocation,
}

impl<'i, E> From<BasicParseError<'i>> for ParseError<'i, E> {
    fn from(error: BasicParseError<'i>) -> Self {
        Self {
            kind: ParseErrorKind::Basic(error.kind),
            location: error.location,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

/// The tokenized input of a [`Parser`].
pub(crate) struct ParserInput<'i> {
    src: &'i str,
    tokens: Vec<Tok<'i>>,
    /// For every block-opening token the index of its closing token, or `tokens.len()` when the
    /// block is closed by the end of input.
    close: Vec<usize>,
}

impl<'i> ParserInput<'i> {
    pub(crate) fn new(src: &'i str) -> Self {
        let tokens = tokenize(src);
        let mut close = vec![tokens.len(); tokens.len()];
        let mut open: Vec<(usize, BlockKind)> = Vec::new();
        for (index, tok) in tokens.iter().enumerate() {
            if let Some(kind) = tok.token.opening_kind() {
                open.push((index, kind));
            } else if let Some(kind) = tok.token.closing_kind()
                && let Some(&(opener, open_kind)) = open.last()
                && open_kind == kind
            {
                close[opener] = index;
                open.pop();
            }
        }
        Self { src, tokens, close }
    }
}

/// A byte offset into the parser's source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourcePosition(usize);

/// Tokens that end a [`Parser::parse_until_after`] range at the top level of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delimiter {
    Semicolon,
    CurlyBracketBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Delimiters {
    semicolon: bool,
    curly: bool,
}

impl Delimiters {
    fn contains(self, token: &Token<'_>) -> bool {
        (self.semicolon && matches!(token, Token::Semicolon))
            || (self.curly && matches!(token, Token::CurlyBracketBlock))
    }
}

impl From<Delimiter> for Delimiters {
    fn from(delimiter: Delimiter) -> Self {
        Self {
            semicolon: delimiter == Delimiter::Semicolon,
            curly: delimiter == Delimiter::CurlyBracketBlock,
        }
    }
}

#[derive(Clone, Copy)]
struct ParserState {
    pos: usize,
    cursor: usize,
    pending: Option<usize>,
}

/// A cursor over a range of the token stream. Sub-parsers for nested blocks and delimited
/// ranges see only their own tokens, so exhaustion means "end of this block or declaration".
pub(crate) struct Parser<'i, 't> {
    input: &'t ParserInput<'i>,
    pos: usize,
    end: usize,
    /// Source offset after the last consumed token (and any comments that followed it).
    cursor: usize,
    /// The block-opening token returned last, whose contents have not been entered or skipped.
    pending: Option<usize>,
}

impl<'i, 't> Parser<'i, 't> {
    pub(crate) fn new(input: &'t ParserInput<'i>) -> Self {
        Self {
            input,
            pos: 0,
            end: input.tokens.len(),
            cursor: 0,
            pending: None,
        }
    }

    fn state(&self) -> ParserState {
        ParserState {
            pos: self.pos,
            cursor: self.cursor,
            pending: self.pending,
        }
    }

    fn reset(&mut self, state: ParserState) {
        self.pos = state.pos;
        self.cursor = state.cursor;
        self.pending = state.pending;
    }

    fn location_at(&self, offset: usize) -> SourceLocation {
        let prefix = &self.input.src[..offset.min(self.input.src.len())];
        let mut line = 1u32;
        let mut line_start = 0usize;
        let mut chars = prefix.char_indices().peekable();
        while let Some((index, c)) = chars.next() {
            let is_break = match c {
                '\n' | '\u{000C}' => true,
                '\r' => {
                    if chars.peek().is_some_and(|(_, next)| *next == '\n') {
                        chars.next();
                        line_start = index + 2;
                        line += 1;
                        continue;
                    }
                    true
                }
                _ => false,
            };
            if is_break {
                line += 1;
                line_start = index + c.len_utf8();
            }
        }
        let column = prefix[line_start.min(prefix.len())..].chars().count() as u32 + 1;
        SourceLocation { line, column }
    }

    fn current_location(&self) -> SourceLocation {
        self.location_at(self.cursor)
    }

    fn limit_offset(&self) -> usize {
        self.input
            .tokens
            .get(self.end)
            .map_or(self.input.src.len(), |tok| tok.start)
    }

    /// The current position: after the last consumed token.
    pub(crate) fn position(&self) -> SourcePosition {
        SourcePosition(self.cursor)
    }

    pub(crate) fn slice(&self, range: Range<SourcePosition>) -> &'i str {
        &self.input.src[range.start.0..range.end.0]
    }

    pub(crate) fn slice_from(&self, start: SourcePosition) -> &'i str {
        &self.input.src[start.0..self.cursor]
    }

    pub(crate) fn new_custom_error<E>(&self, error: E) -> ParseError<'i, E> {
        ParseError {
            kind: ParseErrorKind::Custom(error),
            location: self.current_location(),
        }
    }

    fn new_basic_error(&self, kind: BasicParseErrorKind<'i>) -> BasicParseError<'i> {
        BasicParseError {
            kind,
            location: self.current_location(),
        }
    }

    /// Skips the contents of a block whose opening token was returned but not entered.
    fn skip_pending_block(&mut self) {
        if let Some(opener) = self.pending.take() {
            let close = self.input.close[opener];
            self.pos = (close + 1).min(self.end);
            self.cursor = self
                .input
                .tokens
                .get(close)
                .map_or(self.input.src.len(), |tok| tok.end);
        }
    }

    /// The next token, whitespace included. A block-opening token must be entered with
    /// [`Self::parse_nested_block`] or is skipped together with its contents by the next call.
    pub(crate) fn next_including_whitespace(
        &mut self,
    ) -> Result<&'t Token<'i>, BasicParseError<'i>> {
        self.skip_pending_block();
        if self.pos >= self.end {
            self.cursor = self.limit_offset();
            return Err(self.new_basic_error(BasicParseErrorKind::EndOfInput));
        }
        let tok = &self.input.tokens[self.pos];
        if tok.token.opens_block() {
            self.pending = Some(self.pos);
        }
        self.pos += 1;
        self.cursor = tok.end;
        Ok(&tok.token)
    }

    /// The next token that is not whitespace.
    pub(crate) fn next(&mut self) -> Result<&'t Token<'i>, BasicParseError<'i>> {
        loop {
            match self.next_including_whitespace()? {
                Token::WhiteSpace(_) => {}
                token => return Ok(token),
            }
        }
    }

    fn unexpected(&self, token: &Token<'i>) -> BasicParseError<'i> {
        let location = self.location_at(
            self.input
                .tokens
                .get(self.pos.wrapping_sub(1))
                .map_or(self.cursor, |tok| tok.start),
        );
        BasicParseError {
            kind: BasicParseErrorKind::UnexpectedToken(token.clone()),
            location,
        }
    }

    pub(crate) fn expect_ident_cloned(&mut self) -> Result<Cow<'i, str>, BasicParseError<'i>> {
        match self.next()? {
            Token::Ident(name) => Ok(name.clone()),
            token => Err(self.unexpected(token)),
        }
    }

    pub(crate) fn expect_string_cloned(&mut self) -> Result<Cow<'i, str>, BasicParseError<'i>> {
        match self.next()? {
            Token::QuotedString(value) => Ok(value.clone()),
            token => Err(self.unexpected(token)),
        }
    }

    pub(crate) fn expect_colon(&mut self) -> Result<(), BasicParseError<'i>> {
        match self.next()? {
            Token::Colon => Ok(()),
            token => Err(self.unexpected(token)),
        }
    }

    pub(crate) fn expect_semicolon(&mut self) -> Result<(), BasicParseError<'i>> {
        match self.next()? {
            Token::Semicolon => Ok(()),
            token => Err(self.unexpected(token)),
        }
    }

    /// Succeeds without consuming anything when only whitespace remains.
    pub(crate) fn expect_exhausted(&mut self) -> Result<(), BasicParseError<'i>> {
        let state = self.state();
        let result = match self.next() {
            Err(BasicParseError {
                kind: BasicParseErrorKind::EndOfInput,
                ..
            }) => Ok(()),
            Err(error) => Err(error),
            Ok(token) => Err(self.unexpected(token)),
        };
        self.reset(state);
        result
    }

    pub(crate) fn is_exhausted(&mut self) -> bool {
        self.expect_exhausted().is_ok()
    }

    /// Runs `parse`; on error the parser is restored to where it was.
    pub(crate) fn try_parse<F, T, E>(&mut self, parse: F) -> Result<T, E>
    where
        F: FnOnce(&mut Self) -> Result<T, E>,
    {
        let state = self.state();
        let result = parse(self);
        if result.is_err() {
            self.reset(state);
        }
        result
    }

    fn parse_entirely<F, T, E>(&mut self, parse: F) -> Result<T, ParseError<'i, E>>
    where
        F: FnOnce(&mut Self) -> Result<T, ParseError<'i, E>>,
    {
        let value = parse(self)?;
        self.expect_exhausted()?;
        Ok(value)
    }

    /// Parses the contents of the block opened by the token returned last. The block's closing
    /// token is consumed whether or not `parse` read all of it.
    pub(crate) fn parse_nested_block<F, T, E>(&mut self, parse: F) -> Result<T, ParseError<'i, E>>
    where
        F: FnOnce(&mut Self) -> Result<T, ParseError<'i, E>>,
    {
        let opener = self
            .pending
            .take()
            .expect("parse_nested_block must follow a block-opening token");
        let tokens = &self.input.tokens;
        let close = self.input.close[opener];
        let mut nested = Parser {
            input: self.input,
            pos: opener + 1,
            end: close.min(self.end),
            cursor: tokens[opener].end,
            pending: None,
        };
        let result = nested.parse_entirely(parse);
        self.pos = (close + 1).min(self.end);
        self.cursor = tokens.get(close).map_or(self.input.src.len(), |tok| tok.end);
        result
    }

    /// The index of the first top-level token that is one of `delimiters`, or `self.end`.
    fn find_delimiter(&self, delimiters: Delimiters) -> usize {
        let mut index = self.pos;
        while index < self.end {
            let token = &self.input.tokens[index].token;
            if delimiters.contains(token) {
                return index;
            }
            index = if token.opens_block() {
                (self.input.close[index] + 1).min(self.end)
            } else {
                index + 1
            };
        }
        self.end
    }

    fn parse_until_before_any<F, T, E>(
        &mut self,
        delimiters: Delimiters,
        parse: F,
    ) -> Result<T, ParseError<'i, E>>
    where
        F: FnOnce(&mut Self) -> Result<T, ParseError<'i, E>>,
    {
        self.skip_pending_block();
        let stop = self.find_delimiter(delimiters);
        let mut delimited = Parser {
            input: self.input,
            pos: self.pos,
            end: stop,
            cursor: self.cursor,
            pending: None,
        };
        let result = delimited.parse_entirely(parse);
        self.pos = stop;
        self.cursor = if stop < self.end {
            self.input.tokens[stop].start
        } else {
            self.limit_offset()
        };
        result
    }

    /// Parses the tokens up to (not including) the next top-level `delimiter`.
    pub(crate) fn parse_until_before<F, T, E>(
        &mut self,
        delimiter: Delimiter,
        parse: F,
    ) -> Result<T, ParseError<'i, E>>
    where
        F: FnOnce(&mut Self) -> Result<T, ParseError<'i, E>>,
    {
        self.parse_until_before_any(delimiter.into(), parse)
    }

    /// Parses the tokens up to the next top-level `delimiter` and then consumes the delimiter
    /// (with the block, for a curly bracket block), whatever `parse` returned.
    pub(crate) fn parse_until_after<F, T, E>(
        &mut self,
        delimiter: Delimiter,
        parse: F,
    ) -> Result<T, ParseError<'i, E>>
    where
        F: FnOnce(&mut Self) -> Result<T, ParseError<'i, E>>,
    {
        let result = self.parse_until_before(delimiter, parse);
        if self.pos < self.end {
            let tok = &self.input.tokens[self.pos];
            if tok.token.opens_block() {
                self.pending = Some(self.pos);
                self.pos += 1;
                self.cursor = tok.end;
                self.skip_pending_block();
            } else {
                self.pos += 1;
                self.cursor = tok.end;
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------------------------
// Rule lists
// ---------------------------------------------------------------------------------------------

/// Parses the prelude and block of at-rules ("consume an at-rule", CSS Syntax 5.4.2).
pub(crate) trait AtRuleParser<'i> {
    type Prelude;
    type AtRule;
    type Error;

    fn parse_prelude<'t>(
        &mut self,
        name: Cow<'i, str>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>>;

    /// An at-rule that ends in `;` or at the end of input. `Err` rejects the rule.
    fn rule_without_block(&mut self, prelude: Self::Prelude) -> Result<Self::AtRule, ()>;

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, Self::Error>>;
}

/// Parses the prelude and block of qualified rules ("consume a qualified rule", 5.4.3).
pub(crate) trait QualifiedRuleParser<'i> {
    type Prelude;
    type QualifiedRule;
    type Error;

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>>;

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, Self::Error>>;
}

/// An iterator over the rules of a stylesheet or rule list ("consume a list of rules", 5.4.1).
/// A rule that fails to parse is reported with its source text and skipped as a whole.
pub(crate) struct StyleSheetParser<'a, 'i, 't, P> {
    input: &'a mut Parser<'i, 't>,
    parser: &'a mut P,
}

impl<'a, 'i, 't, P> StyleSheetParser<'a, 'i, 't, P> {
    pub(crate) fn new(input: &'a mut Parser<'i, 't>, parser: &'a mut P) -> Self {
        Self { input, parser }
    }
}

type RuleResult<'i, R, E> = Result<R, (ParseError<'i, E>, &'i str)>;

impl<'i, 't, R, E, P> Iterator for StyleSheetParser<'_, 'i, 't, P>
where
    P: AtRuleParser<'i, AtRule = R, Error = E> + QualifiedRuleParser<'i, QualifiedRule = R, Error = E>,
{
    type Item = RuleResult<'i, R, E>;

    fn next(&mut self) -> Option<Self::Item> {
        let input = &mut *self.input;
        // Whitespace, CDO and CDC between top-level rules are ignored.
        let at_keyword = loop {
            let state = input.state();
            match input.next_including_whitespace() {
                Ok(Token::WhiteSpace(_) | Token::CDO | Token::CDC) => {}
                Ok(Token::AtKeyword(name)) => {
                    let name = name.clone();
                    input.reset(state);
                    break Some(name);
                }
                Ok(_) => {
                    input.reset(state);
                    break None;
                }
                Err(_) => return None,
            }
        };
        let start = input.position();
        let result = match at_keyword {
            Some(name) => {
                // Consume the at-keyword token itself.
                let _ = input.next_including_whitespace();
                parse_at_rule(name, input, self.parser)
            }
            None => parse_qualified_rule(input, self.parser),
        };
        // A rejected rule is reported with its whole source text, block included.
        input.skip_pending_block();
        Some(result.map_err(|error| (error, input.slice_from(start))))
    }
}

fn parse_at_rule<'i, 't, R, E, P>(
    name: Cow<'i, str>,
    input: &mut Parser<'i, 't>,
    parser: &mut P,
) -> Result<R, ParseError<'i, E>>
where
    P: AtRuleParser<'i, AtRule = R, Error = E>,
{
    let delimiters = Delimiters {
        semicolon: true,
        curly: true,
    };
    let prelude = input.parse_until_before_any(delimiters, |input| parser.parse_prelude(name, input));
    match prelude {
        Ok(prelude) => match input.next() {
            Ok(Token::CurlyBracketBlock) => {
                input.parse_nested_block(|input| parser.parse_block(prelude, input))
            }
            Ok(Token::Semicolon) | Err(_) => parser
                .rule_without_block(prelude)
                .map_err(|()| input.new_basic_error(BasicParseErrorKind::UnexpectedToken(Token::Semicolon)).into()),
            Ok(token) => Err(input.unexpected(token).into()),
        },
        Err(error) => {
            // Skip the rest of the rule, including its block.
            loop {
                match input.next() {
                    Ok(Token::Semicolon | Token::CurlyBracketBlock) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            Err(error)
        }
    }
}

fn parse_qualified_rule<'i, 't, R, E, P>(
    input: &mut Parser<'i, 't>,
    parser: &mut P,
) -> Result<R, ParseError<'i, E>>
where
    P: QualifiedRuleParser<'i, QualifiedRule = R, Error = E>,
{
    let prelude = input.parse_until_before(Delimiter::CurlyBracketBlock, |input| {
        parser.parse_prelude(input)
    });
    match input.next() {
        Ok(Token::CurlyBracketBlock) => {
            // The block is consumed even when the prelude was rejected.
            let prelude = prelude?;
            input.parse_nested_block(|input| parser.parse_block(prelude, input))
        }
        Ok(token) => Err(input.unexpected(token).into()),
        Err(_) => {
            prelude?;
            Err(input
                .new_basic_error(BasicParseErrorKind::QualifiedRuleInvalid)
                .into())
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Named colors
// ---------------------------------------------------------------------------------------------

/// The `<named-color>` keywords of CSS Color Module Level 4, section 6.1 (without
/// `transparent` and `currentcolor`), sorted for binary search.
const NAMED_COLORS: &[&str] = &[
    "aliceblue",
    "antiquewhite",
    "aqua",
    "aquamarine",
    "azure",
    "beige",
    "bisque",
    "black",
    "blanchedalmond",
    "blue",
    "blueviolet",
    "brown",
    "burlywood",
    "cadetblue",
    "chartreuse",
    "chocolate",
    "coral",
    "cornflowerblue",
    "cornsilk",
    "crimson",
    "cyan",
    "darkblue",
    "darkcyan",
    "darkgoldenrod",
    "darkgray",
    "darkgreen",
    "darkgrey",
    "darkkhaki",
    "darkmagenta",
    "darkolivegreen",
    "darkorange",
    "darkorchid",
    "darkred",
    "darksalmon",
    "darkseagreen",
    "darkslateblue",
    "darkslategray",
    "darkslategrey",
    "darkturquoise",
    "darkviolet",
    "deeppink",
    "deepskyblue",
    "dimgray",
    "dimgrey",
    "dodgerblue",
    "firebrick",
    "floralwhite",
    "forestgreen",
    "fuchsia",
    "gainsboro",
    "ghostwhite",
    "gold",
    "goldenrod",
    "gray",
    "green",
    "greenyellow",
    "grey",
    "honeydew",
    "hotpink",
    "indianred",
    "indigo",
    "ivory",
    "khaki",
    "lavender",
    "lavenderblush",
    "lawngreen",
    "lemonchiffon",
    "lightblue",
    "lightcoral",
    "lightcyan",
    "lightgoldenrodyellow",
    "lightgray",
    "lightgreen",
    "lightgrey",
    "lightpink",
    "lightsalmon",
    "lightseagreen",
    "lightskyblue",
    "lightslategray",
    "lightslategrey",
    "lightsteelblue",
    "lightyellow",
    "lime",
    "limegreen",
    "linen",
    "magenta",
    "maroon",
    "mediumaquamarine",
    "mediumblue",
    "mediumorchid",
    "mediumpurple",
    "mediumseagreen",
    "mediumslateblue",
    "mediumspringgreen",
    "mediumturquoise",
    "mediumvioletred",
    "midnightblue",
    "mintcream",
    "mistyrose",
    "moccasin",
    "navajowhite",
    "navy",
    "oldlace",
    "olive",
    "olivedrab",
    "orange",
    "orangered",
    "orchid",
    "palegoldenrod",
    "palegreen",
    "paleturquoise",
    "palevioletred",
    "papayawhip",
    "peachpuff",
    "peru",
    "pink",
    "plum",
    "powderblue",
    "purple",
    "rebeccapurple",
    "red",
    "rosybrown",
    "royalblue",
    "saddlebrown",
    "salmon",
    "sandybrown",
    "seagreen",
    "seashell",
    "sienna",
    "silver",
    "skyblue",
    "slateblue",
    "slategray",
    "slategrey",
    "snow",
    "springgreen",
    "steelblue",
    "tan",
    "teal",
    "thistle",
    "tomato",
    "turquoise",
    "violet",
    "wheat",
    "white",
    "whitesmoke",
    "yellow",
    "yellowgreen",
];

/// Returns true when `ident` is a CSS named color keyword (ASCII case-insensitive).
pub(crate) fn is_named_color(ident: &str) -> bool {
    if ident.len() > "lightgoldenrodyellow".len() || !ident.is_ascii() {
        return false;
    }
    let lower = ident.to_ascii_lowercase();
    NAMED_COLORS.binary_search(&lower.as_str()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(src: &str) -> Vec<Token<'_>> {
        tokenize(src).into_iter().map(|tok| tok.token).collect()
    }

    #[test]
    fn tokenizes_identifiers_numbers_and_escapes() {
        assert_eq!(
            tokens(r"a\6f b \31 x"),
            [
                Token::Ident("aob".into()),
                Token::WhiteSpace(" "),
                Token::Ident("1x".into()),
            ]
        );
        assert_eq!(
            tokens("-1.5e2px +3 .5% 4e"),
            [
                Token::Dimension {
                    has_sign: true,
                    value: -150.0,
                    int_value: None,
                    unit: "px".into()
                },
                Token::WhiteSpace(" "),
                Token::Number {
                    has_sign: true,
                    value: 3.0,
                    int_value: Some(3)
                },
                Token::WhiteSpace(" "),
                Token::Percentage {
                    has_sign: false,
                    value: 0.5,
                    int_value: None
                },
                Token::WhiteSpace(" "),
                Token::Dimension {
                    has_sign: false,
                    value: 4.0,
                    int_value: Some(4),
                    unit: "e".into()
                },
            ]
        );
        assert_eq!(
            tokens("90deg-foo"),
            [Token::Dimension {
                has_sign: false,
                value: 90.0,
                int_value: Some(90),
                unit: "deg-foo".into()
            }]
        );
    }

    #[test]
    fn tokenizes_strings_urls_and_bad_tokens() {
        assert_eq!(tokens(r#""a\"b""#), [Token::QuotedString("a\"b".into())]);
        assert_eq!(
            tokens("'a\nb"),
            [
                Token::BadString("a".into()),
                Token::WhiteSpace("\n"),
                Token::Ident("b".into())
            ]
        );
        assert_eq!(tokens("url( a.png )"), [Token::UnquotedUrl("a.png".into())]);
        assert_eq!(tokens("URL(\"a\")")[0], Token::Function("URL".into()));
        assert_eq!(tokens("url( 'a')")[0], Token::Function("url".into()));
        assert_eq!(tokens("u\\72l(a)"), [Token::UnquotedUrl("a".into())]);
        assert_eq!(tokens("url(a b)"), [Token::BadUrl("a".into())]);
        assert_eq!(tokens("url(a'b)"), [Token::BadUrl("a".into())]);
        assert_eq!(tokens("url(a"), [Token::UnquotedUrl("a".into())]);
        assert_eq!(tokens("f(")[0], Token::Function("f".into()));
    }

    #[test]
    fn tokenizes_at_keywords_hashes_delims_and_comments() {
        assert_eq!(
            tokens("@media #a #1 /* c */ <!-- --> ~= \\\n"),
            [
                Token::AtKeyword("media".into()),
                Token::WhiteSpace(" "),
                Token::IDHash("a".into()),
                Token::WhiteSpace(" "),
                Token::Hash("1".into()),
                Token::WhiteSpace(" "),
                Token::WhiteSpace(" "),
                Token::CDO,
                Token::WhiteSpace(" "),
                Token::CDC,
                Token::WhiteSpace(" "),
                Token::Delim('~'),
                Token::Delim('='),
                Token::WhiteSpace(" "),
                Token::Delim('\\'),
                Token::WhiteSpace("\n"),
            ]
        );
    }

    #[test]
    fn blocks_match_their_own_closers_only() {
        let input = ParserInput::new("a( [ ) ] ) b");
        let mut parser = Parser::new(&input);
        assert_eq!(parser.next().unwrap(), &Token::Function("a".into()));
        let inner = parser
            .parse_nested_block(|nested| {
                let mut seen = Vec::new();
                while let Ok(token) = nested.next() {
                    seen.push(format!("{token:?}"));
                }
                Ok::<_, ParseError<'_, ()>>(seen)
            })
            .unwrap();
        // The `)` inside `[ ... ]` does not close the function; the square block swallows it.
        assert_eq!(inner, ["SquareBracketBlock"]);
        assert_eq!(parser.next().unwrap(), &Token::Ident("b".into()));
        assert!(parser.next().is_err());
    }

    #[test]
    fn parse_until_after_consumes_the_delimiter_and_skips_blocks() {
        let input = ParserInput::new("a:f(1;2);b:{x;y};c");
        let mut parser = Parser::new(&input);
        let mut declarations = Vec::new();
        while !parser.is_exhausted() {
            let declaration = parser
                .parse_until_after(Delimiter::Semicolon, |declaration| {
                    let start = declaration.position();
                    while declaration.next_including_whitespace().is_ok() {}
                    Ok::<_, ParseError<'_, ()>>(declaration.slice_from(start).to_string())
                })
                .unwrap();
            declarations.push(declaration);
        }
        assert_eq!(declarations, ["a:f(1;2)", "b:{x;y}", "c"]);
    }

    #[test]
    fn rule_lists_recover_per_rule() {
        struct Collect;
        impl<'i> AtRuleParser<'i> for Collect {
            type Prelude = String;
            type AtRule = String;
            type Error = ();
            fn parse_prelude<'t>(
                &mut self,
                name: Cow<'i, str>,
                input: &mut Parser<'i, 't>,
            ) -> Result<String, ParseError<'i, ()>> {
                while input.next_including_whitespace().is_ok() {}
                Ok(name.to_string())
            }
            fn rule_without_block(&mut self, prelude: String) -> Result<String, ()> {
                Ok(format!("@{prelude};"))
            }
            fn parse_block<'t>(
                &mut self,
                prelude: String,
                input: &mut Parser<'i, 't>,
            ) -> Result<String, ParseError<'i, ()>> {
                while input.next_including_whitespace().is_ok() {}
                Ok(format!("@{prelude}{{}}"))
            }
        }
        impl<'i> QualifiedRuleParser<'i> for Collect {
            type Prelude = ();
            type QualifiedRule = String;
            type Error = ();
            fn parse_prelude<'t>(
                &mut self,
                input: &mut Parser<'i, 't>,
            ) -> Result<(), ParseError<'i, ()>> {
                let mut has_ident = false;
                while let Ok(token) = input.next() {
                    has_ident |= matches!(token, Token::Ident(_));
                }
                if has_ident {
                    Ok(())
                } else {
                    Err(input.new_custom_error(()))
                }
            }
            fn parse_block<'t>(
                &mut self,
                _prelude: (),
                input: &mut Parser<'i, 't>,
            ) -> Result<String, ParseError<'i, ()>> {
                while input.next_including_whitespace().is_ok() {}
                Ok("rule".to_string())
            }
        }

        let input = ParserInput::new("<!-- a{x} 1{y} @m x{a{b}} @i url(x); b{ c } d");
        let mut parser = Parser::new(&input);
        let mut collect = Collect;
        let out: Vec<_> = StyleSheetParser::new(&mut parser, &mut collect)
            .map(|rule| rule.map_err(|(_, text)| text.to_string()))
            .collect();
        assert_eq!(
            out,
            [
                Ok("rule".to_string()),
                Err("1{y}".to_string()),
                Ok("@m{}".to_string()),
                Ok("@i;".to_string()),
                Ok("rule".to_string()),
                Err("d".to_string()),
            ]
        );
    }

    #[test]
    fn number_serialization_matches_css_serialization() {
        assert_eq!(number_to_css(45.0, Some(45), false), "45");
        assert_eq!(number_to_css(0.5, None, false), "0.5");
        assert_eq!(number_to_css(-10.5, None, true), "-10.5");
        assert_eq!(number_to_css(3.0, Some(3), true), "+3");
        assert_eq!(number_to_css(45.0, None, false), "45.0");
        assert_eq!(number_to_css(-0.0, None, true), "-0.0");
    }

    #[test]
    fn named_color_table_is_sorted_and_case_insensitive() {
        assert!(NAMED_COLORS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(NAMED_COLORS.len(), 148);
        assert!(is_named_color("red"));
        assert!(is_named_color("RebeccaPurple"));
        assert!(!is_named_color("transparent"));
        assert!(!is_named_color("redd"));
        assert!(!is_named_color("réd"));
    }
}
