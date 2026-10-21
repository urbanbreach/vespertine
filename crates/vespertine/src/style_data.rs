//! Serializable layout and interactive styling (ELM-006).
//!
//! A [`HostStyle`] is a validated, normalized CSS-like style description received as JSON. It
//! maps onto exactly the refinements and builder calls of the upstream styling API
//! ([`StyleRefinement`], hover, active and focus-visible refinements), so a style received as
//! data produces the same layout and pixels as the equivalent builder code.
//!
//! Parsing validates the whole description first ([`HostStyle::from_json`]); a wrong-typed known
//! property rejects the whole style with an error naming the operation, and nothing is applied.
//! The parsed form is normalized, so `PartialEq` is deep equality that ignores `null`/absent
//! properties and key order, and applying a style is idempotent.

use std::fmt;

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use vespertine_refineable::Refineable as _;

use crate::{
    AbsoluteLength, AlignContent, AlignItems, AlignSelf, BorderStyle, BoxShadow, ColorSpace,
    CornersRefinement, CursorStyle, DefiniteLength, Display, EdgesRefinement, Fill, FlexDirection,
    FlexWrap, FontWeight, GridTemplate, GridTemplateMinSize, Hsla, JustifyContent, Length,
    MouseBlocking, Outline, Overflow, Pixels, PointerEvents, Position, Rgba, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Style, StyleRefinement, Styled, TextAlign,
    TextOverflow, UnderlineStyle, Visibility, WhiteSpace, linear_color_stop, linear_gradient, px,
    relative,
};

/// The default thickness in logical pixels of the scrollbar thumb (CMP-031).
pub const DEFAULT_SCROLLBAR_WIDTH: Pixels = Pixels(6.);

/// A style description was rejected as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostStyleError {
    operation: String,
    message: String,
}

impl HostStyleError {
    fn new(operation: &str, message: String) -> Self {
        Self {
            operation: operation.to_owned(),
            message,
        }
    }

    /// The operation (for example `"setStyle"`) whose style was rejected.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// What was wrong with the style.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for HostStyleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: malformed style: {}", self.operation, self.message)
    }
}

impl std::error::Error for HostStyleError {}

type Parsed<T> = Result<T, HostStyleError>;

/// The shared THM-001 color parser: every color string of a style goes through here.
fn parse_style_color(text: &str) -> Option<Rgba> {
    crate::css_color_to_rgba(text)
}

/// Maps a CSS cursor keyword to the cursor it selects. Matching is case-sensitive, and the
/// diagonal resize keywords select the same cursors on every backend.
pub fn cursor_style_from_keyword(keyword: &str) -> Option<CursorStyle> {
    let cursor = match keyword {
        "default" | "auto" => CursorStyle::Arrow,
        "text" => CursorStyle::IBeam,
        "vertical-text" => CursorStyle::IBeamCursorForVerticalLayout,
        "pointer" => CursorStyle::PointingHand,
        "grab" => CursorStyle::OpenHand,
        "grabbing" | "move" | "all-scroll" => CursorStyle::ClosedHand,
        "crosshair" => CursorStyle::Crosshair,
        "context-menu" => CursorStyle::ContextualMenu,
        "copy" => CursorStyle::DragCopy,
        "alias" => CursorStyle::DragLink,
        "not-allowed" | "no-drop" => CursorStyle::OperationNotAllowed,
        "col-resize" => CursorStyle::ResizeColumn,
        "row-resize" => CursorStyle::ResizeRow,
        "ew-resize" => CursorStyle::ResizeLeftRight,
        "ns-resize" => CursorStyle::ResizeUpDown,
        "nesw-resize" | "ne-resize" | "sw-resize" => CursorStyle::ResizeUpRightDownLeft,
        "nwse-resize" | "nw-resize" | "se-resize" => CursorStyle::ResizeUpLeftDownRight,
        "e-resize" => CursorStyle::ResizeRight,
        "w-resize" => CursorStyle::ResizeLeft,
        "s-resize" => CursorStyle::ResizeDown,
        "n-resize" => CursorStyle::ResizeUp,
        _ => return None,
    };
    Some(cursor)
}

