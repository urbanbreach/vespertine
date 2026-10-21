//! The `canvas` host type (RND-009): a declarative drawing list painted during the paint phase.
//!
//! The `commands` property is an ordered array of drawing operations whose coordinates are
//! logical pixels relative to the host's top-left corner. Everything is clipped to the host's
//! own box on top of the clip, opacity and paint order the host inherits (R6). The canvas has no
//! hit area or accessibility node of its own (R7): the host's style box and the standard
//! properties provide them.
//!
//! * `{ "op": "stroke", "points": [[x, y], ...], "closed": false, "width": 1, "color": "#fff",
//!   "cap": "butt|square|round", "join": "miter|miterClip|round|bevel", "miterLimit": 4,
//!   "dash": [on, off, ...] }` strokes a polyline (open unless `closed`).
//! * `{ "op": "fill", "points": [[x, y], ...], "color": "#fff", "rule": "nonZero|evenOdd" }` fills
//!   a closed polygon.
//! * `{ "op": "path", "segments": [["M", x, y], ["L", x, y], ["Q", cx, cy, x, y],
//!   ["C", c1x, c1y, c2x, c2y, x, y], ["A", rx, ry, rotation, largeArc, sweep, x, y], ["Z"]],
//!   "fill": "#fff", "rule": "nonZero", "stroke": { "color": "#fff", "width": 1, ... } }` paints
//!   a general path filled, stroked or both (the fill first).
//! * `{ "op": "rect", "x", "y", "width", "height", "color", "radius", "borderColor",
//!   "borderWidth" }` and `{ "op": "circle", "cx", "cy", "radius", "color" }` paint quads (a
//!   circle is a square quad whose corner radii equal half its side, R5).
//! * `{ "op": "text", "x", "y", "text", "color", "size", "lineHeight", "family", "weight",
//!   "align": "left|center|right", "width" }` paints one shaped line whose line box has its
//!   top-left corner at `(x, y)`; `width` is the alignment width for centering or right
//!   alignment.
//!
//! An operation that does not parse (unknown `op`, missing or non-numeric coordinates, an
//! unparsable color) is skipped and reported once per change of the property.

use std::rc::Rc;

use serde_json::Value;
use vespertine::{
    AnyElement, App, Bounds, ContentMask, Corners, ElementAdapter, ElementFactory, FillRule,
    FontWeight, HostId, Hsla, IntoElement, LineCap, LineJoin, ParentElement, PathBuilder,
    PathStyle, Pixels, Point, RenderContext, StrokeOptions, Styled, TextAlign, TextRun, Window,
    canvas, fill, point, px, size,
};

const CANVAS_PROPERTIES: &[&str] = &["commands"];
const CANVAS_EVENTS: &[&str] = &["click", "enter", "leave", "fileDrop"];

pub(super) struct CanvasFactory;

impl ElementFactory for CanvasFactory {
    fn type_name(&self) -> &'static str {
        "canvas"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(CanvasAdapter::default())
    }
}

#[derive(Default)]
struct CanvasAdapter {
    commands: Rc<Vec<Command>>,
}

#[derive(Clone, Debug, PartialEq)]
struct StrokeStyle {
    color: Hsla,
    width: f32,
    cap: LineCap,
    join: LineJoin,
    miter_limit: Option<f32>,
    dash: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Quad([f32; 4]),
    Cubic([f32; 6]),
    Arc {
        radii: [f32; 2],
        rotation: f32,
        large: bool,
        sweep: bool,
        to: [f32; 2],
    },
    Close,
}

#[derive(Clone, Debug, PartialEq)]
enum Command {
    Stroke {
        points: Vec<[f32; 2]>,
        closed: bool,
        style: StrokeStyle,
    },
    Fill {
        points: Vec<[f32; 2]>,
        color: Hsla,
        rule: FillRule,
    },
    Path {
        segments: Vec<Segment>,
        fill: Option<(Hsla, FillRule)>,
        stroke: Option<StrokeStyle>,
    },
    Rect {
        bounds: [f32; 4],
        color: Option<Hsla>,
        radius: f32,
        border: Option<(Hsla, f32)>,
    },
    Circle {
        center: [f32; 2],
        radius: f32,
        color: Hsla,
    },
    Text {
        origin: [f32; 2],
        text: String,
        color: Option<Hsla>,
        size: Option<f32>,
        line_height: Option<f32>,
        family: Option<String>,
        weight: Option<f32>,
        align: TextAlign,
        width: Option<f32>,
    },
}

