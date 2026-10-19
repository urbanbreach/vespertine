//! PERF-PRF-017: a long streaming block document with incremental updates (Part A) and 200
//! expanded media rows under a residency budget (Part B) (spec PRF-017, performance workload).
//!
//! Part A (frames 0..600): 10,000 settled one-line paragraphs and a growing trailing paragraph,
//! pinned to the bottom of the viewport; every frame appends 20 characters to the trailing block.
//! `PERF-PRF-017` updates the document incrementally; `PERF-PRF-017-FULL` is the deliberately
//! naive consumer that re-parses and re-measures the whole text on every update.
//!
//! Part B (frames 600..1040): 200 rows of 220 px, each with a label and a 512 x 192 px procedural
//! image under a 64 MiB residency budget; rows are requested while they intersect the viewport
//! and released when they leave. Part B is identical in both scenes.

use crate::common::{INK, SANS, WHITE, hex};
use crate::corpus;
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{
    AnyElement, App, AppContext as _, Entity, Font, ImageSource, IntoElement, ParentElement,
    RenderImage, SharedString, Styled, TextRun, WindowTextSystem, Window, div, font, hsla, img,
    px,
};
use vespertine_residency::{
    BlockDocument, BlockModel, ImageResidency, ParsedBlock, ResidencyConfig,
};

const PART_B_START: u64 = 600;
const WINDOW_HEIGHT: f32 = 480.0;
const LINE_HEIGHT: f32 = 20.0;
const TEXT_WIDTH: f32 = 776.0;
const LEFT_PADDING: f32 = 12.0;
const SETTLED_BLOCKS: usize = 10_000;
const ROWS: u32 = 200;
const ROW_HEIGHT: f32 = 220.0;
const IMAGE_WIDTH: u32 = 1024;
const IMAGE_HEIGHT: u32 = 384;
const CLASS: &str = "preview";

/// Paragraphs separated by a blank line; a paragraph is open until its blank line has arrived.
/// A block is measured by shaping it at the text width: its height in logical pixels.
struct Paragraphs {
    text_system: Arc<WindowTextSystem>,
    font: Font,
}

impl BlockModel for Paragraphs {
    type Block = ();
    type Layout = f32;

    fn parse(&self, source: &str, start: usize) -> ParsedBlock<()> {
        match source[start..].find("\n\n") {
            Some(at) => ParsedBlock {
                block: (),
                end: start + at + 2,
                open: false,
            },
            None => ParsedBlock {
                block: (),
                end: source.len(),
                open: true,
            },
        }
    }

