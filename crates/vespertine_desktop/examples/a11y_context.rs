//! PLT-013 A1: accessibility context enrichment against a fake accessibility
//! tree.
//!
//! One window matches and is enriched; a duplicate title, a blank title,
//! process id 0 and a window that is no longer active each return
//! screenshot-only. Run with `cargo run -p vespertine_desktop --example
//! a11y_context`. The untrusted-observation serialization used to hand the
//! result to another program is shown at the end. `--live PID TITLE` queries
//! the real AT-SPI bus for a window of another running application.

use std::sync::Arc;
use vespertine_desktop::context::{
    AccessibilityContext, AccessibilitySnapshot, ContextQuery, ContextRequest, ContextUnavailable,
    ObservedWindow, SystemClock,
    fake::{FakeBus, FakeNodeSpec},
    serialize_untrusted_observations,
};

fn describe(label: &str, result: &Result<AccessibilityContext, ContextUnavailable>) {
    match result {
        Ok(context) => {
            println!(
                "{label}: enriched (application {:?}, {} bytes, truncated: {})",
                context.application_name,
                context.snapshot.content.len(),
                context.snapshot.truncated
            );
            for line in context.snapshot.content.lines() {
                println!("    {line}");
            }
        }
        Err(reason) => println!(
            "{label}: screenshot-only ({reason}); snapshot = {:?}",
            AccessibilitySnapshot::empty()
        ),
    }
}

fn query(bus: &FakeBus, pid: u32, title: &str) -> Result<AccessibilityContext, ContextUnavailable> {
    smol::block_on(ContextQuery::with_services(
        Arc::new(bus.clone()),
        Arc::new(SystemClock),
        ContextRequest::new(pid, title),
    ))
}

fn main() {
    // `--live PID TITLE` queries the real AT-SPI bus instead of the fake tree.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, pid, title] = arguments.as_slice()
        && flag == "--live"
    {
        let pid: u32 = pid.parse().expect("PID must be a number");
        let result = smol::block_on(ContextQuery::new(ContextRequest::new(pid, title.as_str())));
        describe("live window", &result);
        return;
    }
    let bus = FakeBus::new(Arc::new(SystemClock));
    let editor = bus.add_application(":1.10", 4242, "Editor");
    let main_window = bus.add_window(editor, "notes.txt - Editor", true);
    let toolbar = bus.add_node(main_window, FakeNodeSpec::new("tool bar"));
    bus.add_node(toolbar, FakeNodeSpec::new("push button").name("Save"));
    bus.add_node(toolbar, FakeNodeSpec::new("push button").name("Open"));
    bus.add_node(
        main_window,
        FakeNodeSpec::new("text")
            .name("Document")
            .text("Remember to buy\n  milk and bread."),
    );
    bus.add_node(
        main_window,
        FakeNodeSpec::new("password text")
            .name("Vault key")
            .text("never read")
            .password(),
    );

    let browser = bus.add_application(":1.11", 5151, "Browser");
    bus.add_window(browser, "Inbox", true);
    bus.add_window(browser, "Inbox", true);

    let terminal = bus.add_application(":1.12", 6161, "Terminal");
    bus.add_window(terminal, "shell", false);

    let matching = query(&bus, 4242, "notes.txt - Editor");
    describe("single matching window", &matching);
    describe("duplicate title", &query(&bus, 5151, "Inbox"));
    describe("blank title", &query(&bus, 4242, "   "));
    describe("process id 0", &query(&bus, 0, "notes.txt - Editor"));
    describe("window no longer active", &query(&bus, 6161, "shell"));

    if let Ok(context) = matching {
        println!("\nSerialized as untrusted observed content:\n");
        println!(
            "{}",
            serialize_untrusted_observations(
                &[ObservedWindow {
                    application_name: context.application_name,
                    identifier: None,
                    window_title: Some("notes.txt - Editor".to_owned()),
                    image_reference: "image-1".to_owned(),
                    snapshot: context.snapshot,
                }],
                None,
            )
        );
    }
}
