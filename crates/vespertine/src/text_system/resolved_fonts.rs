use super::{Font, FontId};
use crate::PlatformTextSystem;
use std::{
    cell::RefCell,
    sync::{Arc, Weak},
};

/// Resolved fonts remembered per thread.
pub(crate) const RESOLVED_FONT_CAPACITY: usize = 16;

struct Entry {
    system: Weak<dyn PlatformTextSystem>,
    generation: usize,
    font: Font,
    id: FontId,
}

/// A small per-thread cache of fully resolved fonts (including fallbacks).
///
/// A hit needs the complete requested font, the font generation of the text system and the
/// identity of the live platform text system. Entries hold weak references so a dropped system is
/// never kept alive; a weak reference keeps its allocation reserved, so a system created later can
/// never be mistaken for a dead one at the same address.
#[derive(Default)]
pub(crate) struct ResolvedFontCache {
    /// Oldest inserted first.
    entries: Vec<Entry>,
}

impl ResolvedFontCache {
    pub(crate) fn get(
        &self,
        system: &Arc<dyn PlatformTextSystem>,
        generation: usize,
        font: &Font,
    ) -> Option<FontId> {
        let address = Arc::as_ptr(system).cast::<()>();
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.generation == generation
                    && Weak::as_ptr(&entry.system).cast::<()>() == address
                    && entry.system.strong_count() > 0
                    && entry.font == *font
            })
            .map(|entry| entry.id)
    }

    pub(crate) fn insert(
        &mut self,
        system: &Arc<dyn PlatformTextSystem>,
        generation: usize,
        font: &Font,
        id: FontId,
    ) {
        self.entries.retain(|entry| entry.system.strong_count() > 0);
        if self.entries.len() >= RESOLVED_FONT_CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(Entry {
            system: Arc::downgrade(system),
            generation,
            font: font.clone(),
            id,
        });
    }

    /// Entries held, and how many of them still have a live system.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize) {
        (
            self.entries.len(),
            self.entries
                .iter()
                .filter(|entry| entry.system.strong_count() > 0)
                .count(),
        )
    }
}

thread_local! {
    static RESOLVED_FONTS: RefCell<ResolvedFontCache> = RefCell::new(ResolvedFontCache::default());
}

pub(crate) fn lookup(
    system: &Arc<dyn PlatformTextSystem>,
    generation: usize,
    font: &Font,
) -> Option<FontId> {
    RESOLVED_FONTS.with(|cache| cache.borrow().get(system, generation, font))
}

pub(crate) fn remember(
    system: &Arc<dyn PlatformTextSystem>,
    generation: usize,
    font: &Font,
    id: FontId,
) {
    RESOLVED_FONTS.with(|cache| cache.borrow_mut().insert(system, generation, font, id));
}

#[cfg(test)]
pub(crate) fn counts() -> (usize, usize) {
    RESOLVED_FONTS.with(|cache| cache.borrow().counts())
}

#[cfg(test)]
pub(crate) fn clear() {
    RESOLVED_FONTS.with(|cache| *cache.borrow_mut() = ResolvedFontCache::default());
}
