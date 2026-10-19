//! CMP-006 A7: a command palette over local rows and an asynchronous file source protected by
//! request generations.
//!
//! The "files" source answers after a delay that depends on the query: "ab" answers slowly (300 ms)
//! and "abc" quickly (50 ms), so typing the two queries makes the answers arrive in reverse order;
//! the palette keeps the newer results. A third query ("abcd") fails and shows the error with
//! Retry; pressing Retry begins a new request (a new generation) that succeeds. Local commands are
//! filtered and ranked on every keystroke.
//!
//! Without arguments the example runs offscreen on the production test host with a fake clock and
//! real input and exits 0 when every check holds (1 otherwise); with `--interactive` it opens a
//! window (press Ctrl-K, or the button, to open the palette).

mod controls_common;

use std::{cell::Cell, cell::RefCell, rc::Rc, time::Duration};

use controls_common::{Build, Checks, accessibility, run_window, scripted_host};
use vespertine::{
    AppContext as _, Entity, EntityInputHandler as _, InteractiveElement, IntoElement,
    ParentElement, Role, StatefulInteractiveElement, Styled, div, px, rgb,
};
use vespertine_components::floating::filter_and_rank;
use vespertine_components::picker::Picker;
use vespertine_components::picker::driver::{Fetcher, PickerSources};
use vespertine_components::picker::results::{Paging, Row, SectionSpec};
use vespertine_components::picker::sources::{Answer, SourcePolicy};
use vespertine_test_host::TestHost;

const COMMANDS: [&str; 6] = [
    "Open File",
    "Open Folder",
    "Save",
    "Save All",
    "Close Tab",
    "Toggle Sidebar",
];

struct Parts {
    picker: Entity<Picker>,
    activated: Rc<RefCell<Vec<String>>>,
}

