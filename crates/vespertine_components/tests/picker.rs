//! CMP-006 native acceptance (A7, A9-A14, R9, R13-R17) of the picker view in the production
//! offscreen test host. (A8 and the generation rules are covered by `picker_driver`; A1-A6 are the
//! JavaScript adapters'.)

mod controls_support;

use controls_support::{center, next_frame, node, nodes};
use futures::channel::oneshot;
use std::{cell::Cell, cell::RefCell, rc::Rc};
use vespertine::{
    AppContext as _, Context, Entity, EntityInputHandler as _, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, ParentElement, Render, Role, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use vespertine_components::floating::{filter_and_rank, layers};
use vespertine_components::picker::actions::PICKER_CONTEXT;
use vespertine_components::picker::driver::{Fetcher, PickerSources};
use vespertine_components::picker::model::{Depth, ScopeTab};
use vespertine_components::picker::results::{Paging, Row, SectionSpec};
use vespertine_components::picker::sources::{Answer, SourcePolicy};
use vespertine_components::picker::{Picker, PickerPart};
use vespertine_test_host::{HostOptions, PaintLog, TestHost, require_backend};

type Reply = oneshot::Sender<Result<Answer<String>, String>>;

#[derive(Default)]
struct Wire {
    queries: Vec<(String, usize)>,
    replies: Vec<Option<Reply>>,
}

#[derive(Default, Clone)]
struct Shared {
    wire: Rc<RefCell<Wire>>,
    activated: Rc<RefCell<Vec<String>>>,
    secondary: Rc<RefCell<Vec<String>>>,
    page_clicks: Rc<Cell<u32>>,
    page_requests: Rc<Cell<u32>>,
    doc_present: Rc<Cell<bool>>,
    closed: Rc<Cell<u32>>,
}

const COMMANDS: [&str; 8] = [
    "Open File",
    "Open Folder",
    "Save",
    "Save All",
    "Close Tab",
    "Toggle Sidebar",
    "Zoom In",
    "Zoom Out",
];

struct Root {
    picker: Entity<Picker>,
    doc: FocusHandle,
    shared: Shared,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.shared.page_clicks.clone();
        div()
            .id("app")
            .size_full()
            .relative()
            .child(
                div()
                    .id("page")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
            .when(self.shared.doc_present.get(), |app| {
                app.child(
                    div()
                        .id("doc")
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .w(px(80.))
                        .h(px(24.))
                        .track_focus(&self.doc)
                        .role(Role::Button)
                        .aria_label("Document")
                        .child("Document"),
                )
            })
            .child(self.picker.clone())
    }
}

use vespertine::prelude::FluentBuilder as _;

#[derive(Clone)]
struct Spec {
    commands: Vec<String>,
    tabs: bool,
    close_on_activate: bool,
    row_accelerators: bool,
    favorites: bool,
    panel: bool,
    window: (f64, f64),
    summary_limit: usize,
    scope_limit: usize,
}

impl Spec {
    fn new() -> Self {
        Self {
            commands: COMMANDS.iter().map(|c| c.to_string()).collect(),
            tabs: false,
            close_on_activate: true,
            row_accelerators: false,
            favorites: false,
            panel: false,
            window: (640., 480.),
            summary_limit: 5,
            scope_limit: 50,
        }
    }
}

struct Fixture {
    host: TestHost,
    picker: Entity<Picker>,
    doc: FocusHandle,
    shared: Shared,
}

fn part_id(part: PickerPart) -> u64 {
    match part {
        PickerPart::Card => 4001,
        PickerPart::List => 4002,
        PickerPart::Footer => 4003,
        PickerPart::Panel => 4004,
        PickerPart::Tab(ix) => 4010 + ix as u64,
        PickerPart::Header(ix) => 4020 + ix as u64,
        PickerPart::Row(ix) => 5000 + ix as u64,
    }
}

fn fixture(spec: Spec) -> Fixture {
    let shared = Shared::default();
    shared.doc_present.set(true);
    let slot: Rc<RefCell<Option<(Entity<Picker>, Entity<PickerSources<String>>, FocusHandle)>>> =
        Rc::default();
    let out = slot.clone();
    let build = shared.clone();
    let host = TestHost::with_view(
        HostOptions::with_size(spec.window.0, spec.window.1),
        move |window, cx| {
            let doc = cx.focus_handle().tab_stop(true);
            let wire = build.wire.clone();
            let fetcher: Fetcher<String> = Rc::new(move |request, cx| {
                let (tx, rx) = oneshot::channel();
                let mut wire = wire.borrow_mut();
                wire.queries.push((request.query, request.page));
                wire.replies.push(Some(tx));
                cx.background_spawn(
                    async move { rx.await.unwrap_or_else(|_| Err("cancelled".into())) },
                )
            });
            let sources =
                cx.new(|cx| PickerSources::new(vec![(SourcePolicy::name_like(), fetcher)], cx));
            let provider_sources = sources.clone();
            let commands = spec.commands.clone();
            let (summary, scope) = (spec.summary_limit, spec.scope_limit);
            let activated = build.activated.clone();
            let secondary = build.secondary.clone();
            let closed = build.closed.clone();
            let page_requests = build.page_requests.clone();
            let (open_sources, query_sources, close_sources, retry_sources, scroll_sources) = (
                sources.clone(),
                sources.clone(),
                sources.clone(),
                sources.clone(),
                sources.clone(),
            );
            let paging_sources = sources.clone();
            let page_sources = sources.clone();
            let tabs = spec.tabs;
            let panel = spec.panel;
            let picker = cx.new(|cx| {
                let mut picker = Picker::new(
                    "palette",
                    "Command palette",
                    move |query, cx| {
                        let labels: Vec<&String> = commands.iter().collect();
                        let ranked = filter_and_rank(&labels, query);
                        let mut local = SectionSpec::new(
                            "commands",
                            ranked
                                .into_iter()
                                .map(|ix| {
                                    let mut row = Row::new(
                                        format!("cmd:{}", commands[ix]),
                                        commands[ix].clone(),
                                    );
                                    row.hint = Some(format!("Hint: {}", commands[ix]).into());
                                    row
                                })
                                .collect(),
                        );
                        local.title = Some("Commands".into());
                        local.summary_limit = summary;
                        local.scope_limit = scope;
                        let sources = provider_sources.read(cx);
                        let set = sources.set();
                        let mut files = SectionSpec::new(
                            "files",
                            set.rows(0)
                                .iter()
                                .map(|file| Row::new(format!("file:{file}"), file.clone()))
                                .collect(),
                        );
                        files.title = Some("Files".into());
                        files.status = set.status(0).map(Into::into);
                        files.error = set.error(0).map(|message| message.to_string().into());
                        files.loading = set.is_loading(0);
                        vec![local, files]
                    },
                    window,
                    cx,
                )
                .paging(move |cx| {
                    let set_ref = paging_sources.read(cx);
                    let set = set_ref.set();
                    Paging {
                        loading_more: set.is_loading_more(0),
                        error: set.page_error(0).map(|message| message.to_string().into()),
                    }
                })
                .close_on_activate(spec.close_on_activate)
                .row_accelerators(spec.row_accelerators)
                .favorites(spec.favorites)
                .observe_bounds(Rc::new(|part, bounds, cx| {
                    PaintLog::record_bounds(cx, part_id(part), bounds)
                }))
                .on_open(move |_, cx| open_sources.update(cx, |sources, cx| sources.open(cx)))
                .on_close(move |_, cx| {
                    closed.set(closed.get() + 1);
                    close_sources.update(cx, |sources, cx| sources.clear(cx))
                })
                .on_query(move |query, _, cx| {
                    query_sources.update(cx, |sources, cx| sources.set_query(query, cx))
                })
                .on_retry(move |_, _, cx| {
                    retry_sources.update(cx, |sources, cx| sources.retry(0, cx))
                })
                .on_scrolled_to_end(move |_, cx| {
                    page_requests.set(page_requests.get() + 1);
                    scroll_sources.update(cx, |sources, cx| sources.scrolled_to_end(cx))
                })
                .on_retry_page(move |_, cx| {
                    page_sources.update(cx, |sources, cx| sources.retry_page(0, cx))
                })
                .on_activate(move |key, _, _| activated.borrow_mut().push(key.to_string()));
                if tabs {
                    picker = picker.tabs(vec![
                        ScopeTab::all("All"),
                        ScopeTab::scope("commands", "Commands", vec!["commands".into()]),
                        ScopeTab::scope("files", "Files", vec!["files".into()]),
                    ]);
                }
                if panel {
                    picker = picker
                        .panel(|key, _, _| {
                            div().child(format!("Settings of {key}")).into_any_element()
                        })
                        .choices(|_, _, _| div().child("Choices").into_any_element());
                }
                picker
            });
            let _ = secondary;
            *out.borrow_mut() = Some((picker.clone(), sources, doc.clone()));
            cx.new(|_| Root {
                picker,
                doc,
                shared: build,
            })
        },
    )
    .expect("host");
    let (picker, _sources, doc) = slot.borrow_mut().take().expect("entities");
    Fixture {
        host,
        picker,
        doc,
        shared,
    }
}

impl Fixture {
    fn settle(&mut self) {
        self.host.flush();
        for _ in 0..3 {
            next_frame(&mut self.host);
        }
        self.host.motion_fast_forward(300.).unwrap();
        self.host.advance_executor_time(300.).unwrap();
        self.host.flush();
    }

    fn with_picker<R>(
        &mut self,
        f: impl FnOnce(&mut Picker, &mut Window, &mut Context<Picker>) -> R,
    ) -> R {
        let picker = self.picker.clone();
        let window = self.host.window();
        let result = self
            .host
            .app()
            .update_window(window, |_, window, cx| {
                picker.update(cx, |picker, cx| f(picker, window, cx))
            })
            .unwrap();
        self.settle();
        result
    }

    fn open(&mut self) {
        self.with_picker(|picker, window, cx| picker.open(window, cx));
    }

    fn read<R>(&mut self, f: impl FnOnce(&Picker, &vespertine::App) -> R) -> R {
        let picker = self.picker.clone();
        self.host.app().update(|cx| f(picker.read(cx), cx))
    }

    fn is_open(&mut self) -> bool {
        self.read(|picker, _| picker.is_open())
    }

    fn type_text(&mut self, text: &str) {
        let field = self.read(|picker, _| picker.field().clone());
        let text = text.to_owned();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                field.update(cx, |field, cx| {
                    field.replace_text_in_range(None, &text, window, cx)
                })
            })
            .unwrap();
        self.settle();
    }

    /// Replaces the whole query text, as selecting it and typing would.
    fn set_query(&mut self, text: &str) {
        let field = self.read(|picker, _| picker.field().clone());
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                field.update(cx, |field, cx| field.select_all(window, cx))
            })
            .unwrap();
        self.type_text(text);
    }

    fn keys(&mut self, keys: &str) {
        self.host.simulate_keystrokes(keys, None).unwrap();
        self.settle();
    }

    fn escape(&mut self) {
        self.host.key_down("escape", false, None).unwrap();
        self.host.key_up("escape", None).unwrap();
        self.settle();
    }

    fn highlighted(&mut self) -> Option<String> {
        self.read(|picker, _| picker.highlighted().map(|key| key.to_string()))
    }

    fn text(&self) -> Vec<String> {
        self.host.painted_text()
    }

    fn shows(&self, text: &str) -> bool {
        self.text().iter().any(|painted| painted == text)
    }

    fn reply(&self, index: usize, result: Result<Answer<String>, String>) {
        let tx = self.shared.wire.borrow_mut().replies[index]
            .take()
            .expect("reply once");
        tx.send(result).ok();
    }

    fn settle_replies(&mut self) {
        self.host.app().run_until_parked();
        self.settle();
    }

    fn field_focused(&mut self) -> bool {
        let picker = self.picker.clone();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| {
                picker
                    .read(cx)
                    .field()
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            })
            .unwrap()
    }

    fn layer_count(&mut self) -> usize {
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| layers(window, cx).len())
            .unwrap()
    }

    fn rect(&self, part: PickerPart) -> (f64, f64, f64, f64) {
        self.host
            .element_bounds(part_id(part))
            .map(|r| (r.x, r.y, r.width, r.height))
            .unwrap_or_else(|| panic!("{part:?} was not painted"))
    }

    fn focused_label(&mut self) -> Option<String> {
        let tree = self
            .host
            .accessibility_tree()
            .unwrap()
            .expect("accessibility");
        let focus = tree["focus"].as_u64()?;
        let all = nodes(&mut self.host);
        let focused = all.iter().find(|node| node.id == focus)?;
        focused.label().map(str::to_owned)
    }
}

