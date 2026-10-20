//! The `code` (CMP-014 code block) and `markdown` (CMP-011 document) host types.
//!
//! Both are leaf hosts: the host's style is the outer surface and, for code, the typography;
//! the standard events (click, enter, leave, file drop) come from the shared host plumbing.

use serde_json::{Value, json};
use vespertine::{
    AnyElement, App, ElementAdapter, ElementFactory, HostId, IntoElement, ParentElement,
    RenderContext, Window,
};
use vespertine_components::code::block::CodeBlock;
use vespertine_components::markdown_doc::view::{Markdown, MarkdownDocument};
use vespertine_richtext::{UserSelect, user_select};
use vespertine_theme::document::DocumentTheme;

use super::custom;

const CODE_PROPERTIES: &[&str] = &["code", "language", "path", "showLineNumbers", "theme"];
const CODE_EVENTS: &[&str] = &["click", "enter", "leave", "fileDrop"];

const MARKDOWN_PROPERTIES: &[&str] = &["markdown", "theme"];
const MARKDOWN_EVENTS: &[&str] = &["click", "enter", "leave", "fileDrop", "linkClick"];

fn theme(value: &Value) -> DocumentTheme {
    if value.as_str() == Some("light") {
        DocumentTheme::light()
    } else {
        DocumentTheme::dark()
    }
}

/// A property that is a string: anything else resets it.
fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

// ---- code (CMP-014) -------------------------------------------------------------------------------

pub(super) struct CodeFactory;

impl ElementFactory for CodeFactory {
    fn type_name(&self) -> &'static str {
        "code"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(CodeAdapter {
            code: String::new(),
            language: None,
            path: None,
            line_numbers: false,
            theme: DocumentTheme::dark(),
        })
    }
}

struct CodeAdapter {
    code: String,
    language: Option<String>,
    path: Option<String>,
    line_numbers: bool,
    theme: DocumentTheme,
}

impl ElementAdapter for CodeAdapter {
    fn render(
        &mut self,
        context: RenderContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let mut block = CodeBlock::new(context.element_id(), &self.code)
            .show_line_numbers(self.line_numbers)
            .theme(self.theme.clone());
        if let Some(language) = &self.language {
            block = block.language(language.clone());
        }
        if let Some(path) = &self.path {
            block = block.path(path.clone());
        }
        let element = context.track(context.apply_to(block));
        if context.selectable {
            element.into_any_element()
        } else {
            user_select(UserSelect::None, element).into_any_element()
        }
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        match name {
            "code" => self.code = string(value).unwrap_or_default(),
            "language" => self.language = string(value),
            "path" => self.path = string(value),
            "showLineNumbers" => self.line_numbers = value.as_bool().unwrap_or(false),
            "theme" => self.theme = theme(value),
            _ => {}
        }
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        CODE_PROPERTIES
    }

    fn supported_events(&self) -> &'static [&'static str] {
        CODE_EVENTS
    }

    fn destroy(&mut self, _cx: &mut App) {}
}

// ---- markdown (CMP-011) ---------------------------------------------------------------------------

pub(super) struct MarkdownFactory;

impl ElementFactory for MarkdownFactory {
    fn type_name(&self) -> &'static str {
        "markdown"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(MarkdownAdapter {
            document: MarkdownDocument::new(),
            theme: DocumentTheme::dark(),
        })
    }
}

/// The adapter of one `markdown` host: the document keeps its parse across frames, so a source
/// that extends the previous one reparses only its last block.
struct MarkdownAdapter {
    document: MarkdownDocument,
    theme: DocumentTheme,
}

impl ElementAdapter for MarkdownAdapter {
    fn render(
        &mut self,
        mut context: RenderContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let mut view =
            Markdown::document((context.element_id(), "document"), self.document.clone())
                .theme(self.theme.clone())
                .selectable(context.selectable);
        if context.events.contains("linkClick") {
            let emit = context.emit.clone();
            view = view.on_link(move |activation, window, cx| {
                emit(
                    custom("linkClick", json!({ "value": activation.destination })),
                    window,
                    cx,
                )
            });
        }
        // The host's style frames the document: the surface is the styled box, the view fills it.
        let mut surface = context.surface();
        surface.extend([view.into_any_element()]);
        surface.into_any_element()
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        match name {
            "markdown" => self.document.set_source(value.as_str().unwrap_or("")),
            "theme" => self.theme = theme(value),
            _ => {}
        }
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        MARKDOWN_PROPERTIES
    }

    fn supported_events(&self) -> &'static [&'static str] {
        MARKDOWN_EVENTS
    }

    fn destroy(&mut self, _cx: &mut App) {}
}
