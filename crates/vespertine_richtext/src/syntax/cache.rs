//! Bounded LRU cache of highlighted documents, retained handles and the highlighter adapter
//! (TXT-007 R13-R17).

use super::document::{HighlightError, HighlightedDocument, Limits};
use super::highlighter::{HighlightRequest, first_line, highlight_language};
use super::language::{Language, detect};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Explicit grammar generation: bump whenever the grammar set or the scope-to-kind table
/// changes, so entries of an older generation can never be served (TXT-007 R14).
pub const GRAMMAR_GENERATION: u32 = 2;
/// Most documents the cache retains (TXT-007 R15).
pub const CACHE_MAX_DOCUMENTS: usize = 96;
/// Most estimated weight the cache retains: 24 MiB (TXT-007 R15).
pub const CACHE_MAX_BYTES: usize = 25_165_824;

/// The two bounds of a cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheLimits {
    pub max_documents: usize,
    pub max_bytes: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_documents: CACHE_MAX_DOCUMENTS,
            max_bytes: CACHE_MAX_BYTES,
        }
    }
}

/// Cache counters (TXT-007 R16) plus the number of grammar parses performed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Lookups served from the cache.
    pub hits: u64,
    /// Lookups that were not served (including every failed request).
    pub misses: u64,
    /// Number of times a document was built (parsed) for a miss.
    pub parses: u64,
    /// Resident documents.
    pub documents: usize,
    /// Estimated retained weight of the resident documents, in bytes.
    pub retained_bytes: usize,
}

/// Cache key (TXT-007 R14): language, content digest (128 bits of SHA-256), content length and
/// grammar generation. The theme is never part of the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    language: Language,
    digest: [u8; 16],
    len: usize,
    generation: u32,
}

impl CacheKey {
    fn new(language: Language, source: &str, generation: u32) -> Self {
        let full = Sha256::digest(source.as_bytes());
        let mut digest = [0u8; 16];
        digest.copy_from_slice(&full[..16]);
        Self {
            language,
            digest,
            len: source.len(),
            generation,
        }
    }
}

struct Entry {
    document: Arc<HighlightedDocument>,
    weight: usize,
    last_used: u64,
}

struct Inner {
    entries: HashMap<CacheKey, Entry>,
    tick: u64,
    retained_bytes: usize,
    generation: u32,
    hits: u64,
    misses: u64,
    parses: u64,
}

/// A bounded least-recently-used cache of highlighted documents, shared by any number of code
/// views and editable surfaces (TXT-007 R14-R16). Cached values do not retain the source.
pub struct HighlightCache {
    limits: CacheLimits,
    inner: Mutex<Inner>,
}

impl Default for HighlightCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HighlightCache {
    /// A cache with the specified bounds: 96 documents and 24 MiB.
    pub fn new() -> Self {
        Self::with_limits(CacheLimits::default())
    }

    /// A cache with explicit bounds.
    pub fn with_limits(limits: CacheLimits) -> Self {
        Self {
            limits,
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                tick: 0,
                retained_bytes: 0,
                generation: GRAMMAR_GENERATION,
                hits: 0,
                misses: 0,
                parses: 0,
            }),
        }
    }

    /// The process-wide cache used by views that are not given one.
    pub fn global() -> Arc<HighlightCache> {
        static GLOBAL: OnceLock<Arc<HighlightCache>> = OnceLock::new();
        GLOBAL
            .get_or_init(|| Arc::new(HighlightCache::new()))
            .clone()
    }

    /// The bounds of this cache.
    pub fn limits(&self) -> CacheLimits {
        self.limits
    }

    /// The current grammar generation (part of every key).
    pub fn generation(&self) -> u32 {
        self.inner.lock().generation
    }

    /// Changes the grammar generation: entries of any other generation can never be served.
    pub fn set_generation(&self, generation: u32) {
        self.inner.lock().generation = generation;
    }

    /// Hits, misses, parses, resident documents and retained bytes.
    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.lock();
        CacheStats {
            hits: inner.hits,
            misses: inner.misses,
            parses: inner.parses,
            documents: inner.entries.len(),
            retained_bytes: inner.retained_bytes,
        }
    }

    /// Drops every cached document (the counters are kept).
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.entries.clear();
        inner.retained_bytes = 0;
    }

    /// Cached highlight of a request: the shared document, or `None` (render plain) when the
    /// language is unknown or the request fails. Failures are never cached.
    pub fn highlight(&self, request: &HighlightRequest<'_>) -> Option<Arc<HighlightedDocument>> {
        match request.language() {
            Some(language) => self.highlight_as(language, request.source, request.limits),
            None => {
                self.inner.lock().misses += 1;
                None
            }
        }
    }

    /// Cached highlight of `source` as `language`.
    pub fn highlight_as(
        &self,
        language: Language,
        source: &str,
        limits: Limits,
    ) -> Option<Arc<HighlightedDocument>> {
        self.get_or_build(language, source, limits, || {
            highlight_language(language, source, limits)
        })
    }

    /// Looks `(language, source)` up and, on a miss, builds the document with `build` (a
    /// third-party tokenizer through [`HighlightedDocument::from_absolute_spans`], for example).
    /// A failing build yields `None` and is not cached; a source over `limits` is rejected
    /// before hashing. A built document that is too heavy for the cache is returned but not
    /// retained.
    pub fn get_or_build(
        &self,
        language: Language,
        source: &str,
        limits: Limits,
        build: impl FnOnce() -> Result<HighlightedDocument, HighlightError>,
    ) -> Option<Arc<HighlightedDocument>> {
        if source.len() > limits.max_source_bytes {
            self.inner.lock().misses += 1;
            return None;
        }
        let generation = self.generation();
        let key = CacheKey::new(language, source, generation);
        {
            let mut inner = self.inner.lock();
            inner.tick += 1;
            let tick = inner.tick;
            if let Some(entry) = inner.entries.get_mut(&key) {
                entry.last_used = tick;
                let document = entry.document.clone();
                inner.hits += 1;
                return Some(document);
            }
            inner.misses += 1;
            inner.parses += 1;
        }
        let document = Arc::new(build().ok()?);
        self.insert_key(key, document.clone());
        Some(document)
    }

    /// Inserts an externally built document for `(language, source)` under the current
    /// generation, replacing any previous entry for the key (TXT-007 R15).
    pub fn insert(&self, language: Language, source: &str, document: Arc<HighlightedDocument>) {
        let key = CacheKey::new(language, source, self.generation());
        self.insert_key(key, document);
    }

    fn insert_key(&self, key: CacheKey, document: Arc<HighlightedDocument>) {
        let weight = document.weight();
        let mut inner = self.inner.lock();
        if let Some(old) = inner.entries.remove(&key) {
            inner.retained_bytes -= old.weight;
        }
        if weight > self.limits.max_bytes {
            return;
        }
        inner.tick += 1;
        let last_used = inner.tick;
        inner.entries.insert(
            key,
            Entry {
                document,
                weight,
                last_used,
            },
        );
        inner.retained_bytes += weight;
        while inner.entries.len() > self.limits.max_documents
            || inner.retained_bytes > self.limits.max_bytes
        {
            let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(evicted) = inner.entries.remove(&oldest) {
                inner.retained_bytes -= evicted.weight;
            }
        }
    }
}

