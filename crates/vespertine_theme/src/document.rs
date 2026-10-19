//! Runtime-overridable component palettes and metrics (THM-002).
//!
//! A [`DocumentTheme`] is the one value that framework-drawn document components (code views,
//! diff views, rendered Markdown, text inputs) read every color, font family and layout number
//! from. It has a dark and a light base, thirteen color tokens, twenty-four syntax categories
//! ([`SyntaxPalette`]), two font families and a table of layout metrics ([`DocumentMetrics`]).
//! Any subset can be overridden at runtime from a keyed JSON value; unspecified or invalid
//! entries fall back to the selected base.
//!
//! Colors are parsed with the shared THM-001 parser, so overrides may use any supported CSS
//! color notation.
//!
//! # Override semantics
//!
//! * [`DocumentTheme::from_value`] starts from the dark base. A missing, null or structurally
//!   invalid value (not an object, or any recognized field of the wrong type) yields the plain
//!   dark base, never a partial application, and is reported once through `log`.
//! * `appearance: "light"` selects the light base before anything else is applied. Any other
//!   text leaves the dark base.
//! * Order: base, color tokens, `fontMono`/`fontSans`, `metrics`, `syntax`. An unparsable color
//!   leaves only that token unchanged. Invalid metric numbers keep their default.
//! * [`DocumentTheme::apply_override`] applies the same value on top of an existing theme.
//!   When the override has an `appearance` key it first resets the theme to the base of that
//!   appearance (`light` or, for any other text, dark); without the key the existing theme is
//!   kept as the starting point. A structurally invalid override leaves the theme untouched.
//! * The default colors of the syntax categories `variable`, `parameter`, `operator` and
//!   `punctuation` equal the base's `text` value at base creation and are not recomputed after
//!   a `text` override.

use std::hash::{DefaultHasher, Hash, Hasher};

use serde_json::{Map, Value};
use vespertine::{Hsla, ListState, Rgba, SharedString, css_color_to_rgba, oklch_to_rgba, rgb};

/// Which built-in base a [`DocumentTheme`] starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Appearance {
    /// The dark base (the default).
    #[default]
    Dark,
    /// The light base, selected by `appearance: "light"`.
    Light,
}

/// Default sans-serif family on Linux.
pub const DEFAULT_FONT_SANS: &str = "DejaVu Sans";
/// Default monospace family on Linux.
pub const DEFAULT_FONT_MONO: &str = "DejaVu Sans Mono";

const COLOR_KEYS: [&str; 13] = [
    "bg",
    "border",
    "text",
    "textMuted",
    "textFaint",
    "textDim",
    "accent",
    "caret",
    "codeText",
    "codeWash",
    "diffAdd",
    "diffDel",
    "diffHunkBg",
];

/// Generates the flat theme struct with its color token fields and key lookup.
macro_rules! document_theme {
    ($($field:ident => $key:literal,)*) => {
        /// The document theme value: base appearance, color tokens, fonts, syntax palette and
        /// metrics. Field names are the snake-case spelling of the public JSON keys.
        #[derive(Clone, Debug, PartialEq)]
        pub struct DocumentTheme {
            /// The base this theme was last reset to.
            pub appearance: Appearance,
            $(
                #[doc = concat!("Color token `", $key, "`.")]
                pub $field: Rgba,
            )*
            /// Sans-serif font family (`fontSans`).
            pub font_sans: SharedString,
            /// Monospace font family (`fontMono`).
            pub font_mono: SharedString,
            /// The twenty-four syntax category colors (`syntax`).
            pub syntax: SyntaxPalette,
            /// Layout metrics (`metrics`).
            pub metrics: DocumentMetrics,
        }

        impl DocumentTheme {
            fn color_token_mut(&mut self, key: &str) -> Option<&mut Rgba> {
                match key {
                    $($key => Some(&mut self.$field),)*
                    _ => None,
                }
            }

            /// The color token named by its public key (for example `"textMuted"`).
            pub fn color_token(&self, key: &str) -> Option<Rgba> {
                match key {
                    $($key => Some(self.$field),)*
                    _ => None,
                }
            }
        }
    };
}

