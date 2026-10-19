//! The `highlight` host: inherited search washes and explicit ranges (TXT-009 R1-R3, R6-R12).
//!
//! A [`Highlight`] declares one or more [`HighlightSpec`]s for a subtree. Text runs of the
//! subtree register their text in prepaint (retained text, grouped as logical groups); the host
//! resolves the declaration over every registered group when it starts painting, reports the match
//! count, and each run paints rounded washes for the segments that fall inside it, numbered in
//! paint order. Text generated while a native widget paints is matched against exactly the string
//! it paints.

use crate::registry::{PaintedHighlight, RunKey};
use crate::search::matcher::byte_offsets_to_utf16;
use crate::search::{
    GroupInput, HighlightDeclaration, HighlightSpec, Numbering, Resolution, Resolver,
};
use crate::service::TextService;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window,
};

/// Default alpha of the base wash.
pub const BASE_ALPHA: f32 = 0.30;
/// Default alpha of the active wash.
pub const ACTIVE_ALPHA: f32 = 0.65;

type CountCallback = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// State that survives frames: the resolver cache, the numbering and the last reported identity.
pub struct HostState {
    resolver: Resolver,
    numbering: Rc<Numbering>,
    last_identity: u64,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            resolver: Resolver::new(),
            numbering: Numbering::shared(),
            last_identity: Resolution::empty().identity,
        }
    }
}

/// One wash of a run: a byte range of the displayed text.
#[derive(Clone, Debug)]
pub struct Wash {
    /// Byte range in the run's displayed text.
    pub range: Range<usize>,
    /// Fill color.
    pub color: Hsla,
    /// Corner radius in logical px.
    pub radius: f32,
    /// Whether this is the active match.
    pub active: bool,
}

/// The declaration in force for a subtree during one frame.
pub struct HostFrame {
    declaration: HighlightDeclaration,
    state: Rc<RefCell<HostState>>,
    numbering: Rc<Numbering>,
    key: u64,
    groups: RefCell<Vec<GroupInput>>,
    pieces: RefCell<HashMap<RunKey, (u64, usize)>>,
    resolution: RefCell<Option<Arc<Resolution>>>,
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Identity of a logical group for the resolver.
pub fn group_id(group: Option<RunKey>, run: RunKey) -> u64 {
    hash_of(group.unwrap_or(run))
}

impl HostFrame {
    /// Whether the declaration paints nothing (an empty declaration still owns its subtree).
    pub fn is_empty(&self) -> bool {
        self.declaration.is_empty()
    }

    /// Registers a retained run's text in prepaint; returns the piece index within its group. A
    /// run registered again keeps its piece.
    pub fn register(&self, run: RunKey, group: u64, text: &str) -> usize {
        if let Some((_, piece)) = self.pieces.borrow().get(&run) {
            return *piece;
        }
        let mut groups = self.groups.borrow_mut();
        let piece = match groups.iter_mut().find(|candidate| candidate.id == group) {
            Some(existing) => {
                existing.pieces.push(Arc::from(text));
                existing.pieces.len() - 1
            }
            None => {
                groups.push(GroupInput::new(group, [text]));
                0
            }
        };
        self.pieces.borrow_mut().insert(run, (group, piece));
        piece
    }

    fn color(&self, spec: &HighlightSpec, active: bool, accent: Hsla) -> Hsla {
        if active {
            spec.active_color.unwrap_or(Hsla {
                a: ACTIVE_ALPHA,
                ..accent
            })
        } else {
            spec.color.unwrap_or(Hsla {
                a: BASE_ALPHA,
                ..accent
            })
        }
    }