/// Resolves a font weight given as a number or a string (R14). Numbers clamp to 1..=1000;
/// strings are trimmed and matched case-insensitively against the weight names, then parsed as
/// numbers; anything else is 400.
fn font_weight_from_string(text: &str) -> FontWeight {
    let text = text.trim().to_ascii_lowercase();
    let weight = match text.as_str() {
        "normal" => 400.,
        "bold" => 700.,
        "thin" => 100.,
        "extralight" | "extra-light" => 200.,
        "light" => 300.,
        "medium" => 500.,
        "semibold" | "semi-bold" => 600.,
        "extrabold" | "extra-bold" => 800.,
        "black" => 900.,
        other => match other.parse::<f32>() {
            Ok(value) if value.is_finite() => value.clamp(1., 1000.),
            _ => 400.,
        },
    };
    FontWeight(weight)
}

/// One layer of a style: the base style, or one of the nested interactive state styles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleLayer {
    refinement: StyleRefinement,
    scrollbar_color: Option<Hsla>,
    scrollbar_width: Option<Pixels>,
}

impl StyleLayer {
    /// The layout, paint and text refinement this layer resolves to.
    pub fn refinement(&self) -> &StyleRefinement {
        &self.refinement
    }

    /// The `scrollbarColor` property: the color of the native scrollbar thumb (CMP-031).
    pub fn scrollbar_color(&self) -> Option<Hsla> {
        self.scrollbar_color
    }

    /// The `scrollbarWidth` property: the thumb thickness, [`DEFAULT_SCROLLBAR_WIDTH`] when absent.
    pub fn scrollbar_width(&self) -> Pixels {
        self.scrollbar_width.unwrap_or(DEFAULT_SCROLLBAR_WIDTH)
    }
}

/// A validated style description: a base style plus nested hover, active and focus-visible
/// styles, ready to be applied to any element.
#[derive(Clone, Debug, PartialEq)]
pub struct HostStyle {
    base: StyleLayer,
    hover: Option<StyleLayer>,
    active: Option<StyleLayer>,
    focus_visible: Option<StyleLayer>,
}

impl Default for HostStyle {
    /// The empty style, equal to a style description with no properties.
    fn default() -> Self {
        let mut base = StyleLayer::default();
        base.refinement.pointer_events = Some(PointerEvents::Surface {
            unparsed_fill: false,
        });
        Self {
            base,
            hover: None,
            active: None,
            focus_visible: None,
        }
    }
}

impl<'de> Deserialize<'de> for HostStyle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        HostStyle::from_json(&value, "style").map_err(serde::de::Error::custom)
    }
}

impl HostStyle {
    /// Validates and normalizes a style description received as JSON. `operation` names the
    /// operation the style belongs to and is carried by the error. `null` is the empty style.
    ///
    /// Unknown property names are ignored and `null` members are absent. A value of the wrong
    /// type for a known property rejects the whole style.
    pub fn from_json(value: &Value, operation: &str) -> Result<Self, HostStyleError> {
        let map = match value {
            Value::Null => return Ok(Self::default()),
            Value::Object(map) => map,
            _ => {
                return Err(HostStyleError::new(
                    operation,
                    "a style must be an object".into(),
                ));
            }
        };
        let reader = Reader {
            map,
            operation,
            scope: "",
        };
        let base = parse_layer(&reader, true)?;
        let state = |name: &str| -> Parsed<Option<StyleLayer>> {
            let scope = format!("{name}.");
            match reader.get(name) {
                None => Ok(None),
                Some(Value::Object(nested)) => {
                    let layer = parse_layer(
                        &Reader {
                            map: nested,
                            operation,
                            scope: &scope,
                        },
                        false,
                    )?;
                    Ok((layer != StyleLayer::default()).then_some(layer))
                }
                Some(_) => Err(reader.wrong_type(name, "an object")),
            }
        };
        Ok(Self {
            base,
            hover: state("hover")?,
            active: state("active")?,
            focus_visible: state("focusVisible")?,
        })
    }