document_theme! {
    bg => "bg",
    border => "border",
    text => "text",
    text_muted => "textMuted",
    text_faint => "textFaint",
    text_dim => "textDim",
    accent => "accent",
    caret => "caret",
    code_text => "codeText",
    code_wash => "codeWash",
    diff_add => "diffAdd",
    diff_del => "diffDel",
    diff_hunk_bg => "diffHunkBg",
}

/// Generates the syntax palette, its category enum and key tables.
macro_rules! syntax_palette {
    ($($variant:ident, $field:ident, $key:literal;)*) => {
        /// One of the twenty-four closed syntax categories.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum SyntaxCategory {
            $(#[doc = concat!("Category `", $key, "`.")] $variant,)*
        }

        impl SyntaxCategory {
            /// Every category in the order of the public key table.
            pub const ALL: [SyntaxCategory; 24] = [$(SyntaxCategory::$variant,)*];

            /// The public key of this category.
            pub const fn key(self) -> &'static str {
                match self {
                    $(SyntaxCategory::$variant => $key,)*
                }
            }

            /// Looks a category up by its public key.
            pub fn from_key(key: &str) -> Option<Self> {
                match key {
                    $($key => Some(SyntaxCategory::$variant),)*
                    _ => None,
                }
            }
        }

        /// Colors of the twenty-four syntax categories.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct SyntaxPalette {
            $(
                #[doc = concat!("Color of category `", $key, "`.")]
                pub $field: Rgba,
            )*
        }

        impl SyntaxPalette {
            /// The color of `category`.
            pub fn get(&self, category: SyntaxCategory) -> Rgba {
                match category {
                    $(SyntaxCategory::$variant => self.$field,)*
                }
            }

            fn get_mut(&mut self, category: SyntaxCategory) -> &mut Rgba {
                match category {
                    $(SyntaxCategory::$variant => &mut self.$field,)*
                }
            }
        }
    };
}

syntax_palette! {
    Comment, comment, "comment";
    Keyword, keyword, "keyword";
    String, string, "string";
    StringSpecial, string_special, "stringSpecial";
    Escape, escape, "escape";
    Number, number, "number";
    Boolean, boolean, "boolean";
    TypeName, type_name, "typeName";
    TypeBuiltin, type_builtin, "typeBuiltin";
    Constructor, constructor, "constructor";
    Function, function, "function";
    FunctionBuiltin, function_builtin, "functionBuiltin";
    MacroName, macro_name, "macroName";
    Property, property, "property";
    Constant, constant, "constant";
    Variable, variable, "variable";
    VariableSpecial, variable_special, "variableSpecial";
    Parameter, parameter, "parameter";
    Operator, operator, "operator";
    Punctuation, punctuation, "punctuation";
    Tag, tag, "tag";
    Attribute, attribute, "attribute";
    Label, label, "label";
    Invalid, invalid, "invalid";
}

/// Generates the metrics struct with scalar fields, defaults, key lookup and hashing.
macro_rules! document_metrics {
    ($($field:ident => $key:literal = $default:expr,)*) => {
        /// Layout metrics in logical pixels. Every metric is independently overridable and is
        /// part of the layout identity ([`DocumentMetrics::layout_hash`]).
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct DocumentMetrics {
            $(
                #[doc = concat!("Metric `", $key, "` (default ", stringify!($default), ").")]
                pub $field: f32,
            )*
            /// `mdHeadingSizes`: font size of heading tiers 1, 2, 3 and 4-to-6.
            pub md_heading_sizes: [f32; 4],
            /// `mdHeadingLineHeights`: line height of heading tiers 1, 2, 3 and 4-to-6.
            pub md_heading_line_heights: [f32; 4],
        }

        impl Default for DocumentMetrics {
            fn default() -> Self {
                Self {
                    $($field: $default,)*
                    md_heading_sizes: DEFAULT_HEADING_SIZES,
                    md_heading_line_heights: DEFAULT_HEADING_LINE_HEIGHTS,
                }
            }
        }

        impl DocumentMetrics {
            /// Public keys of the scalar metrics.
            pub const SCALAR_KEYS: &'static [&'static str] = &[$($key,)*];

            fn scalar_mut(&mut self, key: &str) -> Option<&mut f32> {
                match key {
                    $($key => Some(&mut self.$field),)*
                    _ => None,
                }
            }

            /// The scalar metric named by its public key (for example `"diffLineHeight"`).
            pub fn scalar(&self, key: &str) -> Option<f32> {
                match key {
                    $($key => Some(self.$field),)*
                    _ => None,
                }
            }

            fn hash_all(&self, hasher: &mut impl Hasher) {
                $(self.$field.to_bits().hash(hasher);)*
                self.md_heading_sizes.map(f32::to_bits).hash(hasher);
                self.md_heading_line_heights.map(f32::to_bits).hash(hasher);
            }
        }
    };
}

