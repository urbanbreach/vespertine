//! The format-independent compiler: authored role candidates in, a complete hardened variant
//! and its report out (THM-004 R5).

use super::{AccentCandidate, Adjustment, Mapping, VariantReport};
use crate::{
    accent::derive_accent,
    builtin,
    color::{Color, harden_foreground, worst_contrast},
    theme::{
        Appearance, ColorTable, DEFAULT_FROST_TINT_ALPHA, IssueSeverity, SourceMetadata,
        SurfaceTreatment, SyntaxCategory, SyntaxMap, TerminalPalette, ThemeVariant,
        validate_variants,
    },
};
use std::collections::HashMap;

/// A semantic role an adapter supplies candidates for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[allow(missing_docs)]
pub enum Role {
    Background,
    Shell,
    Raised,
    Card,
    Dialog,
    Overlay,
    Input,
    Solid,
    OnSolid,
    Hover,
    Active,
    Border,
    BorderStrong,
    Text,
    TextMuted,
    TextFaint,
    Danger,
    Warning,
    Success,
    Cursor,
    DiffAdd,
    DiffDelete,
    DiffHunk,
    TerminalBackground,
    TerminalForeground,
    TerminalSelection,
}

impl Role {
    /// The semantic role name used in reports.
    pub fn name(self) -> &'static str {
        match self {
            Role::Background => "background",
            Role::Shell => "shell",
            Role::Raised => "raised",
            Role::Card => "card",
            Role::Dialog => "dialog",
            Role::Overlay => "overlay",
            Role::Input => "input",
            Role::Solid => "solid",
            Role::OnSolid => "onSolid",
            Role::Hover => "hover",
            Role::Active => "active",
            Role::Border => "border",
            Role::BorderStrong => "borderStrong",
            Role::Text => "text",
            Role::TextMuted => "textMuted",
            Role::TextFaint => "textFaint",
            Role::Danger => "danger",
            Role::Warning => "warning",
            Role::Success => "success",
            Role::Cursor => "cursor",
            Role::DiffAdd => "diffAdd",
            Role::DiffDelete => "diffDelete",
            Role::DiffHunk => "diffHunk",
            Role::TerminalBackground => "terminal.background",
            Role::TerminalForeground => "terminal.foreground",
            Role::TerminalSelection => "terminal.selection",
        }
    }
}

/// A color found in the source under a source key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    /// The source key.
    pub source_key: String,
    /// The parsed value.
    pub color: Color,
}

/// Accent candidates in preference order.
pub type AccentCandidates = Vec<Candidate>;

/// What an adapter hands to the compiler for one variant. Candidate lists hold only keys that
/// exist in the source, in preference order.
#[derive(Clone, Debug)]
pub struct RawVariant {
    /// Variant identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Family identifier.
    pub family_id: String,
    /// Detected appearance.
    pub appearance: Appearance,
    /// Candidates per role.
    pub roles: HashMap<Role, Vec<Candidate>>,
    /// Accent candidates.
    pub accent: AccentCandidates,
    /// Syntax colors with their source keys.
    pub syntax: Vec<(SyntaxCategory, Candidate)>,
    /// ANSI colors with their source keys.
    pub ansi: [Option<Candidate>; 16],
    /// Warnings the adapter already collected.
    pub warnings: Vec<String>,
    /// Provenance.
    pub source: SourceMetadata,
}

/// The fallback-detection rule of THM-004 R4: light when black contrasts more than white
/// against the editor background, otherwise dark; dark without a background.
pub fn detect_appearance_from_background(background: Option<Color>) -> Appearance {
    match background {
        Some(background)
            if Color::BLACK.contrast(background) > Color::WHITE.contrast(background) =>
        {
            Appearance::Light
        }
        _ => Appearance::Dark,
    }
}

struct Compiler {
    roles: HashMap<Role, Vec<Candidate>>,
    report: VariantReport,
}

