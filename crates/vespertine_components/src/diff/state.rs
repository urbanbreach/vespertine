//! The model cache and the persistent state of a diff viewer (CMP-015 R17-R19, R22, R24-R28).

use super::highlight::{FileDocuments, FileHighlights};
use super::model::{DiffFile, parse_patch, truncate_file_lines};
use super::rows::{
    Annotation, DiffMode, Draft, Flat, FlattenOptions, RowMetrics, Side, find_anchor_row,
    flatten_rows, gutter_width, mean_row_height, row_anchor, sticky_file_header, sticky_push,
};
use super::word::hunk_intervals;
use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use unicode_width::UnicodeWidthChar;
use vespertine::{
    App, AppContext as _, Context, Entity, EntityId, FocusHandle, Font, ListAlignment, ListOffset,
    ListState, MotionClock, TextRun, Window, hsla, px,
};
use vespertine_richtext::syntax::HighlightCache;

/// Overdraw of the virtualized list beyond the viewport, in px (R17).
pub const OVERDRAW: f32 = 1024.;
/// Duration of the fold height tween (R26).
pub const FOLD_TWEEN_MS: f64 = 180.;
/// How long after the toggle the tween is armed; then the stand-in settles (R26).
pub const FOLD_ARMED_MS: f64 = 400.;

/// The parsed patch with its lazily computed per-file caches. Rebuilt only when the patch text
/// (or the documents or the per-file cap) change, never for a color-only theme change.
pub struct Model {
    /// The parsed files.
    pub files: Vec<DiffFile>,
    /// Gutter width `G` of each file (R11).
    pub gutters: Vec<f32>,
    /// The largest gutter of the diff (R22).
    pub max_gutter: f32,
    cache: Arc<HighlightCache>,
    documents: Vec<FileDocuments>,
    highlights: Vec<OnceCell<FileHighlights>>,
    intervals: Vec<OnceCell<Vec<Vec<Option<Range<usize>>>>>>,
    widths: RefCell<HashMap<(usize, u64), f32>>,
}

impl Model {
    pub(super) fn new(
        patch: &str,
        file_cap: Option<usize>,
        documents: Vec<FileDocuments>,
        cache: Arc<HighlightCache>,
    ) -> Self {
        let mut files = parse_patch(patch);
        if let Some(cap) = file_cap {
            for file in &mut files {
                truncate_file_lines(file, cap);
            }
        }
        let gutters: Vec<f32> = files.iter().map(|f| gutter_width(f.max_line)).collect();
        let max_gutter = gutters.iter().copied().fold(0., f32::max);
        Self {
            highlights: files.iter().map(|_| OnceCell::new()).collect(),
            intervals: files.iter().map(|_| OnceCell::new()).collect(),
            files,
            gutters,
            max_gutter,
            cache,
            documents,
            widths: RefCell::new(HashMap::new()),
        }
    }

    /// The syntax spans of a file (R14, R15), computed on first use.
    pub fn highlights(&self, file: usize) -> &FileHighlights {
        self.highlights[file].get_or_init(|| {
            let path = &self.files[file].path;
            let documents = self.documents.iter().find(|d| &d.path == path);
            FileHighlights::build(&self.files[file], &self.cache, documents)
        })
    }

    /// The word-change interval of each line of a hunk (R13), computed on first use.
    pub fn word_intervals(&self, file: usize, hunk: usize) -> &[Option<Range<usize>>] {
        &self.intervals[file].get_or_init(|| {
            self.files[file]
                .hunks
                .iter()
                .map(|hunk| hunk_intervals(&hunk.lines))
                .collect()
        })[hunk]
    }

    /// Whether the new side has a document the context lines can be located in (R16).
    pub fn has_new_document(&self, file: usize) -> bool {
        self.documents
            .iter()
            .find(|d| d.path == self.files[file].path)
            .is_none_or(|d| d.new.is_some())
    }
}

/// A fold tween in progress (R26).
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    /// Motion time of the toggle, in ms.
    pub started: f64,
    /// Whether the body is being revealed (`true`) or hidden.
    pub expanding: bool,
}

impl Tween {
    /// The current body height between the old and new analytic heights (ease-out cubic).
    pub fn height(&self, now: f64, body: f32) -> f32 {
        let t = (((now - self.started) / FOLD_TWEEN_MS) as f32).clamp(0., 1.);
        let eased = 1. - (1. - t).powi(3);
        if self.expanding {
            body * eased
        } else {
            body * (1. - eased)
        }
    }
}