    /// Replaces this style with the one described by `value`. Returns whether the style
    /// changed; an equal style changes nothing and needs no update or repaint. On error the
    /// current style is left untouched.
    pub fn replace(&mut self, value: &Value, operation: &str) -> Result<bool, HostStyleError> {
        let new = Self::from_json(value, operation)?;
        let changed = *self != new;
        if changed {
            *self = new;
        }
        Ok(changed)
    }

    /// The base style layer.
    pub fn base(&self) -> &StyleLayer {
        &self.base
    }

    /// The nested `hover` style.
    pub fn hover(&self) -> Option<&StyleLayer> {
        self.hover.as_ref()
    }

    /// The nested `active` style.
    pub fn active(&self) -> Option<&StyleLayer> {
        self.active.as_ref()
    }

    /// The nested `focusVisible` style.
    pub fn focus_visible(&self) -> Option<&StyleLayer> {
        self.focus_visible.as_ref()
    }

    /// The base style as a refinement.
    pub fn to_refinement(&self) -> StyleRefinement {
        self.base.refinement.clone()
    }

    /// Refines `style` with the base style (not the nested states). Idempotent.
    pub fn apply_to_style(&self, style: &mut StyleRefinement) {
        style.refine(&self.base.refinement);
    }

    /// Applies the base style and the nested hover, active and focus-visible styles natively to
    /// an element, replacing any state styles set before. Applying a style twice gives the same
    /// result as applying it once. Hover and active require the element identity that
    /// [`StatefulInteractiveElement`] implies; focus-visible requires the element to be focusable.
    pub fn apply<E: Styled + StatefulInteractiveElement>(&self, mut element: E) -> E {
        self.apply_to_style(element.style());
        let interactivity = element.interactivity();
        interactivity.hover_style = self
            .hover
            .as_ref()
            .map(|layer| Box::new(layer.refinement.clone()));
        interactivity.active_style = self
            .active
            .as_ref()
            .map(|layer| Box::new(layer.refinement.clone()));
        interactivity.focus_visible_style = self
            .focus_visible
            .as_ref()
            .map(|layer| Box::new(layer.refinement.clone()));
        element
    }

    /// Whether this style blocks mouse hits behind it, and whether the wheel is blocked too
    /// (R17). This is the host-independent form of the policy the element applies when it
    /// inserts its hitbox.
    pub fn mouse_blocking(&self) -> MouseBlocking {
        let mut style = Style::default();
        style.refine(&self.base.refinement);
        style.mouse_blocking().unwrap_or(MouseBlocking::Passive)
    }
}

/// Reports whether the style described by `value` blocks mouse hits (R17), without needing a
/// host. See [`HostStyle::mouse_blocking`].
pub fn style_json_mouse_blocking(
    value: &Value,
    operation: &str,
) -> Result<MouseBlocking, HostStyleError> {
    Ok(HostStyle::from_json(value, operation)?.mouse_blocking())
}

struct Reader<'a> {
    map: &'a Map<String, Value>,
    operation: &'a str,
    scope: &'a str,
}