    fn measure(&self, _block: &(), text: &str) -> f32 {
        let text = text.trim_end_matches('\n');
        if text.is_empty() {
            return LINE_HEIGHT;
        }
        let run = TextRun {
            len: text.len(),
            font: self.font.clone(),
            color: hex(INK),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        self.text_system
            .shape_text(
                SharedString::from(text.to_string()),
                px(14.0),
                &[run],
                Some(px(TEXT_WIDTH)),
                None,
            )
            .map_or(LINE_HEIGHT, |lines| {
                lines
                    .iter()
                    .map(|line| f32::from(line.size(px(LINE_HEIGHT)).height))
                    .sum()
            })
    }
}

/// Characters 0..19 of corpus line `j mod 1000`, the settled block `j`.
fn settled_block(j: usize) -> String {
    corpus::line(j % 1000)[..20].to_string()
}

/// Characters 20..39 of corpus line `f mod 1000`: the chunk appended before frame `f`.
fn chunk(f: u64) -> String {
    corpus::line((f % 1000) as usize)[20..40].to_string()
}

fn settled_text() -> String {
    let mut text = String::with_capacity(SETTLED_BLOCKS * 22);
    for j in 0..SETTLED_BLOCKS {
        text.push_str(&settled_block(j));
        text.push_str("\n\n");
    }
    text
}

/// Rows whose 220 px band intersects the viewport at frame `f` of Part B.
fn visible_rows(f: u64) -> std::ops::RangeInclusive<u32> {
    let offset = scroll_offset(f);
    let first = (offset / ROW_HEIGHT).floor() as u32;
    let last = (((offset + WINDOW_HEIGHT) / ROW_HEIGHT).ceil() as u32).saturating_sub(1);
    first..=last.min(ROWS - 1)
}

/// Scroll offset of Part B at frame `f`: 100 logical px per frame.
fn scroll_offset(f: u64) -> f32 {
    100.0 * (f - PART_B_START) as f32
}

/// The tops of the blocks that reach into the viewport when the document is pinned to the
/// bottom: `heights` are the block heights from the last block backwards; a blank line separates
/// consecutive blocks. Returns `(distance from the last block, top)` pairs.
fn pinned_tops(heights: impl Iterator<Item = f32>) -> Vec<(usize, f32)> {
    let mut visible = Vec::new();
    let mut bottom = WINDOW_HEIGHT;
    for (back, height) in heights.enumerate() {
        let top = bottom - height;
        visible.push((back, top));
        if top <= 0.0 {
            break;
        }
        bottom = top - LINE_HEIGHT;
    }
    visible
}

/// A procedural 1024 x 384 BGRA image: a vertical gradient from HSL(hue, 60%, 70%) at the top to
/// HSL(hue + 60, 60%, 40%) at the bottom, with `hue = (row * 29) mod 360` degrees.
fn gradient_image(row: u32) -> Arc<RenderImage> {
    let hue = ((row * 29) % 360) as f32;
    let mut bytes = Vec::with_capacity((IMAGE_WIDTH * IMAGE_HEIGHT * 4) as usize);
    for y in 0..IMAGE_HEIGHT {
        let t = y as f32 / (IMAGE_HEIGHT - 1) as f32;
        let color = vespertine::Rgba::from(hsla(
            ((hue + 60.0 * t) % 360.0) / 360.0,
            0.6,
            0.7 - 0.3 * t,
            1.0,
        ));
        let pixel = [
            (color.b * 255.0).round() as u8,
            (color.g * 255.0).round() as u8,
            (color.r * 255.0).round() as u8,
            255,
        ];
        for _ in 0..IMAGE_WIDTH {
            bytes.extend_from_slice(&pixel);
        }
    }
    Arc::new(
        RenderImage::from_bgra(IMAGE_WIDTH, IMAGE_HEIGHT, bytes).expect("valid image dimensions"),
    )
}

type Residency = ImageResidency<&'static str, u32>;

struct Prf017 {
    /// Re-parse and re-measure the whole text on every update (the naive consumer).
    full_reparse: bool,
    document: Option<BlockDocument<Paragraphs>>,
    /// The last frame of Part A applied to the document.
    applied: Option<u64>,
    residency: Option<Entity<Residency>>,
    /// Rows requested at the last Part B frame.
    requested: Vec<u32>,
}

impl Prf017 {
    fn new(full_reparse: bool) -> Self {
        Self {
            full_reparse,
            document: None,
            applied: None,
            residency: None,
            requested: Vec::new(),
        }
    }

    fn paragraphs(text_system: &Arc<WindowTextSystem>) -> Paragraphs {
        Paragraphs {
            text_system: text_system.clone(),
            font: font(SANS),
        }
    }

    /// Applies the document updates up to frame `f`: frame 0 is a full load, every later frame
    /// appends its chunk.
    fn update_document(&mut self, f: u64, text_system: &Arc<WindowTextSystem>) {
        let from = match self.applied {
            Some(applied) if applied >= f => return,
            Some(applied) => applied + 1,
            None => 0,
        };
        for frame in from..=f {
            if frame == 0 {
                let mut text = settled_text();
                text.push_str(&chunk(0));
                let mut document = BlockDocument::new(Self::paragraphs(text_system));
                document.update(&text);
                self.document = Some(document);
            } else if self.full_reparse {
                let document = self.document.as_ref().expect("loaded at frame 0");
                let mut text = document.source().to_string();
                text.push_str(&chunk(frame));
                let mut fresh = BlockDocument::new(Self::paragraphs(text_system));
                fresh.update(&text);
                self.document = Some(fresh);
            } else {
                let piece = chunk(frame);
                self.document
                    .as_mut()
                    .expect("loaded at frame 0")
                    .append(&piece);
            }
        }
        self.applied = Some(f);
    }

