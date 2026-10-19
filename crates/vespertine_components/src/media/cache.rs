//! The media cache and renderer release (CMP-020 R13).
//!
//! [`MediaCache`] keys entries by source identity and [`ValidationPolicy`] (the policy is part of
//! the key, including in-flight loads and error entries), accounts every loaded entry with its
//! retained byte estimate, evicts the least recently used entry once a budget of 64 MiB is
//! exceeded and keeps generated previews and user-supplied attachments in separate budgets.
//! Evicted and replaced images go on a pending-free list; [`MediaCache::flush_pending`] and
//! [`release`] drop them from the renderer of every window in a deferred step that runs after
//! the frame that evicted them.

use super::decode::{DecodedMedia, MediaError};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use vespertine::{App, AppContext as _, Entity, Pixels, RenderImage, SharedString, Size, px, size};

/// Budget of one pool of the cache.
pub const CACHE_BUDGET_BYTES: usize = 64 * 1024 * 1024;
/// Delay before the first retry of a failed load.
pub const RETRY_FIRST: Duration = Duration::from_secs(2);
/// Longest delay between retries.
pub const RETRY_MAX: Duration = Duration::from_secs(15);
/// How long a collapsed preview keeps its pixels.
pub const COLLAPSED_GRACE: Duration = Duration::from_millis(600);
/// A preview not painted for this long counts as offscreen.
pub const STALE_AFTER: Duration = Duration::from_secs(2);
/// Largest number of previews shown per host item.
pub const MAX_PREVIEWS_PER_ITEM: usize = 4;

/// The delay before retry number `failures` (1-based) of a failed load: 2 s, 4 s, 8 s, then
/// 15 s for every later attempt.
pub fn retry_delay(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);
    RETRY_FIRST.saturating_mul(1u32 << exponent).min(RETRY_MAX)
}

/// The decoded-thumbnail bound of a list preview: twice the pixel size of its display frame
/// (a 560 x 220 frame gives 1120 x 440).
pub fn thumbnail_bound(frame: Size<Pixels>) -> Size<Pixels> {
    size(
        px(f32::from(frame.width) * 2.0),
        px(f32::from(frame.height) * 2.0),
    )
}

/// How a source is validated; part of the cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValidationPolicy {
    /// Workspace preview (8 MiB cap).
    WorkspacePreview,
    /// Ordinary attachment (24 MiB cap).
    Attachment,
    /// Generated preview with the given byte cap.
    Generated {
        /// The caller's byte cap.
        cap: usize,
    },
    /// Icon-sized preview.
    Icon,
}

/// Which budget an entry is accounted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaClass {
    /// Generated previews (workspace previews, generated images, icons).
    Preview,
    /// User-supplied attachments.
    Attachment,
}

impl ValidationPolicy {
    /// The pool this policy's entries live in.
    pub fn class(self) -> MediaClass {
        match self {
            ValidationPolicy::Attachment => MediaClass::Attachment,
            _ => MediaClass::Preview,
        }
    }

    /// The input byte cap of this policy.
    pub fn byte_cap(self) -> usize {
        match self {
            ValidationPolicy::WorkspacePreview => super::decode::WORKSPACE_PREVIEW_BYTE_CAP,
            ValidationPolicy::Attachment => super::decode::ATTACHMENT_BYTE_CAP,
            ValidationPolicy::Generated { cap } => cap,
            ValidationPolicy::Icon => super::decode::WORKSPACE_PREVIEW_BYTE_CAP,
        }
    }
}

/// Identity of a cache entry: the source and the validation policy.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaKey {
    /// Identity of the source (a path, URI or content hash chosen by the host).
    pub source: SharedString,
    /// How the source is validated.
    pub policy: ValidationPolicy,
}

impl MediaKey {
    /// A key for `source` under `policy`.
    pub fn new(source: impl Into<SharedString>, policy: ValidationPolicy) -> Self {
        Self {
            source: source.into(),
            policy,
        }
    }
}

/// What the cache knows about a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaStatus {
    /// Never requested (or removed).
    Missing,
    /// A load is in flight.
    Loading,
    /// A decoded entry is resident.
    Ready,
    /// The last load failed; a retry is allowed after `retry_in`.
    Failed {
        /// The error of the last attempt.
        error: MediaError,
        /// Time left until a retry may start; zero when it is due.
        retry_in: Duration,
    },
}

struct Loaded {
    media: DecodedMedia,
    /// Logical clock of the last use (LRU order).
    used: u64,
    last_painted: Option<Instant>,
    collapsed_at: Option<Instant>,
    protected: bool,
}

enum Slot {
    Loading,
    Loaded(Box<Loaded>),
    Failed {
        error: MediaError,
        retry_at: Instant,
    },
}

/// The media cache: residency, eviction and the pending-free list.
pub struct MediaCache {
    budgets: [usize; 2],
    slots: HashMap<MediaKey, Slot>,
    /// Failure counts survive between a failed load and its retry.
    failures: HashMap<MediaKey, u32>,
    clock: u64,
    pending_free: Vec<Arc<RenderImage>>,
}