const DEFAULT_HEADING_SIZES: [f32; 4] = [19., 16., 15., 14.];
const DEFAULT_HEADING_LINE_HEIGHTS: [f32; 4] = [27., 24., 22., 22.];

document_metrics! {
    code_text_size => "codeTextSize" = 12.5,
    code_line_height => "codeLineHeight" = 18.,
    code_gutter_digit_width => "codeGutterDigitWidth" = 7.,
    code_gutter_padding_right => "codeGutterPaddingRight" = 12.,
    code_gutter_min_width => "codeGutterMinWidth" = 28.,
    diff_text_size => "diffTextSize" = 12.,
    diff_line_height => "diffLineHeight" = 21.,
    diff_file_header_height => "diffFileHeaderHeight" = 36.,
    diff_hunk_header_height => "diffHunkHeaderHeight" = 28.,
    diff_notice_height => "diffNoticeHeight" = 24.,
    diff_body_bottom_pad => "diffBodyBottomPad" = 8.,
    diff_gutter_width => "diffGutterWidth" = 36.,
    diff_marker_width => "diffMarkerWidth" = 28.,
    diff_accent_bar_width => "diffAccentBarWidth" = 3.,
    diff_row_padding_x => "diffRowPaddingX" = 16.,
    md_text_size => "mdTextSize" = 14.,
    md_line_height => "mdLineHeight" = 22.,
    md_block_gap => "mdBlockGap" = 12.,
    md_table_cell_padding => "mdTableCellPadding" = 12.,
    md_table_min_column_width => "mdTableMinColumnWidth" = 96.,
    md_table_min_column_content => "mdTableMinColumnContent" = 48.,
    md_inline_code_radius => "mdInlineCodeRadius" = 4.5,
    md_code_padding_x => "mdCodePaddingX" = 12.,
    md_code_padding_y => "mdCodePaddingY" = 10.,
    md_code_radius => "mdCodeRadius" = 10.,
    md_code_header_padding_y => "mdCodeHeaderPaddingY" = 5.,
    md_code_header_text_size => "mdCodeHeaderTextSize" = 11.,
}

impl DocumentMetrics {
    /// Public key of the heading font size array.
    pub const HEADING_SIZES_KEY: &'static str = "mdHeadingSizes";
    /// Public key of the heading line height array.
    pub const HEADING_LINE_HEIGHTS_KEY: &'static str = "mdHeadingLineHeights";

    /// Validates a metric number: after conversion to 32-bit float it must be finite and not
    /// negative (zero is accepted). Returns the accepted value (negative zero normalized).
    pub fn validate(value: f64) -> Option<f32> {
        let value = value as f32;
        (value.is_finite() && value >= 0.).then_some(value + 0.)
    }

    /// Heading level to tier (R7): levels 1, 2 and 3 select tiers 1, 2 and 3; levels 4 to 6
    /// select tier 4; levels outside 1..=6 are clamped first. Returns (font size, line height).
    pub fn heading_tier(&self, level: i32) -> (f32, f32) {
        let tier = (level.clamp(1, 6) - 1).min(3) as usize;
        (
            self.md_heading_sizes[tier],
            self.md_heading_line_heights[tier],
        )
    }

