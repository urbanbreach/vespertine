//! References inside the multiline editor (CMP-010): the chips of the projection, insertion and
//! removal of references, attachment reconciliation, hover detail, image-chip activation,
//! chip painting data and the scoped clipboard hooks.

use super::{
    EditKind, EditorHost, TextArea, TextAreaEvent,
    layout::{AreaLayout, RangeSegment},
    now_ms,
};
use crate::{
    mentions::{
        AttachmentScope, ChipIcon, ChipInfo, EditGate, FaceMetrics, HOVER_DETAIL_DELAY_MS,
        HoverChip, HoverDetail, ImageChipActivation, MentionsLayer, POPUP_GAP, POPUP_HEIGHT,
        PillGeometry, PillRect, PopupSide, ReferenceDescriptor, ReferenceGrammar,
        ReferenceInsertion, ScopedClipboardPayload, TimerRequest, centering_offset, detail_text,
        insert_reference, paste_text, pill_geometry, popup_top, remove_attachment_references,
        replace_reference,
    },
    text::core::Affinity,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use vespertine::{
    App, Bounds, ClipboardItem, Context, Font, Hsla, Image, Pixels, Point, Task, Window, point, px,
    size,
};

/// Horizontal padding of the hover detail popup.
pub(super) const POPUP_PADDING_X: f32 = 8.0;

/// A chip of the current projection with its range in the display text.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipRecord {
    /// The chip: descriptor (its `range` is the source range), label, icon and display parts.
    pub info: ChipInfo,
    /// The chip's byte range in the display text.
    pub display: Range<usize>,
}

impl ChipRecord {
    /// The chip's source range.
    pub fn source(&self) -> Range<usize> {
        self.info.descriptor.range.clone()
    }
}

/// A chip under a window position.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipHit {
    /// Index into [`TextArea::chips`].
    pub index: usize,
    /// The chip's reference.
    pub descriptor: ReferenceDescriptor,
    /// The row segment under the position, in window coordinates.
    pub segment: Bounds<Pixels>,
}

/// What a paste inserted: the token with which the owner may amend the pasted text inside the
/// paste's own undo step ([`TextArea::amend_paste`], CMP-010 R16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasteRecord {
    /// The inserted source range.
    pub range: Range<usize>,
    /// The edit revision right after the paste.
    pub revision: u64,
}

/// The placement and text of the hover detail popup, in window coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverPopup {
    /// The popup box (24 px tall).
    pub bounds: Bounds<Pixels>,
    /// The detail text: the full identity, or the attachment's name.
    pub text: String,
    /// Whether the popup is above or below its chip.
    pub side: PopupSide,
}

/// One painted row segment of a chip (CMP-010 R6, R7), in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipPill {
    /// The pill and, on the chip's first segment, its icon well.
    pub geometry: PillGeometry,
    /// Index into [`TextArea::chips`].
    pub chip: usize,
}

/// The colors chips are painted with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ChipColors {
    pub(super) ink: Hsla,
    pub(super) well: Hsla,
    pub(super) glyph: Hsla,
}

#[derive(Default)]
pub(super) struct ReferenceState {
    pub(super) layer: Option<Rc<MentionsLayer>>,
    pub(super) chips: Vec<ChipRecord>,
    pub(super) padding_font: Option<Font>,
    hover: HoverDetail,
    hover_task: Option<Task<()>>,
    hover_anchor: Option<Bounds<Pixels>>,
    hover_pointer: Option<Point<Pixels>>,
    activation: ImageChipActivation,
}

impl ReferenceState {
    pub(super) fn invalidate_hover(&mut self) {
        self.hover.invalidate();
        self.hover_task = None;
        self.hover_anchor = None;
    }

    /// The projection was rebuilt: the hover target must still exist (R8).
    pub(super) fn projection_rebuilt(&mut self) {
        let list = self.hover_chips();
        self.hover.projection_rebuilt(&list);
    }

    fn hover_chips(&self) -> Vec<HoverChip> {
        self.chips.iter().map(hover_chip).collect()
    }
}

fn hover_chip(chip: &ChipRecord) -> HoverChip {
    HoverChip {
        range: chip.source(),
        identity: chip.info.descriptor.identity.clone(),
        image: chip.info.descriptor.kind.is_image(),
    }
}

