//! Theme model (THM-003 R1-R4): variants, families, the registry and structural/contrast
//! validation (THM-004 R6).

use crate::{builtin, color::Color, selection::ThemeSelection};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};
use vespertine::{SharedString, WindowAppearance};

/// Identifier of the built-in dark variant.
pub const DEFAULT_DARK_ID: &str = "default-dark";
/// Identifier of the built-in light variant.
pub const DEFAULT_LIGHT_ID: &str = "default-light";
/// Identifier of the built-in family.
pub const DEFAULT_FAMILY_ID: &str = "default";

/// Light or dark.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// A light variant or effective appearance.
    Light,
    /// A dark variant or effective appearance.
    Dark,
}

impl Appearance {
    /// The built-in default variant identifier of this appearance.
    pub fn default_variant_id(self) -> &'static str {
        match self {
            Appearance::Dark => DEFAULT_DARK_ID,
            Appearance::Light => DEFAULT_LIGHT_ID,
        }
    }

    /// Whether this is the dark appearance.
    pub fn is_dark(self) -> bool {
        self == Appearance::Dark
    }
}

impl From<WindowAppearance> for Appearance {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
        }
    }
}

/// A variant's recommended (or a preference's resolved) surface treatment.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceTreatment {
    /// Solid surfaces.
    Opaque,
    /// Translucent surfaces over a blurred backdrop.
    Frosted,
}

/// The semantic color roles of THM-003 R2.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorTable {
    /// Primary text.
    pub text: Color,
    /// Muted text.
    pub text_muted: Color,
    /// Faint text.
    pub text_faint: Color,
    /// Text on `solid`.
    pub on_solid: Color,
    /// Text cursor.
    pub cursor: Color,
    /// Main content plane.
    pub background: Color,
    /// Surrounding chrome.
    pub shell: Color,
    /// Input surface.
    pub input: Color,
    /// Card surface.
    pub card: Color,
    /// Raised surface.
    pub raised: Color,
    /// Dialog surface.
    pub dialog: Color,
    /// Overlay surface.
    pub overlay: Color,
    /// Solid emphasis fill.
    pub solid: Color,
    /// Hover wash.
    pub hover: Color,
    /// Active wash.
    pub active: Color,
    /// Border.
    pub border: Color,
    /// Strong border.
    pub border_strong: Color,
    /// Danger status.
    pub danger: Color,
    /// Muted danger companion.
    pub danger_muted: Color,
    /// Success status.
    pub success: Color,
    /// Muted success companion.
    pub success_muted: Color,
    /// Warning status.
    pub warning: Color,
    /// Muted warning companion.
    pub warning_muted: Color,
    /// Diff hunk headers.
    pub diff_hunk: Color,
    /// Added diff lines.
    pub diff_add: Color,
    /// Deleted diff lines.
    pub diff_delete: Color,
}

impl ColorTable {
    /// The seven surfaces text must stay readable on (background, shell, raised, card, dialog,
    /// overlay, input).
    pub fn text_surfaces(&self) -> [Color; 7] {
        [
            self.background,
            self.shell,
            self.raised,
            self.card,
            self.dialog,
            self.overlay,
            self.input,
        ]
    }
}

/// The accent roles of THM-003 R2.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccentRoles {
    /// Glyph ramp: light, primary, deep.
    pub glyph: [Color; 3],
    /// Selection wash.
    pub selection: Color,
    /// Caret color.
    pub caret: Color,
    /// Activity indicator color.
    pub activity: Color,
    /// Primary accent.
    pub primary: Color,
    /// Translucent accent wash.
    pub wash: Color,
    /// Stronger accent used as a fill behind `on`.
    pub strong: Color,
    /// Foreground drawn on `strong`.
    pub on: Color,
}

/// Syntax categories: the 24 of THM-002 plus optional markup categories.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(missing_docs)]
pub enum SyntaxCategory {
    Comment = 0,
    Keyword = 1,
    String = 2,
    StringSpecial = 3,
    Escape = 4,
    Number = 5,
    Boolean = 6,
    TypeName = 7,
    TypeBuiltin = 8,
    Constructor = 9,
    Function = 10,
    FunctionBuiltin = 11,
    MacroName = 12,
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
    Invalid = 23,
    MarkupHeading = 24,
    MarkupRaw = 25,
    MarkupLink = 26,
    MarkupReference = 27,
    MarkupEmphasis = 28,
    MarkupStrong = 29,
}

