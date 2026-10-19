//! Symbol tables and mathematical alphabets of the LaTeX front end (CMP-012 R1).

use super::parse::Class;

/// The current math alphabet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Font {
    /// Math italic letters, upright digits and capital Greek (TeX's default).
    Italic,
    Roman,
    Bold,
    Sans,
    Mono,
    DoubleStruck,
    Script,
    Fraktur,
}

/// What a control sequence stands for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Symbol {
    Char(char, Class),
    /// A large operator; the flag says whether its limits sit above and below in display style.
    BigOp(char, bool),
    /// An operator name set upright; the flag as for [`Symbol::BigOp`].
    OpName(&'static str, bool),
    Space(f32),
    Accent {
        ch: char,
        wide: bool,
        over: bool,
    },
}

fn offset(base: u32, ch: char, first: char) -> char {
    char::from_u32(base + (ch as u32 - first as u32)).unwrap_or(ch)
}

fn exception(ch: char, table: &[(char, char)]) -> Option<char> {
    table
        .iter()
        .find(|(from, _)| *from == ch)
        .map(|(_, to)| *to)
}

/// Maps a character into the alphabet of `font` (identity where the alphabet has no letter).
pub(crate) fn apply_font(ch: char, font: Font) -> char {
    let upper = ch.is_ascii_uppercase();
    let lower = ch.is_ascii_lowercase();
    let digit = ch.is_ascii_digit();
    match font {
        Font::Roman => ch,
        Font::Italic => {
            if upper {
                offset(0x1D434, ch, 'A')
            } else if lower {
                if ch == 'h' {
                    '\u{210E}'
                } else {
                    offset(0x1D44E, ch, 'a')
                }
            } else if ('\u{3B1}'..='\u{3C9}').contains(&ch) {
                offset(0x1D6FC, ch, '\u{3B1}')
            } else {
                match ch {
                    '\u{3D1}' => '\u{1D717}',
                    '\u{3D5}' => '\u{1D719}',
                    '\u{3D6}' => '\u{1D71B}',
                    '\u{3F0}' => '\u{1D718}',
                    '\u{3F1}' => '\u{1D71A}',
                    '\u{3F5}' => '\u{1D716}',
                    '\u{2202}' => '\u{1D715}',
                    other => other,
                }
            }
        }
        Font::Bold => {
            if upper {
                offset(0x1D400, ch, 'A')
            } else if lower {
                offset(0x1D41A, ch, 'a')
            } else if digit {
                offset(0x1D7CE, ch, '0')
            } else {
                ch
            }
        }
        Font::Sans => {
            if upper {
                offset(0x1D5A0, ch, 'A')
            } else if lower {
                offset(0x1D5BA, ch, 'a')
            } else if digit {
                offset(0x1D7E2, ch, '0')
            } else {
                ch
            }
        }
        Font::Mono => {
            if upper {
                offset(0x1D670, ch, 'A')
            } else if lower {
                offset(0x1D68A, ch, 'a')
            } else if digit {
                offset(0x1D7F6, ch, '0')
            } else {
                ch
            }
        }
        Font::DoubleStruck => {
            if upper {
                exception(
                    ch,
                    &[
                        ('C', '\u{2102}'),
                        ('H', '\u{210D}'),
                        ('N', '\u{2115}'),
                        ('P', '\u{2119}'),
                        ('Q', '\u{211A}'),
                        ('R', '\u{211D}'),
                        ('Z', '\u{2124}'),
                    ],
                )
                .unwrap_or_else(|| offset(0x1D538, ch, 'A'))
            } else if lower {
                offset(0x1D552, ch, 'a')
            } else if digit {
                offset(0x1D7D8, ch, '0')
            } else {
                ch
            }
        }
        Font::Script => {
            if upper {
                exception(
                    ch,
                    &[
                        ('B', '\u{212C}'),
                        ('E', '\u{2130}'),
                        ('F', '\u{2131}'),
                        ('H', '\u{210B}'),
                        ('I', '\u{2110}'),
                        ('L', '\u{2112}'),
                        ('M', '\u{2133}'),
                        ('R', '\u{211B}'),
                    ],
                )
                .unwrap_or_else(|| offset(0x1D49C, ch, 'A'))
            } else if lower {
                exception(
                    ch,
                    &[('e', '\u{212F}'), ('g', '\u{210A}'), ('o', '\u{2134}')],
                )
                .unwrap_or_else(|| offset(0x1D4B6, ch, 'a'))
            } else {
                ch
            }
        }
        Font::Fraktur => {
            if upper {
                exception(
                    ch,
                    &[
                        ('C', '\u{212D}'),
                        ('H', '\u{210C}'),
                        ('I', '\u{2111}'),
                        ('R', '\u{211C}'),
                        ('Z', '\u{2128}'),
                    ],
                )
                .unwrap_or_else(|| offset(0x1D504, ch, 'A'))
            } else if lower {
                offset(0x1D51E, ch, 'a')
            } else {
                ch
            }
        }
    }
}

