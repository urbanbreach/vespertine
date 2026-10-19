//! The selectable-text seam of custom-element adapters (ELM-007 R10), backed by the window's
//! TXT-008 selection registry.

use crate::{RichText, RunKey, TextService, UserSelect, user_select};
use vespertine::{
    AnyElement, App, ElementId, HostId, HostTextProvider, IntoElement, SharedString, TextRun,
    Window,
};

/// A [`HostTextProvider`] whose runs register with the text service of the window being painted.
///
/// A selectable run is keyed by the host identity plus the adapter's stable sub-index, so the same
/// run keeps its identity across frames (a drag that started elsewhere still sees its glyphs).
/// Chrome runs paint through the chrome path: logged, never registered, never copied. A run whose
/// host disabled selection is wrapped in a `user-select: none` scope.
///
/// Selection queries use the service of the window passed to [`HostTextProvider::clear_selection`]
/// or, for [`HostTextProvider::selected_text`], the service of the root currently painting; hold a
/// [`TextService`] directly for window-level queries outside painting.
#[derive(Clone, Copy, Debug, Default)]
pub struct RichHostText;

fn run_id(kind: &'static str, host: HostId, sub_index: u32) -> ElementId {
    ElementId::NamedInteger(
        SharedString::from(format!("{kind}-{}", host.0)),
        u64::from(sub_index),
    )
}

/// The registry key of the selectable run `(host, sub_index)`.
pub fn host_run_key(host: HostId, sub_index: u32) -> RunKey {
    RunKey::new(host.0, sub_index)
}

impl HostTextProvider for RichHostText {
    fn selectable_run(
        &self,
        host: HostId,
        sub_index: u32,
        selectable: bool,
        text: SharedString,
        runs: Option<Vec<TextRun>>,
    ) -> AnyElement {
        let mut rich = RichText::new(run_id("host-text", host, sub_index), text.to_string())
            .sub_index(sub_index)
            .key(host_run_key(host, sub_index))
            .native();
        if let Some(runs) = runs {
            rich = rich.runs(runs);
        }
        if selectable {
            rich.into_any_element()
        } else {
            user_select(UserSelect::None, rich).into_any_element()
        }
    }

    fn chrome_run(
        &self,
        host: HostId,
        sub_index: u32,
        text: SharedString,
        runs: Option<Vec<TextRun>>,
    ) -> AnyElement {
        let mut rich = RichText::new(run_id("host-chrome", host, sub_index), text.to_string())
            .sub_index(sub_index)
            .chrome();
        if let Some(runs) = runs {
            rich = rich.runs(runs);
        }
        rich.into_any_element()
    }

    fn selected_text(&self) -> Option<String> {
        TextService::current()?.selected_text()
    }

    fn clear_selection(&self, window: &mut Window, cx: &mut App) {
        TextService::of(window, cx).clear_selection(window, cx);
    }
}