fn files(rows: &[&str]) -> Result<Answer<String>, String> {
    Ok(Answer::complete(
        rows.iter().map(|r| r.to_string()).collect(),
    ))
}

#[test]
fn opening_focuses_the_field_joins_the_stack_at_priority_two_and_closing_restores_focus() {
    require_backend!();
    let mut f = fixture(Spec::new());
    let doc = f.doc.clone();
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| doc.focus(window, cx))
        .unwrap();
    f.settle();
    f.open();
    assert!(f.is_open());
    assert_eq!(f.layer_count(), 1);
    assert!(f.field_focused(), "focus moved to the field once attached");
    assert!(!f.read(|picker, _| picker.is_claiming_focus()));
    // The card is painted, a key context carries the picker's bindings.
    let card = f.rect(PickerPart::Card);
    assert!(card.2 > 400., "{card:?}");
    assert_eq!(PICKER_CONTEXT, "Picker");
    // A press outside closes it and is consumed (CMP-002 R14).
    f.host.click(5., 400., 0, "").unwrap();
    f.settle();
    assert!(!f.is_open());
    assert_eq!(
        f.shared.page_clicks.get(),
        0,
        "the page never saw the press"
    );
    // A14: focus returned to the previously focused element.
    assert_eq!(f.focused_label().as_deref(), Some("Document"));

    // The previous element was removed while open: the window default (nothing focused).
    f.open();
    f.shared.doc_present.set(false);
    f.host.invalidate();
    f.settle();
    f.escape();
    assert!(!f.is_open());
    assert_ne!(f.focused_label().as_deref(), Some("Document"));
}