impl Reader<'_> {
    fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key).filter(|value| !value.is_null())
    }

    fn wrong_type(&self, key: &str, expected: &str) -> HostStyleError {
        HostStyleError::new(
            self.operation,
            format!("property `{}{key}` must be {expected}", self.scope),
        )
    }

    fn number(&self, key: &str) -> Parsed<Option<f64>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Number(number)) => Ok(number.as_f64()),
            Some(_) => Err(self.wrong_type(key, "a number")),
        }
    }

    fn string(&self, key: &str) -> Parsed<Option<&str>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text)),
            Some(_) => Err(self.wrong_type(key, "a string")),
        }
    }

    fn boolean(&self, key: &str) -> Parsed<Option<bool>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(self.wrong_type(key, "a boolean")),
        }
    }

    fn color(&self, key: &str) -> Parsed<Option<Option<Rgba>>> {
        Ok(self.string(key)?.map(parse_style_color))
    }

    /// A size grammar: a number of pixels, `"auto"`, a percentage or a numeric string.
    /// `snap_full` maps percentages of at least 99.9% to the full parent size.
    fn dimension(&self, key: &str, snap_full: bool) -> Parsed<Option<Length>> {
        let invalid =
            || self.wrong_type(key, "a number, \"auto\", a percentage or a numeric string");
        match self.get(key) {
            None => Ok(None),
            Some(Value::Number(number)) => Ok(number.as_f64().map(|value| px(value as f32).into())),
            Some(Value::String(text)) => {
                let text = text.trim();
                if text == "auto" {
                    return Ok(Some(Length::Auto));
                }
                if let Some(percent) = text.strip_suffix('%') {
                    let percent: f64 = percent.trim().parse().map_err(|_| invalid())?;
                    if !percent.is_finite() {
                        return Err(invalid());
                    }
                    return Ok(Some(if snap_full && percent >= 99.9 - 1e-9 {
                        relative(1.).into()
                    } else {
                        relative((percent / 100.) as f32).into()
                    }));
                }
                let pixels: f64 = text.parse().map_err(|_| invalid())?;
                if !pixels.is_finite() {
                    return Err(invalid());
                }
                Ok(Some(px(pixels as f32).into()))
            }
            Some(_) => Err(invalid()),
        }
    }

    /// Reads a shorthand and four longhands of one family, shorthand first.
    fn four_sides(&self, shorthand: &str, longhands: [&str; 4]) -> Parsed<[Option<f64>; 4]> {
        let all = self.number(shorthand)?;
        let mut sides = [all; 4];
        for (side, name) in sides.iter_mut().zip(longhands) {
            if let Some(value) = self.number(name)? {
                *side = Some(value);
            }
        }
        Ok(sides)
    }
}

fn pixels(value: f64) -> Pixels {
    px(value as f32)
}