/// Class and mapped character of a literal source character.
pub(crate) fn char_symbol(ch: char, font: Font) -> (Class, char) {
    let class = match ch {
        '+' => Class::Bin,
        '-' => return (Class::Bin, '\u{2212}'),
        '*' => return (Class::Bin, '\u{2217}'),
        '=' | '<' | '>' | ':' => Class::Rel,
        '(' | '[' => Class::Open,
        ')' | ']' => Class::Close,
        ',' | ';' => Class::Punct,
        other => unicode_class(other),
    };
    (class, apply_font(ch, font))
}

/// Class of a character typed directly as Unicode.
fn unicode_class(ch: char) -> Class {
    match ch {
        '\u{2190}'..='\u{21FF}'
        | '\u{27F0}'..='\u{27FF}'
        | '\u{2208}'..='\u{220D}'
        | '\u{2243}'..='\u{2275}'
        | '\u{227A}'..='\u{228B}'
        | '\u{22A2}'..='\u{22A8}'
        | '\u{2A7D}'..='\u{2A7E}'
        | '\u{2AAF}'..='\u{2AB0}' => Class::Rel,
        '\u{00B1}'
        | '\u{00D7}'
        | '\u{00F7}'
        | '\u{2213}'
        | '\u{2227}'..='\u{222A}'
        | '\u{2295}'..='\u{2299}'
        | '\u{22C5}'
        | '\u{2218}'
        | '\u{2219}' => Class::Bin,
        '\u{27E8}' | '\u{2308}' | '\u{230A}' => Class::Open,
        '\u{27E9}' | '\u{2309}' | '\u{230B}' => Class::Close,
        _ => Class::Ord,
    }
}

/// Maps a symbol character of a command through the current font (Greek letters and `\partial`
/// follow the alphabet; operators and relations never change).
pub(crate) fn char_symbol_mapped(ch: char, class: Class, font: Font) -> (Class, char) {
    if class == Class::Ord && !ch.is_ascii() {
        (class, apply_font(ch, font))
    } else {
        (class, ch)
    }
}

use Class::{Bin, Close, Inner, Open, Ord as O, Punct, Rel};

