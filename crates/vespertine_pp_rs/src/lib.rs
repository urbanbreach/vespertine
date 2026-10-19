//! A GLSL preprocessor with the interface naga's GLSL front end uses (`pp_rs::pp::Preprocessor`
//! and `pp_rs::token`).
//!
//! The crates.io crate `pp-rs` is not available to this workspace, so the workspace patches that
//! name to this implementation. It supports what shader programs use: comments, line
//! continuations, `#version`, `#extension` and `#pragma` (handed to the parser as directive
//! tokens), `#define` / `#undef` (object-like and function-like macros), conditional compilation
//! (`#if`, `#ifdef`, `#ifndef`, `#elif`, `#else`, `#endif` with integer expressions and
//! `defined`), `#line`, `#error` and the predefined macros `__LINE__`, `__FILE__` and
//! `__VERSION__`. Every token carries the byte offsets of the original text; tokens produced by a
//! macro expansion carry the span of the invocation, so diagnostics point at the use.

pub mod pp;
pub mod token;

mod expr;
mod lexer;

#[cfg(test)]
mod tests {
    use crate::pp::Preprocessor;
    use crate::token::{Integer, Location, PreprocessorError, Punct, Token, TokenValue};

    fn tokens(source: &str) -> Vec<Result<Token, (PreprocessorError, Location)>> {
        Preprocessor::new(source).collect()
    }

    /// Renders the tokens as text, for compact assertions.
    fn text(source: &str) -> String {
        tokens(source)
            .into_iter()
            .map(|item| match item {
                Ok(Token { value, .. }) => match value {
                    TokenValue::Ident(name) => name,
                    TokenValue::Integer(i) => i.value.to_string(),
                    TokenValue::Float(f) => f.value.to_string(),
                    TokenValue::Punct(p) => format!("{p:?}"),
                    TokenValue::Version(_) => "VERSION".to_owned(),
                    TokenValue::Extension(_) => "EXTENSION".to_owned(),
                    TokenValue::Pragma(_) => "PRAGMA".to_owned(),
                },
                Err((error, _)) => format!("ERR({error:?})"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn version_and_plain_tokens_carry_byte_offsets() {
        let items = tokens("#version 450\nvoid main () {}");
        let Ok(Token {
            value: TokenValue::Version(version),
            location,
        }) = &items[0]
        else {
            panic!("{items:?}");
        };
        assert_eq!(*location, Location { start: 1, end: 8 });
        assert!(version.is_first_directive);
        assert_eq!(
            version.tokens,
            vec![Token {
                value: TokenValue::Integer(Integer {
                    value: 450,
                    signed: true,
                    width: 32
                }),
                location: Location { start: 9, end: 12 }
            }]
        );
        let void = items[1].as_ref().unwrap();
        assert_eq!(void.location, Location { start: 13, end: 17 });
        assert_eq!(items.len(), 7);
    }

    #[test]
    fn comments_and_continuations_vanish() {
        assert_eq!(text("a // x\nb /* y\nz */ c\nd \\\ne"), "a b c d e");
        assert_eq!(text("1.5 .25 3e2 7u 0x1F 010"), "1.5 0.25 300 7 31 8");
    }

    #[test]
    fn object_and_function_macros_expand_with_the_invocation_location() {
        assert_eq!(text("#define A 1 + 2\nA * A"), "1 Plus 2 Star 1 Plus 2");
        assert_eq!(
            text("#define ADD(x, y) ((x) + (y))\nADD(1, ADD(2, 3))"),
            "LeftParen LeftParen 1 RightParen Plus LeftParen LeftParen LeftParen 2 RightParen Plus LeftParen 3 RightParen RightParen RightParen RightParen"
        );
        // A function-like macro without arguments is an identifier; recursion stops.
        assert_eq!(text("#define F(x) x\nF + 1"), "F Plus 1");
        assert_eq!(text("#define A B\n#define B A\nA"), "A");
        let source = "#define NAME vsp[0].x\nvec4 t = NAME;";
        let items = tokens(source);
        let expanded: Vec<_> = items
            .iter()
            .filter_map(|i| i.as_ref().ok())
            .filter(|t| t.location == Location { start: 31, end: 35 })
            .collect();
        assert_eq!(
            expanded.len(),
            6,
            "every expanded token carries the use site: {items:?}"
        );
        assert_eq!(text("#define A 1\n#undef A\nA"), "A");
    }

    #[test]
    fn conditionals_select_branches() {
        let source = "#define X 2\n#if X > 1 && defined(X)\nyes\n#elif 1\nno\n#else\nnever\n#endif\n#ifdef Y\ny\n#else\nnoy\n#endif\n#ifndef Y\nz\n#endif";
        assert_eq!(text(source), "yes noy z");
        assert_eq!(text("#if 0\n#if 1\na\n#endif\n#else\nb\n#endif"), "b");
        assert_eq!(text("#if 0\n#elif 0\n#else\nc\n#endif"), "c");
        assert_eq!(text("#if 1\na\n#else\nb\n#endif"), "a");
    }

    #[test]
    fn predefined_macros_and_directives() {
        assert_eq!(
            text("#version 450\n\n__LINE__ __VERSION__"),
            "VERSION 3 450"
        );
        assert_eq!(
            text("#extension GL_x : enable\n#pragma optimize(on)"),
            "EXTENSION PRAGMA"
        );
        let mut pp = Preprocessor::new("FOO");
        pp.add_define("FOO", "7").unwrap();
        assert!(matches!(
            pp.next(),
            Some(Ok(Token {
                value: TokenValue::Integer(Integer { value: 7, .. }),
                ..
            }))
        ));
        assert!(Preprocessor::new("").add_define("1bad", "x").is_err());
    }

    #[test]
    fn errors_are_reported_with_locations() {
        assert!(text("#else").contains("UnmatchedConditional"));
        assert!(text("#if 1\nx").contains("UnterminatedConditional"));
        assert!(text("#bogus").contains("UnknownDirective"));
        assert!(text("a # b").contains("UnexpectedHash"));
        assert!(text("a @ b").contains("UnexpectedCharacter"));
        assert!(text("#define F(x) x\nF(1, 2)").contains("WrongNumberOfArguments"));
        assert!(text("#error stop here").contains("ErrorDirective(\"stop here\")"));
        assert!(text("/* never closed").contains("UnexpectedEndOfInput"));
        assert!(text("#if 1 / 0\n#endif").contains("DivisionByZero"));
        assert_eq!(text("a >>= b"), "a RightShiftAssign b");
        let _ = Punct::Dot;
    }
}