/// The asset path of a chip icon: names without a path separator or `.svg` extension are looked
/// up as `icons/<name>.svg`.
pub fn icon_asset_path(name: &str) -> String {
    if name.contains('/') || name.ends_with(".svg") {
        name.to_owned()
    } else {
        format!("icons/{name}.svg")
    }
}

impl TextArea {
    // ---- configuration ----------------------------------------------------------------------------

    fn mentions(&mut self) -> Rc<MentionsLayer> {
        if let Some(layer) = &self.refs.layer {
            return layer.clone();
        }
        let layer = Rc::new(MentionsLayer::new());
        self.layers.push(layer.clone());
        self.refs.layer = Some(layer.clone());
        layer
    }

    /// Installs the owner's reference grammars before the first frame (CMP-010 R1): the editor
    /// shows every reference as an atomic chip while the source keeps the canonical text.
    pub fn with_references(mut self, grammars: Vec<Arc<dyn ReferenceGrammar>>) -> Self {
        self.mentions().set_grammars(grammars);
        self.manual_epoch += 1;
        self
    }

    /// Sets the fixed face the chip padding is shaped in (CMP-010 R2); the default is the UI
    /// sans face of the theme.
    pub fn with_chip_padding_font(mut self, font: Font) -> Self {
        self.refs.padding_font = Some(font);
        self.manual_epoch += 1;
        self
    }

    /// Replaces the reference grammars (`set_reference_grammars`).
    pub fn set_reference_grammars(
        &mut self,
        grammars: Vec<Arc<dyn ReferenceGrammar>>,
        cx: &mut Context<Self>,
    ) {
        self.mentions().set_grammars(grammars);
        self.references_changed(cx);
    }

    /// Registers the live attachments of an editing scope (R4): a reference to a number outside
    /// the map is displayed as its plain label. The editor does not own the attachments; the
    /// images are kept for the owner's use through [`attachment_image`](Self::attachment_image).
    /// Replaces an attachment scope attached with
    /// [`set_attachment_scope`](Self::set_attachment_scope).
    pub fn set_live_attachments(
        &mut self,
        scope: impl Into<String>,
        live: BTreeMap<u32, Option<Image>>,
        cx: &mut Context<Self>,
    ) {
        let scope = scope.into();
        self.mentions().set_live(&scope, live);
        self.references_changed(cx);
    }

