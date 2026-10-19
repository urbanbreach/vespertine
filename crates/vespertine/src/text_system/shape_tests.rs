//! Tests for font resolution with fallbacks (TXT-002), the recent-shape and resolved-font caches
//! (PRF-012) and numeric shaping (PRF-013), driven by a scriptable platform text system.

use super::*;
use crate::{PlatformTextSystem, TextRenderingMode, point, size};
use anyhow::bail;
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, AtomicUsize},
};

#[derive(Clone, Copy)]
struct FamilySpec {
    italic: bool,
    bold: bool,
    tabular: bool,
    /// Rejects any request that is not plain (weight 400, upright, no features).
    reject_styled: bool,
}

impl FamilySpec {
    const FULL: Self = Self {
        italic: true,
        bold: true,
        tabular: true,
        reject_styled: false,
    };
}

#[derive(Clone, Debug, PartialEq)]
struct Face {
    family: String,
    weight: f32,
    italic: bool,
    tabular: bool,
}

type Hook = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Scripted {
    families: HashMap<String, FamilySpec>,
    faces: Vec<Face>,
}

#[derive(Default)]
struct TestShaper {
    state: Mutex<Scripted>,
    font_id_calls: AtomicUsize,
    layout_calls: AtomicUsize,
    fail_add_fonts: AtomicBool,
    kerning: Mutex<HashMap<(char, char), f32>>,
    ligatures: Mutex<HashSet<(char, char)>>,
    /// Shifts the last glyph of any line holding three consecutive 9s: a context the pair test
    /// cannot see.
    skew_triple_nines: AtomicBool,
    on_layout: Mutex<Option<Hook>>,
}

impl TestShaper {
    fn with_families(families: &[(&str, FamilySpec)]) -> Arc<Self> {
        let shaper = Self::default();
        for (name, spec) in families {
            shaper
                .state
                .lock()
                .families
                .insert((*name).to_string(), *spec);
        }
        Arc::new(shaper)
    }

    fn face(&self, id: FontId) -> Face {
        self.state.lock().faces[id.0].clone()
    }

    fn char_advance(face: &Face, c: char, size: f32) -> f32 {
        match c {
            '0'..='9' if face.tabular => size * 0.6,
            '1' => size * 0.3,
            '0'..='9' => size * 0.6,
            '.' | ',' => size * 0.25,
            _ => size * 0.5,
        }
    }
}

impl PlatformTextSystem for TestShaper {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        if self.fail_add_fonts.load(Ordering::SeqCst) {
            bail!("installation failed");
        }
        let mut state = self.state.lock();
        for bytes in fonts {
            state.families.insert(
                String::from_utf8_lossy(&bytes).into_owned(),
                FamilySpec::FULL,
            );
        }
        Ok(())
    }

    fn all_font_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.state.lock().families.keys().cloned().collect();
        names.sort();
        names
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        self.font_id_calls.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock();
        let Some(spec) = state.families.get(descriptor.family.as_str()).copied() else {
            bail!("no family {}", descriptor.family);
        };
        let tabular = descriptor
            .features
            .tag_value_list()
            .iter()
            .any(|(tag, value)| tag == "tnum" && *value == 1);
        if spec.reject_styled
            && (descriptor.weight != FontWeight::NORMAL
                || descriptor.style != FontStyle::Normal
                || !descriptor.features.tag_value_list().is_empty())
        {
            bail!("styled request rejected for {}", descriptor.family);
        }
        let face = Face {
            family: descriptor.family.to_string(),
            weight: if descriptor.weight.0 >= 600. && spec.bold {
                700.
            } else {
                400.
            },
            italic: descriptor.style != FontStyle::Normal && spec.italic,
            tabular: tabular && spec.tabular,
        };
        let ix = state
            .faces
            .iter()
            .position(|existing| *existing == face)
            .unwrap_or_else(|| {
                state.faces.push(face);
                state.faces.len() - 1
            });
        Ok(FontId(ix))
    }

    fn font_metrics(&self, _font_id: FontId) -> FontMetrics {
        FontMetrics {
            units_per_em: 1000,
            ascent: 800.,
            descent: 200.,
            line_gap: 0.,
            underline_position: -100.,
            underline_thickness: 50.,
            cap_height: 700.,
            x_height: 500.,
            bounding_box: Bounds {
                origin: point(0., -200.),
                size: size(1000., 1000.),
            },
        }
    }

    fn typographic_bounds(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Bounds<f32>> {
        Ok(Bounds::default())
    }

    fn advance(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Size<f32>> {
        Ok(size(500., 0.))
    }

    fn glyph_for_char(&self, _font_id: FontId, ch: char) -> Option<GlyphId> {
        Some(GlyphId(ch as u32))
    }

    fn glyph_raster_bounds(&self, _params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        Ok(Bounds::default())
    }

    fn rasterize_glyph(
        &self,
        _params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        Ok((raster_bounds.size, Vec::new()))
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.layout_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = &*self.on_layout.lock() {
            hook();
        }
        let size = font_size.0;
        let kerning = self.kerning.lock().clone();
        let ligatures = self.ligatures.lock().clone();
        let mut x = 0.0f32;
        let mut shaped_runs = Vec::new();
        let mut start = 0;
        let mut previous: Option<char> = None;
        for run in runs {
            let face = self.face(run.font_id);
            let mut glyphs = Vec::new();
            let slice = &text[start..start + run.len];
            let mut chars = slice.char_indices().peekable();
            while let Some((ix, c)) = chars.next() {
                if let Some(before) = previous
                    && let Some(delta) = kerning.get(&(before, c))
                {
                    x += *delta * size;
                }
                if let Some(&(_, next)) = chars.peek()
                    && ligatures.contains(&(c, next))
                {
                    glyphs.push(ShapedGlyph {
                        id: GlyphId(0xF000 + c as u32),
                        position: point(px(x), px(0.)),
                        index: start + ix,
                        is_emoji: false,
                    });
                    x += Self::char_advance(&face, c, size) + Self::char_advance(&face, next, size);
                    chars.next();
                    previous = Some(next);
                    continue;
                }
                glyphs.push(ShapedGlyph {
                    id: GlyphId(c as u32),
                    position: point(px(x), px(0.)),
                    index: start + ix,
                    is_emoji: false,
                });
                x += Self::char_advance(&face, c, size);
                previous = Some(c);
            }
            start += run.len;
            if !glyphs.is_empty() {
                shaped_runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs,
                });
            }
        }
        if self.skew_triple_nines.load(Ordering::SeqCst)
            && text.contains("999")
            && let Some(last) = shaped_runs.last_mut().and_then(|run| run.glyphs.last_mut())
        {
            last.position.x += px(0.5);
        }
        LineLayout {
            font_size,
            width: px(x),
            ascent: px(size * 0.8),
            descent: px(size * 0.2),
            runs: shaped_runs,
            len: text.len(),
        }
    }

    fn recommended_rendering_mode(
        &self,
        _font_id: FontId,
        _font_size: Pixels,
    ) -> TextRenderingMode {
        TextRenderingMode::Grayscale
    }
}

