//! ELM-007 R10 / A7 against the real TXT-008 selection registry: selectable runs of a custom
//! element keep their identity across frames, chrome text is logged but never selected or copied,
//! and a host that disabled selection paints text that cannot be selected.

mod support;

use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use support::{Build, Harness, at};
use vespertine::{
    AnyElement, App, CustomElementRegistry, ElementAdapter, ElementFactory, HostId, IntoElement,
    ParentElement, RenderContext, Styled, Window, div, px, rgb,
};
use vespertine_richtext::{RichHostText, host_run_key, text_root};

struct Lines {
    selectable: bool,
}

struct LinesFactory {
    selectable: bool,
}

impl ElementFactory for LinesFactory {
    fn type_name(&self) -> &'static str {
        "lines"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(Lines {
            selectable: self.selectable,
        })
    }
}

impl ElementAdapter for Lines {
    fn render(
        &mut self,
        mut context: RenderContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        context.selectable = self.selectable;
        div()
            .flex()
            .flex_col()
            .child(context.selectable_text(0, "alpha", None))
            .child(context.selectable_text(1, "beta", None))
            .child(context.chrome_text(0, "1", None))
            .into_any_element()
    }

    fn set_property(&mut self, _name: &str, _value: &Value) {}

    fn supported_properties(&self) -> &'static [&'static str] {
        &[]
    }

    fn supported_events(&self) -> &'static [&'static str] {
        &[]
    }

    fn destroy(&mut self, _cx: &mut App) {}
}

fn build(selectable: bool) -> (Build, Rc<RefCell<CustomElementRegistry>>) {
    let mut registry = CustomElementRegistry::empty();
    registry.register(LinesFactory { selectable });
    let registry = Rc::new(RefCell::new(registry));
    let build: Build = {
        let registry = registry.clone();
        Rc::new(move |window, cx| {
            let mut context = RenderContext::new(HostId(7), Rc::new(|_, _, _| {}));
            context.text = Some(Rc::new(RichHostText));
            context.properties = Arc::default();
            let host = registry.borrow_mut().render("lines", context, window, cx);
            text_root(
                div()
                    .size_full()
                    .bg(rgb(0x181818))
                    .font_family("Noto Sans")
                    .text_size(px(16.))
                    .line_height(px(24.))
                    .text_color(rgb(0xeeeeee))
                    .child(host),
            )
            .into_any_element()
        })
    };
    (build, registry)
}

#[test]
fn selectable_runs_keep_identity_and_chrome_is_never_selected_or_copied() {
    let (build, _registry) = build(true);
    let mut harness = Harness::new(240., 200., build);
    let service = harness.service();
    assert_eq!(service.registered_runs(), 2);
    let keys = |service: &vespertine_richtext::TextService| {
        service
            .painted_text()
            .iter()
            .map(|entry| (entry.key, entry.chrome, entry.text.clone()))
            .collect::<Vec<_>>()
    };
    let first = keys(&service);
    assert_eq!(
        first,
        vec![
            (host_run_key(HostId(7), 0), false, "alpha".to_owned()),
            (host_run_key(HostId(7), 1), false, "beta".to_owned()),
            (first[2].0, true, "1".to_owned()),
        ]
    );
    // The same host identity and sub-index on later frames.
    harness.draw();
    harness.draw();
    assert_eq!(keys(&service), first);
    assert_eq!(service.registered_runs(), 2, "chrome is never registered");

    // A drag across both lines and into the chrome row copies content only.
    harness.drag(at(1., 12.), at(100., 60.));
    assert_eq!(service.selected_text().as_deref(), Some("alpha\nbeta"));
}

#[test]
fn a_host_that_disabled_selection_paints_text_that_cannot_be_selected() {
    let (build, _registry) = build(false);
    let mut harness = Harness::new(240., 200., build);
    let service = harness.service();
    assert_eq!(service.registered_runs(), 0);
    let painted = service.painted_text();
    assert!(painted.iter().any(|entry| entry.text == "alpha"));
    assert!(painted.iter().any(|entry| entry.text == "beta"));
    harness.drag(at(1., 12.), at(100., 36.));
    assert_eq!(service.selected_text(), None);
}