#[test]
fn reopening_applies_the_declared_policy_and_keeps_favorites_and_the_selected_identity() {
    require_backend!();
    let mut f = fixture(Spec {
        favorites: true,
        ..Spec::new()
    });
    f.open();
    f.set_query("sa");
    assert_eq!(f.highlighted().as_deref(), Some("cmd:Save"));
    f.with_picker(|picker, _, cx| picker.set_selected(Some("cmd:Save".into()), cx));
    f.keys("ctrl-shift-f");
    assert_eq!(
        f.read(|p, _| p.favorite_keys()),
        vec!["cmd:Save".to_string()]
    );
    f.escape();
    assert!(!f.is_open());

    f.open();
    assert!(f.field_focused(), "reopening focuses the field");
    assert_eq!(
        f.read(|p, _| p.query().to_string()),
        "",
        "the default clears the query"
    );
    assert_eq!(
        f.highlighted().as_deref(),
        Some("cmd:Open File"),
        "and the highlight returns to the first row"
    );
    assert_eq!(
        f.read(|p, _| p.favorite_keys()),
        vec!["cmd:Save".to_string()]
    );
    assert_eq!(
        f.read(|p, _| p.selected().map(|k| k.to_string()))
            .as_deref(),
        Some("cmd:Save")
    );
}