fn text_system(shaper: &Arc<TestShaper>) -> Arc<TextSystem> {
    Arc::new(TextSystem::new(shaper.clone()))
}

fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".to_string(), 1)]))
}

fn run_of(text: &str, font: &Font) -> TextRun {
    TextRun {
        len: text.len(),
        font: font.clone(),
        ..Default::default()
    }
}

fn layout(window: &WindowTextSystem, text: &str, font: &Font) -> Arc<LineLayout> {
    window.layout_line(text, px(16.), &[run_of(text, font)], None)
}

mod fallback {
    use super::*;

    fn requested() -> Font {
        Font {
            family: "No Such Family".into(),
            features: tabular(),
            fallbacks: Some(FontFallbacks::from_fonts(vec!["Glyph Backup".into()])),
            weight: FontWeight::BOLD,
            style: FontStyle::Italic,
        }
    }

    #[test]
    fn fallback_keeps_requested_weight_style_and_features() {
        let shaper = TestShaper::with_families(&[("Adwaita Sans", FamilySpec::FULL)]);
        let system = text_system(&shaper);
        let id = system.resolve_font(&requested());
        let face = shaper.face(id);
        assert_eq!(face.family, "Adwaita Sans");
        assert!(face.weight >= 700., "fallback must stay bold");
        assert!(face.italic, "fallback must stay italic");
        assert!(face.tabular, "fallback must keep tabular numerals");

        // Equal-length digit strings keep equal width after fallback.
        let window = WindowTextSystem::new(system.clone());
        let narrow = layout(&window, "1111", &requested());
        let wide = layout(&window, "0000", &requested());
        assert_eq!(narrow.width, wide.width);

        // A plain fallback would have lost all three attributes.
        let plain = shaper.face(system.resolve_font(&font("Adwaita Sans")));
        assert!(plain.weight < 700. && !plain.italic && !plain.tabular);
    }