impl Compiler {
    fn candidates(&self, role: Role) -> Vec<Candidate> {
        self.roles.get(&role).cloned().unwrap_or_default()
    }

    fn map(&mut self, source_key: &str, role: &str, value: Color) {
        self.report.mappings.push(Mapping {
            source_key: source_key.to_string(),
            role: role.to_string(),
            value: value.to_hex(),
        });
    }

    fn adjust(&mut self, role: &str, original: Color, resolved: Color, reason: String) {
        if original != resolved {
            self.report.adjustments.push(Adjustment {
                role: role.to_string(),
                original: original.to_hex(),
                resolved: resolved.to_hex(),
                reason,
            });
        }
    }

    /// First candidate of the role, or `default` (recorded as such).
    fn first(&mut self, role: Role, default: Color, how: &str) -> Color {
        match self.candidates(role).first() {
            Some(candidate) => {
                self.map(&candidate.source_key, role.name(), candidate.color);
                candidate.color
            }
            None => {
                self.map(how, role.name(), default);
                default
            }
        }
    }

    /// A translucent foundation surface is flattened over `base` and the change recorded.
    fn flatten(&mut self, role: &str, color: Color, base: Color) -> Color {
        if color.a == 255 {
            return color;
        }
        let flat = color.blend_over(base);
        self.adjust(
            role,
            color,
            flat,
            "translucent foundation surface flattened over the theme background".to_string(),
        );
        flat
    }

    /// A foundation surface: the first candidate flattened, or a derivation.
    fn surface(&mut self, role: Role, background: Color, derived: Color) -> Color {
        let color = self.first(role, derived, "(derived)");
        self.flatten(role.name(), color, background)
    }

    /// Raises a foreground to `minimum` against `backgrounds`: alternative source keys that
    /// already satisfy the minimum are preferred; otherwise the hue-preserving mix of THM-003
    /// R15. Adjustments are recorded only when the value changed.
    fn harden(
        &mut self,
        role: Role,
        default: Color,
        backgrounds: &[Color],
        minimum: f64,
        preferred: Option<Color>,
        against: &str,
    ) -> Color {
        let candidates = self.candidates(role);
        let first = candidates.first().map_or(default, |c| c.color);
        if worst_contrast(first, backgrounds) >= minimum {
            self.first(role, default, "(default)");
            return first;
        }
        for alternative in candidates.iter().skip(1) {
            if worst_contrast(alternative.color, backgrounds) >= minimum {
                self.map(&alternative.source_key, role.name(), alternative.color);
                self.adjust(
                    role.name(),
                    first,
                    alternative.color,
                    format!(
                        "alternative source key `{}` meets contrast {minimum:.1}:1 against {against}",
                        alternative.source_key
                    ),
                );
                return alternative.color;
            }
        }
        self.first(role, default, "(default)");
        let hardened = harden_foreground(first, backgrounds, minimum, preferred);
        self.adjust(
            role.name(),
            first,
            hardened,
            format!("raised to contrast {minimum:.1}:1 against {against}"),
        );
        hardened
    }
}

