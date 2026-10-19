//! Neutral token kinds, their precedence and the scope-to-kind table (TXT-007 R6-R8).

/// A neutral, color-free token kind (TXT-007 R6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Comment = 0,
    Keyword = 1,
    String = 2,
    StringSpecial = 3,
    Escape = 4,
    Number = 5,
    Boolean = 6,
    Type = 7,
    TypeBuiltin = 8,
    Constructor = 9,
    Function = 10,
    FunctionBuiltin = 11,
    Macro = 12,
    Property = 13,
    Constant = 14,
    Variable = 15,
    VariableSpecial = 16,
    Parameter = 17,
    Operator = 18,
    Punctuation = 19,
    Tag = 20,
    Attribute = 21,
    Label = 22,
    Embedded = 23,
    Invalid = 24,
}

impl TokenKind {
    /// Every kind, in the order of TXT-007 R6.
    pub const ALL: [TokenKind; 25] = [
        TokenKind::Comment,
        TokenKind::Keyword,
        TokenKind::String,
        TokenKind::StringSpecial,
        TokenKind::Escape,
        TokenKind::Number,
        TokenKind::Boolean,
        TokenKind::Type,
        TokenKind::TypeBuiltin,
        TokenKind::Constructor,
        TokenKind::Function,
        TokenKind::FunctionBuiltin,
        TokenKind::Macro,
        TokenKind::Property,
        TokenKind::Constant,
        TokenKind::Variable,
        TokenKind::VariableSpecial,
        TokenKind::Parameter,
        TokenKind::Operator,
        TokenKind::Punctuation,
        TokenKind::Tag,
        TokenKind::Attribute,
        TokenKind::Label,
        TokenKind::Embedded,
        TokenKind::Invalid,
    ];

    /// The kebab-case name used by TXT-007 (`string-special`, `type-builtin`, ...).
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Comment => "comment",
            TokenKind::Keyword => "keyword",
            TokenKind::String => "string",
            TokenKind::StringSpecial => "string-special",
            TokenKind::Escape => "escape",
            TokenKind::Number => "number",
            TokenKind::Boolean => "boolean",
            TokenKind::Type => "type",
            TokenKind::TypeBuiltin => "type-builtin",
            TokenKind::Constructor => "constructor",
            TokenKind::Function => "function",
            TokenKind::FunctionBuiltin => "function-builtin",
            TokenKind::Macro => "macro",
            TokenKind::Property => "property",
            TokenKind::Constant => "constant",
            TokenKind::Variable => "variable",
            TokenKind::VariableSpecial => "variable-special",
            TokenKind::Parameter => "parameter",
            TokenKind::Operator => "operator",
            TokenKind::Punctuation => "punctuation",
            TokenKind::Tag => "tag",
            TokenKind::Attribute => "attribute",
            TokenKind::Label => "label",
            TokenKind::Embedded => "embedded",
            TokenKind::Invalid => "invalid",
        }
    }

    /// Precedence used to resolve overlapping or nested captures; higher wins (TXT-007 R6).
    pub fn precedence(self) -> u8 {
        match self {
            TokenKind::Invalid => 100,
            TokenKind::Escape => 95,
            TokenKind::Macro => 90,
            TokenKind::Property | TokenKind::Attribute => 85,
            TokenKind::FunctionBuiltin | TokenKind::TypeBuiltin | TokenKind::VariableSpecial => 80,
            TokenKind::StringSpecial | TokenKind::Constructor | TokenKind::Parameter => 75,
            TokenKind::Function
            | TokenKind::Type
            | TokenKind::Constant
            | TokenKind::Tag
            | TokenKind::Label => 70,
            TokenKind::Comment
            | TokenKind::Keyword
            | TokenKind::String
            | TokenKind::Number
            | TokenKind::Boolean => 60,
            TokenKind::Variable | TokenKind::Operator => 50,
            TokenKind::Punctuation | TokenKind::Embedded => 40,
        }
    }
}

