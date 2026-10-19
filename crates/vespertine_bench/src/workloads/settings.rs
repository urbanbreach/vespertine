//! Settings navigation and theme changes (R4).

use super::{BORDER, INK, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use vespertine::prelude::FluentBuilder as _;
use vespertine::{
    AnyView, AppContext as _, Context, Entity, FontWeight, Global, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div, px,
};

const PAGES: usize = 12;
const SETTINGS_PER_PAGE: usize = 8;

/// The theme every view reads while rendering.
#[derive(Clone, Copy)]
struct ThemeMode {
    dark: bool,
}

impl Global for ThemeMode {}

impl ThemeMode {
    fn background(self) -> u32 {
        if self.dark { 0x0d1117 } else { WHITE }
    }

    fn panel(self) -> u32 {
        if self.dark { 0x161b22 } else { SURFACE }
    }

    fn text(self) -> u32 {
        if self.dark { 0xe6edf3 } else { INK }
    }
}

struct Navigation {
    page: usize,
}

struct SidebarEntry {
    index: usize,
    navigation: Entity<Navigation>,
}

impl Render for SidebarEntry {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<ThemeMode>();
        let selected = self.navigation.read(cx).page == self.index;
        div()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .text_color(hex(theme.text()))
            .bg(hex(if selected { 0x0969da } else { theme.panel() }))
            .when(selected, |entry| {
                entry.text_color(hex(WHITE)).font_weight(FontWeight::BOLD)
            })
            .child(format!("Settings page {}", self.index + 1))
    }
}

struct Page {
    index: usize,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<ThemeMode>();
        div()
            .size_full()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .bg(hex(theme.background()))
            .text_color(hex(theme.text()))
            .child(
                div()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Page {} settings", self.index + 1)),
            )
            .children((0..SETTINGS_PER_PAGE).map(|setting| {
                div()
                    .h(px(34.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(hex(BORDER))
                    .child(format!("Option {} of page {}", setting + 1, self.index + 1))
                    .child(div().w(px(34.0)).h(px(18.0)).rounded(px(9.0)).bg(hex(
                        if (setting + self.index).is_multiple_of(2) {
                            0x1a7f37
                        } else {
                            BORDER
                        },
                    )))
            }))
    }
}

struct Content {
    navigation: Entity<Navigation>,
    pages: Vec<Entity<Page>>,
}

impl Render for Content {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.navigation.read(cx).page;
        div().flex_1().h_full().child(self.pages[page].clone())
    }
}

struct SettingsRoot {
    entries: Vec<Entity<SidebarEntry>>,
    content: Entity<Content>,
}

impl Render for SettingsRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<ThemeMode>();
        div()
            .size_full()
            .flex()
            .font_family(SANS)
            .text_size(px(14.0))
            .bg(hex(theme.background()))
            .child(
                div()
                    .w(px(200.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .children(self.entries.iter().cloned()),
            )
            .child(self.content.clone())
    }
}

struct SettingsDriver {
    navigation: Entity<Navigation>,
    /// Navigates every frame; the theme changes every 20 frames when `themes` is set.
    themes: bool,
}

impl Driver for SettingsDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let page = (frame as usize * 5 + 1) % PAGES;
        let flip_theme = self.themes && frame % 20 == 19;
        let navigation = &self.navigation;
        stage.mutate(|app| {
            if flip_theme {
                let dark = !app.global::<ThemeMode>().dark;
                app.set_global(ThemeMode { dark });
            }
            navigation.update(app, |navigation, cx| {
                navigation.page = page;
                cx.notify();
            });
        });
    }
}

fn setup(themes: bool) -> impl Fn(&mut dyn Host) -> Result<Box<dyn Driver>> {
    move |harness| {
        harness.update_app(|cx| cx.set_global(ThemeMode { dark: false }));
        let mut navigation = None;
        harness.open_window(|_, cx| {
            let nav = cx.new(|_| Navigation { page: 0 });
            let entries = (0..PAGES)
                .map(|index| {
                    let navigation = nav.clone();
                    cx.new(|_| SidebarEntry { index, navigation })
                })
                .collect();
            let pages = (0..PAGES).map(|index| cx.new(|_| Page { index })).collect();
            let content = cx.new(|_| Content {
                navigation: nav.clone(),
                pages,
            });
            navigation = Some(nav);
            AnyView::from(cx.new(|_| SettingsRoot { entries, content }))
        })?;
        Ok(Box::new(SettingsDriver {
            navigation: navigation.expect("the window built the navigation"),
            themes,
        }))
    }
}

/// The settings workloads.
pub fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "settings-navigation",
            family: Family::Settings,
            description: "the settings sidebar navigates to another page every frame",
            warmup: 30,
            frames: 200,
            setup: |harness| setup(false)(harness),
        },
        Workload {
            name: "settings-theme",
            family: Family::Settings,
            description: "navigation every frame and a light/dark theme change every 20 frames",
            warmup: 30,
            frames: 200,
            setup: |harness| setup(true)(harness),
        },
    ]
}
