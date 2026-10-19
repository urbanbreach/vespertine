use crate::{
    AbsoluteLength, App, Bounds, DefiniteLength, Edges, ElementId, GridTemplate, Length, Pixels,
    Point, Size, Style, Window, size,
    util::{
        ceil_to_device_pixel, round_half_toward_zero, round_stroke_to_device_pixel,
        round_to_device_pixel,
    },
};
use smallvec::SmallVec;
use std::{
    any::Any,
    fmt::Debug,
    hash::{Hash, Hasher},
    ops::Range,
};
use taffy::{
    TaffyTree,
    geometry::{Point as TaffyPoint, Rect as TaffyRect, Size as TaffySize},
    prelude::{max_content, min_content},
    style::AvailableSpace as TaffyAvailableSpace,
    tree::NodeId,
};
use vespertine_collections::FxHashMap;

const EXPECT_MESSAGE: &str = "we should avoid taffy layout errors by construction if possible";

/// The most constraint probes a measured node remembers for replay (PRF-011 R4).
pub(crate) const PROBE_LOG_CAPACITY: usize = 16;

// ---------------------------------------------------------------------------
// Measure contexts
// ---------------------------------------------------------------------------

/// What to do with the measurement installed at a persistent node when a new
/// measurement is requested for it (LAY-001 R4).
#[allow(
    dead_code,
    reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum MeasureDecision {
    /// The previous measurement stays installed and the node stays clean. The new
    /// measurement is dropped; `reconcile` may already have moved state out of the
    /// previous measurement or rewritten it in place.
    Keep,
    /// Run the new measurement under the probes the node was measured with since it
    /// was last dirtied. When every returned size is unchanged the new measurement is
    /// installed and the node and its ancestors stay clean; otherwise it is installed
    /// and they are dirtied.
    Replay,
    /// Install the new measurement and dirty the node.
    Replace,
}

/// A measuring function with an identity/replay contract. A measured leaf that
/// has no such contract is installed through [`Window::request_measured_layout`]
/// and is always replaced and dirtied.
pub(crate) trait LayoutMeasure: 'static {
    /// Measures the leaf under the given constraints.
    fn measure(
        &mut self,
        known_dimensions: Size<Option<Pixels>>,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels>;

    /// Decides how `self`, the new measurement, relates to `previous`, the
    /// measurement of the same type installed at the node being claimed.
    fn reconcile(&mut self, previous: &mut Self) -> MeasureDecision;
}

/// Object-safe view of a [`LayoutMeasure`].
trait AnyMeasure: 'static {
    fn measure(
        &mut self,
        known_dimensions: Size<Option<Pixels>>,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels>;

    /// `self` is the installed (previous) measurement.
    fn reconcile_previous(&mut self, new: &mut dyn AnyMeasure) -> MeasureDecision;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<M: LayoutMeasure> AnyMeasure for M {
    fn measure(
        &mut self,
        known_dimensions: Size<Option<Pixels>>,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        LayoutMeasure::measure(self, known_dimensions, available_space, window, cx)
    }

    fn reconcile_previous(&mut self, new: &mut dyn AnyMeasure) -> MeasureDecision {
        match new.as_any_mut().downcast_mut::<M>() {
            Some(new) => new.reconcile(self),
            None => MeasureDecision::Replace,
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A measuring closure: it has no equality or replay contract.
struct ClosureMeasure<F>(F);

impl<F> LayoutMeasure for ClosureMeasure<F>
where
    F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
        + 'static,
{
    fn measure(
        &mut self,
        known_dimensions: Size<Option<Pixels>>,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        (self.0)(known_dimensions, available_space, window, cx)
    }

    fn reconcile(&mut self, _previous: &mut Self) -> MeasureDecision {
        MeasureDecision::Replace
    }
}

/// One constraint probe a measured node answered: the arguments its measuring
/// function received and the device-pixel size it produced.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Probe {
    known_dimensions: Size<Option<Pixels>>,
    available_space: Size<AvailableSpace>,
    result: Size<f32>,
}

/// The probes a measured node answered since it was last dirtied (PRF-011 R4).
#[derive(Default)]
struct ProbeLog {
    entries: SmallVec<[Probe; PROBE_LOG_CAPACITY]>,
    overflowed: bool,
}

impl ProbeLog {
    fn record(&mut self, probe: Probe) {
        // A probe that is asked again moves to the end: replay measures the probes in the
        // order they were last asked, so the last measurement is the one layout used.
        if let Some(index) = self.entries.iter().position(|entry| *entry == probe) {
            self.entries.remove(index);
            self.entries.push(probe);
            return;
        }
        if self.entries.len() == PROBE_LOG_CAPACITY {
            self.overflowed = true;
        } else {
            self.entries.push(probe);
        }
    }
}

struct NodeContext {
    measure: Box<dyn AnyMeasure>,
    probes: ProbeLog,
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// The key space a persistent node is looked up in. Layout roots created during
/// prepaint use a space that normal layout requests can never reach (LAY-001 R1).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum KeyNamespace {
    /// Layout requested while elements request their layout.
    Normal = 0,
    /// Layout roots created during prepaint.
    PrepaintRoot = 1,
}

/// A path-derived candidate key for a persistent node.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct LayoutKey {
    namespace: KeyNamespace,
    hash: u64,
}

/// Combines a parent key with a child discriminator. For a fixed `a` the result is a
/// bijection of `b`, so siblings of one parent never share a key through this function.
#[inline]
fn mix(a: u64, b: u64) -> u64 {
    let mut x = a.wrapping_add(b.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ 0x243F_6A88_85A3_08D3;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

const ANONYMOUS_TAG: u64 = 0x2545_F491_4F6C_DD1D;
const ITEM_TAG: u64 = 0x1B87_3593_CC9E_2D51;
const PREPAINT_ROOT_TAG: u64 = 0x85EB_CA6B_C2B2_AE35;
const NORMAL_ROOT_KEY: u64 = 0x6A09_E667_F3BC_C908;

struct KeyFrame {
    key: u64,
    next_child: u32,
    /// For prepaint roots: the path hash whose root counter this frame advances.
    root_path: Option<u64>,
}

/// A captured position in the layout key path (see [`Window::layout_key_context`]).
#[allow(dead_code, reason = "consumed by the retained-frame work (PRF-001)")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayoutKeyContext {
    namespace: KeyNamespace,
    key: u64,
    next_child: u32,
    item_index: Option<u32>,
    /// The prepaint-root path hash when the captured frame is a prepaint root.
    root_path: Option<u64>,
}

/// The path of layout keys of the elements currently requesting layout.
pub(crate) struct LayoutKeyStack {
    frames: Vec<KeyFrame>,
    namespace: KeyNamespace,
    saved_namespaces: Vec<KeyNamespace>,
    item_index: Option<u32>,
    root_counters: FxHashMap<u64, u32>,
}

impl LayoutKeyStack {
    pub(crate) fn new() -> Self {
        let mut stack = Self {
            frames: Vec::new(),
            namespace: KeyNamespace::Normal,
            saved_namespaces: Vec::new(),
            item_index: None,
            root_counters: FxHashMap::default(),
        };
        stack.reset();
        stack
    }

    /// Starts a frame: a single normal-namespace root frame.
    pub(crate) fn reset(&mut self) {
        self.frames.clear();
        self.frames.push(KeyFrame {
            key: NORMAL_ROOT_KEY,
            next_child: 0,
            root_path: None,
        });
        self.namespace = KeyNamespace::Normal;
        self.saved_namespaces.clear();
        self.item_index = None;
        self.root_counters.clear();
    }

    /// Enters an element's request-layout scope. An element with an explicit
    /// identity is keyed by that identity and its ancestors' key; an element
    /// without one is keyed by its ancestors' key and its position among the
    /// anonymous siblings, or by its row index when a virtual list supplied one.
    pub(crate) fn enter_element(&mut self, id: Option<&ElementId>) {
        let item_index = self.item_index.take();
        let parent = self
            .frames
            .last_mut()
            .expect("the root frame is never popped");
        let key = if let Some(id) = id {
            crate::extend_path_hash(parent.key, id)
        } else if let Some(index) = item_index {
            mix(parent.key, u64::from(index) ^ ITEM_TAG)
        } else {
            let position = parent.next_child;
            parent.next_child += 1;
            mix(parent.key, u64::from(position) ^ ANONYMOUS_TAG)
        };
        self.frames.push(KeyFrame {
            key,
            next_child: 0,
            root_path: None,
        });
    }

    pub(crate) fn exit_element(&mut self) {
        self.frames.pop();
        debug_assert!(!self.frames.is_empty());
    }

    /// Enters a prepaint-time layout root. `path_hash` is the hash of the element
    /// id path the root is created under.
    pub(crate) fn begin_root(&mut self, path_hash: u64) {
        self.saved_namespaces.push(self.namespace);
        self.namespace = KeyNamespace::PrepaintRoot;
        let next_child = self.root_counters.get(&path_hash).copied().unwrap_or(0);
        self.frames.push(KeyFrame {
            key: mix(PREPAINT_ROOT_TAG, path_hash),
            next_child,
            root_path: Some(path_hash),
        });
    }

    pub(crate) fn end_root(&mut self) {
        let frame = self.frames.pop().expect("a root frame is open");
        let path = frame.root_path.expect("end_root without begin_root");
        self.root_counters.insert(path, frame.next_child);
        self.namespace = self.saved_namespaces.pop().expect("a namespace is saved");
    }

    /// The position in the key path a request made right now would be keyed at.
    pub(crate) fn context(&self) -> LayoutKeyContext {
        let frame = self.frames.last().expect("the root frame is never popped");
        LayoutKeyContext {
            namespace: self.namespace,
            key: frame.key,
            next_child: frame.next_child,
            item_index: self.item_index,
            root_path: frame.root_path,
        }
    }

    /// Enters a frame that keys the next elements exactly as they were keyed when
    /// `context` was captured.
    pub(crate) fn push_context(&mut self, context: LayoutKeyContext) {
        self.saved_namespaces.push(self.namespace);
        self.namespace = context.namespace;
        self.frames.push(KeyFrame {
            key: context.key,
            next_child: context.next_child,
            root_path: None,
        });
        self.item_index = context.item_index;
    }

    pub(crate) fn pop_context(&mut self) {
        self.frames.pop();
        debug_assert!(!self.frames.is_empty());
        self.namespace = self.saved_namespaces.pop().expect("a namespace is saved");
        self.item_index = None;
    }

    /// Makes the next element that requests layout use `index` as its identity if it
    /// has no explicit identity.
    pub(crate) fn set_item_index(&mut self, index: Option<u32>) {
        self.item_index = index;
    }

    pub(crate) fn claim_key(&self) -> LayoutKey {
        LayoutKey {
            namespace: self.namespace,
            hash: self
                .frames
                .last()
                .expect("the root frame is never popped")
                .key,
        }
    }
}

// ---------------------------------------------------------------------------
// Handles, ownership, counters
// ---------------------------------------------------------------------------

/// A unique identifier for a layout node, generated when requesting a layout from Taffy.
///
/// A cheap copyable handle (slot and generation); it is valid for the frame in
/// which the node was claimed, and for as long as the node is kept alive.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct LayoutId {
    slot: u32,
    generation: u32,
}

/// The persistent layout nodes claimed while a layout owner was innermost (see
/// [`Window::begin_layout_owner`]).
#[allow(
    dead_code,
    reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OwnedLayoutNodes {
    nodes: Vec<LayoutId>,
}

#[allow(
    dead_code,
    reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
)]
impl OwnedLayoutNodes {
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn extend(&mut self, other: &OwnedLayoutNodes) {
        self.nodes.extend_from_slice(&other.nodes);
    }
}