impl SyntaxCategory {
    /// Every category: the 24 core categories followed by the markup categories.
    pub const ALL: [SyntaxCategory; 30] = [
        SyntaxCategory::Comment,
        SyntaxCategory::Keyword,
        SyntaxCategory::String,
        SyntaxCategory::StringSpecial,
        SyntaxCategory::Escape,
        SyntaxCategory::Number,
        SyntaxCategory::Boolean,
        SyntaxCategory::TypeName,
        SyntaxCategory::TypeBuiltin,
        SyntaxCategory::Constructor,
        SyntaxCategory::Function,
        SyntaxCategory::FunctionBuiltin,
        SyntaxCategory::MacroName,
        SyntaxCategory::Property,
        SyntaxCategory::Constant,
        SyntaxCategory::Variable,
        SyntaxCategory::VariableSpecial,
        SyntaxCategory::Parameter,
        SyntaxCategory::Operator,
        SyntaxCategory::Punctuation,
        SyntaxCategory::Tag,
        SyntaxCategory::Attribute,
        SyntaxCategory::Label,
        SyntaxCategory::Invalid,
        SyntaxCategory::MarkupHeading,
        SyntaxCategory::MarkupRaw,
        SyntaxCategory::MarkupLink,
        SyntaxCategory::MarkupReference,
        SyntaxCategory::MarkupEmphasis,
        SyntaxCategory::MarkupStrong,
    ];

    /// The camelCase name used in serialized forms.
    pub fn name(self) -> &'static str {
        match self {
            SyntaxCategory::Comment => "comment",
            SyntaxCategory::Keyword => "keyword",
            SyntaxCategory::String => "string",
            SyntaxCategory::StringSpecial => "stringSpecial",
            SyntaxCategory::Escape => "escape",
            SyntaxCategory::Number => "number",
            SyntaxCategory::Boolean => "boolean",
            SyntaxCategory::TypeName => "typeName",
            SyntaxCategory::TypeBuiltin => "typeBuiltin",
            SyntaxCategory::Constructor => "constructor",
            SyntaxCategory::Function => "function",
            SyntaxCategory::FunctionBuiltin => "functionBuiltin",
            SyntaxCategory::MacroName => "macroName",
            SyntaxCategory::Property => "property",
            SyntaxCategory::Constant => "constant",
            SyntaxCategory::Variable => "variable",
            SyntaxCategory::VariableSpecial => "variableSpecial",
            SyntaxCategory::Parameter => "parameter",
            SyntaxCategory::Operator => "operator",
            SyntaxCategory::Punctuation => "punctuation",
            SyntaxCategory::Tag => "tag",
            SyntaxCategory::Attribute => "attribute",
            SyntaxCategory::Label => "label",
            SyntaxCategory::Invalid => "invalid",
            SyntaxCategory::MarkupHeading => "markupHeading",
            SyntaxCategory::MarkupRaw => "markupRaw",
            SyntaxCategory::MarkupLink => "markupLink",
            SyntaxCategory::MarkupReference => "markupReference",
            SyntaxCategory::MarkupEmphasis => "markupEmphasis",
            SyntaxCategory::MarkupStrong => "markupStrong",
        }
    }
}

impl fmt::Display for SyntaxCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One color per syntax category. A category that is absent falls back to the neutral default
/// of the variant's appearance (see [`ThemeVariant::syntax_color`]).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SyntaxMap(BTreeMap<SyntaxCategory, Color>);

impl SyntaxMap {
    /// An empty map; every category falls back to the neutral default.
    pub fn new() -> Self {
        Self::default()
    }

    /// The color explicitly set for `category`, if any.
    pub fn get(&self, category: SyntaxCategory) -> Option<Color> {
        self.0.get(&category).copied()
    }

    /// Sets the color of `category`.
    pub fn set(&mut self, category: SyntaxCategory, color: Color) {
        self.0.insert(category, color);
    }

    /// Iterates explicitly set categories in category order.
    pub fn iter(&self) -> impl Iterator<Item = (SyntaxCategory, Color)> + '_ {
        self.0.iter().map(|(category, color)| (*category, *color))
    }
}

