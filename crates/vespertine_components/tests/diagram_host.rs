//! CMP-013 host flow: the handler, the background render, the stale-theme discard and the
//! per-frame source toggle, driven through real frames of the offscreen test host.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use vespertine::{AppContext as _, Context, IntoElement, ParentElement, Render, Window, div};
use vespertine_components::diagram::{
    DiagramHandler, DiagramHost, DiagramRenderer, DiagramView, Palette, View,
};
use vespertine_test_host::{HostOptions, TestHost};
use vespertine_theme::document::DocumentTheme;

const SOURCE: &str = "flowchart TD\n  A --> B\n";
const FRAME: &str = "row-1-mermaid-0";

/// A renderer that records the palette of every render and paints the node color.
struct Recording(Arc<Mutex<Vec<Palette>>>);

impl DiagramRenderer for Recording {
    fn render(&self, _source: &str, palette: &Palette) -> Result<String, String> {
        self.0.lock().unwrap().push(palette.clone());
        Ok(format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40" viewBox="0 0 80 40"><rect width="80" height="40" fill="{}"/></svg>"##,
            palette.node.to_hex()
        ))
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Shown {
    Source,
    Failed(String),
    Diagram { show_source: bool },
}

struct Scene {
    host: DiagramHost,
    handler: DiagramHandler,
    theme: DocumentTheme,
    /// Theme switched to right after the next request is made (simulates a theme change while
    /// the render is pending).
    switch_after_request: Option<DocumentTheme>,
    shown: Vec<Shown>,
    /// Renders completed when each diagram frame was shown.
    renders_at_diagram: Vec<usize>,
    renders: Arc<Mutex<Vec<Palette>>>,
    invoke_toggle: bool,
    updates: Rc<RefCell<Vec<Vec<String>>>>,
}

struct Doc(Rc<RefCell<Scene>>);

impl Render for Doc {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut scene = self.0.borrow_mut();
        let theme = scene.theme.clone();
        scene.host.begin_pass(&theme, View::default(), window, cx);
        let handler = scene.handler.clone();
        let view = handler(SOURCE, FRAME, &theme, window, cx);
        if let Some(next) = scene.switch_after_request.take() {
            scene.theme = next.clone();
            scene.host.begin_pass(&next, View::default(), window, cx);
        }
        match view {
            DiagramView::Source => scene.shown.push(Shown::Source),
            DiagramView::Failed(reason) => scene.shown.push(Shown::Failed(reason.to_string())),
            DiagramView::Diagram {
                body,
                show_source,
                toggle,
            } => {
                scene.shown.push(Shown::Diagram { show_source });
                let renders = scene.renders.lock().unwrap().len();
                scene.renders_at_diagram.push(renders);
                if std::mem::take(&mut scene.invoke_toggle) {
                    toggle(window, cx);
                }
                return div().child(body).into_any_element();
            }
        }
        div().into_any_element()
    }
}

fn setup(
    renderer: Arc<dyn DiagramRenderer>,
    renders: Arc<Mutex<Vec<Palette>>>,
    switch_after_request: Option<DocumentTheme>,
) -> Option<(TestHost, Rc<RefCell<Scene>>)> {
    if !vespertine_test_host::is_available() {
        return None;
    }
    let host = DiagramHost::with_renderer(renderer);
    let updates = Rc::new(RefCell::new(Vec::new()));
    let sink = updates.clone();
    host.on_update(move |rows, _| sink.borrow_mut().push(rows.to_vec()));
    let scene = Rc::new(RefCell::new(Scene {
        handler: host.handler(),
        host,
        theme: DocumentTheme::dark(),
        switch_after_request,
        shown: Vec::new(),
        renders_at_diagram: Vec::new(),
        renders,
        invoke_toggle: false,
        updates,
    }));
    let shared = scene.clone();
    let test_host = TestHost::with_view(HostOptions::with_size(200.0, 100.0), move |_, cx| {
        cx.new(|_| Doc(shared))
    })
    .expect("host");
    Some((test_host, scene))
}

