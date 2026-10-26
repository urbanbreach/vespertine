//! `RichText`: shaped text that registers itself with the text service while it paints, paints its
//! selection wash and (through the highlight context) search washes between the run backgrounds and
//! the glyphs, and carries exact link hitboxes (TXT-006, TXT-008, TXT-009).

use crate::display_map::OriginalText;
use crate::geometry::RichLayout;
use crate::highlight::{Wash, group_id, log_entry};
use crate::link_layer::{LinkLayer, LinkOptions};
use crate::links::Link;
use crate::registry::{PaintedText, RegisteredRun, RunKey};
use crate::service::{SELECTION_ALPHA, TextService};
use crate::theme::TextTheme;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, StyledText, TextRun, Window, fill, px,
};

/// A rounded wash under the glyphs of a byte range of the displayed text. Each wrapped row of
/// the range gets one rectangle, expanded by `expand_x` horizontally and inset by `inset_y`
/// vertically.
#[derive(Clone, Debug)]
pub struct UnderWash {
    /// Byte range in the displayed text.
    pub range: std::ops::Range<usize>,
    /// Fill color.
    pub color: vespertine::Hsla,
    /// Corner radius.
    pub radius: Pixels,
    /// Horizontal expansion of each rectangle.
    pub expand_x: Pixels,
    /// Vertical inset of each rectangle.
    pub inset_y: Pixels,
}

/// A callback run at the end of a run's paint with the geometry of the shaped text.
type AfterPaint = Box<dyn FnOnce(&RichLayout, &mut Window, &mut App)>;