/// Terminal palette: background, foreground, selection and sixteen ANSI colors.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalPalette {
    /// ANSI colors: indices 0-7 normal, 8-15 bright.
    pub ansi: [Color; 16],
    /// Terminal foreground.
    pub foreground: Color,
    /// Terminal background.
    pub background: Color,
    /// Terminal selection.
    pub selection: Color,
}

/// Source metadata, kept apart from semantic token values (THM-004 R3).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SourceMetadata {
    /// Source URL or local path.
    pub url: String,
    /// License text.
    pub license: String,
    /// Format name (for example `vscode`).
    pub format: String,
    /// Hash of the source asset.
    pub asset_hash: String,
    /// Revision (`local` for user supplied files).
    pub revision: String,
}

impl SourceMetadata {
    /// Whether url, revision, license and asset hash are all non-empty.
    pub fn is_complete(&self) -> bool {
        !self.url.trim().is_empty()
            && !self.revision.trim().is_empty()
            && !self.license.trim().is_empty()
            && !self.asset_hash.trim().is_empty()
    }
}

/// Default base alpha of frosted card-like tints (THM-003 R17).
pub const DEFAULT_FROST_TINT_ALPHA: f64 = 0.40;

fn default_frost_tint_alpha() -> f64 {
    DEFAULT_FROST_TINT_ALPHA
}

/// One theme variant: a complete set of semantic roles for one appearance.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeVariant {
    /// Display name.
    pub name: String,
    /// Identifier, unique across the registry.
    pub id: String,
    /// Identifier of the family this variant belongs to.
    pub family_id: String,
    /// Recommended surface treatment.
    pub surface: SurfaceTreatment,
    /// Dark or light.
    pub appearance: Appearance,
    /// Semantic colors.
    pub colors: ColorTable,
    /// Accent roles.
    pub accent: AccentRoles,
    /// Syntax colors.
    pub syntax: SyntaxMap,
    /// Terminal palette.
    pub terminal: TerminalPalette,
    /// Source metadata.
    pub source: SourceMetadata,
    /// Base alpha of frosted tints (a theme token).
    #[serde(default = "default_frost_tint_alpha")]
    pub frost_tint_alpha: f64,
    /// Curated variants are used as authored; others are foreground-hardened (THM-003 R15).
    #[serde(skip)]
    pub curated: bool,
}

impl ThemeVariant {
    /// The color of a syntax category, falling back to the neutral default of this appearance.
    pub fn syntax_color(&self, category: SyntaxCategory) -> Color {
        self.syntax
            .get(category)
            .unwrap_or_else(|| builtin::neutral_syntax_color(self.appearance, category))
    }

    /// Foreground hardening (THM-003 R15). Curated variants are returned as authored.
    pub fn hardened(&self) -> ThemeVariant {
        let mut variant = self.clone();
        if !self.curated {
            harden_variant_foregrounds(&mut variant);
        }
        variant
    }

    /// Frosted tint coverage (THM-003 R17) using this variant's base alpha, text and muted text.
    pub fn frosted_alpha(&self, tint: Color) -> f64 {
        crate::accent::frosted_tint_alpha(
            tint,
            self.frost_tint_alpha,
            self.colors.text,
            self.colors.text_muted,
            self.appearance,
        )
    }
}

/// Applies the R15 rules to `variant` in place (text, textMuted, onSolid, terminal foreground).
pub fn harden_variant_foregrounds(variant: &mut ThemeVariant) {
    use crate::color::harden_foreground;
    let surfaces = variant.colors.text_surfaces();
    let text = harden_foreground(variant.colors.text, &surfaces, 4.5, None);
    variant.colors.text = text;
    variant.colors.text_muted =
        harden_foreground(variant.colors.text_muted, &surfaces, 4.5, Some(text));
    variant.colors.on_solid = harden_foreground(
        variant.colors.on_solid,
        &[variant.colors.solid],
        4.5,
        Some(text),
    );
    variant.terminal.foreground = harden_foreground(
        variant.terminal.foreground,
        &[variant.terminal.background],
        4.5,
        Some(text),
    );
}

/// A family groups related variants.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ThemeFamily {
    /// Family identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Variants in authored order.
    pub variants: Vec<ThemeVariant>,
}

