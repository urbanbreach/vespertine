//! Factories of the built-in host element types that render through the adapter registry
//! (ELM-007 R12): the scene registers them in its [`CustomElementRegistry`].

mod canvas;
mod editors;

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde_json::{Value, json};
use vespertine::{
    AnyElement, App, CustomElementRegistry, ElementAdapter, ElementFactory, Entity, HostEvent,
    HostId, IntoElement, ParentElement, RenderContext, Styled, Window,
};
use vespertine_components::diff::highlight::FileDocuments;
use vespertine_components::diff::rows::{Annotation, DiffMode, Side};
use vespertine_components::diff::state::DiffViewState;
use vespertine_components::diff::view::DiffView;
use vespertine_theme::document::DocumentTheme;

/// Registers the built-in adapters of the JavaScript host.
pub fn register_builtin_hosts(registry: &mut CustomElementRegistry) {
    registry.register(DiffFactory);
    registry.register(editors::InputFactory);
    registry.register(editors::TextareaFactory);
    registry.register(canvas::CanvasFactory);
}

fn custom(name: &'static str, payload: Value) -> HostEvent {
    HostEvent::Custom {
        name: name.into(),
        payload,
    }
}

// ---- diff (CMP-015) ---------------------------------------------------------------------------

/// The properties of the `diff` host: the unified patch viewer and the annotated split surface.
const DIFF_PROPERTIES: &[&str] = &[
    "patch",
    "wordDiff",
    "scroll",
    "maxLines",
    "fileLineCap",
    "collapsedPaths",
    "theme",
    "mode",
    "wrapLines",
    "codeFontSize",
    "annotations",
    "draftContextKey",
    "stickyHeaders",
    "frosted",
    "syntax",
    "documents",
];

/// The events of the `diff` host beyond the standard ones. Callbacks are wired only when the
/// host subscribed (CMP-015 R19); the owner decides what each one changes.
const DIFF_EVENTS: &[&str] = &[
    "click",
    "enter",
    "leave",
    "fileDrop",
    "toggleFile",
    "showMore",
    "lineClick",
    "annotationAdd",
    "annotationEdit",
    "annotationRemove",
];

struct DiffFactory;

impl ElementFactory for DiffFactory {
    fn type_name(&self) -> &'static str {
        "diff"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(DiffAdapter::default())
    }
}

/// The adapter of one `diff` host: the viewer state entity and the properties last applied.
struct DiffAdapter {
    state: Option<Entity<DiffViewState>>,
    patch: Arc<str>,
    word_diff: bool,
    scroll: bool,
    max_lines: Option<usize>,
    file_line_cap: Option<usize>,
    collapsed: Option<Vec<String>>,
    light: bool,
    split: bool,
    wrap: bool,
    code_font_size: Option<f32>,
    annotations: Vec<Annotation>,
    draft_key: Option<u64>,
    sticky: bool,
    frosted: bool,
    syntax: bool,
    documents: Vec<FileDocuments>,
}

impl Default for DiffAdapter {
    fn default() -> Self {
        Self {
            state: None,
            patch: Arc::from(""),
            word_diff: false,
            scroll: false,
            max_lines: None,
            file_line_cap: None,
            collapsed: None,
            light: false,
            split: false,
            wrap: false,
            code_font_size: None,
            annotations: Vec::new(),
            draft_key: None,
            sticky: false,
            frosted: false,
            syntax: true,
            documents: Vec::new(),
        }
    }
}

fn count(value: &Value) -> Option<usize> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && *number >= 0.0)
        .map(|number| number as usize)
}

fn side(value: &Value) -> Option<Side> {
    match value.as_str()? {
        "old" => Some(Side::Old),
        "new" => Some(Side::New),
        _ => None,
    }
}

fn annotation(value: &Value) -> Option<Annotation> {
    Some(Annotation {
        id: value.get("id")?.as_str()?.to_owned(),
        path: value.get("path")?.as_str()?.to_owned(),
        side: side(value.get("side")?)?,
        line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
        body: value.get("body")?.as_str()?.to_owned(),
    })
}

fn documents(value: &Value) -> Option<FileDocuments> {
    let text = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(Arc::<str>::from)
    };
    let flag = |name: &str| value.get(name).and_then(Value::as_bool).unwrap_or(false);
    Some(FileDocuments {
        path: value.get("path")?.as_str()?.to_owned(),
        old: text("old"),
        new: text("new"),
        stale: flag("stale"),
        binary: flag("binary"),
        truncated: flag("truncated"),
    })
}