const SYMBOLS: &[(&str, char, Class)] = &[
    // Lowercase Greek.
    ("alpha", '\u{3B1}', O),
    ("beta", '\u{3B2}', O),
    ("gamma", '\u{3B3}', O),
    ("delta", '\u{3B4}', O),
    ("epsilon", '\u{3F5}', O),
    ("varepsilon", '\u{3B5}', O),
    ("zeta", '\u{3B6}', O),
    ("eta", '\u{3B7}', O),
    ("theta", '\u{3B8}', O),
    ("vartheta", '\u{3D1}', O),
    ("iota", '\u{3B9}', O),
    ("kappa", '\u{3BA}', O),
    ("varkappa", '\u{3F0}', O),
    ("lambda", '\u{3BB}', O),
    ("mu", '\u{3BC}', O),
    ("nu", '\u{3BD}', O),
    ("xi", '\u{3BE}', O),
    ("omicron", '\u{3BF}', O),
    ("pi", '\u{3C0}', O),
    ("varpi", '\u{3D6}', O),
    ("rho", '\u{3C1}', O),
    ("varrho", '\u{3F1}', O),
    ("sigma", '\u{3C3}', O),
    ("varsigma", '\u{3C2}', O),
    ("tau", '\u{3C4}', O),
    ("upsilon", '\u{3C5}', O),
    ("phi", '\u{3D5}', O),
    ("varphi", '\u{3C6}', O),
    ("chi", '\u{3C7}', O),
    ("psi", '\u{3C8}', O),
    ("omega", '\u{3C9}', O),
    // Capital Greek (upright).
    ("Gamma", '\u{393}', O),
    ("Delta", '\u{394}', O),
    ("Theta", '\u{398}', O),
    ("Lambda", '\u{39B}', O),
    ("Xi", '\u{39E}', O),
    ("Pi", '\u{3A0}', O),
    ("Sigma", '\u{3A3}', O),
    ("Upsilon", '\u{3A5}', O),
    ("Phi", '\u{3A6}', O),
    ("Psi", '\u{3A8}', O),
    ("Omega", '\u{3A9}', O),
    // Ordinary symbols.
    ("infty", '\u{221E}', O),
    ("partial", '\u{2202}', O),
    ("nabla", '\u{2207}', O),
    ("ell", '\u{2113}', O),
    ("hbar", '\u{210F}', O),
    ("imath", '\u{1D6A4}', O),
    ("jmath", '\u{1D6A5}', O),
    ("emptyset", '\u{2205}', O),
    ("varnothing", '\u{2205}', O),
    ("forall", '\u{2200}', O),
    ("exists", '\u{2203}', O),
    ("nexists", '\u{2204}', O),
    ("neg", '\u{AC}', O),
    ("lnot", '\u{AC}', O),
    ("top", '\u{22A4}', O),
    ("bot", '\u{22A5}', O),
    ("angle", '\u{2220}', O),
    ("prime", '\u{2032}', O),
    ("aleph", '\u{2135}', O),
    ("Re", '\u{211C}', O),
    ("Im", '\u{2111}', O),
    ("wp", '\u{2118}', O),
    ("Box", '\u{25A1}', O),
    ("triangle", '\u{25B3}', O),
    ("vert", '|', O),
    ("lvert", '|', Open),
    ("rvert", '|', Close),
    ("|", '\u{2016}', O),
    ("Vert", '\u{2016}', O),
    ("lVert", '\u{2016}', Open),
    ("rVert", '\u{2016}', Close),
    ("S", '\u{A7}', O),
    ("P", '\u{B6}', O),
    ("checkmark", '\u{2713}', O),
    ("backslash", '\\', O),
    ("%", '%', O),
    ("&", '&', O),
    ("#", '#', O),
    ("_", '_', O),
    ("$", '$', O),
    ("{", '{', Open),
    ("}", '}', Close),
    ("lbrace", '{', Open),
    ("rbrace", '}', Close),
    ("lbrack", '[', Open),
    ("rbrack", ']', Close),
    ("langle", '\u{27E8}', Open),
    ("rangle", '\u{27E9}', Close),
    ("lfloor", '\u{230A}', Open),
    ("rfloor", '\u{230B}', Close),
    ("lceil", '\u{2308}', Open),
    ("rceil", '\u{2309}', Close),
    ("colon", ':', Punct),
    ("ldots", '\u{2026}', Inner),
    ("dots", '\u{2026}', Inner),
    ("cdots", '\u{22EF}', Inner),
    ("vdots", '\u{22EE}', O),
    ("ddots", '\u{22F1}', Inner),
    // Binary operators.
    ("pm", '\u{B1}', Bin),
    ("mp", '\u{2213}', Bin),
    ("times", '\u{D7}', Bin),
    ("div", '\u{F7}', Bin),
    ("cdot", '\u{22C5}', Bin),
    ("ast", '\u{2217}', Bin),
    ("star", '\u{22C6}', Bin),
    ("circ", '\u{2218}', Bin),
    ("bullet", '\u{2219}', Bin),
    ("wedge", '\u{2227}', Bin),
    ("land", '\u{2227}', Bin),
    ("vee", '\u{2228}', Bin),
    ("lor", '\u{2228}', Bin),
    ("cap", '\u{2229}', Bin),
    ("cup", '\u{222A}', Bin),
    ("oplus", '\u{2295}', Bin),
    ("ominus", '\u{2296}', Bin),
    ("otimes", '\u{2297}', Bin),
    ("oslash", '\u{2298}', Bin),
    ("odot", '\u{2299}', Bin),
    ("uplus", '\u{228E}', Bin),
    ("sqcap", '\u{2293}', Bin),
    ("sqcup", '\u{2294}', Bin),
    ("setminus", '\u{2216}', Bin),
    ("diamond", '\u{22C4}', Bin),
    ("wr", '\u{2240}', Bin),
    ("dagger", '\u{2020}', Bin),
    ("ddagger", '\u{2021}', Bin),
    ("amalg", '\u{2A3F}', Bin),
    ("triangleleft", '\u{25C1}', Bin),
    ("triangleright", '\u{25B7}', Bin),
    ("bigtriangleup", '\u{25B3}', Bin),
    ("boxplus", '\u{229E}', Bin),
    ("boxtimes", '\u{22A0}', Bin),
    // Relations.
    ("leq", '\u{2264}', Rel),
    ("le", '\u{2264}', Rel),
    ("geq", '\u{2265}', Rel),
    ("ge", '\u{2265}', Rel),
    ("neq", '\u{2260}', Rel),
    ("ne", '\u{2260}', Rel),
    ("equiv", '\u{2261}', Rel),
    ("approx", '\u{2248}', Rel),
    ("cong", '\u{2245}', Rel),
    ("sim", '\u{223C}', Rel),
    ("simeq", '\u{2243}', Rel),
    ("propto", '\u{221D}', Rel),
    ("in", '\u{2208}', Rel),
    ("notin", '\u{2209}', Rel),
    ("ni", '\u{220B}', Rel),
    ("subset", '\u{2282}', Rel),
    ("supset", '\u{2283}', Rel),
    ("subseteq", '\u{2286}', Rel),
    ("supseteq", '\u{2287}', Rel),
    ("nsubseteq", '\u{2288}', Rel),
    ("ll", '\u{226A}', Rel),
    ("gg", '\u{226B}', Rel),
    ("prec", '\u{227A}', Rel),
    ("succ", '\u{227B}', Rel),
    ("preceq", '\u{2AAF}', Rel),
    ("succeq", '\u{2AB0}', Rel),
    ("perp", '\u{22A5}', Rel),
    ("parallel", '\u{2225}', Rel),
    ("mid", '\u{2223}', Rel),
    ("vdash", '\u{22A2}', Rel),
    ("models", '\u{22A8}', Rel),
    ("asymp", '\u{224D}', Rel),
    ("doteq", '\u{2250}', Rel),
    ("bowtie", '\u{22C8}', Rel),
    ("sqsubset", '\u{228F}', Rel),
    ("sqsubseteq", '\u{2291}', Rel),
    ("lesssim", '\u{2272}', Rel),
    ("gtrsim", '\u{2273}', Rel),
    ("ncong", '\u{2247}', Rel),
    ("nsim", '\u{2241}', Rel),
    ("napprox", '\u{2249}', Rel),
    ("leqslant", '\u{2A7D}', Rel),
    ("geqslant", '\u{2A7E}', Rel),
    ("nleq", '\u{2270}', Rel),
    ("ngeq", '\u{2271}', Rel),
    ("nless", '\u{226E}', Rel),
    ("ngtr", '\u{226F}', Rel),
    ("triangleq", '\u{225C}', Rel),
    ("coloneqq", '\u{2254}', Rel),
    // Arrows.
    ("to", '\u{2192}', Rel),
    ("rightarrow", '\u{2192}', Rel),
    ("gets", '\u{2190}', Rel),
    ("leftarrow", '\u{2190}', Rel),
    ("leftrightarrow", '\u{2194}', Rel),
    ("Rightarrow", '\u{21D2}', Rel),
    ("Leftarrow", '\u{21D0}', Rel),
    ("Leftrightarrow", '\u{21D4}', Rel),
    ("implies", '\u{27F9}', Rel),
    ("impliedby", '\u{27F8}', Rel),
    ("iff", '\u{27FA}', Rel),
    ("mapsto", '\u{21A6}', Rel),
    ("longmapsto", '\u{27FC}', Rel),
    ("longrightarrow", '\u{27F6}', Rel),
    ("longleftarrow", '\u{27F5}', Rel),
    ("longleftrightarrow", '\u{27F7}', Rel),
    ("Longrightarrow", '\u{27F9}', Rel),
    ("Longleftarrow", '\u{27F8}', Rel),
    ("uparrow", '\u{2191}', Rel),
    ("downarrow", '\u{2193}', Rel),
    ("updownarrow", '\u{2195}', Rel),
    ("Uparrow", '\u{21D1}', Rel),
    ("Downarrow", '\u{21D3}', Rel),
    ("nearrow", '\u{2197}', Rel),
    ("searrow", '\u{2198}', Rel),
    ("swarrow", '\u{2199}', Rel),
    ("nwarrow", '\u{2196}', Rel),
    ("hookrightarrow", '\u{21AA}', Rel),
    ("hookleftarrow", '\u{21A9}', Rel),
    ("rightleftharpoons", '\u{21CC}', Rel),
    ("leadsto", '\u{21DD}', Rel),
];