// ---- parsing ------------------------------------------------------------------------------------

fn number(value: &Value, name: &str) -> Option<f32> {
    value
        .get(name)?
        .as_f64()
        .filter(|number| number.is_finite())
        .map(|number| number as f32)
}

fn optional_number(value: &Value, name: &str) -> Option<f32> {
    number(value, name)
}

fn color(value: &Value, name: &str) -> Option<Hsla> {
    vespertine::css_color_to_rgba(value.get(name)?.as_str()?).map(Hsla::from)
}

fn points(value: &Value) -> Option<Vec<[f32; 2]>> {
    value
        .get("points")?
        .as_array()?
        .iter()
        .map(|pair| {
            let pair = pair.as_array()?;
            Some([
                pair.first()?.as_f64()? as f32,
                pair.get(1)?.as_f64()? as f32,
            ])
        })
        .collect()
}

fn rule(value: &Value) -> FillRule {
    match value.get("rule").and_then(Value::as_str) {
        Some("evenOdd") => FillRule::EvenOdd,
        _ => FillRule::NonZero,
    }
}

fn stroke_style(value: &Value, color: Hsla) -> StrokeStyle {
    StrokeStyle {
        color,
        width: optional_number(value, "width").unwrap_or(1.0),
        cap: match value.get("cap").and_then(Value::as_str) {
            Some("square") => LineCap::Square,
            Some("round") => LineCap::Round,
            _ => LineCap::Butt,
        },
        join: match value.get("join").and_then(Value::as_str) {
            Some("miterClip") => LineJoin::MiterClip,
            Some("round") => LineJoin::Round,
            Some("bevel") => LineJoin::Bevel,
            _ => LineJoin::Miter,
        },
        miter_limit: optional_number(value, "miterLimit"),
        dash: value
            .get("dash")
            .and_then(Value::as_array)
            .map(|dash| {
                dash.iter()
                    .filter_map(Value::as_f64)
                    .map(|length| length as f32)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn segments(value: &Value) -> Option<Vec<Segment>> {
    value
        .get("segments")?
        .as_array()?
        .iter()
        .map(|segment| {
            let segment = segment.as_array()?;
            let operator = segment.first()?.as_str()?;
            let numbers: Vec<f32> = segment[1..]
                .iter()
                .map(|number| number.as_f64().map(|number| number as f32))
                .collect::<Option<_>>()?;
            let at = |index: usize| numbers.get(index).copied();
            Some(match operator {
                "M" => Segment::Move(at(0)?, at(1)?),
                "L" => Segment::Line(at(0)?, at(1)?),
                "Q" => Segment::Quad([at(0)?, at(1)?, at(2)?, at(3)?]),
                "C" => Segment::Cubic([at(0)?, at(1)?, at(2)?, at(3)?, at(4)?, at(5)?]),
                "A" => Segment::Arc {
                    radii: [at(0)?, at(1)?],
                    rotation: at(2)?,
                    large: at(3)? != 0.0,
                    sweep: at(4)? != 0.0,
                    to: [at(5)?, at(6)?],
                },
                "Z" => Segment::Close,
                _ => return None,
            })
        })
        .collect()
}

fn command(value: &Value) -> Option<Command> {
    Some(match value.get("op")?.as_str()? {
        "stroke" => Command::Stroke {
            points: points(value)?,
            closed: value
                .get("closed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            style: stroke_style(value, color(value, "color")?),
        },
        "fill" => Command::Fill {
            points: points(value)?,
            color: color(value, "color")?,
            rule: rule(value),
        },
        "path" => {
            let fill = value
                .get("fill")
                .and_then(Value::as_str)
                .and_then(vespertine::css_color_to_rgba)
                .map(|fill| (Hsla::from(fill), rule(value)));
            let stroke = match value.get("stroke") {
                Some(stroke) if stroke.is_object() => {
                    Some(stroke_style(stroke, color(stroke, "color")?))
                }
                _ => None,
            };
            Command::Path {
                segments: segments(value)?,
                fill,
                stroke,
            }
        }
        "rect" => Command::Rect {
            bounds: [
                number(value, "x")?,
                number(value, "y")?,
                number(value, "width")?,
                number(value, "height")?,
            ],
            color: color(value, "color"),
            radius: optional_number(value, "radius").unwrap_or(0.0),
            border: color(value, "borderColor")
                .map(|border| (border, optional_number(value, "borderWidth").unwrap_or(1.0))),
        },
        "circle" => Command::Circle {
            center: [number(value, "cx")?, number(value, "cy")?],
            radius: number(value, "radius")?,
            color: color(value, "color")?,
        },
        "text" => Command::Text {
            origin: [number(value, "x")?, number(value, "y")?],
            text: value.get("text")?.as_str()?.to_owned(),
            color: color(value, "color"),
            size: optional_number(value, "size"),
            line_height: optional_number(value, "lineHeight"),
            family: value
                .get("family")
                .and_then(Value::as_str)
                .map(str::to_owned),
            weight: optional_number(value, "weight"),
            align: match value.get("align").and_then(Value::as_str) {
                Some("center") => TextAlign::Center,
                Some("right") => TextAlign::Right,
                _ => TextAlign::Left,
            },
            width: optional_number(value, "width"),
        },
        _ => return None,
    })
}

// ---- painting -----------------------------------------------------------------------------------

fn at(origin: Point<Pixels>, [x, y]: [f32; 2]) -> Point<Pixels> {
    origin + point(px(x), px(y))
}

fn stroke_builder(style: &StrokeStyle) -> PathBuilder {
    let mut options = StrokeOptions::default()
        .with_line_width(style.width)
        .with_line_cap(style.cap)
        .with_line_join(style.join);
    if let Some(limit) = style.miter_limit {
        options = options.with_miter_limit(limit);
    }
    let builder = PathBuilder::stroke(px(style.width)).with_style(PathStyle::Stroke(options));
    if style.dash.is_empty() {
        builder
    } else {
        let dash: Vec<Pixels> = style.dash.iter().copied().map(px).collect();
        builder.dash_array(&dash)
    }
}

fn fill_builder(rule: FillRule) -> PathBuilder {
    PathBuilder::fill().with_style(PathStyle::Fill(
        vespertine::FillOptions::default().with_fill_rule(rule),
    ))
}

fn paint_path(builder: PathBuilder, color: Hsla, window: &mut Window) {
    // An empty or degenerate path builds to nothing (RND-009 R2): nothing is painted.
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn trace(builder: &mut PathBuilder, segments: &[Segment], origin: Point<Pixels>) {
    for segment in segments {
        match *segment {
            Segment::Move(x, y) => builder.move_to(at(origin, [x, y])),
            Segment::Line(x, y) => builder.line_to(at(origin, [x, y])),
            Segment::Quad([cx, cy, x, y]) => {
                builder.curve_to(at(origin, [x, y]), at(origin, [cx, cy]))
            }
            Segment::Cubic([ax, ay, bx, by, x, y]) => builder.cubic_bezier_to(
                at(origin, [x, y]),
                at(origin, [ax, ay]),
                at(origin, [bx, by]),
            ),
            Segment::Arc {
                radii,
                rotation,
                large,
                sweep,
                to,
            } => builder.arc_to(
                point(px(radii[0]), px(radii[1])),
                px(rotation),
                large,
                sweep,
                at(origin, to),
            ),
            Segment::Close => builder.close(),
        }
    }
}

fn paint_command(command: &Command, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let origin = bounds.origin;
    match command {
        Command::Stroke {
            points,
            closed,
            style,
        } => {
            let mut builder = stroke_builder(style);
            let points: Vec<Point<Pixels>> =
                points.iter().map(|point| at(origin, *point)).collect();
            builder.add_polygon(&points, *closed);
            paint_path(builder, style.color, window);
        }
        Command::Fill {
            points,
            color,
            rule,
        } => {
            let mut builder = fill_builder(*rule);
            let points: Vec<Point<Pixels>> =
                points.iter().map(|point| at(origin, *point)).collect();
            builder.add_polygon(&points, true);
            paint_path(builder, *color, window);
        }
        Command::Path {
            segments,
            fill,
            stroke,
        } => {
            if let Some((color, rule)) = fill {
                let mut builder = fill_builder(*rule);
                trace(&mut builder, segments, origin);
                paint_path(builder, *color, window);
            }
            if let Some(style) = stroke {
                let mut builder = stroke_builder(style);
                trace(&mut builder, segments, origin);
                paint_path(builder, style.color, window);
            }
        }
        Command::Rect {
            bounds: [x, y, width, height],
            color,
            radius,
            border,
        } => {
            let rect = Bounds {
                origin: at(origin, [*x, *y]),
                size: size(px(*width), px(*height)),
            };
            let mut quad = fill(rect, color.unwrap_or_else(Hsla::transparent_black))
                .corner_radii(Corners::all(px(*radius)));
            if let Some((border, width)) = border {
                quad = quad.border_color(*border).border_widths(px(*width));
            }
            window.paint_quad(quad);
        }
        Command::Circle {
            center,
            radius,
            color,
        } => {
            let rect = Bounds {
                origin: at(origin, [center[0] - radius, center[1] - radius]),
                size: size(px(radius * 2.0), px(radius * 2.0)),
            };
            window.paint_quad(fill(rect, *color).corner_radii(Corners::all(px(*radius))));
        }
        Command::Text {
            origin: from,
            text,
            color,
            size: font_size,
            line_height,
            family,
            weight,
            align,
            width,
        } => {
            // One shaped line: a line break would not shape as one line.
            let line_text: String = text.lines().next().unwrap_or_default().to_owned();
            let style = window.text_style();
            let rem = window.rem_size();
            let font_size = font_size
                .map(px)
                .unwrap_or_else(|| style.font_size.to_pixels(rem));
            let line_height = line_height
                .map(px)
                .unwrap_or_else(|| style.line_height.to_pixels(font_size.into(), rem));
            let mut font = style.font();
            if let Some(family) = family {
                font.family = family.clone().into();
            }
            if let Some(weight) = weight {
                font.weight = FontWeight(*weight);
            }
            let run = TextRun {
                background_color: None,
                underline: None,
                strikethrough: None,
                color: color.unwrap_or(style.color),
                font,
                len: line_text.len(),
                letter_spacing: px(0.),
            };
            let line = window
                .text_system()
                .shape_line(line_text.into(), font_size, &[run], None);
            if let Err(error) = line.paint(
                at(origin, *from),
                line_height,
                *align,
                width.map(px),
                window,
                cx,
            ) {
                log::warn!("canvas text did not paint: {error}");
            }
        }
    }
}

impl ElementAdapter for CanvasAdapter {
    fn render(
        &mut self,
        mut context: RenderContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let commands = self.commands.clone();
        let mut surface = context.surface();
        surface.extend([canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                // Drawing never leaves the canvas box, whatever coordinates the commands use.
                let mask = ContentMask { bounds };
                window.with_content_mask(Some(mask), |window| {
                    for command in commands.iter() {
                        paint_command(command, bounds, window, cx);
                    }
                });
            },
        )
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .into_any_element()]);
        surface.into_any_element()
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        if name != "commands" {
            return;
        }
        let mut parsed = Vec::new();
        if let Some(items) = value.as_array() {
            for (index, item) in items.iter().enumerate() {
                match command(item) {
                    Some(command) => parsed.push(command),
                    None => log::warn!("canvas command {index} is not a drawing operation: {item}"),
                }
            }
        }
        self.commands = Rc::new(parsed);
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        CANVAS_PROPERTIES
    }

    fn supported_events(&self) -> &'static [&'static str] {
        CANVAS_EVENTS
    }

    fn destroy(&mut self, _cx: &mut App) {
        self.commands = Rc::default();
    }
}
