//! Runtime font registration (TXT-003).
//!
//! Font bytes are validated against the checks every addressable font must
//! pass before they reach the platform text backend, so that bad data produces
//! an ordinary error instead of a silently unusable family.

use super::TextSystem;
use crate::App;
use std::borrow::Cow;

/// The name identifiers a registered font must declare.
const NAME_ID_FAMILY: u16 = ttf_parser::name_id::FAMILY;
const NAME_ID_FULL_NAME: u16 = ttf_parser::name_id::FULL_NAME;
const NAME_ID_POST_SCRIPT_NAME: u16 = ttf_parser::name_id::POST_SCRIPT_NAME;
const NAME_ID_TYPOGRAPHIC_FAMILY: u16 = ttf_parser::name_id::TYPOGRAPHIC_FAMILY;

/// The index of the face that is validated in font collections.
const VALIDATION_FACE_INDEX: u32 = 0;

/// The error returned when font bytes cannot be registered.
///
/// The first six variants name the first failed validation check; the text
/// system is left untouched when any of them is returned.
#[derive(Debug, thiserror::Error)]
pub enum FontRegistrationError {
    /// The data does not parse as a TrueType or OpenType font.
    #[error("data is not a TrueType or OpenType font")]
    NotAFont,
    /// The first face has no decodable family name record (name id 1).
    #[error("font has no decodable family name (name id 1)")]
    NoFamilyName,
    /// The first face has no decodable full name record (name id 4).
    #[error("font has no decodable full name (name id 4)")]
    NoFullName,
    /// The first face has no decodable PostScript name record (name id 6).
    #[error("font has no decodable PostScript name (name id 6)")]
    NoPostScriptName,
    /// The first face does not map the lowercase letter m (U+006D) to a glyph.
    #[error("font has no glyph for the letter m (U+006D)")]
    NoGlyphForM,
    /// The text backend rejected the font data.
    #[error("the text system failed to install the font: {0:#}")]
    Platform(anyhow::Error),
}

/// The result of successfully validating font bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedFont {
    /// The family name text addresses the font by: the typographic family
    /// (name id 16) when present, otherwise the family name (name id 1).
    pub family: String,
}

/// Validates font bytes without touching any text system.
///
/// Checks, in order, on face index 0: that the data parses as a font; that name
/// ids 1, 4 and 6 are present and decodable as text; and that U+006D maps to a
/// glyph. Returns an error naming the first failed check.
pub fn validate_font_data(data: &[u8]) -> Result<ValidatedFont, FontRegistrationError> {
    let face = ttf_parser::Face::parse(data, VALIDATION_FACE_INDEX)
        .map_err(|_| FontRegistrationError::NotAFont)?;

    let family = decoded_name(&face, NAME_ID_FAMILY).ok_or(FontRegistrationError::NoFamilyName)?;
    decoded_name(&face, NAME_ID_FULL_NAME).ok_or(FontRegistrationError::NoFullName)?;
    decoded_name(&face, NAME_ID_POST_SCRIPT_NAME).ok_or(FontRegistrationError::NoPostScriptName)?;

    match face.glyph_index('m') {
        Some(glyph) if glyph.0 != 0 => {}
        _ => return Err(FontRegistrationError::NoGlyphForM),
    }

    let family = decoded_name(&face, NAME_ID_TYPOGRAPHIC_FAMILY).unwrap_or(family);
    Ok(ValidatedFont { family })
}

/// Returns the first decodable record for `name_id`, preferring records whose
/// language is English (United States).
fn decoded_name(face: &ttf_parser::Face<'_>, name_id: u16) -> Option<String> {
    const ENGLISH_US_WINDOWS_LANGUAGE: u16 = 0x0409;
    let mut fallback = None;
    for name in face.names() {
        if name.name_id != name_id {
            continue;
        }
        let Some(text) = name.to_string() else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if name.language_id == ENGLISH_US_WINDOWS_LANGUAGE {
            return Some(text);
        }
        fallback.get_or_insert(text);
    }
    fallback
}

impl TextSystem {
    /// Validates and registers TrueType or OpenType font bytes.
    ///
    /// The font is then addressable by the family name declared in its own
    /// metadata (see [`ValidatedFont::family`]). The bytes are copied, so the
    /// caller may free or reuse its buffer immediately. On success every cached
    /// failed font resolution becomes retryable, cached layouts are invalidated
    /// and the font generation advances (see [`TextSystem::add_fonts`]). On
    /// failure the text system is unchanged.
    pub fn register_font(&self, data: &[u8]) -> Result<(), FontRegistrationError> {
        self.register_fonts(&[data])
    }

