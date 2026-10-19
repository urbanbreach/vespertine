//! Font catalogs and independent typography roles (TXT-004).
//!
//! A [`FontCatalog`] lists the families an interface can offer: embedded
//! families registered through the validated registration of
//! [`TextSystem::register_font`], the virtual platform default family and the
//! installed families. [`Typography`] is the application global holding, for
//! the interface, code and terminal roles, the user's requested family and the
//! effective family used for rendering, together with the three sizes.
//!
//! # Change notification
//!
//! Every accepted change advances [`Typography::generation`], replaces the
//! global (so `cx.observe_global::<Typography>` observers run) and refreshes
//! every window. The theme layer consumes the global to rebuild its font and
//! size tokens; long-lived layout caches compare the generation, a wrapping
//! 32-bit counter kept separate from the theme's style generation, to
//! invalidate their measurements.

use super::{FontRegistrationError, TextSystem, ValidatedFont, font, validate_font_data};
use crate::{App, Font, Global, Pixels, Rems, SharedString, Window, px, rems};
use std::{borrow::Cow, sync::Arc};

/// The family name the platform text system resolves to its system UI font.
pub const PLATFORM_DEFAULT_FAMILY: &str = ".SystemUIFont";
/// The interface size presets, in logical pixels.
pub const INTERFACE_SIZE_PRESETS: [f32; 7] = [12., 13., 14., 15., 16., 18., 20.];
/// The default interface size, also the baseline interface designs are drawn at.
pub const DEFAULT_INTERFACE_SIZE: f32 = 16.;
/// The default code text size in logical pixels.
pub const DEFAULT_CODE_SIZE: f32 = 12.5;
/// The default terminal text size in logical pixels.
pub const DEFAULT_TERMINAL_SIZE: f32 = 13.;
/// The smallest code or terminal text size in logical pixels.
pub const MIN_TEXT_SIZE: f32 = 8.;
/// The largest code or terminal text size in logical pixels.
pub const MAX_TEXT_SIZE: f32 = 32.;
/// The characters whose advances decide whether a face is fixed width.
pub const FIXED_WIDTH_PROBE_CHARS: [char; 4] = ['i', 'm', 'W', '0'];
/// The largest allowed advance spread, as a fraction of the smallest advance.
pub const FIXED_WIDTH_TOLERANCE: f32 = 0.01;

const PLATFORM_DEFAULT_SETTING: &str = "system";
const INSTALLED_SETTING_PREFIX: &str = "installed:";

/// Converts a design size in pixels at the 16 px baseline to a rem multiple.
///
/// A 14 px design size is 0.875 rem.
pub fn px_to_rem(design_px: f32) -> Rems {
    rems(design_px / DEFAULT_INTERFACE_SIZE)
}

/// Normalizes an interface size to the nearest preset; ties go to the smaller
/// preset and a non-finite size becomes the default.
pub fn normalize_interface_size(size: f32) -> f32 {
    let mut best = DEFAULT_INTERFACE_SIZE;
    let mut best_distance = f32::INFINITY;
    for preset in INTERFACE_SIZE_PRESETS {
        let distance = (preset - size).abs();
        // Presets ascend and the comparison is strict, so ties keep the smaller.
        if distance < best_distance {
            best = preset;
            best_distance = distance;
        }
    }
    best
}

/// Clamps a code or terminal size to 8 to 32 px; NaN becomes `default`.
fn clamp_text_size(size: f32, default: f32) -> f32 {
    if size.is_nan() {
        default
    } else {
        size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE)
    }
}

/// The advances of the [`FIXED_WIDTH_PROBE_CHARS`] in one font face.
///
/// Only ratios between advances matter, so any consistent unit works. A glyph
/// the face does not map is `None`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceAdvances(pub [Option<f32>; 4]);

impl FaceAdvances {
    /// Advances for `i`, `m`, `W` and `0`, in that order.
    pub fn new(i: Option<f32>, m: Option<f32>, w: Option<f32>, zero: Option<f32>) -> Self {
        Self([i, m, w, zero])
    }

    /// A face that maps all four glyphs with the same advance.
    pub fn uniform(advance: f32) -> Self {
        Self([Some(advance); 4])
    }

    /// Whether the face maps U+006D, which every primary font must.
    pub fn maps_m(&self) -> bool {
        self.0[1].is_some()
    }

    /// Whether the face maps all four probe glyphs and their advances differ by
    /// at most [`FIXED_WIDTH_TOLERANCE`] of the smallest advance.
    pub fn is_fixed_width(&self) -> bool {
        let mut smallest = f32::INFINITY;
        let mut largest = f32::NEG_INFINITY;
        for advance in self.0 {
            let Some(advance) = advance else {
                return false;
            };
            smallest = smallest.min(advance);
            largest = largest.max(advance);
        }
        smallest > 0. && largest - smallest <= smallest * FIXED_WIDTH_TOLERANCE
    }
}

/// Whether a family with these faces is fixed width: it has at least one face
/// and every one of its faces is fixed width. A single proportional face, such
/// as an italic, disqualifies the family because selecting a family selects
/// all of its faces.
pub fn faces_are_fixed_width(faces: &[FaceAdvances]) -> bool {
    !faces.is_empty() && faces.iter().all(FaceAdvances::is_fixed_width)
}

/// Provides the font information a [`FontCatalog`] is built from.
///
/// [`TextSystem`] implements it; tests supply mocks.
pub trait FontCatalogSource {
    /// The names of the families the system knows, including registered fonts.
    fn installed_family_names(&self) -> Vec<String>;

    /// The faces of `family` that can serve as a primary font, measured at the
    /// probe glyphs. Empty when the family does not exist.
    fn family_faces(&self, family: &str) -> Vec<FaceAdvances>;

    /// Registers font data; see [`TextSystem::register_fonts`].
    fn register_fonts(&self, fonts: &[&[u8]]) -> Result<(), FontRegistrationError>;
}

impl FontCatalogSource for TextSystem {
    fn installed_family_names(&self) -> Vec<String> {
        self.all_font_names()
    }

    fn family_faces(&self, family: &str) -> Vec<FaceAdvances> {
        self.platform_text_system
            .family_font_ids(family)
            .into_iter()
            .map(|font_id| {
                let units_per_em = self.units_per_em(font_id).max(1) as f32;
                let mut advances = [None; 4];
                for (slot, ch) in advances.iter_mut().zip(FIXED_WIDTH_PROBE_CHARS) {
                    *slot = self
                        .platform_text_system
                        .glyph_for_char(font_id, ch)
                        .and_then(|glyph| self.platform_text_system.advance(font_id, glyph).ok())
                        .map(|advance| advance.width / units_per_em);
                }
                FaceAdvances(advances)
            })
            .collect()
    }

    fn register_fonts(&self, fonts: &[&[u8]]) -> Result<(), FontRegistrationError> {
        TextSystem::register_fonts(self, fonts)
    }
}

/// Which default an embedded family is, for role fallbacks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EmbeddedFontDefault {
    /// The proportional family the interface falls back to.
    Interface,
    /// The fixed-width family the code and terminal roles fall back to.
    FixedWidth,
}

/// A family of fonts embedded in the application.
#[derive(Clone, Debug)]
pub struct EmbeddedFontFamily {
    /// The stable identifier used in settings. It must not be `system` or start
    /// with `installed:`, and must be unique among embedded families.
    pub id: SharedString,
    /// The font data of every face of the family. All faces must declare the
    /// same family name.
    pub faces: Vec<Cow<'static, [u8]>>,
    /// Whether this family is a role default.
    pub default: Option<EmbeddedFontDefault>,
}

