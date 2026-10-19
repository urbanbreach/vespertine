//! Integer expressions of `#if` and `#elif`.

use crate::lexer::{Raw, RawToken};
use crate::token::{PreprocessorError, Punct, TokenValue};

struct Parser<'a> {
    tokens: &'a [RawToken],
    position: usize,
}

type Result<T> = std::result::Result<T, PreprocessorError>;

fn binary_precedence(punct: Punct) -> Option<u8> {
    use Punct::*;
    Some(match punct {
        LogicalOr => 1,
        LogicalAnd => 2,
        Pipe => 3,
        Caret => 4,
        Ampersand => 5,
        EqualEqual | NotEqual => 6,
        LeftAngle | RightAngle | LessEqual | GreaterEqual => 7,
        LeftShift | RightShift => 8,
        Plus | Minus => 9,
        Star | Slash | Percent => 10,
        _ => return None,
    })
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Raw> {
        self.tokens.get(self.position).map(|t| &t.kind)
    }

    fn unary(&mut self) -> Result<i64> {
        let token = self
            .tokens
            .get(self.position)
            .ok_or(PreprocessorError::UnexpectedNewLine)?;
        self.position += 1;
        match &token.kind {
            Raw::Int(integer) => Ok(integer.value as i64),
            // An identifier left after macro expansion is an undefined macro: zero.
            Raw::Ident(_) => Ok(0),
            Raw::Punct(Punct::Minus) => Ok(self.unary()?.wrapping_neg()),
            Raw::Punct(Punct::Plus) => self.unary(),
            Raw::Punct(Punct::Bang) => Ok((self.unary()? == 0) as i64),
            Raw::Punct(Punct::Tilde) => Ok(!self.unary()?),
            Raw::Punct(Punct::LeftParen) => {
                let value = self.binary(0)?;
                match self.peek() {
                    Some(Raw::Punct(Punct::RightParen)) => {
                        self.position += 1;
                        Ok(value)
                    }
                    _ => Err(PreprocessorError::InvalidExpression),
                }
            }
            Raw::Float(_) => Err(PreprocessorError::InvalidExpression),
            Raw::Error(error) => Err(error.clone()),
            other => Err(PreprocessorError::UnexpectedToken(match other {
                Raw::Punct(p) => TokenValue::Punct(*p),
                _ => TokenValue::Ident(String::new()),
            })),
        }
    }

    fn binary(&mut self, min: u8) -> Result<i64> {
        let mut left = self.unary()?;
        loop {
            let Some(Raw::Punct(op)) = self.peek().cloned() else {
                return Ok(left);
            };
            let Some(precedence) = binary_precedence(op) else {
                return Ok(left);
            };
            if precedence < min.max(1) {
                return Ok(left);
            }
            self.position += 1;
            let right = self.binary(precedence + 1)?;
            left = apply(op, left, right)?;
        }
    }
}

fn apply(op: Punct, a: i64, b: i64) -> Result<i64> {
    use Punct::*;
    Ok(match op {
        LogicalOr => (a != 0 || b != 0) as i64,
        LogicalAnd => (a != 0 && b != 0) as i64,
        Pipe => a | b,
        Caret => a ^ b,
        Ampersand => a & b,
        EqualEqual => (a == b) as i64,
        NotEqual => (a != b) as i64,
        LeftAngle => (a < b) as i64,
        RightAngle => (a > b) as i64,
        LessEqual => (a <= b) as i64,
        GreaterEqual => (a >= b) as i64,
        LeftShift => a.wrapping_shl(b as u32),
        RightShift => a.wrapping_shr(b as u32),
        Plus => a.wrapping_add(b),
        Minus => a.wrapping_sub(b),
        Star => a.wrapping_mul(b),
        Slash => {
            if b == 0 {
                return Err(PreprocessorError::DivisionByZero);
            }
            a.wrapping_div(b)
        }
        Percent => {
            if b == 0 {
                return Err(PreprocessorError::DivisionByZero);
            }
            a.wrapping_rem(b)
        }
        _ => return Err(PreprocessorError::InvalidExpression),
    })
}

/// Evaluates an expression whose macros are already expanded.
pub(crate) fn evaluate(tokens: &[RawToken]) -> Result<i64> {
    let mut parser = Parser {
        tokens,
        position: 0,
    };
    let value = parser.binary(0)?;
    if parser.position != tokens.len() {
        return Err(PreprocessorError::InvalidExpression);
    }
    Ok(value)
}