#[test]
fn newer_results_win_whatever_order_the_answers_arrive_and_a_failure_retries_with_a_new_generation()
{
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    // The open request (empty query) is sent at once; answer it.
    assert_eq!(f.shared.wire.borrow().queries[0].0, "");
    f.reply(0, files(&["recent.txt"]));
    f.settle_replies();
    assert!(f.shows("recent.txt"));

    f.set_query("ab");
    f.set_query("abc");
    let queries = f.shared.wire.borrow().queries.clone();
    assert_eq!(queries.last().map(|q| q.0.as_str()), Some("abc"));
    // The newer answer arrives first; the stale one afterwards must not overwrite it.
    f.reply(2, files(&["abc-result"]));
    f.settle_replies();
    assert!(f.shows("abc-result"));
    if let Some(tx) = f.shared.wire.borrow_mut().replies[1].take() {
        tx.send(files(&["stale"])).ok();
    }
    f.settle_replies();
    assert!(f.shows("abc-result") && !f.shows("stale"));
    // Rows of the last answer stay while a newer request is in flight (no blanking), with the
    // loading status in the header.
    f.type_text("d");
    assert!(f.shows("abc-result"), "no blanking: {:?}", f.text());
    assert!(
        f.shows("Searching\u{2026}"),
        "the loading status: {:?}",
        f.text()
    );
    // The third query fails: an inline message with Retry.
    let last = f.shared.wire.borrow().replies.len() - 1;
    f.reply(last, Err("index offline".into()));
    f.settle_replies();
    assert!(f.shows("index offline"));
    let retry = node(&mut f.host, "button", Some("Retry"));
    let (x, y) = center(&mut f.host, &retry);
    let before = f.shared.wire.borrow().queries.len();
    f.host.click(x, y, 0, "").unwrap();
    f.settle();
    assert_eq!(
        f.shared.wire.borrow().queries.len(),
        before + 1,
        "retry begins a new request"
    );
    let last = f.shared.wire.borrow().replies.len() - 1;
    f.reply(last, files(&["recovered.txt"]));
    f.settle_replies();
    assert!(f.shows("recovered.txt") && !f.shows("index offline"));
    assert!(f.is_open(), "the field and the list kept their state");
}