    #[test]
    fn fallback_carries_the_per_glyph_fallback_list_unchanged() {
        let shaper = TestShaper::with_families(&[("Adwaita Sans", FamilySpec::FULL)]);
        let system = text_system(&shaper);
        let seen = Arc::new(Mutex::new(Vec::<Option<FontFallbacks>>::new()));
        struct Recording(Arc<TestShaper>, Arc<Mutex<Vec<Option<FontFallbacks>>>>);
        impl PlatformTextSystem for Recording {
            fn font_id(&self, descriptor: &Font) -> Result<FontId> {
                self.1.lock().push(descriptor.fallbacks.clone());
                self.0.font_id(descriptor)
            }
            fn font_metrics(&self, id: FontId) -> FontMetrics {
                self.0.font_metrics(id)
            }
            fn advance(&self, font: FontId, glyph: GlyphId) -> Result<Size<f32>> {
                self.0.advance(font, glyph)
            }
            fn typographic_bounds(&self, font: FontId, glyph: GlyphId) -> Result<Bounds<f32>> {
                self.0.typographic_bounds(font, glyph)
            }
            fn glyph_for_char(&self, font: FontId, ch: char) -> Option<GlyphId> {
                self.0.glyph_for_char(font, ch)
            }
            fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
                self.0.add_fonts(fonts)
            }
            fn all_font_names(&self) -> Vec<String> {
                self.0.all_font_names()
            }
            fn glyph_raster_bounds(
                &self,
                params: &RenderGlyphParams,
            ) -> Result<Bounds<DevicePixels>> {
                self.0.glyph_raster_bounds(params)
            }
            fn rasterize_glyph(
                &self,
                params: &RenderGlyphParams,
                bounds: Bounds<DevicePixels>,
            ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
                self.0.rasterize_glyph(params, bounds)
            }
            fn layout_line(&self, text: &str, size: Pixels, runs: &[FontRun]) -> LineLayout {
                self.0.layout_line(text, size, runs)
            }
            fn recommended_rendering_mode(&self, font: FontId, size: Pixels) -> TextRenderingMode {
                self.0.recommended_rendering_mode(font, size)
            }
        }
        drop(system);
        let system = Arc::new(TextSystem::new(Arc::new(Recording(shaper, seen.clone()))));
        system.resolve_font(&requested());
        let seen = seen.lock();
        // The styled request to the fallback family carries the original list.
        assert!(seen.iter().any(|fallbacks| {
            fallbacks.as_ref() == requested().fallbacks.as_ref() && fallbacks.is_some()
        }));
    }

    #[test]
    fn family_without_italic_or_bold_uses_nearest_face_and_still_resolves() {
        let shaper = TestShaper::with_families(&[(
            "Adwaita Sans",
            FamilySpec {
                italic: false,
                bold: false,
                tabular: true,
                reject_styled: false,
            },
        )]);
        let system = text_system(&shaper);
        let face = shaper.face(system.resolve_font(&requested()));
        assert_eq!(face.family, "Adwaita Sans");
        assert!(!face.italic);
        assert!(face.weight < 700.);
        assert!(face.tabular);
    }

    #[test]
    fn plain_request_is_used_when_the_styled_request_fails() {
        let shaper = TestShaper::with_families(&[(
            "Helvetica",
            FamilySpec {
                reject_styled: true,
                ..FamilySpec::FULL
            },
        )]);
        let system = text_system(&shaper);
        let face = shaper.face(system.resolve_font(&requested()));
        assert_eq!(face.family, "Helvetica");
        assert!(!face.italic && face.weight < 700. && !face.tabular);
    }

    #[test]
    #[should_panic(expected = "No Such Family")]
    fn total_failure_is_a_hard_failure_naming_the_request() {
        let shaper = TestShaper::with_families(&[]);
        text_system(&shaper).resolve_font(&requested());
    }

    #[test]
    fn resolution_is_cached_by_the_full_request_and_invalidated_by_font_additions() {
        let shaper = TestShaper::with_families(&[("Adwaita Sans", FamilySpec::FULL)]);
        let system = text_system(&shaper);
        let bold = requested();
        let regular = Font {
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
            ..requested()
        };
        let bold_id = system.resolve_font(&bold);
        let regular_id = system.resolve_font(&regular);
        assert_ne!(bold_id, regular_id, "requests must not share a result");

        let calls = shaper.font_id_calls.load(Ordering::SeqCst);
        assert_eq!(system.resolve_font(&bold), bold_id);
        assert_eq!(system.resolve_font(&regular), regular_id);
        assert_eq!(
            shaper.font_id_calls.load(Ordering::SeqCst),
            calls,
            "repeated resolution must not reach the platform"
        );

        system
            .add_fonts(vec![Cow::Borrowed(b"No Such Family".as_slice())])
            .unwrap();
        let direct = shaper.face(system.resolve_font(&bold));
        assert_eq!(direct.family, "No Such Family");
    }
}

mod recent_shapes {
    use super::*;

    fn setup() -> (Arc<TestShaper>, Arc<TextSystem>, WindowTextSystem, Font) {
        let shaper = TestShaper::with_families(&[("Test Sans", FamilySpec::FULL)]);
        let system = text_system(&shaper);
        let window = WindowTextSystem::new(system.clone());
        (shaper, system, window, font("Test Sans"))
    }

    #[test]
    fn scrolled_out_text_is_not_shaped_again() {
        let (shaper, system, window, font) = setup();
        let first_width = layout(&window, "hello world", &font).width;
        for _ in 0..3 {
            window.finish_frame();
        }
        let again = layout(&window, "hello world", &font);
        assert_eq!(shaper.layout_calls.load(Ordering::SeqCst), 1);
        assert_eq!(again.width, first_width);
        assert_eq!(system.shape_stats().recent_hits, 1);
    }

