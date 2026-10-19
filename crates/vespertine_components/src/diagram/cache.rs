//! The bounded diagram cache with lazy request, forget, serialization, stale-theme and eviction
//! semantics (CMP-013 R4, R8, R10, R11).
//!
//! The cache is plain state without a clock or an executor: the host drives it from the UI thread
//! (`begin_frame`, `request`, `take_new_requests`, `next_job`, `finish`) and runs the one render at
//! a time wherever it likes. Every media the cache stops holding is returned to the caller for
//! release.

use super::media::{DiagramMedia, View};
use std::collections::{HashMap, HashSet};

/// Entry bound.
pub const MAX_ENTRIES: usize = 64;
/// Retained raster bound: 64 MiB.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// What a row sees for a source.
#[derive(Clone, Debug)]
pub enum Lookup {
    /// Not rendered yet: show the source fence.
    Pending,
    /// Rendered.
    Ready(DiagramMedia),
    /// Failed with a reason: show the failed view.
    Failed(String),
}

/// The settled state of an entry, for inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Requested, not rendered yet (possibly rendering now).
    Pending,
    /// Rendered.
    Ready,
    /// Failed.
    Failed,
}

/// One render to perform.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// The diagram source.
    pub source: String,
    /// The style identity the render is for; hand it back to [`DiagramCache::finish`].
    pub style_id: u64,
    /// The view to rasterize for.
    pub view: View,
    /// Retained bytes still available to the new raster.
    pub budget: usize,
}

/// The outcome of a finished render or a view change: rows to remeasure and media to release.
#[derive(Debug, Default)]
pub struct Update {
    /// Rows (row keys) that painted the affected sources and must be remeasured.
    pub rows: Vec<String>,
    /// Media the cache no longer holds.
    pub released: Vec<DiagramMedia>,
}

enum State {
    Pending { in_flight: bool },
    Ready(DiagramMedia),
    Failed(String),
}

struct Entry {
    state: State,
    /// The paint pass that last requested the source.
    last_pass: u64,
    /// Request sequence number; the pending source with the highest one renders next.
    seq: u64,
    /// Rows that painted the source.
    rows: Vec<String>,
}

/// The row key of a frame identifier `"<row key>-mermaid-<block index>"`.
pub fn row_key(frame_id: &str) -> &str {
    frame_id
        .rsplit_once("-mermaid-")
        .map_or(frame_id, |(row, _)| row)
}

/// The diagram cache.
pub struct DiagramCache {
    entries: HashMap<String, Entry>,
    max_entries: usize,
    max_bytes: usize,
    pass: u64,
    seq: u64,
    style_id: Option<u64>,
    view: View,
    new_requests: Vec<String>,
    /// Frame identifiers whose source view is shown.
    source_shown: HashSet<String>,
}

impl Default for DiagramCache {
    fn default() -> Self {
        DiagramCache::new()
    }
}

impl DiagramCache {
    /// A cache with the specified bounds: 64 entries and 64 MiB of retained rasters.
    pub fn new() -> DiagramCache {
        DiagramCache::with_limits(MAX_ENTRIES, MAX_BYTES)
    }

    /// A cache with explicit bounds (for tests).
    pub fn with_limits(max_entries: usize, max_bytes: usize) -> DiagramCache {
        DiagramCache {
            entries: HashMap::new(),
            max_entries,
            max_bytes,
            pass: 0,
            seq: 0,
            style_id: None,
            view: View::default(),
            new_requests: Vec::new(),
            source_shown: HashSet::new(),
        }
    }

    /// Starts a paint pass stamped with the style identity of the current theme. When the
    /// identity differs from the previous pass every cached diagram (pending, ready, failed) is
    /// discarded; the media to release are returned.
    pub fn begin_frame(&mut self, style_id: u64) -> Vec<DiagramMedia> {
        self.pass += 1;
        let mut released = Vec::new();
        if self.style_id.is_some_and(|previous| previous != style_id) {
            released = self.drain();
        }
        self.style_id = Some(style_id);
        released
    }

    /// The number of the current paint pass.
    pub fn pass(&self) -> u64 {
        self.pass
    }

    /// The view rasters are made for.
    pub fn view(&self) -> View {
        self.view
    }