/// Counters describing the work the layout engine did (test support and
/// diagnostics). All counters except `live_nodes` accumulate until reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutStats {
    /// Nodes created in the layout engine.
    pub nodes_created: u64,
    /// Requests served by a persistent node claimed from an earlier frame.
    pub nodes_reused: u64,
    /// Nodes released because nothing claimed them (or because they were temporary).
    pub nodes_released: u64,
    /// Style writes to the layout engine.
    pub style_writes: u64,
    /// Child-list writes to the layout engine.
    pub children_writes: u64,
    /// Measure-context writes (installs and removals) to the layout engine.
    pub measure_writes: u64,
    /// Invocations of measuring functions (layout and replay).
    pub measure_calls: u64,
    /// `compute_layout` calls that ran the layout engine.
    pub computed_layouts: u64,
    /// `compute_layout` calls answered because the root was clean at the same size.
    pub skipped_layouts: u64,
    /// Replays that kept the node and its ancestors clean.
    pub replays_succeeded: u64,
    /// Replays that dirtied the node.
    pub replays_failed: u64,
    /// Nodes alive right now (a gauge, not reset).
    pub live_nodes: u64,
}

// ---------------------------------------------------------------------------
// Style fingerprint
// ---------------------------------------------------------------------------

/// A 64-bit multiply-fold hasher used to fingerprint the layout-relevant fields
/// of a [`Style`].
struct Fingerprint(u64);

impl Hasher for Fingerprint {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        // Xor then multiply by an odd constant and xorshift: every step is a bijection of
        // the state for a fixed word, so two inputs that differ in one word never collide.
        self.0 = (self.0 ^ value).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 ^= self.0 >> 32;
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

impl Fingerprint {
    #[inline]
    fn f32(&mut self, value: f32) {
        self.write_u32(value.to_bits());
    }

    #[inline]
    fn tag<T>(&mut self, value: &T) {
        std::mem::discriminant(value).hash(self);
    }

    fn pixels(&mut self, value: Pixels) {
        self.f32(value.0);
    }

    fn absolute(&mut self, value: &AbsoluteLength) {
        self.tag(value);
        match value {
            AbsoluteLength::Pixels(pixels) => self.pixels(*pixels),
            AbsoluteLength::Rems(rems) => self.f32(rems.0),
        }
    }

    fn definite(&mut self, value: &DefiniteLength) {
        self.tag(value);
        match value {
            DefiniteLength::Absolute(length) => self.absolute(length),
            DefiniteLength::Fraction(fraction) => self.f32(*fraction),
        }
    }

    fn length(&mut self, value: &Length) {
        self.tag(value);
        if let Length::Definite(length) = value {
            self.definite(length);
        }
    }

    fn edges<T: Clone + Debug + Default + PartialEq>(
        &mut self,
        edges: &Edges<T>,
        mut each: impl FnMut(&mut Self, &T),
    ) {
        each(self, &edges.top);
        each(self, &edges.right);
        each(self, &edges.bottom);
        each(self, &edges.left);
    }

