//! Shared image media of a rendered diagram: the parsed vector tree and its raster for one view
//! (CMP-013 R9, R10).
//!
//! Rasterization never resolves external resources: the SVG is parsed with resolvers that refuse
//! every image reference (file paths, URLs, data URLs), no stylesheet or font is fetched (the
//! parser has no such loader), and external references are ignored.

use resvg::tiny_skia;
use std::sync::{Arc, LazyLock};
use vespertine::RenderImage;

/// The largest raster side in pixels.
const MAX_SIDE: f32 = 8192.0;

/// The view a raster is made for: the width of the target column and the display scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Width available to the diagram in logical pixels; a diagram wider than this is scaled
    /// down to fit, a narrower one keeps its natural size.
    pub width: f32,
    /// Display scale factor (device pixels per logical pixel).
    pub scale: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            width: 640.0,
            scale: 1.0,
        }
    }
}

/// The face every text falls back to when the SVG's families are not installed: IBM Plex Sans,
/// bundled with the framework (SIL Open Font License 1.1, `assets/fonts/ibm-plex-sans`).
const FALLBACK_FONT: &[u8] =
    include_bytes!("../../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");
const FALLBACK_FAMILY: &str = "IBM Plex Sans";

static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = usvg::fontdb::Database::new();
    db.load_system_fonts();
    db.load_font_data(FALLBACK_FONT.to_vec());
    for set in [
        usvg::fontdb::Database::set_sans_serif_family,
        usvg::fontdb::Database::set_serif_family,
        usvg::fontdb::Database::set_cursive_family,
        usvg::fontdb::Database::set_fantasy_family,
    ] {
        set(&mut db, FALLBACK_FAMILY);
    }
    Arc::new(db)
});

fn options() -> usvg::Options<'static> {
    let default_select = usvg::FontResolver::default_font_selector();
    usvg::Options {
        fontdb: FONTS.clone(),
        font_family: FALLBACK_FAMILY.to_string(),
        font_resolver: usvg::FontResolver {
            // A family that is not installed falls back to the bundled face, never to whatever
            // face the database happens to list first (a symbol font would map Latin letters
            // to Greek ones).
            select_font: Box::new(move |font, db| {
                default_select(font, db).or_else(|| {
                    let query = usvg::fontdb::Query {
                        families: &[usvg::fontdb::Family::Name(FALLBACK_FAMILY)],
                        ..Default::default()
                    };
                    db.query(&query)
                })
            }),
            select_fallback: usvg::FontResolver::default_fallback_selector(),
        },
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// A rendered diagram: shared, immutable and cheap to clone.
#[derive(Clone)]
pub struct DiagramMedia {
    tree: Arc<usvg::Tree>,
    image: Arc<RenderImage>,
    view: View,
    raster: (u32, u32),
}

impl std::fmt::Debug for DiagramMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagramMedia")
            .field("view", &self.view)
            .field("raster", &self.raster)
            .finish()
    }
}

impl DiagramMedia {
    /// Parses `svg` and rasterizes it for `view`, with the raster limited to `budget` bytes.
    pub fn from_svg(svg: &str, view: View, budget: usize) -> Result<DiagramMedia, String> {
        let tree = usvg::Tree::from_str(svg, &options()).map_err(|error| error.to_string())?;
        DiagramMedia::rasterize(Arc::new(tree), view, budget)
    }

    /// The same diagram rasterized for another view within `budget` bytes; the receiver keeps
    /// its own raster until the caller releases it.
    pub fn raster_for_view(&self, view: View, budget: usize) -> Result<DiagramMedia, String> {
        DiagramMedia::rasterize(self.tree.clone(), view, budget)
    }

    fn rasterize(tree: Arc<usvg::Tree>, view: View, budget: usize) -> Result<DiagramMedia, String> {
        let natural = tree.size();
        let (natural_w, natural_h) = (natural.width(), natural.height());
        if !(natural_w > 0.0 && natural_h > 0.0) {
            return Err("Diagram has no size".to_string());
        }
        let fit = (view.width / natural_w).clamp(f32::MIN_POSITIVE, 1.0);
        let mut width = natural_w * fit * view.scale.max(0.1);
        let mut height = natural_h * fit * view.scale.max(0.1);
        let limit = (MAX_SIDE / width).min(MAX_SIDE / height).min(1.0);
        width *= limit;
        height *= limit;
        let bytes = width * height * 4.0;
        if bytes > budget as f32 {
            let shrink = (budget as f32 / bytes).sqrt();
            width *= shrink;
            height *= shrink;
        }
        let (w, h) = (
            (width.floor() as u32).max(1),
            (height.floor() as u32).max(1),
        );
        if (w as usize) * (h as usize) * 4 > budget.max(4) {
            return Err("Diagram raster does not fit the retained budget".to_string());
        }
        let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("Diagram raster is empty")?;
        let transform =
            tiny_skia::Transform::from_scale(w as f32 / natural_w, h as f32 / natural_h);
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        // Straight-alpha BGRA, the layout of shared image media.
        let mut bgra = Vec::with_capacity(pixmap.data().len());
        for pixel in pixmap.pixels() {
            let color = pixel.demultiply();
            bgra.extend_from_slice(&[color.blue(), color.green(), color.red(), color.alpha()]);
        }
        let image = RenderImage::from_bgra(w, h, bgra).ok_or("Diagram raster is empty")?;
        Ok(DiagramMedia {
            tree,
            image: Arc::new(image),
            view,
            raster: (w, h),
        })
    }

    /// Retained bytes of the raster.
    pub fn bytes(&self) -> usize {
        self.raster.0 as usize * self.raster.1 as usize * 4
    }

    /// The raster as shared image media.
    pub fn image(&self) -> &Arc<RenderImage> {
        &self.image
    }

    /// The view the raster was made for.
    pub fn view(&self) -> View {
        self.view
    }

    /// Raster size in device pixels.
    pub fn raster_size(&self) -> (u32, u32) {
        self.raster
    }

    /// The natural size of the diagram in logical pixels.
    pub fn natural_size(&self) -> (f32, f32) {
        let size = self.tree.size();
        (size.width(), size.height())
    }

    /// The size the diagram is displayed at in logical pixels: its natural size scaled down to
    /// the view width when wider.
    pub fn display_size(&self) -> (f32, f32) {
        let (w, h) = self.natural_size();
        let fit = (self.view.width / w).min(1.0);
        (w * fit, h * fit)
    }
}
