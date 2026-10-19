//! Bounded derived-result caches (PRF-017 R4, R5).
//!
//! A [`BoundedCache`] holds at most `max_entries` values whose estimated sizes sum to at most
//! `max_bytes`; inserting evicts least-recently-used entries until both limits hold and reading
//! an entry marks it most recently used. Keys must contain every true dependency of the value;
//! [`DependencyKey`] builds such keys, with the theme identity as an explicit, optional part so
//! that theme-neutral results survive a palette change and themed rasters never leak across one.

use std::{
    collections::{BTreeMap, HashMap},
    hash::{DefaultHasher, Hash, Hasher},
};

/// The two limits of one cache class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheLimits {
    /// Most entries retained.
    pub max_entries: usize,
    /// Most estimated bytes retained.
    pub max_bytes: usize,
}

impl CacheLimits {
    /// Syntax token cache: 96 documents, 24 MiB.
    pub const SYNTAX_TOKENS: Self = Self::new(96, 24 * 1024 * 1024);
    /// Formula (math raster) cache: 4,096 entries, 8 MiB.
    pub const FORMULA_RASTER: Self = Self::new(4_096, 8 * 1024 * 1024);
    /// Diagram raster cache: 64 entries, 64 MiB.
    pub const DIAGRAM_RASTER: Self = Self::new(64, 64 * 1024 * 1024);

    /// Limits of `max_entries` entries and `max_bytes` bytes.
    pub const fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries,
            max_bytes,
        }
    }
}

/// Identity of a theme, the only theme input of a [`DependencyKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeId(pub u64);

/// A cache key made of the true dependencies of a value (source content or its hash, language,
/// size, font and density inputs, ...), as a 128-bit digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DependencyKey(u128);

impl DependencyKey {
    /// Starts a key.
    pub fn builder() -> DependencyKeyBuilder {
        DependencyKeyBuilder::default()
    }
}

/// Builder of a [`DependencyKey`]. Parts are order sensitive.
pub struct DependencyKeyBuilder {
    low: DefaultHasher,
    high: DefaultHasher,
}

impl Default for DependencyKeyBuilder {
    fn default() -> Self {
        let mut high = DefaultHasher::new();
        0x9e37_79b9_7f4a_7c15u64.hash(&mut high);
        Self {
            low: DefaultHasher::new(),
            high,
        }
    }
}

impl DependencyKeyBuilder {
    /// Adds one dependency.
    pub fn part(mut self, part: &(impl Hash + ?Sized)) -> Self {
        part.hash(&mut self.low);
        part.hash(&mut self.high);
        self
    }

    /// Adds the theme identity: use it for themed results (rasters baking in colors) and leave
    /// it out for theme-neutral ones (syntax token spans).
    pub fn theme(self, theme: ThemeId) -> Self {
        self.part(&("theme", theme))
    }

    /// The finished key.
    pub fn build(self) -> DependencyKey {
        DependencyKey(((self.high.finish() as u128) << 64) | self.low.finish() as u128)
    }
}

/// Counters of one cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Reads that found their entry.
    pub hits: u64,
    /// Reads that did not.
    pub misses: u64,
    /// Entries retained by an insertion.
    pub insertions: u64,
    /// Entries evicted to make room.
    pub evictions: u64,
    /// Values handed back uncached because their own size exceeded the byte limit.
    pub oversized: u64,
    /// Entries now retained.
    pub entries: usize,
    /// Estimated bytes now retained.
    pub bytes: usize,
}

/// The result of [`BoundedCache::insert`].
#[derive(Debug, PartialEq, Eq)]
pub enum Inserted<V> {
    /// The value is retained; `evicted` least-recently-used entries made room for it.
    Retained {
        /// Number of entries evicted by this insertion.
        evicted: usize,
    },
    /// The value alone exceeds the byte limit (or the cache holds no entries): it is not
    /// retained and is returned to the caller.
    Oversized(V),
}

struct Entry<V> {
    value: V,
    bytes: usize,
    tick: u64,
}

/// A least-recently-used cache bounded by entry count and by estimated bytes.
pub struct BoundedCache<K, V> {
    limits: CacheLimits,
    size_of: Box<dyn Fn(&V) -> usize>,
    entries: HashMap<K, Entry<V>>,
    /// Recency order: the smallest tick is the least recently used entry.
    order: BTreeMap<u64, K>,
    tick: u64,
    bytes: usize,
    stats: CacheStats,
}

impl<K: Clone + Eq + Hash, V> BoundedCache<K, V> {
    /// A cache with `limits` that sizes values with `size_of`.
    pub fn new(limits: CacheLimits, size_of: impl Fn(&V) -> usize + 'static) -> Self {
        Self {
            limits,
            size_of: Box::new(size_of),
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            bytes: 0,
            stats: CacheStats::default(),
        }
    }