    #[test]
    fn hits_require_full_key_equality_even_when_hashes_collide() {
        let (shaper, system, window, font) = setup();
        system.shape_caches.force_recent_hash_collisions();
        let narrow = layout(&window, "iiii", &font);
        let wide = layout(&window, "wwww", &font);
        let other_size = window.layout_line("iiii", px(20.), &[run_of("iiii", &font)], None);
        let bold = Font::bold(font.clone());
        let bold_run = layout(&window, "iiii", &bold);
        assert_eq!(shaper.layout_calls.load(Ordering::SeqCst), 4);
        for _ in 0..2 {
            window.finish_frame();
        }
        assert_eq!(layout(&window, "wwww", &font).width, wide.width);
        assert_eq!(layout(&window, "iiii", &font).width, narrow.width);
        let resized = window.layout_line("iiii", px(20.), &[run_of("iiii", &font)], None);
        assert_eq!(resized.width, other_size.width);
        assert_eq!(layout(&window, "iiii", &bold).width, bold_run.width);
        assert_eq!(
            shaper.layout_calls.load(Ordering::SeqCst),
            4,
            "every colliding key must still be answered from the store"
        );
    }

    #[test]
    fn font_addition_clears_the_recent_store() {
        let (shaper, system, window, font) = setup();
        layout(&window, "hello", &font);
        window.finish_frame();
        window.finish_frame();
        system
            .add_fonts(vec![Cow::Borrowed(b"Another".as_slice())])
            .unwrap();
        layout(&window, "hello", &font);
        assert_eq!(shaper.layout_calls.load(Ordering::SeqCst), 2);
        assert_eq!(system.shape_stats().recent_hits, 0);
        assert_eq!(system.shape_caches.recent_unit_counts(), (5, 0));
    }

    #[test]
    fn two_clients_of_one_text_system_share_recent_shapes() {
        let (shaper, system, first, font) = setup();
        let second = WindowTextSystem::new(system);
        layout(&first, "shared text", &font);
        layout(&second, "shared text", &font);
        assert_eq!(shaper.layout_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn recent_cache_is_not_locked_while_the_platform_shapes() {
        let (shaper, system, window, font) = setup();
        let weak = Arc::downgrade(&system);
        *shaper.on_layout.lock() = Some(Box::new(move || {
            let system = weak.upgrade().unwrap();
            // Re-entering both stores from inside the platform call must not deadlock.
            let generation = system.font_generation();
            system
                .shape_caches
                .recent_lookup(generation, "probe", px(16.), &[]);
            let _ = system.shape_stats();
        }));
        layout(&window, "reentrant", &font);
    }

    #[test]
    fn measured_text_survives_rollovers_and_moves_between_owners() {
        let (shaper, _system, window, font) = setup();
        let text = "a wrapped line that is long enough to wrap";
        let held = window
            .shape_text(
                text.into(),
                px(16.),
                &[run_of(text, &font)],
                Some(px(60.)),
                None,
            )
            .unwrap();
        assert!(!held[0].layout.wrap_boundaries.is_empty());
        let shapes = shaper.layout_calls.load(Ordering::SeqCst);

        // Several frames pass without a lookup while a measurement holds the layout.
        for _ in 0..4 {
            window.finish_frame();
        }
        let [_, _, previous_lines, previous_wrapped] =
            window.line_layout_cache.cached_entry_counts();
        assert_eq!((previous_lines, previous_wrapped), (1, 1));

        // Another owner asks for the same text and gets the retained layout.
        let moved = window
            .shape_text(
                text.into(),
                px(16.),
                &[run_of(text, &font)],
                Some(px(60.)),
                None,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&held[0].layout, &moved[0].layout));
        assert_eq!(shaper.layout_calls.load(Ordering::SeqCst), shapes);

        // Releasing the last owner lets the wrapper and then its line leave at the next rollover.
        drop(held);
        drop(moved);
        window.finish_frame();
        window.finish_frame();
        assert_eq!(window.line_layout_cache.cached_entry_counts(), [0, 0, 0, 0]);
    }

    #[test]
    fn replayed_ranges_are_rebased_and_refused_after_font_additions() {
        let (_shaper, system, window, font) = setup();
        layout(&window, "first", &font);
        let start = window.layout_index();
        layout(&window, "second", &font);
        layout(&window, "third", &font);
        let end = window.layout_index();
        window.finish_frame();

        layout(&window, "only", &font);
        layout(&window, "only too", &font);
        let replayed = window.reuse_layouts(start.clone()..end);
        // Two used entries were replayed after the two entries already recorded in this frame.
        assert_eq!(window.line_layout_cache.used_entry_counts()[0], 4);
        assert_ne!(
            replayed.start.lines_position(),
            start.lines_position(),
            "the range must be expressed in destination coordinates"
        );
        assert_eq!(
            replayed.end.lines_position() - replayed.start.lines_position(),
            2
        );

        window.finish_frame();
        system
            .add_fonts(vec![Cow::Borrowed(b"Another".as_slice())])
            .unwrap();
        let refused = window.reuse_layouts(replayed);
        assert_eq!(refused.start.lines_position(), refused.end.lines_position());
    }
}

mod resolved_font_cache {
    use super::*;