    /// Layout identity (R8): a hash of every metric, including every tier of both heading
    /// arrays. Components whose layout depends on theme numbers key their layout and caches by
    /// this value, so changing any single metric re-lays them out.
    pub fn layout_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash_all(&mut hasher);
        hasher.finish()
    }

    /// Row-height identity of a virtualized diff view (R8): a hash of only the five metrics
    /// that determine its row heights (`diffLineHeight`, `diffFileHeaderHeight`,
    /// `diffHunkHeaderHeight`, `diffNoticeHeight`, `diffBodyBottomPad`). Its measured-row
    /// cache and scroll anchor are keyed by this value alone.
    pub fn diff_row_height_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        [
            self.diff_line_height,
            self.diff_file_header_height,
            self.diff_hunk_header_height,
            self.diff_notice_height,
            self.diff_body_bottom_pad,
        ]
        .map(f32::to_bits)
        .hash(&mut hasher);
        hasher.finish()
    }

    /// Applies the `metrics` group (R5, R6). The group has already passed the structural check.
    fn apply_group(&mut self, group: &Map<String, Value>) {
        for (key, value) in group {
            if let Some(slot) = self.scalar_mut(key) {
                match value.as_f64().and_then(Self::validate) {
                    Some(valid) => *slot = valid,
                    None => {
                        log::warn!("theme metric `{key}` ignored: {value} is not finite and >= 0")
                    }
                }
                continue;
            }
            let slots = match key.as_str() {
                Self::HEADING_SIZES_KEY => &mut self.md_heading_sizes,
                Self::HEADING_LINE_HEIGHTS_KEY => &mut self.md_heading_line_heights,
                _ => continue,
            };
            let Some(entries) = value.as_array() else {
                continue;
            };
            for (tier, entry) in entries.iter().take(slots.len()).enumerate() {
                match entry.as_f64().and_then(Self::validate) {
                    Some(valid) => slots[tier] = valid,
                    None => log::warn!(
                        "theme metric `{key}` tier {} ignored: {entry} is not finite and >= 0",
                        tier + 1
                    ),
                }
            }
        }
    }
}

/// Palette values authored in OKLCH: lightness 0..1, chroma, hue in degrees.
fn oklch(lightness: f32, chroma: f32, hue: f32) -> Rgba {
    oklch_to_rgba(lightness, chroma, hue, 1.)
}

/// The achromatic OKLCH(L, 0, 0); not an HSL lightness.
fn neutral(lightness: f32) -> Rgba {
    oklch(lightness, 0., 0.)
}

fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// HSL with normalized hue, saturation and lightness (hue 0.6 = 216 degrees).
fn hsl(hue: f32, saturation: f32, lightness: f32, alpha: f32) -> Rgba {
    Rgba::from(Hsla {
        h: hue,
        s: saturation,
        l: lightness,
        a: alpha,
    })
}

/// Syntax softening (R10): HSL saturation multiplied by 0.72; hue, lightness and alpha kept.
fn soften(color: Rgba) -> Rgba {
    let mut hsla = Hsla::from(color);
    hsla.s *= 0.72;
    Rgba::from(hsla)
}

impl SyntaxPalette {
    /// Default palette for a base; `text` and `comment` are the base's values, `danger` the
    /// base's unsoftened danger color. The chromatic bases are shared by both appearances.
    fn base(text: Rgba, comment: Rgba, danger: Rgba) -> Self {
        let indigo = soften(oklch(0.673, 0.182, 276.935));
        let pink = soften(oklch(0.718, 0.202, 349.761));
        let emerald = soften(oklch(0.765, 0.177, 163.223));
        let amber = soften(oklch(0.828, 0.189, 84.429));
        let danger = soften(danger);
        Self {
            comment,
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
            invalid: danger,
        }
    }
}

impl DocumentTheme {
    /// The dark base (the default theme).
    pub fn dark() -> Self {
        let text = neutral(0.922);
        Self {
            appearance: Appearance::Dark,
            bg: rgb(0x060606),
            border: Rgba {
                r: 1.,
                g: 1.,
                b: 1.,
                a: 0.08,
            },
            text,
            text_muted: neutral(0.708),
            text_faint: neutral(0.556),
            text_dim: rgb(0x989898),
            accent: oklch(0.673, 0.182, 276.935),
            caret: oklch(0.673, 0.182, 276.935),
            code_text: oklch(0.811, 0.111, 293.571),
            code_wash: with_alpha(oklch(0.702, 0.183, 293.541), 0.12),
            diff_add: oklch(0.765, 0.177, 163.223),
            diff_del: oklch(0.704, 0.191, 22.216),
            diff_hunk_bg: hsl(0.6, 0.35, 0.6, 0.05),
            font_sans: DEFAULT_FONT_SANS.into(),
            font_mono: DEFAULT_FONT_MONO.into(),
            syntax: SyntaxPalette::base(text, neutral(0.60), oklch(0.704, 0.191, 22.216)),
            metrics: DocumentMetrics::default(),
        }
    }