    /// Washes of a retained run (`piece` from [`HostFrame::register`]) or, for a native run,
    /// of the exact string it paints. Numbering happens here, in paint order.
    pub fn washes(
        &self,
        run: RunKey,
        group: u64,
        piece: Option<usize>,
        text: &str,
        accent: Hsla,
    ) -> Vec<Wash> {
        let mut washes = Vec::new();
        let specs = self.declaration.specs();
        match piece {
            Some(piece) => {
                let resolution = self.resolution.borrow();
                let Some(resolution) = resolution.as_ref() else {
                    return washes;
                };
                for (match_index, segment) in resolution.segments_for(group, piece) {
                    let resolved = &resolution.matches[match_index];
                    let spec = &specs[resolved.spec_index];
                    let ordinal = self
                        .numbering
                        .retained_ordinal(self.key, match_index, resolved);
                    let number = Numbering::display_number(spec, ordinal);
                    let active = Numbering::is_active(spec, number);
                    washes.push(Wash {
                        range: segment.range.clone(),
                        color: self.color(spec, active, accent),
                        radius: spec.radius,
                        active,
                    });
                }
            }
            None => {
                let native_key = hash_of(run);
                for (spec_index, spec) in specs.iter().enumerate() {
                    for (match_index, range) in self
                        .numbering
                        .match_native_text(spec, text)
                        .into_iter()
                        .enumerate()
                    {
                        let ordinal =
                            self.numbering
                                .ordinal_for(spec_index, native_key, match_index);
                        let number = Numbering::display_number(spec, ordinal);
                        let active = Numbering::is_active(spec, number);
                        washes.push(Wash {
                            range,
                            color: self.color(spec, active, accent),
                            radius: spec.radius,
                            active,
                        });
                    }
                }
            }
        }
        washes
    }
}

/// Converts a wash to its inspection-log entry.
pub fn log_entry(
    key: RunKey,
    text: &str,
    wash: &Wash,
    rects: &[Bounds<Pixels>],
) -> PaintedHighlight {
    let units = byte_offsets_to_utf16(text, [wash.range.start, wash.range.end]);
    PaintedHighlight {
        key,
        text: text.to_string(),
        start: units[0],
        end: units[1],
        active: wash.active,
        rects: rects
            .iter()
            .map(|rect| {
                [
                    f32::from(rect.origin.x),
                    f32::from(rect.origin.y),
                    f32::from(rect.size.width),
                    f32::from(rect.size.height),
                ]
            })
            .collect(),
    }
}

/// Declares a highlight for `child`'s subtree.
pub struct Highlight {
    id: ElementId,
    declaration: HighlightDeclaration,
    on_count: Option<CountCallback>,
    numbering: Option<Rc<Numbering>>,
    child: AnyElement,
}

/// Declares `declaration` for `child`'s subtree; `id` keeps the match cache across frames.
pub fn highlight(
    id: impl Into<ElementId>,
    declaration: HighlightDeclaration,
    child: impl IntoElement,
) -> Highlight {
    Highlight {
        id: id.into(),
        declaration,
        on_count: None,
        numbering: None,
        child: child.into_any_element(),
    }
}

impl Highlight {
    /// Reports `matchCount` whenever the match identity changes (never for the initial empty
    /// result, never for `activeIndex`, colors or radius).
    pub fn on_match_count(
        mut self,
        callback: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_count = Some(Rc::new(callback));
        self
    }

    /// Shares the numbering with other contexts created for the same declaration, so that two
    /// views of one subtree draw from one sequence (R8).
    pub fn shared_numbering(mut self, numbering: Rc<Numbering>) -> Self {
        self.numbering = Some(numbering);
        self
    }
}

impl IntoElement for Highlight {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What the host keeps between layout, prepaint and paint.
pub struct HighlightPrepaint {
    frame: Rc<HostFrame>,
}

impl Element for Highlight {
    type RequestLayoutState = Rc<RefCell<HostState>>;
    type PrepaintState = HighlightPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = match id {
            Some(id) => window.with_element_state::<Rc<RefCell<HostState>>, _>(id, |state, _| {
                let state = state.unwrap_or_default();
                (state.clone(), state)
            }),
            None => Rc::default(),
        };
        if let Some(numbering) = &self.numbering {
            state.borrow_mut().numbering = numbering.clone();
        }
        (self.child.request_layout(window, cx), state)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let numbering = request_layout.borrow().numbering.clone();
        let frame = Rc::new(HostFrame {
            declaration: self.declaration.clone(),
            state: request_layout.clone(),
            numbering,
            key: hash_of(id),
            groups: RefCell::default(),
            pieces: RefCell::default(),
            resolution: RefCell::default(),
        });
        let service = TextService::current();
        if let Some(service) = &service {
            service.push_highlight(frame.clone());
        }
        self.child.prepaint(window, cx);
        if let Some(service) = &service {
            service.pop_highlight();
        }
        HighlightPrepaint { frame }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let frame = prepaint.frame.clone();
        let service = TextService::current();
        if let Some(service) = &service {
            let mut state = frame.state.borrow_mut();
            state.numbering.begin_frame_if_new(service.frame_id());
            if !frame.declaration.is_empty() {
                let groups = frame.groups.borrow().clone();
                let resolution = state.resolver.resolve(&frame.declaration, &groups);
                let fire = (resolution.identity != state.last_identity).then(|| {
                    state.last_identity = resolution.identity;
                    resolution.total
                });
                *frame.resolution.borrow_mut() = Some(resolution);
                drop(state);
                if let (Some(total), Some(callback)) = (fire, self.on_count.clone()) {
                    window.defer(cx, move |window, cx| callback(total, window, cx));
                }
            } else if state.last_identity != Resolution::empty().identity {
                // The declaration became empty: report the (now zero) count once.
                state.last_identity = Resolution::empty().identity;
                drop(state);
                if let Some(callback) = self.on_count.clone() {
                    window.defer(cx, move |window, cx| callback(0, window, cx));
                }
            }
            service.push_highlight(frame.clone());
        }
        self.child.paint(window, cx);
        if let Some(service) = &service {
            service.pop_highlight();
        }
    }
}