    fn pair<T: Clone + Debug + Default + PartialEq>(
        &mut self,
        size: &Size<T>,
        mut each: impl FnMut(&mut Self, &T),
    ) {
        each(self, &size.width);
        each(self, &size.height);
    }

    fn grid_template(&mut self, template: &Option<GridTemplate>) {
        match template {
            None => self.write_u8(0),
            Some(template) => {
                self.write_u8(1);
                self.write_u32(u32::from(template.repeat));
                self.tag(&template.min_size);
            }
        }
    }

    fn grid_placement(&mut self, placement: &crate::GridPlacement) {
        self.tag(placement);
        match placement {
            crate::GridPlacement::Line(line) => self.write_u32(*line as u32),
            crate::GridPlacement::Span(span) => self.write_u32(u32::from(*span)),
            crate::GridPlacement::Auto => {}
        }
    }
}

/// Fingerprints every field of `style` the conversion to a layout-engine style
/// consumes, together with the inputs of that conversion.
fn style_fingerprint(style: &Style, rem_size: Pixels, scale_factor: f32) -> u64 {
    let mut h = Fingerprint(0x1F83_D9AB_FB41_BD6B);
    h.pixels(rem_size);
    h.f32(scale_factor);
    h.tag(&style.display);
    h.tag(&style.overflow.x);
    h.tag(&style.overflow.y);
    h.absolute(&style.scrollbar_width);
    h.tag(&style.position);
    h.edges(&style.inset, |h, v| h.length(v));
    h.pair(&style.size, |h, v| h.length(v));
    h.pair(&style.min_size, |h, v| h.length(v));
    h.pair(&style.max_size, |h, v| h.length(v));
    match style.aspect_ratio {
        None => h.write_u8(0),
        Some(ratio) => {
            h.write_u8(1);
            h.f32(ratio);
        }
    }
    h.edges(&style.margin, |h, v| h.length(v));
    h.edges(&style.padding, |h, v| h.definite(v));
    h.edges(&style.border_widths, |h, v| h.absolute(v));
    h.write_u8(style.align_items.map_or(0, |v| 1 + v as u8));
    h.write_u8(style.align_self.map_or(0, |v| 1 + v as u8));
    h.write_u8(style.align_content.map_or(0, |v| 1 + v as u8));
    h.write_u8(style.justify_content.map_or(0, |v| 1 + v as u8));
    h.pair(&style.gap, |h, v| h.definite(v));
    h.tag(&style.flex_direction);
    h.tag(&style.flex_wrap);
    h.length(&style.flex_basis);
    h.f32(style.flex_grow);
    h.f32(style.flex_shrink);
    h.grid_template(&style.grid_rows);
    h.grid_template(&style.grid_cols);
    match &style.grid_location {
        None => h.write_u8(0),
        Some(location) => {
            h.write_u8(1);
            h.grid_placement(&location.row.start);
            h.grid_placement(&location.row.end);
            h.grid_placement(&location.column.start);
            h.grid_placement(&location.column.end);
        }
    }
    h.finish()
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

const NO_SLOT: u32 = u32::MAX;

/// What `stretch_auto_size_to_fill` last wrote into a node's style.
struct Stretch {
    auto_width: bool,
    auto_height: bool,
    applied: TaffySize<f32>,
}

/// The recorded inputs of one persistent node, and its bookkeeping.
struct Slot {
    generation: u32,
    live: bool,
    node: NodeId,
    key: Option<LayoutKey>,
    /// The frame in which the node was last claimed by a request.
    claimed_frame: u64,
    /// The frame in which the node was last claimed or marked live.
    live_frame: u64,
    /// The last frame in which the node survives without being claimed.
    retain_until: u64,
    style_fingerprint: u64,
    /// In debug builds, the converted style a fingerprint match is validated against.
    #[cfg(debug_assertions)]
    debug_style: taffy::style::Style,
    stretch: Option<Stretch>,
    /// The ordered child list last written, valid only while `children_valid`.
    children: Vec<LayoutId>,
    children_valid: bool,
    measured: bool,
    /// The (device-pixel) available space the node was last laid out as a root under.
    computed_space: Option<Size<AvailableSpace>>,
}

/// Absolute bounds of a node, valid for one layout pass of one node generation.
#[derive(Clone, Copy)]
struct BoundsEntry {
    generation: u32,
    pass: u32,
    /// Unrounded absolute border-box top-left in device pixels.
    outer_origin: Point<f32>,
    bounds: Bounds<Pixels>,
}

const EMPTY_BOUNDS: BoundsEntry = BoundsEntry {
    generation: 0,
    pass: 0,
    outer_origin: Point { x: 0.0, y: 0.0 },
    bounds: Bounds {
        origin: Point {
            x: Pixels(0.0),
            y: Pixels(0.0),
        },
        size: Size {
            width: Pixels(0.0),
            height: Pixels(0.0),
        },
    },
};

enum Claim {
    /// A persistent node from an earlier frame with this key.
    Reuse(usize),
    /// No node has this key yet; create one and register it.
    Create(LayoutKey),
    /// The key was claimed already in this frame (or there is none): a temporary node.
    Temporary,
}

pub struct TaffyLayoutEngine {
    taffy: TaffyTree<NodeContext>,
    slots: Vec<Slot>,
    bounds: Vec<BoundsEntry>,
    free: Vec<u32>,
    /// Slot of each taffy node, indexed by the node's slot-map index.
    node_index: Vec<u32>,
    by_key: [FxHashMap<u64, u32>; 2],
    frame: u64,
    /// Incremented every time layout is computed; cached bounds are valid for one pass.
    pass: u32,
    owners: Vec<Vec<LayoutId>>,
    stats: LayoutStats,
    write_count: u64,
    #[cfg(test)]
    fingerprint_override: Option<u64>,
}

impl TaffyLayoutEngine {
    pub fn new() -> Self {
        let mut taffy = TaffyTree::new();
        taffy.disable_rounding();
        TaffyLayoutEngine {
            taffy,
            slots: Vec::new(),
            bounds: Vec::new(),
            free: Vec::new(),
            node_index: Vec::new(),
            by_key: [FxHashMap::default(), FxHashMap::default()],
            frame: 1,
            pass: 1,
            owners: Vec::new(),
            stats: LayoutStats::default(),
            write_count: 0,
            #[cfg(test)]
            fingerprint_override: None,
        }
    }

    // -- bookkeeping --------------------------------------------------------

    fn slot_index(&self, id: LayoutId) -> usize {
        let index = id.slot as usize;
        match self.slots.get(index) {
            Some(slot) if slot.live && slot.generation == id.generation => index,
            _ => panic!("layout id {id:?} is not a live layout node of the current frame"),
        }
    }

    fn node_of(&self, id: LayoutId) -> NodeId {
        self.slots[self.slot_index(id)].node
    }

    fn id_of(&self, index: usize) -> LayoutId {
        LayoutId {
            slot: index as u32,
            generation: self.slots[index].generation,
        }
    }

    fn slot_of_node(&self, node: NodeId) -> Option<usize> {
        let node_index = (u64::from(node) & 0xFFFF_FFFF) as usize;
        match self.node_index.get(node_index) {
            Some(&slot) if slot != NO_SLOT && self.slots[slot as usize].node == node => {
                Some(slot as usize)
            }
            _ => None,
        }
    }

    fn register(&mut self, node: NodeId, key: Option<LayoutKey>, fingerprint: u64) -> usize {
        let frame = self.frame;
        let slot = Slot {
            generation: 0,
            live: true,
            node,
            key,
            claimed_frame: frame,
            live_frame: frame,
            retain_until: 0,
            style_fingerprint: fingerprint,
            #[cfg(debug_assertions)]
            debug_style: taffy::style::Style::default(),
            stretch: None,
            children: Vec::new(),
            children_valid: true,
            measured: false,
            computed_space: None,
        };
        let index = match self.free.pop() {
            Some(index) => {
                let generation = self.slots[index as usize].generation;
                self.slots[index as usize] = Slot { generation, ..slot };
                index as usize
            }
            None => {
                self.slots.push(slot);
                self.bounds.push(EMPTY_BOUNDS);
                self.slots.len() - 1
            }
        };
        let node_index = (u64::from(node) & 0xFFFF_FFFF) as usize;
        if self.node_index.len() <= node_index {
            self.node_index.resize(node_index + 1, NO_SLOT);
        }
        self.node_index[node_index] = index as u32;
        if let Some(key) = key {
            self.by_key[key.namespace as usize].insert(key.hash, index as u32);
        }
        self.stats.nodes_created += 1;
        self.write_count += 1;
        index
    }

    fn claim(&mut self, key: LayoutKey) -> Claim {
        match self.by_key[key.namespace as usize].get(&key.hash) {
            Some(&index) => {
                if self.slots[index as usize].claimed_frame != self.frame {
                    Claim::Reuse(index as usize)
                } else {
                    Claim::Temporary
                }
            }
            None => Claim::Create(key),
        }
    }

    fn mark_claimed(&mut self, index: usize) {
        let slot = &mut self.slots[index];
        slot.claimed_frame = self.frame;
        slot.live_frame = self.frame;
        self.stats.nodes_reused += 1;
    }

    fn record_owner(&mut self, index: usize) {
        if self.slots[index].key.is_some()
            && let Some(owner) = self.owners.last_mut()
        {
            owner.push(LayoutId {
                slot: index as u32,
                generation: self.slots[index].generation,
            });
        }
    }

    fn release(&mut self, index: usize) {
        let node = self.slots[index].node;
        if let Some(key) = self.slots[index].key.take() {
            let by_key = &mut self.by_key[key.namespace as usize];
            if by_key.get(&key.hash) == Some(&(index as u32)) {
                by_key.remove(&key.hash);
            }
        }
        if let Some(parent) = self.taffy.parent(node)
            && let Some(parent_slot) = self.slot_of_node(parent)
        {
            self.slots[parent_slot].children_valid = false;
            self.taffy.mark_dirty(parent).expect(EXPECT_MESSAGE);
        }
        self.taffy.remove(node).expect(EXPECT_MESSAGE);
        let node_index = (u64::from(node) & 0xFFFF_FFFF) as usize;
        self.node_index[node_index] = NO_SLOT;
        let slot = &mut self.slots[index];
        slot.live = false;
        slot.generation = slot.generation.wrapping_add(1);
        slot.children = Vec::new();
        slot.stretch = None;
        slot.computed_space = None;
        self.free.push(index as u32);
        self.stats.nodes_released += 1;
    }

    // -- frames -------------------------------------------------------------

    /// Ends a frame: nodes that were not claimed or marked live in it are released,
    /// except those retained for later frames; temporary nodes are always released.
    pub(crate) fn end_frame(&mut self) {
        debug_assert!(self.owners.is_empty(), "a layout owner was left open");
        let frame = self.frame;
        for index in 0..self.slots.len() {
            let slot = &self.slots[index];
            if !slot.live {
                continue;
            }
            let keep =
                slot.key.is_some() && (slot.live_frame == frame || slot.retain_until >= frame);
            if !keep {
                self.release(index);
            }
        }
        self.frame += 1;
    }

    // -- ownership ----------------------------------------------------------

    #[allow(
        dead_code,
        reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
    )]
    pub(crate) fn begin_owner(&mut self) {
        self.owners.push(Vec::new());
    }