/// The language hint of a code host: a fence label (or host `language` property) and a `path`
/// property (TXT-007 R1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LanguageHint<'a> {
    pub fence_label: Option<&'a str>,
    pub path: Option<&'a str>,
}

impl LanguageHint<'_> {
    /// The language the hint and the first line of `text` resolve to.
    pub fn resolve(&self, text: &str) -> Option<Language> {
        detect(self.fence_label, self.path, Some(first_line(text)))
    }
}

/// The highlighter interface of editable surfaces (TXT-007 R13): given the current text and a
/// language hint, return the document for that revision, or `None` to render plain.
/// Code views and editable surfaces use the same implementation, so they share kinds.
pub trait HighlighterAdapter {
    fn document_for(&self, text: &str, hint: LanguageHint<'_>) -> Option<Arc<HighlightedDocument>>;
}

impl HighlighterAdapter for HighlightCache {
    fn document_for(&self, text: &str, hint: LanguageHint<'_>) -> Option<Arc<HighlightedDocument>> {
        let request = HighlightRequest {
            source: text,
            path: hint.path,
            fence_label: hint.fence_label,
            limits: Limits::default(),
        };
        self.highlight(&request)
    }
}

impl<T: HighlighterAdapter + ?Sized> HighlighterAdapter for Arc<T> {
    fn document_for(&self, text: &str, hint: LanguageHint<'_>) -> Option<Arc<HighlightedDocument>> {
        (**self).document_for(text, hint)
    }
}

/// The resolved document a widget keeps across frames (TXT-007 R17). While the source, the
/// language hint and the cache generation are unchanged, [`RetainedHighlight::resolve`]
/// performs no lookup and no hashing: it compares the retained source with the given one
/// (pointer first, then bytes) and returns the retained handle. Recoloring uses the retained
/// document and never touches the cache.
#[derive(Default)]
pub struct RetainedHighlight {
    source: Option<Arc<str>>,
    fence_label: Option<String>,
    path: Option<String>,
    generation: u32,
    document: Option<Arc<HighlightedDocument>>,
}

impl RetainedHighlight {
    /// An empty handle that resolves on first use.
    pub fn new() -> Self {
        Self::default()
    }

    /// The retained document, `None` while nothing is resolved or the source renders plain.
    pub fn document(&self) -> Option<&Arc<HighlightedDocument>> {
        self.document.as_ref()
    }

    /// Resolves the document for `source` and `hint` against `cache`, only when the source, the
    /// hint or the cache generation changed since the last call. A failed request is retained
    /// as "plain" until one of them changes.
    pub fn resolve(
        &mut self,
        cache: &HighlightCache,
        source: &Arc<str>,
        hint: LanguageHint<'_>,
    ) -> Option<&Arc<HighlightedDocument>> {
        let generation = cache.generation();
        let unchanged = self.source.as_ref().is_some_and(|retained| {
            (Arc::ptr_eq(retained, source) || **retained == **source)
                && self.generation == generation
                && self.fence_label.as_deref() == hint.fence_label
                && self.path.as_deref() == hint.path
        });
        if unchanged {
            return self.document.as_ref();
        }
        self.document = cache.document_for(source, hint);
        self.source = Some(source.clone());
        self.fence_label = hint.fence_label.map(str::to_owned);
        self.path = hint.path.map(str::to_owned);
        self.generation = generation;
        self.document.as_ref()
    }
}
