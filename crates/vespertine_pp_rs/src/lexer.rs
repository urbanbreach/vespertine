//! Splitting text into raw tokens: comments and line continuations are dropped, newlines kept.

use crate::token::{Float, Integer, Location, PreprocessorError, Punct};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Raw {
    Ident(String),
    Int(Integer),
    Float(Float),
    Punct(Punct),
    Hash,
    Newline,
    Error(PreprocessorError),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawToken {
    pub kind: Raw,
    pub loc: Location,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Index after any run of whitespace, line continuations and comments starting at `i`; a
/// comment that is never closed is reported through `error`.
fn skip_blank(b: &[u8], mut i: usize, error: &mut Option<(usize, usize)>) -> usize {
    loop {
        match b.get(i) {
            Some(b' ' | b'\t' | b'\r' | 0x0b | 0x0c) => i += 1,
            Some(b'\\') if matches!(b.get(i + 1), Some(b'\n')) => i += 2,
            Some(b'\\') if matches!((b.get(i + 1), b.get(i + 2)), (Some(b'\r'), Some(b'\n'))) => {
                i += 3
            }
            Some(b'/') if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            Some(b'/') if b.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                loop {
                    match (b.get(i), b.get(i + 1)) {
                        (Some(b'*'), Some(b'/')) => {
                            i += 2;
                            break;
                        }
                        (Some(_), _) => i += 1,
                        (None, _) => {
                            *error = Some((start, b.len()));
                            return b.len();
                        }
                    }
                }
            }
            _ => return i,
        }
    }
}

fn number(text: &str, loc: Location) -> Raw {
    let lower = text.to_ascii_lowercase();
    let is_hex = lower.starts_with("0x");
    let is_float = !is_hex
        && (lower.contains('.')
            || lower.contains('e')
            || lower.ends_with('f')
            || lower.ends_with("lf"));
    let _ = loc;
    if is_float {
        let (body, width) = if let Some(body) = lower.strip_suffix("lf") {
            (body, 64)
        } else if let Some(body) = lower.strip_suffix('f') {
            (body, 32)
        } else {
            (lower.as_str(), 32)
        };
        return match body.parse::<f32>() {
            Ok(value) => Raw::Float(Float { value, width }),
            Err(_) => Raw::Error(PreprocessorError::FloatParsingError),
        };
    }
    let (digits, signed) = match lower.strip_suffix('u') {
        Some(rest) => (rest, false),
        None => (lower.as_str(), true),
    };
    let parsed = if let Some(hex) = digits.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else if digits.len() > 1 && digits.starts_with('0') {
        u64::from_str_radix(&digits[1..], 8)
    } else {
        digits.parse::<u64>()
    };
    match parsed {
        Ok(value) if value <= u32::MAX as u64 => Raw::Int(Integer {
            value,
            signed,
            width: 32,
        }),
        Ok(_) => Raw::Error(PreprocessorError::IntegerOverflow),
        Err(error) => match error.kind() {
            std::num::IntErrorKind::PosOverflow => Raw::Error(PreprocessorError::IntegerOverflow),
            _ => Raw::Error(PreprocessorError::IntegerParsingError),
        },
    }
}

pub(crate) fn lex(input: &str) -> Vec<RawToken> {
    let b = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut unterminated = None;
    while i < b.len() {
        i = skip_blank(b, i, &mut unterminated);
        if let Some((start, end)) = unterminated.take() {
            out.push(RawToken {
                kind: Raw::Error(PreprocessorError::UnexpectedEndOfInput),
                loc: Location::new(start, end),
            });
        }
        let Some(&c) = b.get(i) else { break };
        let start = i;
        let token = |kind: Raw, end: usize| RawToken {
            kind,
            loc: Location::new(start, end),
        };
        if c == b'\n' {
            i += 1;
            out.push(token(Raw::Newline, i));
        } else if is_ident_start(c) {
            while i < b.len() && is_ident_continue(b[i]) {
                i += 1;
            }
            out.push(token(Raw::Ident(input[start..i].to_owned()), i));
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            // A number: digits, an optional fraction and exponent, then suffix letters.
            let hex = c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X'));
            if hex {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if b.get(i) == Some(&b'.') {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if matches!(b.get(i), Some(b'e' | b'E')) {
                    let mut j = i + 1;
                    if matches!(b.get(j), Some(b'+' | b'-')) {
                        j += 1;
                    }
                    if b.get(j).is_some_and(u8::is_ascii_digit) {
                        while j < b.len() && b[j].is_ascii_digit() {
                            j += 1;
                        }
                        i = j;
                    }
                }
            }
            while i < b.len() && matches!(b[i], b'u' | b'U' | b'f' | b'F' | b'l' | b'L') {
                i += 1;
            }
            let loc = Location::new(start, i);
            out.push(token(number(&input[start..i], loc), i));
        } else if c == b'#' {
            i += 1;
            out.push(token(Raw::Hash, i));
        } else if let Some((punct, len)) = punct(&b[i..]) {
            i += len;
            out.push(token(Raw::Punct(punct), i));
        } else {
            // Skip one whole character.
            i += input[i..].chars().next().map_or(1, char::len_utf8);
            out.push(token(Raw::Error(PreprocessorError::UnexpectedCharacter), i));
        }
    }
    out
}

fn punct(b: &[u8]) -> Option<(Punct, usize)> {
    use Punct::*;
    let three = |s: &[u8; 3]| b.len() >= 3 && &b[..3] == s;
    let two = |s: &[u8; 2]| b.len() >= 2 && &b[..2] == s;
    if three(b"<<=") {
        return Some((LeftShiftAssign, 3));
    }
    if three(b">>=") {
        return Some((RightShiftAssign, 3));
    }
    let table2: [(&[u8; 2], Punct); 16] = [
        (b"+=", AddAssign),
        (b"-=", SubAssign),
        (b"*=", MulAssign),
        (b"/=", DivAssign),
        (b"%=", ModAssign),
        (b"&=", AndAssign),
        (b"^=", XorAssign),
        (b"|=", OrAssign),
        (b"++", Increment),
        (b"--", Decrement),
        (b"&&", LogicalAnd),
        (b"||", LogicalOr),
        (b"^^", LogicalXor),
        (b"<=", LessEqual),
        (b">=", GreaterEqual),
        (b"==", EqualEqual),
    ];
    for (text, punct) in table2 {
        if two(text) {
            return Some((punct, 2));
        }
    }
    for (text, punct) in [(b"!=", NotEqual), (b"<<", LeftShift), (b">>", RightShift)] {
        if two(text) {
            return Some((punct, 2));
        }
    }
    let punct = match b[0] {
        b'{' => LeftBrace,
        b'}' => RightBrace,
        b'(' => LeftParen,
        b')' => RightParen,
        b'[' => LeftBracket,
        b']' => RightBracket,
        b'<' => LeftAngle,
        b'>' => RightAngle,
        b';' => Semicolon,
        b',' => Comma,
        b':' => Colon,
        b'.' => Dot,
        b'=' => Equal,
        b'!' => Bang,
        b'-' => Minus,
        b'~' => Tilde,
        b'+' => Plus,
        b'*' => Star,
        b'/' => Slash,
        b'%' => Percent,
        b'|' => Pipe,
        b'^' => Caret,
        b'&' => Ampersand,
        b'?' => Question,
        _ => return None,
    };
    Some((punct, 1))
}