impl ThemeFamily {
    /// Finds a variant by identifier.
    pub fn variant(&self, id: &str) -> Option<&ThemeVariant> {
        self.variants.iter().find(|variant| variant.id == id)
    }

    /// Validates the family on its own (THM-004 R6).
    pub fn validate(&self) -> Vec<ValidationIssue> {
        validate_variants(self.variants.iter())
    }
}

/// Typography roles of TXT-004 carried by the resolved theme (THM-003 R18). Families and
/// absolute sizes of the three roles, with the interface size as the root size in logical
/// pixels. [`crate::appearance::bind_typography`] keeps it in step with `vespertine::Typography`;
/// hosts may also feed it through [`crate::appearance::set_typography`]. A change advances the
/// style generation.
#[derive(Clone, PartialEq, Debug)]
pub struct TypographyRoles {
    /// Interface (sans) family.
    pub ui_family: SharedString,
    /// Interface size (root size, logical pixels).
    pub ui_size: f32,
    /// Code (mono) family.
    pub code_family: SharedString,
    /// Code size in logical pixels.
    pub code_size: f32,
    /// Terminal family.
    pub terminal_family: SharedString,
    /// Terminal size in logical pixels.
    pub terminal_size: f32,
}

impl TypographyRoles {
    /// The roles of the application's TXT-004 typography state: effective families (what
    /// renders) and the three sizes.
    pub fn from_typography(typography: &vespertine::Typography) -> Self {
        use vespertine::TypographyRole;
        Self {
            ui_family: typography.effective_family_name(TypographyRole::Interface),
            ui_size: typography.interface_size(),
            code_family: typography.effective_family_name(TypographyRole::Code),
            code_size: typography.code_size(),
            terminal_family: typography.effective_family_name(TypographyRole::Terminal),
            terminal_size: typography.terminal_size(),
        }
    }
}

impl Default for TypographyRoles {
    fn default() -> Self {
        Self {
            ui_family: "DejaVu Sans".into(),
            ui_size: 16.0,
            code_family: "DejaVu Sans Mono".into(),
            code_size: 12.5,
            terminal_family: "DejaVu Sans Mono".into(),
            terminal_size: 13.0,
        }
    }
}

/// The registry of theme families in stable order. The built-in neutral family is always first
/// and cannot be removed, so resolution always succeeds.
#[derive(Clone, Debug)]
pub struct ThemeRegistry {
    families: Vec<ThemeFamily>,
}

impl ThemeRegistry {
    /// A registry containing only the built-in family.
    pub fn builtin() -> Self {
        Self {
            families: vec![builtin::neutral_family()],
        }
    }

    /// Built-in family followed by `custom` families in the given order.
    pub fn with_custom(custom: impl IntoIterator<Item = ThemeFamily>) -> Self {
        let mut registry = Self::builtin();
        registry.families.extend(custom);
        registry
    }

    /// The active registry of the application (built-in plus installed custom families).
    pub fn active(cx: &vespertine::App) -> &ThemeRegistry {
        crate::appearance::registry(cx)
    }

    /// Families in registry order.
    pub fn families(&self) -> &[ThemeFamily] {
        &self.families
    }

    /// Looks up a variant by identifier (first match in registry order).
    pub fn variant(&self, id: &str) -> Option<&ThemeVariant> {
        self.families.iter().find_map(|family| family.variant(id))
    }

    /// Variants of one appearance in registry order.
    pub fn variants_for(&self, appearance: Appearance) -> Vec<&ThemeVariant> {
        self.families
            .iter()
            .flat_map(|family| family.variants.iter())
            .filter(|variant| variant.appearance == appearance)
            .collect()
    }

    /// The first variant of an appearance in registry order.
    pub fn first_variant(&self, appearance: Appearance) -> &ThemeVariant {
        self.variants_for(appearance)
            .into_iter()
            .next()
            .unwrap_or_else(|| self.fallback(appearance))
    }

    fn fallback(&self, appearance: Appearance) -> &ThemeVariant {
        self.variant(appearance.default_variant_id())
            .expect("the built-in family is always registered")
    }

    /// Resolves the selection of `appearance`. An identifier that is not registered resolves to
    /// the built-in default of the same appearance; the selection itself is not modified
    /// (THM-003 R9).
    pub fn resolve(&self, selection: &ThemeSelection, appearance: Appearance) -> &ThemeVariant {
        self.resolve_id(selection.variant_id(appearance), appearance)
    }

