//! Post-processing of the generated SVG (CMP-013 R7): the diagram follows the theme where the
//! source did not style it itself.
//!
//! Only explicit author styling is left alone: a color that differs from the renderer's default
//! (the theme palette) is never touched. The rewriting works on start tags of the document and
//! never builds a tree: tags that no rule changes are copied byte for byte.

use super::adapter::Palette;
use vespertine_theme::color::Color;

/// The renderer's default grid line stroke.
pub const DEFAULT_GRID_STROKE: &str = "#E2E8F0";

/// Applies the R7 rules to `svg`:
///
/// 1. a full-canvas rectangle whose only attributes are position, size and a fill equal to the
///    canvas color is dropped (the diagram sits on the document's own plate), and so is a
///    canvas-colored background declaration of the root element;
/// 2. rectangle corner radius 3 becomes 8;
/// 3. where a rectangle carries the border stroke, a canvas-colored fill becomes the node color;
/// 4. a default-styled decision diamond (a four-point polygon with the node fill and border
///    stroke, top and bottom points vertically aligned and left and right points horizontally
///    aligned within 0.05) takes the accent wash fill and the accent line stroke;
/// 5. the default grid stroke `#E2E8F0` becomes the grid color.
pub fn restyle(svg: &str, palette: &Palette) -> String {
    let canvas_size = viewbox_size(svg);
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    let mut root = true;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(rest.len(), |ix| ix + 3);
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        if rest.starts_with("<style") {
            // Copy the start tag and the stylesheet up to the end tag.
            let end = rest
                .find("</style>")
                .map_or(rest.len(), |ix| ix + "</style>".len());
            out.push_str(&replace_grid(&rest[..end], palette));
            rest = &rest[end..];
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        rest = &rest[end..];
        let mut tag = Tag::parse(tag);
        let changed = match tag.name.as_str() {
            "svg" if root => restyle_root(&mut tag, palette),
            "rect" => {
                if is_canvas_rect(&tag, palette, canvas_size) {
                    // Drop the rectangle and its immediate end tag.
                    if let Some(after) = rest.strip_prefix("</rect>") {
                        rest = after;
                    }
                    continue;
                }
                restyle_rect(&mut tag, palette)
            }
            "polygon" => restyle_diamond(&mut tag, palette),
            _ => false,
        };
        let grid = tag.replace_grid(palette);
        if tag.name == "svg" {
            root = false;
        }
        if changed || grid {
            out.push_str(&tag.render());
        } else {
            out.push_str(&tag.original);
        }
    }
    out.push_str(rest);
    out
}

fn replace_grid(text: &str, palette: &Palette) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = DEFAULT_GRID_STROKE.to_ascii_lowercase();
    if !lower.contains(&needle) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find(&needle) {
        out.push_str(&text[at..at + found]);
        out.push_str(&palette.grid.to_hex());
        at += found + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// The end of the start tag at the beginning of `text` (just past its `>`), honoring quotes.
fn tag_end(text: &str) -> usize {
    let mut quote: Option<char> = None;
    for (ix, ch) in text.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') | (None, '\'') => quote = Some(ch),
            (None, '>') => return ix + 1,
            _ => {}
        }
    }
    text.len()
}

fn viewbox_size(svg: &str) -> Option<(f32, f32)> {
    let ix = svg.find("viewBox=\"")? + "viewBox=\"".len();
    let end = svg[ix..].find('"')? + ix;
    let numbers: Vec<f32> = svg[ix..end]
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    (numbers.len() == 4).then(|| (numbers[2], numbers[3]))
}

/// A start tag split into its name and attributes.
struct Tag {
    original: String,
    name: String,
    attrs: Vec<(String, String)>,
    self_closing: bool,
}

impl Tag {
    fn parse(text: &str) -> Tag {
        let inner = text
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim_end_matches('/');
        let name_end = inner
            .find(|ch: char| ch.is_whitespace())
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_string();
        let mut attrs = Vec::new();
        let mut rest = inner[name_end..].trim_start();
        while !rest.is_empty() {
            let Some(eq) = rest.find('=') else { break };
            let key = rest[..eq].trim().to_string();
            let after = rest[eq + 1..].trim_start();
            let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(close) = after[1..].find(quote) else {
                break;
            };
            attrs.push((key, after[1..1 + close].to_string()));
            rest = after[close + 2..].trim_start();
        }
        Tag {
            original: text.to_string(),
            name,
            attrs,
            self_closing: text.trim_end_matches('>').ends_with('/'),
        }
    }

