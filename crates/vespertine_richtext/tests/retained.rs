//! TXT-008/TXT-009 under retained frames (PRF-001): frames that replay unchanged views instead of
//! painting them, drawn without any refresh. Registration, logs, highlight numbering, selection and
//! links must be the same as when every view is built (retention disabled).

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use support::{Build, Harness, at};
use vespertine::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, RetentionStats,
    SharedString, Styled, Window, div, px, rgb,
};
use vespertine_richtext::registry::{PaintedHighlight, PaintedText};
use vespertine_richtext::search::{HighlightDeclaration, HighlightSpec, default_hex_color};
use vespertine_richtext::{Link, LinkActivation, LinkOptions, RichText, highlight, text_root};

const WIDTH: f32 = 260.0;
const URL: &str = "https://example.com/retained";

type Activations = Rc<RefCell<Vec<LinkActivation>>>;

/// One line of the document: a view of its own, so it is retained independently.
struct Panel {
    name: &'static str,
    text: SharedString,
    link: Option<(std::ops::Range<usize>, Activations)>,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut run = RichText::new(self.name, self.text.clone());
        if let Some((range, activations)) = &self.link {
            let activations = activations.clone();
            run = run
                .links(vec![Link {
                    range: range.clone(),
                    destination: URL.to_string(),
                }])
                .link_options(LinkOptions::default().on_activate(move |activation, _, _| {
                    activations.borrow_mut().push(activation.clone())
                }));
        }
        div().h(px(24.)).child(run)
    }
}

/// What the text service shows after a frame.
#[derive(Debug, PartialEq)]
struct Snapshot {
    runs: usize,
    text: Vec<PaintedText>,
    washes: Vec<PaintedHighlight>,
    selected: Option<String>,
    counts: Vec<usize>,
}

struct Scenario {
    harness: Harness,
    panels: Vec<Entity<Panel>>,
    counts: Rc<RefCell<Vec<usize>>>,
    activations: Activations,
}

const LINES: [&str; 4] = [
    "first red line",
    "second line",
    "third red RED line",
    "link text",
];

/// The document view: the text root (and the highlight host) around the line views. It is nested
/// in the window's root view, so it is replayed around a changed line.
struct Document {
    panels: Vec<Entity<Panel>>,
    query: Option<&'static str>,
    counts: Rc<RefCell<Vec<usize>>>,
}

impl Render for Document {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let lines = div()
            .size_full()
            .bg(rgb(0x181818))
            .font_family("Noto Sans")
            .text_size(px(16.))
            .line_height(px(24.))
            .text_color(rgb(0xeeeeee))
            .children(self.panels.iter().cloned());
        match self.query {
            Some(query) => {
                let spec = HighlightSpec::new(query)
                    .with_color(default_hex_color("#F0C04066").unwrap())
                    .with_active_color(default_hex_color("#F08040AA").unwrap())
                    .with_active_index(Some(1));
                let counts = self.counts.clone();
                text_root(
                    highlight("h", HighlightDeclaration::from_spec(spec), lines)
                        .on_match_count(move |count, _, _| counts.borrow_mut().push(count)),
                )
                .into_any_element()
            }
            None => text_root(lines).into_any_element(),
        }
    }
}

fn scenario(retained: bool, query: Option<&'static str>) -> Scenario {
    let panels: Rc<RefCell<Vec<Entity<Panel>>>> = Rc::default();
    let counts: Rc<RefCell<Vec<usize>>> = Rc::default();
    let activations: Activations = Rc::default();
    let build: Build = Rc::new({
        let panels = panels.clone();
        let counts = counts.clone();
        let activations = activations.clone();
        let document: Rc<RefCell<Option<Entity<Document>>>> = Rc::default();
        move |_, cx| {
            if document.borrow().is_none() {
                let made: Vec<Entity<Panel>> = LINES
                    .iter()
                    .enumerate()
                    .map(|(ix, text)| {
                        let link = (ix == 3).then(|| (0..4, activations.clone()));
                        cx.new(|_| Panel {
                            name: ["p0", "p1", "p2", "p3"][ix],
                            text: (*text).into(),
                            link,
                        })
                    })
                    .collect();
                *panels.borrow_mut() = made.clone();
                *document.borrow_mut() = Some(cx.new(|_| Document {
                    panels: made,
                    query,
                    counts: counts.clone(),
                }));
            }
            let document = document.borrow().clone().expect("created above");
            div().size_full().child(document).into_any_element()
        }
    });
    let mut harness = Harness::new(WIDTH, 120., build);
    if !retained {
        harness
            .cx
            .update_window(harness.window.into(), |_, window, _| {
                window.set_retention_enabled(false)
            })
            .unwrap();
        harness.redraw();
    }
    let panels = panels.borrow().clone();
    Scenario {
        harness,
        panels,
        counts,
        activations,
    }
}

impl Scenario {
    /// A frame without a refresh: unchanged views are replayed.
    fn frame(&mut self) {
        self.harness.draw();
    }

    fn set_text(&mut self, panel: usize, text: &'static str) {
        let entity = self.panels[panel].clone();
        self.harness.cx.update(|cx| {
            entity.update(cx, |panel, cx| {
                panel.text = text.into();
                cx.notify();
            })
        });
    }

    fn stats(&mut self) -> RetentionStats {
        self.harness
            .cx
            .update_window(self.harness.window.into(), |_, window, _| {
                window.retention_stats()
            })
            .unwrap()
    }

    /// Notifies a line view without changing it: it is built again with the same content.
    fn notify(&mut self, panel: usize) {
        let entity = self.panels[panel].clone();
        self.harness
            .cx
            .update(|cx| entity.update(cx, |_, cx| cx.notify()));
    }

