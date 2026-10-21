//! Flattening of inline runs into shaped-text inputs (CMP-011 R9, R10, R20): the displayed
//! text, one [`TextRun`] per style change, link ranges, and inline-code pill ranges.

use super::tree::{PENDING_LINK, Run};
#[cfg(feature = "math")]
use crate::math::{MathStyle, MathText, Presentation, present_inline, slot_font};
use std::ops::Range;
use vespertine::{
    FontStyle, FontWeight, Hsla, Pixels, SharedString, StrikethroughStyle, TextRun, UnderlineStyle,
    font, px,
};
use vespertine_richtext::Link;
use vespertine_theme::document::DocumentTheme;

use crate::doc_theme::hsla;

/// The base text style of a block.
#[derive(Clone, Debug, PartialEq)]
pub struct TextCtx {
    /// Plain text color.
    pub color: Hsla,
    /// Base weight; bold runs render at semibold when it is lighter, and at bold when the base
    /// is itself semibold or heavier.
    pub weight: FontWeight,
    /// Font size.
    pub size: Pixels,
    /// Line height.
    pub line_height: Pixels,
}

impl TextCtx {
    /// The body style of a theme: size 14, line height 22, the text token.
    pub fn body(theme: &DocumentTheme) -> Self {
        Self {
            color: hsla(theme.text),
            weight: FontWeight::NORMAL,
            size: px(theme.metrics.md_text_size),
            line_height: px(theme.metrics.md_line_height),
        }
    }

    /// The same style in another color.
    pub fn with_color(&self, color: Hsla) -> Self {
        Self {
            color,
            ..self.clone()
        }
    }
}

/// The flattened inline content of a block.
#[derive(Clone, Debug)]
pub struct Flat {
    /// The displayed text.
    pub text: String,
    /// Runs covering the text exactly.
    pub runs: Vec<TextRun>,
    /// Link ranges with their complete destinations (pending links are not included).
    pub links: Vec<Link>,
    /// Merged inline-code ranges (one wash box per wrapped row).
    pub pills: Vec<Range<usize>>,
    /// The paragraph with typeset inline formulas (CMP-012): the displayed text of `text` with a
    /// reserved slot per formula and the original text with the delimited sources. `None` when
    /// no formula is typeset.
    #[cfg(feature = "math")]
    pub math: Option<MathText>,
}

impl Flat {
    /// Whether there is no text.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Flattens `runs`. Task markers are shown as ordinary text unless `hide_task_marker`. Formula
/// runs are typeset (a reserved slot in the slot font) under `math`, else shown as inline code
/// with their delimiters (CMP-012 R6).
pub fn flatten(
    runs: &[Run],
    theme: &DocumentTheme,
    ctx: &TextCtx,
    hide_task_marker: bool,
    #[cfg(feature = "math")] math: &MathStyle,
) -> Flat {
    let muted = hsla(theme.text_muted);
    #[cfg(feature = "math")]
    let mut paragraph = runs
        .iter()
        .any(|run| run.formula.is_some())
        .then(MathText::new);
    let mut text = String::new();
    let mut out: Vec<TextRun> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut pills: Vec<Range<usize>> = Vec::new();
    for run in runs {
        if run.task.is_some() && hide_task_marker {
            continue;
        }
        if run.text.is_empty() {
            continue;
        }
        let start = text.len();
        // A formula that typesets reserves a slot of no-break spaces; one that does not (math
        // disabled, refused, too tall or too wide, or the `math` feature off) is its delimited
        // source as inline code.
        #[cfg(feature = "math")]
        let typeset = run.formula.as_ref().and_then(|source| {
            let paragraph = paragraph.as_mut()?;
            match present_inline(&source.tex, math) {
                Presentation::Formula { formula, slot } => paragraph
                    .push_inline(&source.tex, &formula, math.scale())
                    .then_some(slot),
                _ => None,
            }
        });
        #[cfg(not(feature = "math"))]
        let typeset: Option<String> = None;
        let is_code = typeset.is_none() && (run.style.code || run.formula.is_some());
        let len = match &typeset {
            Some(slot) => {
                text.push_str(slot);
                slot.len()
            }
            None => {
                text.push_str(&run.text);
                #[cfg(feature = "math")]
                if let Some(paragraph) = paragraph.as_mut() {
                    paragraph.push_text(&run.text);
                }
                run.text.len()
            }
        };
        let weight = if run.style.bold {
            if ctx.weight >= FontWeight::SEMIBOLD {
                FontWeight::BOLD
            } else {
                FontWeight::SEMIBOLD
            }
        } else {
            ctx.weight
        };
        let mut run_font = if is_code {
            font(theme.font_mono.clone())
        } else {
            font(theme.font_sans.clone())
        };
        #[cfg(feature = "math")]
        if typeset.is_some() {
            run_font = slot_font();
        }
        if typeset.is_none() {
            run_font.weight = if is_code { FontWeight::NORMAL } else { weight };
            if run.style.italic {
                run_font.style = FontStyle::Italic;
            }
        }
        let color = if is_code {
            hsla(theme.code_text)
        } else {
            ctx.color
        };
        out.push(TextRun {
            len,
            font: run_font,
            color,
            background_color: None,
            underline: run.link.is_some().then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(muted),
                wavy: false,
            }),
            strikethrough: run.style.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(muted),
            }),
            letter_spacing: Pixels::ZERO,
        });
        if is_code {
            match pills.last_mut() {
                Some(last) if last.end == start => last.end = start + len,
                _ => pills.push(start..start + len),
            }
        }
        if let Some(destination) = &run.link
            && destination != PENDING_LINK
        {
            match links.last_mut() {
                Some(last) if last.range.end == start && &last.destination == destination => {
                    last.range.end = start + len
                }
                _ => links.push(Link {
                    range: start..start + len,
                    destination: destination.clone(),
                }),
            }
        }
    }
    #[cfg(feature = "math")]
    let math = paragraph.filter(|paragraph| {
        debug_assert_eq!(paragraph.displayed(), text);
        !paragraph.slots().is_empty()
    });
    Flat {
        text,
        runs: out,
        links,
        pills,
        #[cfg(feature = "math")]
        math,
    }
}