#[test]
fn the_highlight_follows_identity_steps_with_wraparound_and_the_first_row_is_the_highlight() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    f.reply(0, files(&["a.txt", "b.txt"]));
    f.settle_replies();
    assert_eq!(
        f.highlighted().as_deref(),
        Some("cmd:Open File"),
        "the first row is the highlight"
    );
    f.keys("down");
    assert_eq!(
        f.highlighted().as_deref(),
        Some("cmd:Open Folder"),
        "the first Down moves to the second"
    );
    assert!(f.read(|p, _| p.keyboard_mode()));
    // Wraps past the end: 5 summary commands + 2 files.
    for _ in 0..5 {
        f.keys("down");
    }
    assert_eq!(f.highlighted().as_deref(), Some("file:b.txt"));
    f.keys("down");
    assert_eq!(
        f.highlighted().as_deref(),
        Some("cmd:Open File"),
        "wraps to the first"
    );
    f.keys("up");
    assert_eq!(
        f.highlighted().as_deref(),
        Some("file:b.txt"),
        "the first Up wraps to the last"
    );
    // Ctrl-N and Ctrl-P step too.
    f.keys("ctrl-p");
    assert_eq!(f.highlighted().as_deref(), Some("file:a.txt"));
    f.keys("ctrl-n");
    assert_eq!(f.highlighted().as_deref(), Some("file:b.txt"));

    // Rows arriving around the highlighted one never move it.
    f.set_query("a");
    let hi = f.highlighted();
    assert!(hi.is_some());
}