    /// Validates and registers several fonts with one installation.
    ///
    /// Every buffer is validated before any is handed to the backend, so a
    /// rejected buffer leaves the text system unchanged. The font generation
    /// advances once for the whole batch. An empty batch does nothing.
    pub fn register_fonts(&self, fonts: &[&[u8]]) -> Result<(), FontRegistrationError> {
        if fonts.is_empty() {
            return Ok(());
        }
        for data in fonts {
            validate_font_data(data)?;
        }
        let owned = fonts
            .iter()
            .map(|data| Cow::Owned(data.to_vec()))
            .collect::<Vec<_>>();
        self.add_fonts(owned)
            .map_err(FontRegistrationError::Platform)
    }
}

impl App {
    /// Validates and registers font bytes with the app's text system, then
    /// refreshes every window so text that used the family (or its fallback) is
    /// re-laid out and repainted.
    ///
    /// See [`TextSystem::register_font`].
    pub fn register_font(&mut self, data: &[u8]) -> Result<(), FontRegistrationError> {
        self.text_system().register_font(data)?;
        self.refresh_windows();
        Ok(())
    }

    /// Registers several fonts at once; see [`TextSystem::register_fonts`].
    pub fn register_fonts(&mut self, fonts: &[&[u8]]) -> Result<(), FontRegistrationError> {
        if fonts.is_empty() {
            return Ok(());
        }
        self.text_system().register_fonts(fonts)?;
        self.refresh_windows();
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod mock_platform {
    //! A platform text system for tests that records installations, can be
    //! told to fail them, and resolves families from the fonts it was given.

    use crate::{
        Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, FontStyle, GlyphId, LineLayout,
        Pixels, PlatformTextSystem, RenderGlyphParams, Size, TextRenderingMode, point, size,
    };
    use anyhow::{Result, anyhow};
    use parking_lot::Mutex;
    use std::borrow::Cow;

    struct MockFace {
        family: String,
        italic: bool,
        weight: f32,
        data: Vec<u8>,
    }

    #[derive(Default)]
    struct MockState {
        faces: Vec<MockFace>,
        installations: usize,
        fail_next_installation: bool,
    }

    /// Resolves [`Font`] requests against fonts handed to `add_fonts`, measuring
    /// advances from the real font bytes. `FontId`s are indices into the
    /// registered faces, so they stay valid across installations.
    #[derive(Default)]
    pub(crate) struct MockPlatform {
        state: Mutex<MockState>,
    }

    impl MockPlatform {
        /// Makes the next `add_fonts` call fail without installing anything.
        pub(crate) fn fail_next_installation(&self) {
            self.state.lock().fail_next_installation = true;
        }

        /// The number of `add_fonts` calls that installed fonts.
        pub(crate) fn installations(&self) -> usize {
            self.state.lock().installations
        }

        /// The number of installed faces.
        pub(crate) fn face_count(&self) -> usize {
            self.state.lock().faces.len()
        }

        fn with_face<R>(
            &self,
            font_id: FontId,
            read: impl FnOnce(&ttf_parser::Face<'_>) -> R,
        ) -> R {
            let state = self.state.lock();
            let face = &state.faces[font_id.0];
            let parsed = ttf_parser::Face::parse(&face.data, 0).expect("installed fonts parse");
            read(&parsed)
        }
    }

    impl PlatformTextSystem for MockPlatform {
        fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
            let mut state = self.state.lock();
            if std::mem::take(&mut state.fail_next_installation) {
                return Err(anyhow!("installation refused"));
            }
            for data in fonts {
                let validated = super::validate_font_data(&data).expect("mock gets valid fonts");
                let face = ttf_parser::Face::parse(&data, 0).expect("validated fonts parse");
                let italic = face.is_italic();
                let weight = face.weight().to_number() as f32;
                drop(face);
                state.faces.push(MockFace {
                    family: validated.family,
                    italic,
                    weight,
                    data: data.into_owned(),
                });
            }
            state.installations += 1;
            Ok(())
        }

        fn all_font_names(&self) -> Vec<String> {
            self.state
                .lock()
                .faces
                .iter()
                .map(|face| face.family.clone())
                .collect()
        }

        fn font_id(&self, descriptor: &Font) -> Result<FontId> {
            let wants_italic = descriptor.style != FontStyle::Normal;
            self.state
                .lock()
                .faces
                .iter()
                .enumerate()
                .filter(|(_, face)| face.family == descriptor.family.as_str())
                .min_by(|(_, a), (_, b)| {
                    let score = |face: &MockFace| {
                        let style = if face.italic == wants_italic {
                            0.
                        } else {
                            1000.
                        };
                        style + (face.weight - descriptor.weight.0).abs()
                    };
                    score(a).total_cmp(&score(b))
                })
                .map(|(index, _)| FontId(index))
                .ok_or_else(|| anyhow!("no font family named {}", descriptor.family))
        }

        fn family_font_ids(&self, family: &str) -> Vec<FontId> {
            self.state
                .lock()
                .faces
                .iter()
                .enumerate()
                .filter(|(_, face)| face.family == family)
                .map(|(index, _)| FontId(index))
                .collect()
        }

        fn font_metrics(&self, font_id: FontId) -> FontMetrics {
            let units_per_em = self.with_face(font_id, |face| face.units_per_em()) as u32;
            FontMetrics {
                units_per_em,
                ascent: 800.,
                descent: 200.,
                line_gap: 0.,
                underline_position: -100.,
                underline_thickness: 50.,
                cap_height: 700.,
                x_height: 500.,
                bounding_box: Bounds {
                    origin: point(0., 0.),
                    size: size(1000., 1000.),
                },
            }
        }

        fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
            let advance = self.advance(font_id, glyph_id)?;
            Ok(Bounds {
                origin: point(0., 0.),
                size: advance,
            })
        }

        fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
            self.with_face(font_id, |face| {
                face.glyph_hor_advance(ttf_parser::GlyphId(glyph_id.0 as u16))
                    .map(|advance| size(advance as f32, 0.))
                    .ok_or_else(|| anyhow!("glyph {} has no advance", glyph_id.0))
            })
        }

        fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
            self.with_face(font_id, |face| face.glyph_index(ch))
                .map(|glyph| GlyphId(glyph.0 as u32))
        }