impl EmbeddedFontFamily {
    /// Describes an embedded family with no role default.
    pub fn new(id: impl Into<SharedString>, faces: Vec<Cow<'static, [u8]>>) -> Self {
        Self {
            id: id.into(),
            faces,
            default: None,
        }
    }

    /// Marks the family as the default for a role fallback.
    pub fn with_default(mut self, default: EmbeddedFontDefault) -> Self {
        self.default = Some(default);
        self
    }
}

/// A selectable font family.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum FontChoice {
    /// An embedded family, by identifier.
    Embedded(SharedString),
    /// The platform's system UI font.
    PlatformDefault,
    /// An installed family, by name.
    Installed(SharedString),
}

impl FontChoice {
    /// The stable string form for settings stores: the embedded identifier,
    /// `system`, or `installed:` followed by the family name.
    pub fn to_setting(&self) -> String {
        match self {
            FontChoice::Embedded(id) => id.to_string(),
            FontChoice::PlatformDefault => PLATFORM_DEFAULT_SETTING.to_string(),
            FontChoice::Installed(name) => format!("{INSTALLED_SETTING_PREFIX}{name}"),
        }
    }

    /// Parses the string form written by [`FontChoice::to_setting`].
    ///
    /// An installed name is kept even when the family is not installed (the
    /// selection is preserved until it becomes available again). An unknown or
    /// malformed value falls back to the embedded default of `role`, or the
    /// platform default when the catalog has none.
    pub fn from_setting(value: &str, catalog: &FontCatalog, role: TypographyRole) -> Self {
        if value == PLATFORM_DEFAULT_SETTING {
            return FontChoice::PlatformDefault;
        }
        if let Some(name) = value.strip_prefix(INSTALLED_SETTING_PREFIX) {
            if !name.is_empty() {
                return FontChoice::Installed(name.to_string().into());
            }
        } else if catalog
            .embedded_entries()
            .any(|entry| matches!(&entry.choice, FontChoice::Embedded(id) if id.as_str() == value))
        {
            return FontChoice::Embedded(value.to_string().into());
        }
        catalog.default_choice(role)
    }
}

/// One selectable family in a [`FontCatalog`].
#[derive(Clone, Debug, PartialEq)]
pub struct FontCatalogEntry {
    choice: FontChoice,
    family: SharedString,
    available: bool,
    fixed_width: bool,
    default: Option<EmbeddedFontDefault>,
    registration_error: Option<SharedString>,
}

impl FontCatalogEntry {
    /// The selection this entry represents.
    pub fn choice(&self) -> &FontChoice {
        &self.choice
    }

    /// The family name text is styled with (the declared name for embedded
    /// families that registered, the embedded identifier otherwise).
    pub fn family(&self) -> &SharedString {
        &self.family
    }

    /// Whether the family can be used: embedded families that registered, the
    /// platform default and installed families.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Whether the family is available and every face measured fixed width.
    pub fn is_fixed_width(&self) -> bool {
        self.available && self.fixed_width
    }

    /// The role default an embedded family was declared as.
    pub fn embedded_default(&self) -> Option<EmbeddedFontDefault> {
        self.default
    }

    /// Why an embedded family failed to register.
    pub fn registration_error(&self) -> Option<&str> {
        self.registration_error.as_deref()
    }
}

/// The families an interface can offer, in display order: embedded families in
/// registration order, the platform default, then installed families by
/// ascending name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontCatalog {
    entries: Vec<FontCatalogEntry>,
    embedded_len: usize,
}

impl FontCatalog {
    /// Registers the embedded families and enumerates the installed ones.
    ///
    /// The installed list is captured before any embedded family is added so it
    /// holds only operating-system and user fonts. Each embedded family
    /// registers independently: a failure marks that family unavailable and
    /// never hides the others. Measuring installed families loads their faces,
    /// so this can take a moment on systems with many fonts.
    pub fn new(source: &dyn FontCatalogSource, embedded: &[EmbeddedFontFamily]) -> Self {
        let mut installed_names = source.installed_family_names();
        installed_names.sort_unstable();
        installed_names.dedup();

        let mut entries: Vec<FontCatalogEntry> = Vec::with_capacity(embedded.len() + 1);
        for family in embedded {
            let duplicate = entries
                .iter()
                .any(|entry| entry.choice == FontChoice::Embedded(family.id.clone()));
            entries.push(register_embedded_family(source, family, duplicate));
        }
        let embedded_len = entries.len();
        entries.push(FontCatalogEntry {
            choice: FontChoice::PlatformDefault,
            family: PLATFORM_DEFAULT_FAMILY.into(),
            available: true,
            fixed_width: false,
            default: None,
            registration_error: None,
        });
        let mut catalog = Self {
            entries,
            embedded_len,
        };
        catalog.set_installed(source, installed_names);
        catalog
    }

    /// Registers the embedded families with the app's text system, builds the
    /// catalog and refreshes every window.
    pub fn register(cx: &mut App, embedded: &[EmbeddedFontFamily]) -> Self {
        let text_system = cx.text_system().clone();
        let catalog = Self::new(text_system.as_ref(), embedded);
        cx.refresh_windows();
        catalog
    }

    /// Re-enumerates the installed families against `source`, keeping the
    /// embedded entries, for example after a system font was installed or
    /// removed.
    pub fn rescan(&self, source: &dyn FontCatalogSource) -> Self {
        let mut catalog = Self {
            entries: self.entries[..self.embedded_len + 1].to_vec(),
            embedded_len: self.embedded_len,
        };
        let mut names = source.installed_family_names();
        names.sort_unstable();
        names.dedup();
        catalog.set_installed(source, names);
        catalog
    }

    fn set_installed(&mut self, source: &dyn FontCatalogSource, names: Vec<String>) {
        self.entries.truncate(self.embedded_len + 1);
        for name in names {
            if name.starts_with('.') || name.is_empty() {
                continue;
            }
            let is_embedded_name = self.entries[..self.embedded_len]
                .iter()
                .any(|entry| entry.available && entry.family.as_str() == name);
            if is_embedded_name {
                continue;
            }
            let faces = source.family_faces(&name);
            // The layout engine measures with m, so a family none of whose
            // faces maps it can never be a primary font.
            if !faces.iter().any(FaceAdvances::maps_m) {
                continue;
            }
            let fixed_width = faces_are_fixed_width(&faces);
            self.entries.push(FontCatalogEntry {
                choice: FontChoice::Installed(name.clone().into()),
                family: name.into(),
                available: true,
                fixed_width,
                default: None,
                registration_error: None,
            });
        }
    }

    /// Every entry in catalog order.
    pub fn entries(&self) -> &[FontCatalogEntry] {
        &self.entries
    }

    /// The embedded entries in registration order, including failed ones.
    pub fn embedded_entries(&self) -> impl Iterator<Item = &FontCatalogEntry> {
        self.entries[..self.embedded_len].iter()
    }

    /// The installed entries by ascending name.
    pub fn installed_entries(&self) -> impl Iterator<Item = &FontCatalogEntry> {
        self.entries[self.embedded_len + 1..].iter()
    }

    /// The fixed-width subset of the available entries, in catalog order. The
    /// platform default is never fixed width.
    pub fn fixed_width_entries(&self) -> impl Iterator<Item = &FontCatalogEntry> {
        self.entries.iter().filter(|entry| entry.is_fixed_width())
    }

