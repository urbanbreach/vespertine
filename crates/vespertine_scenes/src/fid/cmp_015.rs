//! FID-CMP-015-1 (the native unified patch viewer) and FID-CMP-015-2 (the composed comparison
//! viewer in split mode).

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{
    AnyElement, App, Entity, IntoElement, ParentElement, Rgba, Styled, Window, div, px, rgb,
};
use vespertine_components::diff::compare::{ComparisonHunk, ComparisonView};
use vespertine_components::diff::state::DiffViewState;
use vespertine_components::diff::view::DiffView;
use vespertine_theme::document::DocumentTheme;

const PATCH: &str = "diff --git a/sample.txt b/sample.txt\n--- a/sample.txt\n+++ b/sample.txt\n@@ -1 +1 @@\n-old value\n+new value\n";

/// The theme tokens of FID-CMP-015-1.
fn theme() -> Arc<DocumentTheme> {
    let mut theme = DocumentTheme::dark();
    theme.bg = rgb(0x101010);
    theme.text = rgb(0xeeeeee);
    theme.text_dim = rgb(0x999999);
    theme.text_faint = rgb(0x777777);
    theme.diff_add = rgb(0x00cc88);
    theme.diff_del = rgb(0xff5555);
    theme.diff_hunk_bg = Rgba {
        r: 64. / 255.,
        g: 96. / 255.,
        b: 128. / 255.,
        a: 32. / 255.,
    };
    theme.font_mono = "Liberation Mono".into();
    theme.font_sans = "Liberation Mono".into();
    Arc::new(theme)
}

struct Cmp0151 {
    state: Option<Entity<DiffViewState>>,
}

impl Scene for Cmp0151 {
    fn size(&self) -> (f32, f32) {
        (700.0, 300.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, cx: &mut App) -> AnyElement {
        let state = self
            .state
            .get_or_insert_with(|| DiffViewState::new(cx))
            .clone();
        let viewer = DiffView::new(&state)
            .patch(PATCH)
            .theme(theme())
            .absolute()
            .left(px(20.))
            .top(px(20.))
            .w(px(660.))
            .h(px(114.));
        div()
            .size_full()
            .bg(hex(0x181818))
            .child(viewer)
            .into_any_element()
    }
}

register_scene!("FID-CMP-015-1", || Cmp0151 { state: None });

struct Cmp0152;

impl Scene for Cmp0152 {
    fn size(&self) -> (f32, f32) {
        (700.0, 240.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let hunk = ComparisonHunk {
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 2,
            lines: vec![
                "-same old".to_owned(),
                "+same new".to_owned(),
                " unchanged".to_owned(),
            ],
        };
        let viewer = ComparisonView::new(vec![hunk])
            .split_view(true)
            .font_family("Liberation Mono")
            .font_size(px(13.))
            .line_height(px(20.))
            .absolute()
            .left(px(20.))
            .top(px(20.))
            .w(px(660.));
        div()
            .size_full()
            .bg(hex(0x181818))
            .child(viewer)
            .into_any_element()
    }
}

register_scene!("FID-CMP-015-2", || Cmp0152);