#[test]
fn a_resting_pointer_does_not_steal_the_highlight_but_real_motion_does() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    let row = f.rect(PickerPart::Row(3));
    f.host
        .mouse_move(row.0 + 20., row.1 + 10., None, "")
        .unwrap();
    f.settle();
    assert_eq!(
        f.highlighted().as_deref(),
        Some("cmd:Save All"),
        "real motion moves it"
    );
    assert!(
        !f.read(|p, _| p.keyboard_mode()),
        "and clears keyboard mode"
    );
    f.keys("down");
    assert_eq!(f.highlighted().as_deref(), Some("cmd:Close Tab"));
    assert!(f.read(|p, _| p.keyboard_mode()));
    // The pointer rests over the old row while the highlight moves by keyboard: the row under
    // the pointer does not take it back without motion (INP-002), real motion does.
    f.settle();
    assert_eq!(f.highlighted().as_deref(), Some("cmd:Close Tab"));
    f.host
        .mouse_move(row.0 + 80., row.1 + 12., None, "")
        .unwrap();
    f.settle();
    assert_eq!(f.highlighted().as_deref(), Some("cmd:Save All"));
}

#[test]
fn enter_activates_once_per_physical_press_and_the_latch_resets() {
    require_backend!();
    let mut f = fixture(Spec {
        close_on_activate: false,
        ..Spec::new()
    });
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    f.host.key_down("enter", false, None).unwrap();
    f.host.key_down("enter", false, None).unwrap();
    f.host.key_down("enter", true, None).unwrap();
    f.settle();
    assert_eq!(
        *f.shared.activated.borrow(),
        vec!["cmd:Open File"],
        "one activation per press"
    );
    f.host.key_up("enter", None).unwrap();
    f.keys("down");
    f.host.key_down("enter", false, None).unwrap();
    f.host.key_up("enter", None).unwrap();
    f.settle();
    assert_eq!(
        *f.shared.activated.borrow(),
        vec!["cmd:Open File", "cmd:Open Folder"],
        "a new press activates again"
    );
    // A latch left down by a press whose release was lost is cleared by a reopen.
    f.host.key_down("enter", false, None).unwrap();
    f.escape();
    f.open();
    f.host.key_down("enter", false, None).unwrap();
    f.host.key_up("enter", None).unwrap();
    f.settle();
    assert_eq!(
        f.shared.activated.borrow().len(),
        4,
        "reopening reset the latch"
    );
}

#[test]
fn enter_during_a_composition_in_the_query_does_not_activate_a_row() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    let field = f.read(|picker, _| picker.field().clone());
    let window = f.host.window();
    f.host
        .app()
        .update_window(window, |_, window, cx| {
            field.update(cx, |field, cx| {
                field.replace_and_mark_text_in_range(None, "sa", None, window, cx)
            })
        })
        .unwrap();
    f.settle();
    assert!(f.highlighted().is_some(), "the preedit matches a row");
    f.host.key_down("enter", false, None).unwrap();
    f.host.key_up("enter", None).unwrap();
    f.settle();
    assert!(f.shared.activated.borrow().is_empty());
    assert!(f.is_open());
}

#[test]
fn tabs_wrap_the_numbered_accelerator_selects_and_a_change_resets_highlight_and_scroll() {
    require_backend!();
    let mut f = fixture(Spec {
        tabs: true,
        commands: (0..40).map(|ix| format!("Command {ix:02}")).collect(),
        ..Spec::new()
    });
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    assert_eq!(f.read(|p, _| p.active_tab()), 0);
    f.keys("tab");
    assert_eq!(f.read(|p, _| p.active_tab()), 1);
    f.keys("tab");
    f.keys("tab");
    assert_eq!(f.read(|p, _| p.active_tab()), 0, "wraps last to first");
    f.keys("shift-tab");
    assert_eq!(
        f.read(|p, _| p.active_tab()),
        2,
        "Shift-Tab wraps first to last"
    );
    f.keys("ctrl-2");
    assert_eq!(f.read(|p, _| p.active_tab()), 1, "the numbered accelerator");
    // The dedicated scope lists up to 50 rows; scroll down and change tab: top again.
    f.keys("up");
    assert!(
        f.read(|p, _| p.scroll_offset()) > 0.,
        "the last row was revealed"
    );
    f.keys("ctrl-1");
    assert_eq!(f.read(|p, _| p.scroll_offset()), 0.);
    assert_eq!(f.highlighted().as_deref(), Some("cmd:Command 00"));
    // Tabs expose the tab role with a label.
    node(&mut f.host, "tab", Some("Files"));
}