    /// The light base.
    pub fn light() -> Self {
        let text = neutral(0.25);
        Self {
            appearance: Appearance::Light,
            bg: rgb(0xffffff),
            border: Rgba {
                r: 0.,
                g: 0.,
                b: 0.,
                a: 0.10,
            },
            text,
            text_muted: neutral(0.439),
            text_faint: neutral(0.535),
            text_dim: neutral(0.50),
            accent: oklch(0.511, 0.262, 276.966),
            caret: oklch(0.511, 0.262, 276.966),
            code_text: oklch(0.491, 0.27, 292.581),
            code_wash: with_alpha(oklch(0.541, 0.281, 293.009), 0.10),
            diff_add: oklch(0.596, 0.145, 163.225),
            diff_del: oklch(0.577, 0.245, 27.325),
            diff_hunk_bg: hsl(0.6, 0.35, 0.35, 0.07),
            font_sans: DEFAULT_FONT_SANS.into(),
            font_mono: DEFAULT_FONT_MONO.into(),
            syntax: SyntaxPalette::base(text, neutral(0.48), oklch(0.505, 0.213, 27.518)),
            metrics: DocumentMetrics::default(),
        }
    }

    /// The unmodified base for an appearance.
    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(),
            Appearance::Light => Self::light(),
        }
    }

    /// Builds a theme from an optional keyed value (R1..R6). A missing, null or structurally
    /// invalid value gives the dark base with no overrides.
    pub fn from_value(value: Option<&Value>) -> Self {
        let mut theme = Self::dark();
        if let Some(value) = value {
            if !value.is_null() && theme.apply_override(value).is_err() {
                return Self::dark();
            }
        }
        theme
    }

    /// Applies an override on top of this theme (see the module documentation). Returns
    /// `Err(StructurallyInvalid)` and leaves the theme unchanged when the value is not a keyed
    /// object or a recognized field has the wrong type; that case is logged once.
    pub fn apply_override(&mut self, value: &Value) -> Result<(), StructurallyInvalid> {
        let object = match check_structure(value) {
            Ok(object) => object,
            Err(reason) => {
                log::warn!("theme value ignored (structurally invalid): {reason}");
                return Err(StructurallyInvalid(reason));
            }
        };
        if let Some(appearance) = object.get("appearance").and_then(Value::as_str) {
            *self = Self::for_appearance(if appearance == "light" {
                Appearance::Light
            } else {
                Appearance::Dark
            });
        }
        for key in COLOR_KEYS {
            let Some(text) = object.get(key).and_then(Value::as_str) else {
                continue;
            };
            match css_color_to_rgba(text) {
                Some(color) => {
                    if let Some(slot) = self.color_token_mut(key) {
                        *slot = color;
                    }
                }
                None => log::warn!("theme color `{key}` ignored: `{text}` is not a valid color"),
            }
        }
        if let Some(family) = object.get("fontMono").and_then(Value::as_str) {
            self.font_mono = family.to_string().into();
        }
        if let Some(family) = object.get("fontSans").and_then(Value::as_str) {
            self.font_sans = family.to_string().into();
        }
        if let Some(group) = object.get("metrics").and_then(Value::as_object) {
            self.metrics.apply_group(group);
        }
        if let Some(group) = object.get("syntax").and_then(Value::as_object) {
            for category in SyntaxCategory::ALL {
                let Some(text) = group.get(category.key()).and_then(Value::as_str) else {
                    continue;
                };
                match css_color_to_rgba(text) {
                    Some(color) => *self.syntax.get_mut(category) = color,
                    None => log::warn!(
                        "theme syntax color `{}` ignored: `{text}` is not a valid color",
                        category.key()
                    ),
                }
            }
        }
        Ok(())
    }

    /// Like [`Self::apply_override`], returning the new theme; an invalid override returns the
    /// theme unchanged.
    pub fn with_override(mut self, value: &Value) -> Self {
        let _ = self.apply_override(value);
        self
    }

    /// Heading level to (font size, line height); see [`DocumentMetrics::heading_tier`].
    pub fn heading_tier(&self, level: i32) -> (f32, f32) {
        self.metrics.heading_tier(level)
    }

    /// Layout identity of all metrics; see [`DocumentMetrics::layout_hash`].
    pub fn layout_hash(&self) -> u64 {
        self.metrics.layout_hash()
    }

    /// Diff row-height identity; see [`DocumentMetrics::diff_row_height_hash`].
    pub fn diff_row_height_hash(&self) -> u64 {
        self.metrics.diff_row_height_hash()
    }
}