fn parse_layer(reader: &Reader, is_base: bool) -> Parsed<StyleLayer> {
    let mut layer = StyleLayer::default();
    let r = &mut layer.refinement;

    // Layout.
    match reader.string("display")? {
        Some("flex") => r.display = Some(Display::Flex),
        Some("grid") => r.display = Some(Display::Grid),
        _ => {}
    }
    if reader.string("visibility")? == Some("hidden") {
        r.visibility = Some(Visibility::Hidden);
    }
    match reader.string("flexDirection")? {
        Some("row") => r.flex_direction = Some(FlexDirection::Row),
        Some("column") => r.flex_direction = Some(FlexDirection::Column),
        _ => {}
    }
    match reader.string("flexWrap")? {
        Some("wrap") => r.flex_wrap = Some(FlexWrap::Wrap),
        Some("wrap-reverse") => r.flex_wrap = Some(FlexWrap::WrapReverse),
        Some("nowrap") => r.flex_wrap = Some(FlexWrap::NoWrap),
        _ => {}
    }
    if let Some(value) = reader.number("flexGrow")? {
        r.flex_grow = Some(value as f32);
    }
    if let Some(value) = reader.number("flexShrink")? {
        r.flex_shrink = Some(value as f32);
    }
    if let Some(value) = reader.number("flexBasis")? {
        r.flex_basis = Some(pixels(value).into());
    }
    match reader.string("alignItems")? {
        Some("center") => r.align_items = Some(AlignItems::Center),
        Some("start" | "flex-start") => r.align_items = Some(AlignItems::FlexStart),
        Some("end" | "flex-end") => r.align_items = Some(AlignItems::FlexEnd),
        Some("stretch") => r.align_items = Some(AlignItems::Stretch),
        Some("baseline") => r.align_items = Some(AlignItems::Baseline),
        _ => {}
    }
    match reader.string("alignSelf")? {
        Some("center") => r.align_self = Some(AlignSelf::Center),
        Some("start" | "flex-start") => r.align_self = Some(AlignSelf::FlexStart),
        Some("end" | "flex-end") => r.align_self = Some(AlignSelf::FlexEnd),
        Some("stretch") => r.align_self = Some(AlignSelf::Stretch),
        Some("baseline") => r.align_self = Some(AlignSelf::Baseline),
        _ => {}
    }
    match reader.string("alignContent")? {
        Some("center") => r.align_content = Some(AlignContent::Center),
        Some("start" | "flex-start") => r.align_content = Some(AlignContent::FlexStart),
        Some("end" | "flex-end") => r.align_content = Some(AlignContent::FlexEnd),
        Some("between" | "space-between") => r.align_content = Some(AlignContent::SpaceBetween),
        Some("around" | "space-around") => r.align_content = Some(AlignContent::SpaceAround),
        Some("evenly" | "space-evenly") => r.align_content = Some(AlignContent::SpaceEvenly),
        Some("stretch") => r.align_content = Some(AlignContent::Stretch),
        // "normal" is the default: it adds nothing to a refinement.
        _ => {}
    }
    match reader.string("justifyContent")? {
        Some("center") => r.justify_content = Some(JustifyContent::Center),
        Some("start" | "flex-start") => r.justify_content = Some(JustifyContent::Start),
        Some("end" | "flex-end") => r.justify_content = Some(JustifyContent::End),
        Some("between" | "space-between") => r.justify_content = Some(JustifyContent::SpaceBetween),
        Some("around" | "space-around") => r.justify_content = Some(JustifyContent::SpaceAround),
        Some("evenly" | "space-evenly") => r.justify_content = Some(JustifyContent::SpaceEvenly),
        _ => {}
    }
    if let Some(gap) = reader.number("gap")? {
        r.gap.width = Some(pixels(gap).into());
        r.gap.height = Some(pixels(gap).into());
    }
    if let Some(gap) = reader.number("rowGap")? {
        r.gap.height = Some(pixels(gap).into());
    }
    if let Some(gap) = reader.number("columnGap")? {
        r.gap.width = Some(pixels(gap).into());
    }
    let grid_min = |key: &str| -> Parsed<GridTemplateMinSize> {
        Ok(match reader.string(key)? {
            Some("min-content") => GridTemplateMinSize::MinContent,
            Some("max-content") => GridTemplateMinSize::MaxContent,
            _ => GridTemplateMinSize::Zero,
        })
    };
    let (columns_min, rows_min) = (grid_min("gridColumnMin")?, grid_min("gridRowMin")?);
    let grid_count = |value: f64| value.round().clamp(1., 64.) as u16;
    if let Some(columns) = reader.number("gridTemplateColumns")? {
        r.grid_cols = Some(GridTemplate {
            repeat: grid_count(columns),
            min_size: columns_min,
        });
    }
    if let Some(rows) = reader.number("gridTemplateRows")? {
        r.grid_rows = Some(GridTemplate {
            repeat: grid_count(rows),
            min_size: rows_min,
        });
    }

    // Sizes.
    if let Some(length) = reader.dimension("width", true)? {
        r.size.width = Some(length);
    }
    if let Some(length) = reader.dimension("height", true)? {
        r.size.height = Some(length);
    }
    if let Some(length) = reader.dimension("minWidth", false)? {
        r.min_size.width = Some(length);
    }
    if let Some(length) = reader.dimension("minHeight", false)? {
        r.min_size.height = Some(length);
    }
    if let Some(length) = reader.dimension("maxWidth", false)? {
        r.max_size.width = Some(length);
    }
    if let Some(length) = reader.dimension("maxHeight", false)? {
        r.max_size.height = Some(length);
    }
    if let Some(ratio) = reader.number("aspectRatio")?
        && ratio.is_finite()
        && ratio > 0.
    {
        r.aspect_ratio = Some(ratio as f32);
    }

    // Spacing: shorthand first, then the sides.
    let sides = reader.four_sides(
        "margin",
        ["marginTop", "marginRight", "marginBottom", "marginLeft"],
    )?;
    set_edges(&mut r.margin, sides, |value| pixels(value).into());
    let sides = reader.four_sides(
        "padding",
        ["paddingTop", "paddingRight", "paddingBottom", "paddingLeft"],
    )?;
    set_edges(&mut r.padding, sides, |value| pixels(value).into());

    match reader.string("position")? {
        Some("relative") => r.position = Some(Position::Relative),
        Some("absolute" | "fixed") => r.position = Some(Position::Absolute),
        _ => {}
    }
    let sides = [
        reader.number("top")?,
        reader.number("right")?,
        reader.number("bottom")?,
        reader.number("left")?,
    ];
    set_edges(&mut r.inset, sides, |value| pixels(value).into());

    // Paint: background.
    let mut fill = FillOutcome::Absent;
    if let Some(value) = reader.get("background") {
        fill = match value {
            Value::String(text) => parse_style_color(text)
                .map(|color| FillOutcome::Valid(Fill::from(color)))
                .unwrap_or(FillOutcome::Invalid),
            Value::Object(gradient) => parse_gradient(gradient),
            _ => return Err(reader.wrong_type("background", "a color string or a gradient object")),
        };
    }
    if let Some(color) = reader.color("backgroundColor")? {
        fill = match color {
            Some(color) => FillOutcome::Valid(Fill::from(color)),
            None => FillOutcome::Invalid,
        };
    }
    let unparsed_fill = matches!(fill, FillOutcome::Invalid);
    if let FillOutcome::Valid(fill) = fill {
        r.background = Some(fill);
    }

    if let Some(color) = reader.color("color")?.flatten() {
        r.text.color = Some(color.into());
    }
    if let Some(opacity) = reader.number("opacity")? {
        r.opacity = Some(opacity as f32);
    }

    // Borders.
    let widths = reader.four_sides(
        "borderWidth",
        [
            "borderTopWidth",
            "borderRightWidth",
            "borderBottomWidth",
            "borderLeftWidth",
        ],
    )?;
    set_edges(&mut r.border_widths, widths, |value| {
        AbsoluteLength::from(pixels(value.max(0.)))
    });
    if let Some(color) = reader.color("borderColor")?.flatten() {
        r.border_color = Some(color.into());
    }
    match reader.string("borderStyle")? {
        Some("solid") => r.border_style = Some(BorderStyle::Solid),
        Some("dashed") => r.border_style = Some(BorderStyle::Dashed),
        _ => {}
    }
    let radii = reader.four_sides(
        "borderRadius",
        [
            "borderTopLeftRadius",
            "borderTopRightRadius",
            "borderBottomRightRadius",
            "borderBottomLeftRadius",
        ],
    )?;
    set_corners(&mut r.corner_radii, radii, |value| {
        AbsoluteLength::from(pixels(value.max(0.)))
    });

    // Shadows and outline.
    let shadows = parse_shadows(reader)?;
    if !shadows.is_empty() {
        r.box_shadow = Some(shadows);
    }
    r.outline = parse_outline(reader)?;

    // Text.
    if let Some(size) = reader.number("fontSize")? {
        r.text.font_size = Some(pixels(size).into());
    }
    if let Some(family) = reader.string("fontFamily")? {
        r.text.font_family = Some(SharedString::from(family.to_owned()));
    }
    match reader.get("fontWeight") {
        None => {}
        Some(Value::Number(number)) => {
            if let Some(weight) = number.as_f64() {
                r.text.font_weight = Some(FontWeight((weight as f32).clamp(1., 1000.)));
            }
        }
        Some(Value::String(text)) => r.text.font_weight = Some(font_weight_from_string(text)),
        Some(_) => return Err(reader.wrong_type("fontWeight", "a number or a string")),
    }
    if let Some(height) = reader.number("lineHeight")?
        && height > 0.
    {
        r.text.line_height = Some(DefiniteLength::from(pixels(height)));
    }
    match reader.string("textAlign")? {
        Some("left" | "start") => r.text.text_align = Some(TextAlign::Left),
        Some("center") => r.text.text_align = Some(TextAlign::Center),
        Some("right") => r.text.text_align = Some(TextAlign::Right),
        _ => {}
    }
    match reader.string("whiteSpace")? {
        Some("nowrap") => r.text.white_space = Some(WhiteSpace::Nowrap),
        Some("normal") => r.text.white_space = Some(WhiteSpace::Normal),
        _ => {}
    }
    match reader.string("textOverflow")? {
        Some("ellipsis") => {
            r.text.text_overflow = Some(TextOverflow::Truncate(SharedString::new_static("…")))
        }
        Some("ellipsis-start") => {
            r.text.text_overflow = Some(TextOverflow::TruncateStart(SharedString::new_static("…")))
        }
        _ => {}
    }
    if let Some(lines) = reader.number("lineClamp")?
        && lines >= 1.
    {
        r.text.line_clamp = Some(lines.trunc() as usize);
    }
    match reader.string("textDecoration")? {
        Some("underline") => {
            r.text.underline = Some(UnderlineStyle {
                thickness: px(1.),
                ..Default::default()
            })
        }
        Some("line-through") => {
            r.text.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                ..Default::default()
            })
        }
        // A zero-thickness decoration paints nothing and replaces an inherited one.
        Some("none") => {
            r.text.underline = Some(UnderlineStyle::default());
            r.text.strikethrough = Some(StrikethroughStyle::default());
        }
        _ => {}
    }

    // Overflow, scrolling, cursor, input behavior.
    let overflow = |value: Option<&str>| match value {
        Some("hidden") => Some(Overflow::Hidden),
        Some("scroll") => Some(Overflow::Scroll),
        _ => None,
    };
    if let Some(value) = overflow(reader.string("overflow")?) {
        r.overflow.x = Some(value);
        r.overflow.y = Some(value);
    }
    if let Some(value) = overflow(reader.string("overflowX")?) {
        r.overflow.x = Some(value);
    }
    if let Some(value) = overflow(reader.string("overflowY")?) {
        r.overflow.y = Some(value);
    }
    layer.scrollbar_color = reader.color("scrollbarColor")?.flatten().map(Hsla::from);
    layer.scrollbar_width = reader
        .number("scrollbarWidth")?
        .map(|width| pixels(width.max(0.)));
    // The classic thumbs (CMP-031) are enabled by the style itself: applying the refinement to
    // an element carries the color and the thickness.
    layer.refinement.scrollbar_color = layer.scrollbar_color;
    layer.refinement.scrollbar_width = layer.scrollbar_width.map(AbsoluteLength::Pixels);
    let r = &mut layer.refinement;
    if let Some(cursor) = reader.string("cursor")?.and_then(cursor_style_from_keyword) {
        r.mouse_cursor = Some(cursor);
    }
    let explicit_pointer_events = match reader.string("pointerEvents")? {
        Some("auto") => Some(PointerEvents::Auto),
        Some("none") => Some(PointerEvents::None),
        _ => None,
    };
    // The base style always carries a policy: unset follows the painted surface (R17).
    r.pointer_events =
        explicit_pointer_events.or(is_base.then_some(PointerEvents::Surface { unparsed_fill }));
    match reader.string("userSelect")? {
        Some("text" | "auto") => r.text.user_select = Some(true),
        Some("none") => r.text.user_select = Some(false),
        _ => {}
    }
    if let Some(color) = reader.color("selectionColor")?.flatten() {
        r.text.selection_color = Some(color.into());
    }
    if is_base && let Some(enabled) = reader.boolean("keyboardFocusDim")? {
        r.keyboard_focus_dim = Some(enabled);
    }
    Ok(layer)
}