impl ElementAdapter for DiffAdapter {
    fn render(
        &mut self,
        mut context: RenderContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let state = self
            .state
            .get_or_insert_with(|| DiffViewState::new(cx))
            .clone();
        let theme = Arc::new(if self.light {
            DocumentTheme::light()
        } else {
            DocumentTheme::dark()
        });
        let mut view = DiffView::new(&state)
            .patch(self.patch.clone())
            .theme(theme)
            .word_diff(self.word_diff)
            .scroll(self.scroll)
            .max_lines(self.max_lines)
            .file_line_cap(self.file_line_cap)
            .mode(if self.split {
                DiffMode::Split
            } else {
                DiffMode::Unified
            })
            .wrap_lines(self.wrap)
            .annotations(self.annotations.clone())
            .sticky_headers(self.sticky)
            .frosted(self.frosted)
            .syntax(self.syntax)
            .documents(self.documents.clone());
        if let Some(paths) = &self.collapsed {
            view = view.collapsed_paths(paths.clone());
        }
        if let Some(size) = self.code_font_size {
            view = view.code_font_size(size);
        }
        if let Some(key) = self.draft_key {
            view = view.draft_context_key(key);
        }
        // The style's corner radius is honored on the first header; every other box property
        // belongs to the host surface that holds the viewer.
        view.style().corner_radii = context.style.to_refinement().corner_radii;

        let subscribed = |name: &str| context.events.contains(name);
        let emit = context.emit.clone();
        if subscribed("toggleFile") {
            let emit = emit.clone();
            view = view.on_toggle_file(move |path, window, cx| {
                emit(custom("toggleFile", json!({ "value": path })), window, cx)
            });
        }
        if subscribed("showMore") {
            let emit = emit.clone();
            view = view.on_show_more(move |remaining, window, cx| {
                emit(
                    custom("showMore", json!({ "value": remaining })),
                    window,
                    cx,
                )
            });
        }
        if subscribed("lineClick") {
            let emit = emit.clone();
            view = view.on_line_click(move |click, window, cx| {
                let payload = json!({
                    "path": click.path,
                    "text": click.text,
                    "old": click.old,
                    "new": click.new,
                });
                emit(custom("lineClick", payload), window, cx)
            });
        }
        if subscribed("annotationAdd") {
            let emit = emit.clone();
            view = view.on_add_annotation(move |add, window, cx| {
                let payload = json!({
                    "path": add.path,
                    "side": match add.side { Side::Old => "old", Side::New => "new" },
                    "line": add.line,
                    "body": add.body,
                    "oldPath": add.old_path,
                });
                emit(custom("annotationAdd", payload), window, cx)
            });
        }
        if subscribed("annotationEdit") {
            let emit = emit.clone();
            view = view.on_edit_annotation(move |id, body, window, cx| {
                emit(
                    custom("annotationEdit", json!({ "id": id, "body": body })),
                    window,
                    cx,
                )
            });
        }
        if subscribed("annotationRemove") {
            let emit = emit.clone();
            view = view.on_remove_annotation(move |id, window, cx| {
                emit(custom("annotationRemove", json!({ "id": id })), window, cx)
            });
        }

        let mut surface = context.surface();
        let view = view.w_full();
        surface.extend([if self.scroll { view.h_full() } else { view }.into_any_element()]);
        surface.into_any_element()
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        let flag = |default: bool| value.as_bool().unwrap_or(default);
        match name {
            "patch" => self.patch = Arc::from(value.as_str().unwrap_or("")),
            "wordDiff" => self.word_diff = flag(false),
            "scroll" => self.scroll = flag(false),
            "maxLines" => self.max_lines = count(value),
            "fileLineCap" => self.file_line_cap = count(value),
            "collapsedPaths" => {
                self.collapsed = value.as_array().map(|paths| {
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_owned))
                        .collect()
                })
            }
            "theme" => self.light = value.as_str() == Some("light"),
            "mode" => self.split = value.as_str() == Some("split"),
            "wrapLines" => self.wrap = flag(false),
            "codeFontSize" => {
                self.code_font_size = value
                    .as_f64()
                    .filter(|size| size.is_finite() && *size > 0.0)
                    .map(|size| size as f32)
            }
            "annotations" => {
                self.annotations = value
                    .as_array()
                    .map(|items| items.iter().filter_map(annotation).collect())
                    .unwrap_or_default()
            }
            "draftContextKey" => {
                self.draft_key = (!value.is_null()).then(|| {
                    let mut hasher = std::hash::DefaultHasher::new();
                    value.to_string().hash(&mut hasher);
                    hasher.finish()
                })
            }
            "stickyHeaders" => self.sticky = flag(false),
            "frosted" => self.frosted = flag(false),
            "syntax" => self.syntax = flag(true),
            "documents" => {
                self.documents = value
                    .as_array()
                    .map(|items| items.iter().filter_map(documents).collect())
                    .unwrap_or_default()
            }
            _ => {}
        }
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        DIFF_PROPERTIES
    }

    fn supported_events(&self) -> &'static [&'static str] {
        DIFF_EVENTS
    }

    fn destroy(&mut self, _cx: &mut App) {
        self.state = None;
    }
}