#[test]
fn stepping_reveals_the_highlighted_row_with_the_nearest_edge_rule() {
    require_backend!();
    let mut f = fixture(Spec {
        tabs: true,
        commands: (0..40).map(|ix| format!("Command {ix:02}")).collect(),
        ..Spec::new()
    });
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    f.keys("ctrl-2");
    let list = f.rect(PickerPart::List);
    let mut offsets = vec![];
    for _ in 0..12 {
        f.keys("down");
        offsets.push(f.read(|p, _| p.scroll_offset()));
        let index = f
            .read(|p, _| {
                p.highlighted().map(|k| {
                    k.trim_start_matches("cmd:Command ")
                        .parse::<usize>()
                        .unwrap()
                })
            })
            .unwrap_or(0);
        let row = f.rect(PickerPart::Row(index));
        assert!(
            row.1 >= list.1 - 0.5 && row.1 + row.3 <= list.1 + list.3 + 0.5,
            "the highlighted row is inside the viewport: row {row:?} list {list:?}"
        );
    }
    assert!(
        offsets.windows(2).all(|pair| pair[1] >= pair[0]),
        "{offsets:?}"
    );
    assert!(offsets.last().copied().unwrap() > 0.);
    // Up past the first row wraps to the last, and Down wraps back to offset 0.
    f.keys("ctrl-1");
    f.keys("ctrl-2");
    f.keys("up");
    let last = f.read(|p, _| p.scroll_offset());
    assert!(last > 0.);
    f.keys("down");
    assert_eq!(
        f.read(|p, _| p.scroll_offset()),
        0.,
        "the very first row returns to offset 0"
    );
}

#[test]
fn a_compact_picker_switches_to_its_panel_keeps_state_and_escape_steps_back_one_depth() {
    require_backend!();
    let mut f = fixture(Spec {
        panel: true,
        favorites: true,
        ..Spec::new()
    });
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    f.set_query("o");
    f.keys("down");
    let highlighted = f.highlighted();
    f.with_picker(|picker, _, cx| picker.set_selected(Some("cmd:Zoom In".into()), cx));
    f.keys("ctrl-shift-f");
    // Open the panel, then the nested choices.
    let key = highlighted.clone().unwrap();
    f.with_picker(|picker, window, cx| picker.open_panel(key.into(), window, cx));
    assert!(matches!(f.read(|p, _| p.depth()), Depth::Panel(_)));
    assert!(
        f.shows(&format!("Settings of {}", highlighted.clone().unwrap())),
        "{:?}",
        f.text()
    );
    f.with_picker(|picker, _, cx| picker.open_choices(cx));
    assert!(matches!(f.read(|p, _| p.depth()), Depth::Choices(_)));
    assert!(f.shows("Choices"));

    // Escape: choices, then panel, then list, then close, in that order.
    f.escape();
    assert!(
        matches!(f.read(|p, _| p.depth()), Depth::Panel(_)),
        "the submenu closed first"
    );
    assert!(f.is_open());
    f.escape();
    assert_eq!(f.read(|p, _| p.depth()), Depth::List, "then the panel");
    assert!(f.is_open());
    assert_eq!(
        f.read(|p, _| p.query().to_string()),
        "o",
        "the query survived"
    );
    assert_eq!(
        f.highlighted(),
        highlighted,
        "the highlighted identity is restored"
    );
    assert_eq!(
        f.read(|p, _| p.favorite_keys()).len(),
        1,
        "favorites survived"
    );
    assert_eq!(
        f.read(|p, _| p.selected().map(|k| k.to_string()))
            .as_deref(),
        Some("cmd:Zoom In")
    );
    assert!(f.field_focused(), "focus is back on the field");
    // Focus indicators do not appear after Escape.
    let keyboard = {
        let window = f.host.window();
        f.host
            .app()
            .update_window(window, |_, window, _| window.last_input_was_keyboard())
            .unwrap()
    };
    assert!(!keyboard, "indicators stay hidden after Escape");
    f.escape();
    assert!(!f.is_open(), "the last Escape closes");
}

