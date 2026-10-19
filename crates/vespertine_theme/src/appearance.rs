//! Live appearance resolution (THM-003 R5-R12): the appearance state global, pure resolution,
//! the style generation and the per-window reconciliation.

use crate::{
    accent::derive_accent,
    color::Color,
    hint::{TintRecipe, tint_variant},
    selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
    theme::{
        Appearance, IssueSeverity, SurfaceTreatment, ThemeFamily, ThemeRegistry, ThemeVariant,
        TypographyRoles,
    },
};
use std::{
    collections::HashMap,
    hash::Hash,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};
use vespertine::{App, Global, Subscription, Window, WindowBackgroundAppearance};

static STYLE_GENERATION: AtomicU32 = AtomicU32::new(0);
static CURRENT_DARK: AtomicBool = AtomicBool::new(false);

/// The process-wide style generation (unsigned 32-bit, wrapping, initially 0). It advances
/// whenever the resolved palette or UI typography changes; anything that caches resolved
/// colors or baked text styles must key on it (see [`StyleCache`]).
pub fn style_generation() -> u32 {
    STYLE_GENERATION.load(Ordering::Acquire)
}

fn advance_style_generation() -> u32 {
    STYLE_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1)
}

/// The effective appearance of the installed theme, for context-free paint helpers.
pub fn current_appearance() -> Appearance {
    if CURRENT_DARK.load(Ordering::Acquire) {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

/// The window background appearance every window is given: always composited with alpha so that
/// rounded chrome corners are genuinely transparent (THM-003 R16).
pub const WINDOW_BACKGROUND_APPEARANCE: WindowBackgroundAppearance =
    WindowBackgroundAppearance::Transparent;

/// Window-level glass alpha for a treatment. Desktop-level blur is not assumed (Linux), so
/// frosted window glass resolves to fully opaque regardless of the treatment (THM-003 R16); the
/// treatment itself is untouched and still drives in-window frosted surfaces.
pub fn window_glass_alpha(_treatment: SurfaceTreatment) -> f32 {
    1.0
}

/// Inputs of resolution (THM-003 R8).
#[derive(Clone, PartialEq, Debug)]
pub struct ResolutionInputs {
    /// The effective appearance.
    pub appearance: Appearance,
    /// The selected variant identifier for that appearance (may be unavailable).
    pub selected_id: String,
    /// Accent override.
    pub accent: AccentSelection,
    /// Surface override.
    pub surface: SurfacePreference,
    /// Image-derived hint, only while the feature is enabled.
    pub hint: Option<Color>,
    /// Typography roles.
    pub typography: TypographyRoles,
    /// A previewed candidate that replaces the registry lookup (THM-004 R8).
    pub preview: Option<Arc<ThemeVariant>>,
}

/// The resolved, active theme: a read-only snapshot installed as a global.
#[derive(Clone, Debug)]
pub struct ActiveTheme {
    /// The resolved variant: foreground-hardened if not curated, accent override and hint tint
    /// applied. `variant.surface` remains the variant's own recommendation.
    pub variant: ThemeVariant,
    /// The effective appearance this theme was resolved for.
    pub effective_appearance: Appearance,
    /// The resolved surface treatment (preference applied).
    pub treatment: SurfaceTreatment,
    /// Typography roles (THM-003 R18).
    pub typography: TypographyRoles,
    /// The style generation this theme was installed at.
    pub generation: u32,
    /// Whether this is a preview of an uninstalled candidate.
    pub is_preview: bool,
}

impl Deref for ActiveTheme {
    type Target = ThemeVariant;

    fn deref(&self) -> &ThemeVariant {
        &self.variant
    }
}

impl ActiveTheme {
    /// The effective window glass alpha: always 1 on Linux (THM-003 R16).
    pub fn window_glass_alpha(&self) -> f32 {
        window_glass_alpha(self.treatment)
    }

    /// Checks that the resolved palette is complete and valid: no structural problem and no
    /// contrast error (a per-frame test hook uses this to catch partial or invalid states).
    pub fn check_complete(&self) -> Result<(), String> {
        let family = ThemeFamily {
            id: self.variant.family_id.clone(),
            name: self.variant.name.clone(),
            variants: vec![self.variant.clone()],
        };
        let problems: Vec<String> = family
            .validate()
            .into_iter()
            .filter(|issue| issue.severity == IssueSeverity::Error)
            .map(|issue| issue.message)
            .collect();
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }
}

/// Pure resolution of the inputs over a registry (THM-003 R8, R9).
pub fn resolve_theme(registry: &ThemeRegistry, inputs: &ResolutionInputs) -> ActiveTheme {
    let base = match &inputs.preview {
        Some(candidate) => candidate.as_ref(),
        None => registry.resolve_id(&inputs.selected_id, inputs.appearance),
    };
    let mut variant = base.hardened();
    if let Some(hint) = inputs.hint {
        variant = tint_variant(&variant, hint, &TintRecipe::default());
    } else if let AccentSelection::Color(color) = inputs.accent {
        variant.accent = derive_accent(color, variant.colors.background, variant.appearance);
    }
    ActiveTheme {
        treatment: inputs.surface.resolve(base.surface),
        effective_appearance: inputs.appearance,
        typography: inputs.typography.clone(),
        generation: 0,
        is_preview: inputs.preview.is_some(),
        variant,
    }
}

/// The appearance state global (THM-003 R5-R12).
pub struct AppearanceState {
    mode: AppearanceMode,
    platform: Appearance,
    selection: ThemeSelection,
    accent: AccentSelection,
    surface: SurfacePreference,
    hint: Option<Color>,
    hint_enabled: bool,
    typography: TypographyRoles,
    registry: ThemeRegistry,
    preview: Option<Arc<ThemeVariant>>,
    installed: ResolutionInputs,
    active: ActiveTheme,
}

impl Global for AppearanceState {}

impl AppearanceState {
    fn inputs(&self) -> ResolutionInputs {
        let appearance = self.mode.effective(self.platform);
        ResolutionInputs {
            appearance,
            selected_id: self.selection.variant_id(appearance).to_string(),
            accent: self.accent,
            surface: self.surface,
            hint: self.hint.filter(|_| self.hint_enabled),
            typography: self.typography.clone(),
            preview: self.preview.clone(),
        }
    }

    /// Installs the resolution of the current inputs and advances the generation.
    fn install(&mut self) {
        let inputs = self.inputs();
        let mut active = resolve_theme(&self.registry, &inputs);
        active.generation = advance_style_generation();
        CURRENT_DARK.store(inputs.appearance.is_dark(), Ordering::Release);
        self.installed = inputs;
        self.active = active;
    }

    /// The installed (resolved) theme.
    pub fn theme(&self) -> &ActiveTheme {
        &self.active
    }
}

/// Initializes the appearance state. Must be called before any window opens, so that the first
/// frame of every window already sees the resolved theme (THM-003 R12).
pub fn init(
    cx: &mut App,
    mode: AppearanceMode,
    selection: ThemeSelection,
    accent: AccentSelection,
    surface: SurfacePreference,
) {
    let platform = Appearance::from(cx.window_appearance());
    let registry = ThemeRegistry::builtin();
    let typography = cx
        .try_global::<vespertine::Typography>()
        .map_or_else(TypographyRoles::default, TypographyRoles::from_typography);
    let inputs = ResolutionInputs {
        appearance: mode.effective(platform),
        selected_id: String::new(),
        accent,
        surface,
        hint: None,
        typography: typography.clone(),
        preview: None,
    };
    let active = resolve_theme(&registry, &inputs);
    let mut state = AppearanceState {
        mode,
        platform,
        selection,
        accent,
        surface,
        hint: None,
        hint_enabled: true,
        typography,
        registry,
        preview: None,
        installed: inputs,
        active,
    };
    state.install();
    cx.set_global(state);
    if cx.has_global::<vespertine::Typography>() {
        bind_typography(cx).detach();
    }
}

/// Follows the TXT-004 typography global: the theme carries its families and sizes, so a
/// typography change is a theme change (THM-003 R18). The roles are copied now and again
/// whenever the `Typography` global is published. [`init`] binds automatically when the
/// global already exists; call this after initializing typography otherwise. Keep the
/// returned subscription for as long as the binding should last (or `detach` it).
pub fn bind_typography(cx: &mut App) -> Subscription {
    sync_typography(cx);
    cx.observe_global::<vespertine::Typography>(sync_typography)
}

fn sync_typography(cx: &mut App) {
    let roles = TypographyRoles::from_typography(vespertine::Typography::global(cx));
    set_typography(cx, roles);
}

fn state(cx: &App) -> &AppearanceState {
    cx.try_global::<AppearanceState>()
        .expect("vespertine_theme::init must run before the theme is used")
}

/// The active theme (`theme of context`). Panics if [`init`] has not run.
pub fn theme_of(cx: &App) -> &ActiveTheme {
    &state(cx).active
}

/// The active theme, or `None` before [`init`].
pub fn try_theme_of(cx: &App) -> Option<&ActiveTheme> {
    cx.try_global::<AppearanceState>()
        .map(|state| &state.active)
}

/// The active registry: built-in plus installed custom families.
pub fn registry(cx: &App) -> &ThemeRegistry {
    &state(cx).registry
}

/// The stored appearance mode.
pub fn mode(cx: &App) -> AppearanceMode {
    state(cx).mode
}

/// The appearance last reported by the platform (recorded even while a mode is pinned).
pub fn platform_appearance(cx: &App) -> Appearance {
    state(cx).platform
}

/// The effective appearance (mode applied to the recorded platform appearance).
pub fn effective_appearance(cx: &App) -> Appearance {
    let state = state(cx);
    state.mode.effective(state.platform)
}

/// The stored selection (it may name unavailable themes; it is never rewritten by resolution).
pub fn selection(cx: &App) -> &ThemeSelection {
    &state(cx).selection
}

/// The stored accent selection.
pub fn accent(cx: &App) -> AccentSelection {
    state(cx).accent
}

/// The stored surface preference.
pub fn surface(cx: &App) -> SurfacePreference {
    state(cx).surface
}

/// The stored typography roles.
pub fn typography(cx: &App) -> &TypographyRoles {
    &state(cx).typography
}

/// The image-derived hint and whether the feature is enabled.
pub fn hint(cx: &App) -> (Option<Color>, bool) {
    let state = state(cx);
    (state.hint, state.hint_enabled)
}

/// Registers a callback run whenever the appearance state changes (the change hook).
pub fn observe_changes(cx: &mut App, mut callback: impl FnMut(&mut App) + 'static) -> Subscription {
    cx.observe_global::<AppearanceState>(move |cx| callback(cx))
}

fn update(cx: &mut App, change: impl FnOnce(&mut AppearanceState)) {
    change(cx.global_mut::<AppearanceState>());
    apply(cx);
}

/// Sets the mode.
pub fn set_mode(cx: &mut App, mode: AppearanceMode) {
    update(cx, |state| state.mode = mode);
}

/// Stores the variant identifier for one appearance. The stored identifier may name a theme
/// that is not (yet) registered.
pub fn set_theme(cx: &mut App, appearance: Appearance, id: impl Into<String>) {
    let id = id.into();
    update(cx, |state| state.selection.set_variant(appearance, id));
}

/// Replaces the whole stored selection.
pub fn set_selection(cx: &mut App, selection: ThemeSelection) {
    update(cx, |state| state.selection = selection);
}

/// Sets the accent selection.
pub fn set_accent(cx: &mut App, accent: AccentSelection) {
    update(cx, |state| state.accent = accent);
}

/// Sets the surface preference.
pub fn set_surface(cx: &mut App, surface: SurfacePreference) {
    update(cx, |state| state.surface = surface);
}

/// Sets the typography roles (a typography change is a theme change, THM-003 R18).
pub fn set_typography(cx: &mut App, typography: TypographyRoles) {
    update(cx, |state| state.typography = typography);
}

/// Sets or clears the image-derived hint (THM-004 R15/R16).
pub fn set_hint(cx: &mut App, hint: Option<Color>) {
    update(cx, |state| state.hint = hint);
}

/// Enables or disables the hint tint; the stored hint is kept.
pub fn set_hint_enabled(cx: &mut App, enabled: bool) {
    update(cx, |state| state.hint_enabled = enabled);
}

/// Records the platform's reported appearance (always recorded, even while a mode is pinned)
/// and re-resolves.
pub fn set_platform_appearance(cx: &mut App, appearance: Appearance) {
    update(cx, |state| state.platform = appearance);
}

/// Replaces the preview candidate (THM-004 R8). The swap is one resolution.
pub(crate) fn set_preview(cx: &mut App, preview: Option<Arc<ThemeVariant>>) {
    update(cx, |state| state.preview = preview);
}

/// The previewed candidate, if any.
pub(crate) fn preview(cx: &App) -> Option<&Arc<ThemeVariant>> {
    state(cx).preview.as_ref()
}

/// Re-resolves from the stored inputs (THM-003 R11). When nothing in the resolution inputs
/// moved nothing is replaced and no window is refreshed, but the window background appearance
/// is still reapplied to every window.
pub fn apply(cx: &mut App) {
    apply_inner(cx, false);
}

/// Replaces the custom families of the registry (built-in plus `families`, in order), repoints
/// the stored selection of every appearance whose identifier is in `removed` (explicit removal,
/// THM-003 R9), and reinstalls with a forced generation advance, because a linked theme keeps
/// its identifier across a reload while its colors change (THM-003 R10). Returns whether the
/// stored selection was repointed; the host then persists it.
pub fn apply_registry_change(cx: &mut App, families: Vec<ThemeFamily>, removed: &[String]) -> bool {
    let state = cx.global_mut::<AppearanceState>();
    state.registry = ThemeRegistry::with_custom(families);
    let repointed = state.selection.repoint_removed(&state.registry, removed);
    apply_inner(cx, true);
    repointed
}

/// Ends a preview, installs `families` as the custom registry and optionally stores a selection
/// in ONE resolution (THM-004 R8: no intermediate half-replaced state).
pub(crate) fn commit_install(
    cx: &mut App,
    families: Vec<ThemeFamily>,
    select: Option<(Appearance, String)>,
) {
    let state = cx.global_mut::<AppearanceState>();
    state.registry = ThemeRegistry::with_custom(families);
    state.preview = None;
    if let Some((appearance, id)) = select {
        state.selection.set_variant(appearance, id);
    }
    apply_inner(cx, true);
}

fn apply_inner(cx: &mut App, force: bool) {
    let current = state(cx);
    let moved = force || current.inputs() != current.installed;
    if moved {
        cx.global_mut::<AppearanceState>().install();
        // Colors are read imperatively at paint time: only refreshing every window (which also
        // disables per-view paint caching for the next frame) repaints laid-out elements.
        cx.refresh_windows();
    }
    reapply_window_backgrounds(cx);
}

/// Reapplies the window background appearance to every window. Deferred until every window is
/// back in the application, because the window handling a settings click is taken out while
/// its handler runs.
pub fn reapply_window_backgrounds(cx: &mut App) {
    cx.defer(|cx| {
        for handle in cx.windows() {
            handle
                .update(cx, |_, window, _| {
                    window.set_background_appearance(WINDOW_BACKGROUND_APPEARANCE)
                })
                .ok();
        }
    });
}

/// Reconciles a window with the theme state and subscribes to its appearance changes
/// (THM-003 R12). Call it when the window is created, before its first draw: the platform's
/// application-level appearance can be unreliable very early in startup, so each window
/// reports its OWN appearance. The notification is per window but the value is a system
/// setting, so any one window suffices and re-applying is idempotent.
pub fn observe_window(window: &mut Window, cx: &mut App) {
    window.set_background_appearance(WINDOW_BACKGROUND_APPEARANCE);
    set_platform_appearance(cx, window.appearance().into());
    window
        .observe_window_appearance(|window, cx| {
            set_platform_appearance(cx, window.appearance().into());
        })
        .detach();
}

/// A cache of values derived from resolved colors or text styles. It keys on the style
/// generation and discards every entry when the generation moves (THM-003 R10).
pub struct StyleCache<K, V> {
    generation: u32,
    entries: HashMap<K, V>,
}

impl<K: Eq + Hash, V> Default for StyleCache<K, V> {
    fn default() -> Self {
        Self {
            generation: style_generation(),
            entries: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash, V> StyleCache<K, V> {
    /// An empty cache at the current generation.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards entries if the generation moved; returns the number discarded.
    pub fn sync(&mut self) -> usize {
        let generation = style_generation();
        if generation == self.generation {
            return 0;
        }
        self.generation = generation;
        let discarded = self.entries.len();
        self.entries.clear();
        discarded
    }

    /// Returns the cached value for `key`, computing it with `compute` if absent or stale.
    pub fn get_or_insert_with(&mut self, key: K, compute: impl FnOnce() -> V) -> &V {
        self.sync();
        self.entries.entry(key).or_insert_with(compute)
    }

    /// Number of live entries (after dropping stale ones).
    pub fn len(&mut self) -> usize {
        self.sync();
        self.entries.len()
    }

    /// Whether the cache holds no live entries.
    pub fn is_empty(&mut self) -> bool {
        self.len() == 0
    }
}