    #[allow(
        dead_code,
        reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
    )]
    pub(crate) fn end_owner(&mut self) -> OwnedLayoutNodes {
        OwnedLayoutNodes {
            nodes: self
                .owners
                .pop()
                .expect("end_layout_owner without begin_layout_owner"),
        }
    }

    /// Keeps the nodes alive for the current frame without any writes.
    #[allow(
        dead_code,
        reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
    )]
    pub(crate) fn mark_live(&mut self, nodes: &OwnedLayoutNodes) {
        let frame = self.frame;
        for id in &nodes.nodes {
            if let Some(slot) = self.slots.get_mut(id.slot as usize)
                && slot.live
                && slot.generation == id.generation
            {
                slot.live_frame = frame;
            }
        }
    }

    /// Keeps the nodes alive for `frames` more frames after the current one.
    #[allow(
        dead_code,
        reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
    )]
    pub(crate) fn retain(&mut self, nodes: &OwnedLayoutNodes, frames: u32) {
        let until = self.frame + u64::from(frames);
        for id in &nodes.nodes {
            if let Some(slot) = self.slots.get_mut(id.slot as usize)
                && slot.live
                && slot.generation == id.generation
            {
                slot.retain_until = slot.retain_until.max(until);
            }
        }
    }

    /// Whether `id` still names a live node.
    #[allow(
        dead_code,
        reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
    )]
    pub(crate) fn is_live(&self, id: LayoutId) -> bool {
        self.slots
            .get(id.slot as usize)
            .is_some_and(|slot| slot.live && slot.generation == id.generation)
    }

    /// The monotonic count of style, children and measure-context writes plus created nodes.
    #[allow(
        dead_code,
        reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
    )]
    pub(crate) fn write_count(&self) -> u64 {
        self.write_count
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn stats(&self) -> LayoutStats {
        LayoutStats {
            live_nodes: (self.slots.len() - self.free.len()) as u64,
            ..self.stats
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn reset_stats(&mut self) {
        self.stats = LayoutStats::default();
    }

    /// Whether the node's layout is out of date (its caches were cleared).
    #[allow(
        dead_code,
        reason = "consumed by the retained-frame and text measurement carry/replay work (PRF-001, PRF-005, PRF-011)"
    )]
    pub(crate) fn is_dirty(&self, id: LayoutId) -> bool {
        self.taffy.dirty(self.node_of(id)).expect(EXPECT_MESSAGE)
    }

    /// Whether the node currently has a measuring function.
    #[cfg(test)]
    pub(crate) fn has_measure(&self, id: LayoutId) -> bool {
        self.taffy.get_node_context(self.node_of(id)).is_some()
    }

    /// Forces every style fingerprint to `fingerprint`, simulating collisions.
    #[cfg(test)]
    pub(crate) fn override_style_fingerprint(&mut self, fingerprint: Option<u64>) {
        self.fingerprint_override = fingerprint;
    }

    // -- requests -----------------------------------------------------------

    fn fingerprint(&self, style: &Style, rem_size: Pixels, scale_factor: f32) -> u64 {
        #[cfg(test)]
        if let Some(fingerprint) = self.fingerprint_override {
            return fingerprint;
        }
        style_fingerprint(style, rem_size, scale_factor)
    }

    fn reset_probes(&mut self, node: NodeId) {
        if let Some(context) = self.taffy.get_node_context_mut(node) {
            context.probes = ProbeLog::default();
        }
    }

    /// Writes the style when its fingerprint differs from the recorded one; in debug
    /// builds a matching fingerprint is validated against full structural equality.
    fn sync_style(
        &mut self,
        index: usize,
        style: &Style,
        fingerprint: u64,
        rem_size: Pixels,
        scale_factor: f32,
    ) {
        if self.slots[index].style_fingerprint == fingerprint {
            #[cfg(debug_assertions)]
            {
                let converted = style.to_taffy(rem_size, scale_factor);
                assert!(
                    converted == self.slots[index].debug_style,
                    "style fingerprint collision: two different layout styles share fingerprint \
                     {fingerprint:#018x}\nrecorded: {:?}\nrequested: {converted:?}",
                    self.slots[index].debug_style,
                );
            }
            return;
        }
        let converted = style.to_taffy(rem_size, scale_factor);
        let node = self.slots[index].node;
        #[cfg(debug_assertions)]
        {
            self.slots[index].debug_style = converted.clone();
        }
        self.taffy.set_style(node, converted).expect(EXPECT_MESSAGE);
        let slot = &mut self.slots[index];
        slot.style_fingerprint = fingerprint;
        slot.stretch = None;
        self.reset_probes(node);
        self.stats.style_writes += 1;
        self.write_count += 1;
    }

    fn write_children(&mut self, index: usize, children: &[LayoutId]) {
        let node = self.slots[index].node;
        let nodes: SmallVec<[NodeId; 16]> =
            children.iter().map(|child| self.node_of(*child)).collect();
        for &child in &nodes {
            // A child adopted from another parent leaves that parent's recorded list stale.
            if let Some(previous) = self.taffy.parent(child)
                && previous != node
                && let Some(previous_slot) = self.slot_of_node(previous)
            {
                self.slots[previous_slot].children_valid = false;
            }
        }
        self.taffy.set_children(node, &nodes).expect(EXPECT_MESSAGE);
        let slot = &mut self.slots[index];
        slot.children.clear();
        slot.children.extend_from_slice(children);
        slot.children_valid = true;
        self.reset_probes(node);
        self.stats.children_writes += 1;
        self.write_count += 1;
    }

    fn sync_children(&mut self, index: usize, children: &[LayoutId]) {
        let slot = &self.slots[index];
        if slot.children_valid && slot.children.as_slice() == children {
            return;
        }
        self.write_children(index, children);
    }

    fn remove_measure(&mut self, index: usize) {
        if !self.slots[index].measured {
            return;
        }
        let node = self.slots[index].node;
        self.taffy
            .set_node_context(node, None)
            .expect(EXPECT_MESSAGE);
        self.slots[index].measured = false;
        self.stats.measure_writes += 1;
        self.write_count += 1;
    }

    fn new_node(
        &mut self,
        key: Option<LayoutKey>,
        style: &Style,
        fingerprint: u64,
        rem_size: Pixels,
        scale_factor: f32,
        context: Option<NodeContext>,
    ) -> usize {
        let converted = style.to_taffy(rem_size, scale_factor);
        #[cfg(debug_assertions)]
        let debug_style = converted.clone();
        let measured = context.is_some();
        let node = match context {
            Some(context) => self.taffy.new_leaf_with_context(converted, context),
            None => self.taffy.new_leaf(converted),
        }
        .expect(EXPECT_MESSAGE);
        let index = self.register(node, key, fingerprint);
        #[cfg(debug_assertions)]
        {
            self.slots[index].debug_style = debug_style;
        }
        self.slots[index].measured = measured;
        index
    }

    /// Requests the node for the current key: reuses the persistent node of an earlier
    /// frame (writing only inputs that differ), or creates one.
    pub(crate) fn request_layout(
        &mut self,
        key: LayoutKey,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        let fingerprint = self.fingerprint(style, rem_size, scale_factor);
        let index = match self.claim(key) {
            Claim::Reuse(index) => {
                self.mark_claimed(index);
                self.sync_style(index, style, fingerprint, rem_size, scale_factor);
                self.sync_children(index, children);
                self.remove_measure(index);
                index
            }
            claim => {
                let key = match claim {
                    Claim::Create(key) => Some(key),
                    _ => None,
                };
                let index = self.new_node(key, style, fingerprint, rem_size, scale_factor, None);
                if !children.is_empty() {
                    self.write_children(index, children);
                }
                index
            }
        };
        self.record_owner(index);
        self.id_of(index)
    }

    /// Requests a measured leaf. `env` is required for the replay contract; without it a
    /// `Replay` decision is treated as `Replace`.
    pub(crate) fn request_measured_layout<M: LayoutMeasure>(
        &mut self,
        key: LayoutKey,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        new: M,
        env: Option<(&mut Window, &mut App)>,
    ) -> LayoutId {
        let fingerprint = self.fingerprint(style, rem_size, scale_factor);
        let index = match self.claim(key) {
            Claim::Reuse(index) => {
                self.mark_claimed(index);
                self.sync_style(index, style, fingerprint, rem_size, scale_factor);
                self.sync_children(index, &[]);
                self.reconcile_measure(index, new, scale_factor, env);
                index
            }
            claim => {
                let key = match claim {
                    Claim::Create(key) => Some(key),
                    _ => None,
                };
                let context = NodeContext {
                    measure: Box::new(new),
                    probes: ProbeLog::default(),
                };
                self.new_node(
                    key,
                    style,
                    fingerprint,
                    rem_size,
                    scale_factor,
                    Some(context),
                )
            }
        };
        self.record_owner(index);
        self.id_of(index)
    }

    /// Requests a measured leaf whose measuring function is a closure: it has no
    /// equality or replay contract, so it always replaces the installed one and
    /// dirties the node.
    pub(crate) fn request_measured_closure<F>(
        &mut self,
        key: LayoutKey,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: F,
    ) -> LayoutId
    where
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    {
        self.request_measured_layout(
            key,
            style,
            rem_size,
            scale_factor,
            ClosureMeasure(measure),
            None,
        )
    }

    fn reconcile_measure<M: LayoutMeasure>(
        &mut self,
        index: usize,
        mut new: M,
        scale_factor: f32,
        env: Option<(&mut Window, &mut App)>,
    ) {
        let node = self.slots[index].node;
        let install = |this: &mut Self, new: M| {
            let context = NodeContext {
                measure: Box::new(new),
                probes: ProbeLog::default(),
            };
            this.taffy
                .set_node_context(node, Some(context))
                .expect(EXPECT_MESSAGE);
            this.slots[index].measured = true;
            this.stats.measure_writes += 1;
            this.write_count += 1;
        };
        if !self.slots[index].measured {
            install(self, new);
            return;
        }
        let decision = {
            let context = self
                .taffy
                .get_node_context_mut(node)
                .expect("a measured slot has a measure context");
            context.measure.reconcile_previous(&mut new)
        };
        match decision {
            MeasureDecision::Keep => {}
            MeasureDecision::Replace => install(self, new),
            MeasureDecision::Replay => {
                let Some((window, cx)) = env else {
                    install(self, new);
                    return;
                };
                let (probes, overflowed) = {
                    let log = &self
                        .taffy
                        .get_node_context_mut(node)
                        .expect("a measured slot has a measure context")
                        .probes;
                    (log.entries.clone(), log.overflowed)
                };
                let mut unchanged = !overflowed;
                if unchanged {
                    for probe in &probes {
                        let measured =
                            new.measure(probe.known_dimensions, probe.available_space, window, cx);
                        self.stats.measure_calls += 1;
                        if snap_measured_size_to_device_pixels(measured, scale_factor)
                            != probe.result
                        {
                            unchanged = false;
                            break;
                        }
                    }
                }
                if unchanged {
                    // The measurement is replaced in place: nothing is dirtied and the
                    // probe log stays.
                    self.taffy
                        .get_node_context_mut(node)
                        .expect("a measured slot has a measure context")
                        .measure = Box::new(new);
                    self.stats.replays_succeeded += 1;
                    self.stats.measure_writes += 1;
                    // Nothing is dirtied, so this is not a write that can change layout.
                } else {
                    self.stats.replays_failed += 1;
                    install(self, new);
                }
            }
        }
    }

    /// Treats any `auto` dimension of the given node's style as filling `size`.
    ///
    /// This is applied to window roots before layout so they behave like the
    /// root element on the web, which stretches to fill the initial containing
    /// block (the viewport) unless given an explicit size. Explicitly styled
    /// dimensions are preserved.
    pub fn stretch_auto_size_to_fill(
        &mut self,
        id: LayoutId,
        size: Size<Pixels>,
        scale_factor: f32,
    ) {
        let index = self.slot_index(id);
        let node = self.slots[index].node;
        let target = TaffySize {
            width: round_to_device_pixel(size.width.0, scale_factor),
            height: round_to_device_pixel(size.height.0, scale_factor),
        };
        let (auto_width, auto_height) = match &self.slots[index].stretch {
            Some(stretch) => {
                if stretch.applied == target {
                    return;
                }
                (stretch.auto_width, stretch.auto_height)
            }
            None => {
                let style = self.taffy.style(node).expect(EXPECT_MESSAGE);
                let auto = (style.size.width.is_auto(), style.size.height.is_auto());
                if !auto.0 && !auto.1 {
                    return;
                }
                auto
            }
        };
        let mut style = self.taffy.style(node).expect(EXPECT_MESSAGE).clone();
        if auto_width {
            style.size.width = taffy::style::Dimension::length(target.width);
        }
        if auto_height {
            style.size.height = taffy::style::Dimension::length(target.height);
        }
        self.taffy.set_style(node, style).expect(EXPECT_MESSAGE);
        self.slots[index].stretch = Some(Stretch {
            auto_width,
            auto_height,
            applied: target,
        });
        self.stats.style_writes += 1;
        self.write_count += 1;
    }

    #[cfg_attr(feature = "stacker", stacksafe::stacksafe)]
    pub fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let index = self.slot_index(id);
        let node = self.slots[index].node;
        let scale_factor = window.scale_factor();

        let transform = |v: AvailableSpace| match v {
            AvailableSpace::Definite(pixels) => {
                AvailableSpace::Definite(Pixels(pixels.0 * scale_factor))
            }
            AvailableSpace::MinContent => AvailableSpace::MinContent,
            AvailableSpace::MaxContent => AvailableSpace::MaxContent,
        };
        let available_space = size(
            transform(available_space.width),
            transform(available_space.height),
        );

        // A clean root laid out under the same constraints keeps its layout.
        if !self.taffy.dirty(node).expect(EXPECT_MESSAGE)
            && self.slots[index].computed_space == Some(available_space)
        {
            self.stats.skipped_layouts += 1;
            return;
        }

        let mut measure_calls = 0u64;
        self.taffy
            .compute_layout_with_measure(
                node,
                available_space.into(),
                |known_dimensions, available_space, _id, node_context, _style| {
                    measure_node(
                        node_context,
                        known_dimensions,
                        available_space,
                        scale_factor,
                        window,
                        cx,
                        &mut measure_calls,
                    )
                },
            )
            .expect(EXPECT_MESSAGE);
        self.stats.measure_calls += measure_calls;
        self.stats.computed_layouts += 1;
        self.slots[index].computed_space = Some(available_space);
        self.pass = self.pass.wrapping_add(1).max(1);
    }

    /// Recomputes the layout of the subtree rooted at `id` with the node's size fixed to
    /// `size`, keeping the node's own position within its parent. Cached absolute bounds
    /// are recomputed on demand (from layouts that did not change for the node and its
    /// ancestors).
    #[cfg_attr(feature = "stacker", stacksafe::stacksafe)]
    pub fn relayout_subtree_in_place(
        &mut self,
        id: LayoutId,
        size: Size<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let index = self.slot_index(id);
        let node = self.slots[index].node;
        let scale_factor = window.scale_factor();
        let device_size = TaffySize {
            width: size.width.0 * scale_factor,
            height: size.height.0 * scale_factor,
        };
        let mut measure_calls = 0u64;
        self.taffy
            .compute_subtree_layout_with_measure(
                node,
                device_size,
                |known_dimensions, available_space, _id, node_context, _style| {
                    measure_node(
                        node_context,
                        known_dimensions,
                        available_space,
                        scale_factor,
                        window,
                        cx,
                        &mut measure_calls,
                    )
                },
            )
            .expect(EXPECT_MESSAGE);
        self.stats.measure_calls += measure_calls;
        self.stats.computed_layouts += 1;
        self.slots[index].computed_space = None;
        self.pass = self.pass.wrapping_add(1).max(1);
    }

    // Pixel snapping
    //
    // Painting primitives at non-integer pixel coordinates produces blurry
    // output. Pixel snapping converts layout coordinates into integer
    // device-pixel coordinates so painted edges land exactly on physical
    // pixel boundaries.
    //
    // Non-integer coordinates can arise for several reasons, including:
    //   - flex distribution, percentages, centering, and text measurement
    //     can produce fractional element sizes and positions;
    //   - at fractional scale factors (for example 125% or 150%), integer
    //     logical-pixel values can map to non-integer device-pixel values.
    //
    // We pixel-snap by rounding in device-pixel space, after multiplying
    // by `scale_factor`, so that snapping targets physical pixels. Bounds
    // are divided by `scale_factor` before being returned to VESPERTINE.
    //
    // Midpoints are rounded toward zero. This is a stylistic choice: a
    // 1-logical-pixel line at 150% scale should render as 1 dp rather than
    // 2 dp.
    //
    // Pixel snapping is done in two phases:
    //
    //  1. Pre-layout metric snapping. Before Taffy computes layout, all
    //     authored absolute lengths are rounded in `to_taffy`. This
    //     includes borders, padding, gaps, and explicit sizes.
    //     Custom-measured leaf nodes have their measured sizes rounded up
    //     to integer device-pixel lengths.
    //
    //  2. Post-layout edge snapping. After Taffy resolves the tree, layout
    //     relationships such as flex shares, grid tracks, percentages, and
    //     centering can produce new fractional edge positions. Boxes now
    //     have edges in absolute coordinates, and snapping must decide
    //     where those edges land on the device-pixel grid.
    //
    // Ideally, post-layout snapping would satisfy:
    //
    //  - Edge closure. Two raw layout edges at the same absolute position
    //    should snap to the same pixel column.
    //  - Translation stability. A component's internal geometry should not
    //    change when it moves to a new absolute position.
    //
    // These goals are in tension because rounding is not associative.
    // The simple local schemes make different tradeoffs:
    //
    //  - Absolute edge rounding gives each window coordinate one answer,
    //    so coincident edges always close globally. But a span's snapped
    //    length is `round(far) - round(near)`, which may change by 1 dp
    //    as its absolute origin moves.
    //
    //  - Parent-relative edge rounding rounds each child inside its
    //    parent's coordinate space. This guarantees translation stability,
    //    but a shared edge reached through different parents can
    //    accumulate different rounding, causing non-closure between
    //    cousins.
    //
    //  - Length rounding rounds each width, height, and thickness
    //    independently and then places boxes from those rounded lengths.
    //    Sizes stay stable under translation, but neighboring boxes derive
    //    their shared boundary from different sources, so closure is not
    //    guaranteed.
    //
    // We apply absolute edge rounding for each element's outer box in
    // post-layout rounding to preserve closure. Border and padding widths
    // are not touched by post-layout rounding; they keep their pre-layout
    // rounded value so that they remain stable under translation.
    //
    // This gives both closure and translation stability in the case that
    // all local metrics are integer device-pixel lengths. Pre-layout
    // rounding covers that in most cases. The exception is metrics
    // resolved by layout relationships, such as percentages. Outer box
    // edges will still close globally, and painted border widths are still
    // snapped independently, but the raw content-box origin can carry a
    // 1dp residual into descendants.
    //
    // Absolute bounds and accumulated origins are cached per node slot,
    // validated by the node's generation and the layout pass that produced
    // them, so a recycled slot never returns a previous node's bounds and
    // nothing is cleared when layout is recomputed.

    pub fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        let index = self.slot_index(id);
        let cached = self.bounds[index];
        if cached.generation == id.generation && cached.pass == self.pass {
            return cached.bounds;
        }

        let node = self.slots[index].node;
        let layout = self.taffy.layout(node).expect(EXPECT_MESSAGE);
        let layout_location = layout.location;
        let layout_size = layout.size;
        let parent = self.taffy.parent(node);

        let absolute_outer_origin = match parent {
            Some(parent_node) => {
                let parent_slot = self
                    .slot_of_node(parent_node)
                    .expect("a layout node's parent is a live layout node");
                let parent_id = self.id_of(parent_slot);
                self.layout_bounds(parent_id, scale_factor);
                self.bounds[parent_slot].outer_origin + Point::from(layout_location)
            }
            None => Point::from(layout_location),
        };

        let absolute_far = absolute_outer_origin + Point::from(Size::from(layout_size));
        let snapped_bounds = Bounds::from_corners(
            absolute_outer_origin.map(round_half_toward_zero),
            absolute_far.map(round_half_toward_zero),
        );

        let bounds = (snapped_bounds / scale_factor).map(Pixels);
        self.bounds[index] = BoundsEntry {
            generation: id.generation,
            pass: self.pass,
            outer_origin: absolute_outer_origin,
            bounds,
        };
        bounds
    }

    /// The size Taffy solved for a node in logical pixels, before post-layout
    /// edge snapping.
    #[cfg(test)]
    pub fn unsnapped_size(&self, id: LayoutId, scale_factor: f32) -> Size<Pixels> {
        let node = self.slots[self.slot_index(id)].node;
        let solved = self.taffy.layout(node).expect(EXPECT_MESSAGE).size;
        size(
            Pixels(solved.width / scale_factor),
            Pixels(solved.height / scale_factor),
        )
    }
}

