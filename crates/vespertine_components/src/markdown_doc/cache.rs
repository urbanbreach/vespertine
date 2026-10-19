//! The per-row flatten cache (CMP-011 R18): flattened text, runs and links of settled blocks,
//! keyed by (top-level block, element) and invalidated by block generation and style identity,
//! so per-frame work for a streaming row is O(tail block).

use super::flatten::Flat;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use vespertine_theme::document::DocumentTheme;

/// Hash of everything that changes how a block flattens or lays out: the colors, the font
/// families and every layout metric of the theme.
pub fn style_identity(theme: &DocumentTheme) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    theme.metrics.layout_hash().hash(&mut hasher);
    theme.font_sans.hash(&mut hasher);
    theme.font_mono.hash(&mut hasher);
    for color in [
        theme.text,
        theme.text_muted,
        theme.code_text,
        theme.code_wash,
        theme.accent,
        theme.border,
        theme.bg,
    ] {
        [color.r, color.g, color.b, color.a]
            .map(f32::to_bits)
            .hash(&mut hasher);
    }
    hasher.finish()
}

struct Entry {
    generation: u64,
    style: u64,
    flat: Flat,
}

/// A flatten cache. Generations are assigned per top-level block by the document: blocks before
/// the stable prefix of an append keep theirs.
#[derive(Default)]
pub struct FlattenCache {
    entries: HashMap<(usize, usize), Entry>,
    hits: u64,
    misses: u64,
}

impl FlattenCache {
    /// The cached flat for `(top, element)` when its generation and style identity still match,
    /// else the result of `build` (which is then cached).
    pub fn get_or_insert(
        &mut self,
        top: usize,
        element: usize,
        generation: u64,
        style: u64,
        build: impl FnOnce() -> Flat,
    ) -> Flat {
        if let Some(entry) = self.entries.get(&(top, element))
            && entry.generation == generation
            && entry.style == style
        {
            self.hits += 1;
            return entry.flat.clone();
        }
        self.misses += 1;
        let flat = build();
        self.entries.insert(
            (top, element),
            Entry {
                generation,
                style,
                flat: flat.clone(),
            },
        );
        flat
    }

    /// Drops entries of blocks at or after `top` (the document changed from there on) and of
    /// blocks past `len`.
    pub fn truncate(&mut self, len: usize) {
        self.entries.retain(|(top, _), _| *top < len);
    }

    /// Cache hits so far.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Cache misses (flattenings performed) so far.
    pub fn misses(&self) -> u64 {
        self.misses
    }
}