const BIG_OPS: &[(&str, char, bool)] = &[
    ("sum", '\u{2211}', true),
    ("prod", '\u{220F}', true),
    ("coprod", '\u{2210}', true),
    ("int", '\u{222B}', false),
    ("iint", '\u{222C}', false),
    ("iiint", '\u{222D}', false),
    ("oint", '\u{222E}', false),
    ("bigcup", '\u{22C3}', true),
    ("bigcap", '\u{22C2}', true),
    ("bigvee", '\u{22C1}', true),
    ("bigwedge", '\u{22C0}', true),
    ("bigoplus", '\u{2A01}', true),
    ("bigotimes", '\u{2A02}', true),
    ("bigodot", '\u{2A00}', true),
    ("biguplus", '\u{2A04}', true),
    ("bigsqcup", '\u{2A06}', true),
];

const OP_NAMES: &[(&str, bool)] = &[
    ("sin", false),
    ("cos", false),
    ("tan", false),
    ("cot", false),
    ("sec", false),
    ("csc", false),
    ("arcsin", false),
    ("arccos", false),
    ("arctan", false),
    ("sinh", false),
    ("cosh", false),
    ("tanh", false),
    ("coth", false),
    ("log", false),
    ("ln", false),
    ("lg", false),
    ("exp", false),
    ("dim", false),
    ("ker", false),
    ("hom", false),
    ("deg", false),
    ("arg", false),
    ("det", true),
    ("gcd", true),
    ("inf", true),
    ("sup", true),
    ("lim", true),
    ("liminf", true),
    ("limsup", true),
    ("max", true),
    ("min", true),
    ("Pr", true),
];