    #[test]
    fn cache_is_isolated_per_text_system_and_releases_dead_systems() {
        resolved_fonts::clear();
        let first = TestShaper::with_families(&[("Shared", FamilySpec::FULL)]);
        // The second system has a different face table, so the same request yields another id.
        let second =
            TestShaper::with_families(&[("Other", FamilySpec::FULL), ("Shared", FamilySpec::FULL)]);
        second.font_id(&font("Other")).unwrap();
        let (system_a, system_b) = (text_system(&first), text_system(&second));

        let request = font("Shared");
        let id_a = system_a.resolve_font(&request);
        let id_b = system_b.resolve_font(&request);
        assert_ne!(
            id_a, id_b,
            "the two systems resolve the same request differently"
        );
        for _ in 0..3 {
            assert_eq!(system_a.resolve_font(&request), id_a);
            assert_eq!(system_b.resolve_font(&request), id_b);
        }
        assert_eq!(resolved_fonts::counts(), (2, 2));

        drop(system_a);
        drop(first);
        assert_eq!(
            resolved_fonts::counts(),
            (2, 1),
            "the dead system's entry is released lazily"
        );
        // The next insertion purges it, and a new system is never confused with the dead one.
        let third =
            TestShaper::with_families(&[("Shared", FamilySpec::FULL), ("Third", FamilySpec::FULL)]);
        third.font_id(&font("Third")).unwrap();
        let system_c = text_system(&third);
        let id_c = system_c.resolve_font(&request);
        assert_eq!(shaper_family(&third, id_c), "Shared");
        assert_eq!(resolved_fonts::counts(), (2, 2));
        assert_eq!(system_b.resolve_font(&request), id_b);
    }

    fn shaper_family(shaper: &TestShaper, id: FontId) -> String {
        shaper.face(id).family
    }

    #[test]
    fn cache_holds_at_most_sixteen_fonts_and_drops_the_oldest_inserted() {
        resolved_fonts::clear();
        let families: Vec<String> = (0..20).map(|ix| format!("Family {ix}")).collect();
        let shaper = TestShaper::with_families(
            &families
                .iter()
                .map(|name| (name.as_str(), FamilySpec::FULL))
                .collect::<Vec<_>>(),
        );
        let system = text_system(&shaper);
        for name in &families {
            system.resolve_font(&font(name.clone()));
        }
        assert_eq!(resolved_fonts::counts().0, 16);
        let calls = shaper.font_id_calls.load(Ordering::SeqCst);
        // The newest entries are cached: no platform request is needed.
        system.resolve_font(&font(families[19].clone()));
        assert_eq!(shaper.font_id_calls.load(Ordering::SeqCst), calls);
    }

    #[test]
    fn font_addition_invalidates_thread_cache_entries() {
        resolved_fonts::clear();
        let shaper = TestShaper::with_families(&[("Adwaita Sans", FamilySpec::FULL)]);
        let system = text_system(&shaper);
        let request = font("Late Family");
        let fallback = system.resolve_font(&request);
        assert_eq!(shaper.face(fallback).family, "Adwaita Sans");
        system
            .add_fonts(vec![Cow::Borrowed(b"Late Family".as_slice())])
            .unwrap();
        let direct = system.resolve_font(&request);
        assert_eq!(shaper.face(direct).family, "Late Family");
    }
}

mod numeric {
    use super::*;
    use crate::text_system::shape_cache::{
        NUMERIC_CHECK_INTERVAL, NUMERIC_INITIAL_CHECKS, layouts_agree,
    };

    struct Fixture {
        shaper: Arc<TestShaper>,
        system: Arc<TextSystem>,
        window: WindowTextSystem,
        font: Font,
    }

    fn fixture() -> Fixture {
        let shaper = TestShaper::with_families(&[("Test Sans", FamilySpec::FULL)]);
        shaper.kerning.lock().insert(('1', '7'), -0.05);
        shaper.kerning.lock().insert(('.', '5'), 0.02);
        let system = text_system(&shaper);
        let window = WindowTextSystem::new(system.clone());
        Fixture {
            shaper,
            system,
            window,
            font: font("Test Sans"),
        }
    }

    impl Fixture {
        /// Lays `text` out through the cache and shapes it directly on the platform.
        fn both(&self, text: &str) -> (Arc<LineLayout>, LineLayout) {
            let font_id = self.system.resolve_font(&self.font);
            let platform = self.shaper.layout_line(
                text,
                px(16.),
                &[FontRun {
                    len: text.len(),
                    font_id,
                }],
            );
            (layout(&self.window, text, &self.font), platform)
        }
    }

    fn number(ix: usize) -> String {
        format!("{}.{:02}", ix * 37 % 100000, ix % 100)
    }