    fn reset_stats(&mut self) {
        self.harness
            .cx
            .update_window(self.harness.window.into(), |_, window, _| {
                window.reset_retention_stats()
            })
            .unwrap();
    }

    fn snapshot(&mut self) -> Snapshot {
        let service = self.harness.service();
        Snapshot {
            runs: service.registered_runs(),
            text: service.painted_text(),
            washes: service.painted_highlights(),
            selected: service.selected_text(),
            counts: self.counts.borrow().clone(),
        }
    }
}

/// The frames of one history; returns what the service showed after each.
fn history(retained: bool, query: Option<&'static str>) -> (Vec<Snapshot>, Vec<RetentionStats>) {
    let mut scenario = scenario(retained, query);
    let mut snapshots = vec![scenario.snapshot()];
    let mut stats = Vec::new();
    let mut step = |scenario: &mut Scenario, change: &mut dyn FnMut(&mut Scenario)| {
        scenario.reset_stats();
        change(scenario);
        scenario.frame();
        stats.push(scenario.stats());
        snapshots.push(scenario.snapshot());
    };
    // Two idle frames replay everything.
    step(&mut scenario, &mut |_| {});
    step(&mut scenario, &mut |_| {});
    // A line is built again with the same content: the others are replayed around it.
    step(&mut scenario, &mut |s| s.notify(1));
    // A sibling view changes: the others are replayed (or rebuilt with it, under a highlight).
    step(&mut scenario, &mut |s| s.set_text(1, "second red red line"));
    step(&mut scenario, &mut |_| {});
    step(&mut scenario, &mut |s| s.set_text(2, "third line"));
    // A selection across every run, then idle frames that keep it.
    step(&mut scenario, &mut |s| {
        s.harness.drag(at(1., 12.), at(150., 100.))
    });
    step(&mut scenario, &mut |_| {});
    step(&mut scenario, &mut |_| {});
    (snapshots, stats)
}

#[test]
fn replayed_views_stay_registered_next_to_a_rebuilt_sibling() {
    let mut scenario = scenario(true, None);
    assert_eq!(scenario.snapshot().runs, 4);
    scenario.reset_stats();
    scenario.notify(1);
    scenario.frame();
    let stats = scenario.stats();
    assert_eq!(
        stats.splice_successes, 1,
        "the document is replayed around it: {stats:?}"
    );
    assert!(
        stats.views_rendered <= 2,
        "the other lines are not built: {stats:?}"
    );

    let snapshot = scenario.snapshot();
    assert_eq!(snapshot.runs, 4, "every run is registered exactly once");
    let texts: Vec<&str> = snapshot.text.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, LINES);

    // Lines that change size rebuild their parents: the lines that did not change are replayed
    // one by one under the rebuilt document.
    scenario.reset_stats();
    scenario.set_text(1, "changed second line");
    scenario.frame();
    let stats = scenario.stats();
    assert!(stats.views_reused >= 3, "{stats:?}");
    let snapshot = scenario.snapshot();
    assert_eq!(snapshot.runs, 4, "every run is registered exactly once");

    // The registry the replayed frames ended with drives a selection across all four runs.
    scenario.harness.drag(at(1., 12.), at(150., 100.));
    assert_eq!(
        scenario.snapshot().selected.as_deref(),
        Some("first red line\nchanged second line\nthird red RED line\nlink text")
    );
}

#[test]
fn a_selection_and_a_link_survive_frames_that_replay_the_document() {
    let mut scenario = scenario(true, None);
    scenario.harness.drag(at(1., 12.), at(100., 36.));
    let selected = scenario.snapshot().selected;
    assert!(selected.is_some());
    scenario.frame();
    scenario.reset_stats();
    scenario.frame();
    scenario.frame();
    assert_eq!(
        scenario.stats().views_rendered,
        0,
        "idle frames replay every view"
    );
    assert_eq!(scenario.snapshot().selected, selected);
    assert_eq!(scenario.snapshot().runs, 4);

    // A press and release on the link of a replayed frame activates it (a drag selection is
    // cleared by a press elsewhere first).
    scenario.harness.click(at(150., 110.));
    scenario.frame();
    scenario.frame();
    scenario.harness.click(at(10., 84.));
    assert_eq!(scenario.activations.borrow().len(), 1);
}

#[test]
fn retained_frames_leave_the_service_as_frames_that_build_every_view_do() {
    let mut notified = Vec::new();
    for query in [None, Some("red")] {
        let (retained, stats) = history(true, query);
        let (built, _) = history(false, query);
        assert_eq!(retained, built, "query {query:?}");
        // Idle frames replay every view.
        for idle in [0, 1, 4, 7, 8] {
            assert_eq!(stats[idle].views_rendered, 0, "query {query:?}: {stats:?}");
        }
        notified.push(stats[2].views_rendered);
    }
    // A highlight host numbers washes over everything it covers, so a notified line rebuilds the
    // document and every line with it instead of being replayed around.
    assert!(notified[0] < notified[1], "{notified:?}");
    assert!(notified[1] >= 5, "{notified:?}");
}

#[test]
fn highlight_counts_follow_the_text_of_replayed_documents() {
    let mut scenario = scenario(true, Some("red"));
    // "first red" + "third red RED" = 3 matches.
    assert_eq!(*scenario.counts.borrow(), [3]);
    scenario.frame();
    scenario.frame();
    assert_eq!(
        *scenario.counts.borrow(),
        [3],
        "replayed frames report nothing new"
    );
    scenario.set_text(1, "second red red line");
    scenario.frame();
    assert_eq!(*scenario.counts.borrow(), [3, 5]);
    let washes = scenario.snapshot().washes;
    assert_eq!(washes.len(), 5);
    assert_eq!(washes.iter().filter(|wash| wash.active).count(), 1);
}
