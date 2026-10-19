//! Scaffolding shared by the CMP-001 examples: a shell view that binds Tab traversal and renders a
//! caller-built element tree, a scripted offscreen host (the production test host) and a live
//! window, plus accessibility-tree rows for printing.
//!
//! Each example runs its keyboard script offscreen by default (it exits 0 when every check holds
//! and 1 otherwise) and opens an interactive window with `--interactive`.

#![allow(dead_code)]

use std::rc::Rc;

use serde_json::Value;
use vespertine::{
    AnyElement, App, AppContext as _, Bounds, Context, FocusHandle, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, Styled, Window, WindowBounds, WindowOptions,
    actions, div, px, rgb, size,
};
use vespertine_platform::application;
use vespertine_test_host::{HostOptions, TestHost};

actions!(controls_example, [Tab, TabPrev]);

/// Builds the content of a frame.
pub type Build = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// The root view: focus root, Tab and Shift-Tab traversal, a dark backdrop.
pub struct Shell {
    focus: FocusHandle,
    build: Build,
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("shell")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Tab, window, cx| window.focus_next(cx)))
            .on_action(cx.listener(|_, _: &TabPrev, window, cx| window.focus_prev(cx)))
            .size_full()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .child((self.build)(window, cx))
    }
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", Tab, None),
        KeyBinding::new("shift-tab", TabPrev, None),
    ]);
}

fn shell(build: Build, window: &mut Window, cx: &mut App) -> vespertine::Entity<Shell> {
    let view = cx.new(|cx| Shell {
        focus: cx.focus_handle(),
        build,
    });
    let focus = view.read(cx).focus.clone();
    window.focus(&focus, cx);
    view
}

/// Opens the interactive window and runs until it is closed.
pub fn run_window(width: f32, height: f32, build: Build) {
    application().run(move |cx: &mut App| {
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| shell(build, window, cx),
        )
        .expect("open the window");
        cx.activate(true);
    });
}

/// An offscreen host (production layout, hit testing, dispatch and GPU renderer) showing the
/// shell around `build`. `None` with the reason when no GPU test backend exists.
pub fn scripted_host(width: f64, height: f64, build: Build) -> Result<TestHost, String> {
    if !vespertine_test_host::is_available() {
        return Err(vespertine_test_host::unavailable_reason().unwrap_or_default());
    }
    let mut host = TestHost::with_view(HostOptions::with_size(width, height), move |window, cx| {
        shell(build, window, cx)
    })
    .map_err(|error| error.to_string())?;
    host.app().update(bind_keys);
    Ok(host)
}

/// Collects pass/fail lines of a script.
#[derive(Default)]
pub struct Checks {
    failures: Vec<String>,
}

impl Checks {
    /// Records `what` as passed or failed and prints it.
    pub fn check(&mut self, ok: bool, what: impl AsRef<str>) {
        let what = what.as_ref();
        println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_owned());
        }
    }

    /// Prints the summary and exits the process with 0 (all passed) or 1.
    pub fn finish(self, name: &str) -> ! {
        if self.failures.is_empty() {
            println!("{name}: all checks passed");
            std::process::exit(0);
        }
        println!("{name}: {} check(s) failed", self.failures.len());
        std::process::exit(1);
    }
}

/// One accessibility node, as the assistive technology sees it.
#[derive(Clone, Debug)]
pub struct Node {
    pub role: String,
    pub flags: u32,
    pub properties: Value,
}

impl Node {
    pub fn label(&self) -> &str {
        self.properties
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    pub fn disabled(&self) -> bool {
        self.flags & (1 << 9) != 0
    }

    pub fn busy(&self) -> bool {
        self.flags & (1 << 4) != 0
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.properties.get(key).and_then(Value::as_f64)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.properties.get(key).and_then(Value::as_str)
    }

    /// One printable line of role, label and state.
    pub fn describe(&self) -> String {
        let mut line = format!("role={} label={:?}", self.role, self.label());
        if let Some(toggled) = self.text("toggled") {
            line.push_str(&format!(" checked={toggled}"));
        }
        if let Some(value) = self.number("numericValue") {
            line.push_str(&format!(" value={value}"));
        }
        if let Some(min) = self.number("minNumericValue") {
            line.push_str(&format!(" min={min}"));
        }
        if let Some(max) = self.number("maxNumericValue") {
            line.push_str(&format!(" max={max}"));
        }
        if let Some(step) = self.number("numericValueStep") {
            line.push_str(&format!(" step={step}"));
        }
        if let Some(text) = self.text("value") {
            line.push_str(&format!(" text={text:?}"));
        }
        line.push_str(&format!(
            " disabled={} busy={}",
            self.disabled(),
            self.busy()
        ));
        line
    }
}

/// The accessibility nodes of the last frame (every node that has a role other than the window).
pub fn accessibility(host: &mut TestHost) -> Vec<Node> {
    let tree = host
        .accessibility_tree()
        .expect("the accessibility tree is built")
        .expect("accessibility is enabled");
    tree["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| Node {
            role: entry[1]["role"].as_str().unwrap_or_default().to_owned(),
            flags: entry[1]["flags"].as_u64().unwrap_or(0) as u32,
            properties: entry[1]["properties"].clone(),
        })
        .filter(|node| node.role != "window")
        .collect()
}

/// The node with `role` and `label`.
pub fn find(nodes: &[Node], role: &str, label: &str) -> Node {
    nodes
        .iter()
        .find(|node| node.role == role && node.label() == label)
        .cloned()
        .unwrap_or_else(|| panic!("no {role} named {label:?}"))
}
