//! Preview and install transaction (THM-004 R8).
//!
//! A preview installs an uninstalled candidate as the active theme through the normal
//! resolution, without touching the registry, the persisted selection or the library. Cancel
//! restores the committed theme through the same path; commit installs the candidate into the
//! library and swaps the active theme in one resolution, so no half-replaced state is ever
//! observable.

use crate::{
    appearance,
    import::Compilation,
    library::{InstallMode, Library, LibraryError},
    theme::{ThemeFamily, ThemeVariant, has_blocking_issue},
};
use std::sync::Arc;
use vespertine::{App, Entity, Subscription};

/// Which candidate is being previewed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PreviewInfo {
    /// The candidate's family.
    pub family_id: String,
    /// The previewed variant.
    pub variant_id: String,
}

/// Previews one variant of an uninstalled candidate family: `variant_id` if given, else the
/// first variant of the effective appearance, else the first variant. A candidate with a
/// blocking validation issue is rejected and the active theme is left untouched.
pub fn preview(
    cx: &mut App,
    family: &ThemeFamily,
    variant_id: Option<&str>,
) -> Result<PreviewInfo, LibraryError> {
    let effective = appearance::effective_appearance(cx);
    let variant = match variant_id {
        Some(id) => family
            .variant(id)
            .ok_or_else(|| LibraryError(format!("the candidate has no variant `{id}`")))?,
        None => family
            .variants
            .iter()
            .find(|variant| variant.appearance == effective)
            .or_else(|| family.variants.first())
            .ok_or_else(|| LibraryError("the candidate has no variants".to_string()))?,
    };
    preview_variant(cx, variant)
}

/// Previews a single candidate variant.
pub fn preview_variant(cx: &mut App, variant: &ThemeVariant) -> Result<PreviewInfo, LibraryError> {
    let issues = crate::theme::validate_variants(std::iter::once(variant));
    if has_blocking_issue(&issues) {
        let messages: Vec<String> = issues
            .iter()
            .filter(|issue| issue.is_blocking())
            .map(ToString::to_string)
            .collect();
        return Err(LibraryError(format!(
            "the candidate cannot be previewed: {}",
            messages.join("; ")
        )));
    }
    let info = PreviewInfo {
        family_id: variant.family_id.clone(),
        variant_id: variant.id.clone(),
    };
    appearance::set_preview(cx, Some(Arc::new(variant.clone())));
    Ok(info)
}

/// Whether the active theme is a preview rather than a committed choice.
pub fn is_previewing(cx: &App) -> bool {
    appearance::preview(cx).is_some()
}

/// The previewed candidate, if the active theme is a preview.
pub fn preview_info(cx: &App) -> Option<PreviewInfo> {
    appearance::preview(cx).map(|variant| PreviewInfo {
        family_id: variant.family_id.clone(),
        variant_id: variant.id.clone(),
    })
}

/// Cancels a preview and restores the committed theme through the normal resolution. Returns
/// whether a preview was active.
pub fn cancel(cx: &mut App) -> bool {
    if !is_previewing(cx) {
        return false;
    }
    appearance::set_preview(cx, None);
    true
}

/// Cancels any preview when `owner` is released (closing the owner restores the committed
/// theme). Keep the returned subscription alive as long as the owner.
pub fn cancel_on_release<T: 'static>(cx: &mut App, owner: &Entity<T>) -> Subscription {
    cx.observe_release(owner, |_, cx| {
        cancel(cx);
    })
}

/// The identifier a variant gets when its family `old_family` is re-keyed to `new_family`.
pub fn rekeyed_variant_id(old_family: &str, new_family: &str, variant_id: &str) -> String {
    match variant_id.strip_prefix(old_family) {
        Some(rest) => format!("{new_family}{rest}"),
        None => format!("{new_family}-{variant_id}"),
    }
}

/// Commits a candidate: installs it into `library` (persisting atomically), installs the
/// runtime registry, ends any preview and, when `select_variant` names a variant of the
/// compilation, stores it as the selection of its appearance — all in one resolution. Any
/// failure leaves the active theme, the preview and the library untouched. Returns the entry
/// identifier.
pub fn commit(
    cx: &mut App,
    library: &mut Library,
    compilation: &Compilation,
    selected: &[String],
    mode: InstallMode,
    select_variant: Option<&str>,
) -> Result<String, LibraryError> {
    let id = library.install(compilation, selected, mode)?;
    let select = select_variant.and_then(|variant_id| {
        let rekeyed = rekeyed_variant_id(&compilation.family.id, &id, variant_id);
        let variant = library.entry(&id)?.family.variant(&rekeyed)?;
        Some((variant.appearance, rekeyed))
    });
    appearance::commit_install(cx, library.families(), select);
    Ok(id)
}