    /// The entry for a selection.
    pub fn entry(&self, choice: &FontChoice) -> Option<&FontCatalogEntry> {
        self.entries.iter().find(|entry| &entry.choice == choice)
    }

    /// Whether the family is registered, installed or the platform default.
    pub fn is_available(&self, choice: &FontChoice) -> bool {
        self.entry(choice)
            .is_some_and(FontCatalogEntry::is_available)
    }

    /// Whether the family is available and fixed width.
    pub fn is_fixed_width(&self, choice: &FontChoice) -> bool {
        self.entry(choice)
            .is_some_and(FontCatalogEntry::is_fixed_width)
    }

    /// The family name to style text with for a selection.
    pub fn family_name(&self, choice: &FontChoice) -> SharedString {
        match (self.entry(choice), choice) {
            (Some(entry), _) => entry.family.clone(),
            (None, FontChoice::Installed(name)) => name.clone(),
            (None, FontChoice::Embedded(id)) => id.clone(),
            (None, FontChoice::PlatformDefault) => PLATFORM_DEFAULT_FAMILY.into(),
        }
    }

    /// The first embedded family that registered and is declared the default
    /// of `kind`.
    pub fn embedded_default(&self, kind: EmbeddedFontDefault) -> Option<&FontCatalogEntry> {
        self.embedded_entries()
            .find(|entry| entry.available && entry.default == Some(kind))
    }

    /// The embedded default for a role: the interface default for the
    /// interface role and the fixed-width default for code and terminal,
    /// falling back to the platform default family.
    pub fn default_choice(&self, role: TypographyRole) -> FontChoice {
        let kind = match role {
            TypographyRole::Interface => EmbeddedFontDefault::Interface,
            TypographyRole::Code | TypographyRole::Terminal => EmbeddedFontDefault::FixedWidth,
        };
        self.embedded_default(kind)
            .map_or(FontChoice::PlatformDefault, |entry| entry.choice.clone())
    }

    fn interface_fallback(&self) -> FontChoice {
        self.default_choice(TypographyRole::Interface)
    }

    fn fixed_width_fallback(&self) -> FontChoice {
        self.embedded_default(EmbeddedFontDefault::FixedWidth)
            .map_or_else(|| self.interface_fallback(), |entry| entry.choice.clone())
    }
}

/// Registers one embedded family and builds its entry; the entry records why
/// registration failed instead of returning an error.
fn register_embedded_family(
    source: &dyn FontCatalogSource,
    family: &EmbeddedFontFamily,
    duplicate_id: bool,
) -> FontCatalogEntry {
    let mut entry = FontCatalogEntry {
        choice: FontChoice::Embedded(family.id.clone()),
        family: family.id.clone(),
        available: false,
        fixed_width: false,
        default: family.default,
        registration_error: None,
    };
    match register_embedded_faces(source, family, duplicate_id) {
        Ok(declared) => {
            entry.fixed_width = faces_are_fixed_width(&source.family_faces(&declared.family));
            entry.family = declared.family.into();
            entry.available = true;
        }
        Err(error) => entry.registration_error = Some(error.into()),
    }
    entry
}

fn register_embedded_faces(
    source: &dyn FontCatalogSource,
    family: &EmbeddedFontFamily,
    duplicate_id: bool,
) -> Result<ValidatedFont, String> {
    let id = family.id.as_str();
    if id == PLATFORM_DEFAULT_SETTING || id.starts_with(INSTALLED_SETTING_PREFIX) || id.is_empty() {
        return Err(format!("`{id}` is a reserved or empty embedded family id"));
    }
    if duplicate_id {
        return Err(format!("embedded family id `{id}` is already registered"));
    }
    let mut declared: Option<ValidatedFont> = None;
    for (index, face) in family.faces.iter().enumerate() {
        let validated =
            validate_font_data(face).map_err(|error| format!("face {index}: {error}"))?;
        match &declared {
            None => declared = Some(validated),
            Some(first) if first.family != validated.family => {
                return Err(format!(
                    "face {index} declares family `{}` but face 0 declares `{}`",
                    validated.family, first.family
                ));
            }
            Some(_) => {}
        }
    }
    let declared = declared.ok_or_else(|| format!("embedded family `{id}` has no faces"))?;
    let faces: Vec<&[u8]> = family.faces.iter().map(|face| &face[..]).collect();
    source
        .register_fonts(&faces)
        .map_err(|error| error.to_string())?;
    Ok(declared)
}

/// One of the three independent typography roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TypographyRole {
    /// Interface text, sized relative to the root rem size.
    Interface,
    /// Code text, absolute pixel size; any family is legal.
    Code,
    /// Terminal text, absolute pixel size; fixed-width families only.
    Terminal,
}

impl TypographyRole {
    /// All roles in declaration order.
    pub const ALL: [TypographyRole; 3] = [
        TypographyRole::Interface,
        TypographyRole::Code,
        TypographyRole::Terminal,
    ];

    fn index(self) -> usize {
        match self {
            TypographyRole::Interface => 0,
            TypographyRole::Code => 1,
            TypographyRole::Terminal => 2,
        }
    }
}

/// Requested families and sizes, as persisted by a settings store.
#[derive(Clone, Debug, PartialEq)]
pub struct TypographySettings {
    /// The requested interface family.
    pub interface_family: FontChoice,
    /// The requested code family.
    pub code_family: FontChoice,
    /// The requested terminal family.
    pub terminal_family: FontChoice,
    /// The interface size; normalized to a preset on use.
    pub interface_size: f32,
    /// The code size; clamped to 8 to 32 px on use.
    pub code_size: f32,
    /// The terminal size; clamped to 8 to 32 px on use.
    pub terminal_size: f32,
}

impl TypographySettings {
    /// The default selections of `catalog` and the default sizes.
    pub fn defaults(catalog: &FontCatalog) -> Self {
        Self {
            interface_family: catalog.default_choice(TypographyRole::Interface),
            code_family: catalog.default_choice(TypographyRole::Code),
            terminal_family: catalog.default_choice(TypographyRole::Terminal),
            interface_size: DEFAULT_INTERFACE_SIZE,
            code_size: DEFAULT_CODE_SIZE,
            terminal_size: DEFAULT_TERMINAL_SIZE,
        }
    }
}

/// The typography state of the application, held as a [`Global`].
///
/// The requested families are the user's selections and are never rewritten by
/// fallbacks; the effective families are what renders. Methods named `apply_*`
/// change only this value; the associated functions taking an [`App`] also
/// publish the change and refresh every window.
#[derive(Clone, Debug)]
pub struct Typography {
    catalog: Arc<FontCatalog>,
    requested: [FontChoice; 3],
    effective: [FontChoice; 3],
    interface_size: f32,
    code_size: f32,
    terminal_size: f32,
    generation: u32,
}

impl Global for Typography {}

impl Typography {
    /// Builds the state, resolving the effective family of each role and
    /// normalizing the sizes. Requested families are kept as given, even when
    /// unavailable.
    pub fn new(catalog: FontCatalog, settings: TypographySettings) -> Self {
        let mut typography = Self {
            catalog: Arc::new(catalog),
            requested: [
                settings.interface_family,
                settings.code_family,
                settings.terminal_family,
            ],
            effective: [
                FontChoice::PlatformDefault,
                FontChoice::PlatformDefault,
                FontChoice::PlatformDefault,
            ],
            interface_size: normalize_interface_size(settings.interface_size),
            code_size: clamp_text_size(settings.code_size, DEFAULT_CODE_SIZE),
            terminal_size: clamp_text_size(settings.terminal_size, DEFAULT_TERMINAL_SIZE),
            generation: 0,
        };
        typography.resolve_all();
        typography
    }