/// The row the pointer is over, for the add affordance (R28).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hover {
    /// Row index in the flattened list.
    pub row: usize,
}

/// Everything a render needs besides the state entity: the resolved text metrics.
#[derive(Clone, Debug)]
pub struct Layout {
    pub(super) mode: DiffMode,
    pub(super) wrap: bool,
    pub(super) text_size: f32,
    pub(super) font: Font,
    pub(super) metrics: RowMetrics,
}

/// Visual columns of a line: tabs advance to the next multiple of 4, wide characters count by
/// their display width (R22).
pub fn visual_columns(text: &str) -> usize {
    let mut columns = 0;
    for c in text.chars() {
        if c == '\t' {
            columns = (columns / 4 + 1) * 4;
        } else {
            columns += c.width().unwrap_or(0);
        }
    }
    columns
}

/// The persistent state of one diff viewer: the parsed model, the virtual list, folds, horizontal
/// scroll offsets, hover, the draft and the scroll anchors. Create it with
/// [`DiffViewState::new`] and keep the entity next to the view that owns the viewer.
pub struct DiffViewState {
    pub(super) list: ListState,
    pub(super) model: Rc<Model>,
    pub(super) flat: Rc<Flat>,
    pub(super) layout: Option<Layout>,
    pub(super) owner: Option<EntityId>,
    patch_key: Option<u64>,
    rows_key: Option<u64>,
    structural_key: Option<u64>,
    last_collapsed: Option<HashSet<String>>,
    pub(super) internal_collapsed: HashSet<String>,
    pub(super) controlled: bool,
    pub(super) tweens: HashMap<usize, Tween>,
    pub(super) hscroll: HashMap<usize, f32>,
    pub(super) width: f32,
    pub(super) hover: Option<Hover>,
    pub(super) draft: Option<Draft>,
    draft_key: Option<u64>,
    pub(super) draft_focus: FocusHandle,
    pub(super) focus_draft: bool,
    mode_wrap: Option<(DiffMode, bool)>,
    pub(super) scroll_wired: bool,
}

/// The inputs of one sync (everything that decides the flattened rows).
pub(super) struct SyncInput<'a> {
    pub patch: &'a str,
    pub file_cap: Option<usize>,
    pub documents: &'a [FileDocuments],
    pub cache: &'a Arc<HighlightCache>,
    pub word_diff: bool,
    pub max_lines: Option<usize>,
    pub collapsed: Option<&'a [String]>,
    pub mode: DiffMode,
    pub wrap: bool,
    pub annotations: &'a [Annotation],
    pub draft_key: Option<u64>,
    pub layout: Layout,
    pub scroll: bool,
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