impl Default for MediaCache {
    fn default() -> Self {
        Self::new()
    }
}

fn pool(class: MediaClass) -> usize {
    match class {
        MediaClass::Preview => 0,
        MediaClass::Attachment => 1,
    }
}

impl MediaCache {
    /// A cache with the 64 MiB budget for each of the two pools.
    pub fn new() -> Self {
        Self::with_budgets(CACHE_BUDGET_BYTES, CACHE_BUDGET_BYTES)
    }

    /// A cache with explicit budgets for generated previews and attachments.
    pub fn with_budgets(previews: usize, attachments: usize) -> Self {
        Self {
            budgets: [previews, attachments],
            slots: HashMap::new(),
            failures: HashMap::new(),
            clock: 0,
            pending_free: Vec::new(),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Bytes accounted in one pool.
    pub fn used_bytes(&self, class: MediaClass) -> usize {
        self.slots
            .iter()
            .filter(|(key, _)| key.policy.class() == class)
            .map(|(_, slot)| match slot {
                Slot::Loaded(loaded) => loaded.media.retained_bytes(),
                _ => 0,
            })
            .sum()
    }

    /// Bytes the pool of `class` can still take before it exceeds its budget.
    pub fn available_bytes(&self, class: MediaClass) -> usize {
        self.budgets[pool(class)].saturating_sub(self.used_bytes(class))
    }

    /// Number of entries (any state).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the cache has no entries.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The status of `key` at `now`.
    pub fn status(&self, key: &MediaKey, now: Instant) -> MediaStatus {
        match self.slots.get(key) {
            None => MediaStatus::Missing,
            Some(Slot::Loading) => MediaStatus::Loading,
            Some(Slot::Loaded(_)) => MediaStatus::Ready,
            Some(Slot::Failed {
                error, retry_at, ..
            }) => MediaStatus::Failed {
                error: error.clone(),
                retry_in: retry_at.saturating_duration_since(now),
            },
        }
    }

    /// The resident media of `key`, counting as a use for LRU order.
    pub fn get(&mut self, key: &MediaKey) -> Option<DecodedMedia> {
        let used = self.tick();
        match self.slots.get_mut(key) {
            Some(Slot::Loaded(loaded)) => {
                loaded.used = used;
                Some(loaded.media.clone())
            }
            _ => None,
        }
    }

    /// Looks at the resident media of `key` without counting a use.
    pub fn peek(&self, key: &MediaKey) -> Option<&DecodedMedia> {
        match self.slots.get(key) {
            Some(Slot::Loaded(loaded)) => Some(&loaded.media),
            _ => None,
        }
    }

    /// Marks `key` as being loaded. Returns `false` (starting nothing) when the entry is
    /// loading, resident, or failed with its retry not yet due.
    pub fn begin_load(&mut self, key: &MediaKey, now: Instant) -> bool {
        match self.slots.get(key) {
            Some(Slot::Loading | Slot::Loaded(_)) => false,
            Some(Slot::Failed { retry_at, .. }) if *retry_at > now => false,
            _ => {
                self.slots.insert(key.clone(), Slot::Loading);
                true
            }
        }
    }

    /// Stores the result of a load. A success is accounted and may evict the least recently
    /// used other entries of its pool; a failure becomes an error entry with the retry ladder
    /// delay. A previous image of the key is replaced and put on the pending-free list.
    pub fn complete(
        &mut self,
        key: &MediaKey,
        result: Result<DecodedMedia, MediaError>,
        now: Instant,
    ) {
        match result {
            Ok(media) => {
                self.failures.remove(key);
                let used = self.tick();
                let previous = self.slots.insert(
                    key.clone(),
                    Slot::Loaded(Box::new(Loaded {
                        media,
                        used,
                        last_painted: Some(now),
                        collapsed_at: None,
                        protected: false,
                    })),
                );
                if let Some(Slot::Loaded(old)) = previous {
                    self.pending_free.push(old.media.image.clone());
                }
                self.evict_to_budget(key.policy.class(), Some(key));
            }
            Err(error) => {
                let failures = self.failures.entry(key.clone()).or_insert(0);
                *failures += 1;
                let failures = *failures;
                let previous = self.slots.insert(
                    key.clone(),
                    Slot::Failed {
                        error,
                        retry_at: now + retry_delay(failures),
                    },
                );
                if let Some(Slot::Loaded(old)) = previous {
                    self.pending_free.push(old.media.image.clone());
                }
            }
        }
    }

    /// Evicts least recently used entries of `class` until the pool fits its budget. The entry
    /// `keep` (just inserted) and protected entries are never victims.
    fn evict_to_budget(&mut self, class: MediaClass, keep: Option<&MediaKey>) {
        while self.used_bytes(class) > self.budgets[pool(class)] {
            let victim = self
                .slots
                .iter()
                .filter(|(key, _)| key.policy.class() == class && Some(*key) != keep)
                .filter_map(|(key, slot)| match slot {
                    Slot::Loaded(loaded) if !loaded.protected => Some((key, loaded.used)),
                    _ => None,
                })
                .min_by_key(|(_, used)| *used)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else { break };
            self.remove(&victim);
        }
    }

    /// Protects (or releases the protection of) an entry the host is showing: protected
    /// entries are skipped by eviction.
    pub fn set_protected(&mut self, key: &MediaKey, protected: bool) {
        if let Some(Slot::Loaded(loaded)) = self.slots.get_mut(key) {
            loaded.protected = protected;
        }
    }

    /// Removes an entry in any state; a resident image goes on the pending-free list.
    pub fn remove(&mut self, key: &MediaKey) {
        self.failures.remove(key);
        if let Some(Slot::Loaded(old)) = self.slots.remove(key) {
            self.pending_free.push(old.media.image.clone());
        }
    }

    /// Records that the preview of `key` was painted at `now`.
    pub fn mark_painted(&mut self, key: &MediaKey, now: Instant) {
        if let Some(Slot::Loaded(loaded)) = self.slots.get_mut(key) {
            loaded.last_painted = Some(now);
            loaded.collapsed_at = None;
        }
    }

    /// Records that the user collapsed the preview of `key` at `now`; its pixels are kept for
    /// the 600 ms grace period so toggling does not re-decode.
    pub fn mark_collapsed(&mut self, key: &MediaKey, now: Instant) {
        if let Some(Slot::Loaded(loaded)) = self.slots.get_mut(key) {
            loaded.collapsed_at.get_or_insert(now);
        }
    }

    fn offscreen(loaded: &Loaded, now: Instant) -> bool {
        if let Some(collapsed) = loaded.collapsed_at {
            return now.saturating_duration_since(collapsed) >= COLLAPSED_GRACE;
        }
        match loaded.last_painted {
            Some(painted) => now.saturating_duration_since(painted) >= STALE_AFTER,
            None => true,
        }
    }

    /// Drops offscreen previews while the preview pool exceeds its budget: collapsed previews
    /// past their 600 ms grace and previews not painted for 2 s (a frame-count rule is not used
    /// because list overdraw paints rows beyond the viewport). Protected entries stay. Returns
    /// the number of entries dropped.
    pub fn evict_offscreen(&mut self, now: Instant) -> usize {
        let mut dropped = 0;
        while self.used_bytes(MediaClass::Preview) > self.budgets[pool(MediaClass::Preview)] {
            let victim = self
                .slots
                .iter()
                .filter(|(key, _)| key.policy.class() == MediaClass::Preview)
                .filter_map(|(key, slot)| match slot {
                    Slot::Loaded(loaded) if !loaded.protected && Self::offscreen(loaded, now) => {
                        Some((key, loaded.used))
                    }
                    _ => None,
                })
                .min_by_key(|(_, used)| *used)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else { break };
            self.remove(&victim);
            dropped += 1;
        }
        dropped
    }

    /// Images evicted or replaced and not yet released from the renderer.
    pub fn pending_free(&self) -> &[Arc<RenderImage>] {
        &self.pending_free
    }

    /// Takes the pending-free list.
    pub fn take_pending_free(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.pending_free)
    }

    /// Releases every pending image from the renderer of every window, deferred until after
    /// the frame (or update) that is running returns to the application.
    pub fn flush_pending(&mut self, cx: &mut App) {
        let images = self.take_pending_free();
        if images.is_empty() {
            return;
        }
        cx.defer(move |cx| {
            for image in images {
                cx.drop_image(image, None);
            }
        });
    }
}

/// Releases a media item from the renderer: drops its image from the atlas (textures) of every
/// window. The release is deferred until after the frame that triggered it returns to the
/// application, so the window being updated is visited too.
pub fn release(media: &DecodedMedia, cx: &mut App) {
    let image = media.image.clone();
    cx.defer(move |cx| cx.drop_image(image, None));
}

/// Starts loading `key` into `cache` when no load is in flight or resident and no retry delay
/// is pending: `decode` runs on the background executor (never on the input-handling path)
/// and its result is stored, evicted images are released, and the cache entity is notified.
/// Returns whether a load was started.
pub fn start_load(
    cache: &Entity<MediaCache>,
    key: MediaKey,
    cx: &mut App,
    decode: impl FnOnce() -> Result<DecodedMedia, MediaError> + Send + 'static,
) -> bool {
    let started = cache.update(cx, |cache, _| cache.begin_load(&key, Instant::now()));
    if !started {
        return false;
    }
    let task = cx.background_spawn(async move { decode() });
    let cache = cache.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            cache.update(cx, |cache, cx| {
                cache.complete(&key, result, Instant::now());
                cache.flush_pending(cx);
                cx.notify();
            });
        });
    })
    .detach();
    true
}
