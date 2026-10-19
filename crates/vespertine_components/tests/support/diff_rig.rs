//! A test rig around the offscreen host: a diff viewer whose properties are read from a shared
//! configuration on every frame, with its state entity and an event log.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, Entity, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    px, rgb,
};
use vespertine_components::diff::rows::{Annotation, DiffMode};
use vespertine_components::diff::state::DiffViewState;
use vespertine_components::diff::view::{DiffView, DraftEditor};
use vespertine_richtext::search::{HighlightDeclaration, HighlightSpec};
use vespertine_richtext::{TextService, highlight, text_root};
use vespertine_test_host::{HostOptions, TestHost};
use vespertine_theme::document::DocumentTheme;

/// Viewer properties the element closure reads on every frame.
#[derive(Clone)]
pub struct Config {
    pub patch: String,
    pub word_diff: bool,
    pub scroll: bool,
    pub max_lines: Option<usize>,
    pub collapsed: Option<Vec<String>>,
    pub mode: DiffMode,
    pub wrap: bool,
    pub annotations: Vec<Annotation>,
    pub theme: Arc<DocumentTheme>,
    pub sticky: bool,
    pub frosted: bool,
    pub subscribe: bool,
    pub editor: bool,
    pub draft_key: u64,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub file_cap: Option<usize>,
    pub syntax: bool,
    pub search: Option<String>,
    pub editor_slot: Rc<RefCell<Option<DraftEditor>>>,
}

impl Config {
    pub fn new(patch: impl Into<String>, width: f32, height: f32) -> Self {
        let mut theme = DocumentTheme::dark();
        theme.font_mono = "Liberation Mono".into();
        theme.font_sans = "Liberation Mono".into();
        Self {
            patch: patch.into(),
            word_diff: false,
            scroll: true,
            max_lines: None,
            collapsed: None,
            mode: DiffMode::Unified,
            wrap: false,
            annotations: Vec::new(),
            theme: Arc::new(theme),
            sticky: false,
            frosted: false,
            subscribe: true,
            editor: false,
            draft_key: 0,
            left: 0.,
            top: 0.,
            width,
            height,
            file_cap: None,
            syntax: true,
            search: None,
            editor_slot: Rc::default(),
        }
    }
}

pub type Log = Rc<RefCell<Vec<String>>>;

pub struct Rig {
    pub host: TestHost,
    pub config: Rc<RefCell<Config>>,
    pub state: Rc<RefCell<Option<Entity<DiffViewState>>>>,
    pub log: Log,
}

impl Rig {
    pub fn new(config: Config, viewport: (f64, f64)) -> Self {
        let config = Rc::new(RefCell::new(config));
        let state: Rc<RefCell<Option<Entity<DiffViewState>>>> = Rc::default();
        let log: Log = Rc::default();
        let (c, s, l) = (config.clone(), state.clone(), log.clone());
        let host = TestHost::with_element(
            HostOptions::with_size(viewport.0, viewport.1),
            move |_window: &mut Window, cx: &mut App| -> AnyElement {
                let entity = s
                    .borrow_mut()
                    .get_or_insert_with(|| DiffViewState::new(cx))
                    .clone();
                build(&c.borrow(), &entity, &l)
            },
        )
        .expect("host");
        let mut rig = Self {
            host,
            config,
            state,
            log,
        };
        rig.refresh();
        rig
    }

    pub fn refresh(&mut self) {
        self.host.invalidate();
        self.host.flush();
        self.host.invalidate();
        self.host.flush();
    }

    pub fn set(&mut self, change: impl FnOnce(&mut Config)) {
        change(&mut self.config.borrow_mut());
        self.refresh();
    }

    pub fn entity(&self) -> Entity<DiffViewState> {
        self.state.borrow().clone().expect("state")
    }

    pub fn read<R>(&mut self, f: impl FnOnce(&DiffViewState) -> R) -> R {
        let entity = self.entity();
        self.host.app().update(|cx| f(entity.read(cx)))
    }

