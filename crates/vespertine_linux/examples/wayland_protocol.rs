//! PLT-001 A12: the Wayland protocol traffic of a window with layered composition.
//!
//! The example runs itself as a child process with `WAYLAND_DEBUG=1`, plays a script (setup, a
//! popover, a tooltip alone, hiding and showing the native child) and checks the requests the
//! window sent to the compositor:
//!
//! * the overlay is a *synchronized* subsurface of the window surface and starts with an empty
//!   input region, so pointer events reach the surfaces below;
//! * the native child is a *desynchronized* subsurface of the window surface;
//! * the overlay is stacked above the native child;
//! * a popover (a hitbox on the overlay while it captures input) gives the overlay exactly the
//!   popover's rectangle as input region, and a tooltip alone gives it an empty one;
//! * hiding the native child unmaps it by destroying its `wl_subsurface`, and showing it creates
//!   a new one that is again desynchronized.
//!
//! It exits 0 when every check passes. It needs a Wayland session.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

mod native_support;

use std::{cell::Cell, cell::RefCell, process::Command, rc::Rc};

use native_support::{Seen, SharedSeen, launch, rect};
use vespertine::{
    AnyView, Context, IntoElement, ParentElement, Render, Styled, Window, deferred, div, point,
    prelude::*, px, rgb,
};
use vespertine_linux::SolidChild;

const CHILD: (f32, f32, f32, f32) = (60.0, 60.0, 300.0, 180.0);
const POPOVER: (f32, f32, f32, f32) = (240.0, 120.0, 200.0, 100.0);

fn mark(name: &str) {
    eprintln!("PROBE::{name}");
}

struct Tip(Rc<Cell<bool>>);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.set(true);
        div().w(px(80.0)).h(px(24.0)).bg(rgb(0x202020)).child("tip")
    }
}

struct Probe {
    child: SolidChild,
    child_visible: bool,
    popover: bool,
    tip_shown: Rc<Cell<bool>>,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.child.prepare(window);
        let bounds = rect(CHILD.0, CHILD.1, CHILD.2, CHILD.3);
        self.child.present(window, bounds, 0x2060c0, 0x2060c0);
        let surface = self.child.surface().cloned();
        let visible = self.child_visible;
        let tip_shown = self.tip_shown.clone();
        let mut root = div()
            .relative()
            .size_full()
            .bg(rgb(0xf3f4f6))
            .child(
                div()
                    .absolute()
                    .left(px(CHILD.0))
                    .top(px(CHILD.1))
                    .w(px(CHILD.2))
                    .h(px(CHILD.3))
                    .child(
                        vespertine::canvas(
                            move |bounds, window, _| {
                                if let Some(surface) = &surface {
                                    surface.position(window, bounds, visible);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    ),
            )
            .child(
                div()
                    .id("target")
                    .absolute()
                    .left(px(CHILD.0))
                    .top(px(CHILD.1))
                    .w(px(CHILD.2))
                    .h(px(CHILD.3))
                    .tooltip(move |_, cx| {
                        let shown = tip_shown.clone();
                        AnyView::from(cx.new(|_| Tip(shown)))
                    }),
            );
        if self.popover {
            root = root.child(deferred(
                div()
                    .id("popover")
                    .absolute()
                    .left(px(POPOVER.0))
                    .top(px(POPOVER.1))
                    .w(px(POPOVER.2))
                    .h(px(POPOVER.3))
                    .bg(rgb(0xffffff))
                    .occlude()
                    .child("Popover"),
            ));
        }
        root
    }
}

fn run_child() {
    mark("start");
    let seen: SharedSeen = Rc::new(RefCell::new(Seen::default()));
    let _ = seen;
    launch(
        500.0,
        320.0,
        |_, cx| {
            cx.new(|_| Probe {
                child: SolidChild::new(),
                child_visible: true,
                popover: false,
                tip_shown: Rc::new(Cell::new(false)),
            })
        },
        |mut script| async move {
            script.sleep(500).await;
            mark("setup-done");
            mark("popover");
            script.set(|view, _| view.popover = true);
            script.sleep(500).await;
            mark("popover-done");
            mark("tooltip");
            script.set(|view, _| view.popover = false);
            script.sleep(300).await;
            script.mouse_move(point(px(100.0), px(100.0)));
            let shown = script.until(|view| view.tip_shown.get()).await;
            script.check(shown, "the tooltip shows");
            script.sleep(300).await;
            mark("tooltip-done");
            mark("hide");
            script.set(|view, _| view.child_visible = false);
            script.sleep(500).await;
            mark("hide-done");
            mark("show");
            script.set(|view, _| view.child_visible = true);
            script.sleep(500).await;
            mark("show-done");
            script.failures
        },
    );
}

/// One request the client sent, from a `WAYLAND_DEBUG` line such as
/// `[16:10:28.406532]  -> wl_subsurface#122.set_sync()`.
#[derive(Clone, Debug)]
struct Request {
    object: String,
    method: String,
    args: String,
}

enum Line {
    Mark(String),
    Request(Request),
}

fn parse(line: &str) -> Option<Line> {
    if let Some(name) = line.strip_prefix("PROBE::") {
        return Some(Line::Mark(name.trim().to_owned()));
    }
    let (prefix, request) = line.split_once(" -> ")?;
    // Requests of other event queues carry a queue name; the window's own requests do not.
    if prefix.contains('{') {
        return None;
    }
    let (object, rest) = request.split_once('.')?;
    let (method, args) = rest.split_once('(')?;
    Some(Line::Request(Request {
        object: object.to_owned(),
        method: method.to_owned(),
        args: args.trim_end_matches(')').to_owned(),
    }))
}

struct Check {
    failures: Vec<String>,
}

impl Check {
    fn that(&mut self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_owned());
        }
    }
}