/// Answers a layout-engine measure request for one node: runs its measuring function in
/// logical pixels, snaps the answer up to device pixels and logs the probe for replay.
fn measure_node(
    node_context: Option<&mut NodeContext>,
    known_dimensions: TaffySize<Option<f32>>,
    available_space: TaffySize<TaffyAvailableSpace>,
    scale_factor: f32,
    window: &mut Window,
    cx: &mut App,
    measure_calls: &mut u64,
) -> TaffySize<f32> {
    let Some(node_context) = node_context else {
        return TaffySize::default();
    };

    let known_dimensions = Size {
        width: known_dimensions.width.map(|e| Pixels(e / scale_factor)),
        height: known_dimensions.height.map(|e| Pixels(e / scale_factor)),
    };

    let available_space: Size<AvailableSpace> = available_space.into();
    let untransform = |ev: AvailableSpace| match ev {
        AvailableSpace::Definite(pixels) => {
            AvailableSpace::Definite(Pixels(pixels.0 / scale_factor))
        }
        AvailableSpace::MinContent => AvailableSpace::MinContent,
        AvailableSpace::MaxContent => AvailableSpace::MaxContent,
    };
    let available_space = size(
        untransform(available_space.width),
        untransform(available_space.height),
    );

    let measured_size: Size<Pixels> =
        node_context
            .measure
            .measure(known_dimensions, available_space, window, cx);
    *measure_calls += 1;
    let result = snap_measured_size_to_device_pixels(measured_size, scale_factor);
    node_context.probes.record(Probe {
        known_dimensions,
        available_space,
        result,
    });
    result.into()
}

