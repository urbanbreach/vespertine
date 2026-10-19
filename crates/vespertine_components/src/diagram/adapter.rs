//! The diagram renderer adapter: input limits, palette-derived theming, rendering Mermaid to SVG
//! with `merman`, panic containment and SVG post-processing (CMP-013 R2, R5, R6, R7).

use super::postprocess::restyle;
use merman::MermaidConfig;
use merman::svg::{HeadlessRenderer, SvgOutputPolicy, SvgPipelinePreset};
use serde_json::{Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use vespertine_theme::color::Color;
use vespertine_theme::document::{Appearance, DocumentTheme};

/// Source limit in bytes.
pub const MAX_SOURCE_BYTES: usize = 16 * 1024;
/// Source limit in lines.
pub const MAX_LINES: usize = 256;
/// Source limit in tokens (split at whitespace and at `;`, `>`, `{`, `}`).
pub const MAX_TOKENS: usize = 2048;
/// Generated SVG limit in bytes: 2 MiB.
pub const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;

/// Failure reason of an input over a limit.
pub const COMPLEXITY_LIMIT: &str = "Diagram exceeds preview complexity limit";
/// Failure reason of an oversized generated SVG.
pub const OUTPUT_LIMIT: &str = "Diagram output exceeds preview size limit";
/// Failure reason of a renderer panic.
pub const RENDER_PANIC: &str = "Diagram could not be rendered";

/// Renders are serialized by this global lock: exactly one diagram renders at a time.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

/// The notation families.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Family {
    /// `flowchart`, `flowchart-v2` and `graph`.
    Flowchart,
    /// `sequenceDiagram`.
    Sequence,
    /// `classDiagram`.
    Class,
    /// `stateDiagram`.
    State,
    /// `erDiagram`.
    EntityRelationship,
    /// `gantt`.
    Gantt,
    /// Any other first token; the renderer decides whether it is a diagram it knows.
    Other(String),
}

/// The family of a source: the first token of the first non-blank line that does not start with
/// `%%`, after an optional leading front-matter block (a line `---` up to the next line `---`).
pub fn detect_family(source: &str) -> Option<Family> {
    let mut lines = source.lines().peekable();
    // Front matter: only when the first non-blank line is `---`.
    while lines.peek().is_some_and(|line| line.trim().is_empty()) {
        lines.next();
    }
    if lines.peek().is_some_and(|line| line.trim() == "---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }
    let line = lines.find(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with("%%")
    })?;
    let token = line.split_whitespace().next()?;
    Some(match token {
        "flowchart" | "flowchart-v2" | "graph" => Family::Flowchart,
        "sequenceDiagram" => Family::Sequence,
        "classDiagram" | "classDiagram-v2" => Family::Class,
        "stateDiagram" | "stateDiagram-v2" => Family::State,
        "erDiagram" => Family::EntityRelationship,
        "gantt" => Family::Gantt,
        other => Family::Other(other.to_string()),
    })
}

/// Number of tokens of a source split at whitespace and at `;`, `>`, `{` and `}`.
pub fn count_tokens(source: &str) -> usize {
    source
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ';' | '>' | '{' | '}'))
        .filter(|token| !token.is_empty())
        .count()
}

/// Checks the input limits (before any rendering): at most 16384 bytes, 256 lines and 2048
/// tokens.
pub fn check_limits(source: &str) -> Result<(), String> {
    if source.len() > MAX_SOURCE_BYTES
        || source.lines().count() > MAX_LINES
        || count_tokens(source) > MAX_TOKENS
    {
        return Err(COMPLEXITY_LIMIT.to_string());
    }
    Ok(())
}

/// The theme tokens a palette derives from.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteTokens {
    /// Whether the appearance is dark.
    pub dark: bool,
    /// Background token.
    pub background: Color,
    /// Border token.
    pub border: Color,
    /// Strong-border token.
    pub border_strong: Color,
    /// Text token.
    pub text: Color,
    /// Muted-text token.
    pub text_muted: Color,
    /// Faint-text token.
    pub text_faint: Color,
    /// Accent color.
    pub accent: Color,
    /// The theme's sans family.
    pub font_sans: String,
}

