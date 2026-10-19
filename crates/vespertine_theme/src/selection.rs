//! User selections (THM-003 R5-R7).

use crate::{
    color::Color,
    theme::{Appearance, DEFAULT_DARK_ID, DEFAULT_LIGHT_ID, SurfaceTreatment, ThemeRegistry},
};
use serde::{Deserialize, Serialize};

/// How the effective appearance is chosen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    /// Follow the appearance last reported by the platform.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl AppearanceMode {
    /// The effective appearance given the last platform report.
    pub fn effective(self, platform: Appearance) -> Appearance {
        match self {
            AppearanceMode::System => platform,
            AppearanceMode::Light => Appearance::Light,
            AppearanceMode::Dark => Appearance::Dark,
        }
    }
}

/// One variant identifier for light and one for dark.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ThemeSelection {
    /// Variant used while the effective appearance is light.
    pub light: String,
    /// Variant used while the effective appearance is dark.
    pub dark: String,
}

impl Default for ThemeSelection {
    fn default() -> Self {
        Self {
            light: DEFAULT_LIGHT_ID.to_string(),
            dark: DEFAULT_DARK_ID.to_string(),
        }
    }
}

impl ThemeSelection {
    /// The stored identifier for an appearance (it may name an unavailable theme).
    pub fn variant_id(&self, appearance: Appearance) -> &str {
        match appearance {
            Appearance::Light => &self.light,
            Appearance::Dark => &self.dark,
        }
    }

    /// Stores the identifier for an appearance.
    pub fn set_variant(&mut self, appearance: Appearance, id: impl Into<String>) {
        match appearance {
            Appearance::Light => self.light = id.into(),
            Appearance::Dark => self.dark = id.into(),
        }
    }

    /// Repoints the stored identifier of every appearance that names one of the explicitly
    /// `removed` variants (and is no longer registered) to the first variant of that appearance
    /// in registry order (THM-003 R9). Unrelated unavailable selections are preserved. Returns
    /// whether anything changed, in which case the host persists the selection.
    pub fn repoint_removed(&mut self, registry: &ThemeRegistry, removed: &[String]) -> bool {
        let mut changed = false;
        for appearance in [Appearance::Light, Appearance::Dark] {
            let current = self.variant_id(appearance);
            if removed.iter().any(|id| id == current) && registry.variant(current).is_none() {
                let first = registry.first_variant(appearance).id.clone();
                self.set_variant(appearance, first);
                changed = true;
            }
        }
        changed
    }
}

/// Accent override.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccentSelection {
    /// Use the variant's own accent roles.
    #[default]
    ThemeDefault,
    /// Replace the variant's accent roles by derivation (THM-003 R13).
    Color(Color),
}

/// Surface override.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SurfacePreference {
    /// Use the variant's recommendation.
    #[default]
    ThemeDefault,
    /// Frosted surfaces.
    Frosted,
    /// Opaque surfaces.
    Opaque,
}

impl SurfacePreference {
    /// Resolves against the variant's recommended treatment.
    pub fn resolve(self, recommended: SurfaceTreatment) -> SurfaceTreatment {
        match self {
            SurfacePreference::ThemeDefault => recommended,
            SurfacePreference::Frosted => SurfaceTreatment::Frosted,
            SurfacePreference::Opaque => SurfaceTreatment::Opaque,
        }
    }
}