fn snap_measured_size_to_device_pixels(size: Size<Pixels>, scale_factor: f32) -> Size<f32> {
    size.map(|d| ceil_to_device_pixel(d.0.max(0.0), scale_factor))
}

fn border_widths_to_taffy(
    widths: &Edges<AbsoluteLength>,
    rem_size: Pixels,
    scale_factor: f32,
) -> TaffyRect<taffy::style::LengthPercentage> {
    let snap = |w: &AbsoluteLength| {
        taffy::style::LengthPercentage::length(round_stroke_to_device_pixel(
            w.to_pixels(rem_size).0,
            scale_factor,
        ))
    };
    TaffyRect {
        top: snap(&widths.top),
        right: snap(&widths.right),
        bottom: snap(&widths.bottom),
        left: snap(&widths.left),
    }
}

trait ToTaffy<Output> {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Output;
}

impl ToTaffy<taffy::style::Style> for Style {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Style {
        use taffy::style_helpers::{fr, length, minmax, repeat};

        fn to_grid_line(
            placement: &Range<crate::GridPlacement>,
        ) -> taffy::Line<taffy::GridPlacement> {
            taffy::Line {
                start: placement.start.into(),
                end: placement.end.into(),
            }
        }

        fn to_grid_repeat<T: taffy::style::CheapCloneStr>(
            unit: &Option<GridTemplate>,
        ) -> Vec<taffy::GridTemplateComponent<T>> {
            unit.map(|template| {
                match template.min_size {
                    // grid-template-*: repeat(<number>, minmax(0, 1fr));
                    crate::GridTemplateMinSize::Zero => {
                        vec![repeat(
                            template.repeat,
                            vec![minmax(length(0.0_f32), fr(1.0_f32))],
                        )]
                    }
                    // grid-template-*: repeat(<number>, minmax(min-content, 1fr));
                    crate::GridTemplateMinSize::MinContent => {
                        vec![repeat(
                            template.repeat,
                            vec![minmax(min_content(), fr(1.0_f32))],
                        )]
                    }
                    // grid-template-*: repeat(<number>, minmax(0, max-content))
                    crate::GridTemplateMinSize::MaxContent => {
                        vec![repeat(
                            template.repeat,
                            vec![minmax(length(0.0_f32), max_content())],
                        )]
                    }
                }
            })
            .unwrap_or_default()
        }