impl PaletteTokens {
    /// The tokens of a document theme. A document theme has a single border token, which serves
    /// as both the border and the strong border; use the struct fields directly when a
    /// semantic theme distinguishes them.
    pub fn from_document_theme(theme: &DocumentTheme) -> PaletteTokens {
        PaletteTokens {
            dark: theme.appearance == Appearance::Dark,
            background: Color::from_rgba(theme.bg),
            border: Color::from_rgba(theme.border),
            border_strong: Color::from_rgba(theme.border),
            text: Color::from_rgba(theme.text),
            text_muted: Color::from_rgba(theme.text_muted),
            text_faint: Color::from_rgba(theme.text_faint),
            accent: Color::from_rgba(theme.accent),
            font_sans: theme.font_sans.to_string(),
        }
    }
}

/// The ink color of an appearance: white on a dark theme, black on a light one.
fn ink(dark: bool) -> Color {
    if dark { Color::WHITE } else { Color::BLACK }
}

/// The plate color: the background token blended with the ink color at alpha 0.035, flattened
/// to an opaque color.
pub fn plate_color(background: Color, dark: bool) -> Color {
    ink(dark).with_alpha(0.035).blend_over(background)
}

/// The renderer palette derived from the theme tokens (CMP-013 R6): every color is an opaque
/// color, composited over the named backing color.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// Whether the renderer's base theme is the dark one.
    pub dark: bool,
    /// Background token blended with ink at 0.035.
    pub plate: Color,
    /// The plate over the background.
    pub canvas: Color,
    /// Node fill: the plate blended with ink at 0.06 on dark themes, the background on light.
    pub node: Color,
    /// Group (cluster) fill: ink at 0.03 over the canvas.
    pub group: Color,
    /// Text over the node color.
    pub text: Color,
    /// Muted text over the canvas.
    pub label: Color,
    /// Faint text (lines) over the canvas.
    pub line: Color,
    /// Strong border over the canvas.
    pub border: Color,
    /// Border token (grid lines) over the canvas.
    pub grid: Color,
    /// Accent at alpha 0.6 over the canvas.
    pub accent_line: Color,
    /// Accent at alpha 0.12 over the node color.
    pub accent_wash: Color,
    /// The theme's sans family.
    pub font_family: String,
    /// Font size in px: 14.
    pub font_size: f32,
    /// Node spacing: 36.
    pub node_spacing: f32,
    /// Rank spacing: 40.
    pub rank_spacing: f32,
    /// Node padding along x: 18.
    pub padding_x: f32,
    /// Node padding along y: 10.
    pub padding_y: f32,
}

impl Palette {
    /// The palette of a document theme.
    pub fn from_theme(theme: &DocumentTheme) -> Palette {
        Palette::from_tokens(&PaletteTokens::from_document_theme(theme))
    }

    /// The palette of theme tokens.
    pub fn from_tokens(tokens: &PaletteTokens) -> Palette {
        let ink = ink(tokens.dark);
        let background = tokens.background.blend_over(Color::BLACK);
        let plate = plate_color(background, tokens.dark);
        let canvas = plate.blend_over(background);
        let node = if tokens.dark {
            ink.with_alpha(0.06).blend_over(plate)
        } else {
            background
        };
        Palette {
            dark: tokens.dark,
            plate,
            canvas,
            node,
            group: ink.with_alpha(0.03).blend_over(canvas),
            text: tokens.text.blend_over(node),
            label: tokens.text_muted.blend_over(canvas),
            line: tokens.text_faint.blend_over(canvas),
            border: tokens.border_strong.blend_over(canvas),
            grid: tokens.border.blend_over(canvas),
            accent_line: tokens.accent.with_alpha(0.6).blend_over(canvas),
            accent_wash: tokens.accent.with_alpha(0.12).blend_over(node),
            font_family: tokens.font_sans.clone(),
            font_size: 14.0,
            node_spacing: 36.0,
            rank_spacing: 40.0,
            padding_x: 18.0,
            padding_y: 10.0,
        }
    }

