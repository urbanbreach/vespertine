//! The source cache of the checklist panel (CMP-028 R20).
//!
//! The model (summary, focus window, rows, signature, expanded-state policy, dismissal) and the
//! panel are the status components' (CMP-023 R8-R12, [`crate::status::StatusSection`]); this
//! module adds only the cache of the latest list per source.

use std::collections::HashMap;
use std::rc::Rc;

use vespertine::SharedString;

use crate::status::checklist::Item;

/// The cache of the latest list per source (R20): a list is recomputed only when the source's
/// revision or length changes.
#[derive(Default)]
pub struct ListCache {
    entries: HashMap<SharedString, (u64, usize, Rc<[Item]>)>,
    builds: usize,
}

impl ListCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The list of `source` at `revision` with `len` items: the cached one when the key
    /// `(source, revision, len)` is unchanged, else the result of `build`.
    pub fn get_or_build(
        &mut self,
        source: &SharedString,
        revision: u64,
        len: usize,
        build: impl FnOnce() -> Vec<Item>,
    ) -> Rc<[Item]> {
        if let Some((cached_revision, cached_len, items)) = self.entries.get(source)
            && *cached_revision == revision
            && *cached_len == len
        {
            return items.clone();
        }
        let items: Rc<[Item]> = build().into();
        self.builds += 1;
        self.entries
            .insert(source.clone(), (revision, len, items.clone()));
        items
    }

    /// How many times a list was (re)built.
    pub fn builds(&self) -> usize {
        self.builds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::checklist::ItemStatus;

    fn items(count: usize) -> Vec<Item> {
        (0..count)
            .map(|index| Item::new(format!("item {index}"), ItemStatus::Pending))
            .collect()
    }

    #[test]
    fn the_cache_rebuilds_only_when_revision_or_length_change() {
        let mut cache = ListCache::new();
        let source: SharedString = "conversation-1".into();
        let first = cache.get_or_build(&source, 1, 2, || items(2));
        let again = cache.get_or_build(&source, 1, 2, || unreachable!("cached"));
        assert!(Rc::ptr_eq(&first, &again));
        assert_eq!(cache.builds(), 1);
        cache.get_or_build(&source, 2, 2, || items(2));
        cache.get_or_build(&source, 2, 3, || items(3));
        assert_eq!(cache.builds(), 3);
        let other: SharedString = "conversation-2".into();
        cache.get_or_build(&other, 1, 2, || items(2));
        assert_eq!(cache.builds(), 4);
    }
}