        fn glyph_raster_bounds(&self, _: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
            Err(anyhow!("the mock platform does not rasterize"))
        }

        fn rasterize_glyph(
            &self,
            _: &RenderGlyphParams,
            _: Bounds<DevicePixels>,
        ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
            Err(anyhow!("the mock platform does not rasterize"))
        }

        fn layout_line(&self, _: &str, _: Pixels, _: &[FontRun]) -> LineLayout {
            LineLayout::default()
        }

        fn recommended_rendering_mode(&self, _: FontId, _: Pixels) -> TextRenderingMode {
            TextRenderingMode::Grayscale
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock_platform::MockPlatform;
    use super::*;
    use crate::PlatformTextSystem as _;
    use crate::text_system::test_font::TestFontBuilder;
    use crate::{
        Context, Entity, IntoElement, Render, StyleRefinement, TestAppContext, Window, div, font,
        prelude::*,
    };
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{Arc, atomic::Ordering},
    };

    fn text_system() -> (Arc<MockPlatform>, TextSystem) {
        let platform = Arc::new(MockPlatform::default());
        (platform.clone(), TextSystem::new(platform))
    }

    fn generation(text_system: &TextSystem) -> usize {
        text_system.font_generation.load(Ordering::Acquire)
    }

    fn cached_resolutions(text_system: &TextSystem) -> usize {
        text_system.font_ids_by_font.read().len()
    }

    // TXT-003 A1

    #[test]
    fn valid_font_reports_its_declared_family() {
        let bytes = TestFontBuilder::new("Registered Sample").build();
        assert_eq!(
            validate_font_data(&bytes).unwrap(),
            ValidatedFont {
                family: "Registered Sample".into()
            }
        );
    }

    #[test]
    fn typographic_family_takes_precedence_over_the_family_name() {
        let bytes = TestFontBuilder::new("Sample Light")
            .typographic_family("Sample")
            .build();
        assert_eq!(validate_font_data(&bytes).unwrap().family, "Sample");
    }

