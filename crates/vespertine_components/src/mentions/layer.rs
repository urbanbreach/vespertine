//! The mentions layer of the multiline editor (CMP-010 R1 to R4, R13).
//!
//! The layer registers the owner's reference grammars as the editor's projection: every reference
//! becomes an atomic replacement showing a chip, a dangling attachment reference becomes its plain
//! label. It also holds the live attachment numbers of the editor's scope and, optionally, the
//! owner's [`AttachmentScope`] so that the editor reconciles it after every text change.

use super::{
    attachments::AttachmentScope,
    chip::{ChipDisplay, project_references},
    reference::{ChipIcon, GrammarSet, ReferenceDescriptor, ReferenceGrammar},
};
use crate::text::area::{AreaLayer, Replacement};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    rc::Rc,
    sync::Arc,
};
use vespertine::Image;

/// A chip of the current projection: what painting, hit testing and hover need to know.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipInfo {
    /// The reference the chip shows; `descriptor.range` is the chip's source range.
    pub descriptor: ReferenceDescriptor,
    /// The label text (disambiguated and compacted).
    pub label: String,
    /// The icon the owner's provider chose.
    pub icon: ChipIcon,
    /// The display string's parts.
    pub layout: ChipDisplay,
}

type Reconciler = Box<dyn FnMut(&BTreeSet<u32>) -> BTreeSet<u32>>;

#[derive(Default)]
struct State {
    grammars: GrammarSet,
    /// The attachment numbers the owner registered; `None` treats every number as live.
    live: Option<BTreeSet<u32>>,
    images: BTreeMap<u32, Option<Image>>,
    scope: String,
    version: u64,
    chips: Vec<ChipInfo>,
    reconciler: Option<Reconciler>,
}

/// The mentions layer; attach it with
/// [`TextArea::set_reference_grammars`](crate::text::TextArea::set_reference_grammars).
#[derive(Default)]
pub struct MentionsLayer {
    state: RefCell<State>,
}

impl MentionsLayer {
    /// A layer without grammars: the source is shown verbatim.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the grammars (`set_reference_grammars`).
    pub fn set_grammars(&self, grammars: Vec<Arc<dyn ReferenceGrammar>>) {
        let mut state = self.state.borrow_mut();
        state.grammars = GrammarSet::from_grammars(grammars);
        state.version += 1;
    }

    /// The grammars.
    pub fn grammars(&self) -> GrammarSet {
        self.state.borrow().grammars.clone()
    }

    /// Whether any grammar is installed.
    pub fn has_grammars(&self) -> bool {
        !self.state.borrow().grammars.is_empty()
    }

    /// Registers the live attachments of `scope` (R4): references to numbers outside the map are
    /// dangling and show their plain label. Replaces an attached [`AttachmentScope`].
    pub fn set_live(&self, scope: &str, live: BTreeMap<u32, Option<Image>>) {
        let mut state = self.state.borrow_mut();
        state.reconciler = None;
        state.scope = scope.to_owned();
        state.live = Some(live.keys().copied().collect());
        state.images = live;
        state.version += 1;
    }

    /// Attaches the owner's attachment scope: the layer reads its live numbers and reconciles it
    /// with [`reconcile`](Self::reconcile). The scope's callbacks run during reconciliation and
    /// must not touch the scope or the editor.
    pub fn set_scope<T: 'static>(&self, scope: Rc<RefCell<AttachmentScope<T>>>) {
        let mut state = self.state.borrow_mut();
        state.scope = scope.borrow().scope_id().to_owned();
        state.live = Some(scope.borrow().live_numbers());
        state.images.clear();
        state.reconciler = Some(Box::new(move |referenced| {
            let mut scope = scope.borrow_mut();
            scope.reconcile(referenced);
            scope.live_numbers()
        }));
        state.version += 1;
    }

    /// Whether an attachment scope or live set was registered.
    pub fn has_attachments(&self) -> bool {
        self.state.borrow().live.is_some()
    }

    /// The attachment scope identifier written into clipboard metadata (empty until registered).
    pub fn scope_id(&self) -> String {
        self.state.borrow().scope.clone()
    }

    /// The registered live attachment numbers; `None` when every number counts as live.
    pub fn live_numbers(&self) -> Option<BTreeSet<u32>> {
        self.state.borrow().live.clone()
    }

    /// The image registered for a live attachment number.
    pub fn image(&self, number: u32) -> Option<Image> {
        self.state.borrow().images.get(&number)?.clone()
    }

    /// Reconciles the attached scope with the numbers referenced in `source` (R13). Returns
    /// whether the live set changed (the projection then needs a rebuild). Without an attached
    /// scope nothing happens.
    pub fn reconcile(&self, source: &str) -> bool {
        let mut state = self.state.borrow_mut();
        if state.reconciler.is_none() {
            return false;
        }
        let referenced = state.grammars.attachment_numbers(source);
        let reconciler = state.reconciler.as_mut().expect("checked above");
        let live = reconciler(&referenced);
        if state.live.as_ref() == Some(&live) {
            return false;
        }
        state.live = Some(live);
        state.version += 1;
        true
    }

    /// The references of `source` (`references()`).
    pub fn references(&self, source: &str) -> Vec<ReferenceDescriptor> {
        self.state.borrow().grammars.references(source)
    }

    /// The chips of the last projection, in source order.
    pub fn chips(&self) -> Vec<ChipInfo> {
        self.state.borrow().chips.clone()
    }

    /// The source ranges and expanded texts of every reference (used by the clipboard).
    pub fn expanded_text(&self, source: &str, range: Range<usize>) -> String {
        super::clipboard::expanded_plain_text(source, range, &self.state.borrow().grammars)
    }
}

impl AreaLayer for MentionsLayer {
    fn version(&self) -> u64 {
        self.state.borrow().version
    }

    fn replacements(
        &self,
        source: &str,
        _active: Range<usize>,
        marked: Option<Range<usize>>,
    ) -> Vec<Replacement> {
        let mut state = self.state.borrow_mut();
        if state.grammars.is_empty() {
            state.chips.clear();
            return Vec::new();
        }
        let projected = project_references(source, &state.grammars, state.live.as_ref());
        let mut replacements = Vec::with_capacity(projected.len());
        let mut chips = Vec::new();
        // A reference that the composition touches is not projected until it ends: as an atomic
        // chip it would widen the input method's next replacement over the whole reference.
        let composing = |range: &Range<usize>| {
            marked
                .as_ref()
                .is_some_and(|marked| range.start < marked.end && marked.start < range.end)
        };
        for item in projected {
            if composing(&item.replacement.range) {
                continue;
            }
            replacements.push(Replacement {
                range: item.replacement.range.clone(),
                display: item.replacement.display.clone(),
                atomic: item.replacement.atomic,
            });
            if let (Some(layout), Some(icon)) = (item.layout, item.icon) {
                chips.push(ChipInfo {
                    descriptor: item.reference.descriptor,
                    label: item.label,
                    icon,
                    layout,
                });
            }
        }
        state.chips = chips;
        replacements
    }
}