impl DiffViewState {
    /// A new viewer state.
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let model = Rc::new(Model::new("", None, Vec::new(), HighlightCache::global()));
            Self {
                list: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
                model,
                flat: Rc::new(Flat::default()),
                layout: None,
                owner: None,
                patch_key: None,
                rows_key: None,
                structural_key: None,
                last_collapsed: None,
                internal_collapsed: HashSet::new(),
                controlled: false,
                tweens: HashMap::new(),
                hscroll: HashMap::new(),
                width: 0.,
                hover: None,
                draft: None,
                draft_key: None,
                draft_focus: cx.focus_handle(),
                focus_draft: false,
                mode_wrap: None,
                scroll_wired: false,
            }
        })
    }

    /// The flattened rows and per-file row ranges.
    pub fn flat(&self) -> &Flat {
        &self.flat
    }

    /// The parsed files.
    pub fn files(&self) -> &[DiffFile] {
        &self.model.files
    }

    /// The parsed model (for highlights and intervals).
    pub fn model(&self) -> &Rc<Model> {
        &self.model
    }

    /// The virtual list state (scroll position, anchors).
    pub fn list_state(&self) -> &ListState {
        &self.list
    }

    /// The collapsed display paths currently in effect, sorted.
    pub fn collapsed_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.effective_collapsed().into_iter().collect();
        paths.sort();
        paths
    }

    pub(super) fn effective_collapsed(&self) -> HashSet<String> {
        self.last_collapsed
            .clone()
            .unwrap_or_else(|| self.internal_collapsed.clone())
    }

    /// The row the pointer is over (for the add affordance), if any.
    pub fn hovered_row(&self) -> Option<usize> {
        self.hover.map(|hover| hover.row)
    }

    /// The draft being edited, if any.
    pub fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }

    /// The current horizontal scroll offset of a file's code (R22).
    pub fn horizontal_offset(&self, file: usize) -> f32 {
        self.hscroll.get(&file).copied().unwrap_or(0.)
    }

    /// Files whose collapse is currently animating.
    pub fn folding_files(&self) -> Vec<usize> {
        let mut files: Vec<usize> = self.tweens.keys().copied().collect();
        files.sort_unstable();
        files
    }

    /// The focus handle of the draft editor (the viewer focuses it when a draft opens).
    pub fn draft_focus_handle(&self) -> &FocusHandle {
        &self.draft_focus
    }

    /// The sticky file header of the current scroll position (R25): the file and how far the
    /// next file's header pushes it up (`min(next_header_top - 36, 0)`, never overlapping).
    /// Resolved from the flattened per-file row ranges and the list's logical position; a
    /// briefly stale position yields none.
    pub fn sticky(&self) -> Option<(usize, f32)> {
        let layout = self.layout.as_ref()?;
        let top = self.list.logical_scroll_top();
        let file =
            sticky_file_header(&self.flat.files, top.item_ix, f32::from(top.offset_in_item))?;
        let position = self.flat.files.iter().position(|rows| rows.file == file)?;
        let next_top = self.flat.files.get(position + 1).and_then(|next| {
            self.list
                .bounds_for_item(next.range.start)
                .map(|bounds| f32::from(bounds.top() - self.list.viewport_bounds().top()))
        });
        Some((file, sticky_push(next_top, layout.metrics.file_header)))
    }
    /// Marks the state changed: re-renders the state's owner view.
    pub(super) fn changed(&self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(owner) = self.owner {
            App::notify(cx, owner);
        }
    }

    /// The text size, line height and font the viewer last laid out with.
    pub fn text_metrics(&self) -> Option<(f32, f32)> {
        self.layout.as_ref().map(|l| (l.text_size, l.metrics.line))
    }

    /// The content width of a file's code (R22): the widest shaped line plus padding. Unified:
    /// `widest + 12 + 24 + 2 x (largest gutter - this gutter)`; split: `widest + 6 + 24 +
    /// (largest gutter - this gutter)`. Identical for both halves of a split row.
    pub fn content_width(&self, file: usize, window: &mut Window) -> f32 {
        let Some(layout) = &self.layout else {
            return 0.;
        };
        let widest = widest_line(&self.model, file, &layout.font, layout.text_size, window);
        let slack = self.model.max_gutter - self.model.gutters[file];
        match layout.mode {
            DiffMode::Unified => widest + 12. + 24. + 2. * slack,
            DiffMode::Split => widest + 6. + 24. + slack,
        }
    }

    /// Width of the code viewport of a file at the current viewer width.
    fn viewport_width(&self, file: usize) -> f32 {
        let g = self.model.gutters[file];
        match self.layout.as_ref().map(|l| l.mode).unwrap_or_default() {
            DiffMode::Unified => self.width - (3. + 2. * g + 28.),
            DiffMode::Split => (self.width - 1.) / 2. - (3. + g + 18.),
        }
    }

    /// Largest horizontal offset of a file (content width minus the code viewport).
    pub fn max_horizontal_offset(&self, file: usize, window: &mut Window) -> f32 {
        if self.layout.as_ref().is_some_and(|l| l.wrap) {
            return 0.;
        }
        (self.content_width(file, window) - self.viewport_width(file)).max(0.)
    }

    /// Pans a file's code viewport by `delta` px (positive scrolls right), clamped to the
    /// content (R22). Gutters stay; other files keep their own offsets.
    pub fn pan(&mut self, file: usize, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let max = self.max_horizontal_offset(file, window);
        let current = self.horizontal_offset(file);
        let next = (current + delta).clamp(0., max);
        if (next - current).abs() > f32::EPSILON {
            self.hscroll.insert(file, next);
            self.changed(cx);
        }
    }

    /// Toggles a file's collapsed state in the viewer's own tracking (R26). In a controlled
    /// viewer the owner decides; this has no effect there.
    pub fn toggle_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.controlled {
            return;
        }
        if !self.internal_collapsed.remove(path) {
            self.internal_collapsed.insert(path.to_owned());
        }
        self.changed(cx);
    }

    /// Collapse-all (R26): collapses every file, or expands every file when all are already
    /// collapsed, as a steady-state write without tweens that keeps the scroll anchor on the
    /// nearest header. Returns the new set of collapsed paths; in a controlled viewer the owner
    /// applies it (nothing is written here).
    pub fn collapse_all(&mut self, cx: &mut Context<Self>) -> Vec<String> {
        let paths: Vec<String> = self.model.files.iter().map(|f| f.path.clone()).collect();
        let current = self.effective_collapsed();
        let all_collapsed = !paths.is_empty() && paths.iter().all(|p| current.contains(p));
        let next: Vec<String> = if all_collapsed { Vec::new() } else { paths };
        if !self.controlled {
            self.internal_collapsed = next.iter().cloned().collect();
            // A steady-state write: no tween for the files that changed.
            self.last_collapsed = Some(self.internal_collapsed.clone());
            self.changed(cx);
        }
        next
    }

    /// Opens a draft (R28) at an anchor and focuses its editor. Mode switches and a changed
    /// draft context key discard it.
    pub fn open_draft(&mut self, draft: Draft, cx: &mut Context<Self>) {
        self.draft = Some(draft);
        self.focus_draft = true;
        self.changed(cx);
    }

    /// Discards the draft.
    pub fn cancel_draft(&mut self, cx: &mut Context<Self>) {
        if self.draft.take().is_some() {
            self.changed(cx);
        }
    }

    pub(super) fn set_hover(&mut self, hover: Option<Hover>, cx: &mut Context<Self>) {
        if self.hover != hover {
            self.hover = hover;
            self.changed(cx);
        }
    }

    /// Brings the model, the flattened rows and the list sizing up to date with the inputs of a
    /// render, preserving the logical scroll position (R18, R24, R26).
    pub(super) fn sync(
        &mut self,
        input: &SyncInput<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = MotionClock::now_ms(cx);
        let docs_key = hash_of(
            input
                .documents
                .iter()
                .map(|d| {
                    (
                        &d.path,
                        d.old.as_deref(),
                        d.new.as_deref(),
                        d.stale,
                        d.binary,
                        d.truncated,
                    )
                })
                .collect::<Vec<_>>(),
        );
        let patch_key = hash_of((input.patch, input.file_cap, docs_key));
        let patch_changed = self.patch_key != Some(patch_key);
        if patch_changed {
            self.model = Rc::new(Model::new(
                input.patch,
                input.file_cap,
                input.documents.to_vec(),
                input.cache.clone(),
            ));
            self.patch_key = Some(patch_key);
            self.tweens.clear();
            self.hover = None;
            // Cached widths belong to the old model; offsets are clamped when used.
        }
        self.controlled = input.collapsed.is_some();
        let collapsed: HashSet<String> = match input.collapsed {
            Some(paths) => paths.iter().cloned().collect(),
            None => self.internal_collapsed.clone(),
        };

        // Mode and wrap switches reset horizontal scroll and clear hover (R22, R28).
        let mode_wrap = (input.mode, input.wrap);
        if self.mode_wrap.is_some_and(|previous| previous != mode_wrap) {
            self.hscroll.clear();
            self.hover = None;
            // Switching wrap (or mode) mid-tween settles the stand-in first (R26).
            self.tweens.clear();
        }
        self.mode_wrap = Some(mode_wrap);

        // Fold tweens start for files whose collapsed state changed (R26): unwrapped sections
        // only, never on first sight, never for a reduced-motion user.
        if let Some(previous) = self.last_collapsed.clone()
            && !patch_changed
            && !input.wrap
            && !cx.reduce_motion()
        {
            for (index, file) in self.model.files.iter().enumerate() {
                let was = previous.contains(&file.path);
                let is = collapsed.contains(&file.path);
                if was != is && !self.tweens.contains_key(&index) {
                    self.tweens.insert(
                        index,
                        Tween {
                            started: now,
                            expanding: !is,
                        },
                    );
                }
            }
        }
        self.tweens
            .retain(|_, tween| now - tween.started < FOLD_ARMED_MS && !input.wrap);
        self.last_collapsed = Some(collapsed.clone());

        // A changed draft context key discards the draft (R28).
        if self.draft_key != input.draft_key {
            if self.draft_key.is_some() || input.draft_key.is_some() {
                self.draft = None;
            }
            self.draft_key = input.draft_key;
        }

        let folding: HashSet<usize> = self.tweens.keys().copied().collect();
        let mut sorted: Vec<&String> = collapsed.iter().collect();
        sorted.sort();
        let metrics = input.layout.metrics;
        let structural_key = hash_of((patch_key, input.word_diff, input.max_lines, input.scroll));
        let rows_key = hash_of((
            structural_key,
            &sorted,
            input.mode,
            input.wrap,
            input.annotations,
            self.draft
                .as_ref()
                .map(|d| (&d.path, d.side == Side::Old, d.line, d.editing.as_ref())),
            {
                let mut f: Vec<_> = folding.iter().copied().collect();
                f.sort_unstable();
                f
            },
            [
                metrics.file_header.to_bits(),
                metrics.notice.to_bits(),
                metrics.hunk_header.to_bits(),
                metrics.line.to_bits(),
                metrics.bottom_pad.to_bits(),
                metrics.show_more.to_bits(),
            ],
        ));
        self.layout = Some(input.layout.clone());
        if self.rows_key != Some(rows_key) {
            self.rebuild(
                input,
                &collapsed,
                &folding,
                structural_key,
                rows_key,
                patch_changed,
            );
        }
        if !self.tweens.is_empty() {
            window.request_animation_frame();
        }
    }

    fn rebuild(
        &mut self,
        input: &SyncInput<'_>,
        collapsed: &HashSet<String>,
        folding: &HashSet<usize>,
        structural_key: u64,
        rows_key: u64,
        patch_changed: bool,
    ) {
        // Capture the logical position before the heights are reset (R18, R24). Anchors refer to
        // the previous rows of the same parsed files, so a new patch uses the row index instead.
        let top = self.list.logical_scroll_top();
        let old_flat = self.flat.clone();
        let old_anchor = (!patch_changed)
            .then(|| row_anchor(&self.model.files, &old_flat, top.item_ix))
            .flatten();
        let structural = patch_changed || self.structural_key != Some(structural_key);
        let options = FlattenOptions {
            metrics: input.layout.metrics,
            max_lines: input.max_lines,
            collapsed,
            mode: input.mode,
            annotations: input.annotations,
            draft: self.draft.as_ref(),
            folding,
        };
        let flat = flatten_rows(&self.model.files, &options);
        let count = flat.rows.len();
        let mean = mean_row_height(&flat, &input.layout.metrics, input.annotations);
        self.list.reset_with_uniform_height(count, px(mean));
        let target = if structural || old_anchor.is_none() {
            // New text, word washes or cap: keep the row index (clamped to the new count).
            (top.item_ix > 0 && count > 0).then(|| top.item_ix.min(count - 1))
        } else {
            old_anchor
                .and_then(|anchor| find_anchor_row(&self.model.files, &flat, anchor))
                .filter(|&row| row > 0 || top.item_ix > 0)
        };
        self.flat = Rc::new(flat);
        self.rows_key = Some(rows_key);
        self.structural_key = Some(structural_key);
        if let Some(row) = target {
            self.list.scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: top.offset_in_item,
            });
        }
    }
}