    #[test]
    fn validation_reports_the_first_failed_check() {
        fn error(bytes: &[u8]) -> String {
            validate_font_data(bytes).unwrap_err().to_string()
        }
        assert!(matches!(
            validate_font_data(&[1, 2, 3, 4]),
            Err(FontRegistrationError::NotAFont)
        ));
        assert!(matches!(
            validate_font_data(&[]),
            Err(FontRegistrationError::NotAFont)
        ));
        let truncated = &TestFontBuilder::new("Cut").build()[..40];
        assert!(matches!(
            validate_font_data(truncated),
            Err(FontRegistrationError::NotAFont)
        ));
        assert!(matches!(
            validate_font_data(&TestFontBuilder::new("X").without_family_name().build()),
            Err(FontRegistrationError::NoFamilyName)
        ));
        assert!(matches!(
            validate_font_data(&TestFontBuilder::new("X").without_full_name().build()),
            Err(FontRegistrationError::NoFullName)
        ));
        assert!(matches!(
            validate_font_data(&TestFontBuilder::new("X").without_postscript_name().build()),
            Err(FontRegistrationError::NoPostScriptName)
        ));
        assert!(matches!(
            validate_font_data(&TestFontBuilder::new("X").without_glyph('m').build()),
            Err(FontRegistrationError::NoGlyphForM)
        ));
        // Checks run in order: the family name is reported before the full name
        // and the full name before the missing glyph.
        let everything_wrong = TestFontBuilder::new("X")
            .without_family_name()
            .without_full_name()
            .without_glyph('m')
            .build();
        assert!(matches!(
            validate_font_data(&everything_wrong),
            Err(FontRegistrationError::NoFamilyName)
        ));
        let name_and_glyph = TestFontBuilder::new("X")
            .without_full_name()
            .without_glyph('m')
            .build();
        assert!(matches!(
            validate_font_data(&name_and_glyph),
            Err(FontRegistrationError::NoFullName)
        ));
        assert!(error(&[1, 2, 3, 4]).contains("not a TrueType or OpenType font"));
        assert!(
            error(&TestFontBuilder::new("X").without_glyph('m').build())
                .contains("glyph for the letter m")
        );
    }

    #[test]
    fn rejected_fixtures_leave_the_text_system_untouched() {
        let (platform, text_system) = text_system();
        let family = "Not Installed Anywhere";
        let requested = font(family);
        assert!(text_system.font_id(&requested).is_err());
        let resolutions = cached_resolutions(&text_system);
        let before = generation(&text_system);

        let fixtures: [Vec<u8>; 3] = [
            vec![1, 2, 3, 4],
            TestFontBuilder::new(family).without_full_name().build(),
            TestFontBuilder::new(family).without_glyph('m').build(),
        ];
        for fixture in &fixtures {
            assert!(text_system.register_font(fixture).is_err());
        }
        // A batch containing one bad font installs none of them.
        let good = TestFontBuilder::new(family).build();
        assert!(text_system.register_fonts(&[&good, &fixtures[1]]).is_err());

        assert_eq!(platform.installations(), 0);
        assert_eq!(platform.face_count(), 0);
        assert_eq!(generation(&text_system), before);
        assert_eq!(cached_resolutions(&text_system), resolutions);
        assert!(text_system.font_id(&requested).is_err());
    }

    #[test]
    fn registration_copies_the_callers_buffer() {
        let (platform, text_system) = text_system();
        let mut bytes = TestFontBuilder::new("Copied").build();
        text_system.register_font(&bytes).unwrap();
        bytes.fill(0);
        drop(bytes);
        let font_id = text_system.font_id(&font("Copied")).unwrap();
        assert!(platform.glyph_for_char(font_id, 'A').is_some());
    }

    // TXT-003 A3

    #[test]
    fn failed_lookup_becomes_resolvable_after_registration_without_recreating_the_text_system() {
        let (_, text_system) = text_system();
        let regular = font("Late Family");
        let bold = font("Late Family").bold();
        assert!(text_system.font_id(&regular).is_err());
        assert!(text_system.font_id(&bold).is_err());
        // An unrelated failure is cached too and must become retryable.
        assert!(text_system.font_id(&font("Other Late Family")).is_err());

        text_system
            .register_font(&TestFontBuilder::new("Late Family").build())
            .unwrap();
        text_system
            .register_font(
                &TestFontBuilder::new("Other Late Family")
                    .weight(700)
                    .build(),
            )
            .unwrap();

        assert!(text_system.font_id(&regular).is_ok());
        assert!(text_system.font_id(&bold).is_ok());
        assert!(text_system.font_id(&font("Other Late Family")).is_ok());
        assert_eq!(
            text_system.resolve_font(&regular),
            text_system.font_id(&regular).unwrap()
        );
    }