    /// Installs the state as the application global.
    pub fn init(catalog: FontCatalog, settings: TypographySettings, cx: &mut App) {
        cx.set_global(Self::new(catalog, settings));
    }

    /// The application's typography state.
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// The catalog the roles were resolved against.
    pub fn catalog(&self) -> &FontCatalog {
        &self.catalog
    }

    /// The user's selection for a role, preserved even when unavailable.
    pub fn requested_family(&self, role: TypographyRole) -> &FontChoice {
        &self.requested[role.index()]
    }

    /// The family a role renders with.
    pub fn effective_family(&self, role: TypographyRole) -> &FontChoice {
        &self.effective[role.index()]
    }

    /// The family name to style a role's text with.
    pub fn effective_family_name(&self, role: TypographyRole) -> SharedString {
        self.catalog.family_name(self.effective_family(role))
    }

    /// A [`Font`] for a role's effective family.
    pub fn font(&self, role: TypographyRole) -> Font {
        font(self.effective_family_name(role))
    }

    /// The interface size in logical pixels: the root rem size.
    pub fn interface_size(&self) -> f32 {
        self.interface_size
    }

    /// The code text size in logical pixels.
    pub fn code_size(&self) -> f32 {
        self.code_size
    }

    /// The terminal text size in logical pixels.
    pub fn terminal_size(&self) -> f32 {
        self.terminal_size
    }

    /// The root rem size interface text scales with.
    pub fn rem_size(&self) -> Pixels {
        px(self.interface_size)
    }

    /// The text size of a role in logical pixels; for the interface role this
    /// is the root size.
    pub fn size(&self, role: TypographyRole) -> f32 {
        match role {
            TypographyRole::Interface => self.interface_size,
            TypographyRole::Code => self.code_size,
            TypographyRole::Terminal => self.terminal_size,
        }
    }

    /// The monotonic wrapping counter advanced by every accepted change, for
    /// long-lived layout caches to compare.
    pub fn generation(&self) -> u32 {
        self.generation
    }

    fn resolve(&self, role: TypographyRole) -> FontChoice {
        let requested = self.requested_family(role);
        match role {
            TypographyRole::Interface if self.catalog.is_available(requested) => requested.clone(),
            TypographyRole::Interface => self.catalog.interface_fallback(),
            TypographyRole::Code if self.catalog.is_available(requested) => requested.clone(),
            TypographyRole::Terminal if self.catalog.is_fixed_width(requested) => requested.clone(),
            TypographyRole::Code | TypographyRole::Terminal => self.catalog.fixed_width_fallback(),
        }
    }

    fn resolve_all(&mut self) {
        for role in TypographyRole::ALL {
            self.effective[role.index()] = self.resolve(role);
        }
    }

    fn advance_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Selects a family for a role. Returns `false` without changing anything
    /// when the family is not available (for the terminal: not fixed width) or
    /// is already selected.
    ///
    /// The interface role advances the generation only when its effective
    /// family changes; code and terminal advance it on every accepted change.
    pub fn apply_family(&mut self, role: TypographyRole, choice: FontChoice) -> bool {
        let acceptable = match role {
            TypographyRole::Terminal => self.catalog.is_fixed_width(&choice),
            TypographyRole::Interface | TypographyRole::Code => self.catalog.is_available(&choice),
        };
        if !acceptable || self.requested_family(role) == &choice {
            return false;
        }
        self.requested[role.index()] = choice;
        let effective = self.resolve(role);
        let effective_changed = self.effective_family(role) != &effective;
        self.effective[role.index()] = effective;
        if role != TypographyRole::Interface || effective_changed {
            self.advance_generation();
        }
        true
    }

    /// Replaces the catalog, for example after fonts were installed or removed,
    /// and re-resolves every role. Requested selections are untouched, so
    /// reinstalling a family restores the user's choice. Returns whether any
    /// effective family changed, which also advances the generation once.
    pub fn apply_catalog(&mut self, catalog: FontCatalog) -> bool {
        self.catalog = Arc::new(catalog);
        let before = self.effective.clone();
        self.resolve_all();
        let changed = before != self.effective;
        if changed {
            self.advance_generation();
        }
        changed
    }

    /// Sets the interface size, normalized to the nearest preset. Returns
    /// whether the normalized size differs from the current one.
    pub fn apply_interface_size(&mut self, size: f32) -> bool {
        let size = normalize_interface_size(size);
        if size == self.interface_size {
            return false;
        }
        self.interface_size = size;
        self.advance_generation();
        true
    }

    /// Sets the code text size, clamped to 8 to 32 px. Returns whether it
    /// differs from the current size.
    pub fn apply_code_size(&mut self, size: f32) -> bool {
        let size = clamp_text_size(size, self.code_size);
        if size == self.code_size {
            return false;
        }
        self.code_size = size;
        self.advance_generation();
        true
    }

    /// Sets the terminal text size, clamped to 8 to 32 px. Returns whether it
    /// differs from the current size.
    pub fn apply_terminal_size(&mut self, size: f32) -> bool {
        let size = clamp_text_size(size, self.terminal_size);
        if size == self.terminal_size {
            return false;
        }
        self.terminal_size = size;
        self.advance_generation();
        true
    }

