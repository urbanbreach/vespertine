//! A span-preserving HTML tokenizer for the label sanitizer.
//!
//! The tokenizer follows the WHATWG HTML Standard, section 13.2.5 ("Tokenization"), for the
//! constructs that decide where one token ends and the next begins: tags with their attribute
//! syntax, comments, bogus comments, DOCTYPEs, CDATA sections, and the RCDATA, RAWTEXT, script
//! data (including its escaped and double-escaped sub-states) and PLAINTEXT text modes.
//!
//! It deliberately does not decode character references. Every token is reported as a byte range
//! of the source so the sanitizer can copy unmodified markup verbatim and decode only the
//! attribute values it has to inspect. Character references never contain `<`, `>`, quotes, or
//! whitespace, so skipping them cannot change where a token ends.
//!
//! The tree-construction stage drives the text mode and the CDATA switch (see
//! `html_simulator.rs`), exactly like the "tree construction dispatcher" drives a conforming
//! parser's tokenizer.

use std::ops::Range;

/// The tokenizer's text mode, switched by the tree-construction feedback after a start tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextMode {
    Data,
    RcData,
    RawText,
    ScriptData,
    PlainText,
}

/// What a [`Token::Text`] range contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextKind {
    /// Markup-free character data with raw (undecoded) character references.
    Data,
    /// A `<` that does not start any markup.
    StrayLessThan,
    /// Content of an RCDATA element (`title`, `textarea`).
    RcData,
    /// Content of a RAWTEXT element.
    RawText,
    /// Content of a `script` element.
    ScriptData,
    /// Everything after a `plaintext` start tag.
    PlainText,
    /// The characters of a CDATA section, without the delimiters.
    CData,
    /// A tag cut off by the end of input inside its name. A conforming parser discards it; the
    /// sanitizer passes the source text through unchanged, as the HTML rewriter it replaces did.
    /// Truncated tags that already have attribute syntax are discarded.
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttrSpan {
    pub(crate) name: Range<usize>,
    /// Name through the end of the value, including quotes and whitespace around `=`.
    pub(crate) raw: Range<usize>,
    /// The value without quotes; empty when the attribute has no value.
    pub(crate) value: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagSpan {
    pub(crate) span: Range<usize>,
    pub(crate) name: Range<usize>,
    pub(crate) attrs: Vec<AttrSpan>,
    pub(crate) self_closing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Token {
    Text { range: Range<usize>, kind: TextKind },
    StartTag(TagSpan),
    EndTag(TagSpan),
    Comment,
    Doctype,
}

pub(crate) struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    mode: TextMode,
    allow_cdata: bool,
    last_start_tag: String,
}

fn is_ws(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

fn is_tag_name_end(byte: u8) -> bool {
    is_ws(byte) || matches!(byte, b'/' | b'>')
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        Self {
            bytes: src.as_bytes(),
            pos: 0,
            mode: TextMode::Data,
            allow_cdata: false,
            last_start_tag: String::new(),
        }
    }

    /// Switches the text mode; `last_start_tag` is the lowercase name of the start tag that
    /// requested it (the only end tag that can leave RCDATA, RAWTEXT and script data).
    pub(crate) fn set_text_mode(&mut self, mode: TextMode, last_start_tag: &str) {
        self.mode = mode;
        if mode != TextMode::Data {
            self.last_start_tag.clear();
            self.last_start_tag.push_str(last_start_tag);
        }
    }

    pub(crate) fn set_allow_cdata(&mut self, allow: bool) {
        self.allow_cdata = allow;
    }

    /// Returns the next token, or `None` at the end of input. A tag or other construct cut off by
    /// the end of input is discarded, as a conforming parser does ("eof-in-tag").
    pub(crate) fn next_token(&mut self) -> Option<Token> {
        loop {
            if self.pos >= self.bytes.len() {
                return None;
            }
            let step = match self.mode {
                TextMode::Data => self.data_step(),
                TextMode::PlainText => {
                    let range = self.pos..self.bytes.len();
                    self.pos = self.bytes.len();
                    Step::Token(Token::Text {
                        range,
                        kind: TextKind::PlainText,
                    })
                }
                TextMode::RcData | TextMode::RawText | TextMode::ScriptData => self.text_mode_step(),
            };
            match step {
                Step::Token(token) => return Some(token),
                Step::Continue => {}
                Step::Eof => {
                    self.pos = self.bytes.len();
                    return None;
                }
            }
        }
    }

    fn data_step(&mut self) -> Step {
        let start = self.pos;
        if self.bytes[start] != b'<' {
            let end = self.bytes[start..]
                .iter()
                .position(|byte| *byte == b'<')
                .map_or(self.bytes.len(), |offset| start + offset);
            self.pos = end;
            return Step::Token(Token::Text {
                range: start..end,
                kind: TextKind::Data,
            });
        }

        let Some(&next) = self.bytes.get(start + 1) else {
            return self.stray_less_than();
        };
        match next {
            byte if byte.is_ascii_alphabetic() => match self.parse_tag(start, false) {
                Some(tag) => Step::Token(Token::StartTag(tag)),
                None => self.truncated_tag(start),
            },
            b'/' => match self.bytes.get(start + 2) {
                None => {
                    // `</` at the end of input is text.
                    self.pos = self.bytes.len();
                    Step::Token(Token::Text {
                        range: start..self.bytes.len(),
                        kind: TextKind::StrayLessThan,
                    })
                }
                Some(byte) if byte.is_ascii_alphabetic() => match self.parse_tag(start, true) {
                    Some(tag) => Step::Token(Token::EndTag(tag)),
                    None => self.truncated_tag(start),
                },
                Some(b'>') => {
                    // `</>` produces no token.
                    self.pos = start + 3;
                    Step::Continue
                }
                Some(_) => self.bogus_comment(start + 2),
            },
            b'!' => self.markup_declaration(start),
            b'?' => self.bogus_comment(start + 1),
            _ => self.stray_less_than(),
        }
    }

    fn stray_less_than(&mut self) -> Step {
        let start = self.pos;
        self.pos = start + 1;
        Step::Token(Token::Text {
            range: start..start + 1,
            kind: TextKind::StrayLessThan,
        })
    }

    fn truncated_tag(&mut self, start: usize) -> Step {
        let name_start = start + if self.bytes.get(start + 1) == Some(&b'/') { 2 } else { 1 };
        let has_attribute_syntax = self.bytes[name_start..]
            .iter()
            .any(|byte| is_tag_name_end(*byte));
        if has_attribute_syntax {
            return Step::Eof;
        }
        self.pos = self.bytes.len();
        Step::Token(Token::Text {
            range: start..self.bytes.len(),
            kind: TextKind::Truncated,
        })
    }

    fn bogus_comment(&mut self, from: usize) -> Step {
        match self.bytes[from..].iter().position(|byte| *byte == b'>') {
            Some(offset) => {
                self.pos = from + offset + 1;
                Step::Token(Token::Comment)
            }
            None => Step::Eof,
        }
    }

    fn starts_with_ignore_case(&self, at: usize, needle: &[u8]) -> bool {
        self.bytes
            .get(at..at + needle.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
    }

    fn markup_declaration(&mut self, start: usize) -> Step {
        let after_bang = start + 2;
        if self.bytes[after_bang..].starts_with(b"--") {
            return match self.comment_end(after_bang + 2) {
                Some(end) => {
                    self.pos = end;
                    Step::Token(Token::Comment)
                }
                None => Step::Eof,
            };
        }
        if self.starts_with_ignore_case(after_bang, b"doctype") {
            // Every DOCTYPE state ends at the first `>`.
            return match self.bytes[after_bang..].iter().position(|byte| *byte == b'>') {
                Some(offset) => {
                    self.pos = after_bang + offset + 1;
                    Step::Token(Token::Doctype)
                }
                None => Step::Eof,
            };
        }
        if self.allow_cdata && self.bytes[after_bang..].starts_with(b"[CDATA[") {
            let content = after_bang + b"[CDATA[".len();
            let (text_end, next) = match find_subslice(&self.bytes[content..], b"]]>") {
                Some(offset) => (content + offset, content + offset + 3),
                None => (self.bytes.len(), self.bytes.len()),
            };
            self.pos = next;
            if text_end == content {
                return Step::Continue;
            }
            return Step::Token(Token::Text {
                range: content..text_end,
                kind: TextKind::CData,
            });
        }
        self.bogus_comment(after_bang)
    }

    /// Finds the end (exclusive) of a comment whose `<!--` ends at `from`, following the
    /// comment states of the HTML Standard, including the nested-comment states.
    fn comment_end(&self, from: usize) -> Option<usize> {
        #[derive(Clone, Copy)]
        enum State {
            Start,
            StartDash,
            Comment,
            LessThan,
            LessThanBang,
            LessThanBangDash,
            EndDash,
            End,
            EndBang,
        }

        let bytes = self.bytes;
        let mut state = State::Start;
        let mut i = from;
        while i < bytes.len() {
            let byte = bytes[i];
            match state {
                State::Start => match byte {
                    b'-' => state = State::StartDash,
                    b'>' => return Some(i + 1),
                    _ => {
                        state = State::Comment;
                        continue;
                    }
                },
                State::StartDash => match byte {
                    b'-' => state = State::End,
                    b'>' => return Some(i + 1),
                    _ => {
                        state = State::Comment;
                        continue;
                    }
                },
                State::Comment => match byte {
                    b'<' => state = State::LessThan,
                    b'-' => state = State::EndDash,
                    _ => {}
                },
                State::LessThan => match byte {
                    b'!' => state = State::LessThanBang,
                    b'<' => {}
                    _ => {
                        state = State::Comment;
                        continue;
                    }
                },
                State::LessThanBang => match byte {
                    b'-' => state = State::LessThanBangDash,
                    _ => {
                        state = State::Comment;
                        continue;
                    }
                },
                State::LessThanBangDash => match byte {
                    b'-' => state = State::End,
                    _ => {
                        state = State::EndDash;
                        continue;
                    }
                },
                State::EndDash => match byte {
                    b'-' => state = State::End,
                    _ => {
                        state = State::Comment;
                        continue;
                    }
                },
                State::End => match byte {
                    b'>' => return Some(i + 1),
                    b'!' => state = State::EndBang,
                    b'-' => {}
                    _ => {
                        state = State::Comment;
                        continue;
                    }
                },
                State::EndBang => match byte {
                    b'-' => state = State::EndDash,
                    b'>' => return Some(i + 1),
                    _ => {
                        state = State::Comment;
                        continue;
                    }
                },
            }
            i += 1;
        }
        None
    }

    /// Parses the start or end tag whose `<` is at `lt`. Returns `None` when the input ends
    /// inside the tag.
    fn parse_tag(&mut self, lt: usize, is_end: bool) -> Option<TagSpan> {
        let bytes = self.bytes;
        let len = bytes.len();
        let name_start = lt + if is_end { 2 } else { 1 };
        let mut i = name_start;
        while i < len && !is_tag_name_end(bytes[i]) {
            i += 1;
        }
        if i >= len {
            return None;
        }
        let name = name_start..i;
        let mut attrs = Vec::new();
        let mut self_closing = false;

        loop {
            while i < len && is_ws(bytes[i]) {
                i += 1;
            }
            if i >= len {
                return None;
            }
            match bytes[i] {
                b'>' => {
                    i += 1;
                    break;
                }
                b'/' => {
                    if bytes.get(i + 1) == Some(&b'>') {
                        self_closing = true;
                        i += 2;
                        break;
                    }
                    i += 1;
                    continue;
                }
                _ => {}
            }

            // The first character of an attribute name is part of the name even when it is `=`.
            let attr_start = i;
            i += 1;
            while i < len && !(is_tag_name_end(bytes[i]) || bytes[i] == b'=') {
                i += 1;
            }
            if i >= len {
                return None;
            }
            let attr_name = attr_start..i;

            let mut j = i;
            while j < len && is_ws(bytes[j]) {
                j += 1;
            }
            if j >= len {
                return None;
            }
            if bytes[j] != b'=' {
                attrs.push(AttrSpan {
                    raw: attr_name.clone(),
                    name: attr_name,
                    value: i..i,
                });
                continue;
            }
            j += 1;
            while j < len && is_ws(bytes[j]) {
                j += 1;
            }
            if j >= len {
                return None;
            }
            let (value, end) = match bytes[j] {
                quote @ (b'"' | b'\'') => {
                    let close = bytes[j + 1..].iter().position(|byte| *byte == quote)?;
                    let close = j + 1 + close;
                    (j + 1..close, close + 1)
                }
                b'>' => (j..j, j),
                _ => {
                    let mut k = j;
                    while k < len && !(is_ws(bytes[k]) || bytes[k] == b'>') {
                        k += 1;
                    }
                    if k >= len {
                        return None;
                    }
                    (j..k, k)
                }
            };
            attrs.push(AttrSpan {
                name: attr_name,
                raw: attr_start..end,
                value,
            });
            i = end;
        }

        self.pos = i;
        Some(TagSpan {
            span: lt..i,
            name,
            attrs,
            self_closing,
        })
    }

    /// Returns true when an "appropriate end tag" (`</name` followed by whitespace, `/` or `>`)
    /// starts at `at`.
    fn appropriate_end_tag_at(&self, at: usize) -> bool {
        let name = self.last_start_tag.as_bytes();
        let name_start = at + 2;
        if !self.bytes[at..].starts_with(b"</") || !self.starts_with_ignore_case(name_start, name) {
            return false;
        }
        self.bytes
            .get(name_start + name.len())
            .is_some_and(|byte| is_tag_name_end(*byte))
    }

    fn text_mode_step(&mut self) -> Step {
        let start = self.pos;
        let kind = match self.mode {
            TextMode::RcData => TextKind::RcData,
            TextMode::RawText => TextKind::RawText,
            _ => TextKind::ScriptData,
        };
        let end_tag = if self.mode == TextMode::ScriptData {
            self.find_script_data_end(start)
        } else {
            let mut at = start;
            loop {
                match self.bytes[at..].iter().position(|byte| *byte == b'<') {
                    Some(offset) => {
                        at += offset;
                        if self.appropriate_end_tag_at(at) {
                            break Some(at);
                        }
                        at += 1;
                    }
                    None => break None,
                }
            }
        };
        match end_tag {
            Some(at) if at == start => match self.parse_tag(at, true) {
                Some(tag) => Step::Token(Token::EndTag(tag)),
                None => self.truncated_tag(at),
            },
            Some(at) => {
                self.pos = at;
                Step::Token(Token::Text {
                    range: start..at,
                    kind,
                })
            }
            None => {
                self.pos = self.bytes.len();
                Step::Token(Token::Text {
                    range: start..self.bytes.len(),
                    kind,
                })
            }
        }
    }

    /// Finds the `<` of the end tag that leaves script data, following the script data escape
    /// states (`<!--`, `<script`, `-->`) of the HTML Standard.
    fn find_script_data_end(&self, from: usize) -> Option<usize> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            Data,
            LessThan,
            EscapeStart,
            EscapeStartDash,
            Escaped,
            EscapedDash,
            EscapedDashDash,
            EscapedLessThan,
            DoubleEscapeStart,
            DoubleEscaped,
            DoubleEscapedDash,
            DoubleEscapedDashDash,
            DoubleEscapedLessThan,
            DoubleEscapeEnd,
        }

        let bytes = self.bytes;
        let mut state = State::Data;
        let mut buffer = [0u8; 6];
        let mut buffer_len = 0usize;
        let mut buffer_overflow = false;
        let mut i = from;
        while i < bytes.len() {
            let byte = bytes[i];
            match state {
                State::Data => {
                    if byte == b'<' {
                        state = State::LessThan;
                    }
                }
                State::LessThan => match byte {
                    b'/' => {
                        if self.appropriate_end_tag_at(i - 1) {
                            return Some(i - 1);
                        }
                        state = State::Data;
                    }
                    b'!' => state = State::EscapeStart,
                    _ => {
                        state = State::Data;
                        continue;
                    }
                },
                State::EscapeStart => match byte {
                    b'-' => state = State::EscapeStartDash,
                    _ => {
                        state = State::Data;
                        continue;
                    }
                },
                State::EscapeStartDash => match byte {
                    b'-' => state = State::EscapedDashDash,
                    _ => {
                        state = State::Data;
                        continue;
                    }
                },
                State::Escaped => match byte {
                    b'-' => state = State::EscapedDash,
                    b'<' => state = State::EscapedLessThan,
                    _ => {}
                },
                State::EscapedDash => match byte {
                    b'-' => state = State::EscapedDashDash,
                    b'<' => state = State::EscapedLessThan,
                    _ => state = State::Escaped,
                },
                State::EscapedDashDash => match byte {
                    b'-' => {}
                    b'<' => state = State::EscapedLessThan,
                    b'>' => state = State::Data,
                    _ => state = State::Escaped,
                },
                State::EscapedLessThan => match byte {
                    b'/' => {
                        if self.appropriate_end_tag_at(i - 1) {
                            return Some(i - 1);
                        }
                        state = State::Escaped;
                    }
                    byte if byte.is_ascii_alphabetic() => {
                        buffer_len = 0;
                        buffer_overflow = false;
                        state = State::DoubleEscapeStart;
                        continue;
                    }
                    _ => {
                        state = State::Escaped;
                        continue;
                    }
                },
                State::DoubleEscapeStart => match byte {
                    byte if is_tag_name_end(byte) => {
                        state = if !buffer_overflow && &buffer[..buffer_len] == b"script" {
                            State::DoubleEscaped
                        } else {
                            State::Escaped
                        };
                    }
                    byte if byte.is_ascii_alphabetic() => {
                        if buffer_len < buffer.len() {
                            buffer[buffer_len] = byte.to_ascii_lowercase();
                            buffer_len += 1;
                        } else {
                            buffer_overflow = true;
                        }
                    }
                    _ => {
                        state = State::Escaped;
                        continue;
                    }
                },
                State::DoubleEscaped => match byte {
                    b'-' => state = State::DoubleEscapedDash,
                    b'<' => state = State::DoubleEscapedLessThan,
                    _ => {}
                },
                State::DoubleEscapedDash => match byte {
                    b'-' => state = State::DoubleEscapedDashDash,
                    b'<' => state = State::DoubleEscapedLessThan,
                    _ => state = State::DoubleEscaped,
                },
                State::DoubleEscapedDashDash => match byte {
                    b'-' => {}
                    b'<' => state = State::DoubleEscapedLessThan,
                    b'>' => state = State::Data,
                    _ => state = State::DoubleEscaped,
                },
                State::DoubleEscapedLessThan => match byte {
                    b'/' => {
                        buffer_len = 0;
                        buffer_overflow = false;
                        state = State::DoubleEscapeEnd;
                    }
                    _ => {
                        state = State::DoubleEscaped;
                        continue;
                    }
                },
                State::DoubleEscapeEnd => match byte {
                    byte if is_tag_name_end(byte) => {
                        state = if !buffer_overflow && &buffer[..buffer_len] == b"script" {
                            State::Escaped
                        } else {
                            State::DoubleEscaped
                        };
                    }
                    byte if byte.is_ascii_alphabetic() => {
                        if buffer_len < buffer.len() {
                            buffer[buffer_len] = byte.to_ascii_lowercase();
                            buffer_len += 1;
                        } else {
                            buffer_overflow = true;
                        }
                    }
                    _ => {
                        state = State::DoubleEscaped;
                        continue;
                    }
                },
            }
            i += 1;
        }
        None
    }
}