    /// Attaches the owner's [`AttachmentScope`] (R13): the editor reads its live numbers and
    /// reconciles it with the numbers referenced in the source after every edit, undo and redo
    /// and now (the scope changed). The scope's callbacks run during the reconciliation and must
    /// not touch the scope or this editor.
    pub fn set_attachment_scope<T: 'static>(
        &mut self,
        scope: Rc<RefCell<AttachmentScope<T>>>,
        cx: &mut Context<Self>,
    ) {
        let layer = self.mentions();
        layer.set_scope(scope);
        layer.reconcile(self.core.text());
        self.references_changed(cx);
    }

    /// The attachment scope identifier written into clipboard metadata; empty until a scope or
    /// live set was registered.
    pub fn attachment_scope_id(&self) -> String {
        self.refs
            .layer
            .as_ref()
            .map(|layer| layer.scope_id())
            .unwrap_or_default()
    }

    /// The image registered for a live attachment number.
    pub fn attachment_image(&self, number: u32) -> Option<Image> {
        self.refs.layer.as_ref()?.image(number)
    }

    fn references_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh_projection(cx);
        cx.notify();
    }

    /// The chips of the layer whose atomic replacement survived the projection's validation,
    /// with their display ranges.
    pub(super) fn surviving_chips(&self, atomics: &[Range<usize>]) -> Vec<ChipRecord> {
        let Some(layer) = &self.refs.layer else {
            return Vec::new();
        };
        layer
            .chips()
            .into_iter()
            .filter_map(|info| {
                let range = info.descriptor.range.clone();
                let found = atomics.binary_search_by_key(&range.start, |atomic| atomic.start);
                found
                    .ok()
                    .filter(|&index| atomics[index].end == range.end)?;
                let display = self.display_of(range.start)..self.display_of(range.end);
                (display.len() == info.layout.text.len()).then_some(ChipRecord { info, display })
            })
            .collect()
    }

    /// The references of the source, merged and sorted (R1), dangling ones included.
    pub fn references(&self) -> Vec<ReferenceDescriptor> {
        self.refs
            .layer
            .as_ref()
            .map(|layer| layer.references(self.core.text()))
            .unwrap_or_default()
    }

    /// The chips of the current projection (dangling references are not chips), in source order.
    pub fn chips(&self) -> &[ChipRecord] {
        &self.refs.chips
    }

    // ---- text changes -------------------------------------------------------------------------------

    /// The text changed: reconciles the attachment scope, keeps the projection (and with it the
    /// atomic ranges) current and invalidates the hover detail (R8, R13).
    pub(super) fn text_changed(&mut self, cx: &App) {
        self.refs.invalidate_hover();
        self.refs.activation.cancel();
        if let Some(layer) = self.refs.layer.clone() {
            layer.reconcile(self.core.text());
        }
        self.refresh_projection(cx);
    }

    // ---- insertion and removal (R10, R11) ------------------------------------------------------------

    fn edit_gate(&self) -> EditGate {
        EditGate {
            read_only: self.core.is_read_only() || self.disabled,
            composing: self.core.marked().is_some(),
        }
    }

    fn apply_insertion(
        &mut self,
        insertion: ReferenceInsertion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = now_ms(cx);
        let changed = self.core.apply(
            insertion.range.clone(),
            &insertion.text,
            EditKind::Other,
            now,
        );
        self.refresh_projection(cx);
        self.core
            .set_selection(insertion.caret..insertion.caret, false);
        self.finish_edit(changed, window, cx);
        changed
    }

    /// Replaces the selection with a reference link (R10): the link gets a leading space after a
    /// word character and a trailing separator by the next character, the caret ends after the
    /// link and any separator, and the whole edit is one undo step that never coalesces with
    /// typing. Several links are inserted at once by joining them with single spaces
    /// ([`join_links`](crate::mentions::join_links)). Returns `false` and changes nothing when the
    /// editor is read-only, disabled or has a marked range. A pending external replacement is
    /// committed first.
    pub fn insert_reference(
        &mut self,
        link: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let gate = self.edit_gate();
        if !gate.allows_reference_edit() {
            return false;
        }
        self.cancel_external(cx);
        let selection = self.core.selection();
        let Some(insertion) = insert_reference(self.core.text(), selection, link, gate) else {
            return false;
        };
        self.apply_insertion(insertion, window, cx)
    }

    /// Replaces a completed completion token with a reference link (R10): the same trailing
    /// rule as [`insert_reference`](Self::insert_reference) and no leading separator.
    pub fn replace_reference(
        &mut self,
        range: Range<usize>,
        link: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let gate = self.edit_gate();
        if !gate.allows_reference_edit() {
            return false;
        }
        self.cancel_external(cx);
        let Some(insertion) = replace_reference(self.core.text(), range, link, gate) else {
            return false;
        };
        self.apply_insertion(insertion, window, cx)
    }

    /// Removes every chip that references attachment `number`, each with one following space
    /// when it stood at the start or after whitespace (R11), as one undo step; the selection
    /// shifts by the removed widths. Rejected when read-only, disabled or composing; returns
    /// whether anything was removed.
    pub fn remove_attachment_references(
        &mut self,
        number: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.edit_gate().allows_reference_edit() {
            return false;
        }
        let descriptors = self.references();
        let Some(removal) = remove_attachment_references(self.core.text(), &descriptors, number)
        else {
            return false;
        };
        self.cancel_external(cx);
        let (Some(first), Some(last)) = (removal.removed.first(), removal.removed.last()) else {
            return false;
        };
        let span = first.start..last.end;
        let mut replacement = String::with_capacity(span.len());
        {
            let text = self.core.text();
            let mut cursor = span.start;
            for range in &removal.removed {
                replacement.push_str(&text[cursor..range.start]);
                cursor = range.end;
            }
            replacement.push_str(&text[cursor..span.end]);
        }
        let selection = removal.map_range(self.core.selection());
        let reversed = self.core.is_reversed();
        let now = now_ms(cx);
        let changed = self.core.apply(span, &replacement, EditKind::Other, now);
        self.refresh_projection(cx);
        self.core.set_selection(selection, reversed);
        self.finish_edit(changed, window, cx);
        changed
    }

    // ---- clipboard (R15, R16) --------------------------------------------------------------------------

    /// The clipboard item of a copy: the expanded plain text with the scoped structured
    /// metadata, or `None` when the editor has no reference grammar.
    pub(super) fn scoped_copy_item(&self) -> Option<ClipboardItem> {
        let layer = self
            .refs
            .layer
            .as_ref()
            .filter(|layer| layer.has_grammars())?;
        ScopedClipboardPayload::for_selection(
            self.core.text(),
            self.core.selection(),
            &layer.grammars(),
            &layer.scope_id(),
        )
        .map(|payload| payload.to_clipboard_item())
    }

    /// The text a paste inserts (R16): the metadata's source when it still matches the clipboard
    /// text and its attachments belong to this scope (or it references none), the expanded plain
    /// text otherwise.
    pub(super) fn restored_paste_text(&self, text: String, metadata: Option<&str>) -> String {
        let Some(layer) = self
            .refs
            .layer
            .as_ref()
            .filter(|layer| layer.has_grammars())
        else {
            return text;
        };
        let payload = metadata.and_then(ScopedClipboardPayload::from_json);
        paste_text(
            &text,
            payload.as_ref(),
            &layer.scope_id(),
            &layer.grammars(),
        )
    }

    /// Reports a paste that inserted `start..caret` so the owner may amend it (R16).
    pub(super) fn emit_pasted(&mut self, start: usize, cx: &mut Context<Self>) {
        let record = PasteRecord {
            range: start..self.core.cursor(),
            revision: self.core.edit_epoch(),
        };
        cx.emit(TextAreaEvent::Pasted(record));
    }

    /// Applies an owner's post-processing of pasted text (for example turning typed tokens into
    /// references) inside the paste's undo step (R16): only when the edit revision, the text, the
    /// empty marked range and the caret at the paste end are all unchanged since the paste.
    /// `range` is the part of the pasted range to replace with `text`. Returns whether the
    /// amendment was applied.
    pub fn amend_paste(
        &mut self,
        record: &PasteRecord,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let end = record.range.end;
        if self.disabled
            || self.core.is_read_only()
            || record.revision != self.core.edit_epoch()
            || self.core.marked().is_some()
            || self.core.selection() != (end..end)
            || range.start < record.range.start
            || range.end > end
            || range.start > range.end
        {
            return false;
        }
        let new_end = end - range.len() + text.len();
        if !self.core.replace_unrecorded(range, text) {
            return false;
        }
        self.refresh_projection(cx);
        self.core.set_selection(new_end..new_end, false);
        self.finish_edit(true, window, cx);
        true
    }

    // ---- hit testing ----------------------------------------------------------------------------------

    fn segment_bounds(&self, bounds: Bounds<Pixels>, segment: &RangeSegment) -> Bounds<Pixels> {
        let origin = self.content_origin(bounds);
        Bounds {
            origin: origin + segment.bounds.origin,
            size: segment.bounds.size,
        }
    }

    /// The chip row segment under a window position, clipped to the visible editor area (R8).
    pub fn chip_at(&self, position: Point<Pixels>) -> Option<ChipHit> {
        let bounds = self.bounds?;
        if !bounds.contains(&position) || self.refs.chips.is_empty() {
            return None;
        }
        let layout = self.layout.as_ref()?;
        let (offset, _) = self.hit_display(position)?;
        let first = self
            .refs
            .chips
            .partition_point(|chip| chip.display.end < offset);
        for (index, chip) in self.refs.chips.iter().enumerate().skip(first).take(2) {
            for segment in layout.range_segments(chip.display.clone()) {
                let rect = self.segment_bounds(bounds, &segment);
                if rect.contains(&position) {
                    return Some(ChipHit {
                        index,
                        descriptor: chip.info.descriptor.clone(),
                        segment: rect,
                    });
                }
            }
        }
        None
    }

    // ---- hover detail (R8) -----------------------------------------------------------------------------

    /// The pointer moved to `position`; `over` tells whether the editor is the topmost element
    /// there. Entering a non-image chip starts the 420 ms wait, motion inside it changes nothing,
    /// leaving hides the popup unless the pointer is inside it.
    pub(super) fn on_hover_move(
        &mut self,
        position: Point<Pixels>,
        over: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refs.chips.is_empty() && !self.refs.hover.is_visible() {
            return;
        }
        let was = self.refs.hover.state().clone();
        if let Some(popup) = self.hover_popup(window) {
            let mut region = popup.bounds;
            if popup.side == PopupSide::Above {
                region.size.height += px(POPUP_GAP);
            }
            if region.contains(&position) {
                self.refs.hover.popup_entered();
                return;
            }
            self.refs.hover.popup_left();
        }
        let hit = (over && self.drag.is_none())
            .then(|| self.chip_at(position))
            .flatten();
        let chip = hit
            .as_ref()
            .map(|hit| hover_chip(&self.refs.chips[hit.index]));
        if let Some(hit) = &hit {
            self.refs.hover_anchor = Some(hit.segment);
        }
        self.refs.hover_pointer = hit.is_some().then_some(position);
        let request = self
            .refs
            .hover
            .pointer_over(chip.as_ref(), now_ms(cx) as u64);
        if let Some(request) = request {
            self.schedule_hover(request, window, cx);
        }
        if *self.refs.hover.state() != was {
            cx.notify();
        }
    }

    fn schedule_hover(
        &mut self,
        request: TimerRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let generation = request.generation;
        self.refs.hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(HOVER_DETAIL_DELAY_MS))
                .await;
            let _ = this.update(cx, |this, cx| {
                let now = now_ms(cx) as u64;
                this.hover_timer_fired(generation, now, cx);
            });
        }));
    }

    /// The hover timer of `generation` fired at `now_ms` (motion-clock milliseconds): the popup
    /// shows only if that wait ran its full delay and the chip is still under the pointer.
    #[doc(hidden)]
    pub fn hover_timer_fired(&mut self, generation: u64, now_ms: u64, cx: &mut Context<Self>) {
        let under_pointer: Vec<HoverChip> = self
            .refs
            .hover_pointer
            .and_then(|position| self.chip_at(position))
            .map(|hit| vec![hover_chip(&self.refs.chips[hit.index])])
            .unwrap_or_default();
        let before = self.refs.hover.state().clone();
        self.refs
            .hover
            .timer_fired(generation, now_ms, &under_pointer);
        if *self.refs.hover.state() != before {
            cx.notify();
        }
    }

    /// The generation of the hover wait (changes on every invalidation).
    #[doc(hidden)]
    pub fn hover_generation(&self) -> u64 {
        self.refs.hover.generation()
    }

    /// Whether the hover detail popup is shown.
    pub fn hover_detail_visible(&self) -> bool {
        self.refs.hover.is_visible()
    }

    /// The popup to render while the hover detail is visible: 24 px tall, above the chip with a
    /// 1 px gap, or below it flush with the chip bottom when there is no room above.
    pub fn hover_popup(&self, window: &Window) -> Option<HoverPopup> {
        let target = self.refs.hover.visible_target()?;
        let chip = self.refs.chips.iter().find(|chip| {
            chip.source() == target.range && chip.info.descriptor.identity == target.identity
        })?;
        let anchor = self.refs.hover_anchor?;
        let descriptor = &chip.info.descriptor;
        let text = detail_text(
            &descriptor.kind,
            &descriptor.name,
            &descriptor.identity,
            descriptor.attachment.is_some(),
        );
        let run = vespertine::TextRun {
            len: text.len(),
            font: self.resolved_font.clone(),
            color: self.theme.unwrap_or_default().text,
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: Pixels::ZERO,
        };
        let shaped = window.text_system().shape_line(
            vespertine::SharedString::from(text.clone()),
            self.metrics.size,
            &[run],
            None,
        );
        let (top, side) = popup_top(f32::from(anchor.top()), f32::from(anchor.bottom()), 0.0);
        Some(HoverPopup {
            bounds: Bounds {
                origin: point(anchor.left(), px(top)),
                size: size(shaped.width() + px(2.0 * POPUP_PADDING_X), px(POPUP_HEIGHT)),
            },
            text,
            side,
        })
    }

    /// The pointer entered or left the popup (R8): it stays while the pointer is inside it.
    pub(super) fn popup_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        let before = self.refs.hover.state().clone();
        if hovered {
            self.refs.hover.popup_entered();
        } else {
            self.refs.hover.popup_left();
        }
        if *self.refs.hover.state() != before {
            cx.notify();
        }
    }

    // ---- activation (R9) -----------------------------------------------------------------------------

    /// A primary press at `position`: records a plain press on an image chip.
    pub(super) fn chip_pointer_down(
        &mut self,
        position: Point<Pixels>,
        shift: bool,
        click_count: usize,
    ) {
        self.refs.invalidate_hover();
        let hit = self.chip_at(position);
        self.refs.activation.press(
            hit.as_ref().map(|hit| &hit.descriptor),
            (f32::from(position.x), f32::from(position.y)),
            shift,
            click_count as u32,
        );
    }

    /// The primary button was released at `position`: activates the pressed image chip when the
    /// release is over the same chip and the pointer moved at most 4 px.
    pub(super) fn chip_pointer_up(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.refs.activation.is_pending() {
            return;
        }
        let hit = self.chip_at(position);
        let target = self.refs.activation.release(
            hit.as_ref().map(|hit| &hit.descriptor),
            (f32::from(position.x), f32::from(position.y)),
        );
        if let Some(target) = target {
            cx.emit(TextAreaEvent::ActivateReference(target.attachment));
        }
    }

    /// Whether the pointer cursor over `position` is the pointing hand (an activatable chip).
    pub(crate) fn pointing_hand_at(&self, position: Point<Pixels>) -> bool {
        self.chip_at(position)
            .is_some_and(|hit| crate::mentions::shows_pointing_hand(&hit.descriptor))
    }

    // ---- painting data ----------------------------------------------------------------------------------

    /// The face metrics of the face that shaped a chip's label (R7).
    fn label_metrics(
        &self,
        layout: &AreaLayout,
        chip: &ChipRecord,
        window: &Window,
    ) -> FaceMetrics {
        let text_system = window.text_system();
        let label_start = chip.display.start + chip.info.layout.label.start;
        let row = layout.row_at(label_start, Affinity::Downstream);
        let shaped = &layout.lines[row.line].shape.rows[row.row];
        let font_id = shaped
            .font_id_for_index(label_start.saturating_sub(row.start))
            .unwrap_or_else(|| text_system.resolve_font(&self.resolved_font));
        let size = self.metrics.size;
        let cap = f32::from(text_system.cap_height(font_id, size));
        FaceMetrics {
            ascent: f32::from(text_system.ascent(font_id, size)),
            descent: f32::from(text_system.descent(font_id, size)),
            cap_height: (cap > 0.0).then_some(cap),
        }
    }

    /// The pill of every visible chip row segment (R6, R7), in window coordinates.
    pub fn chip_pills(&self, window: &Window) -> Vec<ChipPill> {
        let (Some(layout), Some(bounds)) = (&self.layout, self.bounds) else {
            return Vec::new();
        };
        let line_height = f32::from(self.metrics.line_height);
        let top = bounds.top() - self.metrics.line_height;
        let bottom = bounds.bottom();
        let scale = window.scale_factor();
        let mut paints = Vec::new();
        for (index, chip) in self.refs.chips.iter().enumerate() {
            let metrics = self.label_metrics(layout, chip, window);
            for segment in layout.range_segments(chip.display.clone()) {
                let rect = self.segment_bounds(bounds, &segment);
                if rect.top() < top || rect.top() > bottom {
                    continue;
                }
                let row = PillRect::new(
                    f32::from(rect.left()),
                    f32::from(rect.top()),
                    f32::from(rect.size.width),
                    line_height,
                );
                let offset = centering_offset(row.y, line_height, metrics, scale);
                paints.push(ChipPill {
                    geometry: pill_geometry(row, offset, segment.starts),
                    chip: index,
                });
            }
        }
        paints
    }

    /// The icon of the chip a pill belongs to.
    pub(super) fn pill_icon(&self, paint: &ChipPill) -> &ChipIcon {
        &self.refs.chips[paint.chip].info.icon
    }

    /// The colors chips are painted with: the ink overlay at 9%, the file-icon well and the
    /// muted text role for glyph icons.
    pub(super) fn chip_colors(&self, cx: &App) -> ChipColors {
        let theme = self.resolved_theme(cx);
        let mut ink = theme.text;
        ink.a = crate::mentions::pill::PILL_FILL_OPACITY;
        let well = vespertine_theme::appearance::try_theme_of(cx)
            .map(|active| active.colors.raised.to_hsla())
            .unwrap_or(theme.surface);
        ChipColors {
            ink,
            well,
            glyph: theme.muted,
        }
    }

    /// The font chip padding is shaped in: the owner's, else the UI sans family of the theme.
    pub(super) fn padding_font(&self, cx: &App) -> Font {
        if let Some(font) = &self.refs.padding_font {
            return font.clone();
        }
        match vespertine_theme::appearance::try_theme_of(cx) {
            Some(theme) => vespertine::font(theme.typography.ui_family.clone()),
            None => Font::default(),
        }
    }

    /// The live attachment numbers referenced by chips that are not dangling.
    pub fn live_attachment_numbers(&self) -> BTreeSet<u32> {
        self.refs
            .chips
            .iter()
            .filter_map(|chip| chip.info.descriptor.attachment)
            .collect()
    }
}