        taffy::style::Style {
            display: self.display.into(),
            overflow: self.overflow.into(),
            scrollbar_width: self.scrollbar_width.to_taffy(rem_size, scale_factor),
            position: self.position.into(),
            inset: self.inset.to_taffy(rem_size, scale_factor),
            size: self.size.to_taffy(rem_size, scale_factor),
            min_size: self.min_size.to_taffy(rem_size, scale_factor),
            max_size: self.max_size.to_taffy(rem_size, scale_factor),
            aspect_ratio: self.aspect_ratio,
            margin: self.margin.to_taffy(rem_size, scale_factor),
            padding: self.padding.to_taffy(rem_size, scale_factor),
            border: border_widths_to_taffy(&self.border_widths, rem_size, scale_factor),
            align_items: self.align_items.map(|x| x.into()),
            align_self: self.align_self.map(|x| x.into()),
            align_content: self.align_content.map(|x| x.into()),
            justify_content: self.justify_content.map(|x| x.into()),
            gap: self.gap.to_taffy(rem_size, scale_factor),
            flex_direction: self.flex_direction.into(),
            flex_wrap: self.flex_wrap.into(),
            flex_basis: self.flex_basis.to_taffy(rem_size, scale_factor),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            grid_template_rows: to_grid_repeat(&self.grid_rows),
            grid_template_columns: to_grid_repeat(&self.grid_cols),
            grid_row: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.row))
                .unwrap_or_default(),
            grid_column: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.column))
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl ToTaffy<f32> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> f32 {
        round_to_device_pixel(self.to_pixels(rem_size).0, scale_factor)
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for Length {
    fn to_taffy(
        &self,
        rem_size: Pixels,
        scale_factor: f32,
    ) -> taffy::prelude::LengthPercentageAuto {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::LengthPercentageAuto::auto(),
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::prelude::Dimension {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::Dimension::auto(),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            DefiniteLength::Absolute(length) => length.to_taffy(rem_size, scale_factor),
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentage::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentageAuto {
        match self {
            DefiniteLength::Absolute(length) => length.to_taffy(rem_size, scale_factor),
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentageAuto::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Dimension {
        match self {
            DefiniteLength::Absolute(length) => length.to_taffy(rem_size, scale_factor),
            DefiniteLength::Fraction(fraction) => taffy::style::Dimension::percent(*fraction),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        taffy::style::LengthPercentage::length(self.to_taffy(rem_size, scale_factor))
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentageAuto {
        taffy::style::LengthPercentageAuto::length(self.to_taffy(rem_size, scale_factor))
    }
}

impl ToTaffy<taffy::style::Dimension> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Dimension {
        taffy::style::Dimension::length(self.to_taffy(rem_size, scale_factor))
    }
}

impl<T, T2> From<TaffyPoint<T>> for Point<T2>
where
    T: Into<T2>,
    T2: Clone + Debug + Default + PartialEq,
{
    fn from(point: TaffyPoint<T>) -> Point<T2> {
        Point {
            x: point.x.into(),
            y: point.y.into(),
        }
    }
}

impl<T, T2> From<Point<T>> for TaffyPoint<T2>
where
    T: Into<T2> + Clone + Debug + Default + PartialEq,
{
    fn from(val: Point<T>) -> Self {
        TaffyPoint {
            x: val.x.into(),
            y: val.y.into(),
        }
    }
}

impl<T, U> ToTaffy<TaffySize<U>> for Size<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffySize<U> {
        TaffySize {
            width: self.width.to_taffy(rem_size, scale_factor),
            height: self.height.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> ToTaffy<TaffyRect<U>> for Edges<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffyRect<U> {
        TaffyRect {
            top: self.top.to_taffy(rem_size, scale_factor),
            right: self.right.to_taffy(rem_size, scale_factor),
            bottom: self.bottom.to_taffy(rem_size, scale_factor),
            left: self.left.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> From<TaffySize<T>> for Size<U>
where
    T: Into<U>,
    U: Clone + Debug + Default + PartialEq,
{
    fn from(taffy_size: TaffySize<T>) -> Self {
        Size {
            width: taffy_size.width.into(),
            height: taffy_size.height.into(),
        }
    }
}

impl<T, U> From<Size<T>> for TaffySize<U>
where
    T: Into<U> + Clone + Debug + Default + PartialEq,
{
    fn from(size: Size<T>) -> Self {
        TaffySize {
            width: size.width.into(),
            height: size.height.into(),
        }
    }
}

/// The space available for an element to be laid out in
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub enum AvailableSpace {
    /// The amount of space available is the specified number of pixels
    Definite(Pixels),
    /// The amount of space available is indefinite and the node should be laid out under a min-content constraint
    #[default]
    MinContent,
    /// The amount of space available is indefinite and the node should be laid out under a max-content constraint
    MaxContent,
}

impl AvailableSpace {
    /// Returns a `Size` with both width and height set to `AvailableSpace::MinContent`.
    ///
    /// This function is useful when you want to create a `Size` with the minimum content constraints
    /// for both dimensions.
    ///
    /// # Examples
    ///
    /// ```
    /// use vespertine::AvailableSpace;
    /// let min_content_size = AvailableSpace::min_size();
    /// assert_eq!(min_content_size.width, AvailableSpace::MinContent);
    /// assert_eq!(min_content_size.height, AvailableSpace::MinContent);
    /// ```
    pub const fn min_size() -> Size<Self> {
        Size {
            width: Self::MinContent,
            height: Self::MinContent,
        }
    }
}

impl From<AvailableSpace> for TaffyAvailableSpace {
    fn from(space: AvailableSpace) -> TaffyAvailableSpace {
        match space {
            AvailableSpace::Definite(Pixels(value)) => TaffyAvailableSpace::Definite(value),
            AvailableSpace::MinContent => TaffyAvailableSpace::MinContent,
            AvailableSpace::MaxContent => TaffyAvailableSpace::MaxContent,
        }
    }
}

impl From<TaffyAvailableSpace> for AvailableSpace {
    fn from(space: TaffyAvailableSpace) -> AvailableSpace {
        match space {
            TaffyAvailableSpace::Definite(value) => AvailableSpace::Definite(Pixels(value)),
            TaffyAvailableSpace::MinContent => AvailableSpace::MinContent,
            TaffyAvailableSpace::MaxContent => AvailableSpace::MaxContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(pixels: Pixels) -> Self {
        AvailableSpace::Definite(pixels)
    }
}

impl From<Size<Pixels>> for Size<AvailableSpace> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: AvailableSpace::Definite(size.width),
            height: AvailableSpace::Definite(size.height),
        }
    }
}

#[cfg(test)]
mod persistence_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_widths_to_taffy_use_stroke_snapping() {
        let border_widths = Edges {
            top: Pixels(0.0).into(),
            right: Pixels(0.4).into(),
            bottom: Pixels(0.5).into(),
            left: Pixels(1.6).into(),
        };
        let taffy_border = border_widths_to_taffy(&border_widths, Pixels(16.0), 1.0);

        assert_eq!(
            taffy_border.top,
            taffy::style::LengthPercentage::length(0.0)
        );
        assert_eq!(
            taffy_border.right,
            taffy::style::LengthPercentage::length(1.0)
        );
        assert_eq!(
            taffy_border.bottom,
            taffy::style::LengthPercentage::length(1.0)
        );
        assert_eq!(
            taffy_border.left,
            taffy::style::LengthPercentage::length(2.0)
        );
    }
}