    #[test]
    fn synthesized_lines_match_platform_shaping_in_every_phase() {
        let f = fixture();
        for ix in 0..600 {
            let text = number(ix * 7919 + 13);
            let (cached, platform) = f.both(&text);
            assert!(
                layouts_agree(&cached, &platform),
                "line {text} differs from platform shaping"
            );
        }
        let stats = f.system.shape_stats();
        assert!(stats.synthesized_lines > 500, "{stats:?}");
        assert_eq!(stats.disabled_font_sizes, 0);
    }

    #[test]
    fn ineligible_text_is_shaped_by_the_platform() {
        let f = fixture();
        let eligible_64 = "1".repeat(64);
        let cases: Vec<(String, bool)> = vec![
            (String::new(), false),
            ("1".repeat(65), false),
            ("12\u{e9}4".into(), false),
            ("12a4".into(), false),
            ("12:34".into(), false),
            ("12x34".into(), false),
            (eligible_64, true),
            ("-1,234.5%".into(), true),
        ];
        for (text, eligible) in cases {
            let before = f.system.shape_stats();
            if text.is_empty() {
                continue;
            }
            // Warm the line so characters and pairs are known, then compare counts.
            layout(&f.window, &text, &f.font);
            f.window.finish_frame();
            f.window.finish_frame();
            f.system
                .shape_caches
                .recent_lookup(usize::MAX, "", px(0.), &[]);
            let after = f.system.shape_stats();
            assert_eq!(
                after.synthesized_lines > before.synthesized_lines,
                eligible,
                "{text:?}"
            );
        }
        // Multi-font-run input is never synthesized.
        let first = "12";
        let second = "34";
        let bold = Font::bold(f.font.clone());
        let before = f.system.shape_stats().synthesized_lines;
        f.window.layout_line(
            "1234",
            px(16.),
            &[run_of(first, &f.font), run_of(second, &bold)],
            None,
        );
        assert_eq!(f.system.shape_stats().synthesized_lines, before);
    }

    #[test]
    fn ligatures_among_the_alphabet_fall_back_immediately() {
        let f = fixture();
        f.shaper.ligatures.lock().insert(('1', '2'));
        for _ in 0..3 {
            let (cached, platform) = f.both("0120");
            assert!(layouts_agree(&cached, &platform));
            f.window.finish_frame();
            f.window.finish_frame();
        }
        assert_eq!(f.system.shape_stats().synthesized_lines, 0);
        // Lines avoiding the ligature pair are still synthesized.
        layout(&f.window, "0340", &f.font);
        layout(&f.window, "0350", &f.font);
        assert!(f.system.shape_stats().synthesized_lines >= 1);
    }

    #[test]
    fn an_injected_mismatch_returns_the_platform_line_and_disables_that_font_and_size() {
        let f = fixture();
        f.shaper.skew_triple_nines.store(true, Ordering::SeqCst);
        // Learn characters and pairs on lines that do not trigger the skew.
        layout(&f.window, "9.9", &f.font);
        let skewed = "999";
        let (cached, platform) = f.both(skewed);
        assert!(
            layouts_agree(&cached, &platform),
            "the platform line is returned on mismatch"
        );
        let font_id = f.system.resolve_font(&f.font);
        assert!(f.system.shape_caches.numeric_is_disabled(font_id, px(16.)));
        assert_eq!(f.system.shape_stats().disabled_font_sizes, 1);

        // Later lines of the disabled font and size are never synthesized...
        let synthesized = f.system.shape_stats().synthesized_lines;
        layout(&f.window, "1.25", &f.font);
        layout(&f.window, "7.50", &f.font);
        assert_eq!(f.system.shape_stats().synthesized_lines, synthesized);

        // ...while another size and another font are unaffected.
        for text in ["1.25", "3.50", "7.75"] {
            f.window
                .layout_line(text, px(20.), &[run_of(text, &f.font)], None);
        }
        assert!(f.system.shape_stats().synthesized_lines > synthesized);
        assert!(!f.system.shape_caches.numeric_is_disabled(font_id, px(20.)));
    }

    #[test]
    fn font_addition_discards_learned_state_and_restarts_the_initial_checks() {
        let f = fixture();
        for ix in 0..(NUMERIC_INITIAL_CHECKS as usize + 4) {
            layout(&f.window, &number(ix * 31 + 5), &f.font);
        }
        let checks = f.system.shape_stats().validation_checks;
        assert_eq!(checks, NUMERIC_INITIAL_CHECKS);
        assert!(f.system.shape_caches.numeric_has_state());

        f.system
            .add_fonts(vec![Cow::Borrowed(b"Another".as_slice())])
            .unwrap();
        layout(&f.window, "1.25", &f.font);
        // Learning restarted: the first synthesized line after the addition is checked again.
        assert_eq!(f.system.shape_stats().validation_checks, checks + 1);
    }