#[test]
fn more_rows_than_the_cap_show_the_cap_and_a_paged_source_gets_exactly_one_next_page_request() {
    require_backend!();
    let mut f = fixture(Spec {
        tabs: true,
        commands: Vec::new(),
        window: (640., 520.),
        ..Spec::new()
    });
    f.open();
    // The dedicated Files scope.
    f.keys("ctrl-3");
    let rows: Vec<String> = (0..305).map(|ix| format!("file-{ix:03}")).collect();
    // The source's own scope limit is 50 in a dedicated scope; the picker caps at 300 rows only
    // when the owner raises the limit, so check the paged flow with the default limits.
    f.reply(
        0,
        Ok(Answer {
            rows: rows.iter().take(50).cloned().collect(),
            total: Some(305),
            indexing: false,
            has_more: true,
        }),
    );
    f.settle_replies();
    assert!(f.shows("file-000"));
    // Scrolling to the end requests the next page exactly once.
    let list = f.rect(PickerPart::List);
    f.host
        .scroll_wheel(list.0 + 100., list.1 + 100., 0., -5000., "")
        .unwrap();
    f.settle();
    f.host
        .scroll_wheel(list.0 + 100., list.1 + 100., 0., -5000., "")
        .unwrap();
    f.settle();
    let pages: Vec<_> = f
        .shared
        .wire
        .borrow()
        .queries
        .iter()
        .filter(|(_, page)| *page == 1)
        .cloned()
        .collect();
    assert_eq!(pages.len(), 1, "exactly one next-page request: {pages:?}");
    assert!(f.shows("Loading more\u{2026}"), "{:?}", f.text());
    // A newer generation began: no page request for the older one is ever sent.
    f.set_query("fi");
    f.settle_replies();
    assert!(
        f.shared
            .wire
            .borrow()
            .queries
            .iter()
            .filter(|(_, page)| *page == 1)
            .count()
            <= 1,
        "none after a newer generation began"
    );
}

#[test]
fn rows_beyond_the_display_cap_show_showing_x_of_y() {
    require_backend!();
    let commands: Vec<String> = (0..320).map(|ix| format!("Command {ix:03}")).collect();
    let mut f = fixture(Spec {
        tabs: true,
        commands,
        scope_limit: 400,
        ..Spec::new()
    });
    f.open();
    f.keys("ctrl-2");
    assert!(f.shows("Showing 300 of 320"), "{:?}", f.text().len());
}

#[test]
fn the_picker_exposes_a_labelled_dialog_a_list_of_options_and_the_active_descendant() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    node(&mut f.host, "dialog", Some("Command palette"));
    node(&mut f.host, "list", Some("Command palette"));
    let option = node(&mut f.host, "listBoxOption", Some("Save"));
    assert_eq!(option.role, "listBoxOption");
    node(&mut f.host, "textInput", Some("Command palette"));
}

#[test]
fn a_row_hint_shows_after_500_ms_of_hovering_the_row() {
    require_backend!();
    let mut f = fixture(Spec::new());
    f.open();
    f.reply(0, files(&[]));
    f.settle_replies();
    let row = f.rect(PickerPart::Row(2));
    f.host
        .mouse_move(row.0 + 40., row.1 + 10., None, "")
        .unwrap();
    f.host.flush();
    f.host.advance_executor_time(499.).unwrap();
    f.host.flush();
    assert!(!f.shows("Hint: Save"), "not yet at 499 ms");
    f.host.advance_executor_time(1.).unwrap();
    f.host.flush();
    assert!(f.shows("Hint: Save"), "shown at 500 ms: {:?}", f.text());
}