/// Accent commands: combining character, whether the accent stretches, over or under.
const ACCENTS: &[(&str, char, bool, bool)] = &[
    ("hat", '\u{302}', false, true),
    ("widehat", '\u{302}', true, true),
    ("check", '\u{30C}', false, true),
    ("breve", '\u{306}', false, true),
    ("acute", '\u{301}', false, true),
    ("grave", '\u{300}', false, true),
    ("tilde", '\u{303}', false, true),
    ("widetilde", '\u{303}', true, true),
    ("bar", '\u{304}', false, true),
    ("vec", '\u{20D7}', false, true),
    ("dot", '\u{307}', false, true),
    ("ddot", '\u{308}', false, true),
    ("dddot", '\u{20DB}', false, true),
    ("mathring", '\u{30A}', false, true),
    ("overrightarrow", '\u{20D7}', true, true),
    ("overleftarrow", '\u{20D6}', true, true),
    ("overbrace", '\u{23DE}', true, true),
    ("underbrace", '\u{23DF}', true, false),
    ("underrightarrow", '\u{20EF}', true, false),
    ("underleftarrow", '\u{20EE}', true, false),
];

/// Looks a control sequence up.
pub(crate) fn command_symbol(name: &str) -> Option<Symbol> {
    if let Some(&(_, ch, class)) = SYMBOLS.iter().find(|(n, _, _)| *n == name) {
        return Some(Symbol::Char(ch, class));
    }
    if let Some(&(_, ch, limits)) = BIG_OPS.iter().find(|(n, _, _)| *n == name) {
        return Some(Symbol::BigOp(ch, limits));
    }
    if let Some(&(text, limits)) = OP_NAMES.iter().find(|(n, _)| *n == name) {
        return Some(Symbol::OpName(text, limits));
    }
    if let Some(&(_, ch, wide, over)) = ACCENTS.iter().find(|(n, _, _, _)| *n == name) {
        return Some(Symbol::Accent { ch, wide, over });
    }
    let space = match name {
        "," | "thinspace" => 3.0 / 18.0,
        ":" | "medspace" => 4.0 / 18.0,
        ";" | "thickspace" => 5.0 / 18.0,
        "!" | "negthinspace" => -3.0 / 18.0,
        "negmedspace" => -4.0 / 18.0,
        "negthickspace" => -5.0 / 18.0,
        "quad" => 1.0,
        "qquad" => 2.0,
        "enspace" => 0.5,
        " " => 1.0 / 3.0,
        _ => return None,
    };
    Some(Symbol::Space(space))
}

