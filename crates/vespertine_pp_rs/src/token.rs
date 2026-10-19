//! Tokens produced by the preprocessor.

/// A byte range of the preprocessed text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Location {
    /// Offset of the first byte.
    pub start: u32,
    /// Offset one past the last byte.
    pub end: u32,
}

impl Location {
    pub(crate) fn new(start: usize, end: usize) -> Self {
        Self {
            start: start as u32,
            end: end as u32,
        }
    }

    pub(crate) fn join(self, other: Location) -> Location {
        Location {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// An integer literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Integer {
    /// The value.
    pub value: u64,
    /// Whether the literal is signed (no `u` suffix).
    pub signed: bool,
    /// Width in bits.
    pub width: i32,
}

/// A floating point literal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Float {
    /// The value.
    pub value: f32,
    /// Width in bits.
    pub width: i32,
}

/// Punctuation and operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Punct {
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    LeftShiftAssign,
    RightShiftAssign,
    AndAssign,
    XorAssign,
    OrAssign,
    Increment,
    Decrement,
    LogicalAnd,
    LogicalOr,
    LogicalXor,
    LessEqual,
    GreaterEqual,
    EqualEqual,
    NotEqual,
    LeftShift,
    RightShift,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftAngle,
    RightAngle,
    Semicolon,
    Comma,
    Colon,
    Dot,
    Equal,
    Bang,
    Minus,
    Tilde,
    Plus,
    Star,
    Slash,
    Percent,
    Pipe,
    Caret,
    Ampersand,
    Question,
}

/// A `#version` directive.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    /// The tokens after `version`.
    pub tokens: Vec<Token>,
    /// Whether it is the first directive of the text.
    pub is_first_directive: bool,
    /// Whether a comment precedes it.
    pub has_comments_before: bool,
}

/// An `#extension` directive.
#[derive(Clone, Debug, PartialEq)]
pub struct Extension {
    /// The tokens after `extension`.
    pub tokens: Vec<Token>,
    /// Whether a comment precedes it.
    pub has_comments_before: bool,
}

/// A `#pragma` directive.
#[derive(Clone, Debug, PartialEq)]
pub struct Pragma {
    /// The tokens after `pragma`.
    pub tokens: Vec<Token>,
}

/// The value of a token.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenValue {
    /// An identifier or keyword.
    Ident(String),
    /// An integer literal.
    Integer(Integer),
    /// A floating point literal.
    Float(Float),
    /// Punctuation.
    Punct(Punct),
    /// A `#version` directive.
    Version(Version),
    /// An `#extension` directive.
    Extension(Extension),
    /// A `#pragma` directive.
    Pragma(Pragma),
}

/// A token with its location.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    /// The value.
    pub value: TokenValue,
    /// Where it came from.
    pub location: Location,
}

/// A preprocessing error.
#[derive(Clone, Debug, PartialEq)]
pub enum PreprocessorError {
    /// A character that cannot start a token.
    UnexpectedCharacter,
    /// A token where another was expected.
    UnexpectedToken(TokenValue),
    /// A `#` that does not start a directive.
    UnexpectedHash,
    /// The line ended where a token was expected.
    UnexpectedNewLine,
    /// The input ended inside a construct (an unterminated comment, macro call or conditional).
    UnexpectedEndOfInput,
    /// An unknown directive.
    UnknownDirective(String),
    /// An integer literal that does not fit.
    IntegerOverflow,
    /// A malformed floating point literal.
    FloatParsingError,
    /// A malformed integer literal.
    IntegerParsingError,
    /// `#else`, `#elif` or `#endif` without a matching `#if`.
    UnmatchedConditional,
    /// `#else` or `#elif` after `#else`.
    ConditionalAfterElse,
    /// A conditional without `#endif`.
    UnterminatedConditional,
    /// A macro called with the wrong number of arguments.
    WrongNumberOfArguments {
        /// The macro name.
        name: String,
        /// Parameters the macro takes.
        expected: usize,
        /// Arguments supplied.
        found: usize,
    },
    /// A macro parameter name used twice.
    DuplicateParameter(String),
    /// A redefinition of a reserved or predefined macro.
    ReservedMacro(String),
    /// Division or remainder by zero in a conditional expression.
    DivisionByZero,
    /// A malformed conditional expression.
    InvalidExpression,
    /// An `#error` directive; the text is the rest of the line.
    ErrorDirective(String),
}