fn set_edges<T: Clone + fmt::Debug + Default + PartialEq>(
    edges: &mut EdgesRefinement<T>,
    [top, right, bottom, left]: [Option<f64>; 4],
    convert: impl Fn(f64) -> T,
) {
    edges.top = top.map(&convert).or(edges.top.take());
    edges.right = right.map(&convert).or(edges.right.take());
    edges.bottom = bottom.map(&convert).or(edges.bottom.take());
    edges.left = left.map(&convert).or(edges.left.take());
}

fn set_corners<T: Clone + fmt::Debug + Default + PartialEq>(
    corners: &mut CornersRefinement<T>,
    [top_left, top_right, bottom_right, bottom_left]: [Option<f64>; 4],
    convert: impl Fn(f64) -> T,
) {
    corners.top_left = top_left.map(&convert).or(corners.top_left.take());
    corners.top_right = top_right.map(&convert).or(corners.top_right.take());
    corners.bottom_right = bottom_right.map(&convert).or(corners.bottom_right.take());
    corners.bottom_left = bottom_left.map(&convert).or(corners.bottom_left.take());
}

enum FillOutcome {
    Absent,
    Valid(Fill),
    /// The fill could not be parsed: nothing is painted but the surface still counts as filled
    /// for mouse blocking.
    Invalid,
}