impl Default for DocumentTheme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Why a theme value was rejected as structurally invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructurallyInvalid(pub String);

impl std::fmt::Display for StructurallyInvalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StructurallyInvalid {}

/// R1: the value must be a keyed object and every recognized field must have its type.
fn check_structure(value: &Value) -> Result<&Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "the theme value is not a keyed object".to_string())?;
    let expect_text = |key: &str, value: &Value| {
        value
            .is_string()
            .then_some(())
            .ok_or_else(|| format!("`{key}` must be text"))
    };
    for key in ["appearance", "fontSans", "fontMono"]
        .into_iter()
        .chain(COLOR_KEYS)
    {
        if let Some(value) = object.get(key) {
            expect_text(key, value)?;
        }
    }
    if let Some(group) = object.get("syntax") {
        let group = group
            .as_object()
            .ok_or_else(|| "`syntax` must be a keyed object".to_string())?;
        for category in SyntaxCategory::ALL {
            if let Some(value) = group.get(category.key()) {
                expect_text(&format!("syntax.{}", category.key()), value)?;
            }
        }
    }
    if let Some(group) = object.get("metrics") {
        let group = group
            .as_object()
            .ok_or_else(|| "`metrics` must be a keyed object".to_string())?;
        for key in DocumentMetrics::SCALAR_KEYS {
            if let Some(value) = group.get(*key) {
                if !value.is_number() {
                    return Err(format!("metrics.{key} must be a number"));
                }
            }
        }
        for key in [
            DocumentMetrics::HEADING_SIZES_KEY,
            DocumentMetrics::HEADING_LINE_HEIGHTS_KEY,
        ] {
            if let Some(value) = group.get(key) {
                let entries = value
                    .as_array()
                    .ok_or_else(|| format!("metrics.{key} must be an array of numbers"))?;
                if entries.iter().take(4).any(|entry| !entry.is_number()) {
                    return Err(format!("metrics.{key} entries must be numbers"));
                }
            }
        }
    }
    Ok(object)
}

/// Remembers the layout identities a host last laid out with, so a changed theme can
/// invalidate the cached row model of a variable-height list (R8, R13).
///
/// A document host keeps one tracker next to its [`ListState`] and calls
/// [`Self::remeasure_on_layout_change`] (or, for a virtualized diff view whose measured heights
/// are keyed only by the five row-height metrics, [`Self::remeasure_on_diff_row_change`]) with
/// the current theme at the start of each render. A change of any other metric repaints
/// through the usual notify path but leaves a diff view's measured heights and scroll position
/// alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutIdentity {
    layout: Option<u64>,
    diff_rows: Option<u64>,
}

impl LayoutIdentity {
    /// A tracker that has seen no theme yet; the first sync records the identity without
    /// remeasuring.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the theme's layout identity; returns whether it differs from the previous one.
    pub fn layout_changed(&mut self, theme: &DocumentTheme) -> bool {
        let hash = theme.layout_hash();
        self.layout
            .replace(hash)
            .is_some_and(|previous| previous != hash)
    }

    /// Records the theme's diff row-height identity; returns whether it differs.
    pub fn diff_rows_changed(&mut self, theme: &DocumentTheme) -> bool {
        let hash = theme.diff_row_height_hash();
        self.diff_rows
            .replace(hash)
            .is_some_and(|previous| previous != hash)
    }

    /// Remeasures every row of `list` when any metric changed. Returns whether it did.
    pub fn remeasure_on_layout_change(&mut self, theme: &DocumentTheme, list: &ListState) -> bool {
        let changed = self.layout_changed(theme);
        if changed {
            list.remeasure();
        }
        changed
    }

    /// Remeasures every row of `list` when one of the five diff row-height metrics changed.
    /// Returns whether it did.
    pub fn remeasure_on_diff_row_change(
        &mut self,
        theme: &DocumentTheme,
        list: &ListState,
    ) -> bool {
        let changed = self.diff_rows_changed(theme);
        if changed {
            list.remeasure();
        }
        changed
    }
}