/// The kind of a scope-name prefix of the TXT-007 R7 table.
fn prefix_kind(prefix: &str) -> Option<TokenKind> {
    Some(match prefix {
        "entity.other.attribute-name" => TokenKind::Attribute,
        "entity.name.struct" => TokenKind::Type,
        "support.type.property-name" => TokenKind::Property,
        "variable.language" => TokenKind::VariableSpecial,
        "constant.language.undefined" => TokenKind::Constant,
        "constant.numeric" => TokenKind::Number,
        "entity.name.function.macro" => TokenKind::Macro,
        "entity.name.class" => TokenKind::Type,
        "constant.character.escape" => TokenKind::Escape,
        "entity.name.label" => TokenKind::Label,
        "constant.language.boolean" => TokenKind::Boolean,
        "entity.name.macro" => TokenKind::Macro,
        "entity.name.constructor" => TokenKind::Constructor,
        "entity.name.trait" => TokenKind::Type,
        "meta.object-literal.key" => TokenKind::Property,
        "keyword.operator" => TokenKind::Operator,
        "support.function.builtin" => TokenKind::FunctionBuiltin,
        "meta.mapping.key" => TokenKind::Property,
        "support.function.macro" => TokenKind::Macro,
        "storage.modifier" => TokenKind::Keyword,
        "variable.other.property" => TokenKind::Property,
        "string.quoted.other" => TokenKind::StringSpecial,
        "variable.other.constant" => TokenKind::Constant,
        "support.constant" => TokenKind::Constant,
        "constant.language.none" => TokenKind::Constant,
        "support.function" => TokenKind::FunctionBuiltin,
        "constant.language.null" => TokenKind::Constant,
        "constant.other" => TokenKind::Constant,
        "support.class.builtin" => TokenKind::TypeBuiltin,
        "entity.name.enum" => TokenKind::Type,
        "entity.name.interface" => TokenKind::Type,
        "entity.name.tag" => TokenKind::Tag,
        "entity.name.namespace" => TokenKind::Type,
        "entity.name.type" => TokenKind::Type,
        "entity.name.function" => TokenKind::Function,
        "keyword.control" => TokenKind::Keyword,
        "variable.other.member" => TokenKind::Property,
        "string.unquoted" => TokenKind::StringSpecial,
        "constant.language.nil" => TokenKind::Constant,
        "meta.embedded" => TokenKind::Embedded,
        "string.interpolated" => TokenKind::StringSpecial,
        "string.escape" => TokenKind::Escape,
        "constant.character" => TokenKind::Escape,
        "string.regexp" => TokenKind::StringSpecial,
        "entity.name.section" => TokenKind::Label,
        "support.class" => TokenKind::TypeBuiltin,
        "variable.parameter" => TokenKind::Parameter,
        "support.macro" => TokenKind::Macro,
        "constant.language" => TokenKind::Boolean,
        "storage.type" => TokenKind::Keyword,
        "support.type" => TokenKind::TypeBuiltin,
        "punctuation" => TokenKind::Punctuation,
        "constructor" => TokenKind::Constructor,
        "variable" => TokenKind::Variable,
        "function" => TokenKind::Function,
        "operator" => TokenKind::Operator,
        "property" => TokenKind::Property,
        "constant" => TokenKind::Constant,
        "keyword" => TokenKind::Keyword,
        "storage" => TokenKind::Keyword,
        "boolean" => TokenKind::Boolean,
        "comment" => TokenKind::Comment,
        "invalid" => TokenKind::Invalid,
        "string" => TokenKind::String,
        "markup" => TokenKind::Embedded,
        "number" => TokenKind::Number,
        "label" => TokenKind::Label,
        "type" => TokenKind::Type,
        _ => return None,
    })
}