    /// A stable identity of everything that changes the generated SVG: two palettes with the
    /// same identity render identically.
    pub fn style_id(&self) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.dark.hash(&mut hasher);
        for color in [
            self.plate,
            self.canvas,
            self.node,
            self.group,
            self.text,
            self.label,
            self.line,
            self.border,
            self.grid,
            self.accent_line,
            self.accent_wash,
        ] {
            color.hash(&mut hasher);
        }
        self.font_family.hash(&mut hasher);
        hasher.finish()
    }

    /// The renderer configuration for a diagram family: the base theme follows the appearance
    /// and the palette fills the theme variables.
    pub fn mermaid_config(&self, family: &Family) -> Value {
        let hex = |color: Color| color.to_hex();
        // For Gantt diagrams the primary border color is the accent line.
        let primary_border = if *family == Family::Gantt {
            self.accent_line
        } else {
            self.border
        };
        let mut variables = serde_json::Map::new();
        let mut set = |key: &str, value: Value| {
            variables.insert(key.to_string(), value);
        };
        set("darkMode", Value::Bool(self.dark));
        set("background", Value::String(hex(self.canvas)));
        set("fontFamily", Value::String(self.font_family.clone()));
        set("fontSize", Value::String(format!("{}px", self.font_size)));
        set("primaryColor", Value::String(hex(self.node)));
        set("secondaryColor", Value::String(hex(self.group)));
        set("tertiaryColor", Value::String(hex(self.group)));
        set("primaryTextColor", Value::String(hex(self.text)));
        set("secondaryTextColor", Value::String(hex(self.text)));
        set("tertiaryTextColor", Value::String(hex(self.text)));
        set("primaryBorderColor", Value::String(hex(primary_border)));
        set("secondaryBorderColor", Value::String(hex(self.border)));
        set("tertiaryBorderColor", Value::String(hex(self.border)));
        set("textColor", Value::String(hex(self.text)));
        set("lineColor", Value::String(hex(self.line)));
        set("edgeLabelBackground", Value::String(hex(self.canvas)));
        set("clusterBkg", Value::String(hex(self.group)));
        set("clusterBorder", Value::String(hex(self.border)));
        set("labelTextColor", Value::String(hex(self.label)));
        set("titleColor", Value::String(hex(self.label)));
        set("gridColor", Value::String(hex(self.grid)));
        set("actorBkg", Value::String(hex(self.node)));
        set("actorBorder", Value::String(hex(self.border)));
        set("actorTextColor", Value::String(hex(self.text)));
        set("actorLineColor", Value::String(hex(self.line)));
        set("signalColor", Value::String(hex(self.line)));
        set("signalTextColor", Value::String(hex(self.text)));
        set("noteBkgColor", Value::String(hex(self.accent_wash)));
        set("noteBorderColor", Value::String(hex(self.accent_line)));
        set("noteTextColor", Value::String(hex(self.text)));
        set("activationBkgColor", Value::String(hex(self.accent_wash)));
        set(
            "activationBorderColor",
            Value::String(hex(self.accent_line)),
        );
        set("taskBkgColor", Value::String(hex(self.node)));
        set("taskBorderColor", Value::String(hex(primary_border)));
        set("taskTextColor", Value::String(hex(self.text)));
        set("taskTextLightColor", Value::String(hex(self.text)));
        set("taskTextDarkColor", Value::String(hex(self.text)));
        set("taskTextOutsideColor", Value::String(hex(self.text)));
        set("activeTaskBkgColor", Value::String(hex(self.accent_wash)));
        set(
            "activeTaskBorderColor",
            Value::String(hex(self.accent_line)),
        );
        set("doneTaskBkgColor", Value::String(hex(self.group)));
        set("doneTaskBorderColor", Value::String(hex(self.border)));
        set("sectionBkgColor", Value::String(hex(self.group)));
        set("altSectionBkgColor", Value::String(hex(self.canvas)));
        set("sectionBkgColor2", Value::String(hex(self.group)));
        set("todayLineColor", Value::String(hex(self.accent_line)));
        let spacing = json!({ "nodeSpacing": self.node_spacing, "rankSpacing": self.rank_spacing });
        json!({
            "theme": "base",
            "fontFamily": self.font_family,
            "fontSize": self.font_size,
            "themeVariables": Value::Object(variables),
            // Mermaid has one padding knob per diagram; the x padding is the one it applies
            // around labels.
            "flowchart": {
                "nodeSpacing": self.node_spacing,
                "rankSpacing": self.rank_spacing,
                "padding": self.padding_x,
                "htmlLabels": false
            },
            "state": spacing,
            "class": spacing,
        })
    }
}

