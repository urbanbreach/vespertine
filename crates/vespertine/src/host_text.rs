//! The selectable-text seam of host adapters (ELM-007 R10).
//!
//! The window-wide selection model (TXT-008) lives in `vespertine_richtext`, which depends on this
//! crate, so adapters reach it through a [`HostTextProvider`] carried by the
//! [`RenderContext`](crate::RenderContext). `vespertine_richtext::RichHostText` is the provider
//! backed by the real selection registry. Without a provider, text is painted as plain text with
//! no selection support.

use crate::{AnyElement, App, HostId, IntoElement, SharedString, StyledText, TextRun, Window};

/// Builds the text elements of host adapters and exposes the live text selection.
pub trait HostTextProvider {
    /// A selectable run identified by `host` plus `sub_index`, which the adapter keeps stable
    /// across frames for the same logical run. It registers with the window selection registry
    /// while painting. When `selectable` is false (the host sits in a subtree that disabled
    /// selection) the text is painted and logged but not registered.
    fn selectable_run(
        &self,
        host: HostId,
        sub_index: u32,
        selectable: bool,
        text: SharedString,
        runs: Option<Vec<TextRun>>,
    ) -> AnyElement;

    /// Chrome text (line numbers, language tags, file headers): painted and logged, never part of
    /// a selection, so copied text never contains it.
    fn chrome_run(
        &self,
        host: HostId,
        sub_index: u32,
        text: SharedString,
        runs: Option<Vec<TextRun>>,
    ) -> AnyElement;

    /// The joined selected text, or `None` when nothing is selected.
    fn selected_text(&self) -> Option<String>;

    /// Removes the selection and its washes.
    fn clear_selection(&self, window: &mut Window, cx: &mut App);
}

/// Plain text without selection support: used when a context carries no provider.
pub(crate) fn plain_text(text: SharedString, runs: Option<Vec<TextRun>>) -> AnyElement {
    let styled = StyledText::new(text);
    match runs {
        Some(runs) => styled.with_runs(runs).into_any_element(),
        None => styled.into_any_element(),
    }
}