/// A paint-only streaming fade (R18): the last `tail_bytes` bytes of a streaming block are
/// painted at `alpha` times their color alpha. It splits runs at the boundary but never changes
/// the text, the fonts or the run lengths' sum, so shaping and wrapping stay identical.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamingFade {
    /// How many trailing bytes of the block's text fade.
    pub tail_bytes: usize,
    /// The alpha factor of the faded bytes.
    pub alpha: f32,
}

impl Flat {
    /// Applies a streaming fade to the tail of the text.
    pub fn fade_tail(&mut self, fade: StreamingFade) {
        if fade.tail_bytes == 0 || self.text.is_empty() {
            return;
        }
        let mut boundary = self.text.len().saturating_sub(fade.tail_bytes);
        while !self.text.is_char_boundary(boundary) {
            boundary += 1;
        }
        let mut out: Vec<TextRun> = Vec::with_capacity(self.runs.len() + 1);
        let mut offset = 0;
        for run in self.runs.drain(..) {
            let end = offset + run.len;
            if end <= boundary {
                out.push(run);
            } else if offset >= boundary {
                let mut faded = run;
                faded.color.a *= fade.alpha;
                out.push(faded);
            } else {
                let mut head = run.clone();
                head.len = boundary - offset;
                let mut tail = run;
                tail.len = end - boundary;
                tail.color.a *= fade.alpha;
                out.push(head);
                out.push(tail);
            }
            offset = end;
        }
        self.runs = out;
    }
}

/// The label of a heading or cell as a shared string without line breaks (table measurement
/// treats newlines as spaces).
pub fn single_line(text: &str) -> SharedString {
    if text.contains('\n') {
        text.replace('\n', " ").into()
    } else {
        text.to_owned().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_doc::tree::{Run, Style};

    #[test]
    fn a_fade_splits_runs_without_changing_text_or_total_length() {
        let theme = DocumentTheme::dark();
        let ctx = TextCtx::body(&theme);
        let runs = vec![
            Run::text("plain "),
            Run::styled(
                "bold text",
                Style {
                    bold: true,
                    ..Style::default()
                },
                None,
            ),
        ];
        #[cfg(feature = "math")]
        let mut flat = flatten(&runs, &theme, &ctx, false, &MathStyle::new());
        #[cfg(not(feature = "math"))]
        let mut flat = flatten(&runs, &theme, &ctx, false);
        let before = (
            flat.text.clone(),
            flat.runs.iter().map(|r| r.len).sum::<usize>(),
        );
        flat.fade_tail(StreamingFade {
            tail_bytes: 4,
            alpha: 0.5,
        });
        assert_eq!(flat.text, before.0);
        assert_eq!(flat.runs.iter().map(|r| r.len).sum::<usize>(), before.1);
        assert_eq!(flat.runs.len(), 3);
        assert_eq!(flat.runs[2].len, 4);
        assert_eq!(flat.runs[2].color.a, ctx.color.a * 0.5);
        assert_eq!(flat.runs[1].color.a, ctx.color.a);
        assert_eq!(flat.runs[1].font, flat.runs[2].font, "fonts never change");
    }
}
