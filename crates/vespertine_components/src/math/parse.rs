//! LaTeX math parser: source text to a syntax tree (CMP-012 R1, R3).
//!
//! Nesting is limited ([`MAX_NESTING`]); unknown commands, unbalanced groups and every other
//! malformed construct are errors, never panics.

use super::symbols::{self, Font, Symbol};

/// Nesting limit of groups, arguments, environments and `\left...\right` pairs.
pub const MAX_NESTING: usize = 64;

/// Why a formula was refused by the engine. The text is for diagnostics only: a document never
/// shows it (CMP-012 R6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MathError {
    /// The source is longer than 16 KiB.
    TooLong,
    /// A syntax error (unknown command, unbalanced group, ...).
    Syntax(String),
    /// Nesting deeper than 64.
    TooDeep,
    /// More emitted items than allowed for the source length.
    TooManyItems,
    /// Advance wider than 2000 em.
    TooWide,
    /// Ascent plus descent taller than 200 em.
    TooTall,
    /// A character without a glyph in any face.
    MissingGlyph(char),
    /// An overflowing or non-finite extent.
    NonFinite,
}

impl std::fmt::Display for MathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MathError::TooLong => write!(f, "formula source is longer than 16384 bytes"),
            MathError::Syntax(message) => write!(f, "{message}"),
            MathError::TooDeep => write!(f, "formula nesting is deeper than {MAX_NESTING}"),
            MathError::TooManyItems => write!(f, "formula emits too many items"),
            MathError::TooWide => write!(f, "formula is wider than 2000 em"),
            MathError::TooTall => write!(f, "formula is taller than 200 em"),
            MathError::MissingGlyph(ch) => write!(f, "no glyph for U+{:04X}", u32::from(*ch)),
            MathError::NonFinite => write!(f, "formula extent is not finite"),
        }
    }
}

impl std::error::Error for MathError {}

fn syntax<T>(message: impl Into<String>) -> Result<T, MathError> {
    Err(MathError::Syntax(message.into()))
}

/// TeX atom classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
}

/// Where scripts of an atom go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Limits {
    /// Always beside the nucleus.
    Never,
    /// Above and below in display style only.
    DisplayOnly,
    /// Above and below in every style.
    Always,
}

/// A TeX style level without the cramped flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Display,
    Text,
    Script,
    ScriptScript,
}

pub(crate) type Group = Vec<Node>;

pub(crate) type Delim = Option<char>;

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Atom(Atom),
    /// Explicit horizontal space in ems of the current style (negative allowed).
    Space(f32),
    /// A style switch applying to the rest of the list.
    Style(Level),
}

#[derive(Clone, Debug)]
pub(crate) struct Atom {
    pub class: Class,
    pub kind: Kind,
    pub sub: Option<Group>,
    pub sup: Option<Group>,
    pub limits: Limits,
}