    /// Changes the view (target column width and display scale). Every retained raster is redone
    /// for the new view within the retained budget remaining after the other entries (the
    /// budget minus the other entries' bytes); superseded rasters are returned for release and
    /// the rows that painted them must be remeasured.
    pub fn set_view(&mut self, view: View) -> Update {
        let mut update = Update::default();
        if view == self.view {
            return update;
        }
        self.view = view;
        let mut sources: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.state, State::Ready(_)))
            .map(|(source, _)| source.clone())
            .collect();
        sources.sort();
        for source in sources {
            let total = self.retained_bytes();
            let Some(entry) = self.entries.get_mut(&source) else {
                continue;
            };
            let State::Ready(old) = &entry.state else {
                continue;
            };
            let budget = self.max_bytes.saturating_sub(total - old.bytes());
            let redone = old.raster_for_view(view, budget);
            let old = match std::mem::replace(&mut entry.state, State::Pending { in_flight: false })
            {
                State::Ready(old) => old,
                _ => continue,
            };
            entry.state = match redone {
                Ok(media) => State::Ready(media),
                Err(reason) => State::Failed(reason),
            };
            update.released.push(old);
            for row in &entry.rows {
                if !update.rows.contains(row) {
                    update.rows.push(row.clone());
                }
            }
        }
        update
    }

    /// A row painting `source` asks for it. The request is recorded with the current paint pass
    /// and the row; a source seen for the first time becomes pending.
    pub fn request(&mut self, source: &str, frame_id: &str) -> Lookup {
        self.seq += 1;
        let (pass, seq) = (self.pass, self.seq);
        let row = row_key(frame_id);
        if !self.entries.contains_key(source) {
            self.entries.insert(
                source.to_string(),
                Entry {
                    state: State::Pending { in_flight: false },
                    last_pass: pass,
                    seq,
                    rows: Vec::new(),
                },
            );
            self.new_requests.push(source.to_string());
        }
        let entry = self
            .entries
            .get_mut(source)
            .expect("the entry was inserted above");
        entry.last_pass = pass;
        entry.seq = seq;
        if !entry.rows.iter().any(|existing| existing == row) {
            entry.rows.push(row.to_string());
        }
        match &entry.state {
            State::Pending { .. } => Lookup::Pending,
            State::Ready(media) => Lookup::Ready(media.clone()),
            State::Failed(reason) => Lookup::Failed(reason.clone()),
        }
    }

    /// The sources that became pending since the last call (a host starts its worker when this
    /// is not empty).
    pub fn take_new_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.new_requests)
    }

    /// The next render: pending entries that were not requested in the latest two paint passes
    /// are forgotten first (their rows scrolled away; they are requested again if the row
    /// returns), then the pending source most recently requested is chosen.
    pub fn next_job(&mut self) -> Option<Job> {
        let pass = self.pass;
        self.entries.retain(|_, entry| {
            !matches!(entry.state, State::Pending { .. }) || entry.last_pass + 1 >= pass
        });
        let source = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.state, State::Pending { in_flight: false }))
            .max_by_key(|(_, entry)| entry.seq)
            .map(|(source, _)| source.clone())?;
        if let Some(entry) = self.entries.get_mut(&source) {
            entry.state = State::Pending { in_flight: true };
        }
        Some(Job {
            source,
            style_id: self.style_id.unwrap_or_default(),
            view: self.view,
            budget: self.max_bytes.saturating_sub(self.retained_bytes()),
        })
    }

    /// Stores a render result. A result stamped with a style identity other than the current one
    /// is dropped silently (`None`: no entry, no remeasure), as is one for a source that was
    /// forgotten meanwhile. Otherwise the entry settles, settled entries are evicted least
    /// recently painted first until both bounds hold (never entries painted in the latest two
    /// passes and never pending ones), and the rows to remeasure and the evicted media come
    /// back.
    pub fn finish(
        &mut self,
        source: &str,
        style_id: u64,
        result: Result<DiagramMedia, String>,
    ) -> Option<Update> {
        if self.style_id != Some(style_id) {
            return None;
        }
        let view = self.view;
        let budget = self.max_bytes.saturating_sub(self.retained_bytes());
        let entry = self.entries.get_mut(source)?;
        if !matches!(entry.state, State::Pending { .. }) {
            return None;
        }
        entry.state = match result {
            // A view change while the render ran: redo the raster for the current view.
            Ok(media) if media.view() != view => match media.raster_for_view(view, budget) {
                Ok(media) => State::Ready(media),
                Err(reason) => State::Failed(reason),
            },
            Ok(media) => State::Ready(media),
            Err(reason) => State::Failed(reason),
        };
        let rows = entry.rows.clone();
        Some(Update {
            rows,
            released: self.evict(),
        })
    }

    fn evict(&mut self) -> Vec<DiagramMedia> {
        let mut released = Vec::new();
        let protected_from = self.pass.saturating_sub(1);
        while self.entries.len() > self.max_entries || self.retained_bytes() > self.max_bytes {
            let victim = self
                .entries
                .iter()
                .filter(|(_, entry)| {
                    !matches!(entry.state, State::Pending { .. })
                        && entry.last_pass < protected_from
                })
                .min_by_key(|(_, entry)| (entry.last_pass, entry.seq))
                .map(|(source, _)| source.clone());
            let Some(victim) = victim else {
                // Only entries painted in the latest two passes (or pending ones) remain: the
                // bounds are soft.
                break;
            };
            if let Some(Entry {
                state: State::Ready(media),
                ..
            }) = self.entries.remove(&victim)
            {
                released.push(media);
            }
        }
        released
    }

    /// Retained raster bytes, counted over ready entries.
    pub fn retained_bytes(&self) -> usize {
        self.entries
            .values()
            .map(|entry| match &entry.state {
                State::Ready(media) => media.bytes(),
                _ => 0,
            })
            .sum()
    }

    /// Discards every entry and returns the media to release (source toggles stay).
    pub fn drain(&mut self) -> Vec<DiagramMedia> {
        self.new_requests.clear();
        self.entries
            .drain()
            .filter_map(|(_, entry)| match entry.state {
                State::Ready(media) => Some(media),
                _ => None,
            })
            .collect()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no entry.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The state of `source`'s entry, if any.
    pub fn status(&self, source: &str) -> Option<Status> {
        self.entries.get(source).map(|entry| match entry.state {
            State::Pending { .. } => Status::Pending,
            State::Ready(_) => Status::Ready,
            State::Failed(_) => Status::Failed,
        })
    }

    /// Whether the source view is shown for `frame_id` (default: the rendered diagram).
    pub fn source_visible(&self, frame_id: &str) -> bool {
        self.source_shown.contains(frame_id)
    }

    /// Switches `frame_id` between the diagram and its source; returns whether the source is
    /// shown afterwards.
    pub fn toggle_source(&mut self, frame_id: &str) -> bool {
        if !self.source_shown.remove(frame_id) {
            self.source_shown.insert(frame_id.to_string());
            return true;
        }
        false
    }

    /// Prunes the source toggles of rows that no longer exist: `is_live` answers for a row key.
    pub fn retain_rows(&mut self, is_live: impl Fn(&str) -> bool) {
        self.source_shown
            .retain(|frame_id| is_live(row_key(frame_id)));
    }
}