/// The widest line of a file, measured on shaped text (R22): at least the visual columns times
/// the column advance. Only the lines with the most visual columns are shaped.
pub fn widest_line(model: &Model, file: usize, font: &Font, size: f32, window: &mut Window) -> f32 {
    let key = (file, hash_of((font, size.to_bits())));
    if let Some(width) = model.widths.borrow().get(&key) {
        return *width;
    }
    let size_px = px(size);
    let text_system = window.text_system().clone();
    let advance = text_system
        .em_advance(text_system.resolve_font(font), size_px)
        .map_or(size * 0.6, f32::from);
    let mut lines: Vec<(usize, &str)> = model.files[file]
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .map(|line| (visual_columns(&line.text), line.text.as_str()))
        .collect();
    const CANDIDATES: usize = 64;
    if lines.len() > CANDIDATES {
        lines.select_nth_unstable_by(CANDIDATES, |a, b| b.0.cmp(&a.0));
        lines.truncate(CANDIDATES);
    }
    let mut widest = 0f32;
    for (columns, text) in lines {
        let mut shaped = 0.;
        if !text.is_empty() {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: hsla(0., 0., 0., 1.),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let layout = text_system.layout_line(text, size_px, &[run], None);
            shaped = f32::from(layout.width);
        }
        widest = widest.max(shaped).max(columns as f32 * advance);
    }
    model.widths.borrow_mut().insert(key, widest);
    widest
}