    /// Applies `change` to a copy of the global and, when it reports a change,
    /// publishes the copy (notifying global observers) and refreshes every
    /// window so text is re-measured and repainted. Returns whether anything
    /// changed; no observer runs otherwise.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut Typography) -> bool) -> bool {
        let mut next = cx.global::<Self>().clone();
        if !change(&mut next) {
            return false;
        }
        cx.set_global(next);
        cx.refresh_windows();
        true
    }

    /// Selects a family for a role; see [`Typography::apply_family`].
    pub fn set_family(role: TypographyRole, choice: FontChoice, cx: &mut App) -> bool {
        Self::update(cx, |typography| typography.apply_family(role, choice))
    }

    /// Replaces the catalog; see [`Typography::apply_catalog`]. The catalog is
    /// stored even when no effective family changes, but windows are only
    /// refreshed when one does.
    pub fn set_catalog(catalog: FontCatalog, cx: &mut App) -> bool {
        let mut next = cx.global::<Self>().clone();
        let changed = next.apply_catalog(catalog);
        cx.set_global(next);
        if changed {
            cx.refresh_windows();
        }
        changed
    }

    /// Sets the interface size and the root rem size of `window` (and of every
    /// other window that is not currently being updated), then refreshes every
    /// window. The code and terminal sizes are unaffected. The window's rem
    /// size is applied even when the size is unchanged, so a window opened
    /// after the size was chosen can be brought in line.
    pub fn set_interface_size(window: &mut Window, size: f32, cx: &mut App) -> bool {
        let changed = Self::update(cx, |typography| typography.apply_interface_size(size));
        let rem_size = cx.global::<Self>().rem_size();
        let window_was_current = window.rem_size() == rem_size;
        window.set_rem_size(rem_size);
        if changed {
            for handle in cx.windows() {
                // The window being updated is leased out and fails here; it was
                // set above.
                handle
                    .update(cx, |_, other, _| other.set_rem_size(rem_size))
                    .ok();
            }
        } else if !window_was_current {
            window.refresh();
        }
        changed
    }

    /// Sets the code text size; see [`Typography::apply_code_size`].
    pub fn set_code_size(size: f32, cx: &mut App) -> bool {
        Self::update(cx, |typography| typography.apply_code_size(size))
    }

    /// Sets the terminal text size; see [`Typography::apply_terminal_size`].
    pub fn set_terminal_size(size: f32, cx: &mut App) -> bool {
        Self::update(cx, |typography| typography.apply_terminal_size(size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Context, Entity, IntoElement, Render, TestAppContext, div, prelude::*,
        text_system::font_registration::mock_platform::MockPlatform,
        text_system::test_font::TestFontBuilder,
    };
    use std::{cell::Cell, collections::HashMap, rc::Rc, sync::Mutex};

    const PROPORTIONAL: FaceAdvances = FaceAdvances([Some(0.3), Some(0.9), Some(1.0), Some(0.6)]);

    #[derive(Default)]
    struct MockSource {
        installed: Vec<String>,
        faces: HashMap<String, Vec<FaceAdvances>>,
        registered_batches: Mutex<Vec<usize>>,
        fail_installation: bool,
    }

    impl MockSource {
        fn family(mut self, name: &str, faces: &[FaceAdvances], installed: bool) -> Self {
            self.faces.insert(name.to_string(), faces.to_vec());
            if installed {
                self.installed.push(name.to_string());
            }
            self
        }
    }

    impl FontCatalogSource for MockSource {
        fn installed_family_names(&self) -> Vec<String> {
            self.installed.clone()
        }

        fn family_faces(&self, family: &str) -> Vec<FaceAdvances> {
            self.faces.get(family).cloned().unwrap_or_default()
        }

        fn register_fonts(&self, fonts: &[&[u8]]) -> Result<(), FontRegistrationError> {
            if self.fail_installation {
                return Err(FontRegistrationError::Platform(anyhow::anyhow!("refused")));
            }
            self.registered_batches.lock().unwrap().push(fonts.len());
            Ok(())
        }
    }

    fn embedded(
        id: &str,
        family: &str,
        default: Option<EmbeddedFontDefault>,
    ) -> EmbeddedFontFamily {
        let face = TestFontBuilder::new(family).build();
        EmbeddedFontFamily {
            id: id.to_string().into(),
            faces: vec![Cow::Owned(face)],
            default,
        }
    }

    fn installed(name: &str) -> FontChoice {
        FontChoice::Installed(name.to_string().into())
    }

    fn embedded_choice(id: &str) -> FontChoice {
        FontChoice::Embedded(id.to_string().into())
    }

    /// A catalog with an embedded proportional default, an embedded fixed-width
    /// default, and installed families: a proportional one, a monospaced one
    /// and one with a proportional italic.
    fn standard_source() -> MockSource {
        MockSource::default()
            .family("Embedded Sans", &[PROPORTIONAL], false)
            .family("Embedded Mono", &[FaceAdvances::uniform(0.6)], false)
            .family("Installed Sans", &[PROPORTIONAL], true)
            .family("Installed Mono", &[FaceAdvances::uniform(0.5)], true)
            .family(
                "Mixed Mono",
                &[FaceAdvances::uniform(0.5), PROPORTIONAL],
                true,
            )
    }

    fn standard_embedded() -> Vec<EmbeddedFontFamily> {
        vec![
            embedded(
                "sans",
                "Embedded Sans",
                Some(EmbeddedFontDefault::Interface),
            ),
            embedded(
                "mono",
                "Embedded Mono",
                Some(EmbeddedFontDefault::FixedWidth),
            ),
        ]
    }

    fn standard_catalog() -> FontCatalog {
        FontCatalog::new(&standard_source(), &standard_embedded())
    }

    fn standard_typography() -> Typography {
        let catalog = standard_catalog();
        let settings = TypographySettings::defaults(&catalog);
        Typography::new(catalog, settings)
    }

    // TXT-004 A5

    #[test]
    fn monospaced_face_is_fixed_width_and_proportional_is_not() {
        assert!(faces_are_fixed_width(&[FaceAdvances::uniform(0.6)]));
        assert!(!faces_are_fixed_width(&[PROPORTIONAL]));
        assert!(
            !faces_are_fixed_width(&[]),
            "a family needs at least a face"
        );
    }

    #[test]
    fn fixed_width_tolerance_is_one_percent_of_smallest_advance() {
        let within = FaceAdvances([Some(0.5), Some(0.504), Some(0.5), Some(0.5)]);
        let beyond = FaceAdvances([Some(0.5), Some(0.506), Some(0.5), Some(0.5)]);
        assert!(within.is_fixed_width());
        assert!(!beyond.is_fixed_width());
        assert!(!FaceAdvances::uniform(0.).is_fixed_width());
    }

    #[test]
    fn proportional_italic_face_disqualifies_family_with_monospaced_regular() {
        let regular = FaceAdvances::uniform(0.6);
        let italic = PROPORTIONAL;
        assert!(faces_are_fixed_width(&[regular]));
        assert!(!faces_are_fixed_width(&[regular, italic]));
    }

    #[test]
    fn face_lacking_a_probe_glyph_disqualifies_family() {
        for missing in 0..4 {
            let mut advances = [Some(0.6); 4];
            advances[missing] = None;
            assert!(!faces_are_fixed_width(&[
                FaceAdvances::uniform(0.6),
                FaceAdvances(advances)
            ]));
        }
    }

    #[test]
    fn text_system_source_measures_registered_faces_not_metadata_flags() {
        let platform = Arc::new(MockPlatform::default());
        let text_system = TextSystem::new(platform);
        let mono = TestFontBuilder::monospace("Measured Mono", 600).build();
        let italic = TestFontBuilder::proportional("Measured Mono")
            .italic(true)
            .fixed_pitch_flag(true)
            .subfamily("Italic")
            .build();
        let liar = TestFontBuilder::proportional("Flagged Mono")
            .fixed_pitch_flag(true)
            .build();
        text_system
            .register_fonts(&[&mono, &italic, &liar])
            .unwrap();

        let faces = FontCatalogSource::family_faces(&text_system, "Measured Mono");
        assert_eq!(faces.len(), 2);
        assert!(faces[0].is_fixed_width());
        assert!(!faces[1].is_fixed_width());
        assert!(!faces_are_fixed_width(&faces));
        let flagged = FontCatalogSource::family_faces(&text_system, "Flagged Mono");
        assert!(!faces_are_fixed_width(&flagged));
        assert!(FontCatalogSource::family_faces(&text_system, "Nothing").is_empty());
    }

    // TXT-004 A6

    #[test]
    fn catalog_order_is_embedded_platform_default_then_installed_ascending() {
        let source = standard_source().family("Aardvark Sans", &[PROPORTIONAL], true);
        let catalog = FontCatalog::new(&source, &standard_embedded());
        let order: Vec<FontChoice> = catalog
            .entries()
            .iter()
            .map(|entry| entry.choice().clone())
            .collect();
        assert_eq!(
            order,
            vec![
                embedded_choice("sans"),
                embedded_choice("mono"),
                FontChoice::PlatformDefault,
                installed("Aardvark Sans"),
                installed("Installed Mono"),
                installed("Installed Sans"),
                installed("Mixed Mono"),
            ]
        );
        assert_eq!(
            catalog.family_name(&FontChoice::PlatformDefault).as_str(),
            PLATFORM_DEFAULT_FAMILY
        );
        assert_eq!(
            catalog.family_name(&embedded_choice("mono")).as_str(),
            "Embedded Mono"
        );
    }

    #[test]
    fn catalog_excludes_period_names_embedded_names_and_faces_without_m() {
        let no_m = FaceAdvances::new(Some(0.3), None, Some(1.0), Some(0.6));
        let mut source = standard_source()
            .family(".Hidden Virtual", &[PROPORTIONAL], true)
            .family("No M Family", &[no_m], true)
            .family("Empty Family", &[], true);
        // The operating system also ships a family named like an embedded one.
        source.installed.push("Embedded Sans".into());
        let catalog = FontCatalog::new(&source, &standard_embedded());
        let installed_names: Vec<&str> = catalog
            .installed_entries()
            .map(|entry| entry.family().as_str())
            .collect();
        assert_eq!(
            installed_names,
            ["Installed Mono", "Installed Sans", "Mixed Mono"]
        );
    }

    #[test]
    fn faces_without_m_do_not_hide_a_family_that_has_an_m_face() {
        let no_m = FaceAdvances::new(Some(0.3), None, Some(1.0), Some(0.6));
        let source = MockSource::default().family("Partial", &[no_m, PROPORTIONAL], true);
        let catalog = FontCatalog::new(&source, &[]);
        assert!(catalog.is_available(&installed("Partial")));
    }

    #[test]
    fn embedded_families_register_independently() {
        let source = standard_source();
        let embedded = vec![
            EmbeddedFontFamily::new("broken", vec![Cow::Borrowed(&[1u8, 2, 3, 4])]),
            embedded(
                "sans",
                "Embedded Sans",
                Some(EmbeddedFontDefault::Interface),
            ),
            EmbeddedFontFamily::new("empty", vec![]),
            embedded("sans", "Embedded Sans", None),
            embedded("system", "Reserved", None),
            embedded(
                "mono",
                "Embedded Mono",
                Some(EmbeddedFontDefault::FixedWidth),
            ),
        ];
        let catalog = FontCatalog::new(&source, &embedded);
        let availability: Vec<(String, bool)> = catalog
            .embedded_entries()
            .map(|entry| (entry.choice().to_setting(), entry.is_available()))
            .collect();
        assert_eq!(
            availability,
            [
                ("broken".to_string(), false),
                ("sans".to_string(), true),
                ("empty".to_string(), false),
                ("sans".to_string(), false),
                ("system".to_string(), false),
                ("mono".to_string(), true),
            ]
        );
        let error = |index: usize| {
            catalog
                .embedded_entries()
                .nth(index)
                .and_then(|entry| entry.registration_error())
                .map(str::to_string)
        };
        assert!(
            error(0)
                .unwrap()
                .contains("not a TrueType or OpenType font")
        );
        assert!(error(2).unwrap().contains("no faces"));
        assert!(error(3).unwrap().contains("already registered"));
        assert!(error(4).unwrap().contains("reserved"));
        assert_eq!(error(1), None);
        // Each valid family was handed to the backend on its own.
        assert_eq!(*source.registered_batches.lock().unwrap(), [1, 1]);
    }

    #[test]
    fn embedded_registration_failure_in_the_backend_marks_only_that_family() {
        let source = MockSource {
            fail_installation: true,
            ..standard_source()
        };
        let catalog = FontCatalog::new(&source, &standard_embedded());
        assert!(
            catalog
                .embedded_entries()
                .all(|entry| !entry.is_available())
        );
        assert!(
            catalog
                .embedded_entries()
                .all(|entry| entry.registration_error().unwrap().contains("refused"))
        );
        assert!(catalog.is_available(&FontChoice::PlatformDefault));
    }

    #[test]
    fn embedded_faces_must_share_a_family_name() {
        let family = EmbeddedFontFamily::new(
            "split",
            vec![
                Cow::Owned(TestFontBuilder::new("One").build()),
                Cow::Owned(TestFontBuilder::new("Two").build()),
            ],
        );
        let catalog = FontCatalog::new(&MockSource::default(), &[family]);
        let entry = catalog.embedded_entries().next().unwrap();
        assert!(!entry.is_available());
        assert!(entry.registration_error().unwrap().contains("face 1"));
    }

    #[test]
    fn fixed_width_subset_keeps_catalog_order_and_excludes_platform_default() {
        let catalog = standard_catalog();
        let fixed: Vec<FontChoice> = catalog
            .fixed_width_entries()
            .map(|entry| entry.choice().clone())
            .collect();
        assert_eq!(
            fixed,
            vec![embedded_choice("mono"), installed("Installed Mono")]
        );
        assert!(!catalog.is_fixed_width(&FontChoice::PlatformDefault));
        assert!(!catalog.is_fixed_width(&installed("Mixed Mono")));
        assert!(!catalog.is_fixed_width(&installed("Not Installed")));
    }

    // TXT-004 A2

    #[test]
    fn removed_installed_family_falls_back_and_preserves_the_request() {
        let mut typography = standard_typography();
        for role in [TypographyRole::Interface, TypographyRole::Code] {
            assert!(typography.apply_family(role, installed("Installed Sans")));
        }
        assert!(typography.apply_family(TypographyRole::Terminal, installed("Installed Mono")));
        assert_eq!(
            typography.effective_family(TypographyRole::Terminal),
            &installed("Installed Mono")
        );

        // The selected families disappear from the system.
        let source = MockSource::default()
            .family("Embedded Sans", &[PROPORTIONAL], false)
            .family("Embedded Mono", &[FaceAdvances::uniform(0.6)], false);
        let generation = typography.generation();
        let rescanned = typography.catalog().rescan(&source);
        assert!(typography.apply_catalog(rescanned));
        assert_eq!(typography.generation(), generation.wrapping_add(1));

        assert_eq!(
            typography.effective_family(TypographyRole::Interface),
            &embedded_choice("sans")
        );
        assert_eq!(
            typography.effective_family(TypographyRole::Code),
            &embedded_choice("mono")
        );
        assert_eq!(
            typography.effective_family(TypographyRole::Terminal),
            &embedded_choice("mono")
        );
        assert_eq!(
            typography.requested_family(TypographyRole::Interface),
            &installed("Installed Sans")
        );
        assert_eq!(
            typography.requested_family(TypographyRole::Terminal),
            &installed("Installed Mono")
        );

        // Reinstalling restores the user's choice.
        let restored = typography.catalog().rescan(&standard_source());
        assert!(typography.apply_catalog(restored));
        assert_eq!(
            typography.effective_family(TypographyRole::Interface),
            &installed("Installed Sans")
        );
        assert_eq!(
            typography.effective_family(TypographyRole::Terminal),
            &installed("Installed Mono")
        );
    }

    #[test]
    fn catalog_change_that_leaves_effective_families_alone_is_not_a_change() {
        let mut typography = standard_typography();
        let generation = typography.generation();
        let catalog = typography.catalog().clone();
        assert!(!typography.apply_catalog(catalog));
        assert_eq!(typography.generation(), generation);
    }

    // TXT-004 A3

    #[test]
    fn persisted_proportional_terminal_selection_falls_back_but_is_legal_for_code() {
        let catalog = standard_catalog();
        let settings = TypographySettings {
            code_family: installed("Installed Sans"),
            terminal_family: installed("Installed Sans"),
            ..TypographySettings::defaults(&catalog)
        };
        let typography = Typography::new(catalog, settings);
        assert_eq!(
            typography.effective_family(TypographyRole::Code),
            &installed("Installed Sans")
        );
        assert_eq!(
            typography.effective_family(TypographyRole::Terminal),
            &embedded_choice("mono")
        );
        assert_eq!(
            typography.requested_family(TypographyRole::Terminal),
            &installed("Installed Sans")
        );
    }

    #[test]
    fn terminal_rejects_families_that_are_not_fixed_width() {
        let mut typography = standard_typography();
        let generation = typography.generation();
        for choice in [
            installed("Installed Sans"),
            installed("Mixed Mono"),
            FontChoice::PlatformDefault,
            embedded_choice("sans"),
            installed("Not Installed"),
        ] {
            assert!(!typography.apply_family(TypographyRole::Terminal, choice));
        }
        assert_eq!(typography.generation(), generation);
        assert_eq!(
            typography.requested_family(TypographyRole::Terminal),
            &embedded_choice("mono")
        );
        assert!(typography.apply_family(TypographyRole::Terminal, installed("Installed Mono")));
        // The same proportional families are legal for code.
        assert!(typography.apply_family(TypographyRole::Code, installed("Installed Sans")));
        assert!(typography.apply_family(TypographyRole::Code, FontChoice::PlatformDefault));
        assert!(!typography.apply_family(TypographyRole::Code, installed("Not Installed")));
    }

    #[test]
    fn code_and_terminal_use_the_interface_fallback_only_without_a_fixed_width_default() {
        // The embedded fixed-width default failed to register.
        let source = standard_source();
        let embedded = vec![
            embedded(
                "sans",
                "Embedded Sans",
                Some(EmbeddedFontDefault::Interface),
            ),
            EmbeddedFontFamily::new("mono", vec![Cow::Borrowed(&[0u8, 1, 2, 3])])
                .with_default(EmbeddedFontDefault::FixedWidth),
        ];
        let catalog = FontCatalog::new(&source, &embedded);
        let settings = TypographySettings {
            code_family: installed("Gone"),
            terminal_family: installed("Gone"),
            ..TypographySettings::defaults(&catalog)
        };
        let typography = Typography::new(catalog, settings);
        for role in TypographyRole::ALL {
            assert_eq!(
                typography.effective_family(role),
                &embedded_choice("sans"),
                "{role:?}"
            );
        }

        // With neither embedded default the platform default is used.
        let catalog = FontCatalog::new(&source, &[]);
        let settings = TypographySettings {
            interface_family: installed("Gone"),
            code_family: installed("Gone"),
            terminal_family: installed("Gone"),
            ..TypographySettings::defaults(&catalog)
        };
        let typography = Typography::new(catalog, settings);
        for role in TypographyRole::ALL {
            assert_eq!(
                typography.effective_family(role),
                &FontChoice::PlatformDefault,
                "{role:?}"
            );
        }

        // With the fixed-width default registered they stay fixed width.
        let typography = {
            let catalog = standard_catalog();
            let settings = TypographySettings {
                interface_family: installed("Gone"),
                code_family: installed("Gone"),
                terminal_family: installed("Gone"),
                ..TypographySettings::defaults(&catalog)
            };
            Typography::new(catalog, settings)
        };
        assert_eq!(
            typography.effective_family(TypographyRole::Interface),
            &embedded_choice("sans")
        );
        for role in [TypographyRole::Code, TypographyRole::Terminal] {
            assert_eq!(typography.effective_family(role), &embedded_choice("mono"));
        }
    }

    // TXT-004 A4

    #[test]
    fn interface_size_normalizes_to_presets() {
        assert_eq!(normalize_interface_size(19.), 18.);
        assert_eq!(normalize_interface_size(250.), 20.);
        assert_eq!(normalize_interface_size(11.), 12.);
        assert_eq!(
            normalize_interface_size(13.5),
            13.,
            "ties go to the smaller"
        );
        assert_eq!(normalize_interface_size(17.), 16.);
        assert_eq!(normalize_interface_size(f32::NAN), DEFAULT_INTERFACE_SIZE);
        for preset in INTERFACE_SIZE_PRESETS {
            assert_eq!(normalize_interface_size(preset), preset);
        }
        let catalog = standard_catalog();
        for (requested, expected) in [(19., 18.), (250., 20.), (11., 12.)] {
            let settings = TypographySettings {
                interface_size: requested,
                ..TypographySettings::defaults(&catalog)
            };
            let typography = Typography::new(catalog.clone(), settings);
            assert_eq!(typography.interface_size(), expected);
        }
    }

    #[test]
    fn code_and_terminal_sizes_clamp_and_default() {
        let mut typography = standard_typography();
        assert_eq!(typography.code_size(), 12.5);
        assert_eq!(typography.terminal_size(), 13.);
        assert_eq!(typography.interface_size(), 16.);
        assert!(typography.apply_code_size(4.));
        assert_eq!(typography.code_size(), 8.);
        assert!(typography.apply_code_size(100.));
        assert_eq!(typography.code_size(), 32.);
        assert!(typography.apply_terminal_size(4.));
        assert_eq!(typography.terminal_size(), 8.);
        assert!(typography.apply_terminal_size(100.));
        assert_eq!(typography.terminal_size(), 32.);
        assert!(
            !typography.apply_terminal_size(f32::NAN),
            "NaN keeps the size"
        );
        assert_eq!(typography.terminal_size(), 32.);
    }

    #[test]
    fn design_pixels_convert_to_rem_at_the_sixteen_pixel_baseline() {
        assert_eq!(px_to_rem(14.), rems(0.875));
        assert_eq!(px_to_rem(16.), rems(1.));
        assert_eq!(px_to_rem(24.), rems(1.5));
    }

    // TXT-004 R7

    #[test]
    fn font_choice_round_trips_through_its_setting_string() {
        let catalog = standard_catalog();
        for choice in [
            embedded_choice("sans"),
            embedded_choice("mono"),
            FontChoice::PlatformDefault,
            installed("Installed Sans"),
            installed("Gone Family"),
        ] {
            let setting = choice.to_setting();
            assert_eq!(
                FontChoice::from_setting(&setting, &catalog, TypographyRole::Code),
                choice
            );
        }
        assert_eq!(embedded_choice("sans").to_setting(), "sans");
        assert_eq!(FontChoice::PlatformDefault.to_setting(), "system");
        assert_eq!(installed("A:B").to_setting(), "installed:A:B");
    }

    #[test]
    fn unknown_or_malformed_settings_fall_back_to_the_embedded_default() {
        let catalog = standard_catalog();
        for value in ["", "nonsense", "installed:", "Installed Sans", "SYSTEM"] {
            assert_eq!(
                FontChoice::from_setting(value, &catalog, TypographyRole::Interface),
                embedded_choice("sans"),
                "{value:?}"
            );
            assert_eq!(
                FontChoice::from_setting(value, &catalog, TypographyRole::Terminal),
                embedded_choice("mono"),
                "{value:?}"
            );
        }
        let bare = FontCatalog::new(&MockSource::default(), &[]);
        assert_eq!(
            FontChoice::from_setting("nonsense", &bare, TypographyRole::Code),
            FontChoice::PlatformDefault
        );
    }

    // TXT-004 A7 (pure part)

    #[test]
    fn accepted_changes_advance_the_generation_and_noops_do_not() {
        let mut typography = standard_typography();
        let mut expected = typography.generation();
        let mut accepted = |typography: &Typography, changed: bool| {
            if changed {
                expected = expected.wrapping_add(1);
            }
            assert_eq!(typography.generation(), expected);
        };

        let changed = typography.apply_family(TypographyRole::Code, installed("Installed Sans"));
        assert!(changed);
        accepted(&typography, changed);
        let changed = typography.apply_family(TypographyRole::Code, installed("Installed Sans"));
        assert!(!changed);
        accepted(&typography, changed);

        let changed =
            typography.apply_family(TypographyRole::Interface, installed("Installed Sans"));
        assert!(changed);
        accepted(&typography, changed);
        let changed =
            typography.apply_family(TypographyRole::Interface, installed("Installed Sans"));
        assert!(!changed);
        accepted(&typography, changed);

        let changed = typography.apply_interface_size(20.);
        assert!(changed);
        accepted(&typography, changed);
        let changed = typography.apply_interface_size(19.5);
        assert!(!changed, "19.5 normalizes to the current 20");
        accepted(&typography, changed);

        let changed = typography.apply_code_size(14.);
        assert!(changed);
        accepted(&typography, changed);
        let changed = typography.apply_code_size(14.);
        assert!(!changed);
        accepted(&typography, changed);

        let changed = typography.apply_terminal_size(100.);
        assert!(changed);
        accepted(&typography, changed);
        let changed = typography.apply_terminal_size(32.);
        assert!(!changed);
        accepted(&typography, changed);
    }

    #[test]
    fn interface_family_change_without_effective_change_keeps_the_generation() {
        // The requested family is unavailable, so the embedded default renders.
        let catalog = standard_catalog();
        let settings = TypographySettings {
            interface_family: installed("Gone"),
            ..TypographySettings::defaults(&catalog)
        };
        let mut typography = Typography::new(catalog, settings);
        assert_eq!(
            typography.effective_family(TypographyRole::Interface),
            &embedded_choice("sans")
        );
        let generation = typography.generation();
        // Explicitly choosing what already renders changes the request only.
        assert!(typography.apply_family(TypographyRole::Interface, embedded_choice("sans")));
        assert_eq!(typography.generation(), generation);
        assert_eq!(
            typography.requested_family(TypographyRole::Interface),
            &embedded_choice("sans")
        );
    }

    #[test]
    fn generation_wraps_around() {
        let mut typography = standard_typography();
        typography.generation = u32::MAX;
        assert!(typography.apply_code_size(20.));
        assert_eq!(typography.generation(), 0);
    }

    struct Child {
        renders: Rc<Cell<usize>>,
    }

    impl Render for Child {
        fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div()
        }
    }

    struct Root {
        child: Entity<Child>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(self.child.clone().cached(crate::StyleRefinement::default()))
        }
    }

    fn draw(cx: &mut TestAppContext, window: crate::AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    // TXT-004 A7

    #[vespertine::test]
    fn interface_size_change_updates_rem_size_and_refreshes_every_window(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let catalog = standard_catalog();
            let settings = TypographySettings::defaults(&catalog);
            Typography::init(catalog, settings, cx);
        });
        let first_renders = Rc::new(Cell::new(0));
        let second_renders = Rc::new(Cell::new(0));
        let windows: Vec<_> = [first_renders.clone(), second_renders.clone()]
            .into_iter()
            .map(|renders| {
                let child = cx.new(|_| Child { renders });
                cx.add_window(|_, _| Root { child })
            })
            .collect();
        for window in &windows {
            draw(cx, (*window).into());
        }
        assert!(first_renders.get() > 0 && second_renders.get() > 0);
        let observed = Rc::new(Cell::new(0));
        let _subscription = cx.update(|cx| {
            let observed = observed.clone();
            cx.observe_global::<Typography>(move |_| observed.set(observed.get() + 1))
        });
        let (code_size, terminal_size, generation) = cx.read(|cx| {
            let typography = Typography::global(cx);
            (
                typography.code_size(),
                typography.terminal_size(),
                typography.generation(),
            )
        });

        // A no-op change neither notifies observers nor changes the state.
        let changed = cx
            .update_window(windows[0].into(), |_, window, cx| {
                Typography::set_interface_size(window, 16., cx)
            })
            .unwrap();
        assert!(!changed);
        assert_eq!(observed.get(), 0);
        cx.read(|cx| assert_eq!(Typography::global(cx).generation(), generation));
        for window in &windows {
            draw(cx, (*window).into());
        }
        let (first_before, second_before) = (first_renders.get(), second_renders.get());

        let changed = cx
            .update_window(windows[0].into(), |_, window, cx| {
                Typography::set_interface_size(window, 20., cx)
            })
            .unwrap();
        assert!(changed);
        for window in &windows {
            draw(cx, (*window).into());
        }
        assert_eq!(observed.get(), 1);
        assert!(first_renders.get() > first_before, "first window refreshed");
        assert!(
            second_renders.get() > second_before,
            "second window refreshed"
        );
        for window in &windows {
            let rem = cx
                .update_window((*window).into(), |_, window, _| window.rem_size())
                .unwrap();
            assert_eq!(rem, px(20.));
        }
        cx.read(|cx| {
            let typography = Typography::global(cx);
            assert_eq!(typography.interface_size(), 20.);
            assert_eq!(typography.code_size(), code_size);
            assert_eq!(typography.terminal_size(), terminal_size);
            assert_eq!(typography.generation(), generation.wrapping_add(1));
        });

        // Code and terminal size changes refresh too, without touching the root size.
        let before = first_renders.get();
        assert!(cx.update(|cx| Typography::set_code_size(14., cx)));
        assert!(cx.update(|cx| Typography::set_terminal_size(15., cx)));
        assert!(!cx.update(|cx| Typography::set_code_size(14., cx)));
        draw(cx, windows[0].into());
        assert!(first_renders.get() > before);
        assert_eq!(observed.get(), 3);
        cx.read(|cx| {
            let typography = Typography::global(cx);
            assert_eq!(typography.interface_size(), 20.);
            assert_eq!(typography.code_size(), 14.);
            assert_eq!(typography.terminal_size(), 15.);
            assert_eq!(typography.generation(), generation.wrapping_add(3));
        });
    }

    #[vespertine::test]
    fn family_changes_through_the_app_publish_and_refresh(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let catalog = standard_catalog();
            let settings = TypographySettings::defaults(&catalog);
            Typography::init(catalog, settings, cx);
        });
        let renders = Rc::new(Cell::new(0));
        let child = cx.new(|_| Child {
            renders: renders.clone(),
        });
        let window = cx.add_window(|_, _| Root { child });
        draw(cx, window.into());
        let before = renders.get();

        assert!(!cx.update(|cx| {
            Typography::set_family(TypographyRole::Terminal, installed("Installed Sans"), cx)
        }));
        assert!(cx.update(|cx| {
            Typography::set_family(TypographyRole::Terminal, installed("Installed Mono"), cx)
        }));
        draw(cx, window.into());
        assert!(renders.get() > before);
        cx.read(|cx| {
            let typography = Typography::global(cx);
            assert_eq!(
                typography
                    .effective_family_name(TypographyRole::Terminal)
                    .as_str(),
                "Installed Mono"
            );
            assert_eq!(
                typography.font(TypographyRole::Interface).family.as_str(),
                "Embedded Sans"
            );
        });

        let catalog = cx.read(|cx| {
            Typography::global(cx)
                .catalog()
                .rescan(&MockSource::default())
        });
        assert!(cx.update(|cx| Typography::set_catalog(catalog, cx)));
        cx.read(|cx| {
            assert_eq!(
                Typography::global(cx).effective_family(TypographyRole::Terminal),
                &embedded_choice("mono")
            );
        });
    }
}