    /// Requests the rows visible at frame `f` of Part B (generating the image of a row that is
    /// not resident) and releases the rows that left; returns the visible rows with their
    /// images.
    fn request_rows(&mut self, f: u64, cx: &mut App) -> Vec<(u32, Arc<RenderImage>)> {
        let residency = self
            .residency
            .get_or_insert_with(|| cx.new(|_| Residency::new(ResidencyConfig::PREVIEWS)))
            .clone();
        let visible: Vec<u32> = visible_rows(f).collect();
        for row in &self.requested {
            if !visible.contains(row) {
                residency.update(cx, |residency, cx| residency.release(&CLASS, row, cx));
            }
        }
        let mut rows = Vec::with_capacity(visible.len());
        for &row in &visible {
            let image = match residency.update(cx, |r, cx| r.request(&CLASS, &row, cx)) {
                Some(image) => image,
                None => {
                    let image = gradient_image(row);
                    residency.update(cx, |r, cx| r.insert(&CLASS, row, image.clone(), cx));
                    image
                }
            };
            rows.push((row, image));
        }
        self.requested = visible;
        rows
    }

    fn part_a(&mut self, f: u64, text_system: &Arc<WindowTextSystem>) -> AnyElement {
        self.update_document(f, text_system);
        let document = self.document.as_ref().expect("loaded at frame 0");
        let blocks = document.blocks();
        let tops = pinned_tops(blocks.iter().rev().map(|entry| entry.layout));
        let mut root = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .line_height(px(LINE_HEIGHT))
            .text_color(hex(INK));
        for (back, top) in tops {
            let entry = &blocks[blocks.len() - 1 - back];
            let text = document.text(entry).trim_end_matches('\n').to_string();
            root = root.child(
                div()
                    .absolute()
                    .left(px(LEFT_PADDING))
                    .top(px(top))
                    .w(px(TEXT_WIDTH))
                    .child(text),
            );
        }
        root.into_any_element()
    }

    fn part_b(&mut self, f: u64, cx: &mut App) -> AnyElement {
        let offset = scroll_offset(f);
        let mut root = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .line_height(px(LINE_HEIGHT))
            .text_color(hex(INK));
        for (row, image) in self.request_rows(f, cx) {
            root = root.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(row as f32 * ROW_HEIGHT - offset))
                    .w(px(512.0))
                    .h(px(ROW_HEIGHT))
                    .child(div().h(px(LINE_HEIGHT)).child(format!("Media {row}")))
                    .child(
                        img(ImageSource::Render(image))
                            .w(px(512.0))
                            .h(px(192.0)),
                    ),
            );
        }
        root.into_any_element()
    }
}

impl Scene for Prf017 {
    fn size(&self) -> (f32, f32) {
        (800.0, WINDOW_HEIGHT)
    }

    fn frame(&mut self, f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        if f < PART_B_START {
            let text_system = window.text_system().clone();
            self.part_a(f, &text_system)
        } else {
            self.part_b(f, cx)
        }
    }
}