    #[test]
    fn platform_line_shapes_drop_to_about_one_in_256_after_learning() {
        let f = fixture();
        let warm = NUMERIC_INITIAL_CHECKS as usize + 20;
        for ix in 0..warm {
            layout(&f.window, &number(ix * 131 + 1), &f.font);
        }
        let before = f.system.shape_stats();
        let lines = 2048usize;
        for ix in 0..lines {
            layout(&f.window, &number(ix * 7 + 100_000), &f.font);
        }
        let after = f.system.shape_stats();
        let platform = after.platform_line_shapes - before.platform_line_shapes;
        let expected = lines as u64 / NUMERIC_CHECK_INTERVAL;
        assert!(
            platform >= expected && platform <= expected + 2,
            "{platform} platform shapes for {lines} lines"
        );
    }

    #[test]
    fn disabling_numeric_shaping_uses_the_platform_for_every_unseen_line() {
        let f = fixture();
        f.system.set_numeric_shaping_enabled(false);
        for ix in 0..50 {
            layout(&f.window, &number(ix + 3), &f.font);
        }
        let stats = f.system.shape_stats();
        assert_eq!(stats.synthesized_lines, 0);
        assert_eq!(stats.platform_line_shapes, 50);
    }
}

mod appended {
    use super::*;
    use crate::text_system::shape_cache::{
        APPEND_CHECK_INTERVAL, APPEND_INITIAL_CHECKS, APPEND_MAX_CHAIN, APPEND_MIN_LEN,
        layouts_agree_within,
    };

    struct Fixture {
        shaper: Arc<TestShaper>,
        system: Arc<TextSystem>,
        window: WindowTextSystem,
        font: Font,
    }

    fn fixture() -> Fixture {
        let shaper = TestShaper::with_families(&[("Test Sans", FamilySpec::FULL)]);
        let system = text_system(&shaper);
        let window = WindowTextSystem::new(system.clone());
        Fixture {
            shaper,
            system,
            window,
            font: font("Test Sans"),
        }
    }

    impl Fixture {
        fn platform(&self, text: &str) -> LineLayout {
            let font_id = self.system.resolve_font(&self.font);
            self.shaper.layout_line(
                text,
                px(16.),
                &[FontRun {
                    len: text.len(),
                    font_id,
                }],
            )
        }

        /// Lays `text` out through the cache and checks it against full platform shaping.
        fn checked(&self, text: &str) -> Arc<LineLayout> {
            let cached = layout(&self.window, text, &self.font);
            let platform = self.platform(text);
            let tolerance = 0.001f32.max(platform.width.0 * 8.0 * f32::EPSILON);
            assert!(
                layouts_agree_within(&cached, &platform, tolerance),
                "`{text}` differs from platform shaping"
            );
            cached
        }
    }

    fn stream(words: usize) -> String {
        let mut text = String::from("a streaming message begins with some ordinary words");
        for ix in 0..words {
            text.push_str(&format!(" w{ix}"));
        }
        text
    }

    /// Lines of the growing stream that are assembled rather than shaped in full: every
    /// `APPEND_MAX_CHAIN + 1`-th line anchors a new chain.
    fn assembled_of(steps: usize) -> u64 {
        (steps - steps / (APPEND_MAX_CHAIN as usize + 1)) as u64
    }

    #[test]
    fn a_growing_line_shapes_only_its_new_words() {
        let f = fixture();
        assert!(stream(0).len() >= APPEND_MIN_LEN);
        f.checked(&stream(0));
        let steps = APPEND_INITIAL_CHECKS as usize + 12;
        for words in 1..=steps {
            f.checked(&stream(words));
        }
        let stats = f.system.shape_stats();
        assert_eq!(stats.appended_lines, assembled_of(steps));
        assert_eq!(stats.append_checks, APPEND_INITIAL_CHECKS);
        assert!(!stats.append_disabled);

        // The next line extends a chain: only the words after the last shared boundary reach
        // the platform (the direct comparison below is not counted by the cache).
        let next = steps + 1;
        assert_ne!(next % (APPEND_MAX_CHAIN as usize + 1), 0);
        let before = f.system.shape_stats().platform_line_shapes;
        f.checked(&stream(next));
        assert_eq!(f.system.shape_stats().platform_line_shapes - before, 1);
    }

    #[test]
    fn a_chain_ends_with_a_full_shape_that_anchors_the_next() {
        let f = fixture();
        f.checked(&stream(0));
        let chain = APPEND_MAX_CHAIN as usize;
        for words in 1..=chain {
            f.checked(&stream(words));
        }
        assert_eq!(f.system.shape_stats().appended_lines, chain as u64);
        // The chain is exhausted: the next line is shaped in full and starts another chain.
        f.checked(&stream(chain + 1));
        assert_eq!(f.system.shape_stats().appended_lines, chain as u64);
        f.checked(&stream(chain + 2));
        assert_eq!(f.system.shape_stats().appended_lines, chain as u64 + 1);
    }

    #[test]
    fn lines_that_do_not_extend_a_tracked_line_are_shaped_in_full() {
        let f = fixture();
        f.checked(&stream(3));
        f.checked(&format!(
            "{} and then something else",
            stream(3).replace("some", "any")
        ));
        f.checked("short");
        f.checked(&"x".repeat(APPEND_MIN_LEN + 10));
        assert_eq!(f.system.shape_stats().appended_lines, 0);
    }