    // TXT-003 A4

    #[test]
    fn platform_failure_propagates_and_changes_neither_generation_nor_caches() {
        let (platform, text_system) = text_system();
        let requested = font("Refused Family");
        assert!(text_system.font_id(&requested).is_err());
        let before = generation(&text_system);
        let resolutions = cached_resolutions(&text_system);

        platform.fail_next_installation();
        let error = text_system
            .register_font(&TestFontBuilder::new("Refused Family").build())
            .unwrap_err();
        assert!(matches!(error, FontRegistrationError::Platform(_)));
        assert!(error.to_string().contains("installation refused"));
        assert_eq!(generation(&text_system), before);
        assert_eq!(cached_resolutions(&text_system), resolutions);
        assert_eq!(platform.face_count(), 0);
        assert!(text_system.font_id(&requested).is_err());

        // The failure was one-shot; a retry succeeds.
        text_system
            .register_font(&TestFontBuilder::new("Refused Family").build())
            .unwrap();
        assert_eq!(generation(&text_system), before + 1);
        assert!(text_system.font_id(&requested).is_ok());
    }

    // TXT-003 A5

    #[test]
    fn earlier_resolutions_survive_and_the_generation_advances_once_per_installation() {
        let (_, text_system) = text_system();
        assert_eq!(generation(&text_system), 0);
        text_system
            .register_font(&TestFontBuilder::new("First Family").build())
            .unwrap();
        assert_eq!(generation(&text_system), 1);
        let first = text_system.font_id(&font("First Family")).unwrap();

        text_system
            .register_font(&TestFontBuilder::new("Second Family").build())
            .unwrap();
        assert_eq!(generation(&text_system), 2);
        assert_eq!(text_system.font_id(&font("First Family")).unwrap(), first);
        assert_eq!(text_system.resolve_font(&font("First Family")), first);
        assert_ne!(text_system.font_id(&font("Second Family")).unwrap(), first);

        // A batch is one installation.
        let third = TestFontBuilder::new("Third Family").build();
        let fourth = TestFontBuilder::new("Fourth Family").build();
        text_system.register_fonts(&[&third, &fourth]).unwrap();
        assert_eq!(generation(&text_system), 3);
        text_system.register_fonts(&[]).unwrap();
        assert_eq!(generation(&text_system), 3);
    }

    #[test]
    fn registration_resets_missing_glyph_reporting() {
        let (_, text_system) = text_system();
        let before = text_system
            .missing_glyph_reporter
            .generation
            .load(Ordering::Acquire);
        text_system
            .register_font(&TestFontBuilder::new("Resetting").build())
            .unwrap();
        let after = text_system
            .missing_glyph_reporter
            .generation
            .load(Ordering::Acquire);
        assert_eq!(after, before + 1);
    }

    struct Child {
        renders: Rc<Cell<usize>>,
    }

    impl Render for Child {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div()
        }
    }

    struct Root {
        child: Entity<Child>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(self.child.clone().cached(StyleRefinement::default()))
        }
    }

    #[vespertine::test]
    fn app_registration_refreshes_windows_only_on_success(cx: &mut TestAppContext) {
        let renders = Rc::new(Cell::new(0));
        let child = cx.new(|_| Child {
            renders: renders.clone(),
        });
        let window = cx.add_window(|_, _| Root { child });
        let draw = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap()
        };
        draw(cx);
        draw(cx);
        let settled = renders.get();

        let rejected = cx.update(|cx| cx.register_font(&[1, 2, 3, 4]));
        assert!(matches!(rejected, Err(FontRegistrationError::NotAFont)));
        draw(cx);
        assert_eq!(renders.get(), settled, "a rejected font repaints nothing");

        let bytes = TestFontBuilder::new("App Registered").build();
        cx.update(|cx| cx.register_font(&bytes)).unwrap();
        draw(cx);
        assert!(
            renders.get() > settled,
            "a registered font repaints windows"
        );
    }
}