/// The character of a delimiter written after `\left`, `\right`, `\middle` or `\big`.
pub(crate) fn delimiter_char(name: &str) -> Option<char> {
    Some(match name {
        "(" => '(',
        ")" => ')',
        "[" | "\\lbrack" => '[',
        "]" | "\\rbrack" => ']',
        "\\{" | "\\lbrace" => '{',
        "\\}" | "\\rbrace" => '}',
        "|" | "\\vert" | "\\lvert" | "\\rvert" => '|',
        "\\|" | "\\Vert" | "\\lVert" | "\\rVert" => '\u{2016}',
        "/" => '/',
        "\\backslash" => '\\',
        "\\langle" | "<" => '\u{27E8}',
        "\\rangle" | ">" => '\u{27E9}',
        "\\lfloor" => '\u{230A}',
        "\\rfloor" => '\u{230B}',
        "\\lceil" => '\u{2308}',
        "\\rceil" => '\u{2309}',
        "\\uparrow" => '\u{2191}',
        "\\downarrow" => '\u{2193}',
        "\\updownarrow" => '\u{2195}',
        "\\Uparrow" => '\u{21D1}',
        "\\Downarrow" => '\u{21D3}',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabets_use_the_unicode_exceptions() {
        assert_eq!(apply_font('R', Font::DoubleStruck), '\u{211D}');
        assert_eq!(apply_font('A', Font::DoubleStruck), '\u{1D538}');
        assert_eq!(apply_font('B', Font::Script), '\u{212C}');
        assert_eq!(apply_font('h', Font::Italic), '\u{210E}');
        assert_eq!(apply_font('x', Font::Italic), '\u{1D465}');
        assert_eq!(apply_font('7', Font::Italic), '7');
        assert_eq!(apply_font('\u{3B1}', Font::Italic), '\u{1D6FC}');
        assert_eq!(apply_font('\u{3C9}', Font::Italic), '\u{1D714}');
        assert_eq!(apply_font('\u{393}', Font::Italic), '\u{393}');
    }

    #[test]
    fn every_listed_symbol_is_unique() {
        for (ix, (name, _, _)) in SYMBOLS.iter().enumerate() {
            assert!(
                SYMBOLS[ix + 1..].iter().all(|(other, _, _)| other != name),
                "{name}"
            );
        }
    }
}