impl Atom {
    fn new(class: Class, kind: Kind) -> Atom {
        Atom {
            class,
            kind,
            sub: None,
            sup: None,
            limits: Limits::Never,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
pub(crate) struct Frac {
    pub num: Group,
    pub den: Group,
    pub rule: bool,
    pub forced: Option<Level>,
    pub open: Delim,
    pub close: Delim,
}

#[derive(Clone, Debug)]
pub(crate) struct Matrix {
    pub rows: Vec<Vec<Group>>,
    pub aligns: Vec<Align>,
    /// Gap after each column except the last, in ems.
    pub gaps: Vec<f32>,
    pub level: Level,
    pub open: Delim,
    pub close: Delim,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    /// A character already mapped to its mathematical alphabet.
    Char(char),
    /// A braced group: an ordinary atom around a list.
    Group(Group),
    /// Upright text.
    Text(String),
    Frac(Box<Frac>),
    Sqrt {
        degree: Option<Group>,
        body: Group,
    },
    /// `\left ... \middle ... \right`.
    Fenced {
        open: Delim,
        close: Delim,
        parts: Vec<Group>,
        middles: Vec<Delim>,
    },
    /// `\big` and friends: a delimiter of a fixed size (index 1 to 4).
    Sized {
        size: u8,
        ch: Delim,
    },
    Matrix(Box<Matrix>),
    /// A character accent over (or under) a body; `wide` accents stretch with the body.
    Accent {
        ch: char,
        body: Group,
        over: bool,
        wide: bool,
    },
    /// `\overline` and `\underline`.
    Bar {
        over: bool,
        body: Group,
    },
    /// `\overset`, `\underset`, `\stackrel`.
    Stack {
        base: Group,
        over: Option<Group>,
        under: Option<Group>,
    },
    Phantom {
        body: Group,
        horizontal: bool,
        vertical: bool,
    },
    /// An operator name set upright (`\sin`, `\operatorname{rank}`).
    OpName(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Eof,
    CloseBrace,
    Right,
    Middle,
    End,
    Amp,
    Newline,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    depth: usize,
    font: Font,
}

/// Parses a formula into a syntax tree.
pub(crate) fn parse(src: &str) -> Result<Group, MathError> {
    let mut parser = Parser {
        src,
        pos: 0,
        depth: 0,
        font: Font::Italic,
    };
    let (group, stop) = parser.parse_list()?;
    match stop {
        Stop::Eof => Ok(group),
        Stop::CloseBrace => syntax("unbalanced closing brace"),
        Stop::Right | Stop::Middle => syntax("\\right or \\middle without \\left"),
        Stop::End => syntax("\\end without \\begin"),
        Stop::Amp | Stop::Newline => syntax("alignment outside an environment"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Char(char),
    Command(String),
}

impl<'a> Parser<'a> {
    fn skip_space(&mut self) {
        loop {
            let rest = &self.src[self.pos..];
            let trimmed = rest.trim_start_matches(|c: char| c.is_whitespace());
            self.pos += rest.len() - trimmed.len();
            if self.src[self.pos..].starts_with('%') {
                let rest = &self.src[self.pos..];
                self.pos += rest.find('\n').map_or(rest.len(), |ix| ix + 1);
            } else {
                break;
            }
        }
    }

    fn peek(&mut self) -> Option<Token> {
        let saved = self.pos;
        let token = self.next();
        self.pos = saved;
        token
    }

    fn next(&mut self) -> Option<Token> {
        self.skip_space();
        let mut chars = self.src[self.pos..].chars();
        let ch = chars.next()?;
        self.pos += ch.len_utf8();
        if ch != '\\' {
            return Some(Token::Char(ch));
        }
        let rest = &self.src[self.pos..];
        let name_len = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        if name_len > 0 {
            let name = rest[..name_len].to_string();
            self.pos += name_len;
            Some(Token::Command(name))
        } else if let Some(c) = rest.chars().next() {
            self.pos += c.len_utf8();
            Some(Token::Command(c.to_string()))
        } else {
            None
        }
    }

    fn eat_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(Token::Char(expected)) {
            self.next();
            true
        } else {
            false
        }
    }

    fn nest<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, MathError>,
    ) -> Result<T, MathError> {
        if self.depth >= MAX_NESTING {
            return Err(MathError::TooDeep);
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }

    /// Parses atoms until a terminator, which is consumed and reported.
    fn parse_list(&mut self) -> Result<(Group, Stop), MathError> {
        let mut nodes: Group = Vec::new();
        let mut infix: Option<(Group, Frac)> = None;
        let stop = loop {
            let Some(token) = self.peek() else {
                break Stop::Eof;
            };
            match &token {
                Token::Char('}') => {
                    self.next();
                    break Stop::CloseBrace;
                }
                Token::Char('&') => {
                    self.next();
                    break Stop::Amp;
                }
                Token::Command(name) => match name.as_str() {
                    "right" => {
                        self.next();
                        break Stop::Right;
                    }
                    "middle" => {
                        self.next();
                        break Stop::Middle;
                    }
                    "end" => {
                        self.next();
                        break Stop::End;
                    }
                    "\\" => {
                        self.next();
                        break Stop::Newline;
                    }
                    "over" | "atop" | "choose" => {
                        let name = name.clone();
                        self.next();
                        if infix.is_some() {
                            return syntax("ambiguous infix fraction");
                        }
                        let (open, close, rule) = match name.as_str() {
                            "over" => (None, None, true),
                            "atop" => (None, None, false),
                            _ => (Some('('), Some(')'), false),
                        };
                        let frac = Frac {
                            num: Vec::new(),
                            den: Vec::new(),
                            rule,
                            forced: None,
                            open,
                            close,
                        };
                        infix = Some((std::mem::take(&mut nodes), frac));
                        continue;
                    }
                    _ => {}
                },
                _ => {}
            }
            let node = self.parse_node()?;
            nodes.push(node);
        };
        if let Some((num, mut frac)) = infix {
            frac.num = num;
            frac.den = nodes;
            let atom = Atom::new(Class::Inner, Kind::Frac(Box::new(frac)));
            let atom = if frac_is_plain(&atom) {
                Atom {
                    class: Class::Ord,
                    ..atom
                }
            } else {
                atom
            };
            nodes = vec![Node::Atom(atom)];
        }
        Ok((nodes, stop))
    }

    /// One node with its scripts.
    fn parse_node(&mut self) -> Result<Node, MathError> {
        let mut node = self.parse_nucleus()?;
        loop {
            match self.peek() {
                Some(Token::Char('^')) => {
                    self.next();
                    let arg = self.parse_arg()?;
                    let atom = script_target(&mut node);
                    match &mut atom.sup {
                        Some(existing) if atom.sub.is_none() && is_prime_only(existing) => {
                            existing.extend(arg);
                        }
                        Some(_) => return syntax("double superscript"),
                        None => atom.sup = Some(arg),
                    }
                }
                Some(Token::Char('_')) => {
                    self.next();
                    let arg = self.parse_arg()?;
                    let atom = script_target(&mut node);
                    if atom.sub.is_some() {
                        return syntax("double subscript");
                    }
                    atom.sub = Some(arg);
                }
                Some(Token::Char('\'')) => {
                    self.next();
                    let atom = script_target(&mut node);
                    let prime = Node::Atom(Atom::new(Class::Ord, Kind::Char('\u{2032}')));
                    match &mut atom.sup {
                        Some(sup) if is_prime_only(sup) => sup.push(prime),
                        Some(_) => return syntax("double superscript"),
                        None => atom.sup = Some(vec![prime]),
                    }
                }
                _ => break,
            }
        }
        Ok(node)
    }

    /// A braced group or a single token as an argument.
    fn parse_arg(&mut self) -> Result<Group, MathError> {
        self.nest(|this| match this.peek() {
            Some(Token::Char('{')) => {
                this.next();
                let (group, stop) = this.parse_list()?;
                if stop != Stop::CloseBrace {
                    return syntax("unbalanced group");
                }
                Ok(group)
            }
            None => syntax("missing argument"),
            Some(Token::Char('}')) => syntax("missing argument"),
            Some(_) => Ok(vec![this.parse_nucleus()?]),
        })
    }

    /// An argument parsed with a different font state.
    fn parse_arg_in(&mut self, font: Font) -> Result<Group, MathError> {
        let saved = std::mem::replace(&mut self.font, font);
        let result = self.parse_arg();
        self.font = saved;
        result
    }

    fn parse_delimiter(&mut self) -> Result<Delim, MathError> {
        match self.next() {
            Some(Token::Char('.')) => Ok(None),
            Some(Token::Char(ch)) => symbols::delimiter_char(&ch.to_string())
                .map(Some)
                .ok_or_else(|| MathError::Syntax(format!("bad delimiter {ch}"))),
            Some(Token::Command(name)) => symbols::delimiter_char(&format!("\\{name}"))
                .map(Some)
                .ok_or_else(|| MathError::Syntax(format!("bad delimiter \\{name}"))),
            None => syntax("missing delimiter"),
        }
    }

    fn parse_nucleus(&mut self) -> Result<Node, MathError> {
        let Some(token) = self.next() else {
            return syntax("unexpected end of formula");
        };
        match token {
            Token::Char('{') => {
                let group = self.nest(|this| {
                    let (group, stop) = this.parse_list()?;
                    if stop != Stop::CloseBrace {
                        return syntax("unbalanced group");
                    }
                    Ok(group)
                })?;
                Ok(Node::Atom(Atom::new(Class::Ord, Kind::Group(group))))
            }
            Token::Char('}') => syntax("unbalanced closing brace"),
            Token::Char('^') | Token::Char('_') => syntax("script without nucleus"),
            Token::Char('~') => Ok(Node::Space(1.0 / 3.0)),
            Token::Char('#') | Token::Char('$') => syntax("unsupported character"),
            Token::Char(ch) => Ok(self.char_atom(ch)),
            Token::Command(name) => self.parse_command(&name),
        }
    }

    fn char_atom(&self, ch: char) -> Node {
        let (class, mapped) = symbols::char_symbol(ch, self.font);
        Node::Atom(Atom::new(class, Kind::Char(mapped)))
    }

    fn parse_command(&mut self, name: &str) -> Result<Node, MathError> {
        match name {
            "left" => return self.parse_fenced(),
            "begin" => return self.parse_environment(),
            "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" => {
                let num = self.parse_arg()?;
                let den = self.parse_arg()?;
                let (open, close) = if name.contains("binom") {
                    (Some('('), Some(')'))
                } else {
                    (None, None)
                };
                let forced = match name {
                    "dfrac" | "dbinom" | "cfrac" => Some(Level::Display),
                    "tfrac" | "tbinom" => Some(Level::Text),
                    _ => None,
                };
                let frac = Frac {
                    num,
                    den,
                    rule: !name.contains("binom"),
                    forced,
                    open,
                    close,
                };
                let mut atom = Atom::new(Class::Ord, Kind::Frac(Box::new(frac)));
                if name.contains("binom") {
                    atom.class = Class::Ord;
                }
                return Ok(Node::Atom(atom));
            }
            "sqrt" => {
                let degree = if self.peek() == Some(Token::Char('[')) {
                    self.next();
                    let start = self.pos;
                    let depth = self.nest(|this| {
                        // Read the degree up to the closing bracket at the top nesting level.
                        let mut level = 0usize;
                        let mut end = None;
                        while let Some(token) = this.next() {
                            match token {
                                Token::Char('{') => level += 1,
                                Token::Char('}') => level = level.saturating_sub(1),
                                Token::Char(']') if level == 0 => {
                                    end = Some(this.pos - 1);
                                    break;
                                }
                                _ => {}
                            }
                        }
                        end.ok_or_else(|| MathError::Syntax("unbalanced bracket".into()))
                    })?;
                    let inner = &self.src[start..depth];
                    let mut sub = Parser {
                        src: inner,
                        pos: 0,
                        depth: self.depth + 1,
                        font: self.font,
                    };
                    if sub.depth > MAX_NESTING {
                        return Err(MathError::TooDeep);
                    }
                    let (group, stop) = sub.parse_list()?;
                    if stop != Stop::Eof {
                        return syntax("unbalanced radical degree");
                    }
                    Some(group)
                } else {
                    None
                };
                let body = self.parse_arg()?;
                return Ok(Node::Atom(Atom::new(
                    Class::Ord,
                    Kind::Sqrt { degree, body },
                )));
            }
            "mathrm" | "textrm" | "mathit" | "mathbf" | "boldsymbol" | "bm" | "mathsf"
            | "mathtt" | "mathbb" | "mathcal" | "mathscr" | "mathfrak" | "mathnormal" => {
                let font = match name {
                    "mathrm" | "textrm" => Font::Roman,
                    "mathit" | "mathnormal" => Font::Italic,
                    "mathbf" | "boldsymbol" | "bm" => Font::Bold,
                    "mathsf" => Font::Sans,
                    "mathtt" => Font::Mono,
                    "mathbb" => Font::DoubleStruck,
                    "mathcal" | "mathscr" => Font::Script,
                    _ => Font::Fraktur,
                };
                let group = self.parse_arg_in(font)?;
                return Ok(Node::Atom(Atom::new(Class::Ord, Kind::Group(group))));
            }
            "text" | "mbox" | "hbox" | "textnormal" | "textit" | "textbf" => {
                return self.parse_text();
            }
            "operatorname" => {
                let star = self.eat_char('*');
                let name = self.parse_text_raw()?;
                let mut atom = Atom::new(Class::Op, Kind::OpName(name));
                atom.limits = if star {
                    Limits::DisplayOnly
                } else {
                    Limits::Never
                };
                return Ok(Node::Atom(atom));
            }
            "overline" | "underline" => {
                let body = self.parse_arg()?;
                return Ok(Node::Atom(Atom::new(
                    Class::Ord,
                    Kind::Bar {
                        over: name == "overline",
                        body,
                    },
                )));
            }
            "overset" | "underset" | "stackrel" => {
                let label = self.parse_arg()?;
                let base = self.parse_arg()?;
                let over = name != "underset";
                let kind = Kind::Stack {
                    base,
                    over: over.then(|| label.clone()),
                    under: (!over).then_some(label),
                };
                let class = if name == "stackrel" {
                    Class::Rel
                } else {
                    Class::Ord
                };
                return Ok(Node::Atom(Atom::new(class, kind)));
            }
            "phantom" | "hphantom" | "vphantom" => {
                let body = self.parse_arg()?;
                return Ok(Node::Atom(Atom::new(
                    Class::Ord,
                    Kind::Phantom {
                        body,
                        horizontal: name != "vphantom",
                        vertical: name != "hphantom",
                    },
                )));
            }
            "displaystyle" => return Ok(Node::Style(Level::Display)),
            "textstyle" => return Ok(Node::Style(Level::Text)),
            "scriptstyle" => return Ok(Node::Style(Level::Script)),
            "scriptscriptstyle" => return Ok(Node::Style(Level::ScriptScript)),
            "bigl" | "bigr" | "bigm" | "big" | "Bigl" | "Bigr" | "Bigm" | "Big" | "biggl"
            | "biggr" | "biggm" | "bigg" | "Biggl" | "Biggr" | "Biggm" | "Bigg" => {
                let size = match name.trim_end_matches(['l', 'r', 'm']) {
                    "big" => 1,
                    "Big" => 2,
                    "bigg" => 3,
                    _ => 4,
                };
                let class = match name.chars().last() {
                    Some('l') => Class::Open,
                    Some('r') => Class::Close,
                    _ => Class::Ord,
                };
                let ch = self.parse_delimiter()?;
                return Ok(Node::Atom(Atom::new(class, Kind::Sized { size, ch })));
            }
            "limits" | "nolimits" => return syntax("limits modifier without an operator"),
            "pmod" => {
                let body = self.parse_arg()?;
                let mut group: Group = vec![
                    Node::Space(1.0),
                    Node::Atom(Atom::new(Class::Open, Kind::Char('('))),
                    Node::Atom(Atom::new(Class::Op, Kind::OpName("mod".into()))),
                    Node::Space(1.0 / 3.0),
                ];
                group.extend(body);
                group.push(Node::Atom(Atom::new(Class::Close, Kind::Char(')'))));
                return Ok(Node::Atom(Atom::new(Class::Ord, Kind::Group(group))));
            }
            "bmod" => {
                return Ok(Node::Atom(Atom::new(
                    Class::Bin,
                    Kind::OpName("mod".into()),
                )));
            }
            "mod" => {
                let group: Group = vec![
                    Node::Space(1.0),
                    Node::Atom(Atom::new(Class::Op, Kind::OpName("mod".into()))),
                ];
                return Ok(Node::Atom(Atom::new(Class::Ord, Kind::Group(group))));
            }
            "mathbin" | "mathrel" | "mathord" | "mathop" | "mathopen" | "mathclose"
            | "mathpunct" | "mathinner" => {
                let class = match name {
                    "mathbin" => Class::Bin,
                    "mathrel" => Class::Rel,
                    "mathopen" => Class::Open,
                    "mathclose" => Class::Close,
                    "mathpunct" => Class::Punct,
                    "mathinner" => Class::Inner,
                    "mathop" => Class::Op,
                    _ => Class::Ord,
                };
                let group = self.parse_arg()?;
                let mut atom = Atom::new(class, Kind::Group(group));
                if class == Class::Op {
                    atom.limits = Limits::DisplayOnly;
                }
                return Ok(Node::Atom(atom));
            }
            _ => {}
        }
        match symbols::command_symbol(name) {
            Some(Symbol::Char(ch, class)) => {
                let (_, mapped) = symbols::char_symbol_mapped(ch, class, self.font);
                Ok(Node::Atom(Atom::new(class, Kind::Char(mapped))))
            }
            Some(Symbol::BigOp(ch, limits)) => {
                let mut atom = Atom::new(Class::Op, Kind::Char(ch));
                atom.limits = if limits {
                    Limits::DisplayOnly
                } else {
                    Limits::Never
                };
                self.limit_modifier(&mut atom);
                Ok(Node::Atom(atom))
            }
            Some(Symbol::OpName(text, limits)) => {
                let mut atom = Atom::new(Class::Op, Kind::OpName(text.to_string()));
                atom.limits = if limits {
                    Limits::DisplayOnly
                } else {
                    Limits::Never
                };
                self.limit_modifier(&mut atom);
                Ok(Node::Atom(atom))
            }
            Some(Symbol::Space(em)) => Ok(Node::Space(em)),
            Some(Symbol::Accent { ch, wide, over }) => {
                let body = self.parse_arg()?;
                let mut atom = Atom::new(
                    Class::Ord,
                    Kind::Accent {
                        ch,
                        body,
                        over,
                        wide,
                    },
                );
                if wide && matches!(ch, '\u{23DE}' | '\u{23DF}') {
                    atom.limits = Limits::Always;
                }
                Ok(Node::Atom(atom))
            }
            None => syntax(format!("unknown command \\{name}")),
        }
    }

    /// Applies a following `\limits` or `\nolimits` to an operator.
    fn limit_modifier(&mut self, atom: &mut Atom) {
        match self.peek() {
            Some(Token::Command(name)) if name == "limits" => {
                self.next();
                atom.limits = Limits::Always;
            }
            Some(Token::Command(name)) if name == "nolimits" => {
                self.next();
                atom.limits = Limits::Never;
            }
            _ => {}
        }
    }

    fn parse_fenced(&mut self) -> Result<Node, MathError> {
        let open = self.parse_delimiter()?;
        let (parts, middles, close) = self.nest(|this| {
            let mut parts = Vec::new();
            let mut middles = Vec::new();
            loop {
                let (group, stop) = this.parse_list()?;
                parts.push(group);
                match stop {
                    Stop::Middle => middles.push(this.parse_delimiter()?),
                    Stop::Right => {
                        let close = this.parse_delimiter()?;
                        return Ok((parts, middles, close));
                    }
                    _ => return syntax("\\left without \\right"),
                }
            }
        })?;
        Ok(Node::Atom(Atom::new(
            Class::Inner,
            Kind::Fenced {
                open,
                close,
                parts,
                middles,
            },
        )))
    }

    /// Raw text of a braced argument: characters and a few escaped symbols.
    fn parse_text_raw(&mut self) -> Result<String, MathError> {
        self.skip_space();
        if !self.src[self.pos..].starts_with('{') {
            return syntax("missing text argument");
        }
        self.pos += 1;
        let mut out = String::new();
        let mut level = 0usize;
        loop {
            let Some(ch) = self.src[self.pos..].chars().next() else {
                return syntax("unbalanced text group");
            };
            self.pos += ch.len_utf8();
            match ch {
                '{' => level += 1,
                '}' if level == 0 => return Ok(out),
                '}' => level -= 1,
                '\\' => {
                    let Some(next) = self.src[self.pos..].chars().next() else {
                        return syntax("unbalanced text group");
                    };
                    self.pos += next.len_utf8();
                    match next {
                        '%' | '&' | '#' | '_' | '{' | '}' | '$' => out.push(next),
                        ' ' | ',' | ';' | ':' => out.push(' '),
                        _ => return syntax("unsupported command in text"),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn parse_text(&mut self) -> Result<Node, MathError> {
        let text = self.parse_text_raw()?;
        Ok(Node::Atom(Atom::new(Class::Ord, Kind::Text(text))))
    }

    fn parse_environment(&mut self) -> Result<Node, MathError> {
        let name = self.parse_text_raw()?;
        let (open, close, aligns, gaps, level): (Delim, Delim, Option<Vec<Align>>, Gaps, Level) =
            match name.as_str() {
                "matrix" => (None, None, None, Gaps::Uniform, Level::Text),
                "pmatrix" => (Some('('), Some(')'), None, Gaps::Uniform, Level::Text),
                "bmatrix" => (Some('['), Some(']'), None, Gaps::Uniform, Level::Text),
                "Bmatrix" => (Some('{'), Some('}'), None, Gaps::Uniform, Level::Text),
                "vmatrix" => (Some('|'), Some('|'), None, Gaps::Uniform, Level::Text),
                "Vmatrix" => (
                    Some('\u{2016}'),
                    Some('\u{2016}'),
                    None,
                    Gaps::Uniform,
                    Level::Text,
                ),
                "smallmatrix" => (None, None, None, Gaps::Uniform, Level::Script),
                "cases" => (
                    Some('{'),
                    None,
                    Some(vec![Align::Left, Align::Left]),
                    Gaps::Cases,
                    Level::Text,
                ),
                "aligned" | "align" | "align*" | "split" | "eqnarray" => {
                    (None, None, Some(Vec::new()), Gaps::Aligned, Level::Display)
                }
                "gathered" | "gather" | "gather*" => (
                    None,
                    None,
                    Some(vec![Align::Center]),
                    Gaps::Uniform,
                    Level::Display,
                ),
                "array" => (
                    None,
                    None,
                    Some(self.parse_colspec()?),
                    Gaps::Uniform,
                    Level::Text,
                ),
                other => return syntax(format!("unknown environment {other}")),
            };
        let rows = self.nest(|this| {
            let mut rows: Vec<Vec<Group>> = Vec::new();
            let mut row: Vec<Group> = Vec::new();
            loop {
                let (cell, stop) = this.parse_list()?;
                row.push(cell);
                match stop {
                    Stop::Amp => {}
                    Stop::Newline => {
                        // An optional row spacing argument is accepted and ignored.
                        if this.peek() == Some(Token::Char('[')) {
                            let rest = &this.src[this.pos..];
                            match rest.find(']') {
                                Some(end) => this.pos += end + 1,
                                None => return syntax("unbalanced row spacing"),
                            }
                        }
                        rows.push(std::mem::take(&mut row));
                    }
                    Stop::End => {
                        let end = this.parse_text_raw()?;
                        if end != name {
                            return syntax(format!("\\begin{{{name}}} ended by \\end{{{end}}}"));
                        }
                        let trailing_empty = row.len() == 1 && row[0].is_empty();
                        if !(trailing_empty && !rows.is_empty()) {
                            rows.push(row);
                        }
                        return Ok(rows);
                    }
                    _ => return syntax(format!("unterminated environment {name}")),
                }
            }
        })?;
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        let aligns = match aligns {
            Some(list) if gaps == Gaps::Aligned => (0..columns)
                .map(|ix| {
                    list.get(ix).copied().unwrap_or(if ix % 2 == 0 {
                        Align::Right
                    } else {
                        Align::Left
                    })
                })
                .collect(),
            Some(list) if list.is_empty() => vec![Align::Center; columns],
            Some(list) => (0..columns)
                .map(|ix| {
                    list.get(ix)
                        .or(list.last())
                        .copied()
                        .unwrap_or(Align::Center)
                })
                .collect(),
            None => vec![Align::Center; columns],
        };
        let gap_list = (0..columns.saturating_sub(1))
            .map(|ix| match gaps {
                Gaps::Uniform => 1.0,
                Gaps::Cases => 1.0,
                Gaps::Aligned => {
                    if ix % 2 == 0 {
                        0.0
                    } else {
                        2.0
                    }
                }
            })
            .collect();
        let matrix = Matrix {
            rows,
            aligns,
            gaps: gap_list,
            level,
            open,
            close,
        };
        Ok(Node::Atom(Atom::new(
            if open.is_some() || close.is_some() {
                Class::Inner
            } else {
                Class::Ord
            },
            Kind::Matrix(Box::new(matrix)),
        )))
    }

    fn parse_colspec(&mut self) -> Result<Vec<Align>, MathError> {
        let spec = self.parse_text_raw()?;
        let mut aligns = Vec::new();
        for ch in spec.chars() {
            match ch {
                'l' => aligns.push(Align::Left),
                'c' => aligns.push(Align::Center),
                'r' => aligns.push(Align::Right),
                '|' | ' ' => {}
                other => return syntax(format!("unsupported column type {other}")),
            }
        }
        if aligns.is_empty() {
            return syntax("empty column specification");
        }
        Ok(aligns)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gaps {
    Uniform,
    Cases,
    Aligned,
}

fn frac_is_plain(atom: &Atom) -> bool {
    matches!(&atom.kind, Kind::Frac(frac) if frac.open.is_none() && frac.close.is_none())
}

fn is_prime_only(group: &Group) -> bool {
    !group.is_empty()
        && group.iter().all(|node| {
            matches!(
                node,
                Node::Atom(Atom {
                    kind: Kind::Char('\u{2032}'),
                    sup: None,
                    sub: None,
                    ..
                })
            )
        })
}

/// The atom that receives scripts: an empty ordinary atom stands in when the previous node is a
/// space or style switch.
fn script_target(node: &mut Node) -> &mut Atom {
    if !matches!(node, Node::Atom(_)) {
        let previous = std::mem::replace(node, Node::Space(0.0));
        *node = Node::Atom(Atom::new(Class::Ord, Kind::Group(vec![previous])));
    }
    match node {
        Node::Atom(atom) => atom,
        _ => unreachable!("replaced with an atom above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_commands_and_unbalanced_groups_are_errors() {
        assert!(matches!(parse(r"\nosuchthing"), Err(MathError::Syntax(_))));
        assert!(matches!(parse("{a"), Err(MathError::Syntax(_))));
        assert!(matches!(parse("a}"), Err(MathError::Syntax(_))));
        assert!(matches!(parse(r"\left( a"), Err(MathError::Syntax(_))));
        assert!(matches!(parse("x^1^2"), Err(MathError::Syntax(_))));
        assert!(matches!(parse("x_"), Err(MathError::Syntax(_))));
        assert!(parse(r"\frac{a}{b}").is_ok());
    }

    #[test]
    fn nesting_is_limited_to_sixty_four_levels() {
        let ok = format!("{}x{}", "{".repeat(64), "}".repeat(64));
        assert!(parse(&ok).is_ok());
        let deep = format!("{}x{}", "{".repeat(65), "}".repeat(65));
        assert_eq!(parse(&deep).unwrap_err(), MathError::TooDeep);
        let radicals = format!("{}x", r"\sqrt".repeat(200));
        assert_eq!(parse(&radicals).unwrap_err(), MathError::TooDeep);
    }

    #[test]
    fn environments_must_match() {
        assert!(parse(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}").is_ok());
        assert!(parse(r"\begin{pmatrix} a \end{bmatrix}").is_err());
        assert!(parse(r"\begin{pmatrix} a").is_err());
        assert!(parse(r"a & b").is_err());
    }
}
