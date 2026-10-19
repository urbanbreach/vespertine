//! `RichText`: shaped text that registers itself with the text service while it paints, paints its
//! selection wash and (through the highlight context) search washes between the run backgrounds and
//! the glyphs, and carries exact link hitboxes (TXT-006, TXT-008, TXT-009).

use crate::display_map::OriginalText;
use crate::geometry::RichLayout;
use crate::highlight::{group_id, log_entry};
use crate::link_layer::{LinkLayer, LinkOptions};
use crate::links::Link;
use crate::registry::{PaintedText, RegisteredRun, RunKey};
use crate::service::TextService;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, SharedString, StyledText, TextRun, Window, fill, px,
};

/// A shaped text run of a document. Construct with [`RichText::new`].
pub struct RichText {
    id: ElementId,
    key: Option<RunKey>,
    sub: u32,
    original: OriginalText,
    runs: Option<Vec<TextRun>>,
    group: Option<RunKey>,
    chrome: bool,
    native: bool,
    links: Vec<Link>,
    link_options: LinkOptions,
    styled: Option<StyledText>,
}

impl RichText {
    /// A run showing `text` as written. `id` identifies the run (its key is derived from the
    /// element identity).
    pub fn new(id: impl Into<ElementId>, text: impl Into<Arc<str>>) -> Self {
        Self::with_original(id, OriginalText::plain(text))
    }

    /// A run showing `original.displayed()` for the source `original.source()`: hit testing,
    /// selection and copy resolve to the source through the display-offset map.
    pub fn with_original(id: impl Into<ElementId>, original: OriginalText) -> Self {
        Self {
            id: id.into(),
            key: None,
            sub: 0,
            original,
            runs: None,
            group: None,
            chrome: false,
            native: false,
            links: Vec::new(),
            link_options: LinkOptions::default(),
            styled: None,
        }
    }

    /// Run list covering the displayed text exactly (otherwise the inherited text style is used).
    pub fn runs(mut self, runs: Vec<TextRun>) -> Self {
        self.runs = Some(runs);
        self
    }

    /// Sub-index within the owner: a code view registers one run per line.
    pub fn sub_index(mut self, sub: u32) -> Self {
        self.sub = sub;
        self
    }

    /// Group identity: pieces authored as adjacent children of one host share the key of the
    /// first piece so that they copy as one line (R10).
    pub fn group(mut self, group: RunKey) -> Self {
        self.group = Some(group);
        self
    }

    /// Paints through the chrome path (R11): logged for inspection, never registered, so it can
    /// never be selected or copied (line-number gutters, headers, list markers).
    pub fn chrome(mut self) -> Self {
        self.chrome = true;
        self
    }

    /// Marks text generated while a native widget paints (code, Markdown, diff lines): search
    /// washes match the exact string it paints (TXT-009 R8).
    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    /// Links of this run: displayed byte ranges with their complete destinations.
    pub fn links(mut self, links: Vec<Link>) -> Self {
        self.links = links;
        self
    }

    /// Link behavior: activation callback and scheme policy.
    pub fn link_options(mut self, options: LinkOptions) -> Self {
        self.link_options = options;
        self
    }

    /// Registers under an explicit key instead of one derived from the element identity, so a
    /// host can address the run (programmatic selection, inspection).
    pub fn key(mut self, key: RunKey) -> Self {
        self.key = Some(key);
        self
    }
}

impl IntoElement for RichText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What a run keeps between layout, prepaint and paint.
pub struct RichPrepaint {
    layout: RichLayout,
    key: RunKey,
    group_id: u64,
    piece: Option<usize>,
    links: Option<LinkLayer>,
}

impl Element for RichText {
    type RequestLayoutState = Option<LinkLayer>;
    type PrepaintState = RichPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut styled = StyledText::new(SharedString::from(self.original.displayed().to_string()));
        if let Some(runs) = self.runs.take() {
            styled = styled.with_runs(runs);
        }
        let (layout_id, ()) = styled.request_layout(id, inspector_id, window, cx);
        self.styled = Some(styled);
        let links = (!self.links.is_empty())
            .then(|| id.map(|id| LinkLayer::attach(id, window, cx)))
            .flatten();
        (layout_id, links)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let styled = self
            .styled
            .as_mut()
            .expect("request_layout runs before prepaint");
        styled.prepaint(id, inspector_id, bounds, &mut (), window, cx);
        let layout = RichLayout::new(styled.layout().clone());
        let key = self.key.unwrap_or_else(|| RunKey::new(id, self.sub));
        let links = request_layout.take().map(|mut layer| {
            layer.prepaint(
                &self.links,
                &self.link_options,
                self.original.clone(),
                &layout,
                window,
                cx,
            );
            layer
        });
        let group_id = group_id(self.group, key);
        // Retained text registers with the nearest declaring ancestor's highlight; native
        // (generated) text is matched at paint against the exact string it paints.
        let piece = TextService::current()
            .and_then(|service| service.highlight())
            .filter(|host| !self.native && !self.chrome && !host.is_empty())
            .map(|host| host.register(key, group_id, self.original.displayed()));
        RichPrepaint {
            layout,
            key,
            group_id,
            piece,
            links,
        }
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
        let service = TextService::current();
        let key = prepaint.key;
        let layout = &prepaint.layout;
        let mut wash = None;
        let mut washes = Vec::new();
        if let Some(service) = &service {
            if let Some(host) = service.highlight()
                && !host.is_empty()
                && !self.chrome
                && (self.native || prepaint.piece.is_some())
            {
                washes = host.washes(
                    key,
                    prepaint.group_id,
                    prepaint.piece,
                    self.original.displayed(),
                    service.accent(),
                );
            }
            service.log_text(PaintedText {
                key,
                text: self.original.displayed().to_string(),
                chrome: self.chrome,
            });
            if !self.chrome && service.selectable_now() {
                service.register_run(RegisteredRun {
                    key,
                    surface: service.surface(),
                    text: Arc::new(self.original.clone()),
                    geometry: Rc::new(layout.clone()),
                    group: self.group,
                });
                wash = service
                    .selection_wash(key)
                    .map(|range| (range, service.selection_color()));
            }
        }

        let text_layout = layout.text_layout();
        // Run backgrounds first, then search washes, then the selection wash, then the glyphs.
        text_layout.paint_background(window, cx).ok();
        for highlight_wash in &washes {
            let rects =
                layout.range_rects(highlight_wash.range.clone(), Pixels::ZERO, Pixels::ZERO);
            for rect in &rects {
                window.paint_quad(
                    fill(*rect, highlight_wash.color).corner_radii(px(highlight_wash.radius)),
                );
            }
            if let Some(service) = &service {
                service.log_highlight(log_entry(
                    key,
                    self.original.displayed(),
                    highlight_wash,
                    &rects,
                ));
            }
        }
        if let Some((range, color)) = wash {
            for rect in layout.range_rects(range, Pixels::ZERO, Pixels::ZERO) {
                window.paint_quad(fill(rect, color));
            }
        }
        text_layout.paint_foreground(window, cx).ok();
        if let Some(links) = prepaint.links.as_mut() {
            links.paint(window, cx);
        }
    }
}