    /// Resolves a variant identifier with the same fallback as [`ThemeRegistry::resolve`].
    pub fn resolve_id(&self, id: &str, appearance: Appearance) -> &ThemeVariant {
        self.variant(id)
            .unwrap_or_else(|| self.fallback(appearance))
    }

    /// Validates every variant of the registry, including identifiers duplicated across
    /// families (THM-004 R6).
    pub fn validate(&self) -> Vec<ValidationIssue> {
        validate_variants(
            self.families
                .iter()
                .flat_map(|family| family.variants.iter()),
        )
    }
}

impl Default for ThemeRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Issue category.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueCategory {
    /// Identifier or provenance problem.
    Structural,
    /// Contrast problem.
    Contrast,
}

/// Issue severity.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    /// Informational; never blocks.
    Warning,
    /// A problem; only structural errors block installation.
    Error,
}

/// One validation finding.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// The variant the issue is about (may be empty for an empty identifier).
    pub variant_id: String,
    /// Category.
    pub category: IssueCategory,
    /// Severity.
    pub severity: IssueSeverity,
    /// Readable message.
    pub message: String,
}

impl ValidationIssue {
    /// Only structural errors block installation.
    pub fn is_blocking(&self) -> bool {
        self.category == IssueCategory::Structural && self.severity == IssueSeverity::Error
    }
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.variant_id, self.message)
    }
}

/// Formats a contrast message: `<role> contrast is <actual>:1; expected <minimum>:1`.
pub fn contrast_message(role: &str, actual: f64, minimum: f64) -> String {
    format!("{role} contrast is {actual:.2}:1; expected {minimum:.1}:1")
}

pub(crate) fn validate_variants<'a>(
    variants: impl Iterator<Item = &'a ThemeVariant>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for variant in variants {
        let issue = |category, severity, message: String| ValidationIssue {
            variant_id: variant.id.clone(),
            category,
            severity,
            message,
        };
        if variant.id.trim().is_empty() {
            issues.push(issue(
                IssueCategory::Structural,
                IssueSeverity::Error,
                "variant identifier is empty".to_string(),
            ));
        } else if !seen.insert(variant.id.as_str()) {
            issues.push(issue(
                IssueCategory::Structural,
                IssueSeverity::Error,
                format!("variant identifier `{}` is duplicated", variant.id),
            ));
        }
        for (field, value) in [
            ("source url", &variant.source.url),
            ("source revision", &variant.source.revision),
            ("source license", &variant.source.license),
            ("source asset hash", &variant.source.asset_hash),
        ] {
            if value.trim().is_empty() {
                issues.push(issue(
                    IssueCategory::Structural,
                    IssueSeverity::Error,
                    format!("provenance is incomplete: {field} is empty"),
                ));
            }
        }
        let colors = &variant.colors;
        let checks = [
            ("text", colors.text, colors.background, 4.5),
            ("textMuted", colors.text_muted, colors.background, 4.5),
            (
                "accent primary",
                variant.accent.primary,
                colors.background,
                3.0,
            ),
            ("on-accent", variant.accent.on, variant.accent.strong, 4.5),
            (
                "terminal foreground",
                variant.terminal.foreground,
                variant.terminal.background,
                4.5,
            ),
        ];
        for (role, foreground, background, minimum) in checks {
            let actual = foreground.contrast(background);
            if actual < minimum {
                issues.push(issue(
                    IssueCategory::Contrast,
                    IssueSeverity::Error,
                    contrast_message(role, actual, minimum),
                ));
            }
        }
        for (index, color) in variant.terminal.ansi.iter().enumerate() {
            if index == 0 || index == 8 {
                continue;
            }
            let actual = color.contrast(variant.terminal.background);
            if actual < 3.0 {
                issues.push(issue(
                    IssueCategory::Contrast,
                    IssueSeverity::Warning,
                    contrast_message(&format!("terminal ansi {index}"), actual, 3.0),
                ));
            }
        }
    }
    issues
}

/// Whether any issue blocks installation.
pub fn has_blocking_issue(issues: &[ValidationIssue]) -> bool {
    issues.iter().any(ValidationIssue::is_blocking)
}