    /// The limits.
    pub fn limits(&self) -> CacheLimits {
        self.limits
    }

    /// Replaces the limits and evicts until they hold.
    pub fn set_limits(&mut self, limits: CacheLimits) {
        self.limits = limits;
        self.evict_to_fit(0, 0);
    }

    /// Entries retained.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Estimated bytes retained.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The counters.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            bytes: self.bytes,
            ..self.stats
        }
    }

    /// Whether `key` is retained; does not change recency or the counters.
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// The value of `key` without marking it recently used or counting a read.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|entry| &entry.value)
    }

    /// The value of `key`, marking it most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let Some(entry) = self.entries.get_mut(key) else {
            self.stats.misses += 1;
            return None;
        };
        self.stats.hits += 1;
        self.order.remove(&entry.tick);
        self.tick += 1;
        entry.tick = self.tick;
        self.order.insert(self.tick, key.clone());
        Some(&entry.value)
    }

    /// Inserts `value` as the most recently used entry (replacing an entry of the same key) and
    /// evicts least-recently-used entries until both limits hold. A value whose own size
    /// exceeds the byte limit is not retained: it is returned and any entry of the same key is
    /// dropped (the key's old value is stale).
    pub fn insert(&mut self, key: K, value: V) -> Inserted<V> {
        self.remove(&key);
        let bytes = (self.size_of)(&value);
        if bytes > self.limits.max_bytes || self.limits.max_entries == 0 {
            self.stats.oversized += 1;
            return Inserted::Oversized(value);
        }
        let evicted = self.evict_to_fit(bytes, 1);
        self.tick += 1;
        self.order.insert(self.tick, key.clone());
        self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                tick: self.tick,
            },
        );
        self.bytes += bytes;
        self.stats.insertions += 1;
        Inserted::Retained { evicted }
    }

    /// The value of `key`, computing and inserting it on a miss. An oversized computed value is
    /// returned without being retained.
    pub fn get_or_insert_with(&mut self, key: K, compute: impl FnOnce() -> V) -> V
    where
        V: Clone,
    {
        if let Some(value) = self.get(&key) {
            return value.clone();
        }
        let value = compute();
        self.insert(key, value.clone());
        value
    }

    /// Removes `key`.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.remove(key)?;
        self.order.remove(&entry.tick);
        self.bytes -= entry.bytes;
        Some(entry.value)
    }

    /// Removes every entry for which `keep` is false (for example the themed entries of an
    /// outdated theme).
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let doomed: Vec<K> = self
            .entries
            .iter()
            .filter(|(key, entry)| !keep(key, &entry.value))
            .map(|(key, _)| key.clone())
            .collect();
        for key in doomed {
            self.remove(&key);
        }
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    /// Evicts least-recently-used entries until `incoming_bytes` more bytes and `incoming_entries`
    /// more entries fit both limits.
    fn evict_to_fit(&mut self, incoming_bytes: usize, incoming_entries: usize) -> usize {
        let mut evicted = 0;
        while self.entries.len() + incoming_entries > self.limits.max_entries
            || self.bytes + incoming_bytes > self.limits.max_bytes
        {
            let Some((_, key)) = self.order.pop_first() else {
                break;
            };
            let entry = self.entries.remove(&key).expect("ordered entry exists");
            self.bytes -= entry.bytes;
            self.stats.evictions += 1;
            evicted += 1;
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(limits: CacheLimits) -> BoundedCache<u32, usize> {
        BoundedCache::new(limits, |bytes| *bytes)
    }

    fn keys(cache: &BoundedCache<u32, usize>) -> Vec<u32> {
        let mut keys: Vec<u32> = cache.entries.keys().copied().collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn entry_limit_evicts_least_recently_used_and_reads_refresh_recency() {
        let mut cache = cache(CacheLimits::new(3, usize::MAX));
        for key in 0..3 {
            assert_eq!(cache.insert(key, 1), Inserted::Retained { evicted: 0 });
        }
        // Reading 0 makes 1 the least recently used.
        assert!(cache.get(&0).is_some());
        assert_eq!(cache.insert(3, 1), Inserted::Retained { evicted: 1 });
        assert_eq!(keys(&cache), vec![0, 2, 3]);
        // peek does not refresh: 2 is the least recently used and goes next.
        assert!(cache.peek(&2).is_some());
        cache.insert(4, 1);
        assert_eq!(keys(&cache), vec![0, 3, 4]);
        assert!(cache.len() <= 3);
    }

    #[test]
    fn byte_limit_evicts_until_it_holds_and_is_never_exceeded() {
        let mut cache = cache(CacheLimits::new(100, 10));
        for key in 0..5 {
            cache.insert(key, 3);
            assert!(cache.bytes() <= 10, "after inserting {key}");
        }
        // 3 entries of 3 bytes fit; 0 and 1 were evicted in order.
        assert_eq!(keys(&cache), vec![2, 3, 4]);
        assert_eq!(cache.bytes(), 9);
        // A bigger entry evicts several least-recently-used entries at once.
        assert_eq!(cache.insert(9, 8), Inserted::Retained { evicted: 3 });
        assert_eq!(keys(&cache), vec![9]);
        assert_eq!(cache.bytes(), 8);
        assert_eq!(cache.stats().evictions, 5);
    }

    #[test]
    fn an_oversized_entry_is_returned_uncached_and_evicts_nothing() {
        let mut cache = cache(CacheLimits::new(4, 10));
        cache.insert(1, 4);
        cache.insert(2, 4);
        assert_eq!(cache.insert(3, 11), Inserted::Oversized(11));
        assert_eq!(keys(&cache), vec![1, 2]);
        assert_eq!(cache.stats().oversized, 1);

        // A stale value of the same key is dropped, not served.
        assert_eq!(cache.insert(1, 12), Inserted::Oversized(12));
        assert!(!cache.contains(&1));
        // get_or_insert_with returns the computed value without retaining it.
        assert_eq!(cache.get_or_insert_with(7, || 50), 50);
        assert!(!cache.contains(&7));
        assert_eq!(cache.get_or_insert_with(2, || unreachable!()), 4);
    }

    #[test]
    fn replacing_a_key_accounts_bytes_once() {
        let mut cache = cache(CacheLimits::new(4, 100));
        cache.insert(1, 30);
        cache.insert(1, 20);
        assert_eq!((cache.len(), cache.bytes()), (1, 20));
        cache.clear();
        assert_eq!((cache.len(), cache.bytes()), (0, 0));
    }

    #[test]
    fn shrinking_the_limits_evicts_and_retain_filters() {
        let mut cache = cache(CacheLimits::new(10, 100));
        for key in 0..6 {
            cache.insert(key, 10);
        }
        cache.set_limits(CacheLimits::new(2, 100));
        assert_eq!(keys(&cache), vec![4, 5]);
        cache.retain(|key, _| *key != 4);
        assert_eq!(keys(&cache), vec![5]);
        assert_eq!(cache.bytes(), 10);
    }

    #[test]
    fn default_class_limits_match_the_spec() {
        assert_eq!(CacheLimits::SYNTAX_TOKENS, CacheLimits::new(96, 25_165_824));
        assert_eq!(
            CacheLimits::FORMULA_RASTER,
            CacheLimits::new(4_096, 8_388_608)
        );
        assert_eq!(
            CacheLimits::DIAGRAM_RASTER,
            CacheLimits::new(64, 67_108_864)
        );
    }

    #[test]
    fn dependency_keys_differ_in_any_dependency_and_only_themed_keys_see_the_theme() {
        let key = |source: &str, language: &str, size: u32| {
            DependencyKey::builder()
                .part(source)
                .part(language)
                .part(&size)
                .build()
        };
        let base = key("fn main() {}", "rust", 14);
        assert_eq!(base, key("fn main() {}", "rust", 14));
        assert_ne!(base, key("fn main() {}!", "rust", 14), "source");
        assert_ne!(base, key("fn main() {}", "c", 14), "language");
        assert_ne!(base, key("fn main() {}", "rust", 15), "size");

        let themed = |theme: u64| {
            DependencyKey::builder()
                .part("graph a-->b")
                .theme(ThemeId(theme))
                .build()
        };
        assert_eq!(themed(1), themed(1));
        assert_ne!(themed(1), themed(2), "theme");
        // Parts are not concatenated: ("ab", "c") differs from ("a", "bc").
        assert_ne!(
            DependencyKey::builder().part("ab").part("c").build(),
            DependencyKey::builder().part("a").part("bc").build()
        );
    }

    #[test]
    fn theme_neutral_entries_survive_a_theme_change_and_themed_ones_miss() {
        let mut syntax: BoundedCache<DependencyKey, usize> =
            BoundedCache::new(CacheLimits::SYNTAX_TOKENS, |bytes| *bytes);
        let mut diagrams: BoundedCache<DependencyKey, usize> =
            BoundedCache::new(CacheLimits::DIAGRAM_RASTER, |bytes| *bytes);

        let tokens = DependencyKey::builder()
            .part("let x = 1;")
            .part("rust")
            .build();
        let raster = |theme| {
            DependencyKey::builder()
                .part("graph a-->b")
                .theme(ThemeId(theme))
                .build()
        };
        syntax.insert(tokens, 100);
        diagrams.insert(raster(1), 1_000);

        // The palette changes from theme 1 to theme 2.
        assert!(
            syntax.get(&tokens).is_some(),
            "token spans are theme neutral"
        );
        assert!(
            diagrams.get(&raster(2)).is_none(),
            "themed raster of another theme misses"
        );
        assert!(
            diagrams.get(&raster(1)).is_some(),
            "and never replaced the old one"
        );
    }
}