/// A Mermaid renderer: source text and palette in, SVG out.
pub trait DiagramRenderer: Send + Sync {
    /// Renders `source` to an SVG string or reports why it cannot.
    fn render(&self, source: &str, palette: &Palette) -> Result<String, String>;
}

/// The `merman` renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct MermanRenderer;

impl DiagramRenderer for MermanRenderer {
    fn render(&self, source: &str, palette: &Palette) -> Result<String, String> {
        let family = detect_family(source).ok_or_else(|| "Empty diagram".to_string())?;
        let config = MermaidConfig::from_value(palette.mermaid_config(&family));
        let output = SvgOutputPolicy {
            preset: SvgPipelinePreset::ResvgSafe,
            ..SvgOutputPolicy::default()
        };
        let renderer = HeadlessRenderer::new()
            .with_site_config(config)
            .with_strict_parsing()
            .with_svg_pipeline(output.pipeline())
            .with_diagram_id("diagram");
        match renderer.render_svg_sync(source) {
            Ok(Some(svg)) => Ok(svg),
            Ok(None) => Err(match family {
                Family::Other(token) => format!("Unsupported diagram type: {token}"),
                _ => "No diagram found".to_string(),
            }),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Renders `source` through `renderer`: input limits are checked first, the render is
/// serialized by the global lock, a renderer panic is contained, an SVG over 2 MiB is rejected
/// and the result is restyled (CMP-013 R7).
pub fn render_with(
    renderer: &dyn DiagramRenderer,
    source: &str,
    palette: &Palette,
) -> Result<String, String> {
    check_limits(source)?;
    let result = {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        catch_unwind(AssertUnwindSafe(|| renderer.render(source, palette)))
    };
    let svg = match result {
        Ok(result) => result?,
        Err(_) => return Err(RENDER_PANIC.to_string()),
    };
    if svg.len() > MAX_SVG_BYTES {
        return Err(OUTPUT_LIMIT.to_string());
    }
    Ok(restyle(&svg, palette))
}

/// Renders `source` with `merman` and the palette: the SVG string or the failure reason.
pub fn render(source: &str, palette: &Palette) -> Result<String, String> {
    render_with(&MermanRenderer, source, palette)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_family_is_the_first_token_after_front_matter_and_comments() {
        assert_eq!(
            detect_family("flowchart TD\nA-->B"),
            Some(Family::Flowchart)
        );
        assert_eq!(detect_family("graph LR"), Some(Family::Flowchart));
        assert_eq!(
            detect_family("---\ntitle: x\n---\n%% note\n\n  sequenceDiagram\n"),
            Some(Family::Sequence)
        );
        assert_eq!(
            detect_family("%% c\nGANTT"),
            Some(Family::Other("GANTT".into()))
        );
        assert_eq!(detect_family("gantt\n"), Some(Family::Gantt));
        assert_eq!(detect_family("   \n%% only comment"), None);
        assert_eq!(detect_family("erDiagram"), Some(Family::EntityRelationship));
        assert_eq!(detect_family("stateDiagram-v2"), Some(Family::State));
        assert_eq!(detect_family("classDiagram"), Some(Family::Class));
    }

    #[test]
    fn tokens_split_at_whitespace_and_the_listed_characters() {
        assert_eq!(count_tokens("a b;c>d{e}f"), 6);
        assert_eq!(count_tokens("  \n\t "), 0);
        assert_eq!(count_tokens("A-->B"), 2);
    }
}