fn parts(
    slot: &Rc<RefCell<Option<Parts>>>,
    window: &mut vespertine::Window,
    cx: &mut vespertine::App,
) -> Entity<Picker> {
    let mut slot = slot.borrow_mut();
    slot.get_or_insert_with(|| {
        let failed_once = Rc::new(Cell::new(false));
        let fetcher: Fetcher<String> = Rc::new(move |request, cx| {
            let failed_once = failed_once.clone();
            let (delay, result) = match request.query.as_str() {
                "" => (0, Ok(vec!["recent.txt"])),
                "ab" => (300, Ok(vec!["ab-old (stale)"])),
                "abc" => (50, Ok(vec!["abc-new"])),
                "abcd" if !failed_once.replace(true) => (20, Err("index offline".to_string())),
                "abcd" => (20, Ok(vec!["abcd-recovered"])),
                _ => (0, Ok(vec![])),
            };
            // A timer on the executor, so the fake clock of the offscreen host drives it.
            let timer = cx.background_executor().timer(Duration::from_millis(delay));
            cx.background_spawn(async move {
                timer.await;
                result.map(|rows| Answer::complete(rows.into_iter().map(String::from).collect()))
            })
        });
        let sources =
            cx.new(|cx| PickerSources::new(vec![(SourcePolicy::name_like(), fetcher)], cx));
        let activated = Rc::new(RefCell::new(Vec::new()));
        let provider_sources = sources.clone();
        let (open, query, close, retry) = (
            sources.clone(),
            sources.clone(),
            sources.clone(),
            sources.clone(),
        );
        let log = activated.clone();
        let picker = cx.new(|cx| {
            Picker::new(
                "palette",
                "Command palette",
                move |query, cx| {
                    let labels: Vec<&str> = COMMANDS.to_vec();
                    let rows = filter_and_rank(&labels, query)
                        .into_iter()
                        .map(|ix| Row::new(format!("cmd:{}", COMMANDS[ix]), COMMANDS[ix]))
                        .collect();
                    let mut local = SectionSpec::new("commands", rows);
                    local.title = Some("Commands".into());
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
            .paging(|_| Paging::default())
            .on_open(move |_, cx| open.update(cx, |sources, cx| sources.open(cx)))
            .on_close(move |_, cx| close.update(cx, |sources, cx| sources.clear(cx)))
            .on_query(move |text, _, cx| {
                query.update(cx, |sources, cx| sources.set_query(text, cx))
            })
            .on_retry(move |_, _, cx| retry.update(cx, |sources, cx| sources.retry(0, cx)))
            .on_activate(move |key, _, _| {
                println!("activated {key}");
                log.borrow_mut().push(key.to_string());
            })
        });
        Parts { picker, activated }
    })
    .picker
    .clone()
}

fn build(slot: &Rc<RefCell<Option<Parts>>>) -> Build {
    let slot = slot.clone();
    Rc::new(move |window, cx| {
        let picker = parts(&slot, window, cx);
        let opener = picker.clone();
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("open-palette")
                    .role(Role::Button)
                    .aria_label("Open palette")
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .w(px(140.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(rgb(0x2B3038))
                    .child("Open palette")
                    .on_click(move |_, window, cx| {
                        opener.update(cx, |picker, cx| picker.open(window, cx))
                    }),
            )
            .child(picker)
            .into_any_element()
    })
}

fn main() {
    let slot = Rc::new(RefCell::new(None));
    if std::env::args().any(|argument| argument == "--interactive") {
        run_window(640.0, 480.0, build(&slot));
        return;
    }
    let mut host = match scripted_host(640.0, 480.0, build(&slot)) {
        Ok(host) => host,
        Err(reason) => {
            println!("picker_palette: no offscreen host here ({reason}); run with --interactive");
            return;
        }
    };
    host.flush();
    let mut checks = Checks::default();
    let (picker, activated) = {
        let parts = slot.borrow();
        let parts = parts
            .as_ref()
            .expect("the parts exist after the first frame");
        (parts.picker.clone(), parts.activated.clone())
    };
    let shows = |host: &TestHost, text: &str| host.painted_text().iter().any(|t| t == text);
    let pass = |host: &mut TestHost, ms: f64| {
        host.advance_executor_time(ms).unwrap();
        host.app().run_until_parked();
        host.flush();
    };
    let settle = |host: &mut TestHost| {
        host.flush();
        for _ in 0..3 {
            let window = host.window();
            host.app()
                .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap();
            host.flush();
        }
        pass(host, 1.);
    };
    let type_query = |host: &mut TestHost, text: &str| {
        let field = picker.clone();
        let window = host.window();
        let text = text.to_owned();
        host.app()
            .update_window(window, |_, window, cx| {
                let field = field.read(cx).field().clone();
                field.update(cx, |field, cx| {
                    field.select_all(window, cx);
                    field.replace_text_in_range(None, &text, window, cx);
                });
            })
            .unwrap();
        settle(host);
    };

    host.click(80., 35., 0, "").unwrap();
    settle(&mut host);
    pass(&mut host, 10.);
    checks.check(
        shows(&host, "recent.txt"),
        "opening starts the default request at once: the recent list shows",
    );

    // Two queries whose answers arrive in reverse order: the newer results stay.
    type_query(&mut host, "ab");
    type_query(&mut host, "abc");
    pass(&mut host, 60.);
    checks.check(
        shows(&host, "abc-new"),
        "the newer query's quick answer is shown",
    );
    pass(&mut host, 400.);
    checks.check(
        shows(&host, "abc-new") && !shows(&host, "ab-old (stale)"),
        "the older, slower answer never overwrites the newer results",
    );

    // A third query fails: error with Retry; Retry succeeds with a new generation.
    type_query(&mut host, "abcd");
    checks.check(
        shows(&host, "abc-new"),
        "the previous rows stay while the next request is in flight",
    );
    pass(&mut host, 30.);
    checks.check(
        shows(&host, "index offline"),
        "the failed answer shows the error",
    );
    let nodes = accessibility(&mut host);
    checks.check(
        nodes
            .iter()
            .any(|node| node.role == "button" && node.label() == "Retry"),
        "the error line offers a Retry button",
    );
    let tree = host.accessibility_tree().unwrap().unwrap();
    let bounds = tree["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry[1]["role"] == "button" && entry[1]["properties"]["label"] == "Retry")
        .map(|entry| entry[1]["properties"]["bounds"].clone())
        .expect("the Retry button");
    let scale = f64::from(host.scale_factor());
    let x = (bounds["x0"].as_f64().unwrap() + bounds["x1"].as_f64().unwrap()) / 2.0 / scale;
    let y = (bounds["y0"].as_f64().unwrap() + bounds["y1"].as_f64().unwrap()) / 2.0 / scale;
    host.click(x, y, 0, "").unwrap();
    settle(&mut host);
    pass(&mut host, 30.);
    checks.check(
        shows(&host, "abcd-recovered") && !shows(&host, "index offline"),
        "Retry begins a new request that succeeds",
    );

    // Local commands filter and rank on every keystroke; Enter activates once.
    type_query(&mut host, "sa");
    checks.check(
        shows(&host, "Save") && shows(&host, "Save All") && !shows(&host, "Close Tab"),
        "local rows are filtered and ranked synchronously",
    );
    host.simulate_keystrokes("down", None).unwrap();
    host.simulate_keystrokes("enter", None).unwrap();
    settle(&mut host);
    checks.check(
        *activated.borrow() == ["cmd:Save All".to_string()],
        "Down then Enter activates the second row once",
    );
    let open = host.app().update(|cx| picker.read(cx).is_open());
    checks.check(!open, "activating closes the palette");
    checks.finish("picker_palette");
}
