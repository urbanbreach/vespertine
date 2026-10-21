//! PERF-TXT-007: two code blocks (TypeScript and Rust documents) through the bounded syntax
//! cache; a new pair of documents every third frame and a palette change every 150 frames.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px, rgb};
use vespertine_components::code::CodeBlock;
use vespertine_richtext::syntax::{HighlightCache, LanguageHint, RetainedHighlight};
use vespertine_theme::document::DocumentTheme;

/// TypeScript document `n`: 40 lines, a pure function of `n`.
pub fn typescript_document(n: u64) -> String {
    let mut out = String::new();
    for k in 1..=40u64 {
        if k % 2 == 1 {
            out.push_str(&format!(
                "export const item{n}_{k}: string = \"value-{n}-{k}\"; // note {k}\n"
            ));
        } else {
            out.push_str(&format!(
                "function f{n}_{k}(x: number): number {{ return x * {k} + {n}; }}\n"
            ));
        }
    }
    out
}

/// Rust document `n`: 40 lines, a pure function of `n`.
pub fn rust_document(n: u64) -> String {
    let mut out = String::new();
    for k in 1..=40u64 {
        if k % 2 == 1 {
            out.push_str(&format!(
                "pub const ITEM_{n}_{k}: u32 = {}; // note {k}\n",
                n * k
            ));
        } else {
            out.push_str(&format!("fn f{n}_{k}(x: u32) -> u32 {{ x * {k} + {n} }}\n"));
        }
    }
    out
}

/// Theme A (THM-002 dark defaults) when `floor(f / 150)` is even, theme B otherwise.
pub fn theme(f: u64) -> DocumentTheme {
    let mut theme = DocumentTheme::dark();
    theme.text = rgb(0xEEEEEE);
    if !(f / 150).is_multiple_of(2) {
        theme.syntax.keyword = rgb(0xFF6688);
        theme.syntax.string = rgb(0x66DDAA);
        theme.syntax.comment = rgb(0x99AA88);
        theme.syntax.number = rgb(0xFFCC44);
    }
    theme
}

pub struct Txt007Perf {
    cache: Arc<HighlightCache>,
    document: Option<u64>,
    typescript: Arc<str>,
    rust: Arc<str>,
    typescript_handle: RetainedHighlight,
    rust_handle: RetainedHighlight,
}

impl Txt007Perf {
    pub fn new(cache: Arc<HighlightCache>) -> Self {
        Self {
            cache,
            document: None,
            typescript: Arc::from(""),
            rust: Arc::from(""),
            typescript_handle: RetainedHighlight::new(),
            rust_handle: RetainedHighlight::new(),
        }
    }

    /// Applies the update for frame `f` and builds the two blocks (idempotent per `f`).
    pub fn views(&mut self, f: u64) -> (CodeBlock, CodeBlock) {
        let n = f / 3;
        if self.document != Some(n) {
            self.typescript = Arc::from(typescript_document(n));
            self.rust = Arc::from(rust_document(n));
            self.document = Some(n);
        }
        let typescript = self
            .typescript_handle
            .resolve(
                &self.cache,
                &self.typescript,
                LanguageHint {
                    fence_label: Some("typescript"),
                    path: None,
                },
            )
            .cloned();
        let rust = self
            .rust_handle
            .resolve(
                &self.cache,
                &self.rust,
                LanguageHint {
                    fence_label: Some("rust"),
                    path: None,
                },
            )
            .cloned();
        let left = CodeBlock::new("perf-txt-007-typescript", &*self.typescript)
            .language("typescript")
            .highlight(typescript)
            .theme(theme(f))
            .font_family("Liberation Mono")
            .text_size(px(12.))
            .line_height(px(16.))
            .absolute()
            .left(px(20.))
            .top(px(20.))
            .w(px(430.))
            .h(px(460.));
        let right = CodeBlock::new("perf-txt-007-rust", &*self.rust)
            .language("rust")
            .highlight(rust)
            .theme(theme(f))
            .font_family("Liberation Mono")
            .text_size(px(12.))
            .line_height(px(16.))
            .absolute()
            .left(px(470.))
            .top(px(20.))
            .w(px(410.))
            .h(px(460.));
        (left, right)
    }
}

impl Scene for Txt007Perf {
    fn size(&self) -> (f32, f32) {
        (900.0, 500.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let (left, right) = self.views(f);
        div()
            .size_full()
            .bg(hex(0x181818))
            .child(left)
            .child(right)
            .into_any_element()
    }
}

register_scene!("PERF-TXT-007", || Txt007Perf::new(HighlightCache::global()));

#[cfg(test)]
mod tests {
    use super::*;

    /// Replays the PERF-TXT-007 workload through the scene's own view construction and prints
    /// the cache statistics (the measurements of the A8 workload).
    #[test]
    fn perf_txt_007_workload_parses_once_per_document_and_never_on_warm_frames() {
        let cache = Arc::new(HighlightCache::new());
        let mut scene = Txt007Perf::new(cache.clone());
        let mut resolve_times = Vec::new();
        for f in 0..720u64 {
            let before = cache.stats();
            let started = std::time::Instant::now();
            let (left, right) = scene.views(f);
            drop((left, right));
            let elapsed = started.elapsed();
            if f == 0 {
                println!("frame 0 resolve (two new documents, grammar load included): {elapsed:?}");
            } else if f % 3 == 0 {
                resolve_times.push(elapsed);
            }
            let after = cache.stats();
            if f % 3 != 0 {
                assert_eq!(before, after, "warm frame {f} must not touch the cache");
            }
        }
        resolve_times.sort();
        println!(
            "resolving a new document pair (hash + parse both): min {:?}, median {:?}",
            resolve_times[0],
            resolve_times[resolve_times.len() / 2]
        );
        let stats = cache.stats();
        println!(
            "PERF-TXT-007 replay (720 frames, 240 document pairs): parses={} hits={} misses={} documents={} retained_bytes={}",
            stats.parses, stats.hits, stats.misses, stats.documents, stats.retained_bytes
        );
        assert_eq!(stats.parses, 480);
        assert_eq!(stats.hits, 0);
        assert!(stats.documents <= 96);
        assert!(stats.retained_bytes <= 25_165_824);
    }
}