/// Compiles a raw variant into a complete variant and its report (THM-004 R5): translucent
/// surfaces flattened, foreground roles raised, status/cursor/strong-border colors raised to
/// 3:1 across background and shell, accent chosen and re-derived.
pub fn compile_variant(raw: RawVariant) -> (ThemeVariant, VariantReport) {
    let appearance = raw.appearance;
    let neutral = builtin::neutral_variant(appearance);
    let defaults = neutral.colors;
    let mut compiler = Compiler {
        roles: raw.roles,
        report: VariantReport {
            variant_id: raw.id.clone(),
            warnings: raw.warnings,
            ..VariantReport::default()
        },
    };
    compiler.report.accent_candidates = raw
        .accent
        .iter()
        .map(|candidate| AccentCandidate {
            source_key: candidate.source_key.clone(),
            value: candidate.color.to_hex(),
        })
        .collect();

    // Foundation surfaces.
    if compiler.candidates(Role::Background).is_empty() {
        compiler.report.warnings.push(
            "the source defines no editor background; the neutral default background is used"
                .to_string(),
        );
    }
    let background_raw = compiler.first(Role::Background, defaults.background, "(default)");
    let background = compiler.flatten("background", background_raw, defaults.background);
    let text_basis = compiler
        .candidates(Role::Text)
        .first()
        .map_or(defaults.text, |c| c.color.blend_over(background));
    let derive = |amount: f64| background.mix(text_basis, amount);
    let shell = compiler.surface(Role::Shell, background, derive(0.02));
    let raised = compiler.surface(Role::Raised, background, derive(0.05));
    let card = compiler.surface(Role::Card, background, derive(0.07));
    let dialog = compiler.surface(Role::Dialog, background, derive(0.06));
    let overlay = compiler.surface(Role::Overlay, background, derive(0.09));
    let input = compiler.surface(Role::Input, background, derive(0.04));
    let solid = compiler.surface(Role::Solid, background, text_basis);
    let surfaces = [background, shell, raised, card, dialog, overlay, input];

    // Foreground roles.
    let text = compiler.harden(
        Role::Text,
        defaults.text,
        &surfaces,
        4.5,
        None,
        "background, shell, raised, card, dialog, overlay and input",
    );
    let text_muted = compiler.harden(
        Role::TextMuted,
        text.mix(background, 0.35),
        &surfaces,
        4.5,
        Some(text),
        "background, shell, raised, card, dialog, overlay and input",
    );
    let text_faint = compiler.harden(
        Role::TextFaint,
        text.mix(background, 0.55),
        &surfaces,
        3.0,
        Some(text_muted),
        "background, shell, raised, card, dialog, overlay and input",
    );
    let on_solid = compiler.harden(
        Role::OnSolid,
        solid.best_on_color(),
        &[solid],
        4.5,
        Some(text),
        "solid",
    );

    // Borders and washes.
    let border = compiler.first(Role::Border, defaults.border, "(default)");
    let shell_pair = [background, shell];
    let border_strong = compiler.harden(
        Role::BorderStrong,
        text.with_alpha(0.30),
        &shell_pair,
        3.0,
        None,
        "background and shell",
    );
    let hover = compiler.first(Role::Hover, text.with_alpha(0.06), "(derived)");
    let active = compiler.first(Role::Active, text.with_alpha(0.10), "(derived)");

    // Status and cursor colors.
    let danger = compiler.harden(
        Role::Danger,
        defaults.danger,
        &shell_pair,
        3.0,
        None,
        "background and shell",
    );
    let warning = compiler.harden(
        Role::Warning,
        defaults.warning,
        &shell_pair,
        3.0,
        None,
        "background and shell",
    );
    let success = compiler.harden(
        Role::Success,
        defaults.success,
        &shell_pair,
        3.0,
        None,
        "background and shell",
    );
    let cursor = compiler.harden(
        Role::Cursor,
        text,
        &shell_pair,
        3.0,
        None,
        "background and shell",
    );
    let diff_add = compiler.first(Role::DiffAdd, defaults.diff_add, "(default)");
    let diff_delete = compiler.first(Role::DiffDelete, defaults.diff_delete, "(default)");
    let diff_hunk = compiler.first(Role::DiffHunk, defaults.diff_hunk, "(default)");

    // Accent: recorded candidates in order, then hue-preserving adjustment; roles re-derived.
    let accent_surfaces = [background, shell, raised, card];
    let accent_default = builtin::neutral_tokens(appearance).accent;
    if raw.accent.is_empty() {
        compiler.report.warnings.push(
            "the source defines no accent color; the neutral default accent is used".to_string(),
        );
        compiler.map("(default)", "accent.primary", accent_default);
    }
    let first_accent = raw.accent.first().map_or(accent_default, |c| c.color);
    let mut accent_primary = first_accent;
    if worst_contrast(first_accent, &accent_surfaces) >= 3.0 {
        if let Some(candidate) = raw.accent.first() {
            compiler.map(&candidate.source_key, "accent.primary", candidate.color);
        }
    } else if let Some(alternative) = raw
        .accent
        .iter()
        .skip(1)
        .find(|candidate| worst_contrast(candidate.color, &accent_surfaces) >= 3.0)
    {
        compiler.map(&alternative.source_key, "accent.primary", alternative.color);
        compiler.adjust(
            "accent.primary",
            first_accent,
            alternative.color,
            format!(
                "alternative accent candidate `{}` meets contrast 3.0:1 against background, shell, raised and card",
                alternative.source_key
            ),
        );
        accent_primary = alternative.color;
    } else {
        if let Some(candidate) = raw.accent.first() {
            compiler.map(&candidate.source_key, "accent.primary", candidate.color);
        }
        accent_primary = harden_foreground(first_accent, &accent_surfaces, 3.0, None);
        compiler.adjust(
            "accent.primary",
            first_accent,
            accent_primary,
            "raised to contrast 3.0:1 against background, shell, raised and card".to_string(),
        );
    }
    let accent = derive_accent(accent_primary, background, appearance);

    // Terminal.
    let terminal_background_raw = compiler.first(Role::TerminalBackground, background, "(derived)");
    let terminal_background =
        compiler.flatten("terminal.background", terminal_background_raw, background);
    let terminal_foreground = compiler.harden(
        Role::TerminalForeground,
        text,
        &[terminal_background],
        4.5,
        Some(text),
        "the terminal background",
    );
    let terminal_selection = compiler.first(Role::TerminalSelection, accent.selection, "(derived)");
    let neutral_ansi = builtin::neutral_ansi(appearance);
    let mut missing_ansi = Vec::new();
    let mut ansi = neutral_ansi;
    for (index, candidate) in raw.ansi.iter().enumerate() {
        match candidate {
            Some(candidate) => {
                ansi[index] = candidate.color;
                compiler.map(
                    &candidate.source_key,
                    &format!("terminal.ansi[{index}]"),
                    candidate.color,
                );
            }
            None => missing_ansi.push(index.to_string()),
        }
    }
    if !missing_ansi.is_empty() {
        compiler.report.warnings.push(format!(
            "terminal ANSI colors {} are not defined by the source; neutral defaults are used",
            missing_ansi.join(", ")
        ));
    }

    let mut syntax = SyntaxMap::new();
    for (category, candidate) in &raw.syntax {
        syntax.set(*category, candidate.color);
        compiler.map(
            &candidate.source_key,
            &format!("syntax.{category}"),
            candidate.color,
        );
    }

    let colors = ColorTable {
        background,
        shell,
        raised,
        card,
        dialog,
        overlay,
        input,
        solid,
        hover,
        active,
        border,
        border_strong,
        text,
        text_muted,
        text_faint,
        on_solid,
        danger,
        danger_muted: danger.with_alpha(0.16),
        warning,
        warning_muted: warning.with_alpha(0.16),
        success,
        success_muted: success.with_alpha(0.16),
        cursor,
        diff_add,
        diff_delete,
        diff_hunk,
    };
    let variant = ThemeVariant {
        id: raw.id,
        family_id: raw.family_id,
        name: raw.name,
        appearance,
        surface: SurfaceTreatment::Opaque,
        colors,
        accent,
        syntax,
        terminal: TerminalPalette {
            background: terminal_background,
            foreground: terminal_foreground,
            selection: terminal_selection,
            ansi,
        },
        source: raw.source,
        frost_tint_alpha: DEFAULT_FROST_TINT_ALPHA,
        curated: false,
    };
    let mut report = compiler.report;
    for issue in validate_variants(std::iter::once(&variant)) {
        if issue.category != crate::theme::IssueCategory::Contrast {
            continue;
        }
        match issue.severity {
            IssueSeverity::Error => report.errors.push(issue.message),
            IssueSeverity::Warning => report.warnings.push(issue.message),
        }
    }
    (variant, report)
}