/// A shaped text run of a document. Construct with [`RichText::new`].
pub struct RichText {
    id: ElementId,
    key: Option<RunKey>,
    sub: u32,
    original: OriginalText,
    runs: Option<Vec<TextRun>>,
    group: Option<RunKey>,
    chrome: bool,
    under_washes: Vec<UnderWash>,
    native: bool,
    links: Vec<Link>,
    link_options: LinkOptions,
    styled: Option<StyledText>,
    after_paint: Option<AfterPaint>,
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
            under_washes: Vec::new(),
            native: false,
            links: Vec::new(),
            link_options: LinkOptions::default(),
            styled: None,
            after_paint: None,
        }
    }

    /// Runs `paint` at the end of this run's paint (after the glyphs) with the run's geometry:
    /// the hook for overlays that paint over reserved slots, such as formulas.
    pub fn after_paint(
        mut self,
        paint: impl FnOnce(&RichLayout, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.after_paint = Some(Box::new(paint));
        self
    }

    /// Run list covering the displayed text exactly (otherwise the inherited text style is used).
    /// Runs without letter spacing of their own take the inherited text style's.
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

    /// Rounded washes painted under the glyphs of byte ranges of the displayed text, below the
    /// search and selection washes (word-level change washes of diff lines).
    pub fn under_washes(mut self, washes: Vec<UnderWash>) -> Self {
        self.under_washes = washes;
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
    links: Option<LinkLayer>,
}

/// What painting a run does to the text service, and what it needs back to paint. Painting
/// records it as a paint effect, so that replaying the run's view performs it again (the run
/// registers, logs and numbers its washes exactly as when it paints).
struct RunPaint {
    key: RunKey,
    group_id: u64,
    group: Option<RunKey>,
    original: Arc<OriginalText>,
    layout: Rc<RichLayout>,
    chrome: bool,
    native: bool,
    service: Option<TextService>,
    /// The inherited `user_select` of the text style the run paints under.
    selectable: bool,
    /// The wash color of the run's selection (inherited `selection_color` or the accent).
    selection_color: Hsla,
}

/// The washes a run paints.
struct RunWashes {
    /// Search washes with the rectangles they paint, in paint order.
    highlights: Vec<(Wash, Vec<Bounds<Pixels>>)>,
    /// The selection wash range and color.
    selection: Option<(std::ops::Range<usize>, Hsla)>,
}

impl RunPaint {
    fn apply(&self) -> RunWashes {
        let mut highlights = Vec::new();
        let mut selection = None;
        let text = self.original.displayed();
        if let Some(service) = &self.service {
            if let Some(host) = service.highlight()
                && !host.is_empty()
                && !self.chrome
            {
                // Retained text registered with the host in prepaint; native (generated) text is
                // matched against the exact string it paints.
                let piece = host.piece_of(self.key).filter(|_| !self.native);
                if self.native || piece.is_some() {
                    for wash in host.washes(self.key, self.group_id, piece, text, service.accent())
                    {
                        let rects =
                            self.layout
                                .range_rects(wash.range.clone(), Pixels::ZERO, Pixels::ZERO);
                        service.log_highlight_now(log_entry(self.key, text, &wash, &rects));
                        highlights.push((wash, rects));
                    }
                }
            }
            service.log_text_now(PaintedText {
                key: self.key,
                text: text.to_string(),
                chrome: self.chrome,
            });
            if !self.chrome && self.selectable {
                service.register_run_now(RegisteredRun {
                    key: self.key,
                    surface: service.surface(),
                    text: self.original.clone(),
                    geometry: self.layout.clone(),
                    group: self.group,
                });
                selection = service
                    .selection_wash(self.key)
                    .map(|range| (range, self.selection_color));
            }
        }
        RunWashes {
            highlights,
            selection,
        }
    }
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

    /// A run is a label holding its displayed text (A11Y-003).
    fn a11y_role(&self) -> Option<vespertine::Role> {
        Some(vespertine::Role::Label)
    }

    fn write_a11y_info(&self, node: &mut vespertine::accesskit::Node) {
        node.set_value(self.original.displayed());
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut styled = StyledText::new(SharedString::from(self.original.displayed().to_string()));
        if let Some(mut runs) = self.runs.take() {
            // Runs carry fonts and colors; letter spacing belongs to the inherited text style
            // (TXT-010), which is live here, so the caret, selection, search and link geometry
            // derived from the shaped layout follow the spaced glyphs.
            let spacing = window
                .text_style()
                .letter_spacing_in_pixels(window.rem_size());
            if spacing != Pixels::ZERO {
                for run in runs
                    .iter_mut()
                    .filter(|run| run.letter_spacing == Pixels::ZERO)
                {
                    run.letter_spacing = spacing;
                }
            }
            styled = styled.with_runs(runs);
        }
        let (layout_id, ()) = styled.request_layout(global_id, inspector_id, window, cx);
        self.styled = Some(styled);
        let links = (!self.links.is_empty())
            .then(|| global_id.map(|id| LinkLayer::attach(id, window, cx)))
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
        // (generated) text is matched at paint against the exact string it paints, and only
        // notes that string so explicit ranges can number after its matches.
        if !self.chrome {
            let service = TextService::current();
            let text: Arc<str> = Arc::from(self.original.displayed());
            let native = self.native;
            window.prepaint_effect(
                cx,
                Rc::new(move |_, _| {
                    if let Some(host) = service.as_ref().and_then(TextService::highlight)
                        && !host.is_empty()
                    {
                        if native {
                            host.register_native(key, &text);
                        } else {
                            host.register(key, group_id, &text);
                        }
                    }
                }),
            );
        }
        RichPrepaint {
            layout,
            key,
            group_id,
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
        let layout = &prepaint.layout;
        let service = TextService::current();
        // `userSelect` and `selectionColor` are the framework's inherited text style properties
        // (ELM-006); they are read here, where the style stack is live, and recorded with the run.
        let style = window.text_style();
        let run = Rc::new(RunPaint {
            key: prepaint.key,
            group_id: prepaint.group_id,
            group: self.group,
            original: Arc::new(self.original.clone()),
            layout: Rc::new(layout.clone()),
            chrome: self.chrome,
            native: self.native,
            selectable: style.user_select,
            selection_color: style.selection_color.unwrap_or(Hsla {
                a: SELECTION_ALPHA,
                ..service
                    .as_ref()
                    .map_or_else(|| TextTheme::current(cx).accent, TextService::accent)
            }),
            service,
        });
        let RunWashes {
            highlights,
            selection,
        } = run.apply();
        window.record_paint_effect(Rc::new(move |_, _| {
            run.apply();
        }));

        let text_layout = layout.text_layout();
        // Run backgrounds first, then search washes, then the selection wash, then the glyphs.
        text_layout.paint_background(window, cx).ok();
        // Decorative washes sit under the search and selection washes and under the glyphs.
        for under in &self.under_washes {
            let range = under.range.start.min(layout.len())..under.range.end.min(layout.len());
            for rect in layout.range_rects(range, under.expand_x, under.inset_y) {
                window.paint_quad(fill(rect, under.color).corner_radii(under.radius));
            }
        }
        for (wash, rects) in &highlights {
            for rect in rects {
                window.paint_quad(fill(*rect, wash.color).corner_radii(px(wash.radius)));
            }
        }
        if let Some((range, color)) = selection {
            for rect in layout.range_rects(range, Pixels::ZERO, Pixels::ZERO) {
                window.paint_quad(fill(rect, color));
            }
        }
        text_layout.paint_foreground(window, cx).ok();
        if let Some(links) = prepaint.links.as_mut() {
            links.paint(window, cx);
        }
        if let Some(after_paint) = self.after_paint.take() {
            after_paint(layout, window, cx);
        }
    }
}