register_scene!("PERF-PRF-017", || Prf017::new(false));
register_scene!("PERF-PRF-017-FULL", || Prf017::new(true));

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use vespertine::TestAppContext;

    #[test]
    fn part_b_rows_follow_the_scroll_and_cover_the_whole_list() {
        assert_eq!(visible_rows(600), 0..=2);
        assert_eq!(visible_rows(601), 0..=2, "100 px: row 0 still reaches 100..220");
        assert_eq!(visible_rows(603), 1..=3);
        let mut seen = std::collections::BTreeSet::new();
        for f in 600..1040 {
            seen.extend(visible_rows(f));
        }
        assert_eq!(seen.len(), ROWS as usize);
        assert_eq!(*visible_rows(1039).end(), ROWS - 1);
    }

    #[test]
    fn the_gradient_has_the_specified_size_and_endpoints() {
        let image = gradient_image(1);
        assert_eq!(vespertine_residency::decoded_bytes(&image), 1_572_864);
        let bytes = image.as_bytes(0).unwrap();
        // Row 1: hue 29 deg; top is HSL(29, 60%, 70%), bottom HSL(89, 60%, 40%).
        let pixel = |y: usize| {
            let at = y * IMAGE_WIDTH as usize * 4;
            [bytes[at + 2], bytes[at + 1], bytes[at]]
        };
        let top = pixel(0);
        assert!(top[0] > top[1] && top[1] > top[2], "orange-ish top: {top:?}");
        let bottom = pixel(IMAGE_HEIGHT as usize - 1);
        assert!(bottom[1] > bottom[0] && bottom[1] > bottom[2], "green-ish bottom: {bottom:?}");
    }

    #[test]
    fn the_last_block_is_pinned_to_the_bottom() {
        let tops = pinned_tops([60.0, 20.0, 20.0].into_iter().chain(std::iter::repeat(20.0)));
        assert_eq!(tops[0], (0, 420.0), "trailing block bottom at the viewport bottom");
        assert_eq!(tops[1], (1, 380.0), "a blank line separates blocks");
        assert!(tops.last().unwrap().1 <= 0.0);
        assert_eq!(tops.len(), 12);
    }

    #[vespertine::test]
    fn part_a_incremental_and_full_reparse_agree_and_only_the_naive_consumer_redoes_settled_blocks(
        cx: &mut TestAppContext,
    ) {
        let text_system = cx.update(|cx| Arc::new(WindowTextSystem::new(cx.text_system().clone())));
        let mut incremental = Prf017::new(false);
        let mut naive = Prf017::new(true);
        let mut incremental_parsed = 0;
        for f in 0..6u64 {
            incremental.update_document(f, &text_system);
            naive.update_document(f, &text_system);
            if f > 0 {
                let stats = incremental.document.as_ref().unwrap().stats();
                incremental_parsed = stats.blocks_parsed;
            }
            let (a, b) = (
                incremental.document.as_ref().unwrap(),
                naive.document.as_ref().unwrap(),
            );
            assert_eq!(a.source(), b.source());
            assert_eq!(a.blocks().len(), SETTLED_BLOCKS + 1, "the trailing block has index 10,000");
            let layouts = |d: &BlockDocument<Paragraphs>| {
                d.blocks().iter().map(|e| (e.range.clone(), e.layout)).collect::<Vec<_>>()
            };
            assert_eq!(layouts(a), layouts(b), "frame {f}");
        }
        // The incremental document parsed the load once and one block per later update.
        assert_eq!(incremental_parsed, SETTLED_BLOCKS as u64 + 1 + 5);
        let naive_parsed = naive.document.as_ref().unwrap().stats().blocks_parsed;
        assert_eq!(naive_parsed, SETTLED_BLOCKS as u64 + 1, "its last fresh document parsed everything");
    }

    #[vespertine::test]
    fn part_b_retains_at_most_the_budget_and_releases_what_leaves(cx: &mut TestAppContext) {
        let mut scene = Prf017::new(false);
        let mut peak = 0;
        for f in PART_B_START..1040 {
            let rows = cx.update(|cx| scene.request_rows(f, cx));
            assert!(rows.len() <= 4);
            cx.executor().advance_clock(Duration::from_micros(16_667));
            cx.run_until_parked();
            let residency = scene.residency.clone().unwrap();
            let resident = residency.read_with(cx, |r, _| r.resident_bytes());
            peak = peak.max(resident);
            assert!(resident <= 64 * 1024 * 1024, "frame {f}: {resident}");
        }
        println!("PERF-PRF-017 part B: peak resident media bytes {peak} (budget 67108864)");
        assert!(peak <= 42 * 1_572_864);
        for _ in 0..40 {
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
        }
        let residency = scene.residency.clone().unwrap();
        let stats = residency.read_with(cx, |r, _| r.stats(&CLASS));
        assert_eq!(stats.inserts, ROWS as u64, "each row's image is generated once");
        assert!(stats.requested <= 4);
    }
}