fn parse_gradient(gradient: &Map<String, Value>) -> FillOutcome {
    let parsed = (|| {
        if gradient.get("type")?.as_str()? != "linear-gradient" {
            return None;
        }
        let angle = gradient.get("angle")?.as_f64()?;
        if !angle.is_finite() {
            return None;
        }
        let color_space = match gradient.get("colorSpace") {
            None | Some(Value::Null) => ColorSpace::Srgb,
            Some(Value::String(name)) if name == "srgb" => ColorSpace::Srgb,
            Some(Value::String(name)) if name == "oklab" => ColorSpace::Oklab,
            Some(_) => return None,
        };
        let stops = gradient.get("stops")?.as_array()?;
        let [first, second] = stops.as_slice() else {
            return None;
        };
        let stop = |stop: &Value| {
            let stop = stop.as_object()?;
            let position = stop.get("position")?.as_f64()?;
            if !(0. ..=1.).contains(&position) {
                return None;
            }
            let color = parse_style_color(stop.get("color")?.as_str()?)?;
            Some(linear_color_stop(color, position as f32))
        };
        Some(linear_gradient(angle as f32, stop(first)?, stop(second)?).color_space(color_space))
    })();
    match parsed {
        Some(background) => FillOutcome::Valid(Fill::from(background)),
        None => FillOutcome::Invalid,
    }
}