    fn render(&self) -> String {
        let mut out = format!("<{}", self.name);
        for (key, value) in &self.attrs {
            out.push_str(&format!(" {key}=\"{value}\""));
        }
        out.push_str(if self.self_closing { "/>" } else { ">" });
        out
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn set_attr(&mut self, name: &str, value: String) {
        match self.attrs.iter_mut().find(|(key, _)| key == name) {
            Some((_, existing)) => *existing = value,
            None => self.attrs.push((name.to_string(), value)),
        }
    }

    /// The declared value of a paint property: the inline style wins over the attribute.
    fn declared(&self, property: &str) -> Option<String> {
        if let Some(style) = self.attr("style") {
            for declaration in style.split(';') {
                if let Some((key, value)) = declaration.split_once(':')
                    && key.trim() == property
                {
                    return Some(value.trim().to_string());
                }
            }
        }
        self.attr(property).map(|value| value.trim().to_string())
    }

    /// Sets a paint property where it is declared: in the inline style when the style declares
    /// it or the tag has no attribute for it, else in the attribute.
    fn set_paint(&mut self, property: &str, color: Color) {
        let value = color.to_hex();
        let in_style = self.attr("style").is_some_and(|style| {
            style
                .split(';')
                .any(|d| d.split_once(':').is_some_and(|(k, _)| k.trim() == property))
        });
        if in_style {
            let style = self.attr("style").unwrap_or_default().to_string();
            let rewritten: Vec<String> = style
                .split(';')
                .filter(|declaration| !declaration.trim().is_empty())
                .map(|declaration| match declaration.split_once(':') {
                    Some((key, _)) if key.trim() == property => format!("{}:{value}", key.trim()),
                    _ => declaration.trim().to_string(),
                })
                .collect();
            self.set_attr("style", rewritten.join(";"));
        } else if self.attr(property).is_some() {
            self.set_attr(property, value);
        } else {
            // No declaration at all: an inline style outranks the stylesheet's defaults.
            let mut style = self
                .attr("style")
                .unwrap_or_default()
                .trim_end_matches(';')
                .to_string();
            if !style.is_empty() {
                style.push(';');
            }
            style.push_str(&format!("{property}:{value}"));
            self.set_attr("style", style);
        }
    }

    fn replace_grid(&mut self, palette: &Palette) -> bool {
        let mut changed = false;
        for (_, value) in &mut self.attrs {
            let replaced = replace_grid(value, palette);
            if replaced != *value {
                *value = replaced;
                changed = true;
            }
        }
        changed
    }
}

fn is_color(value: Option<&str>, color: Color) -> bool {
    value.is_some_and(|value| {
        let value = value.trim().trim_end_matches("!important").trim();
        Color::parse(value).is_ok_and(|parsed| parsed.blend_over(Color::BLACK) == color)
    })
}

fn number(value: Option<&str>) -> Option<f32> {
    value.and_then(|value| value.trim().trim_end_matches("px").parse().ok())
}

fn restyle_root(tag: &mut Tag, palette: &Palette) -> bool {
    let Some(style) = tag.attr("style").map(str::to_string) else {
        return false;
    };
    let mut changed = false;
    let kept: Vec<&str> = style
        .split(';')
        .filter(|declaration| !declaration.trim().is_empty())
        .filter(|declaration| {
            let background = declaration
                .split_once(':')
                .is_some_and(|(key, _)| key.trim() == "background-color");
            // Plate colors never leak out of the diagram: the document provides the plate.
            changed |= background;
            !background
        })
        .collect();
    let _ = palette;
    if changed {
        tag.set_attr("style", kept.join(";"));
    }
    changed
}

fn is_canvas_rect(tag: &Tag, palette: &Palette, canvas: Option<(f32, f32)>) -> bool {
    let allowed = ["x", "y", "width", "height", "fill"];
    if !tag
        .attrs
        .iter()
        .all(|(key, _)| allowed.contains(&key.as_str()))
    {
        return false;
    }
    if !is_color(tag.attr("fill"), palette.canvas) {
        return false;
    }
    let origin = |name| tag.attr(name).is_none_or(|v| number(Some(v)) == Some(0.0));
    if !(origin("x") && origin("y")) {
        return false;
    }
    let covers = |name: &str, extent: Option<f32>| match tag.attr(name) {
        Some("100%") => true,
        value => match (number(value), extent) {
            (Some(size), Some(extent)) => (size - extent).abs() <= 0.5,
            _ => false,
        },
    };
    covers("width", canvas.map(|c| c.0)) && covers("height", canvas.map(|c| c.1))
}

fn restyle_rect(tag: &mut Tag, palette: &Palette) -> bool {
    let mut changed = false;
    for name in ["rx", "ry"] {
        if number(tag.attr(name)) == Some(3.0) {
            tag.set_attr(name, "8".to_string());
            changed = true;
        }
    }
    let stroke = tag.declared("stroke");
    let fill = tag.declared("fill");
    if is_color(stroke.as_deref(), palette.border) && is_color(fill.as_deref(), palette.canvas) {
        tag.set_paint("fill", palette.node);
        changed = true;
    }
    changed
}

fn restyle_diamond(tag: &mut Tag, palette: &Palette) -> bool {
    let Some(points) = tag.attr("points").map(parse_points) else {
        return false;
    };
    if points.len() != 4 || !is_diamond(&points) {
        return false;
    }
    let mut fill = tag.declared("fill");
    let mut stroke = tag.declared("stroke");
    // A node polygon without its own paint is styled by the theme's default node rule.
    if fill.is_none()
        && stroke.is_none()
        && tag
            .attr("class")
            .is_some_and(|c| c.contains("label-container"))
    {
        fill = Some(palette.node.to_hex());
        stroke = Some(palette.border.to_hex());
    }
    if is_color(fill.as_deref(), palette.node) && is_color(stroke.as_deref(), palette.border) {
        tag.set_paint("fill", palette.accent_wash);
        tag.set_paint("stroke", palette.accent_line);
        return true;
    }
    false
}

fn parse_points(text: &str) -> Vec<(f32, f32)> {
    let numbers: Vec<f32> = text
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    numbers
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

/// Four points forming a diamond: the topmost and bottommost points share x and the leftmost
/// and rightmost points share y, within 0.05.
fn is_diamond(points: &[(f32, f32)]) -> bool {
    const TOLERANCE: f32 = 0.05;
    let by = |key: fn(&(f32, f32)) -> f32, max: bool| {
        points
            .iter()
            .copied()
            .reduce(|a, b| if (key(&b) > key(&a)) == max { b } else { a })
    };
    let (Some(top), Some(bottom), Some(left), Some(right)) = (
        by(|p| p.1, false),
        by(|p| p.1, true),
        by(|p| p.0, false),
        by(|p| p.0, true),
    ) else {
        return false;
    };
    (top.0 - bottom.0).abs() <= TOLERANCE
        && (left.1 - right.1).abs() <= TOLERANCE
        && top != bottom
        && left != right
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagram::adapter::PaletteTokens;

    fn palette() -> Palette {
        Palette::from_tokens(&PaletteTokens {
            dark: true,
            background: Color::parse("#14161a").unwrap(),
            border: Color::parse("#3a3f4a").unwrap(),
            border_strong: Color::parse("#4a5060").unwrap(),
            text: Color::parse("#e6e8eb").unwrap(),
            text_muted: Color::parse("#a0a6b0").unwrap(),
            text_faint: Color::parse("#70778a").unwrap(),
            accent: Color::parse("#6c8cff").unwrap(),
            font_sans: "DejaVu Sans".into(),
        })
    }

    fn svg(body: &str) -> String {
        format!(
            r#"<svg viewBox="0 0 200 100" style="max-width:200px;background-color:white">{body}</svg>"#
        )
    }

    #[test]
    fn a_full_canvas_rectangle_with_the_canvas_fill_is_dropped() {
        let p = palette();
        let canvas = p.canvas.to_hex();
        let input = svg(&format!(
            r##"<rect x="0" y="0" width="200" height="100" fill="{canvas}"/><rect x="0" y="0" width="200" height="100" fill="{canvas}" stroke="red"/><rect x="0" y="0" width="200" height="100" fill="#ff0000"/><rect x="5" y="0" width="200" height="100" fill="{canvas}"/>"##
        ));
        let out = restyle(&input, &p);
        assert_eq!(out.matches("<rect").count(), 3, "{out}");
        assert!(out.contains(r##"fill="#ff0000""##));
        assert!(
            !out.contains("background-color"),
            "the root plate is dropped"
        );
    }

    #[test]
    fn corner_radius_three_becomes_eight_and_other_radii_stay() {
        let out = restyle(
            &svg(r#"<rect rx="3" ry="3" width="10" height="10"/><rect rx="5" ry="5"/>"#),
            &palette(),
        );
        assert!(out.contains(r#"rx="8" ry="8""#));
        assert!(out.contains(r#"rx="5" ry="5""#));
    }

    #[test]
    fn a_bordered_rectangle_with_a_canvas_fill_takes_the_node_fill() {
        let p = palette();
        let rect = |fill: &str| {
            format!(
                r#"<rect width="10" height="10" fill="{fill}" stroke="{}"/>"#,
                p.border.to_hex()
            )
        };
        let out = restyle(&svg(&rect(&p.canvas.to_hex())), &p);
        assert!(
            out.contains(&format!(r#"fill="{}""#, p.node.to_hex())),
            "{out}"
        );
        let author = restyle(&svg(&rect("#123456")), &p);
        assert!(
            author.contains(r##"fill="#123456""##),
            "author colors are untouched"
        );
        let styled = restyle(
            &svg(&format!(
                r#"<rect style="fill:{};stroke:{}"/>"#,
                p.canvas.to_hex(),
                p.border.to_hex()
            )),
            &p,
        );
        assert!(
            styled.contains(&format!("fill:{}", p.node.to_hex())),
            "{styled}"
        );
    }

    #[test]
    fn a_default_styled_decision_diamond_takes_the_accent_wash() {
        let p = palette();
        let diamond =
            |attrs: &str, points: &str| format!(r#"<polygon points="{points}" {attrs}/>"#);
        let default = format!(
            r#"fill="{}" stroke="{}""#,
            p.node.to_hex(),
            p.border.to_hex()
        );
        let out = restyle(&svg(&diamond(&default, "50,0 100,30 50,60 0,30")), &p);
        assert!(
            out.contains(&p.accent_wash.to_hex()) && out.contains(&p.accent_line.to_hex()),
            "{out}"
        );
        // Themed by the stylesheet only (a node polygon without paint of its own).
        let themed = restyle(
            &svg(&diamond(
                r#"class="label-container""#,
                "50,0 100,30 50,60 0,30",
            )),
            &p,
        );
        assert!(themed.contains(&p.accent_wash.to_hex()), "{themed}");
        // Explicit author colors stay.
        let author = restyle(
            &svg(&diamond(
                r##"fill="#ff0000" stroke="#00ff00""##,
                "50,0 100,30 50,60 0,30",
            )),
            &p,
        );
        assert!(author.contains("#ff0000") && !author.contains(&p.accent_wash.to_hex()));
        // A skewed quadrilateral is not a diamond (top and bottom points are not aligned).
        let skew = restyle(&svg(&diamond(&default, "50,0 100,30 55,60 0,30")), &p);
        assert!(!skew.contains(&p.accent_wash.to_hex()));
        // Within 0.05 still counts.
        let near = restyle(&svg(&diamond(&default, "50,0 100,30 50.04,60 0,30.04")), &p);
        assert!(near.contains(&p.accent_wash.to_hex()));
        // Three points (an arrow head) never does.
        let arrow = restyle(
            &svg(&diamond(r#"class="label-container""#, "0,7 11.5,14 11.5,0")),
            &p,
        );
        assert!(!arrow.contains(&p.accent_wash.to_hex()));
    }

    #[test]
    fn the_default_grid_stroke_becomes_the_grid_color() {
        let p = palette();
        let out = restyle(
            &svg(
                r##"<style>.grid{stroke:#E2E8F0}</style><line stroke="#e2e8f0"/><line stroke="#123456"/><path style="stroke:#E2E8F0;fill:none"/>"##,
            ),
            &p,
        );
        assert!(!out.to_ascii_lowercase().contains("e2e8f0"), "{out}");
        assert_eq!(out.matches(&p.grid.to_hex()).count(), 3);
        assert!(out.contains("#123456"));
    }

    #[test]
    fn untouched_tags_are_copied_byte_for_byte() {
        let input = svg(r#"<g class="x"><path d="M 0 0  L 10 10" /></g><text>a &lt; b</text>"#);
        let out = restyle(&input, &palette());
        assert!(
            out.contains(r#"<g class="x"><path d="M 0 0  L 10 10" /></g><text>a &lt; b</text>"#)
        );
    }
}