    pub fn with_window<R>(&mut self, f: impl FnOnce(&DiffViewState, &mut Window) -> R) -> R {
        let entity = self.entity();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| f(entity.read(cx), window))
            .expect("window")
    }

    pub fn with_app<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        let window = self.host.window();
        let result = self
            .host
            .app()
            .update_window(window, |_, window, cx| f(window, cx))
            .expect("window");
        self.refresh();
        result
    }

    pub fn update<R>(
        &mut self,
        f: impl FnOnce(&mut DiffViewState, &mut vespertine::Context<DiffViewState>) -> R,
    ) -> R {
        let entity = self.entity();
        let result = self.host.app().update(|cx| entity.update(cx, f));
        self.refresh();
        result
    }

    pub fn service(&mut self) -> TextService {
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| TextService::of(window, cx))
            .expect("window")
    }

    pub fn events(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

fn build(config: &Config, entity: &Entity<DiffViewState>, log: &Log) -> AnyElement {
    let mut view = DiffView::new(entity)
        .patch(config.patch.clone())
        .theme(config.theme.clone())
        .word_diff(config.word_diff)
        .scroll(config.scroll)
        .max_lines(config.max_lines)
        .file_line_cap(config.file_cap)
        .mode(config.mode)
        .wrap_lines(config.wrap)
        .annotations(config.annotations.clone())
        .sticky_headers(config.sticky)
        .frosted(config.frosted)
        .syntax(config.syntax)
        .draft_context_key(config.draft_key)
        .absolute()
        .left(px(config.left))
        .top(px(config.top))
        .w(px(config.width))
        .h(px(config.height));
    if let Some(collapsed) = &config.collapsed {
        view = view.collapsed_paths(collapsed.clone());
    }
    if config.subscribe {
        let l = log.clone();
        view = view.on_toggle_file(move |path, _, _| l.borrow_mut().push(format!("toggle {path}")));
        let l = log.clone();
        view = view.on_show_more(move |n, _, _| l.borrow_mut().push(format!("more {n}")));
        let l = log.clone();
        view = view.on_line_click(move |click, _, _| {
            l.borrow_mut().push(format!(
                "line {} {:?} {:?} {:?}",
                click.text, click.old, click.new, click.source
            ))
        });
        let l = log.clone();
        view = view.on_add_annotation(move |add, _, _| {
            l.borrow_mut().push(format!(
                "add {} {:?} {} {:?} {:?}",
                add.path, add.side, add.line, add.body, add.old_path
            ))
        });
        let l = log.clone();
        view = view.on_edit_annotation(move |id, body, _, _| {
            l.borrow_mut().push(format!("edit {id} {body}"))
        });
        let l = log.clone();
        view =
            view.on_remove_annotation(move |id, _, _| l.borrow_mut().push(format!("remove {id}")));
    }
    if config.editor {
        let slot = config.editor_slot.clone();
        let l = log.clone();
        view = view.draft_editor(move |editor, _window, _cx| {
            *slot.borrow_mut() = Some(editor.clone());
            let l = l.clone();
            div()
                .id("draft-editor")
                .track_focus(&editor.focus)
                .size_full()
                .bg(rgb(0x223344))
                .on_key_down(move |event, _, _| {
                    l.borrow_mut()
                        .push(format!("editor key {}", event.keystroke.key));
                })
                .into_any_element()
        });
    }
    let content = div()
        .size_full()
        .bg(rgb(0x181818))
        .relative()
        .child(view)
        .into_any_element();
    let content = match &config.search {
        Some(query) => highlight(
            "find",
            HighlightDeclaration::from_spec(HighlightSpec::new(query.clone())),
            content,
        )
        .into_any_element(),
        None => content,
    };
    text_root(content).into_any_element()
}

/// A patch of `files` files, each with `hunks` hunks of `lines` changed lines (a deletion and
/// an addition per line), in a source language.
pub fn many_files(files: usize, hunks: usize, lines: usize) -> String {
    let mut patch = String::new();
    for file in 0..files {
        patch.push_str(&format!(
            "diff --git a/src/file{file}.rs b/src/file{file}.rs\n--- a/src/file{file}.rs\n+++ b/src/file{file}.rs\n"
        ));
        for hunk in 0..hunks {
            let start = 1 + hunk * 100;
            patch.push_str(&format!("@@ -{start},{lines} +{start},{lines} @@\n"));
            for line in 0..lines {
                patch.push_str(&format!("-let old_{file}_{hunk}_{line} = {line};\n"));
                patch.push_str(&format!("+let new_{file}_{hunk}_{line} = {line};\n"));
            }
        }
    }
    patch
}

pub fn image_pixel(image: &image::RgbaImage, x: f32, y: f32) -> [u8; 4] {
    let scale = 2.0;
    image.get_pixel((x * scale) as u32, (y * scale) as u32).0
}