fn parse_shadows(reader: &Reader) -> Parsed<Vec<BoxShadow>> {
    let entry = |value: &Value| -> Option<BoxShadow> {
        let entry = value.as_object()?;
        let number = |key: &str| match entry.get(key) {
            None | Some(Value::Null) => Some(0.),
            Some(value) => value.as_f64(),
        };
        let color = parse_style_color(entry.get("color")?.as_str()?)?;
        let inset = match entry.get("inset") {
            None | Some(Value::Null) => false,
            Some(value) => value.as_bool()?,
        };
        let mut shadow = BoxShadow::new(
            pixels(number("offsetX")?),
            pixels(number("offsetY")?),
            color.into(),
        )
        .blur_radius(pixels(number("blurRadius")?.max(0.)))
        .spread_radius(pixels(number("spreadRadius")?));
        shadow.inset = inset;
        Some(shadow)
    };
    match reader.get("boxShadow") {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) => Ok(entries.iter().filter_map(entry).collect()),
        Some(value @ Value::Object(_)) => Ok(entry(value).into_iter().collect()),
        Some(_) => Err(reader.wrong_type("boxShadow", "a shadow object or an array of them")),
    }
}

fn parse_outline(reader: &Reader) -> Parsed<Option<Outline>> {
    let width = reader.number("outlineWidth")?;
    let color = reader.color("outlineColor")?;
    let offset = reader.number("outlineOffset")?;
    if width.is_none() && color.is_none() {
        return Ok(None);
    }
    Ok(Some(Outline {
        width: width.map_or(px(1.), |width| pixels(width.max(0.))),
        color: color
            .flatten()
            .map_or(crate::transparent_black(), Hsla::from),
        offset: pixels(offset.unwrap_or(0.)),
    }))
}