enum Step {
    Token(Token),
    Continue,
    Eof,
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_all(src: &str) -> Vec<String> {
        let mut lexer = Lexer::new(src);
        let mut out = Vec::new();
        while let Some(token) = lexer.next_token() {
            out.push(match token {
                Token::Text { range, kind } => format!("text:{kind:?}:{}", &src[range]),
                Token::StartTag(tag) => {
                    let attrs: Vec<String> = tag
                        .attrs
                        .iter()
                        .map(|a| format!("{}={}", &src[a.name.clone()], &src[a.value.clone()]))
                        .collect();
                    format!(
                        "start:{}[{}]{}",
                        &src[tag.name],
                        attrs.join(","),
                        if tag.self_closing { "/" } else { "" }
                    )
                }
                Token::EndTag(tag) => format!("end:{}", &src[tag.name]),
                Token::Comment => "comment".into(),
                Token::Doctype => "doctype".into(),
            });
        }
        out
    }

    #[test]
    fn tags_and_attribute_syntax_follow_the_standard() {
        assert_eq!(
            lex_all(r#"<a href=x/y title="a > b" data-q='"' disabled>t"#),
            [
                "start:a[href=x/y,title=a > b,data-q=\",disabled=]",
                "text:Data:t"
            ]
        );
        // `disabled =1/` is one attribute whose unquoted value is `1/`.
        assert_eq!(lex_all("<a disabled =1/>"), ["start:a[disabled=1/]"]);
        assert_eq!(lex_all("<br/>"), ["start:br[]/"]);
        assert_eq!(lex_all("<br a/>"), ["start:br[a=]/"]);
        // An attribute name may start with `=`.
        assert_eq!(lex_all("<b =x y>"), ["start:b[=x=,y=]"]);
        // Missing whitespace between attributes.
        assert_eq!(lex_all(r#"<b a="1"c='2'>"#), ["start:b[a=1,c=2]"]);
        assert_eq!(lex_all("<b a=>x"), ["start:b[a=]", "text:Data:x"]);
    }

    #[test]
    fn incomplete_constructs_are_discarded_or_passed_through() {
        for src in [
            "<!-- x",
            "<!x",
            "<?x",
            "<!doctype x",
            "</ x",
            "<b ",
            "<b a",
            "<b a=",
            "<b a=\"x",
            "<b a=x",
            "<b/",
            "</b x=\"y",
        ] {
            assert!(lex_all(src).is_empty(), "{src}");
        }
        for src in ["<b", "</b", "<Less&lt;"] {
            assert_eq!(lex_all(src), [format!("text:Truncated:{src}")], "{src}");
        }
        assert_eq!(lex_all("a<b"), ["text:Data:a", "text:Truncated:<b"]);
        assert_eq!(lex_all("a<"), ["text:Data:a", "text:StrayLessThan:<"]);
        assert_eq!(lex_all("a</"), ["text:Data:a", "text:StrayLessThan:</"]);
    }

    #[test]
    fn comments_follow_the_comment_states() {
        assert_eq!(lex_all("<!---->x"), ["comment", "text:Data:x"]);
        assert_eq!(lex_all("<!-->x"), ["comment", "text:Data:x"]);
        assert_eq!(lex_all("<!--->x"), ["comment", "text:Data:x"]);
        assert_eq!(lex_all("<!-- a --!>x"), ["comment", "text:Data:x"]);
        assert_eq!(lex_all("<!-- a > b -->x"), ["comment", "text:Data:x"]);
        assert_eq!(lex_all("<!-- <!--> x"), ["comment", "text:Data: x"]);
        assert_eq!(lex_all("<!-- a <!-- b --> x"), ["comment", "text:Data: x"]);
        assert_eq!(lex_all("<? a > b"), ["comment", "text:Data: b"]);
        assert_eq!(lex_all("</ a > b"), ["comment", "text:Data: b"]);
        assert_eq!(lex_all("</>b"), ["text:Data:b"]);
        assert_eq!(lex_all("<!DocType html>b"), ["doctype", "text:Data:b"]);
        // CDATA is a bogus comment in HTML content.
        assert_eq!(lex_all("<![CDATA[a>b]]>"), ["comment", "text:Data:b]]>"]);
    }

    #[test]
    fn cdata_is_text_in_foreign_content_only() {
        let src = "<![CDATA[a<b>]]>z";
        let mut lexer = Lexer::new(src);
        lexer.set_allow_cdata(true);
        assert_eq!(
            lexer.next_token(),
            Some(Token::Text {
                range: 9..13,
                kind: TextKind::CData
            })
        );
        assert_eq!(
            lexer.next_token(),
            Some(Token::Text {
                range: 16..17,
                kind: TextKind::Data
            })
        );
    }

    #[test]
    fn rcdata_and_rawtext_end_only_at_the_appropriate_end_tag() {
        let src = "a</b><c></TEXTAREA >z";
        let mut lexer = Lexer::new(src);
        lexer.set_text_mode(TextMode::RcData, "textarea");
        assert_eq!(
            lexer.next_token(),
            Some(Token::Text {
                range: 0..8,
                kind: TextKind::RcData
            })
        );
        assert!(matches!(lexer.next_token(), Some(Token::EndTag(_))));
        lexer.set_text_mode(TextMode::Data, "");
        assert_eq!(
            lexer.next_token(),
            Some(Token::Text {
                range: 20..21,
                kind: TextKind::Data
            })
        );
        // `</textareax>` is not an end tag.
        assert_eq!(
            {
                let mut lexer = Lexer::new("</textareax></textarea/>");
                lexer.set_text_mode(TextMode::RawText, "textarea");
                let first = lexer.next_token();
                let second = lexer.next_token();
                (first, matches!(second, Some(Token::EndTag(_))))
            },
            (
                Some(Token::Text {
                    range: 0..12,
                    kind: TextKind::RawText
                }),
                true
            )
        );
    }

    #[test]
    fn script_data_escape_states_hide_nested_end_tags() {
        // Inside `<!--<script>` the first `</script>` does not end the element.
        let src = "<!--<script>a</script>b--></script>c";
        let mut lexer = Lexer::new(src);
        lexer.set_text_mode(TextMode::ScriptData, "script");
        assert_eq!(
            lexer.next_token(),
            Some(Token::Text {
                range: 0..src.len() - "</script>c".len(),
                kind: TextKind::ScriptData
            })
        );
        assert!(matches!(lexer.next_token(), Some(Token::EndTag(_))));

        // Without the double escape the first end tag wins.
        let src = "<!-- a </script>b";
        let mut lexer = Lexer::new(src);
        lexer.set_text_mode(TextMode::ScriptData, "script");
        assert_eq!(
            lexer.next_token(),
            Some(Token::Text {
                range: 0..7,
                kind: TextKind::ScriptData
            })
        );
        assert!(matches!(lexer.next_token(), Some(Token::EndTag(_))));
    }

    #[test]
    fn plaintext_runs_to_the_end_of_input() {
        let mut lexer = Lexer::new("<b>x</plaintext>");
        lexer.set_text_mode(TextMode::PlainText, "plaintext");
        assert_eq!(
            lexer.next_token(),
            Some(Token::Text {
                range: 0..16,
                kind: TextKind::PlainText
            })
        );
        assert_eq!(lexer.next_token(), None);
    }
}
