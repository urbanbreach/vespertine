//! Theme mapping of token kinds to syntax palette categories and colors (TXT-007 R11).
//!
//! [`SyntaxColors`] is a neutral palette struct with the twenty-four THM-002 categories; the
//! theme crate's palette converts into it. Colors live outside the document cache, so changing
//! them recolors cached spans without any parse or lookup.

use super::kind::TokenKind;
use vespertine::{Hsla, rgb};

/// The twenty-four syntax categories of THM-002 R11, spelled as their theme keys.
pub const CATEGORY_KEYS: [&str; 24] = [
    "comment",
    "keyword",
    "string",
    "stringSpecial",
    "escape",
    "number",
    "boolean",
    "typeName",
    "typeBuiltin",
    "constructor",
    "function",
    "functionBuiltin",
    "macroName",
    "property",
    "constant",
    "variable",
    "variableSpecial",
    "parameter",
    "operator",
    "punctuation",
    "tag",
    "attribute",
    "label",
    "invalid",
];

impl TokenKind {
    /// The palette category key of this kind (TXT-007 R11; `embedded` uses `punctuation`).
    pub fn category_key(self) -> &'static str {
        match self {
            TokenKind::Comment => "comment",
            TokenKind::Keyword => "keyword",
            TokenKind::String => "string",
            TokenKind::StringSpecial => "stringSpecial",
            TokenKind::Escape => "escape",
            TokenKind::Number => "number",
            TokenKind::Boolean => "boolean",
            TokenKind::Type => "typeName",
            TokenKind::TypeBuiltin => "typeBuiltin",
            TokenKind::Constructor => "constructor",
            TokenKind::Function => "function",
            TokenKind::FunctionBuiltin => "functionBuiltin",
            TokenKind::Macro => "macroName",
            TokenKind::Property => "property",
            TokenKind::Constant => "constant",
            TokenKind::Variable => "variable",
            TokenKind::VariableSpecial => "variableSpecial",
            TokenKind::Parameter => "parameter",
            TokenKind::Operator => "operator",
            TokenKind::Punctuation | TokenKind::Embedded => "punctuation",
            TokenKind::Tag => "tag",
            TokenKind::Attribute => "attribute",
            TokenKind::Label => "label",
            TokenKind::Invalid => "invalid",
        }
    }
}

/// A syntax palette: one color per THM-002 category plus the plain foreground used for
/// unclassified text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxColors {
    /// Color of unclassified text and the fallback of every undefined category.
    pub plain: Hsla,
    pub comment: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub string_special: Hsla,
    pub escape: Hsla,
    pub number: Hsla,
    pub boolean: Hsla,
    pub type_name: Hsla,
    pub type_builtin: Hsla,
    pub constructor: Hsla,
    pub function: Hsla,
    pub function_builtin: Hsla,
    pub macro_name: Hsla,
    pub property: Hsla,
    pub constant: Hsla,
    pub variable: Hsla,
    pub variable_special: Hsla,
    pub parameter: Hsla,
    pub operator: Hsla,
    pub punctuation: Hsla,
    pub tag: Hsla,
    pub attribute: Hsla,
    pub label: Hsla,
    pub invalid: Hsla,
}

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

impl SyntaxColors {
    /// The THM-002 dark defaults: softened indigo, emerald, pink and amber, the neutral comment
    /// gray, softened danger, and the dark `text` color for variable, parameter, operator and
    /// punctuation. The plain foreground is the dark `text` color as well.
    pub fn dark() -> Self {
        let text = hex(0xE5E5E5);
        let indigo = hex(0x8F96ED);
        let emerald = hex(0x1EB787);
        let pink = hex(0xE679B4);
        let amber = hex(0xDBA924);
        Self {
            plain: text,
            comment: hex(0x808080),
            keyword: indigo,
            string: emerald,
            string_special: pink,
            escape: pink,
            number: amber,
            boolean: amber,
            type_name: amber,
            type_builtin: emerald,
            constructor: amber,
            function: indigo,
            function_builtin: pink,
            macro_name: pink,
            property: amber,
            constant: emerald,
            variable: text,
            variable_special: pink,
            parameter: text,
            operator: text,
            punctuation: text,
            tag: pink,
            attribute: amber,
            label: amber,
            invalid: hex(0xE9797C),
        }
    }

    /// The color of a palette category key (`"typeName"`, `"macroName"`, ...), or `None` for a
    /// key outside the closed set of twenty-four categories.
    pub fn category(&self, key: &str) -> Option<Hsla> {
        Some(match key {
            "comment" => self.comment,
            "keyword" => self.keyword,
            "string" => self.string,
            "stringSpecial" => self.string_special,
            "escape" => self.escape,
            "number" => self.number,
            "boolean" => self.boolean,
            "typeName" => self.type_name,
            "typeBuiltin" => self.type_builtin,
            "constructor" => self.constructor,
            "function" => self.function,
            "functionBuiltin" => self.function_builtin,
            "macroName" => self.macro_name,
            "property" => self.property,
            "constant" => self.constant,
            "variable" => self.variable,
            "variableSpecial" => self.variable_special,
            "parameter" => self.parameter,
            "operator" => self.operator,
            "punctuation" => self.punctuation,
            "tag" => self.tag,
            "attribute" => self.attribute,
            "label" => self.label,
            "invalid" => self.invalid,
            _ => return None,
        })
    }

    /// Overrides one category by key; an unknown key leaves the palette unchanged and returns
    /// `false`.
    pub fn set_category(&mut self, key: &str, color: Hsla) -> bool {
        let slot = match key {
            "comment" => &mut self.comment,
            "keyword" => &mut self.keyword,
            "string" => &mut self.string,
            "stringSpecial" => &mut self.string_special,
            "escape" => &mut self.escape,
            "number" => &mut self.number,
            "boolean" => &mut self.boolean,
            "typeName" => &mut self.type_name,
            "typeBuiltin" => &mut self.type_builtin,
            "constructor" => &mut self.constructor,
            "function" => &mut self.function,
            "functionBuiltin" => &mut self.function_builtin,
            "macroName" => &mut self.macro_name,
            "property" => &mut self.property,
            "constant" => &mut self.constant,
            "variable" => &mut self.variable,
            "variableSpecial" => &mut self.variable_special,
            "parameter" => &mut self.parameter,
            "operator" => &mut self.operator,
            "punctuation" => &mut self.punctuation,
            "tag" => &mut self.tag,
            "attribute" => &mut self.attribute,
            "label" => &mut self.label,
            "invalid" => &mut self.invalid,
            _ => return false,
        };
        *slot = color;
        true
    }

    /// Returns the palette with one category replaced (builder form of
    /// [`SyntaxColors::set_category`]; an unknown key changes nothing).
    pub fn with_category(mut self, key: &str, color: Hsla) -> Self {
        self.set_category(key, color);
        self
    }

    /// Returns the palette with another plain foreground.
    pub fn with_plain(mut self, plain: Hsla) -> Self {
        self.plain = plain;
        self
    }

    /// The color of a token kind through its palette category (TXT-007 R11).
    pub fn color_for(&self, kind: TokenKind) -> Hsla {
        self.category(kind.category_key()).unwrap_or(self.plain)
    }
}

impl Default for SyntaxColors {
    fn default() -> Self {
        Self::dark()
    }
}