/// The input region the surface `surface` was last given in `requests`: `None` when it was
/// never set, `Some(rectangles)` otherwise (an empty list is an empty region).
fn input_region(requests: &[Request], surface: &str) -> Option<Vec<String>> {
    let mut result = None;
    for (index, request) in requests.iter().enumerate() {
        if request.object != surface || request.method != "set_input_region" {
            continue;
        }
        let region = request.args.trim().to_owned();
        if region == "nil" {
            result = Some(vec!["nil".to_owned()]);
            continue;
        }
        let rectangles = requests[..index]
            .iter()
            .filter(|earlier| earlier.object == region && earlier.method == "add")
            .map(|earlier| earlier.args.clone())
            .collect();
        result = Some(rectangles);
    }
    result
}

fn main() {
    if std::env::var_os("PROBE_CHILD").is_some() {
        run_child();
        return;
    }
    let executable = std::env::current_exe().expect("the example's path");
    let output = Command::new(executable)
        .env("PROBE_CHILD", "1")
        .env("WAYLAND_DEBUG", "1")
        .output()
        .expect("run the child");
    print!("{}", String::from_utf8_lossy(&output.stdout));
    let trace = String::from_utf8_lossy(&output.stderr);

    let mut segments: Vec<(String, Vec<Request>)> = vec![("before".to_owned(), Vec::new())];
    for line in trace.lines() {
        match parse(line) {
            Some(Line::Mark(name)) => segments.push((name, Vec::new())),
            Some(Line::Request(request)) => segments.last_mut().expect("a segment").1.push(request),
            None => {}
        }
    }
    let segment = |name: &str| -> Vec<Request> {
        segments
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, requests)| requests.clone())
            .unwrap_or_default()
    };
    let mut check = Check {
        failures: Vec::new(),
    };
    check.that(output.status.success(), "the scripted window run completed");

    let setup = segment("start");
    let subsurfaces: Vec<&Request> = setup
        .iter()
        .filter(|request| request.method == "get_subsurface")
        .collect();
    check.that(
        subsurfaces.len() == 2,
        "the window creates two subsurfaces: the overlay and the native child",
    );
    // get_subsurface(new id wl_subsurface#N, wl_surface#S, wl_surface#P)
    let fields = |request: &Request| -> (String, String, String) {
        let parts: Vec<&str> = request.args.split(", ").collect();
        (
            parts[0].trim_start_matches("new id ").to_owned(),
            parts[1].to_owned(),
            parts[2].to_owned(),
        )
    };
    if subsurfaces.len() == 2 {
        let (overlay_sub, overlay_surface, overlay_parent) = fields(subsurfaces[0]);
        let (native_sub, native_surface, native_parent) = fields(subsurfaces[1]);
        check.that(
            overlay_parent == native_parent,
            "both are subsurfaces of the window surface",
        );
        let mode_of = |requests: &[Request], subsurface: &str| {
            requests
                .iter()
                .rev()
                .find(|request| {
                    request.object == subsurface
                        && (request.method == "set_sync" || request.method == "set_desync")
                })
                .map(|request| request.method.clone())
        };
        check.that(
            mode_of(&setup, &overlay_sub).as_deref() == Some("set_sync"),
            "the overlay subsurface is synchronized",
        );
        check.that(
            mode_of(&setup, &native_sub).as_deref() == Some("set_desync"),
            "the native subsurface is desynchronized",
        );
        check.that(
            setup.iter().any(|request| {
                request.object == overlay_sub
                    && request.method == "place_above"
                    && request.args == native_surface
            }),
            "the overlay is stacked above the native child",
        );
        check.that(
            input_region(&setup, &overlay_surface) == Some(Vec::new()),
            "an overlay without hitboxes has an empty input region",
        );

        let all: Vec<Request> = segments
            .iter()
            .flat_map(|(_, requests)| requests.clone())
            .collect();
        // Every request up to and including the segment `name`.
        let through = |name: &str| -> Vec<Request> {
            let mut requests = Vec::new();
            for (candidate, segment) in &segments {
                requests.extend(segment.iter().cloned());
                if candidate == name {
                    break;
                }
            }
            requests
        };
        let popover = segment("popover");
        let expected = format!(
            "{}, {}, {}, {}",
            POPOVER.0 as i32, POPOVER.1 as i32, POPOVER.2 as i32, POPOVER.3 as i32
        );
        check.that(
            popover.iter().any(|request| {
                request.object == overlay_surface && request.method == "set_input_region"
            }) && input_region(&through("popover"), &overlay_surface) == Some(vec![expected]),
            "a popover gives the overlay exactly the popover's rectangle as input region",
        );
        let tooltip = segment("tooltip");
        check.that(
            tooltip.iter().any(|request| {
                request.object == overlay_surface && request.method == "set_input_region"
            }) && input_region(&through("tooltip"), &overlay_surface) == Some(Vec::new()),
            "a tooltip alone leaves the overlay's input region empty",
        );
        check.that(
            !all.iter().any(|request| {
                request.object == overlay_surface
                    && request.method == "set_input_region"
                    && request.args.trim() == "nil"
            }),
            "the overlay never accepts input on its whole surface",
        );

        let hide = segment("hide");
        check.that(
            hide.iter()
                .any(|request| request.object == native_sub && request.method == "destroy"),
            "hiding the native child unmaps it by destroying its wl_subsurface",
        );
        let show = segment("show");
        let recreated = show.iter().find(|request| {
            request.method == "get_subsurface" && request.args.contains(&native_surface)
        });
        check.that(
            recreated.is_some(),
            "showing the native child creates its wl_subsurface again",
        );
        if let Some(recreated) = recreated {
            let (new_sub, _, _) = fields(recreated);
            check.that(
                mode_of(&show, &new_sub).as_deref() == Some("set_desync"),
                "the recreated native subsurface is desynchronized",
            );
        }
    }
    if !check.failures.is_empty() {
        eprintln!("failed checks: {:?}", check.failures);
        std::process::exit(1);
    }
}