/// True when `scope` equals `prefix` or starts with `prefix` followed by a dot (TXT-007 R7).
pub fn scope_matches_prefix(scope: &str, prefix: &str) -> bool {
    scope
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// Scope-to-kind lookup (TXT-007 R7): the kind of the LONGEST table prefix matching `scope`, or
/// `None` when no prefix matches (language-root scopes such as `source.rust` match nothing).
pub fn kind_for_scope(scope: &str) -> Option<TokenKind> {
    // Dotted prefixes from the longest (the whole name) to the shortest; the first table hit
    // is the longest matching prefix.
    let mut end = scope.len();
    loop {
        if let Some(kind) = prefix_kind(&scope[..end]) {
            return Some(kind);
        }
        match scope[..end].rfind('.') {
            Some(dot) => end = dot,
            None => return None,
        }
    }
}

/// Kind of a token region from its scope stack, outermost first (TXT-007 R8 step 3): the
/// highest-precedence mapped kind, later (deeper) scopes winning ties.
pub fn kind_for_scope_stack<'a>(scopes: impl IntoIterator<Item = &'a str>) -> Option<TokenKind> {
    let mut best: Option<TokenKind> = None;
    for scope in scopes {
        if let Some(kind) = kind_for_scope(scope) {
            if best.is_none_or(|b| kind.precedence() >= b.precedence()) {
                best = Some(kind);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_decides() {
        assert_eq!(
            kind_for_scope("constant.language.boolean.json"),
            Some(TokenKind::Boolean)
        );
        assert_eq!(
            kind_for_scope("constant.language.null.json"),
            Some(TokenKind::Constant)
        );
        assert_eq!(
            kind_for_scope("support.type.property-name.json"),
            Some(TokenKind::Property)
        );
        assert_eq!(
            kind_for_scope("support.type.primitive.ts"),
            Some(TokenKind::TypeBuiltin)
        );
        assert_eq!(
            kind_for_scope("keyword.operator.assignment"),
            Some(TokenKind::Operator)
        );
        assert_eq!(
            kind_for_scope("entity.name.function.macro.rust"),
            Some(TokenKind::Macro)
        );
        assert_eq!(
            kind_for_scope("entity.name.function.rust"),
            Some(TokenKind::Function)
        );
    }

    #[test]
    fn prefix_requires_dot_boundary_and_roots_match_nothing() {
        assert_eq!(kind_for_scope("stringy"), None);
        assert_eq!(kind_for_scope("string"), Some(TokenKind::String));
        assert_eq!(kind_for_scope("source.rust"), None);
        assert_eq!(kind_for_scope("text.html.markdown"), None);
        assert!(scope_matches_prefix(
            "constant.language",
            "constant.language"
        ));
        assert!(!scope_matches_prefix(
            "constant.languages",
            "constant.language"
        ));
    }

    #[test]
    fn deeper_scope_wins_precedence_ties() {
        let stack = ["string.quoted.double", "comment.line"];
        assert_eq!(kind_for_scope_stack(stack), Some(TokenKind::Comment));
        let stack = ["comment.line", "string.quoted.double"];
        assert_eq!(kind_for_scope_stack(stack), Some(TokenKind::String));
        let stack = [
            "meta.mapping.key.json",
            "string.quoted.double",
            "punctuation.begin",
        ];
        assert_eq!(kind_for_scope_stack(stack), Some(TokenKind::Property));
        assert_eq!(kind_for_scope_stack(["source.rust"]), None);
    }

    #[test]
    fn twenty_five_kinds_with_the_spec_precedences() {
        assert_eq!(TokenKind::ALL.len(), 25);
        assert_eq!(TokenKind::Invalid.precedence(), 100);
        assert_eq!(TokenKind::Escape.precedence(), 95);
        assert_eq!(TokenKind::Macro.precedence(), 90);
        assert_eq!(TokenKind::Parameter.precedence(), 75);
        assert_eq!(TokenKind::Embedded.precedence(), 40);
        assert_eq!(TokenKind::StringSpecial.name(), "string-special");
    }
}