#[test]
fn a_render_never_blocks_painting_and_the_result_replaces_the_source_fence() {
    let palettes = Arc::new(Mutex::new(Vec::new()));
    let Some((mut host, scene)) = setup(
        Arc::new(Recording(palettes.clone())),
        palettes.clone(),
        None,
    ) else {
        return;
    };
    host.flush();
    let scene = scene.borrow();
    assert_eq!(
        scene.shown.first(),
        Some(&Shown::Source),
        "the first frame shows the source"
    );
    assert_eq!(
        scene.shown.last(),
        Some(&Shown::Diagram { show_source: false }),
        "the finished render is shown"
    );
    assert_eq!(
        palettes.lock().unwrap().len(),
        1,
        "one render for one source"
    );
    assert_eq!(
        scene.updates.borrow().first(),
        Some(&vec!["row-1".to_string()]),
        "the row that painted the source is remeasured"
    );
}

#[test]
fn switching_the_theme_while_a_render_is_pending_never_shows_the_old_palette() {
    let palettes = Arc::new(Mutex::new(Vec::new()));
    let Some((mut host, scene)) = setup(
        Arc::new(Recording(palettes.clone())),
        palettes.clone(),
        Some(DocumentTheme::light()),
    ) else {
        return;
    };
    host.flush();
    host.paint_frame();
    let scene = scene.borrow();
    assert_eq!(scene.shown.first(), Some(&Shown::Source));
    assert!(
        scene.renders_at_diagram.iter().all(|renders| *renders >= 2),
        "no diagram was shown before the render of the new theme finished: {:?} {:?}",
        scene.shown,
        scene.renders_at_diagram
    );
    let palettes = palettes.lock().unwrap();
    let dark = Palette::from_theme(&DocumentTheme::dark());
    let light = Palette::from_theme(&DocumentTheme::light());
    assert_eq!(
        palettes.len(),
        2,
        "the stale render ran and its result was dropped"
    );
    assert_eq!(palettes[0], dark);
    assert_eq!(palettes[1], light);
    // The displayed diagram is the one made under the light palette: the cache holds one entry
    // of the current style and the stale result never created an entry or a remeasure.
    assert_eq!(
        scene.updates.borrow().len(),
        1,
        "only the current-style render remeasures rows"
    );
    assert_eq!(
        scene.shown.last(),
        Some(&Shown::Diagram { show_source: false })
    );
}

#[test]
fn a_failed_render_shows_the_failed_view_with_its_reason() {
    struct Failing;
    impl DiagramRenderer for Failing {
        fn render(&self, _: &str, _: &Palette) -> Result<String, String> {
            Err("Parse error on line 2".to_string())
        }
    }
    let Some((mut host, scene)) = setup(Arc::new(Failing), Arc::default(), None) else {
        return;
    };
    host.flush();
    assert_eq!(
        scene.borrow().shown.last(),
        Some(&Shown::Failed("Parse error on line 2".to_string()))
    );
}

#[test]
fn the_toggle_switches_between_diagram_and_source_for_its_frame() {
    let renders: Arc<Mutex<Vec<Palette>>> = Arc::default();
    let Some((mut host, scene)) = setup(Arc::new(Recording(renders.clone())), renders, None) else {
        return;
    };
    host.flush();
    {
        let scene = scene.borrow();
        assert_eq!(
            scene.shown.last(),
            Some(&Shown::Diagram { show_source: false })
        );
        scene
            .host
            .with_cache(|cache| assert!(!cache.source_visible(FRAME)));
    }
    // The toggle callback is what a header action calls.
    scene.borrow_mut().invoke_toggle = true;
    host.paint_frame();
    host.paint_frame();
    assert_eq!(
        scene.borrow().shown.last(),
        Some(&Shown::Diagram { show_source: true })
    );
    // Row retention prunes the toggle with its row.
    scene.borrow().host.retain_rows(|row| row != "row-1");
    host.paint_frame();
    assert_eq!(
        scene.borrow().shown.last(),
        Some(&Shown::Diagram { show_source: false })
    );
}