    #[test]
    fn non_ascii_text_is_never_assembled() {
        let f = fixture();
        let base = format!("{} caf\u{e9} au lait", stream(4));
        f.checked(&base);
        f.checked(&format!("{base} et du pain"));
        f.checked(&format!("{} and more", stream(4)));
        f.checked(&format!("{} and more words \u{e9}", stream(4)));
        assert_eq!(f.system.shape_stats().appended_lines, 0);
    }

    #[test]
    fn lines_with_several_font_runs_are_shaped_in_full() {
        let f = fixture();
        let text = stream(3);
        let split = 10;
        let bold = Font {
            weight: FontWeight::BOLD,
            ..f.font.clone()
        };
        let runs = [
            TextRun {
                len: split,
                font: f.font.clone(),
                ..Default::default()
            },
            TextRun {
                len: text.len() - split,
                font: bold,
                ..Default::default()
            },
        ];
        f.window.layout_line(&text, px(16.), &runs, None);
        let longer = format!("{text} and more");
        let longer_runs = [
            runs[0].clone(),
            TextRun {
                len: longer.len() - split,
                ..runs[1].clone()
            },
        ];
        f.window.layout_line(&longer, px(16.), &longer_runs, None);
        assert_eq!(f.system.shape_stats().appended_lines, 0);
    }

    /// PRF-017 R3: runs that differ only in paint-only attributes share one font run, so
    /// recoloring neither reshapes nor changes the shaped layout, and such lines are assembled
    /// like plain text.
    #[test]
    fn paint_only_runs_do_not_split_the_font_run() {
        let f = fixture();
        let text = stream(3);
        let split = 10;
        let colored = |len: usize| {
            [
                TextRun {
                    len: split,
                    font: f.font.clone(),
                    ..Default::default()
                },
                TextRun {
                    len: len - split,
                    font: f.font.clone(),
                    color: crate::red(),
                    ..Default::default()
                },
            ]
        };
        f.window
            .layout_line(&text, px(16.), &colored(text.len()), None);
        let shapes = f.system.shape_stats().platform_line_shapes;
        // Different colors, the same font run: served from the cache without any shaping.
        f.window.layout_line(
            &text,
            px(16.),
            &[TextRun {
                len: text.len(),
                font: f.font.clone(),
                color: crate::blue(),
                ..Default::default()
            }],
            None,
        );
        assert_eq!(f.system.shape_stats().platform_line_shapes, shapes);

        let longer = format!("{text} and more");
        let layout = f
            .window
            .layout_line(&longer, px(16.), &colored(longer.len()), None);
        assert_eq!(f.system.shape_stats().appended_lines, 1);
        let platform = f.platform(&longer);
        let tolerance = 0.001f32.max(platform.width.0 * 8.0 * f32::EPSILON);
        assert!(layouts_agree_within(&layout, &platform, tolerance));
    }

    #[test]
    fn a_platform_that_shapes_across_spaces_disables_assembly() {
        let f = fixture();
        // A glyph shift that depends on a word before the shaped tail.
        f.shaper.skew_triple_nines.store(true, Ordering::SeqCst);
        let base = "a streaming message begins with 999 and some ordinary words";
        f.checked(base);
        f.checked(&format!("{base} w0"));
        let stats = f.system.shape_stats();
        assert!(stats.append_disabled, "{stats:?}");
        assert_eq!(stats.append_checks, 1);
        let assembled = stats.appended_lines;
        for words in 1..6 {
            f.checked(&format!("{base} w0 w{words}"));
        }
        assert_eq!(f.system.shape_stats().appended_lines, assembled);
    }

    #[test]
    fn sampled_comparisons_continue_after_the_initial_checks() {
        let f = fixture();
        f.checked(&stream(0));
        let steps = (APPEND_INITIAL_CHECKS + 2 * APPEND_CHECK_INTERVAL) as usize * 8 / 7 + 8;
        for words in 1..=steps {
            f.checked(&stream(words));
        }
        let stats = f.system.shape_stats();
        assert_eq!(stats.appended_lines, assembled_of(steps));
        assert!(stats.appended_lines > APPEND_INITIAL_CHECKS + 2 * APPEND_CHECK_INTERVAL);
        assert_eq!(
            stats.append_checks,
            APPEND_INITIAL_CHECKS + stats.appended_lines / APPEND_CHECK_INTERVAL
        );
        assert!(!stats.append_disabled);
    }

    #[test]
    fn a_new_font_generation_forgets_tracked_lines() {
        let f = fixture();
        f.checked(&stream(0));
        f.checked(&stream(1));
        assert_eq!(f.system.shape_stats().appended_lines, 1);
        f.system
            .add_fonts(vec![Cow::Borrowed(b"Another Family".as_slice())])
            .unwrap();
        f.checked(&stream(2));
        assert_eq!(f.system.shape_stats().appended_lines, 1);
    }
}
